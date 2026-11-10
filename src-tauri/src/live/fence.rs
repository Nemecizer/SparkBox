//! The write fence (P1.1): which nodes and classes of change the test build (the debug build) may
//! make, until when. The owner opens it in the release build with Touch ID, and the debug build
//! checks it before every change. The release build's own changes need Admin mode, not a fence
//! (exit.rs `BUILD_NEEDS_FENCE`). A release-build record from before that still parses, and
//! changes nothing.
//!
//! It lives in ~/Library/Application Support/SparkBox Fence/, shared by both builds whatever
//! their own identifiers: `fence.json` (the record and its signature) and `fence-key.pub` (the
//! signing key's public half and its id). Each build keeps, in its own config folder, the key it
//! trusts (`fence-trust.json`), the tamper latch (`fence-failed.json`) and the newest record it
//! has accepted and closed (`fence-seen.json`).
//!
//! The record is signed with ECDSA P-256 over SHA-256: the signed bytes are the record without
//! `sig` as serde_json writes it, and the signature is checked with the key in fence-trust.json,
//! never the key file next to the record. A record that fails its check sets the latch: every
//! change is refused until a record with a new id, opened after the failure, replaces it. Putting
//! the old file back doesn't clear it.
//!
//! A well-signed record can still be an old one put back: a fence the owner closed, or a wider
//! one replaced since by a narrower one. Closing writes `fence-closed.json` next to the record
//! (so the other build learns of it too) and this build's `fence-seen.json`; a record created
//! before the last close, or before the newest record this build accepted, fails its check
//! and sets the latch.
//!
//! The key is a software P-256 key whose private half is kept in a Keychain item of its own
//! (service "com.nemecj.sparkbox.fence", account "signing-key"), outside the vault the debug
//! build shares, read only after a fresh Touch ID and only by release-build code.
//! What this catches is mistakes and edits to the fence file; someone who changes this check in
//! the debug build's source isn't stopped by it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use base64::Engine;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
// Signing happens only in the release build (and in tests).
#[cfg(any(test, not(debug_assertions)))]
use p256::ecdsa::{signature::Signer, SigningKey};
#[cfg(any(test, not(debug_assertions)))]
use p256::pkcs8::EncodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::fence_class::FenceClass;

/// The longest a fence may stay open.
pub const MAX_MS: u64 = 8 * 3_600_000;
pub const RECORD: &str = "fence.json";
pub const KEY_FILE: &str = "fence-key.pub";
pub const TRUST: &str = "fence-trust.json";
pub const LATCH: &str = "fence-failed.json";
pub const SEEN: &str = "fence-seen.json";
pub const CLOSED: &str = "fence-closed.json";
/// Where the release build keeps the signing key: a Keychain item of its own, never in the vault.
#[cfg(not(debug_assertions))]
pub const KEYCHAIN_SERVICE: &str = "com.nemecj.sparkbox.fence";
#[cfg(not(debug_assertions))]
pub const KEYCHAIN_ACCOUNT: &str = "signing-key";
/// The Tauri event sent when the fence opens, closes or runs out.
pub const EVENT: &str = "fence://changed";
/// How far ahead of this Mac's clock a record's creation may be (clock adjustments).
const SKEW_MS: u64 = 5 * 60_000;

/// ~/Library/Application Support/SparkBox Fence (the same for both builds).
pub fn default_dir() -> PathBuf {
    super::sshconfig::home().join("Library/Application Support/SparkBox Fence")
}

/// The fence record, fields in their signed order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub v: u32,
    pub id: String,
    /// release | dev: the build it's for.
    pub instance: String,
    pub nodes: Vec<String>,
    pub classes: Vec<FenceClass>,
    /// Per class, the only items it covers (unit names, containers, packages).
    pub only: BTreeMap<FenceClass, Vec<String>>,
    pub created_ms: u64,
    pub expires_ms: u64,
    /// The Mac's ComputerName where it was opened.
    pub mac: String,
    pub key_id: String,
    pub note: String,
    /// Base64 DER ECDSA P-256 SHA-256 over `signed_bytes`.
    pub sig: String,
}

