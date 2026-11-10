//! Readiness means the model answers (P1.35): a SparkBox load is Ready only once, on its head
//! node, /health is 200, /v1/models lists its served name, a one-token test comes back with its
//! key, and, the first time a recipe meets an image, "What is 17+25?" comes back as 42 (the
//! GB10 garbage-output bug answers "4 2 2 2…"). The sanity verdict is kept in sanity.json by
//! recipe (`recipe_key`): a passed one isn't asked again, a failed one is asked on the next load.
//! The question turns thinking off where the chat template has the switch; a model still thinking
//! when its eight tokens run out (gpt-oss, a template without the switch) is asked again with
//! room to answer, and one still thinking then gets no verdict: it's never called wrong for that.
//! So is a reply cut off with no 42, which can be thinking with no tags around it (DeepSeek-R1
//! without a reasoning parser) as well as garbage: only the second ask can tell them apart.
//!
//! Once it is Ready, `probe_protocols` reads /openapi.json (P1.37), up to three times, and records
//! whether the engine serves Anthropic's /v1/messages (with count_tokens) and OpenAI's
//! /v1/responses itself, kept in protocols.json by image id for P4.
//!
//! Every request runs with curl on the head node. The key, when there is one, is a header line on
//! stdin (`-H @-`), never in a command. The model list and /openapi.json are reads; the test token
//! and the sanity prompt are inference (`exit::as_use`), which look-only holds: a held check isn't a
//! failure, readiness just waits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::endpoints::{self, CanaryReply, CanaryRequest, Engine};
use super::now_ms;
use super::plan::{self, DeploySpec};
use crate::errors::{ErrorKind, SbError};
use crate::live::{audit, exit, ssh, Live};

/// The question, and the answer it must contain.
pub const SANITY_PROMPT: &str = "What is 17+25? Reply with the number only.";
pub const SANITY_ANSWER: &str = "42";
/// What to try when the image answers garbage: NVIDIA's own vLLM container.
pub const FALLBACK_IMAGE: &str = "nvcr.io/nvidia/vllm:26.05-py3";
/// The answer is kept this long (characters).
const ANSWER_CHARS: usize = 200;
/// The first ask's tokens: a model answering garbage is cut off soon.
const SANITY_TOKENS: u32 = 8;
/// A model still thinking when those ran out is asked once more with this many, and this long.
pub const THINKING_TOKENS: u32 = 512;
const THINKING_TIMEOUT_S: u64 = 300;
/// The waits before each ask for /openapi.json: a server just up can miss one.
#[cfg(not(test))]
const OPENAPI_WAITS: [Duration; 3] = [Duration::ZERO, Duration::from_secs(5), Duration::from_secs(30)];
#[cfg(test)]
const OPENAPI_WAITS: [Duration; 3] = [Duration::ZERO, Duration::from_millis(5), Duration::from_millis(30)];

/// What the last readiness check found. `ok` only when every step passed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub health: bool,
    pub models_listed: bool,
    /// The one-token test: None when it wasn't asked (an earlier step failed, or look-only held it).
    pub token: Option<bool>,
    /// The sanity prompt, when it was asked and answered.
    pub sanity: Option<Sanity>,
    pub ok: bool,
    /// Why it isn't ready, in a sentence (None when ok).
    pub why: Option<String>,
    pub at_ms: u64,
    // ── r4-ready: beyond the contract (additive) ──
    /// How long the test token took (ms).
    #[serde(default)]
    pub token_ms: Option<u64>,
    /// The line the card shows: "Ready: /health 200, gemma-3-27b-it listed, …", or `why`.
    #[serde(default)]
    pub summary: String,
}

impl Readiness {
    /// It answered wrongly: the load has failed (the containers stay for their logs).
    pub fn wrong(&self) -> bool {
        self.sanity.as_ref().is_some_and(|s| !s.ok)
    }
}

/// The sanity prompt's verdict.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sanity {
    pub ok: bool,
    /// The reply's first 200 characters.
    pub answer: String,
    pub at_ms: u64,
}

/// The native APIs an engine's /openapi.json lists (P1.37, for P4's routing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Protocols {
    /// Anthropic's /v1/messages.
    pub messages: bool,
    /// /v1/messages/count_tokens.
    pub count_tokens: bool,
    /// OpenAI's /v1/responses.
    pub responses: bool,
    pub image_id: String,
    pub checked_ms: u64,
}

// ---- Commands (C-R)

/// The key header SparkBox sends on stdin.
fn key_header(key: &str) -> Vec<u8> {
    format!("Authorization: Bearer {key}\n").into_bytes()
}

/// R-health: the /health part of `plan::probe_command`, for a caller that hasn't just asked.
pub fn health_command(host: &str, port: u16) -> String {
    format!("curl -s -o /dev/null -m 3 -w '%{{http_code}}\\n' http://{host}:{port}/health || echo 000")
}

/// R-models: the model list with the key on stdin (`with_key`), then the status on its own line.
pub fn models_command(host: &str, port: u16, with_key: bool) -> String {
    let key = if with_key { " -H @-" } else { "" };
    format!("curl -s -m 10{key} -w '\\n%{{http_code}}' http://{host}:{port}/v1/models")
}

/// R-sanity: the question at temperature 0, eight tokens at most, with thinking turned off for
/// the chat templates that have the switch (Qwen3's, GLM's; the others ignore it). The body's
/// fields are in C-R's order (serde_json would sort them).
pub fn sanity_command(host: &str, port: u16, served: &str, with_key: bool) -> String {
    sanity_asking(host, port, served, with_key, SANITY_TOKENS, 120)
}

/// R-sanity again for a model still thinking when the first ask's tokens ran out: room to finish
/// its thinking and answer, and time for that on a slow model.
pub fn thinking_command(host: &str, port: u16, served: &str, with_key: bool) -> String {
    sanity_asking(host, port, served, with_key, THINKING_TOKENS, THINKING_TIMEOUT_S)
}

fn sanity_asking(host: &str, port: u16, served: &str, with_key: bool, max_tokens: u32, timeout_s: u64) -> String {
    let text = |s: &str| Value::String(s.to_string()).to_string();
    let body = format!(
        "{{\"model\":{},\"messages\":[{{\"role\":\"user\",\"content\":{}}}],\"max_tokens\":{max_tokens},\"temperature\":0,\"chat_template_kwargs\":{{\"enable_thinking\":false}}}}",
        text(served),
        text(SANITY_PROMPT)
    );
    let key = if with_key { " -H @-" } else { "" };
    format!(
        "curl -s -m {timeout_s}{key} -H 'Content-Type: application/json' -d {} -w '\\n%{{http_code}}' http://{host}:{port}/v1/chat/completions",
        crate::live::sh_quote(&body)
    )
}

