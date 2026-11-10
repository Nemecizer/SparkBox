//! One SSH connection to a node, direct, through the VPN's SOCKS proxy or through a
//! jump host (ProxyJump, one hop), with the host key checked against SparkBox's known
//! hosts. Login tries ssh-agent keys, then key files, then the cluster password.
//! Commands run on channels of the same connection.
//!
//! Host keys are trusted explicitly: a node SparkBox hasn't seen before is
//! refused with its fingerprint (`SshError::HostKeyUnknown`) until the user
//! compares it and calls `trust`, unless ~/.ssh/known_hosts already vouches
//! for that exact key. A changed key is always refused. A known_hosts.json
//! that can't be read fails closed: no connection until host keys are forgotten.
//!
//! Each connection keeps a channel budget. OpenSSH allows 10 session channels per connection
//! (MaxSessions): SparkBox opens at most 8 for commands, keeps 1 for the collector stream, and
//! leaves 1 spare for a channel whose command is still ending on the node. A command waits up to
//! 20 s for a place; a node that refuses a channel anyway ("administratively prohibited") is
//! asked again after 100, 200, 400, 800 and 1600 ms. Port forwards aren't session channels and
//! don't count. A streaming channel its caller lets go of (`Reader::abandon`) gets TERM and end
//! of input, and keeps its place until the node has closed it: sshd counts it until then.
//!
//! sshd hangs up only on a PTY session whose channel closes; a plain command runs on until it
//! reads end of input or writes to its closed output. So SparkBox ends what it lets go of
//! itself: a command whose caller stops waiting (its future dropped) gets TERM before its
//! channel closes, and a terminal that's dropped is hung up (`Reader::hang_up`).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use russh::client::{self, AuthResult, DisconnectReason, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf, Disconnect, Sig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use super::sshlog::SshLog;
use crate::util;

/// Session channels one connection opens at once for commands (the collector has its own).
pub const CHANNELS: usize = 8;
/// How long a command waits for one of them before it fails as busy.
pub const CHANNEL_WAIT: Duration = Duration::from_secs(20);
/// Waits before asking again after the node refused a channel.
const REFUSED_BACKOFF_MS: [u64; 5] = [100, 200, 400, 800, 1600];
/// How long opening a channel may take when the caller gave no limit.
pub const OPEN_LIMIT: Duration = Duration::from_secs(20);
/// How long an abandoned channel keeps its place while the node ends its command and closes it.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// What a command gets when no channel came free: kind "busy", not a timeout or a lost node.
pub fn busy_message(node: &str) -> String {
    format!("SSH channels busy on {node}")
}

pub fn is_busy(message: &str) -> bool {
    message.starts_with("SSH channels busy on ")
}

/// What a command its time limit stopped fails with (the start of it, and the limit).
pub fn took_longer(cmd: &str, limit: Duration) -> String {
    format!("`{}` took longer than {}s", cmd.chars().take(120).collect::<String>(), limit.as_secs())
}

/// SOCKS5 entry point of the VPN tunnel: (address, user, password).
pub type Socks = (String, String, String);

/// Most output any one command may return (stdout and stderr each).
pub const OUTPUT_CAP: usize = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub code: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    /// The time limit ran out; the command was sent TERM.
    pub timed_out: bool,
    /// Output went past OUTPUT_CAP and the rest was dropped.
    pub truncated: bool,
    /// Cancelled by the caller; the command was sent TERM.
    pub cancelled: bool,
    /// It was killed by this signal ("KILL", "TERM"…): sshd sent exit-signal, not exit-status.
    pub signal: Option<String>,
}

impl Output {
    /// Why a command that should have succeeded didn't, or None: an exit status other than 0,
    /// a death by signal, or no status at all (the channel just closed).
    pub fn failure(&self, what: &str, node: &str) -> Option<String> {
        match (self.code, &self.signal) {
            (Some(0), _) => None,
            (_, Some(sig)) => Some(format!("{what} on {node} was killed (SIG{sig})")),
            (Some(c), None) => Some(format!("{what} on {node} failed (exit {c}){}", tail_of(&self.stderr))),
            (None, None) => Some(format!("{what} on {node} ended without reporting success")),
        }
    }
}

fn tail_of(stderr: &str) -> String {
    match stderr.trim().lines().last() {
        Some(l) if !l.trim().is_empty() => format!(": {}", l.trim()),
        _ => String::new(),
    }
}