/// The record without its signature, borrowed: what is signed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    v: u32,
    id: &'a str,
    instance: &'a str,
    nodes: &'a [String],
    classes: &'a [FenceClass],
    only: &'a BTreeMap<FenceClass, Vec<String>>,
    created_ms: u64,
    expires_ms: u64,
    mac: &'a str,
    key_id: &'a str,
    note: &'a str,
}

impl Record {
    pub fn signed_bytes(&self) -> Vec<u8> {
        let body = Body {
            v: self.v,
            id: &self.id,
            instance: &self.instance,
            nodes: &self.nodes,
            classes: &self.classes,
            only: &self.only,
            created_ms: self.created_ms,
            expires_ms: self.expires_ms,
            mac: &self.mac,
            key_id: &self.key_id,
            note: &self.note,
        };
        serde_json::to_vec(&body).unwrap_or_default()
    }

    /// "cell2 · 6 classes · 3 h", for the banner and the Touch ID sheet.
    pub fn summary(&self, now_ms: u64) -> String {
        let n = self.classes.len();
        format!("{} · {n} class{} · {}", self.nodes.join(", "), if n == 1 { "" } else { "es" }, left(self.expires_ms.saturating_sub(now_ms)))
    }
}

/// "2 h 41 min", "35 min", "under a minute".
pub fn left(ms: u64) -> String {
    let min = ms / 60_000;
    match (min / 60, min % 60) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// fence-key.pub: the signing key's public half.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyFile {
    pub key_id: String,
    /// Base64 DER SubjectPublicKeyInfo.
    pub public_key: String,
    /// keychain | secureEnclave
    pub kind: String,
}

/// fence-trust.json: the key this build accepts fences from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trust {
    pub key_id: String,
    pub public_key: String,
}

/// fence-failed.json: a record failed its check here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Latch {
    pub id: String,
    pub sha256: String,
    pub seen_ms: u64,
}

/// fence-seen.json: the newest record this build accepted, and the last close it knows of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Seen {
    pub last_id: String,
    pub last_created_ms: u64,
    pub closed_id: String,
    pub closed_ms: u64,
}

/// fence-closed.json, next to the record: the fence closed last, and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Closed {
    pub id: String,
    pub closed_ms: u64,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// The first 16 hex characters of the SHA-256 of the key's DER encoding.
pub fn key_id(public_der: &[u8]) -> String {
    sha_hex(public_der)[..16].to_string()
}

/// The public half as fence-key.pub and fence-trust.json hold it.
#[cfg(any(test, not(debug_assertions)))]
pub fn public_of(key: &SigningKey) -> Result<(String, String), String> {
    let der = VerifyingKey::from(key).to_public_key_der().map_err(|e| e.to_string())?;
    Ok((key_id(der.as_bytes()), b64().encode(der.as_bytes())))
}

/// A new software key from the OS random source.
#[cfg(any(test, not(debug_assertions)))]
pub fn new_key() -> Result<SigningKey, String> {
    loop {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        // A scalar of 0 or past the curve order is refused; draw again (vanishingly rare).
        if let Ok(k) = SigningKey::from_slice(&bytes) {
            return Ok(k);
        }
    }
}

#[cfg(any(test, not(debug_assertions)))]
pub fn key_to_text(key: &SigningKey) -> String {
    b64().encode(key.to_bytes())
}

#[cfg(any(test, not(debug_assertions)))]
pub fn key_from_text(text: &str) -> Result<SigningKey, String> {
    let bytes = b64().decode(text.trim()).map_err(|_| "The fence key in the Keychain is unreadable.".to_string())?;
    SigningKey::from_slice(&bytes).map_err(|_| "The fence key in the Keychain is unreadable.".to_string())
}

/// Signs `r` (its `sig` is replaced).
#[cfg(any(test, not(debug_assertions)))]
pub fn sign(r: &mut Record, key: &SigningKey) {
    let sig: Signature = key.sign(&r.signed_bytes());
    r.sig = b64().encode(sig.to_der().as_bytes());
}