/// R-openapi: vLLM's API description (not behind the key).
pub fn openapi_command(host: &str, port: u16) -> String {
    format!("curl -s -m 10 -w '\\n%{{http_code}}' http://{host}:{port}/openapi.json")
}

// ---- Parsers

/// (body, status) from curl's `-w '\n%{http_code}'`; 0 when curl never got an answer.
fn split_status(out: &str) -> (&str, u16) {
    let t = out.trim_end();
    match t.rsplit_once('\n') {
        Some((body, code)) => (body, code.trim().parse().unwrap_or(0)),
        None => ("", t.trim().parse().unwrap_or(0)),
    }
}

/// The output of a request that ran to its end on the node, or why it didn't: the exit refused
/// it (look-only), the exec failed, or the connection went (or it was killed) before an exit
/// status came back.
fn finished(r: Result<ssh::Output, String>) -> Result<String, SbError> {
    let o = r.map_err(|e| SbError::classify(&e))?;
    match (o.code, o.signal.as_deref()) {
        (_, Some(sig)) => Err(SbError::classify(&format!("it was killed on the node (SIG{sig})"))),
        (None, None) => Err(SbError::classify("the connection closed before it answered")),
        _ => Ok(o.stdout),
    }
}

/// What /v1/models said about the served name.
#[derive(Debug, PartialEq)]
pub enum Listed {
    Yes,
    No,
    /// 401/403: it wants a key SparkBox didn't send, or refused the one it sent.
    Refused(u16),
    /// No answer, or an error status.
    Failed(String),
}

pub fn parse_models(out: &str, served: &str) -> Listed {
    match split_status(out) {
        (body, 200) => {
            let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let (ids, _) = endpoints::parse_models(&v);
            if ids.iter().any(|id| id == served) {
                Listed::Yes
            } else {
                Listed::No
            }
        }
        (_, c @ (401 | 403)) => Listed::Refused(c),
        (_, 0) => Listed::Failed("no answer".into()),
        (_, c) => Listed::Failed(format!("HTTP {c}")),
    }
}

/// What a sanity reply holds.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Its answer, without a thinking model's finished thinking.
    Answer(String),
    /// Its tokens ran out with no 42 in what it wrote: thinking with no tags around it, or garbage
    /// that runs on. Not an answer yet.
    CutOff(String),
    /// Its tokens ran out while it was still thinking, before any answer: the thinking so far.
    Thinking(String),
}

/// The sanity reply, for a 200 with a message; Err(why) otherwise. A thinking model's thinking is
/// in the content between <think> tags (no reasoning parser), or beside it in `reasoning`
/// (`reasoning_content` in older vLLM) with the content null until the answer.
pub fn parse_sanity(out: &str) -> Result<Reply, String> {
    let (body, code) = split_status(out);
    match code {
        200 => {}
        0 => return Err("no answer".into()),
        c => return Err(format!("HTTP {c}")),
    }
    let v: Value = serde_json::from_str(body).map_err(|_| "the reply wasn't JSON".to_string())?;
    let choice = &v["choices"][0];
    let m = &choice["message"];
    if !m.is_object() {
        return Err("the reply had no message".into());
    }
    let content = m["content"].as_str().unwrap_or("");
    let thought = m["reasoning"].as_str().or_else(|| m["reasoning_content"].as_str()).unwrap_or("");
    let (answer, open) = match content.rsplit_once("</think>") {
        Some((_, after)) => (after, ""),
        None => match content.trim_start().strip_prefix("<think>") {
            Some(thinking) => ("", thinking),
            None => (content, ""),
        },
    };
    let cut = choice["finish_reason"].as_str() == Some("length");
    let answer = answer.trim();
    if !answer.is_empty() {
        let text = answer.to_string();
        return Ok(if cut && !answer.contains(SANITY_ANSWER) { Reply::CutOff(text) } else { Reply::Answer(text) });
    }
    let thinking = if open.trim().is_empty() { thought } else { open }.trim().to_string();
    if cut {
        return Ok(Reply::Thinking(thinking));
    }
    // It finished with all of it as thinking: that's its reply (a parser that expects a <think>
    // the template left out, deepseek_r1's with thinking off, files the answer there).
    if !thinking.is_empty() {
        return Ok(Reply::Answer(thinking));
    }
    Err("the reply was empty".into())
}

/// The verdict on a sanity reply: it contains 42, trimmed; the first 200 characters are kept.
pub fn sanity_verdict(answer: &str, at_ms: u64) -> Sanity {
    let text = answer.trim();
    Sanity { ok: text.contains(SANITY_ANSWER), answer: text.chars().take(ANSWER_CHARS).collect(), at_ms }
}

/// Whether a reply has any words: two letters in a row. The garbage-output bug writes digits and
/// punctuation; a model thinking out loud, however long, writes sentences.
fn has_words(text: &str) -> bool {
    let mut letters = 0;
    text.chars().any(|c| {
        letters = if c.is_alphabetic() { letters + 1 } else { 0 };
        letters >= 2
    })
}

/// The native APIs in an /openapi.json reply; None when there isn't one (a 404, no answer, not JSON).
pub fn parse_openapi(out: &str, image_id: &str, at_ms: u64) -> Option<Protocols> {
    let (body, code) = split_status(out);
    if code != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(body).ok()?;
    let paths = v["paths"].as_object()?;
    Some(Protocols {
        messages: paths.contains_key("/v1/messages"),
        count_tokens: paths.contains_key("/v1/messages/count_tokens"),
        responses: paths.contains_key("/v1/responses"),
        image_id: image_id.to_string(),
        checked_ms: at_ms,
    })
}