/// A signal's name as OpenSSH sends it ("KILL", "TERM").
pub fn sig_name(sig: &Sig) -> String {
    match sig {
        Sig::Custom(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Which stream a chunk of output came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Lets a caller stop a running command (Fleet's Stop button, a cancelled task).
#[derive(Clone, Default)]
pub struct CancelToken(Arc<(AtomicBool, Notify)>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0 .0.store(true, Ordering::SeqCst);
        self.0 .1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0 .0.load(Ordering::SeqCst)
    }

    /// Resolves once `cancel` is called.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0 .1.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Why a connection couldn't be made.
#[derive(Clone, Debug, PartialEq)]
pub enum SshError {
    /// First contact: the key isn't trusted yet. Show the fingerprint and let the user decide.
    HostKeyUnknown { address: String, fingerprint: String },
    /// The node presents a different key from the one pinned for it.
    HostKeyChanged { address: String, pinned: String, fingerprint: String },
    Failed(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshError::HostKeyUnknown { address, fingerprint } => write!(
                f,
                "SparkBox hasn't connected to {address} before. Its host key is {fingerprint}. \
                 Compare it with the node (ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub) and trust it in Connect or Settings › Nodes."
            ),
            SshError::HostKeyChanged { address, pinned, fingerprint } => write!(
                f,
                "The host key for {address} changed (was {pinned}, now {fingerprint}). If you reinstalled the node, \
                 use Settings › Diagnostics › Forget host keys; otherwise something may be intercepting the connection."
            ),
            SshError::Failed(m) => f.write_str(m),
        }
    }
}

impl From<SshError> for String {
    fn from(e: SshError) -> String {
        e.to_string()
    }
}

impl From<String> for SshError {
    fn from(s: String) -> SshError {
        SshError::Failed(s)
    }
}

// ---- Known hosts

static KNOWN_LOCK: Mutex<()> = Mutex::new(());

fn broken_message(backup: &str) -> String {
    format!(
        "known_hosts.json couldn't be read; the original is kept at {backup}; use Settings › Diagnostics › Forget host keys to start over."
    )
}

/// Host-key fingerprints by "address:port". Fails closed: a file that can't be read
/// (now, or earlier this session) refuses every connection until host keys are forgotten.
pub fn load_known(path: &Path) -> Result<HashMap<String, String>, String> {
    if let Some(w) = util::warning_for(path) {
        return Err(broken_message(&w.backup));
    }
    match util::load_json_or_quarantine::<HashMap<String, String>>(path) {
        Ok(Some(map)) => Ok(map),
        Ok(None) => {
            // A backup next to a missing file means it was set aside before; don't start trusting afresh.
            if let Some(backup) = corrupt_sibling(path) {
                return Err(broken_message(&backup));
            }
            Ok(HashMap::new())
        }
        Err(q) => Err(broken_message(&q.backup.display().to_string())),
    }
}

fn corrupt_sibling(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let dir = path.parent()?;
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{name}.corrupt-")))
    }).map(|p| p.display().to_string())
}

/// Pins `fingerprint` for "address:port" (after the user compared it).
pub fn trust(path: &Path, address: &str, fingerprint: &str) -> Result<(), String> {
    if !fingerprint.starts_with("SHA256:") || fingerprint.len() < 20 || address.trim().is_empty() {
        return Err("That doesn't look like an SSH host key fingerprint (SHA256:…).".into());
    }
    let _guard = KNOWN_LOCK.lock().unwrap();
    let mut known = load_known(path)?;
    known.insert(address.trim().to_string(), fingerprint.trim().to_string());
    util::save_json(path, &known)
}

/// Forgets every pinned key (after reinstalling a node), including a known_hosts.json that couldn't be read.
pub fn forget(path: &Path) -> Result<(), String> {
    let _guard = KNOWN_LOCK.lock().unwrap();
    util::save_json(path, &HashMap::<String, String>::new())?;
    util::clear_warning(path);
    Ok(())
}

/// The SHA-256 fingerprint of an OpenSSH public key blob, as `ssh-keygen -l` prints it.
pub fn fingerprint_of_blob(blob: &[u8]) -> String {
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// What ~/.ssh/known_hosts says about `host` (plain, unhashed entries): the fingerprints listed for it.
pub fn openssh_fingerprints(known_hosts: &str, addr: &str, port: u16) -> Vec<String> {
    let want = if port == 22 { addr.to_string() } else { format!("[{addr}]:{port}") };
    known_hosts
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('@') || line.starts_with('|') {
                return None;
            }
            let mut parts = line.split_whitespace();
            let hosts = parts.next()?;
            let _kind = parts.next()?;
            let key = parts.next()?;
            if !hosts.split(',').any(|h| h == want) {
                return None;
            }
            let blob = base64::engine::general_purpose::STANDARD.decode(key).ok()?;
            Some(fingerprint_of_blob(&blob))
        })
        .collect()
}

fn user_known_hosts() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    std::fs::read_to_string(Path::new(&home).join(".ssh/known_hosts")).ok()
}

/// What the handshake found out about the host key.
#[derive(Default)]
struct KeyVerdict {
    /// (pinned, presented)
    mismatch: Option<(String, String)>,
    /// The key the server presented, whatever the verdict.
    presented: Option<String>,
    unknown: Option<String>,
    broken: Option<String>,
}

struct Client {
    host: String,
    addr: String,
    port: u16,
    known_hosts: PathBuf,
    verdict: Arc<Mutex<KeyVerdict>>,
    trust: Trust,
    /// Why the connection ended, when it ended on its own (the node hung up, keepalive lost).
    ended: Arc<Mutex<Option<String>>>,
}

/// Which host keys a connection accepts, and whether it saves any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Keys SparkBox pinned, and a key ~/.ssh/known_hosts vouches for (pinned then: the same
    /// decision, made earlier in Terminal).
    Normal,
    /// The Connect wizard on a node in the inventory: pinned or vouched keys, nothing saved.
    CheckOnly,
    /// The Connect wizard on a node that isn't in the inventory yet: only a key the user
    /// trusted in SparkBox, nothing saved.
    PinnedOnly,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256).to_string(),
            PublicKeyOrCertificate::Certificate(cert) => cert.public_key().fingerprint(HashAlg::Sha256).to_string(),
        };
        let _guard = KNOWN_LOCK.lock().unwrap();
        self.verdict.lock().unwrap().presented = Some(fingerprint.clone());
        let mut known = match load_known(&self.known_hosts) {
            Ok(k) => k,
            Err(e) => {
                self.verdict.lock().unwrap().broken = Some(e);
                return Ok(false);
            }
        };
        match known.get(&self.host) {
            Some(pinned) if *pinned == fingerprint => Ok(true),
            Some(pinned) => {
                self.verdict.lock().unwrap().mismatch = Some((pinned.clone(), fingerprint));
                Ok(false)
            }
            None => {
                // The user's own ssh already trusts this exact key: that's the same decision, made earlier.
                let vouched = self.trust != Trust::PinnedOnly && user_known_hosts().is_some_and(|t| openssh_fingerprints(&t, &self.addr, self.port).contains(&fingerprint));
                if vouched && self.trust == Trust::CheckOnly {
                    Ok(true)
                } else if vouched {
                    known.insert(self.host.clone(), fingerprint);
                    let _ = util::save_json(&self.known_hosts, &known);
                    Ok(true)
                } else {
                    self.verdict.lock().unwrap().unknown = Some(fingerprint);
                    Ok(false)
                }
            }
        }
    }

    async fn disconnected(&mut self, reason: DisconnectReason<Self::Error>) -> Result<(), Self::Error> {
        let why = match &reason {
            DisconnectReason::ReceivedDisconnect(info) if info.message.trim().is_empty() => format!("the node hung up ({:?})", info.reason_code),
            DisconnectReason::ReceivedDisconnect(info) => format!("the node hung up: {}", info.message.trim()),
            DisconnectReason::Error(russh::Error::KeepaliveTimeout) => "keepalive".to_string(),
            DisconnectReason::Error(e) => e.to_string(),
        };
        self.ended.lock().unwrap().get_or_insert(why);
        match reason {
            DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            DisconnectReason::Error(e) => Err(e),
        }
    }
}