/// Whether `r`'s signature is good for the trusted key.
pub fn verify(r: &Record, trust: &Trust) -> bool {
    let Ok(der) = b64().decode(&trust.public_key) else { return false };
    if key_id(&der) != trust.key_id {
        return false;
    }
    let Ok(key) = VerifyingKey::from_public_key_der(&der) else { return false };
    let Ok(sig) = b64().decode(&r.sig).map_err(|_| ()).and_then(|s| Signature::from_der(&s).map_err(|_| ())) else { return false };
    key.verify(&r.signed_bytes(), &sig).is_ok()
}

/// The record's own rules: what a well-formed fence is, whoever signed it.
pub fn rules(r: &Record, trusted_key: &str) -> Result<(), String> {
    if r.v != 1 {
        return Err(format!("unknown fence version {}", r.v));
    }
    if r.key_id != trusted_key {
        return Err("it was signed with a key this build doesn't trust".into());
    }
    if !matches!(r.instance.as_str(), "release" | "dev") {
        return Err("it names no SparkBox build".into());
    }
    if r.expires_ms <= r.created_ms || r.expires_ms - r.created_ms > MAX_MS {
        return Err("a fence lasts at most 8 hours".into());
    }
    if r.nodes.is_empty() || r.classes.is_empty() {
        return Err("it names no nodes or no classes".into());
    }
    if r.classes.contains(&FenceClass::Other) {
        return Err("`other` can't be allowed on its own".into());
    }
    if r.classes.contains(&FenceClass::Any) && r.instance != "release" {
        return Err("`any` is only for the release build".into());
    }
    if let Some(c) = r.only.keys().find(|c| !r.classes.contains(c)) {
        return Err(format!("an `only` list for {}, which it doesn't allow", c.id()));
    }
    Ok(())
}

/// What the owner asks for in Settings › Fence (`instance` is the build it's for: the app opens
/// fences only for the test build, exit.rs `for_the_test_build`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Spec {
    pub nodes: Vec<String>,
    pub classes: Vec<FenceClass>,
    pub only: BTreeMap<FenceClass, Vec<String>>,
    /// How long it stays open, in minutes (at most 8 hours).
    pub minutes: u64,
    /// release | dev
    pub instance: String,
    pub note: String,
}

/// Checks `spec` before it's signed: real nodes, 8 hours at most, `any` only for the release build.
pub fn validate(spec: &Spec, inventory: &[String]) -> Result<(), String> {
    if spec.nodes.is_empty() {
        return Err("Choose at least one node.".into());
    }
    if let Some(n) = spec.nodes.iter().find(|n| !inventory.contains(n)) {
        return Err(format!("{n} isn't one of your nodes."));
    }
    if spec.classes.is_empty() {
        return Err("Choose at least one class of change.".into());
    }
    if spec.classes.contains(&FenceClass::Other) {
        return Err("`other` is allowed only through `any`.".into());
    }
    if !matches!(spec.instance.as_str(), "release" | "dev") {
        return Err("Choose the release or the dev build.".into());
    }
    if spec.classes.contains(&FenceClass::Any) && spec.instance != "release" {
        return Err("`any` is only for the release build: the dev build gets named classes.".into());
    }
    if spec.minutes == 0 || spec.minutes * 60_000 > MAX_MS {
        return Err("A fence lasts from 1 minute to 8 hours.".into());
    }
    if let Some(c) = spec.only.keys().find(|c| !spec.classes.contains(c)) {
        return Err(format!("There's an `only` list for {}, which the fence doesn't allow.", c.id()));
    }
    let item_ok = |s: &String| !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || "@._:+-".contains(c));
    if spec.only.values().flatten().any(|s| !item_ok(s)) {
        return Err("An `only` item has characters a unit, container or package name doesn't.".into());
    }
    Ok(())
}

/// A new record for `spec`, signed with `key`.
#[cfg(any(test, not(debug_assertions)))]
pub fn record_for(spec: &Spec, key: &SigningKey, now_ms: u64, mac: &str) -> Result<Record, String> {
    let (key_id, _) = public_of(key)?;
    let mut rand = [0u8; 4];
    getrandom::fill(&mut rand).map_err(|e| e.to_string())?;
    let only = spec.only.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (*k, v.clone())).collect();
    let mut r = Record {
        v: 1,
        id: format!("fence-{now_ms}-{}", rand.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        instance: spec.instance.clone(),
        nodes: spec.nodes.clone(),
        classes: spec.classes.clone(),
        only,
        created_ms: now_ms,
        expires_ms: now_ms + spec.minutes * 60_000,
        mac: mac.to_string(),
        key_id,
        note: spec.note.trim().to_string(),
        sig: String::new(),
    };
    sign(&mut r, key);
    Ok(r)
}