/// A recipe, as the sanity verdict knows it: sha256 over the image's local id, the model folder,
/// the served name and vLLM's arguments without --host and --port (where it listens doesn't
/// change what it answers).
pub fn recipe_key(spec: &DeploySpec) -> String {
    let mut parts: Vec<String> = vec![spec.image_id.clone(), spec.model_path.clone(), spec.served_name.clone()];
    let args = plan::vllm_args(spec);
    let mut i = 0;
    while i < args.len() {
        if matches!(args[i].as_str(), "--host" | "--port") {
            i += 2;
            continue;
        }
        parts.push(args[i].clone());
        i += 1;
    }
    let digest = Sha256::digest(parts.join("\0").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- The words

/// "0.4 s".
fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

fn ready_line(served: &str, token_ms: Option<u64>, sanity: Option<&Sanity>) -> String {
    let token = match token_ms {
        Some(ms) => format!("a test token in {}", secs(ms)),
        None => "a test token".into(),
    };
    match sanity {
        Some(_) => format!("Ready: /health 200, {served} listed, {token}, 17+25 → 42 (first run of this recipe and image)"),
        None => format!("Ready: /health 200, {served} listed, {token} (17+25 was checked on an earlier load of this recipe and image)"),
    }
}

/// The garbage-output sentence.
pub fn wrong_line(answer: &str) -> String {
    format!("It answers, but wrongly: asked 17+25, it said '{answer}'. This is the GB10 garbage-output bug; try the fallback image ({FALLBACK_IMAGE}).")
}

// ---- The check

/// Runs R-health, R-models, R-token and (when `sanity_needed`) R-sanity on `node`, in that order,
/// stopping at the first step that doesn't pass. Ready (`ok`) only when every step passed. (The
/// load's watcher has just asked /health itself, so it calls `check_with`.)
#[cfg(test)]
pub async fn check(live: &Live, node: &str, host: &str, port: u16, served: &str, key: Option<&str>, sanity_needed: bool) -> Readiness {
    check_with(live, node, host, port, served, key, sanity_needed, None).await
}

/// `check`, taking the caller's /health status when it has just asked (follow's probe).
#[allow(clippy::too_many_arguments)]
pub async fn check_with(live: &Live, node: &str, host: &str, port: u16, served: &str, key: Option<&str>, sanity_needed: bool, health: Option<u16>) -> Readiness {
    let mut r = Readiness::default();
    let done = |mut r: Readiness, why: Option<String>| {
        r.ok = why.is_none();
        r.summary = why.clone().unwrap_or_else(|| ready_line(served, r.token_ms, r.sanity.as_ref()));
        r.why = why;
        r.at_ms = now_ms();
        r
    };
    let limit = Duration::from_secs(20);
    let stdin = key.map(key_header);

    // R-health.
    let status = match health {
        Some(s) => s,
        None => match finished(live.probe(node, &health_command(host, port), limit).await) {
            Ok(out) => out.trim().lines().last().and_then(|l| l.trim().parse().ok()).unwrap_or(0),
            Err(e) => return done(r, Some(format!("Couldn't ask {node}: {}.", e.message.trim_end_matches('.')))),
        },
    };
    if status != 200 {
        let what = if status == 0 { "no answer".to_string() } else { format!("HTTP {status}") };
        return done(r, Some(format!("It doesn't answer /health yet ({what}).")));
    }
    r.health = true;

    // R-models: a read, with the key on stdin.
    let cmd = models_command(host, port, key.is_some());
    let out = match finished(audit::as_probe(live.run(node, &cmd, stdin.as_deref(), limit)).await) {
        Ok(out) => out,
        Err(e) => return done(r, Some(format!("Couldn't ask {node} for its model list: {}.", e.message.trim_end_matches('.')))),
    };
    match parse_models(&out, served) {
        Listed::Yes => r.models_listed = true,
        Listed::No => return done(r, Some(format!("It answers /health, but /v1/models doesn't list {served}."))),
        Listed::Refused(c) => return done(r, Some(format!("It answers /health, but /v1/models refused SparkBox's key (HTTP {c})."))),
        Listed::Failed(why) => return done(r, Some(format!("It answers /health, but /v1/models failed: {why}."))),
    }

    // R-token: inference, so look-only holds it.
    let req = CanaryRequest { model: served.to_string(), chat: false, keep_alive_s: None };
    match token(live, node, host, port, &req, stdin.as_deref()).await {
        Ok(ms) => {
            r.token = Some(true);
            r.token_ms = ms;
        }
        Err(Held) => return done(r, Some(HELD.into())),
        Err(Failed(why)) => {
            r.token = Some(false);
            return done(r, Some(format!("The test token failed: {why}.")));
        }
    }

    // R-sanity, the first time this recipe meets this image (or after a wrong answer). A reply cut
    // off with no 42, or still thinking, when the first ask's tokens ran out is asked again with
    // room to answer, and the second reply decides. One still cut off then is wrong only with no
    // words in it; otherwise it's a model still thinking: no verdict (it's asked at the next
    // check), never a wrong one.
    if sanity_needed {
        let with_key = key.is_some();
        let asks = [(sanity_command(host, port, served, with_key), 130), (thinking_command(host, port, served, with_key), THINKING_TIMEOUT_S + 10)];
        let mut thinking = String::new();
        // The first cut-off reply, the short one to quote, and whether the last reply was cut off.
        let (mut quote, mut cut) = (None, false);
        for (cmd, limit) in asks {
            let out = match finished(exit::as_use(audit::as_probe(live.run(node, &cmd, stdin.as_deref(), Duration::from_secs(limit)))).await) {
                Ok(out) => out,
                Err(e) if e.kind == ErrorKind::LookOnly => return done(r, Some(HELD.into())),
                Err(e) => return done(r, Some(format!("The 17+25 check failed: {}.", e.message.trim_end_matches('.')))),
            };
            match parse_sanity(&out) {
                Ok(Reply::Answer(answer)) => {
                    let verdict = sanity_verdict(&answer, now_ms());
                    let wrong = (!verdict.ok).then(|| wrong_line(&verdict.answer));
                    r.sanity = Some(verdict);
                    return done(r, wrong);
                }
                Ok(Reply::CutOff(text)) => {
                    quote.get_or_insert_with(|| text.clone());
                    cut = true;
                    thinking = text;
                }
                Ok(Reply::Thinking(so_far)) => {
                    cut = false;
                    thinking = so_far;
                }
                Err(why) => return done(r, Some(format!("The 17+25 check failed: {why}."))),
            }
        }
        if cut && !has_words(&thinking) {
            let verdict = sanity_verdict(quote.as_deref().unwrap_or(&thinking), now_ms());
            let wrong = wrong_line(&verdict.answer);
            r.sanity = Some(verdict);
            return done(r, Some(wrong));
        }
        return done(r, Some(thinking_line(&thinking)));
    }
    done(r, None)
}

/// A model that was still thinking after THINKING_TOKENS: no verdict.
fn thinking_line(so_far: &str) -> String {
    let quoted: String = so_far.chars().take(ANSWER_CHARS).collect();
    let tail = if quoted.is_empty() { String::new() } else { format!("; it was still thinking ('{quoted}')") };
    format!("Asked 17+25, it gave no answer within {THINKING_TOKENS} tokens{tail}. It's asked again at the next check.")
}

/// What look-only says while it holds the test token or the sanity prompt.
pub const HELD: &str = "Look-only mode holds the test token; it's checked once look-only ends.";

enum TokenError {
    Held,
    Failed(String),
}
use TokenError::{Failed, Held};

/// One test token (endpoints::canary_command), to /v1/chat/completions if completions isn't served.
async fn token(live: &Live, node: &str, host: &str, port: u16, req: &CanaryRequest, stdin: Option<&[u8]>) -> Result<Option<u64>, TokenError> {
    let mut req = req.clone();
    let limit = Duration::from_secs(u64::from(endpoints::CANARY_TIMEOUT_S) + 10);
    for _ in 0..2 {
        let cmd = endpoints::canary_command(Engine::Vllm, host, port, &req, stdin.is_some());
        let out = match finished(exit::as_use(audit::as_probe(live.run(node, &cmd, stdin, limit))).await) {
            Ok(out) => out,
            Err(e) if e.kind == ErrorKind::LookOnly => return Err(Held),
            Err(e) => return Err(Failed(e.message.trim_end_matches('.').to_string())),
        };
        match endpoints::parse_canary(Engine::Vllm, &out, now_ms()) {
            CanaryReply::Done(c) if c.ok => return Ok(c.latency_ms),
            CanaryReply::Done(c) => return Err(Failed(c.error.unwrap_or_else(|| "no token came back".into()))),
            CanaryReply::ChatOnly if !req.chat => req.chat = true,
            CanaryReply::ChatOnly => return Err(Failed("it serves neither completions nor chat".into())),
            CanaryReply::NeedsKey => return Err(Failed("it refused SparkBox's key".into())),
        }
    }
    Err(Failed("it serves neither completions nor chat".into()))
}

/// Reads /openapi.json on the head node once the load is Ready (a read, no key), up to three
/// times (OPENAPI_WAITS): an engine that has no such document says so with a 404, and that's its
/// answer. None then, or when it never answered: the load stays Ready either way.
pub async fn probe_protocols(live: &Live, node: &str, host: &str, port: u16, image_id: &str) -> Option<Protocols> {
    for wait in OPENAPI_WAITS {
        tokio::time::sleep(wait).await;
        let Ok(out) = finished(audit::as_probe(live.run(node, &openapi_command(host, port), None, Duration::from_secs(20))).await) else { continue };
        if matches!(split_status(&out).1, 200 | 404) {
            return parse_openapi(&out, image_id, now_ms());
        }
    }
    None
}

// ---- Stores (in the models data folder)

/// Sanity verdicts by recipe (sanity.json).
pub struct SanityStore {
    path: PathBuf,
    verdicts: Mutex<BTreeMap<String, Sanity>>,
}

impl SanityStore {
    pub fn open(dir: &Path) -> SanityStore {
        let path = dir.join("sanity.json");
        let verdicts = crate::util::load_json_or_default(&path);
        SanityStore { path, verdicts: Mutex::new(verdicts) }
    }

    /// True unless a passed verdict is kept for this recipe.
    pub fn needs(&self, key: &str) -> bool {
        !self.verdicts.lock().unwrap().get(key).is_some_and(|s| s.ok)
    }

    /// Keeps the verdict (a failed one too, so the next load asks again and says it failed before).
    pub fn record(&self, key: &str, verdict: &Sanity) {
        let mut v = self.verdicts.lock().unwrap();
        v.insert(key.to_string(), verdict.clone());
        if let Err(e) = crate::util::save_json(&self.path, &*v) {
            eprintln!("{e}");
        }
    }
}

/// What each image's engine offers natively, by local image id (protocols.json), for P4.
pub struct ProtocolStore {
    path: PathBuf,
    by_image: Mutex<BTreeMap<String, Protocols>>,
}

impl ProtocolStore {
    pub fn open(dir: &Path) -> ProtocolStore {
        let path = dir.join("protocols.json");
        let by_image = crate::util::load_json_or_default(&path);
        ProtocolStore { path, by_image: Mutex::new(by_image) }
    }

    pub fn record(&self, p: &Protocols) {
        let mut m = self.by_image.lock().unwrap();
        m.insert(p.image_id.clone(), p.clone());
        if let Err(e) = crate::util::save_json(&self.path, &*m) {
            eprintln!("{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, temp};
    use crate::models::fake_vllm::{self, Knobs};

    const KEY: &str = "sbk-readiness-test";

    #[test]
    fn the_commands_are_c_r_byte_for_byte() {
        let host = "192.168.50.154";
        assert_eq!(health_command(host, 8000), "curl -s -o /dev/null -m 3 -w '%{http_code}\\n' http://192.168.50.154:8000/health || echo 000");
        assert!(plan::probe_command("sparkbox-d1", host, 8000).contains(&health_command(host, 8000)), "R-health is today's probe's /health");
        assert_eq!(models_command(host, 8000, true), "curl -s -m 10 -H @- -w '\\n%{http_code}' http://192.168.50.154:8000/v1/models");
        assert_eq!(
            sanity_command(host, 8000, "gemma-3-27b-it", true),
            r#"curl -s -m 120 -H @- -H 'Content-Type: application/json' -d '{"model":"gemma-3-27b-it","messages":[{"role":"user","content":"What is 17+25? Reply with the number only."}],"max_tokens":8,"temperature":0,"chat_template_kwargs":{"enable_thinking":false}}' -w '\n%{http_code}' http://192.168.50.154:8000/v1/chat/completions"#
        );
        // Asked again, only when the first ask's tokens ran out while it was thinking.
        assert_eq!(
            thinking_command(host, 8000, "gpt-oss-120b", true),
            r#"curl -s -m 300 -H @- -H 'Content-Type: application/json' -d '{"model":"gpt-oss-120b","messages":[{"role":"user","content":"What is 17+25? Reply with the number only."}],"max_tokens":512,"temperature":0,"chat_template_kwargs":{"enable_thinking":false}}' -w '\n%{http_code}' http://192.168.50.154:8000/v1/chat/completions"#
        );
        assert_eq!(openapi_command(host, 8000), "curl -s -m 10 -w '\\n%{http_code}' http://192.168.50.154:8000/openapi.json");
        // Without a key, nothing is read from stdin.
        assert!(!models_command(host, 8000, false).contains("@-") && !sanity_command(host, 8000, "x", false).contains("@-"));
    }

    #[test]
    fn replies_are_read_as_vllm_writes_them() {
        let list = "{\"object\":\"list\",\"data\":[{\"id\":\"gemma-3-27b-it\",\"object\":\"model\",\"max_model_len\":32768}]}\n200";
        assert_eq!(parse_models(list, "gemma-3-27b-it"), Listed::Yes);
        assert_eq!(parse_models(list, "qwen3"), Listed::No);
        assert_eq!(parse_models("{\"error\":\"Unauthorized\"}\n401", "gemma-3-27b-it"), Listed::Refused(401));
        assert_eq!(parse_models("\n000", "gemma-3-27b-it"), Listed::Failed("no answer".into()));
        assert_eq!(parse_models("Internal Server Error\n500", "gemma-3-27b-it"), Listed::Failed("HTTP 500".into()));

        let chat = |content: &str| format!("{}\n200", serde_json::json!({ "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] }));
        assert_eq!(parse_sanity(&chat("42")), Ok(Reply::Answer("42".into())));
        assert_eq!(parse_sanity("{}\n200"), Err("the reply had no message".into()));
        assert_eq!(parse_sanity("<html>\n502"), Err("HTTP 502".into()));
        assert_eq!(parse_sanity("\n000"), Err("no answer".into()));
        // Cut off with no 42: the garbage-output bug running on, or thinking with no tags around
        // it. Neither is an answer yet; with a 42 in it, it is.
        let cut = |message: Value| format!("{}\n200", serde_json::json!({ "choices": [{ "index": 0, "message": message, "finish_reason": "length" }] }));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": "4 2 2 2 2 2 2 2", "reasoning_content": null }))), Ok(Reply::CutOff("4 2 2 2 2 2 2 2".into())));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": "Okay, so I need to add 17", "reasoning_content": null }))), Ok(Reply::CutOff("Okay, so I need to add 17".into())));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": "17+25 = 42, so the answer", "reasoning_content": null }))), Ok(Reply::Answer("17+25 = 42, so the answer".into())));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": "Okay.\n</think>\n\nThe ans" }))), Ok(Reply::CutOff("The ans".into())));
        // A reply that ended by itself is an answer, right or wrong.
        assert_eq!(parse_sanity(&chat("41")), Ok(Reply::Answer("41".into())));
        // Thinking models, as vLLM returns them. Qwen3 without a reasoning parser, cut off while
        // thinking, and finished: the thinking isn't the answer.
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": "<think>\nOkay, so I need to add", "reasoning_content": null }))), Ok(Reply::Thinking("Okay, so I need to add".into())));
        assert_eq!(parse_sanity(&chat("<think>\nOkay, 17+25.\n</think>\n\n42")), Ok(Reply::Answer("42".into())));
        assert_eq!(parse_sanity(&chat("Okay, 17+25.\n</think>\n\n42")), Ok(Reply::Answer("42".into())), "the template wrote the <think>");
        // gpt-oss, or a reasoning parser: the thinking beside a null content (reasoning_content in
        // older vLLM, reasoning since), cut off and finished.
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": null, "reasoning_content": "We need to compute 17+25" }))), Ok(Reply::Thinking("We need to compute 17+25".into())));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": null, "reasoning": "We need to compute" }))), Ok(Reply::Thinking("We need to compute".into())));
        assert_eq!(parse_sanity(&cut(serde_json::json!({ "content": null }))), Ok(Reply::Thinking(String::new())));
        let done = |message: Value| format!("{}\n200", serde_json::json!({ "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }] }));
        assert_eq!(parse_sanity(&done(serde_json::json!({ "content": "42", "reasoning": "17+25 = 42." }))), Ok(Reply::Answer("42".into())));
        // deepseek_r1's parser with thinking off files the whole reply as thinking.
        assert_eq!(parse_sanity(&done(serde_json::json!({ "content": null, "reasoning_content": "42" }))), Ok(Reply::Answer("42".into())));
        assert_eq!(parse_sanity(&done(serde_json::json!({ "content": "" }))), Err("the reply was empty".into()));
        assert_eq!(
            thinking_line("We need to compute 17+25"),
            "Asked 17+25, it gave no answer within 512 tokens; it was still thinking ('We need to compute 17+25'). It's asked again at the next check."
        );
        assert_eq!(thinking_line(""), "Asked 17+25, it gave no answer within 512 tokens. It's asked again at the next check.");
        for garbage in ["4 2 2 2 2 2 2 2", "!!!!!!!!", "a 4 b 2 c", ""] {
            assert!(!has_words(garbage), "{garbage}");
        }
        for words in ["Okay, so I need", "17 and 25 to", "我需要 17"] {
            assert!(has_words(words), "{words}");
        }
        assert!(sanity_verdict(" 42\n", 1).ok);
        assert!(sanity_verdict("The answer is 42.", 1).ok);
        let garbage = sanity_verdict("4 2 2 2 2 2 2 2", 7);
        assert_eq!(garbage, Sanity { ok: false, answer: "4 2 2 2 2 2 2 2".into(), at_ms: 7 });
        assert_eq!(sanity_verdict(&"x".repeat(500), 1).answer.len(), 200, "the first 200 characters are kept");

        let doc = |paths: &[&str]| format!("{}\n200", serde_json::json!({ "openapi": "3.1.0", "paths": paths.iter().map(|p| (p.to_string(), serde_json::json!({}))).collect::<serde_json::Map<_, _>>() }));
        let p = parse_openapi(&doc(&["/v1/chat/completions", "/v1/messages", "/v1/messages/count_tokens", "/v1/responses"]), "sha256:ab", 5).unwrap();
        assert_eq!(p, Protocols { messages: true, count_tokens: true, responses: true, image_id: "sha256:ab".into(), checked_ms: 5 });
        let p = parse_openapi(&doc(&["/v1/chat/completions", "/v1/completions", "/v1/models"]), "sha256:ab", 5).unwrap();
        assert!(!p.messages && !p.count_tokens && !p.responses);
        assert_eq!(parse_openapi("{\"detail\":\"Not Found\"}\n404", "sha256:ab", 5), None);
        assert_eq!(parse_openapi("not json\n200", "sha256:ab", 5), None);
        assert_eq!(parse_openapi("\n000", "sha256:ab", 5), None);
    }

    #[test]
    fn a_recipe_is_its_image_folder_name_and_arguments_not_where_it_listens() {
        let spec = DeploySpec {
            id: "d1".into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: "/home/sparkuser/models/google/gemma-3-27b-it".into(),
            nodes: vec!["cell2".into()],
            max_model_len: Some(32768),
            gpu_memory_utilization: 0.75,
            port: 8000,
            host: "192.168.50.154".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            ..Default::default()
        };
        let key = recipe_key(&spec);
        assert_eq!(key.len(), 64);
        assert_eq!(recipe_key(&DeploySpec { port: 8001, host: "127.0.0.1".into(), id: "d2".into(), ..spec.clone() }), key, "the port, the address and the load's id don't count");
        assert_ne!(recipe_key(&DeploySpec { image_id: format!("sha256:{}", "b".repeat(64)), ..spec.clone() }), key, "a new image is a new recipe");
        assert_ne!(recipe_key(&DeploySpec { max_model_len: Some(8192), ..spec.clone() }), key);
        assert_ne!(recipe_key(&DeploySpec { served_name: "gemma".into(), ..spec.clone() }), key);
        assert_ne!(recipe_key(&DeploySpec { model_path: "/home/sparkuser/models/google/gemma-3-12b-it".into(), ..spec.clone() }), key);
    }

    #[test]
    fn the_stores_keep_verdicts_and_protocols_across_restarts() {
        let dir = temp("readiness-stores");
        let s = SanityStore::open(&dir);
        assert!(s.needs("r1"), "never asked");
        s.record("r1", &Sanity { ok: false, answer: "4 2 2".into(), at_ms: 1 });
        assert!(s.needs("r1"), "a wrong answer is asked again next time");
        s.record("r1", &Sanity { ok: true, answer: "42".into(), at_ms: 2 });
        assert!(!s.needs("r1"));
        assert!(s.needs("r2"));
        let again = SanityStore::open(&dir);
        assert!(!again.needs("r1"), "kept in sanity.json");
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sanity.json")).unwrap()).unwrap();
        assert_eq!(saved["r1"], serde_json::json!({ "ok": true, "answer": "42", "atMs": 2 }));

        let p = ProtocolStore::open(&dir);
        let found = Protocols { messages: true, count_tokens: false, responses: true, image_id: "sha256:ab".into(), checked_ms: 9 };
        p.record(&found);
        let other = Protocols { image_id: "sha256:cd".into(), ..Default::default() };
        ProtocolStore::open(&dir).record(&other);
        let saved: std::collections::BTreeMap<String, Protocols> = serde_json::from_str(&std::fs::read_to_string(dir.join("protocols.json")).unwrap()).unwrap();
        assert_eq!(saved.get("sha256:ab"), Some(&found), "kept by image id across a restart");
        assert_eq!(saved.get("sha256:cd"), Some(&other));
        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("protocols.json")).unwrap()).unwrap();
        assert_eq!(raw["sha256:ab"]["countTokens"], false);
    }

    /// A fake cell2 with the stand-in, connected; the load started with its key (C-L).
    async fn loaded(name: &str, knobs: Knobs, key: bool) -> (fake_node::Node, fake_vllm::Standin, std::sync::Arc<Live>, DeploySpec) {
        let home = temp(&format!("ready-{name}"));
        let (node, port, s) = fake_vllm::node(&home, knobs).await;
        let (live, _) = fake_node::connect(&temp(&format!("ready-{name}-app")), &[("cell2", port)]).await;
        let spec = fake_vllm::gemma(&home, name, key);
        fake_vllm::launch_files(&live, "cell2", &spec, 0, &[], key.then_some(KEY)).await;
        let out = live.run("cell2", &plan::rank_command(&spec, 0), None, Duration::from_secs(60)).await.unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        (node, s, live, spec)
    }

    async fn ready(live: &Live, spec: &DeploySpec, key: Option<&str>, sanity: bool) -> Readiness {
        check(live, "cell2", "127.0.0.1", spec.port, &spec.served_name, key, sanity).await
    }

    #[tokio::test]
    async fn ready_only_once_it_answers_every_check() {
        let (node, _s, live, spec) = loaded("all4", Knobs { warmup_s: 3.0, ..Knobs::default() }, true).await;
        // Warming up: /health is 503, and nothing past it is asked.
        for _ in 0..50 {
            if fake_vllm::status(&live, spec.port, "/health").await == "503" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(!r.ok && !r.health && r.token.is_none(), "{r:?}");
        assert_eq!(r.why.as_deref(), Some("It doesn't answer /health yet (HTTP 503)."));
        assert!(node.ran("/v1/models").is_empty());
        fake_vllm::until_healthy(&live, spec.port).await;

        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(r.ok && r.health && r.models_listed && r.token == Some(true), "{r:?}");
        assert_eq!(r.sanity.as_ref().map(|s| (s.ok, s.answer.as_str())), Some((true, "42")));
        assert!(r.why.is_none());
        let ms = r.token_ms.expect("curl's time");
        assert_eq!(r.summary, format!("Ready: /health 200, gemma-3-27b-it listed, a test token in {}, 17+25 → 42 (first run of this recipe and image)", secs(ms)));
        // The order, and the key only ever on stdin.
        let log = node.log.lock().unwrap().clone();
        let at = |p: &str| log.iter().position(|c| c.contains(p)).unwrap_or_else(|| panic!("{p} never asked: {log:?}"));
        assert!(at("/health") < at("/v1/models") && at("/v1/models") < at("/v1/completions") && at("/v1/completions") < at("17+25"));
        assert!(!log.iter().any(|c| c.contains(KEY)), "no key in a command");
        let sent: Vec<_> = node.stdin.lock().unwrap().iter().filter(|(c, _)| c.contains("/v1/")).map(|(_, i)| String::from_utf8_lossy(i).into_owned()).collect();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|i| i == &format!("Authorization: Bearer {KEY}\n")), "{sent:?}");
        assert!(!live.audit.entries().iter().any(|e| e.command.contains(KEY)));

        // Asked before: the sanity prompt isn't sent again.
        let r = ready(&live, &spec, Some(KEY), false).await;
        assert!(r.ok && r.sanity.is_none(), "{r:?}");
        assert!(r.summary.ends_with("(17+25 was checked on an earlier load of this recipe and image)"), "{}", r.summary);
        assert_eq!(node.ran("17+25").len(), 1);
        live.stop();
    }

    #[tokio::test]
    async fn a_wrong_answer_fails_the_load_and_is_asked_again_next_time() {
        let (node, s, live, spec) = loaded("garbage", Knobs { garbage: true, ..Knobs::default() }, true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let store = SanityStore::open(&temp("ready-garbage-store"));
        let key = recipe_key(&spec);
        assert!(store.needs(&key));
        let r = ready(&live, &spec, Some(KEY), store.needs(&key)).await;
        assert!(!r.ok && r.wrong() && r.token == Some(true), "{r:?}");
        assert_eq!(r.why.as_deref(), Some("It answers, but wrongly: asked 17+25, it said '4 2 2 2 2 2 2 2'. This is the GB10 garbage-output bug; try the fallback image (nvcr.io/nvidia/vllm:26.05-py3)."));
        store.record(&key, r.sanity.as_ref().unwrap());
        assert!(store.needs(&key), "checked again on the next load");

        // The next load of the same recipe, on an image that answers right: asked again, and kept.
        live.run("cell2", &plan::unload_command("garbage"), None, Duration::from_secs(60)).await.unwrap();
        s.set_knobs(Knobs::default());
        fake_vllm::launch_files(&live, "cell2", &spec, 0, &[], Some(KEY)).await;
        live.run("cell2", &plan::rank_command(&spec, 0), None, Duration::from_secs(60)).await.unwrap();
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = ready(&live, &spec, Some(KEY), store.needs(&key)).await;
        assert!(r.ok, "{r:?}");
        store.record(&key, r.sanity.as_ref().unwrap());
        assert!(!store.needs(&key));
        assert_eq!(node.ran("17+25").len(), 3, "twice for the garbage, which ran on both times, once for the right answer");
        // A new image id is a new recipe: asked again.
        assert!(store.needs(&recipe_key(&DeploySpec { image_id: format!("sha256:{}", "c".repeat(64)), ..spec.clone() })));
        live.stop();
    }

    /// Thinking models on the stand-in: Qwen3 turns thinking off when asked and answers at once;
    /// one that thinks anyway, inline or beside the content (gpt-oss), is asked again with room
    /// to answer; one still thinking then gets no verdict. None is called wrong for thinking.
    #[tokio::test]
    async fn a_thinking_model_is_asked_again_and_never_called_wrong_for_thinking() {
        use fake_vllm::Thinks;
        for (name, how, tokens, asks, ok) in [("qwen3", Thinks::Inline, 40, 1, true), ("always", Thinks::InlineAlways, 40, 2, true), ("gptoss", Thinks::Parsed, 40, 2, true), ("forever", Thinks::Parsed, 600, 2, false)] {
            let (node, _s, live, spec) = loaded(&format!("think-{name}"), Knobs { thinks: Some((how, tokens)), ..Knobs::default() }, true).await;
            fake_vllm::until_healthy(&live, spec.port).await;
            let r = ready(&live, &spec, Some(KEY), true).await;
            let want = [sanity_command("127.0.0.1", spec.port, &spec.served_name, true), thinking_command("127.0.0.1", spec.port, &spec.served_name, true)];
            assert_eq!(node.ran("17+25"), want[..asks], "{name}");
            assert!(!r.wrong(), "{name}: {r:?}");
            if ok {
                assert!(r.ok && r.sanity.as_ref().is_some_and(|s| s.ok && s.answer == "42"), "{name}: {r:?}");
                assert!(r.summary.ends_with(", 17+25 → 42 (first run of this recipe and image)"), "{name}: {}", r.summary);
            } else {
                assert!(!r.ok && r.sanity.is_none() && r.token == Some(true), "no verdict to keep: {r:?}");
                let why = r.why.unwrap();
                assert!(why.starts_with("Asked 17+25, it gave no answer within 512 tokens; it was still thinking ('Okay, so I need to add 17 and 25.Okay,") && why.ends_with("'). It's asked again at the next check."), "{why}");
            }
            // The key went on stdin for each ask, and each was inference (held in look-only).
            let sent = node.stdin.lock().unwrap().iter().filter(|(c, _)| c.contains("17+25")).map(|(_, i)| String::from_utf8_lossy(i).into_owned()).collect::<Vec<_>>();
            assert_eq!(sent, vec![format!("Authorization: Bearer {KEY}\n"); asks], "{name}");
            for cmd in &want[..asks] {
                let a = live.admits().into_iter().find(|a| &a.cmd == cmd).expect("asked through the exit");
                assert_eq!((a.verdict, a.admitted), (exit::Verdict::Use, true), "{cmd}");
            }
            live.stop();
        }
    }

    /// DeepSeek-R1 without a reasoning parser: the template opens the <think>, so the thinking comes
    /// back as plain content, and eight tokens of it look like a wrong answer. It's asked again and
    /// the second reply decides; one that thinks past that too has words in it, so it gets no
    /// verdict. Garbage is still wrong: it has no words however long it runs.
    #[tokio::test]
    async fn thinking_with_no_tags_is_asked_again_and_garbage_still_fails() {
        use fake_vllm::Thinks;
        let (node, _s, live, spec) = loaded("untagged", Knobs { thinks: Some((Thinks::Untagged, 40)), ..Knobs::default() }, true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(r.ok && !r.wrong() && r.sanity.as_ref().is_some_and(|s| s.ok && s.answer == "42"), "{r:?}");
        let want = [sanity_command("127.0.0.1", spec.port, &spec.served_name, true), thinking_command("127.0.0.1", spec.port, &spec.served_name, true)];
        assert_eq!(node.ran("17+25"), want, "the second ask gave the verdict");
        live.stop();

        let (_node, _s, live, spec) = loaded("untagged-long", Knobs { thinks: Some((Thinks::Untagged, 600)), ..Knobs::default() }, true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(!r.ok && !r.wrong() && r.sanity.is_none() && r.token == Some(true), "no verdict, and not a wrong one: {r:?}");
        let why = r.why.unwrap();
        assert!(why.starts_with("Asked 17+25, it gave no answer within 512 tokens; it was still thinking ('Okay, so I need to add 17 and 25.Okay,") && why.ends_with("'). It's asked again at the next check."), "{why}");
        live.stop();

        let (node, _s, live, spec) = loaded("garbage-twice", Knobs { garbage: true, ..Knobs::default() }, true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(!r.ok && r.wrong(), "{r:?}");
        assert_eq!(r.why.as_deref(), Some(wrong_line("4 2 2 2 2 2 2 2").as_str()), "it quotes the short reply");
        assert_eq!(r.sanity.as_ref().map(|s| s.answer.as_str()), Some("4 2 2 2 2 2 2 2"));
        assert_eq!(node.ran("17+25"), [sanity_command("127.0.0.1", spec.port, &spec.served_name, true), thinking_command("127.0.0.1", spec.port, &spec.served_name, true)]);
        live.stop();
    }

    #[tokio::test]
    async fn without_the_key_the_model_list_is_refused() {
        let (node, _s, live, spec) = loaded("nokey", Knobs::default(), true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = ready(&live, &spec, None, true).await;
        assert!(!r.ok && r.health && !r.models_listed && r.token.is_none(), "{r:?}");
        assert_eq!(r.why.as_deref(), Some("It answers /health, but /v1/models refused SparkBox's key (HTTP 401)."));
        assert!(node.ran("/v1/completions").is_empty());
        // A load without a key needs none.
        let (_n2, _s2, live2, open) = loaded("open", Knobs::default(), false).await;
        fake_vllm::until_healthy(&live2, open.port).await;
        assert!(ready(&live2, &open, None, true).await.ok);
        live.stop();
        live2.stop();
    }

    #[tokio::test]
    async fn a_model_that_isnt_listed_is_not_ready() {
        let (_node, _s, live, spec) = loaded("unlisted", Knobs::default(), false).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let r = check(&live, "cell2", "127.0.0.1", spec.port, "gemma-3-12b-it", None, true).await;
        assert!(!r.ok && r.health && !r.models_listed);
        assert_eq!(r.why.as_deref(), Some("It answers /health, but /v1/models doesn't list gemma-3-12b-it."));
        live.stop();
    }

    #[tokio::test]
    async fn look_only_holds_the_token_and_the_sanity_prompt_and_readiness_waits() {
        let (node, _s, live, spec) = loaded("lookonly", Knobs::default(), true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Allow), None);
        gate.set_look_only(true).unwrap();
        live.set_write_policy(gate.clone());
        let r = ready(&live, &spec, Some(KEY), true).await;
        assert!(!r.ok && !r.wrong() && r.models_listed && r.token.is_none() && r.sanity.is_none(), "held, not failed: {r:?}");
        assert_eq!(r.why.as_deref(), Some(HELD));
        assert!(node.ran("/v1/completions").is_empty() && node.ran("17+25").is_empty(), "nothing inferred");
        // Look-only off: the same check passes.
        gate.set_look_only(false).unwrap();
        assert!(ready(&live, &spec, Some(KEY), true).await.ok);
        live.stop();
    }

    #[tokio::test]
    async fn a_node_that_drops_mid_check_is_not_ready_and_says_why() {
        use std::sync::atomic::Ordering;
        let (node, _s, live, spec) = loaded("drop", Knobs::default(), false).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        // Replies come slowly, and the node goes away while one is on its way.
        node.knobs.slow_ms.store(600, Ordering::SeqCst);
        let asking = {
            let (live, port) = (live.clone(), spec.port);
            tokio::spawn(async move { check(&live, "cell2", "127.0.0.1", port, "gemma-3-27b-it", None, true).await })
        };
        tokio::time::sleep(Duration::from_millis(250)).await;
        node.drop_connections().await;
        let r = asking.await.unwrap();
        assert!(!r.ok && !r.wrong() && r.token.is_none() && r.sanity.is_none(), "not ready, and no verdict: {r:?}");
        assert!(r.why.as_deref().is_some_and(|w| w.starts_with("Couldn't ask cell2")), "{r:?}");
        // Back: the next check passes.
        node.knobs.slow_ms.store(0, Ordering::SeqCst);
        let mut r = Readiness::default();
        for _ in 0..100 {
            r = ready(&live, &spec, None, true).await;
            if r.ok {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(r.ok, "{r:?}");
        live.stop();
    }

    #[tokio::test]
    async fn the_protocol_probe_reads_openapi_without_the_key() {
        let (node, s, live, spec) = loaded("openapi", Knobs::default(), true).await;
        fake_vllm::until_healthy(&live, spec.port).await;
        let p = probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await.unwrap();
        assert!(p.messages && p.count_tokens && p.responses && p.image_id == spec.image_id && p.checked_ms > 0, "{p:?}");
        let (cmd, stdin) = node.stdin.lock().unwrap().iter().find(|(c, _)| c.contains("/openapi.json")).cloned().unwrap();
        assert_eq!(cmd, openapi_command("127.0.0.1", spec.port));
        assert!(stdin.is_empty(), "no key sent");

        // An engine without the native routes, and one without the document at all.
        for (paths, want) in [(vec!["/v1/chat/completions", "/v1/completions", "/v1/models"], Some((false, false, false))), (vec![], None)] {
            live.run("cell2", &plan::unload_command("openapi"), None, Duration::from_secs(60)).await.unwrap();
            s.set_knobs(Knobs { openapi: paths.iter().map(|p| p.to_string()).collect(), ..Knobs::default() });
            fake_vllm::launch_files(&live, "cell2", &spec, 0, &[], Some(KEY)).await;
        live.run("cell2", &plan::rank_command(&spec, 0), None, Duration::from_secs(60)).await.unwrap();
            fake_vllm::until_healthy(&live, spec.port).await;
            let got = probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await;
            assert_eq!(got.map(|p| (p.messages, p.count_tokens, p.responses)), want);
            // A load without /openapi.json is still ready.
            assert!(ready(&live, &spec, Some(KEY), false).await.ok);
        }
        live.stop();
    }

    /// A server just up can miss a request: /openapi.json is asked again, up to three times. A 404
    /// is the engine's answer, and isn't.
    #[tokio::test]
    async fn the_protocol_probe_is_asked_again_when_it_gets_no_answer() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let misses = std::sync::Arc::new(AtomicU32::new(1));
        let left = misses.clone();
        // curl's own output for a request that got no answer, then the stand-in's.
        let flaky: fake_node::Script = std::sync::Arc::new(move |cmd, _| {
            let miss = cmd.contains("/openapi.json") && left.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
            miss.then(|| ("\n000".to_string(), 28))
        });
        let home = temp("ready-openapi-once");
        let (node, port, s) = fake_vllm::node_with(&home, Knobs::default(), flaky).await;
        let (live, _) = fake_node::connect(&temp("ready-openapi-once-app"), &[("cell2", port)]).await;
        let spec = fake_vllm::gemma(&home, "openapi-once", false);
        fake_vllm::launch_files(&live, "cell2", &spec, 0, &[], None).await;
        live.run("cell2", &plan::rank_command(&spec, 0), None, Duration::from_secs(60)).await.unwrap();
        fake_vllm::until_healthy(&live, spec.port).await;
        let asked = || node.ran("/openapi.json").len();

        let p = probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await;
        assert!(p.is_some_and(|p| p.messages && p.responses), "the second ask read it");
        assert_eq!(asked(), 2);

        // Two misses: the third ask reads it. Three: it gives up, and the load stays Ready.
        misses.store(2, Ordering::SeqCst);
        assert!(probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await.is_some());
        assert_eq!(asked(), 5);
        misses.store(3, Ordering::SeqCst);
        assert_eq!(probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await, None);
        assert_eq!(asked(), 8);

        // No such document: a 404 is an answer, asked once.
        live.run("cell2", &plan::unload_command("openapi-once"), None, Duration::from_secs(60)).await.unwrap();
        s.set_knobs(Knobs { openapi: Vec::new(), ..Knobs::default() });
        fake_vllm::launch_files(&live, "cell2", &spec, 0, &[], None).await;
        live.run("cell2", &plan::rank_command(&spec, 0), None, Duration::from_secs(60)).await.unwrap();
        fake_vllm::until_healthy(&live, spec.port).await;
        assert_eq!(probe_protocols(&live, "cell2", "127.0.0.1", spec.port, &spec.image_id).await, None);
        assert_eq!(asked(), 9);
        live.stop();
    }

    struct Allow;
    impl crate::admin::Authenticator for Allow {
        fn authenticate(&self, _reason: &str) -> Result<(), String> {
            Ok(())
        }
    }
}