/// Where a node is: its address, port and user, and the jump host in front of it, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub addr: String,
    pub port: u16,
    pub user: String,
    pub jump: Option<Box<Target>>,
}

/// A private key file to offer, as it was written (for messages) and as a full path.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyFile {
    pub shown: String,
    pub path: PathBuf,
    pub passphrase: Option<String>,
}

/// What to log in with, tried in this order: the agent's keys, the key files, the password.
#[derive(Clone, Debug, Default)]
pub struct Login {
    pub agent: Option<PathBuf>,
    pub keys: Vec<KeyFile>,
    /// IdentitiesOnly: offer only agent keys that match one of `keys`.
    pub identities_only: bool,
    pub password: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Agent,
    Key,
    Password,
}

/// How a connection logged in: "~/.ssh/id_ed25519 via agent", "~/.ssh/id_ed25519", "password".
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginInfo {
    pub method: AuthMethod,
    pub detail: String,
}

pub struct Session {
    handle: Handle<Client>,
    /// This connection's number, unique in this run of SparkBox (`serial`).
    serial: u64,
    pub login: LoginInfo,
    /// The host key the node presented (and SparkBox accepted), "SHA256:…".
    pub host_key: Option<String>,
    /// The jump host's connection, which carries this one.
    _jump: Option<Box<Session>>,
    /// The node it's for, in messages and the SSH log (the address until Live names it).
    pub node: String,
    log: SshLog,
    budget: Budget,
    ended: Arc<Mutex<Option<String>>>,
    /// Facts checked once on this connection (a helper script's checksum). A command that exits
    /// COPY_STALE found one no longer true: they're all checked again.
    checked: Arc<Mutex<HashSet<String>>>,
}

/// One connection's channel budget (see the module notes).
struct Budget {
    general: Arc<Semaphore>,
    collector: Arc<Semaphore>,
    queued: AtomicUsize,
    refusals: AtomicU64,
}

impl Budget {
    fn new() -> Self {
        Budget { general: Arc::new(Semaphore::new(CHANNELS)), collector: Arc::new(Semaphore::new(1)), queued: AtomicUsize::new(0), refusals: AtomicU64::new(0) }
    }
}

/// A channel's place in its connection's budget, given back when dropped.
pub struct Slot {
    _permit: OwnedSemaphorePermit,
    of: Arc<Semaphore>,
}

/// How a node's connections are using their channels (Export diagnostics, the SSH log).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStats {
    pub in_use: usize,
    pub queued: usize,
    pub refusals: u64,
    pub bulk_connections: usize,
}

impl std::ops::AddAssign for ChannelStats {
    fn add_assign(&mut self, o: ChannelStats) {
        self.in_use += o.in_use;
        self.queued += o.queued;
        self.refusals += o.refusals;
        self.bulk_connections += o.bulk_connections;
    }
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    })
}


/// Connects to `target` (through its jump host, if any) and logs in with `login`.
pub async fn connect_to(target: &Target, login: &Login, socks: Option<Socks>, known_hosts: &Path, trust: Trust) -> Result<Session, SshError> {
    connect_transport(target, login, socks, known_hosts, trust, None).await
}

/// A discovery connection already bound to the selected local interface. It cannot use a
/// jump host, proxy or a different address, and still requires explicitly pinned host keys.
pub(crate) async fn connect_discovery(target: &Target, login: &Login, stream: TcpStream, known_hosts: &Path) -> Result<Session, SshError> {
    if target.jump.is_some() { return Err(SshError::Failed("Discovery cannot use a jump host.".into())); }
    connect_transport(target, login, None, known_hosts, Trust::PinnedOnly, Some(stream)).await
}