/// fence.json's state for this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    None,
    Valid,
    Expired,
    Failed,
    Untrusted,
}

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub state: State,
    pub record: Option<Record>,
    /// Why it failed its check, or why the key isn't trusted.
    pub why: Option<String>,
}

impl Checked {
    fn of(state: State) -> Checked {
        Checked { state, record: None, why: None }
    }
}

/// What a record's bytes came to, kept while the file and the trusted key stay the same.
#[derive(Clone)]
struct Cached {
    stamp: (Option<SystemTime>, u64, String),
    trusted: String,
    outcome: Result<Record, (String, String)>,
}

/// Checks fence.json before every change, cached by the file's (mtime, length, SHA-256).
#[derive(Default)]
pub struct Verifier {
    cache: Mutex<Option<Cached>>,
}

impl Verifier {
    /// The fence's state for a build whose own config folder is `config`. Expiry is checked
    /// against `now_ms` every time; the signature only when the file or the trusted key changed.
    pub fn check(&self, dir: &Path, config: &Path, now_ms: u64) -> Checked {
        let bytes = match std::fs::read(dir.join(RECORD)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Checked::of(State::None),
            Err(e) => return Checked { why: Some(format!("fence.json can't be read: {e}")), ..Checked::of(State::Failed) },
        };
        let Some(trust) = load::<Trust>(&config.join(TRUST)) else {
            return Checked { why: Some("This build doesn't trust the fence key yet.".into()), ..Checked::of(State::Untrusted) };
        };
        if let Some(k) = load::<KeyFile>(&dir.join(KEY_FILE)) {
            if k.key_id != trust.key_id {
                return Checked { why: Some(format!("The fence key changed ({} here, {} trusted).", k.key_id, trust.key_id)), ..Checked::of(State::Untrusted) };
            }
        }
        let meta = std::fs::metadata(dir.join(RECORD)).ok();
        let stamp = (meta.as_ref().and_then(|m| m.modified().ok()), bytes.len() as u64, sha_hex(&bytes));
        let outcome = {
            let mut cache = self.cache.lock().unwrap();
            match cache.as_ref().filter(|c| c.stamp == stamp && c.trusted == trust.key_id) {
                Some(c) => c.outcome.clone(),
                None => {
                    let outcome = judge(&bytes, &trust);
                    *cache = Some(Cached { stamp: stamp.clone(), trusted: trust.key_id.clone(), outcome: outcome.clone() });
                    outcome
                }
            }
        };
        let latch_path = config.join(LATCH);
        let record = match outcome {
            Ok(r) => r,
            Err((id, why)) => {
                // Remember it, so putting the old file back doesn't open the fence again.
                if load::<Latch>(&latch_path).is_none_or(|l| l.sha256 != stamp.2) {
                    let _ = crate::util::save_json(&latch_path, &Latch { id, sha256: stamp.2.clone(), seen_ms: now_ms });
                }
                return Checked { why: Some(why), ..Checked::of(State::Failed) };
            }
        };
        if let Some(latch) = load::<Latch>(&latch_path) {
            if record.id == latch.id || record.created_ms <= latch.seen_ms {
                return Checked { record: Some(record), why: Some("A fence record failed its check here; open a new fence.".into()), ..Checked::of(State::Failed) };
            }
            // A new fence, opened after the failure: the latch is done.
            let _ = std::fs::remove_file(&latch_path);
        }
        // An old record put back: closed since, or replaced by a newer one.
        let seen = seen_with_closed(dir, config);
        let old = if record.id == seen.closed_id || record.created_ms <= seen.closed_ms {
            Some(format!("It's a fence that was closed at {}: putting its file back doesn't open it again.", clock(seen.closed_ms)))
        } else if record.created_ms < seen.last_created_ms {
            Some("It's an older fence put back over a newer one.".to_string())
        } else {
            None
        };
        if let Some(why) = old {
            let _ = crate::util::save_json(&latch_path, &Latch { id: record.id.clone(), sha256: stamp.2.clone(), seen_ms: now_ms });
            return Checked { record: Some(record), why: Some(why), ..Checked::of(State::Failed) };
        }
        if record.created_ms > now_ms + SKEW_MS {
            return Checked { record: Some(record), why: Some("Its start is in the future.".into()), ..Checked::of(State::Failed) };
        }
        let state = if now_ms >= record.expires_ms { State::Expired } else { State::Valid };
        if state == State::Valid && record.created_ms > seen.last_created_ms {
            note_seen(config, Seen { last_id: record.id.clone(), last_created_ms: record.created_ms, ..seen });
        }
        Checked { state, record: Some(record), why: None }
    }
}

/// This build's fence-seen.json, with a newer close the other build wrote next to the record
/// folded in (kept here, so deleting fence-closed.json doesn't undo it).
fn seen_with_closed(dir: &Path, config: &Path) -> Seen {
    let mut seen = load::<Seen>(&config.join(SEEN)).unwrap_or_default();
    if let Some(c) = load::<Closed>(&dir.join(CLOSED)).filter(|c| c.closed_ms > seen.closed_ms) {
        seen.closed_id = c.id;
        seen.closed_ms = c.closed_ms;
        note_seen(config, seen.clone());
    }
    seen
}

fn note_seen(config: &Path, seen: Seen) {
    let _ = crate::util::save_json(&config.join(SEEN), &seen);
}

/// Parses and checks a record's bytes: Ok, or Err((its id as far as it can be read, why)).
fn judge(bytes: &[u8], trust: &Trust) -> Result<Record, (String, String)> {
    let id = || serde_json::from_slice::<serde_json::Value>(bytes).ok().and_then(|v| v["id"].as_str().map(String::from)).unwrap_or_default();
    let r: Record = serde_json::from_slice(bytes).map_err(|e| (id(), format!("fence.json isn't a fence record ({e}).")))?;
    if !verify(&r, trust) {
        return Err((r.id, "Its signature doesn't match: it was changed after it was opened.".into()));
    }
    rules(&r, &trust.key_id).map_err(|why| (r.id.clone(), format!("It breaks the fence rules: {why}.")))?;
    Ok(r)
}

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn load_trust(config: &Path) -> Option<Trust> {
    load(&config.join(TRUST))
}

pub fn load_key_file(dir: &Path) -> Option<KeyFile> {
    load(&dir.join(KEY_FILE))
}

/// Pins `key` as the one this build trusts.
pub fn save_trust(config: &Path, key: &KeyFile) -> Result<(), String> {
    crate::util::save_json(&config.join(TRUST), &Trust { key_id: key.key_id.clone(), public_key: key.public_key.clone() })
}

/// Writes fence-key.pub and the record, each atomically, clears this build's latch and notes the
/// record as the newest this build accepted.
#[cfg(any(test, not(debug_assertions)))]
pub fn write(dir: &Path, config: &Path, key: &KeyFile, record: &Record) -> Result<(), String> {
    crate::util::save_json(&dir.join(KEY_FILE), key)?;
    crate::util::save_json(&dir.join(RECORD), record)?;
    let _ = std::fs::remove_file(config.join(LATCH));
    let seen = load::<Seen>(&config.join(SEEN)).unwrap_or_default();
    if record.created_ms > seen.last_created_ms {
        note_seen(config, Seen { last_id: record.id.clone(), last_created_ms: record.created_ms, ..seen });
    }
    Ok(())
}

/// Deletes fence.json (the key stays), after noting the close in fence-closed.json next to it and
/// in this build's fence-seen.json, so the file put back stays closed. Nothing to delete is fine.
pub fn close(dir: &Path, config: &Path, now_ms: u64) -> Result<(), String> {
    let id = load::<serde_json::Value>(&dir.join(RECORD)).and_then(|v| v["id"].as_str().map(String::from)).unwrap_or_default();
    let closed = Closed { id, closed_ms: now_ms };
    crate::util::save_json(&config.join(SEEN), &Seen { closed_id: closed.id.clone(), closed_ms: now_ms, ..load::<Seen>(&config.join(SEEN)).unwrap_or_default() })
        .map_err(|e| format!("Couldn't close the fence: {e}"))?;
    if dir.is_dir() {
        crate::util::save_json(&dir.join(CLOSED), &closed).map_err(|e| format!("Couldn't close the fence: {e}"))?;
    }
    match std::fs::remove_file(dir.join(RECORD)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Couldn't close the fence: {e}")),
    }
}