async fn connect_transport(target: &Target, login: &Login, socks: Option<Socks>, known_hosts: &Path, trust: Trust, stream: Option<TcpStream>) -> Result<Session, SshError> {
    let (addr, port) = (target.addr.as_str(), target.port);
    let host = format!("{addr}:{port}");
    let verdict = Arc::new(Mutex::new(KeyVerdict::default()));
    let ended: Arc<Mutex<Option<String>>> = Arc::default();
    let handler = Client {
        host: host.clone(),
        addr: addr.to_string(),
        port,
        known_hosts: known_hosts.to_path_buf(),
        verdict: verdict.clone(),
        trust,
        ended: ended.clone(),
    };
    let limit = Duration::from_secs(10);
    let via_vpn = socks.is_some();

    let mut jump_session = None;
    let handle = if let Some(stream) = stream {
        timeout(limit, client::connect_stream(config(), stream, handler)).await
    } else { match (&target.jump, socks) {
        (Some(jump), socks) => {
            let j = Box::pin(connect_to(jump, login, socks, known_hosts, trust)).await.map_err(|e| match e {
                SshError::Failed(m) => SshError::Failed(format!("Through the jump host {}: {m}", jump.addr)),
                other => other,
            })?;
            let ch = j
                .handle
                .channel_open_direct_tcpip(addr, port as u32, "127.0.0.1", 0)
                .await
                .map_err(|e| format!("The jump host {} couldn't reach {host}: {e}", jump.addr))?;
            jump_session = Some(Box::new(j));
            timeout(limit, client::connect_stream(config(), ch.into_stream(), handler)).await
        }
        (None, Some((proxy, proxy_user, proxy_pass))) => {
            let stream = timeout(
                limit,
                tokio_socks::tcp::Socks5Stream::connect_with_password(proxy.as_str(), (addr, port), &proxy_user, &proxy_pass),
            )
            .await
            .map_err(|_| format!("{host} didn't answer through the VPN within {} s", limit.as_secs()))?
            .map_err(|e| format!("Couldn't reach {host} through the VPN: {e}"))?;
            timeout(limit, client::connect_stream(config(), stream, handler)).await
        }
        (None, None) => {
            let stream = timeout(limit, TcpStream::connect((addr, port)))
                .await
                // Direct, so either the node is quiet or this Mac isn't on its network: say both, guess neither.
                .map_err(|_| format!("SSH timeout: no reply from {host} in {} s. The node may be off or restarting, or this Mac isn't on its network.", limit.as_secs()))?
                .map_err(|e| format!("Couldn't reach {host}: {e}"))?;
            let _ = stream.set_nodelay(true);
            timeout(limit, client::connect_stream(config(), stream, handler)).await
        }
    }}
    .map_err(|_| format!("SSH handshake with {host} timed out{}", if via_vpn { " (through the VPN)" } else { "" }))?
    .map_err(|e| {
        let v = verdict.lock().unwrap();
        if let Some(fp) = &v.unknown {
            return SshError::HostKeyUnknown { address: host.clone(), fingerprint: fp.clone() };
        }
        if let (None, Some((pinned, fingerprint))) = (&v.broken, &v.mismatch) {
            return SshError::HostKeyChanged { address: host.clone(), pinned: pinned.clone(), fingerprint: fingerprint.clone() };
        }
        SshError::Failed(v.broken.clone().unwrap_or_else(|| format!("SSH handshake with {host} failed: {e}")))
    })?;

    let mut handle = handle;
    let login = authenticate(&mut handle, &target.user, addr, login, &ended).await?;
    let host_key = verdict.lock().unwrap().presented.clone();
    Ok(Session {
        handle,
        serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
        login,
        host_key,
        _jump: jump_session,
        node: host,
        log: SshLog::off(),
        budget: Budget::new(),
        ended,
        checked: Arc::default(),
    })
}

/// The public half of a key file: its .pub next to it, or the key itself when it has no passphrase.
fn public_half(k: &KeyFile) -> Option<PublicKey> {
    let pub_path = PathBuf::from(format!("{}.pub", k.path.display()));
    if let Ok(text) = std::fs::read_to_string(&pub_path) {
        if let Ok(p) = PublicKey::from_openssh(text.trim()) {
            return Some(p);
        }
    }
    russh::keys::load_secret_key(&k.path, k.passphrase.as_deref()).ok().map(|key| key.public_key().clone())
}

/// Logs in: agent keys, then key files, then the password. Says which one worked. `ended` is why
/// the connection ended, once it has (`Client::disconnected`).
async fn authenticate(handle: &mut Handle<Client>, user: &str, addr: &str, login: &Login, ended: &Mutex<Option<String>>) -> Result<LoginInfo, SshError> {
    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
    let named: Vec<(String, PublicKey)> = login.keys.iter().filter_map(|k| public_half(k).map(|p| (k.shown.clone(), p))).collect();
    let mut tried: Vec<String> = Vec::new();
    if let Some(sock) = login.agent.as_ref().filter(|s| s.exists()) {
        if let Ok(mut agent) = AgentClient::connect_uds(sock).await {
            let ids = agent.request_identities().await.unwrap_or_default();
            let mut offered = 0;
            for id in ids {
                let AgentIdentity::PublicKey { key, comment } = id else { continue };
                let name = named.iter().find(|(_, p)| p.key_data() == key.key_data()).map(|(n, _)| n.clone());
                if login.identities_only && name.is_none() {
                    continue;
                }
                offered += 1;
                if let Ok(AuthResult::Success) = handle.authenticate_publickey_with(user, key.clone(), rsa_hash, &mut agent).await {
                    let shown = name.unwrap_or_else(|| if comment.trim().is_empty() { key.fingerprint(HashAlg::Sha256).to_string() } else { comment });
                    return Ok(LoginInfo { method: AuthMethod::Agent, detail: format!("{shown} via agent") });
                }
            }
            if offered > 0 {
                tried.push(format!("{offered} agent key{}", if offered == 1 { "" } else { "s" }));
            }
        }
    }
    for k in &login.keys {
        let key = match russh::keys::load_secret_key(&k.path, k.passphrase.as_deref()) {
            Ok(key) => key,
            Err(russh::keys::Error::KeyIsEncrypted) => {
                tried.push(format!("{} (needs its passphrase)", k.shown));
                continue;
            }
            Err(_) if !k.path.exists() => continue,
            Err(e) => {
                tried.push(format!("{} ({e})", k.shown));
                continue;
            }
        };
        match handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash)).await {
            Ok(AuthResult::Success) => return Ok(LoginInfo { method: AuthMethod::Key, detail: k.shown.clone() }),
            _ => tried.push(k.shown.clone()),
        }
    }
    if let Some(password) = &login.password {
        return match handle.authenticate_password(user, password).await.map_err(|e| e.to_string())? {
            AuthResult::Success => Ok(LoginInfo { method: AuthMethod::Password, detail: "password".into() }),
            // russh answers Failure when the connection closed before the node did: nothing was rejected.
            AuthResult::Failure { .. } if handle.is_closed() => Err(closed_at_login(addr, ended)),
            AuthResult::Failure { .. } if tried.is_empty() => Err(format!("The password for {user}@{addr} was rejected. Check it in Settings.").into()),
            AuthResult::Failure { .. } => Err(format!("{user}@{addr} didn't accept {} or the password. Check Settings › Cluster login.", tried.join(", ")).into()),
        };
    }
    if handle.is_closed() {
        return Err(closed_at_login(addr, ended));
    }
    if tried.is_empty() {
        return Err("No SSH key was found and no cluster password is saved. Save the cluster password, or set up key login, in Settings › Cluster login.".to_string().into());
    }
    Err(format!("{user}@{addr} didn't accept {}. Add the key to the node's authorized_keys or save the cluster password in Settings.", tried.join(", ")).into())
}

/// A login the connection closed under, which russh reports as a failed login: the node's own
/// words when it hung up with a reason (sshd's "Too many authentication failures"), otherwise a
/// drop (sshd going away, the link down), never a rejected password or key.
fn closed_at_login(addr: &str, ended: &Mutex<Option<String>>) -> SshError {
    match ended.lock().unwrap().clone().and_then(|why| why.strip_prefix("the node hung up: ").map(String::from)) {
        Some(why) => SshError::Failed(format!("{addr} ended the login: {why}")),
        None => SshError::Failed(format!("The connection to {addr} closed during the login.")),
    }
}

/// Appends at most what's left under the cap; says whether anything was dropped.
fn push_capped(buf: &mut Vec<u8>, data: &[u8]) -> bool {
    let room = OUTPUT_CAP.saturating_sub(buf.len());
    buf.extend_from_slice(&data[..data.len().min(room)]);
    data.len() > room
}

/// Closes a channel when dropped, so an abandoned stream (its future dropped) doesn't stay open
/// on the node. With `term`, its command gets TERM first: sshd doesn't stop a command (one
/// without a PTY) whose channel closed, and one stuck where it can't notice would keep its
/// session there.
pub struct CloseOnDrop(Option<ChannelWriteHalf<client::Msg>>, bool);

impl CloseOnDrop {
    pub fn new(w: ChannelWriteHalf<client::Msg>) -> Self {
        CloseOnDrop(Some(w), false)
    }

    /// Sends TERM, then closes, when dropped.
    pub fn term(w: ChannelWriteHalf<client::Msg>) -> Self {
        CloseOnDrop(Some(w), true)
    }

    pub fn get(&self) -> &ChannelWriteHalf<client::Msg> {
        self.0.as_ref().expect("channel writer present until drop")
    }

    /// The writer itself, no longer closed when this is dropped (`Reader::abandon` closes it).
    pub fn take(&mut self) -> Option<ChannelWriteHalf<client::Msg>> {
        self.0.take()
    }
}

impl std::ops::Deref for CloseOnDrop {
    type Target = ChannelWriteHalf<client::Msg>;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let term = self.1;
        if let (Some(w), Ok(rt)) = (self.0.take(), tokio::runtime::Handle::try_current()) {
            rt.spawn(async move {
                // A channel already closed sends neither.
                if term {
                    let _ = w.signal(Sig::TERM).await;
                }
                let _ = w.close().await;
            });
        }
    }
}

/// How far a channel's command got, as its reader saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Seen {
    Running,
    /// Its exit status (or signal) came; the channel isn't closed yet.
    Exited,
    Closed,
}

/// The read half of a channel a caller streams from (Live's `Exec`). It notes how far the
/// command got, so letting go of it only stops what's still running; an exit status of
/// COPY_STALE has the connection check its helpers again.
pub struct Reader {
    read: Option<ChannelReadHalf>,
    seen: Seen,
    checked: Arc<Mutex<HashSet<String>>>,
}

impl Reader {
    /// The channel's next message; None once it's closed.
    pub async fn wait(&mut self) -> Option<ChannelMsg> {
        let msg = match self.read.as_mut() {
            Some(r) => r.wait().await,
            None => None,
        };
        match &msg {
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                self.seen = self.seen.max(Seen::Exited);
                if *exit_status == super::COPY_STALE {
                    self.checked.lock().unwrap().clear();
                }
            }
            Some(ChannelMsg::ExitSignal { .. }) => self.seen = self.seen.max(Seen::Exited),
            Some(ChannelMsg::Close) | None => self.seen = Seen::Closed,
            Some(_) => {}
        }
        msg
    }

    /// This reader, leaving one that reads nothing in its place.
    pub fn take(&mut self) -> Reader {
        let empty = Reader { read: None, seen: Seen::Closed, checked: self.checked.clone() };
        std::mem::replace(self, empty)
    }

    /// The channel's read half itself (`Exec::into_parts`, `Pty::into_parts`).
    pub fn into_inner(self) -> Option<ChannelReadHalf> {
        self.read
    }

    /// Ends a command its caller let go of: TERM while it still runs, end of input, then the
    /// close once the node has closed the channel (or after CLOSE_WAIT). `hold` (its place in the
    /// budget, its connection) is kept until then, so SparkBox's count of a connection's
    /// channels never runs below sshd's.
    pub fn abandon(mut self, write: ChannelWriteHalf<client::Msg>, hold: impl Send + 'static) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            if self.seen < Seen::Closed {
                if self.seen < Seen::Exited {
                    let _ = write.signal(Sig::TERM).await;
                }
                let _ = write.eof().await;
                let _ = timeout(CLOSE_WAIT, async { while self.wait().await.is_some() {} }).await;
                let _ = write.close().await;
            }
            drop(hold);
        });
    }

    /// Ends a terminal its caller let go of: TERM to what runs there, then the close, which hangs
    /// the terminal up on the node (sshd's pty cleanup sends the shell HUP). `hold` is kept until
    /// the node has closed the channel too (or CLOSE_WAIT), as `abandon` keeps it.
    pub fn hang_up(mut self, write: ChannelWriteHalf<client::Msg>, hold: impl Send + 'static) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            if self.seen < Seen::Closed {
                if self.seen < Seen::Exited {
                    let _ = write.signal(Sig::TERM).await;
                }
                let _ = write.close().await;
                let _ = timeout(CLOSE_WAIT, async { while self.wait().await.is_some() {} }).await;
            }
            drop(hold);
        });
    }
}