/// "14:05", this Mac's local time.
pub fn clock(ms: u64) -> String {
    jiff::Timestamp::from_millisecond(ms as i64).map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn dirs(name: &str) -> (PathBuf, PathBuf) {
        let base = crate::fake_node::temp(name);
        let (dir, config) = (base.join("SparkBox Fence"), base.join("config"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        (dir, config)
    }

    /// A key, its key file written and trusted by `config`.
    pub(crate) fn trusted_key(dir: &Path, config: &Path) -> (SigningKey, KeyFile) {
        let key = new_key().unwrap();
        let (key_id, public_key) = public_of(&key).unwrap();
        let file = KeyFile { key_id, public_key, kind: "keychain".into() };
        crate::util::save_json(&dir.join(KEY_FILE), &file).unwrap();
        save_trust(config, &file).unwrap();
        (key, file)
    }

    pub(crate) fn spec(nodes: &[&str], classes: &[FenceClass], instance: &str) -> Spec {
        Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes: classes.to_vec(), minutes: 180, instance: instance.into(), ..Default::default() }
    }

    const NOW: u64 = 1_790_000_000_000;

    #[test]
    fn a_signed_record_checks_out_and_any_edit_breaks_it() {
        let (dir, config) = dirs("fence-sign");
        let (key, file) = trusted_key(&dir, &config);
        let r = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW, "Studio").unwrap();
        assert!(r.id.starts_with(&format!("fence-{NOW}-")) && r.id.len() == "fence-".len() + 13 + 1 + 8, "{}", r.id);
        assert_eq!((r.expires_ms - r.created_ms, r.key_id.as_str()), (180 * 60_000, file.key_id.as_str()));
        let trust = load_trust(&config).unwrap();
        assert!(verify(&r, &trust));
        for edit in [
            Record { nodes: vec!["cell2".into(), "gx10-0000".into()], ..r.clone() },
            Record { expires_ms: r.expires_ms + 1, ..r.clone() },
            Record { classes: vec![FenceClass::Any], ..r.clone() },
            Record { instance: "dev".into(), ..r.clone() },
        ] {
            assert!(!verify(&edit, &trust), "{edit:?}");
        }
        // Another key, even one written next to the record, isn't the trusted one.
        let other = new_key().unwrap();
        let mut forged = r.clone();
        sign(&mut forged, &other);
        assert!(!verify(&forged, &trust));
        // The key id is the first 16 hex of the key's SHA-256.
        assert_eq!(file.key_id.len(), 16);
        assert_eq!(key_id(&base64::engine::general_purpose::STANDARD.decode(&file.public_key).unwrap()), file.key_id);
    }

    #[test]
    fn the_rules_and_the_spec_checks() {
        let ids: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].iter().map(|s| s.to_string()).collect();
        assert_eq!(validate(&spec(&["cell2"], &[FenceClass::Service], "dev"), &ids), Ok(()));
        let any_dev = validate(&spec(&["cell2"], &[FenceClass::Any], "dev"), &ids).unwrap_err();
        assert!(any_dev.contains("only for the release build"), "{any_dev}");
        assert_eq!(validate(&spec(&["cell2"], &[FenceClass::Any], "release"), &ids), Ok(()));
        assert!(validate(&spec(&["cell9"], &[FenceClass::Service], "release"), &ids).unwrap_err().contains("cell9 isn't one of your nodes"));
        assert!(validate(&Spec { minutes: 8 * 60 + 1, ..spec(&["cell2"], &[FenceClass::Service], "release") }, &ids).is_err());
        assert!(validate(&spec(&["cell2"], &[FenceClass::Other], "release"), &ids).is_err());
        let mut s = spec(&["cell2"], &[FenceClass::Service], "release");
        s.only.insert(FenceClass::Shell, vec!["x".into()]);
        assert!(validate(&s, &ids).is_err(), "an `only` list for a class it doesn't allow");
        s.only = [(FenceClass::Service, vec!["earlyoom.service; rm -rf /".to_string()])].into();
        assert!(validate(&s, &ids).is_err());

        // A record signed by the right key that breaks the rules still fails.
        let (dir, config) = dirs("fence-rules");
        let (key, _) = trusted_key(&dir, &config);
        let trust = load_trust(&config).unwrap();
        let mut long = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW, "Studio").unwrap();
        long.expires_ms = long.created_ms + MAX_MS + 1;
        sign(&mut long, &key);
        assert!(verify(&long, &trust) && rules(&long, &trust.key_id).is_err());
        let mut any = record_for(&spec(&["cell2"], &[FenceClass::Service], "dev"), &key, NOW, "Studio").unwrap();
        any.classes = vec![FenceClass::Any];
        sign(&mut any, &key);
        assert!(rules(&any, &trust.key_id).unwrap_err().contains("only for the release build"));
    }

    #[test]
    fn checks_states_and_the_tamper_latch() {
        let (dir, config) = dirs("fence-latch");
        let v = Verifier::default();
        assert_eq!(v.check(&dir, &config, NOW).state, State::None);
        let (key, file) = trusted_key(&dir, &config);
        let r = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW, "Studio").unwrap();
        write(&dir, &config, &file, &r).unwrap();
        let original = std::fs::read(dir.join(RECORD)).unwrap();
        let ok = v.check(&dir, &config, NOW + 1000);
        assert_eq!((ok.state, ok.record.as_ref().map(|r| r.id.clone())), (State::Valid, Some(r.id.clone())));
        assert_eq!(v.check(&dir, &config, r.expires_ms).state, State::Expired, "expiry is checked every time, cache or not");

        // A hand edit that adds gx10-0000: refused, and the latch is set.
        let mut edited: serde_json::Value = serde_json::from_slice(&original).unwrap();
        edited["nodes"] = serde_json::json!(["cell2", "gx10-0000"]);
        std::fs::write(dir.join(RECORD), serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
        let bad = v.check(&dir, &config, NOW + 2000);
        assert_eq!(bad.state, State::Failed);
        assert!(bad.why.unwrap().contains("signature"));
        let latch: Latch = serde_json::from_slice(&std::fs::read(config.join(LATCH)).unwrap()).unwrap();
        assert_eq!(latch.id, r.id);
        // Putting the original back doesn't open it again.
        std::fs::write(dir.join(RECORD), &original).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 3000).state, State::Failed);
        assert_eq!(Verifier::default().check(&dir, &config, NOW + 3000).state, State::Failed, "nor does a fresh cache");
        // A new fence, opened after the failure, does; the latch is gone.
        let fresh = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW + 4000, "Studio").unwrap();
        crate::util::save_json(&dir.join(RECORD), &fresh).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 5000).state, State::Valid);
        assert!(!config.join(LATCH).exists());

        // An unknown field, or garbage, fails too.
        let mut extra: serde_json::Value = serde_json::to_value(&fresh).unwrap();
        extra["allowAll"] = true.into();
        std::fs::write(dir.join(RECORD), serde_json::to_vec(&extra).unwrap()).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 6000).state, State::Failed);
        std::fs::write(dir.join(RECORD), b"{not json").unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 6000).state, State::Failed);

        // Closing removes the record and keeps the key.
        close(&dir, &config, NOW + 6500).unwrap();
        close(&dir, &config, NOW + 6600).unwrap();
        assert!(!dir.join(RECORD).exists() && dir.join(KEY_FILE).exists());
        assert_eq!(v.check(&dir, &config, NOW + 7000).state, State::None);
    }

    #[test]
    fn a_closed_fence_put_back_stays_closed() {
        let (dir, config) = dirs("fence-closed");
        let v = Verifier::default();
        let (key, file) = trusted_key(&dir, &config);
        let r = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW, "Studio").unwrap();
        write(&dir, &config, &file, &r).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 1000).state, State::Valid);
        let saved = std::fs::read(dir.join(RECORD)).unwrap();
        close(&dir, &config, NOW + 2000).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 3000).state, State::None);
        // The same bytes back, signature and all: still closed, and the latch is set.
        std::fs::write(dir.join(RECORD), &saved).unwrap();
        let back = v.check(&dir, &config, NOW + 4000);
        assert_eq!(back.state, State::Failed);
        assert!(back.why.as_deref().unwrap().contains("was closed at"), "{:?}", back.why);
        assert_eq!(Verifier::default().check(&dir, &config, NOW + 4000).state, State::Failed, "a fresh cache too");
        assert!(config.join(LATCH).exists());
        // The other build, which never closed it, learns of the close from fence-closed.json…
        let other = dir.parent().unwrap().join("dev-config");
        save_trust(&other, &file).unwrap();
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 4000).state, State::Failed);
        // …and keeps it when that file is deleted too.
        std::fs::remove_file(dir.join(CLOSED)).unwrap();
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 4000).state, State::Failed);
        // A new fence opens.
        let fresh = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW + 5000, "Studio").unwrap();
        write(&dir, &config, &file, &fresh).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 6000).state, State::Valid);
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 6000).state, State::Valid);
    }

    #[test]
    fn an_older_wider_fence_put_back_over_a_newer_one_fails() {
        let (dir, config) = dirs("fence-older");
        let v = Verifier::default();
        let (key, file) = trusted_key(&dir, &config);
        let wide = record_for(&spec(&["cell1", "cell2", "cell3", "gx10-0000"], &[FenceClass::Service, FenceClass::PowerOne], "release"), &key, NOW, "Studio").unwrap();
        write(&dir, &config, &file, &wide).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 1000).state, State::Valid);
        let saved = std::fs::read(dir.join(RECORD)).unwrap();
        // The owner narrows it to cell2.
        let narrow = record_for(&spec(&["cell2"], &[FenceClass::Service], "release"), &key, NOW + 2000, "Studio").unwrap();
        write(&dir, &config, &file, &narrow).unwrap();
        assert_eq!(v.check(&dir, &config, NOW + 3000).record.map(|r| r.id), Some(narrow.id.clone()));
        // The wide one's file put back: refused.
        std::fs::write(dir.join(RECORD), &saved).unwrap();
        let back = v.check(&dir, &config, NOW + 4000);
        assert_eq!((back.state, back.why.as_deref()), (State::Failed, Some("It's an older fence put back over a newer one.")));
        // The build that only ever saw the wide one learns of the newer one when it checks it.
        let other = dir.parent().unwrap().join("dev-config");
        save_trust(&other, &file).unwrap();
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 4000).state, State::Valid, "it never saw the narrow one");
        crate::util::save_json(&dir.join(RECORD), &narrow).unwrap();
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 5000).state, State::Valid);
        std::fs::write(dir.join(RECORD), &saved).unwrap();
        assert_eq!(Verifier::default().check(&dir, &other, NOW + 6000).state, State::Failed);
    }

    #[test]
    fn a_key_this_build_doesnt_trust() {
        let (dir, config) = dirs("fence-untrusted");
        let (key, file) = trusted_key(&dir, &config);
        let r = record_for(&spec(&["cell2"], &[FenceClass::Service], "dev"), &key, NOW, "Studio").unwrap();
        write(&dir, &config, &file, &r).unwrap();
        // Another build's config folder, which never trusted the key.
        let other = dir.parent().unwrap().join("dev-config");
        std::fs::create_dir_all(&other).unwrap();
        let c = Verifier::default().check(&dir, &other, NOW);
        assert_eq!(c.state, State::Untrusted);
        // A key file swapped for a new key: untrusted here too, never verified with it.
        let (_, swapped) = trusted_key(&dir, &other);
        crate::util::save_json(&dir.join(KEY_FILE), &swapped).unwrap();
        assert_eq!(Verifier::default().check(&dir, &config, NOW).state, State::Untrusted);
        assert_eq!(left(2 * 3_600_000 + 41 * 60_000 + 5_000), "2 h 41 min");
        assert_eq!(left(35 * 60_000), "35 min");
        assert_eq!(r.summary(NOW + 3_600_000), "cell2 · 1 class · 2 h");
    }
}