/// How the collector's command ended: its exit status or signal, and the end of its stderr.
#[derive(Clone, Debug, Default)]
pub struct StreamEnd {
    pub code: Option<u32>,
    pub signal: Option<String>,
    pub stderr: String,
}

/// The next connection's `serial`.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

impl Session {
    /// A number no other connection in this run of SparkBox has had or will have: what a fact
    /// read once per connection is kept against (a later connection can reuse an address in
    /// memory, never a serial).
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Names the node this connection is for and where its channel events are logged.
    pub fn label(&mut self, node: &str, log: SshLog) {
        self.node = node.to_string();
        self.log = log;
    }

    /// A place for one more command channel now, if there is one.
    pub fn try_slot(&self) -> Option<Slot> {
        let of = self.budget.general.clone();
        of.clone().try_acquire_owned().ok().map(|p| Slot { _permit: p, of })
    }

    /// A place for one more command channel, waiting up to CHANNEL_WAIT for one to come free.
    pub async fn slot(&self) -> Result<Slot, String> {
        self.slot_in(&self.budget.general).await
    }

    async fn slot_in(&self, pool: &Arc<Semaphore>) -> Result<Slot, String> {
        if let Ok(p) = pool.clone().try_acquire_owned() {
            return Ok(Slot { _permit: p, of: pool.clone() });
        }
        self.budget.queued.fetch_add(1, Ordering::SeqCst);
        let waited = timeout(CHANNEL_WAIT, pool.clone().acquire_owned()).await;
        self.budget.queued.fetch_sub(1, Ordering::SeqCst);
        match waited {
            Ok(Ok(p)) => Ok(Slot { _permit: p, of: pool.clone() }),
            _ => {
                self.log.event(&self.node, "channels_busy", &[("waited_s", &CHANNEL_WAIT.as_secs()), ("queued", &self.budget.queued.load(Ordering::SeqCst))]);
                Err(busy_message(&self.node))
            }
        }
    }

    /// How this connection's channels are used right now (`bulk_connections` is Live's to fill in).
    pub fn stats(&self) -> ChannelStats {
        ChannelStats {
            in_use: CHANNELS - self.budget.general.available_permits() + (1 - self.budget.collector.available_permits()),
            queued: self.budget.queued.load(Ordering::SeqCst),
            refusals: self.budget.refusals.load(Ordering::SeqCst),
            bulk_connections: 0,
        }
    }

    /// Whether `fact` was already checked on this connection (`mark_checked`).
    pub fn checked(&self, fact: &str) -> bool {
        self.checked.lock().unwrap().contains(fact)
    }

    pub fn mark_checked(&self, fact: &str) {
        self.checked.lock().unwrap().insert(fact.to_string());
    }

    /// Everything checked on this connection is checked again on next use.
    pub fn forget_checked(&self) {
        self.checked.lock().unwrap().clear();
    }

    /// Why the connection ended by itself ("keepalive", "the node hung up: …"), once it has.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap().clone()
    }

    /// Opens a session channel, asking again (with back-off) while the node refuses it as
    /// over its limit, until `deadline`. A refusal is back-pressure, never a dead node. OpenSSH
    /// refuses a session past MaxSessions as "connect failed" ("no more sessions" in its log;
    /// a session channel has nothing else to connect to); other servers say "prohibited".
    async fn open(&self, deadline: tokio::time::Instant) -> Result<Option<russh::Channel<client::Msg>>, String> {
        for (attempt, wait) in REFUSED_BACKOFF_MS.iter().enumerate() {
            match tokio::time::timeout_at(deadline, self.handle.channel_open_session()).await {
                Err(_) => return Ok(None),
                Ok(Ok(ch)) => return Ok(Some(ch)),
                Ok(Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed | ChannelOpenFailure::AdministrativelyProhibited))) => {
                    self.budget.refusals.fetch_add(1, Ordering::SeqCst);
                    self.log.event(&self.node, "channel_refused", &[("attempt", &(attempt + 1)), ("in_use", &self.stats().in_use)]);
                    let wait = Duration::from_millis(*wait);
                    if attempt + 1 == REFUSED_BACKOFF_MS.len() || tokio::time::Instant::now() + wait >= deadline {
                        break;
                    }
                    tokio::time::sleep(wait).await;
                }
                Ok(Err(e)) => return Err(e.to_string()),
            }
        }
        Err(busy_message(&self.node))
    }

    /// Runs a command to completion, optionally feeding it stdin.
    pub async fn exec(&self, cmd: &str, stdin: Option<&[u8]>, limit: Duration) -> Result<Output, String> {
        let out = self.exec_with(cmd, stdin, limit, &CancelToken::new(), &mut |_, _| {}).await?;
        if out.timed_out {
            return Err(took_longer(cmd, limit));
        }
        Ok(out)
    }

    /// Runs a command, handing each chunk of output to `on_chunk` and keeping up to
    /// OUTPUT_CAP of each stream. On cancel or timeout the command gets TERM and the
    /// channel is closed; the returned Output says which.
    pub async fn exec_with(
        &self,
        cmd: &str,
        stdin: Option<&[u8]>,
        limit: Duration,
        cancel: &CancelToken,
        on_chunk: &mut (dyn FnMut(StreamKind, &[u8]) + Send),
    ) -> Result<Output, String> {
        let slot = self.slot().await?;
        self.exec_on(slot, cmd, stdin, limit, cancel, on_chunk).await
    }

    /// `exec_with` on a place already taken on this connection.
    /// The limit covers opening the channel too: a node that doesn't answer times out.
    async fn exec_on(
        &self,
        slot: Slot,
        cmd: &str,
        stdin: Option<&[u8]>,
        limit: Duration,
        cancel: &CancelToken,
        on_chunk: &mut (dyn FnMut(StreamKind, &[u8]) + Send),
    ) -> Result<Output, String> {
        let deadline = tokio::time::Instant::now() + limit;
        let ch = self.open(deadline).await?;
        self.exec_in(ch, slot, cmd, stdin, deadline, cancel, on_chunk).await
    }

    /// Opens a session channel on a place already taken on this connection (Live picks a bulk
    /// connection by it), within `limit`, asking again while the node refuses it. None: the node
    /// didn't answer in time.
    pub async fn open_on(&self, slot: &Slot, limit: Duration) -> Result<Option<russh::Channel<client::Msg>>, String> {
        debug_assert!(Arc::ptr_eq(&slot.of, &self.budget.general), "a slot from another connection");
        self.open(tokio::time::Instant::now() + limit).await
    }

    /// What a channel that didn't open in OPEN_LIMIT reports.
    pub fn not_opened(&self) -> String {
        format!("{} didn't open a channel in {} s", self.node, OPEN_LIMIT.as_secs())
    }

    /// Runs a command on a channel `open_on` opened (None: it timed out opening), until
    /// `deadline`, like `exec_with`. The place is given back when it ends.
    #[allow(clippy::too_many_arguments)]
    pub async fn exec_in(
        &self,
        ch: Option<russh::Channel<client::Msg>>,
        slot: Slot,
        cmd: &str,
        stdin: Option<&[u8]>,
        deadline: tokio::time::Instant,
        cancel: &CancelToken,
        on_chunk: &mut (dyn FnMut(StreamKind, &[u8]) + Send),
    ) -> Result<Output, String> {
        debug_assert!(Arc::ptr_eq(&slot.of, &self.budget.general), "a slot from another connection");
        let Some(ch) = ch else {
            return Ok(Output { timed_out: true, ..Default::default() });
        };
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        let (mut read, write) = ch.split();
        // A caller that stops waiting (its future dropped) ends the command too: sshd wouldn't.
        let write = CloseOnDrop::term(write);
        if let Some(data) = stdin {
            write.get().data(data).await.map_err(|e| e.to_string())?;
        }
        // Always end stdin, so a command that reads it gets EOF instead of waiting.
        write.get().eof().await.map_err(|e| e.to_string())?;
        let out = collect(&mut read, write, deadline.saturating_duration_since(tokio::time::Instant::now()), cancel, on_chunk).await;
        if out.as_ref().is_ok_and(|o| o.code == Some(super::COPY_STALE)) {
            self.forget_checked();
        }
        drop(slot);
        out
    }

    /// Runs `true` on the place kept for the collector (free while the collector is stopped),
    /// within `limit`: whether the connection still answers, whatever else is running on it.
    pub async fn answers(&self, limit: Duration) -> Result<(), String> {
        let _slot = self.slot_in(&self.budget.collector).await?;
        let deadline = tokio::time::Instant::now() + limit;
        let Some(ch) = self.open(deadline).await? else {
            return Err(format!("no reply in {} s", limit.as_secs()));
        };
        ch.exec(true, "true").await.map_err(|e| e.to_string())?;
        let (mut read, write) = ch.split();
        let out = collect(&mut read, CloseOnDrop::new(write), deadline.saturating_duration_since(tokio::time::Instant::now()), &CancelToken::new(), &mut |_, _| {}).await?;
        match (out.timed_out, out.code) {
            (true, _) => Err(format!("no reply in {} s", limit.as_secs())),
            (_, Some(0)) => Ok(()),
            (_, code) => Err(format!("`true` ended with {code:?}")),
        }
    }

    /// Runs a long-lived command (the collector), calling `on_line` for each line of output
    /// until it exits, on the place kept for it. Returns how it ended and the end of stderr.
    /// Dropping the future (a stall) sends the command TERM and closes the channel.
    pub async fn stream(&self, cmd: &str, mut on_line: impl FnMut(&str)) -> Result<StreamEnd, String> {
        let _slot = self.slot_in(&self.budget.collector).await?;
        let Some(ch) = self.open(tokio::time::Instant::now() + OPEN_LIMIT).await? else {
            return Err(format!("{} didn't open a channel in {} s", self.node, OPEN_LIMIT.as_secs()));
        };
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        let (mut read, write) = ch.split();
        let _close = CloseOnDrop::term(write);
        let _ = _close.get().eof().await;
        let mut buf = Vec::new();
        let mut end = StreamEnd::default();
        let stderr = &mut end.stderr;
        while let Some(msg) = read.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    buf.extend_from_slice(&data);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        on_line(String::from_utf8_lossy(&line).trim_end());
                    }
                    if buf.len() > OUTPUT_CAP {
                        buf.clear(); // a line that never ends isn't a frame
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    stderr.push_str(&String::from_utf8_lossy(&data));
                    if stderr.len() > 4000 {
                        // The last 2000 bytes or so, cut where a character starts.
                        let cut = (stderr.len() - 2000..stderr.len()).find(|i| stderr.is_char_boundary(*i)).unwrap_or(stderr.len());
                        *stderr = stderr.split_off(cut);
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => end.code = Some(exit_status),
                ChannelMsg::ExitSignal { signal_name, .. } => end.signal = Some(sig_name(&signal_name)),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(end)
    }

    /// Starts a command and hands back both ends of its channel, for streaming stdin and
    /// stdout, with its place in the budget (held until the caller drops it).
    pub async fn exec_io(&self, cmd: &str) -> Result<(ChannelReadHalf, ChannelWriteHalf<client::Msg>, Slot), String> {
        let slot = self.slot().await?;
        let ch = self.open_on(&slot, OPEN_LIMIT).await?.ok_or_else(|| self.not_opened())?;
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        let (read, write) = ch.split();
        Ok((read, write, slot))
    }

    /// Starts `cmd` on a channel `open_on` opened and hands back both ends, for streaming stdin
    /// and stdout (Live's `Exec`).
    pub async fn exec_io_in(&self, ch: russh::Channel<client::Msg>, cmd: &str) -> Result<(Reader, CloseOnDrop), String> {
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        let (read, write) = ch.split();
        Ok((Reader { read: Some(read), seen: Seen::Running, checked: self.checked.clone() }, CloseOnDrop::new(write)))
    }

    /// A TCP connection made from the node to `host:port` (an SSH local port forward). Not a
    /// session channel: sshd's MaxSessions doesn't count it, and neither does the budget.
    pub async fn direct_tcpip(&self, host: &str, port: u16, local_port: u16) -> Result<russh::ChannelStream<client::Msg>, String> {
        let ch = self
            .handle
            .channel_open_direct_tcpip(host, port as u32, "127.0.0.1", local_port as u32)
            .await
            .map_err(|e| format!("the node refused the forward to {host}:{port}: {e}"))?;
        Ok(ch.into_stream())
    }

    /// Opens an interactive terminal on a channel `open_on` opened: a login shell, or `command`
    /// under a PTY (e.g. `docker exec -it`).
    pub async fn pty_in(&self, ch: russh::Channel<client::Msg>, cols: u32, rows: u32, command: Option<&str>) -> Result<(Reader, ChannelWriteHalf<client::Msg>), String> {
        ch.request_pty(false, "xterm-256color", cols, rows, 0, 0, &[]).await.map_err(|e| e.to_string())?;
        match command {
            Some(c) => ch.exec(false, c).await,
            None => ch.request_shell(false).await,
        }
        .map_err(|e| e.to_string())?;
        let (read, write) = ch.split();
        Ok((Reader { read: Some(read), seen: Seen::Running, checked: self.checked.clone() }, write))
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Ends the connection now (rather than whenever the last reference goes).
    pub async fn close(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "SparkBox disconnected", "en").await;
    }
}

/// Reads a channel to its end (or the limit, or a cancel), keeping capped output.
pub async fn collect(
    read: &mut ChannelReadHalf,
    mut write: CloseOnDrop,
    limit: Duration,
    cancel: &CancelToken,
    on_chunk: &mut (dyn FnMut(StreamKind, &[u8]) + Send),
) -> Result<Output, String> {
    let mut out = Output::default();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        tokio::select! {
            msg = read.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    on_chunk(StreamKind::Stdout, &data);
                    out.truncated |= push_capped(&mut stdout, &data);
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    on_chunk(StreamKind::Stderr, &data);
                    out.truncated |= push_capped(&mut stderr, &data);
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => out.code = Some(exit_status),
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => out.signal = Some(sig_name(&signal_name)),
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            _ = cancel.cancelled() => {
                out.cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                out.timed_out = true;
                break;
            }
        }
    }
    if out.cancelled || out.timed_out {
        // Ask the node to stop it (once), then let CloseOnDrop close the channel.
        let _ = write.get().signal(Sig::TERM).await;
        write.1 = false;
    }
    drop(write);
    out.stdout = String::from_utf8_lossy(&stdout).into_owned();
    out.stderr = String::from_utf8_lossy(&stderr).into_owned();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openssh_known_hosts_entries_vouch_for_a_key() {
        let blob = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20abcdefghijklmnopqrstuvwxyz012345";
        let key = base64::engine::general_purpose::STANDARD.encode(blob);
        let text = format!("# comment\n192.168.50.51,cell1-example ssh-ed25519 {key}\n[192.168.50.98]:2222 ssh-ed25519 {key}\n|1|hashed|entry ssh-ed25519 {key}\n");
        let fp = fingerprint_of_blob(blob);
        assert!(fp.starts_with("SHA256:") && !fp.ends_with('='));
        assert_eq!(openssh_fingerprints(&text, "192.168.50.51", 22), vec![fp.clone()]);
        assert_eq!(openssh_fingerprints(&text, "192.168.50.98", 2222), vec![fp]);
        assert!(openssh_fingerprints(&text, "192.168.50.98", 22).is_empty());
    }

    #[test]
    fn an_unreadable_known_hosts_fails_closed_until_forgotten() {
        let dir = std::env::temp_dir().join(format!("sparkbox-kh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts.json");
        trust(&path, "10.0.0.1:22", "SHA256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(load_known(&path).unwrap().len(), 1);

        std::fs::write(&path, b"{\"10.0.0.1:22\": \"SHA2").unwrap();
        let e = load_known(&path).unwrap_err();
        assert!(e.starts_with("known_hosts.json couldn't be read; the original is kept at "), "{e}");
        assert!(e.contains("Forget host keys"));
        assert!(load_known(&path).is_err(), "still refused: the file being gone doesn't mean trust everything");
        assert!(trust(&path, "10.0.0.2:22", "SHA256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").is_err());
        let backups = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).count();
        assert_eq!(backups, 1, "the original is kept");

        forget(&path).unwrap();
        assert!(load_known(&path).unwrap().is_empty());
        assert!(trust(&path, "x", "nope").is_err(), "fingerprints are checked");
    }

    #[tokio::test]
    async fn a_cancel_token_wakes_waiters() {
        let t = CancelToken::new();
        let t2 = t.clone();
        let waiter = tokio::spawn(async move { t2.cancelled().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        t.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
        assert!(t.is_cancelled());
        t.cancelled().await; // already cancelled: returns at once
    }

    /// A login the connection closed under is never a rejected password: a drop says so, and a
    /// node that hung up with a reason gives it (a login problem again, as it should read).
    #[test]
    fn a_login_the_connection_closed_under_says_why_and_is_never_a_rejection() {
        let ended = Mutex::new(Some("Disconnected".to_string()));
        let drop = closed_at_login("192.168.50.154", &ended).to_string();
        assert_eq!(drop, "The connection to 192.168.50.154 closed during the login.");
        assert_eq!(crate::live::error_kind_of(&drop), None);
        *ended.lock().unwrap() = Some("the node hung up: Too many authentication failures".into());
        let hung_up = closed_at_login("192.168.50.154", &ended).to_string();
        assert_eq!(hung_up, "192.168.50.154 ended the login: Too many authentication failures");
        assert_eq!(crate::live::error_kind_of(&hung_up), Some("authFailed"));
    }
}
