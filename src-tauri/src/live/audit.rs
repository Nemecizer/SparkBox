//! The activity log: every command SparkBox runs on a node and every change it makes on the
//! Mac (admin unlocks, saved passwords, VPN profiles) — what, where, when, who asked, how it
//! ended, and up to 64 KiB of its output. Commands that belong to one operation ("Restart 3
//! containers") share an op id so the window can group them. Demo actions are logged too,
//! marked simulated, with the command they would have run.
//!
//! Write-ahead: `begin` writes a "running" record before the command is sent and `end` appends
//! the outcome, so a command that never reports back (SparkBox quit, the connection dropped)
//! still shows, as "Didn't finish". Failures before anything ran are recorded too
//! (`record_failed`). A command SparkBox stops waiting for (the future running it is dropped)
//! never leaves its row "running": `begin_guarded`'s row ends itself as cancelled.
//!
//! A background poll runs as one operation with source "poll" (`with_op_id("poll-…", …)`
//! inside `as_probe`): the window shows its rows as one ("Check updates · 4 nodes · ok").
//!
//! Classes: `probe` (SparkBox's own tagged checks), `read` (listings, log and journal
//! followers), `session` (a shell, one entry from open to close; broadcast on and off),
//! `change`, `destructive` and `app` (a change on this Mac). Only `change`, `destructive` and
//! `app` are changes (the Changes view), plus a root shell. A command you ran (Fleet, a
//! terminal) or one run through sudo is never `read`, whatever the classifier thinks of it.
//!
//! One outcome vocabulary (`Outcome`: ok, warning, failed, cancelled) is shared with tasks.rs
//! and the toasts: a task's head line ends with the task's own outcome (`Tasks::on_finish`).
//!
//! Kept in memory for the window (the newest 5000 rows, plus the newest 2000 of SparkBox's own
//! checks and of the reads windows make outside an operation, so a day of polling never pushes
//! a change out) and appended to activity.jsonl (mode 0600) in the app's data folder. The file rotates daily or at 10 MB to activity.<time>.jsonl, and
//! rotated files whose newest entry is older than the retention set in Settings › Security &
//! privacy are deleted (at start, on each rotation and when the setting changes); there is no
//! cap on how many files that keeps. An entry that changes later is appended again; the last
//! copy wins when the files are read back. Commands are stored whole after `redact` has hidden
//! anything that looks like a secret; sudo passwords and keys never reach a command line.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

pub use crate::ops::classify::{classify, escalates, Kind};

/// Rows memory keeps for the window, besides the checks (`is_check`)...
const KEEP: usize = 5000;
/// ... and the checks: SparkBox's own (polls) and the reads windows make outside an operation.
const KEEP_CHECKS: usize = 2000;
/// At start, how much of the newest files is read back into memory.
const LOAD_BYTES: usize = 64 * 1024 * 1024;
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// Stored output per stream and entry.
const TAIL_BYTES: usize = 64 * 1024;
/// What a begun entry that never ended says once SparkBox has restarted.
pub const UNFINISHED: &str = "Didn't finish (app quit or connection lost)";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    pub ts_ms: u64,
    pub node: String,
    /// command | sudo | terminal | stream | op | app
    pub kind: String,
    pub command: String,
    pub code: Option<u32>,
    pub ms: u64,
    /// False for reads and SparkBox's own checks (status polls, listings).
    pub changes: bool,
    pub error: Option<String>,
    /// Unique per entry; a later copy with the same id replaces it.
    pub id: String,
    /// Shared by every entry of one operation.
    pub op_id: Option<String>,
    /// What the operation was for, in words: "Restart vllm-head on cell1".
    pub title: String,
    /// Where it was started: ui | tray | palette | system.
    pub source: String,
    /// Who: ui (you) | tray (the menu bar menu) | system (SparkBox on its own).
    pub actor: String,
    /// Ran as root through sudo.
    pub escalated: bool,
    /// Ran with the node's root through Docker (Docker is root-equivalent): `docker run -v /:…`,
    /// `--privileged`, `docker exec -u 0` and the like (P1.42). `escalated` covers the sudo path.
    #[serde(default)]
    pub root: bool,
    /// running | ok | warning | failed | cancelled | unfinished
    pub state: String,
    pub stdout_tail: Option<String>,
    pub stderr_tail: Option<String>,
    /// Demo mode: nothing ran; `command` is what would have.
    pub simulated: bool,
    /// probe | read | session | change | destructive | app
    pub class: String,
    /// When this copy was written; `activity_log_since` uses it.
    pub updated_ms: u64,
    /// Which data source it was recorded under (Demo or Live), so each shows only its own
    /// rows. None on rows written before round 4 (`is_demo` works it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demo: Option<bool>,
    // ── p1r1-x ──
    /// The fence class that let this change through (live/exit.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<String>,
}

/// How an operation ended, in the words tasks, op head lines and toasts share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    /// Done, but its check found a problem ("Restarted, but llama-3.3-70b didn't come back").
    Warning,
    Failed,
    /// Stopped by you part-way.
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Warning => "warning",
            Outcome::Failed => "failed",
            Outcome::Cancelled => "cancelled",
        }
    }
}

/// How a begun entry ended (`Log::end`).
#[derive(Clone, Debug, Default)]
pub struct Ending {
    /// The exit status; None when there was none (a signal, a dropped connection).
    pub code: Option<u32>,
    pub stdout_tail: Option<String>,
    pub stderr_tail: Option<String>,
    /// As measured by the caller; None takes the time since `begin`.
    pub duration_ms: Option<u64>,
    /// Why it failed, in words (timed out, killed by a signal, connection lost); with
    /// `outcome: Warning`, what the warning says.
    pub error: Option<String>,
    /// Says how it ended; when None it's worked out from `error` and `code`.
    pub outcome: Option<Outcome>,
}

impl Ending {
    /// Finished without an exit status of its own (an operation's head line).
    pub fn ok(summary: Option<String>) -> Ending {
        Ending { code: None, stdout_tail: summary, outcome: Some(Outcome::Ok), ..Default::default() }
    }

    pub fn failed(error: impl Into<String>) -> Ending {
        Ending { error: Some(error.into()), outcome: Some(Outcome::Failed), ..Default::default() }
    }

    /// An operation's head line: the outcome, its summary and what went wrong (or the warning).
    pub fn of(outcome: Outcome, summary: Option<String>, error: Option<String>) -> Ending {
        Ending { stdout_tail: summary, error, outcome: Some(outcome), ..Default::default() }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn new_id(prefix: &str) -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::fill(&mut b);
    format!("{prefix}{}", b.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Commands SparkBox runs only to look; the log hides them unless asked.
pub fn read_only(cmd: &str) -> bool {
    !matches!(classify(cmd), Kind::Change | Kind::Destructive)
}

// ---- Redaction (src/lib/redact.ts applies the same rules in the UI; both are tested
// against src/lib/redact_cases.json)

pub const REDACTED: &str = "‹redacted›";

struct Patterns {
    pem: Regex,
    curl_user: Regex,
    named_secret_flag: Regex,
    aws: Regex,
    slack: Regex,
    webhook: Regex,
    signed: Regex,
    ini: Regex,
    ntfy: Regex,
    assignment: Regex,
    json_secret: Regex,
    py_secret: Regex,
    header: Regex,
    bearer: Regex,
    msp_token: Regex,
    secret_flag: Regex,
    mysql_p: Regex,
    docker_p: Regex,
    url_password: Regex,
    hf: Regex,
    sk: Regex,
    sbk: Regex,
    github: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // A private key from its BEGIN line through its END line, or to the end when cut off.
        // PGP's ends in "PRIVATE KEY BLOCK".
        pem: Regex::new(r"(?s)-----BEGIN ([A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?)-----(.*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|.*)").unwrap(),
        // curl -u user:password (the user stays; without a password curl asks, so nothing to hide).
        curl_user: Regex::new(r#"\b(curl\b[^\n;|&]*?\s(?:-u|--user)(?:\s+|=))('[^']*'|"[^"]*"|[^\s'"]+)"#).unwrap(),
        // --db-password=…, --proxy-password …, --client-secret …, --access-token …
        named_secret_flag: Regex::new(r#"(--(?:[a-z0-9]+-)+(?:password|passwd|secret|token))([= ])('[^']*'|"[^"]*"|[^\s'"]+)"#).unwrap(),
        aws: Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").unwrap(),
        slack: Regex::new(r"(https?://hooks\.slack\.com/services/)[A-Za-z0-9/_-]+").unwrap(),
        // Discord, Teams (Office 365 connectors) and Google Chat webhooks, and Healthchecks
        // ping URLs: the path is the secret.
        webhook: Regex::new(r#"(https?://(?:(?:(?:canary|ptb)\.)?discord(?:app)?\.com/api/webhooks/|[A-Za-z0-9.-]+\.webhook\.office\.com/webhookb2/|outlook\.office(?:365)?\.com/webhook/|chat\.googleapis\.com/v1/spaces/|hc-ping\.com/))[^\s'"]+"#).unwrap(),
        // Teams workflow (Power Automate) URLs carry a signature.
        signed: Regex::new(r#"([?&]sig=)[^&\s'"]+"#).unwrap(),
        // INI and TOML style, with spaces around the = (~/.aws/credentials).
        ini: Regex::new(r#"(?i)\b([a-z0-9_.-]*(?:secret[a-z0-9_]*|key|token|password|passwd|pass))(\s+=\s*|\s*=\s+)('[^']*'|"[^"]*"|[^\s'"]+)"#).unwrap(),
        ntfy: Regex::new(r"\btk_[A-Za-z0-9]{24,}").unwrap(),
        assignment: Regex::new(r#"(?i)\b([A-Z0-9_]*(?:TOKEN|KEY|SECRET|PASSWORD|PASSWD|PASS))=('[^']*'|"[^"]*"|[^\s'"]+)"#).unwrap(),
        json_secret: Regex::new(r#"(?i)("[a-z0-9_-]*(?:api_?key|token|secret|password|authorization)"\s*:\s*")((?:[^"\\]|\\.)*)""#).unwrap(),
        // A Python dict as a log prints it: vLLM's "non-default args: {'api_key': ['…']}".
        py_secret: Regex::new(r#"(?i)('[a-z0-9_-]*(?:api_?key|token|secret|password|authorization)'\s*:\s*\[?\s*')((?:[^'\\]|\\.)*)'"#).unwrap(),
        header: Regex::new(r#"(?i)\b(authorization|x-api-key)(\s*:\s*)([^'"\r\n]+)"#).unwrap(),
        bearer: Regex::new(r#"(?i)\bBearer\s+[^\s'"]+"#).unwrap(),
        // "Token <value>" is how Firewalla's cloud is sent its key, here without the header's name.
        // Case-sensitive and long, so "Token limit reached" stays.
        msp_token: Regex::new(r"\bToken\s+[A-Za-z0-9._~+/=-]{20,}").unwrap(),
        secret_flag: Regex::new(r#"(?i)(--(?:password|passwd|token|api-key|apikey|secret|key))([= ])('[^']*'|"[^"]*"|[^\s'"]+)"#).unwrap(),
        mysql_p: Regex::new(r#"\b((?:mysql|mysqldump|mysqladmin|mariadb)\b[^\n;|&]*?\s)-p([^\s'"]+)"#).unwrap(),
        docker_p: Regex::new(r#"\b(docker\s+login\b[^\n;|&]*?\s)-p(\s+|=)?([^\s'"]+)"#).unwrap(),
        url_password: Regex::new(r"(\b[a-z][a-z0-9+.-]*://[^:/@\s]+:)[^@\s]+@").unwrap(),
        // HF tokens are hf_ + 34 letters and digits. Shorter lowercase words (the hf_transfer
        // package) are left alone.
        hf: Regex::new(r"\bhf_[A-Za-z0-9]{8,}").unwrap(),
        sk: Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}").unwrap(),
        // A load's API key (sbk- and 32 hex). The masked form the app shows has no ASCII after sbk-.
        sbk: Regex::new(r"\bsbk-[A-Za-z0-9_-]{8,}").unwrap(),
        github: Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})").unwrap(),
    })
}

fn looks_like_token(s: &str) -> bool {
    let body = &s[3..];
    body.len() >= 30 || body.chars().any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

/// Hides anything that looks like a secret: private key blocks, KEY=value pairs, secret-looking
/// JSON fields, Authorization and X-Api-Key headers, bearer and Firewalla tokens, --password/--token/
/// --api-key/--secret/--key values and --anything-password/-secret/-token ones, curl -u's
/// password, mysql's -p<password> and `docker login -p`, passwords in URLs, Slack, Discord,
/// Teams and Google Chat webhook paths, Healthchecks ping URLs, `sig=` signatures, INI-style
/// `aws_secret_access_key = …`, PGP private key blocks, Python dict fields as a log prints them
/// (`{'api_key': ['…']}`), and Hugging Face (hf_…), OpenAI/Anthropic (sk-…), a load's own (sbk-…),
/// GitHub, AWS (AKIA…) and ntfy (tk_…) tokens.
pub fn redact(text: &str) -> String {
    let p = patterns();
    let t = p.pem.replace_all(text, |c: &regex::Captures| {
        let ended = c[2].trim_end().ends_with("-----") && c[2].contains("-----END ");
        if ended {
            format!("-----BEGIN {}-----\n{REDACTED}\n-----END {}-----", &c[1], &c[1])
        } else {
            format!("-----BEGIN {}-----\n{REDACTED}", &c[1])
        }
    });
    let t = p.curl_user.replace_all(&t, |c: &regex::Captures| {
        let v = &c[2];
        let quote = v.chars().next().filter(|q| *q == '\'' || *q == '"').map(String::from).unwrap_or_default();
        let inner = v.trim_matches(|ch| ch == '\'' || ch == '"');
        match inner.split_once(':') {
            Some((user, _)) => format!("{}{quote}{user}:{REDACTED}{quote}", &c[1]),
            None => c[0].to_string(),
        }
    });
    let t = p.named_secret_flag.replace_all(&t, |c: &regex::Captures| format!("{}{}{REDACTED}", &c[1], &c[2]));
    let t = p.json_secret.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}\"", &c[1]));
    let t = p.py_secret.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}'", &c[1]));
    let t = p.header.replace_all(&t, |c: &regex::Captures| format!("{}{}{REDACTED}", &c[1], &c[2]));
    let t = p.assignment.replace_all(&t, |c: &regex::Captures| format!("{}={REDACTED}", &c[1]));
    let t = p.ini.replace_all(&t, |c: &regex::Captures| format!("{}{}{REDACTED}", &c[1], &c[2]));
    let t = p.bearer.replace_all(&t, format!("Bearer {REDACTED}").as_str());
    let t = p.msp_token.replace_all(&t, format!("Token {REDACTED}").as_str());
    let t = p.secret_flag.replace_all(&t, |c: &regex::Captures| format!("{}{}{REDACTED}", &c[1], &c[2]));
    let t = p.mysql_p.replace_all(&t, |c: &regex::Captures| format!("{}-p{REDACTED}", &c[1]));
    let t = p.docker_p.replace_all(&t, |c: &regex::Captures| format!("{}-p{}{REDACTED}", &c[1], c.get(2).map_or("", |m| m.as_str())));
    let t = p.url_password.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}@", &c[1]));
    let t = p.hf.replace_all(&t, |c: &regex::Captures| if looks_like_token(&c[0]) { REDACTED.to_string() } else { c[0].to_string() });
    let t = p.sk.replace_all(&t, REDACTED);
    let t = p.sbk.replace_all(&t, REDACTED);
    let t = p.github.replace_all(&t, REDACTED);
    let t = p.aws.replace_all(&t, REDACTED);
    let t = p.slack.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}", &c[1]));
    let t = p.webhook.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}", &c[1]));
    let t = p.signed.replace_all(&t, |c: &regex::Captures| format!("{}{REDACTED}", &c[1]));
    let t = p.ntfy.replace_all(&t, REDACTED);
    t.into_owned()
}

/// Hides secrets in every process command line before a snapshot reaches the UI or "Copy
/// command line" (`vllm serve … --api-key KEY` shows masked). Done in Rust so no window ever
/// sees the raw value; `process_reveal` reads one in full behind Admin.
pub fn redact_processes(procs: &mut [crate::model::Process]) {
    for p in procs {
        p.cmd = redact(&p.cmd);
    }
}

/// Hides secrets in every container's command the same way (its `command` from the frame).
pub fn redact_containers(containers: &mut [crate::model::Container]) {
    for c in containers {
        c.command = redact(&c.command);
    }
}

/// The last 64 KiB of some output, redacted; None when empty.
pub fn tail(text: &str) -> Option<String> {
    let t = text.trim_end();
    if t.is_empty() {
        return None;
    }
    let mut start = t.len().saturating_sub(TAIL_BYTES);
    while !t.is_char_boundary(start) {
        start += 1;
    }
    let cut = if start > 0 { format!("…{}", &t[start..]) } else { t.to_string() };
    Some(redact(&cut))
}

// ---- Operations: commands run for one purpose share an op id (task-local)

#[derive(Clone, Debug, Default)]
pub struct OpMeta {
    pub title: String,
    pub source: String,
}

#[derive(Clone, Debug)]
struct OpCtx {
    id: String,
    title: String,
    source: String,
}

tokio::task_local! {
    static OP: OpCtx;
    static PROBE: ();
    static TYPED: ();
}

/// Runs `fut` as one operation with a fresh id (tests; the app uses its task's id).
#[cfg(test)]
pub async fn with_op<F: Future>(meta: OpMeta, fut: F) -> F::Output {
    with_op_id(new_id("op"), meta, fut).await
}

/// Runs `fut` as one operation with a chosen id (a task's id, so its log and its task line
/// up): every entry it records carries the same op id and title.
pub async fn with_op_id<F: Future>(id: String, meta: OpMeta, fut: F) -> F::Output {
    let source = if meta.source.is_empty() { "ui".to_string() } else { meta.source };
    OP.scope(OpCtx { id, title: meta.title, source }, fut).await
}

/// Runs `fut` as SparkBox's own background check: what it records is classed "probe" (hidden
/// unless asked), whatever the command says. Tagged where the call is made, never guessed
/// from the command's text.
pub async fn as_probe<F: Future>(fut: F) -> F::Output {
    PROBE.scope((), fut).await
}

/// Whether this code runs inside `as_probe`.
pub fn in_probe() -> bool {
    PROBE.try_with(|_| ()).is_ok()
}

/// Runs `fut` as commands someone typed (Fleet Command): what it records is never classed
/// "read", whatever the classifier makes of the text, so the Changes view always shows it.
pub async fn as_typed<F: Future>(fut: F) -> F::Output {
    TYPED.scope((), fut).await
}

/// Whether this code runs inside `as_typed` (the write exit asks too: live/exit.rs).
pub fn in_typed() -> bool {
    TYPED.try_with(|_| ()).is_ok()
}

/// The id of the operation this code runs in, if any.
pub fn current_op() -> Option<String> {
    OP.try_with(|o| o.id.clone()).ok()
}

fn actor_of(source: &str) -> &'static str {
    match source {
        "tray" => "tray",
        // A background poll is SparkBox on its own.
        "system" | "poll" => "system",
        _ => "ui",
    }
}

/// What a row says when SparkBox stopped waiting for a command it had sent: the channel's
/// CloseOnDrop::term sent the command TERM as it closed.
pub const STOPPED_WAITING: &str = "SparkBox stopped waiting; TERM was sent to the command";
/// ... and when it stopped waiting before the command was sent: nothing ran.
pub const STOPPED_BEFORE_SENT: &str = "SparkBox stopped waiting before the command was sent; it didn't run";

/// A row `begin_guarded` wrote, never left "running": its caller ends it (`into_id`, then
/// `end`); dropped before that, because SparkBox stopped waiting for its command, it ends as
/// cancelled, saying whether the command had been sent (`sent`). A row something else ended
/// meanwhile (a reset connection marks its commands interrupted) is left as it is.
pub struct Begun<'a> {
    log: &'a Log,
    id: Option<String>,
    sent: AtomicBool,
}

impl Begun<'_> {
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or_default()
    }

    /// The command is on its way to the node: from now on, a drop has sent it TERM.
    pub fn sent(&self) {
        self.sent.store(true, Ordering::SeqCst);
    }

    /// The caller ends the row itself from here on.
    pub fn into_id(mut self) -> String {
        self.id.take().unwrap_or_default()
    }
}

impl Drop for Begun<'_> {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        let why = if self.sent.load(Ordering::SeqCst) { STOPPED_WAITING } else { STOPPED_BEFORE_SENT };
        self.log.end_running(&id, Ending { error: Some(why.into()), outcome: Some(Outcome::Cancelled), ..Default::default() });
    }
}

type Listener = Box<dyn Fn(&Entry) + Send + Sync>;

pub struct Log {
    entries: Mutex<VecDeque<Entry>>,
    path: Mutex<Option<PathBuf>>,
    retention_days: AtomicU32,
    listener: Mutex<Option<Listener>>,
    /// The data source in use (Demo until told otherwise), stamped on every new row.
    demo_now: AtomicBool,
}

impl Default for Log {
    fn default() -> Self {
        Log::new()
    }
}

impl Log {
    pub fn new() -> Log {
        Log { entries: Mutex::new(VecDeque::new()), path: Mutex::new(None), retention_days: AtomicU32::new(90), listener: Mutex::new(None), demo_now: AtomicBool::new(true) }
    }

    /// Tells the log which data source is in use; new rows carry it (`Entry::demo`).
    pub fn set_demo(&self, demo: bool) {
        self.demo_now.store(demo, Ordering::SeqCst);
    }

    /// Called with every new or changed entry (the window's live updates).
    pub fn on_record(&self, f: impl Fn(&Entry) + Send + Sync + 'static) {
        *self.listener.lock().unwrap() = Some(Box::new(f));
    }

    /// Sets how long the log is kept and deletes the files that are now too old.
    pub fn set_retention_days(&self, days: u32) {
        self.retention_days.store(days.max(1), Ordering::SeqCst);
        if let Some(path) = self.path() {
            prune(&path, self.cutoff_ms());
        }
        let cutoff = self.cutoff_ms();
        self.entries.lock().unwrap().retain(|e| e.ts_ms >= cutoff);
    }

    fn cutoff_ms(&self) -> u64 {
        now_ms().saturating_sub(u64::from(self.retention_days.load(Ordering::SeqCst)) * 86_400_000)
    }

    /// Where to append. Deletes files older than the retention period and loads what memory
    /// keeps (the newest 5000 entries and 2000 checks) from the newest 64 MB of files; anything
    /// still "running" from the last session didn't finish.
    pub fn set_path(&self, path: PathBuf) {
        let cutoff = self.cutoff_ms();
        prune(&path, cutoff);
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut loaded: Vec<Entry> = Vec::new();
        let (mut rest, mut checks, mut read) = (0, 0, 0);
        'files: for (i, (_, file)) in log_files(&path).into_iter().enumerate() {
            if read >= LOAD_BYTES {
                break;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            read += text.len();
            restrict(&file);
            // Newest first, so the first copy of an id seen is its last.
            for (n, line) in text.lines().enumerate().collect::<Vec<_>>().into_iter().rev() {
                // Once memory has its checks, the rest of them (most of the file) aren't parsed.
                if checks >= KEEP_CHECKS && check_line(line) {
                    continue;
                }
                let Ok(mut e) = serde_json::from_str::<Entry>(line) else { continue };
                if e.id.is_empty() {
                    e.id = format!("old-{i}-{n}-{}", e.ts_ms);
                }
                if seen.insert(e.id.clone(), ()).is_some() || e.ts_ms < cutoff {
                    continue;
                }
                let e = read_again(unfinished(redacted(e)));
                let count = if is_check(&e) { (&mut checks, KEEP_CHECKS) } else { (&mut rest, KEEP) };
                if *count.0 >= count.1 {
                    continue;
                }
                *count.0 += 1;
                loaded.push(e);
                if rest >= KEEP && checks >= KEEP_CHECKS {
                    break 'files;
                }
            }
        }
        loaded.sort_by_key(|e| e.ts_ms);
        *self.entries.lock().unwrap() = loaded.into_iter().collect();
        *self.path.lock().unwrap() = Some(path);
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path.lock().unwrap().clone()
    }

    /// Records an entry, filling in what the caller left out: id, time, the operation it
    /// belongs to, its class, escalation and outcome. Everything text is redacted.
    pub fn record(&self, mut entry: Entry) -> String {
        let now = now_ms();
        if entry.id.is_empty() {
            entry.id = new_id("a");
        }
        if entry.ts_ms == 0 {
            entry.ts_ms = now;
        }
        entry.updated_ms = now;
        if let Ok(op) = OP.try_with(|o| o.clone()) {
            entry.op_id.get_or_insert(op.id);
            if entry.title.is_empty() {
                entry.title = op.title;
            }
            if entry.source.is_empty() {
                entry.source = op.source;
            }
        }
        // Any part of it run through sudo, doas, pkexec or su -c, not only a leading "sudo ".
        if entry.kind == "sudo" || escalates(&entry.command) {
            entry.escalated = true;
        }
        // A docker command that is root-equivalent on the node is tagged root (P1.42), so the
        // log shows a silent root (no sudo) as plainly as a sudo one.
        if !entry.root && crate::ops::classify::verdict(&entry.command).root {
            entry.root = true;
        }
        if entry.demo.is_none() {
            entry.demo = Some(entry.simulated || self.demo_now.load(Ordering::SeqCst));
        }
        if entry.class.is_empty() && in_probe() {
            entry.class = Kind::Probe.as_str().into();
        }
        if entry.class.is_empty() {
            entry.class = match entry.kind.as_str() {
                "app" => "app".into(),
                "op" if entry.command.trim().is_empty() => "change".into(),
                "terminal" | "stream" => match classify(&entry.command) {
                    Kind::Destructive => "destructive".into(),
                    _ => "change".into(),
                },
                // Root never only reads as far as the log is concerned.
                _ if entry.escalated => match classify(&entry.command) {
                    Kind::Destructive => "destructive".into(),
                    _ => "change".into(),
                },
                _ => match classify(&entry.command) {
                    Kind::Probe | Kind::Read => "read".into(),
                    k => k.as_str().into(),
                },
            };
        }
        if in_typed() && matches!(entry.class.as_str(), "read" | "probe") {
            entry.class = "change".into();
        }
        // A shell is a session; a root shell counts as a change.
        entry.changes = matches!(entry.class.as_str(), "change" | "destructive" | "app") || (entry.class == "session" && entry.escalated);
        // Reads outside an operation are SparkBox fetching what a window shows (a listing, a
        // unit's status), not something you asked to change.
        if entry.source.is_empty() {
            entry.source = if matches!(entry.class.as_str(), "probe" | "read") { "system".into() } else { "ui".into() };
        }
        if entry.actor.is_empty() {
            entry.actor = actor_of(&entry.source).into();
        }
        if entry.state.is_empty() {
            entry.state = outcome(&entry).into();
        }
        entry.command = redact(&entry.command);
        entry.title = redact(&entry.title);
        entry.error = entry.error.as_deref().map(redact);
        entry.stdout_tail = entry.stdout_tail.as_deref().and_then(tail);
        entry.stderr_tail = entry.stderr_tail.as_deref().and_then(tail);
        let id = entry.id.clone();
        self.store(entry, true);
        id
    }

    /// Write-ahead: records `entry` as "running" before it runs (on disk at once); finish it
    /// with `end`. Returns its id.
    pub fn begin(&self, mut entry: Entry) -> String {
        entry.state = "running".into();
        entry.code = None;
        self.record(entry)
    }

    /// `begin`, for a command whose future may be dropped before it ends: the row ends itself
    /// then (`Begun`).
    pub fn begin_guarded(&self, entry: Entry) -> Begun<'_> {
        Begun { log: self, id: Some(self.begin(entry)), sent: AtomicBool::new(false) }
    }

    /// Appends how a begun entry ended.
    pub fn end(&self, id: &str, ending: Ending) {
        self.update(id, |e| apply_ending(e, ending, now_ms()));
    }

    /// `end`, only while the entry is still running; true when it ended it.
    pub fn end_running(&self, id: &str, ending: Ending) -> bool {
        self.update_if(id, |e| {
            let running = e.state == "running";
            if running {
                apply_ending(e, ending, now_ms());
            }
            running
        })
    }

    /// Ends the head line of task `op_id` (its kind "op" entry) with the task's outcome, so
    /// the activity log and Tasks never disagree. A head that already says so is left as is, and
    /// a Refused line inside the task keeps saying why it was refused.
    pub fn end_heads(&self, op_id: &str, outcome: Outcome, summary: Option<String>, error: Option<String>) {
        let heads: Vec<String> = {
            let entries = self.entries.lock().unwrap();
            entries.iter().rev().take(2000).filter(|e| e.op_id.as_deref() == Some(op_id) && e.kind == "op" && e.state != outcome.as_str() && e.title != "Refused").map(|e| e.id.clone()).collect()
        };
        for id in heads {
            self.end(&id, Ending::of(outcome, summary.clone(), error.clone()));
        }
    }

    /// Something that failed before (or instead of) running: a refused name, a node that
    /// isn't connected, sudo without a password. Logged as a failed change.
    pub fn record_failed(&self, node: &str, title: &str, command: &str, error: &str) -> String {
        let class = match classify(command) {
            Kind::Destructive => "destructive",
            _ => "change",
        };
        self.record(Entry {
            node: node.into(),
            kind: "op".into(),
            command: command.into(),
            title: title.into(),
            error: Some(error.into()),
            state: "failed".into(),
            class: class.into(),
            ..Default::default()
        })
    }

    /// Changes an entry already recorded (a running operation finishing, output arriving).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Entry)) {
        self.update_if(id, |e| {
            f(e);
            true
        });
    }

    /// `update` when `f` says it changed the entry (true); else nothing is written. True when it did.
    fn update_if(&self, id: &str, f: impl FnOnce(&mut Entry) -> bool) -> bool {
        let changed = {
            let mut entries = self.entries.lock().unwrap();
            let Some(e) = entries.iter_mut().rev().find(|e| e.id == id) else { return false };
            if !f(e) {
                return false;
            }
            e.updated_ms = now_ms();
            e.error = e.error.as_deref().map(redact);
            e.stdout_tail = e.stdout_tail.as_deref().and_then(tail);
            e.stderr_tail = e.stderr_tail.as_deref().and_then(tail);
            e.clone()
        };
        self.store(changed, false);
        true
    }

    /// Adds the output of a command just run to its entry (the newest one on that node
    /// running `command` in this operation).
    pub fn attach_output(&self, node: &str, command: &str, stdout: &str, stderr: &str) {
        let op = current_op();
        let command = redact(command.trim());
        let id = {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .rev()
                .take(200)
                .find(|e| e.node == node && e.op_id == op && e.stdout_tail.is_none() && e.stderr_tail.is_none() && e.command.contains(&command))
                .map(|e| e.id.clone())
        };
        if let Some(id) = id {
            self.update(&id, |e| {
                e.stdout_tail = Some(stdout.to_string());
                e.stderr_tail = Some(stderr.to_string());
            });
        }
    }

    /// Demo mode: notes what would have run.
    pub fn record_simulated(&self, node: &str, title: &str, command: &str) -> String {
        self.record(Entry {
            node: node.into(),
            kind: if command.starts_with("sudo ") { "sudo".into() } else { "command".into() },
            command: command.into(),
            title: title.into(),
            code: Some(0),
            simulated: true,
            state: "ok".into(),
            ..Default::default()
        })
    }

    /// A change on the Mac itself (admin unlocked, a password saved), logged as node "mac".
    pub fn record_app(&self, title: &str, detail: &str) -> String {
        self.record(Entry { node: "mac".into(), kind: "app".into(), command: detail.into(), title: title.into(), state: "ok".into(), ..Default::default() })
    }

    /// Starts a long operation's own line (state "running", written ahead); finish it with `end_op`.
    pub fn begin_op(&self, node: &str, title: &str, command: &str, simulated: bool) -> String {
        self.begin(Entry { node: node.into(), kind: "op".into(), command: command.into(), title: title.into(), simulated, ..Default::default() })
    }

    /// Ends an operation's own line: ok, ok with a warning (`ok` and an `error`), or failed.
    pub fn end_op(&self, id: &str, ok: bool, summary: Option<String>, error: Option<String>) {
        let outcome = match (ok, &error) {
            (true, None) => Outcome::Ok,
            (true, Some(_)) => Outcome::Warning,
            (false, _) => Outcome::Failed,
        };
        let error = if ok { error } else { Some(error.unwrap_or_else(|| "Failed".into())) };
        self.end(id, Ending::of(outcome, summary, error));
    }

    /// Newest first, Demo and Live alike (diagnostics); the window uses `entries_for`.
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().iter().rev().cloned().collect()
    }

    /// One data source's entries, newest first: what the Activity log (and with it Endpoint
    /// History's change markers) shows. Demo rows never show in Live, nor Live rows in Demo.
    pub fn entries_for(&self, demo: bool) -> Vec<Entry> {
        let entries = self.entries.lock().unwrap();
        let ops = op_sources(entries.iter());
        entries.iter().rev().filter(|e| is_demo(e, &ops) == demo).cloned().collect()
    }

    /// One data source's entries added or changed at or after `ts_ms`, newest first, at most `limit`.
    pub fn since_for(&self, demo: bool, ts_ms: u64, limit: usize) -> Vec<Entry> {
        let entries = self.entries.lock().unwrap();
        let ops = op_sources(entries.iter());
        entries.iter().rev().filter(|e| e.updated_ms.max(e.ts_ms) >= ts_ms && is_demo(e, &ops) == demo).take(limit).cloned().collect()
    }

    fn store(&self, entry: Entry, new: bool) {
        if let Some(path) = self.path.lock().unwrap().as_ref() {
            if append(path, &entry) {
                prune(path, self.cutoff_ms());
            }
        }
        if new {
            let mut entries = self.entries.lock().unwrap();
            entries.push_back(entry.clone());
            cap(&mut entries);
        }
        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            listener(&entry);
        }
    }

    /// Every entry from `from_ms` to `to_ms` (oldest first), read from the files on disk (and
    /// from memory when there's no file), last copy of each winning.
    pub fn range(&self, from_ms: u64, to_ms: u64) -> Vec<Entry> {
        let mut by_id: HashMap<String, Entry> = HashMap::new();
        if let Some(path) = self.path() {
            for (newest, file) in log_files(&path) {
                if newest < from_ms {
                    break;
                }
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                for line in text.lines().rev() {
                    let Ok(e) = serde_json::from_str::<Entry>(line) else { continue };
                    if e.ts_ms >= from_ms && e.ts_ms <= to_ms && !e.id.is_empty() {
                        by_id.entry(e.id.clone()).or_insert_with(|| read_again(redacted(e)));
                    }
                }
            }
        }
        // What's in memory is current (still running, not "didn't finish").
        for e in self.entries.lock().unwrap().iter().filter(|e| e.ts_ms >= from_ms && e.ts_ms <= to_ms) {
            by_id.insert(e.id.clone(), e.clone());
        }
        let cutoff = self.cutoff_ms();
        let live: std::collections::HashSet<String> = self.entries.lock().unwrap().iter().map(|e| e.id.clone()).collect();
        let mut out: Vec<Entry> = by_id.into_values().filter(|e| e.ts_ms >= cutoff).map(|e| if live.contains(&e.id) { e } else { unfinished(e) }).collect();
        out.sort_by_key(|e| e.ts_ms);
        out
    }

    /// Writes the entries from `from_ms` to `to_ms` (oldest first) as CSV or JSON: one data
    /// source's (`demo`), or both when None.
    pub fn export(&self, format: &str, from_ms: u64, to_ms: u64, demo: Option<bool>) -> Result<String, String> {
        if !matches!(format, "csv" | "json") {
            return Err("Export as csv or json.".into());
        }
        let mut entries = self.range(from_ms, to_ms);
        if let Some(demo) = demo {
            let ops = op_sources(entries.iter());
            entries.retain(|e| is_demo(e, &ops) == demo);
        }
        match format {
            "json" => serde_json::to_string_pretty(&entries).map_err(|e| e.to_string()),
            _ => {
                let mut out = String::from("time,node,operation,title,command,result,exit code,duration ms,source,as root,demo,class,error\n");
                for e in &entries {
                    let row = [
                        iso_time(e.ts_ms),
                        e.node.clone(),
                        e.op_id.clone().unwrap_or_default(),
                        e.title.clone(),
                        e.command.clone(),
                        e.state.clone(),
                        e.code.map(|c| c.to_string()).unwrap_or_default(),
                        e.ms.to_string(),
                        e.source.clone(),
                        e.escalated.to_string(),
                        e.simulated.to_string(),
                        e.class.clone(),
                        e.error.clone().unwrap_or_default(),
                    ];
                    out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
                    out.push('\n');
                }
                Ok(out)
            }
        }
    }
}

/// Fills in how a begun entry ended.
fn apply_ending(e: &mut Entry, ending: Ending, now: u64) {
    e.code = ending.code;
    e.ms = ending.duration_ms.unwrap_or_else(|| now.saturating_sub(e.ts_ms));
    e.error = ending.error;
    if ending.stdout_tail.is_some() {
        e.stdout_tail = ending.stdout_tail;
    }
    if ending.stderr_tail.is_some() {
        e.stderr_tail = ending.stderr_tail;
    }
    let failed = e.error.is_some() || e.code.is_some_and(|c| c != 0);
    e.state = ending.outcome.map(Outcome::as_str).unwrap_or(if failed { "failed" } else { "ok" }).into();
}

/// A check: SparkBox's own (a background poll's rows) or a read a window made outside any
/// operation. Memory keeps fewer of them (KEEP_CHECKS); the rows of an operation you started
/// stay together with the rest.
fn is_check(e: &Entry) -> bool {
    matches!(e.class.as_str(), "probe" | "read") && (e.op_id.is_none() || e.source == "poll")
}

/// `is_check` of a line in the file, told from its text (serde writes these fields so; inside a
/// string their quotes are escaped). A line it can't tell is parsed.
fn check_line(line: &str) -> bool {
    (line.contains(r#""class":"probe""#) || line.contains(r#""class":"read""#)) && (line.contains(r#""opId":null"#) || line.contains(r#""source":"poll""#))
}

/// Drops the oldest checks past KEEP_CHECKS and the oldest other rows past KEEP.
fn cap(entries: &mut VecDeque<Entry>) {
    let checks = entries.iter().filter(|e| is_check(e)).count();
    for (check, over) in [(true, checks.saturating_sub(KEEP_CHECKS)), (false, (entries.len() - checks).saturating_sub(KEEP))] {
        for _ in 0..over {
            if let Some(i) = entries.iter().position(|e| is_check(e) == check) {
                entries.remove(i);
            }
        }
    }
}

/// Per operation, whether it ran in Demo, from what its rows say for sure: a simulated row
/// (Demo) or a command that really ran on a node (Live). For rows written before round 4.
fn op_sources<'a>(entries: impl Iterator<Item = &'a Entry>) -> HashMap<String, bool> {
    let mut ops = HashMap::new();
    for e in entries {
        let Some(op) = &e.op_id else { continue };
        let known = e.demo.or_else(|| e.simulated.then_some(true)).or_else(|| ran_on_a_node(e).then_some(false));
        if let Some(demo) = known {
            ops.entry(op.clone()).or_insert(demo);
        }
    }
    ops
}

fn ran_on_a_node(e: &Entry) -> bool {
    matches!(e.kind.as_str(), "command" | "sudo" | "terminal" | "stream") && !e.simulated && e.node != "mac"
}

/// Whether a row belongs to Demo: its own stamp; else (rows from before round 4) simulated
/// rows and anything in a Demo operation are Demo, commands that really ran on a node are
/// Live, and the rest is Demo, since Live was first used on Oct 29 (plan §3.1).
pub fn is_demo(e: &Entry, ops: &HashMap<String, bool>) -> bool {
    if let Some(demo) = e.demo {
        return demo;
    }
    if e.simulated {
        return true;
    }
    if let Some(demo) = e.op_id.as_ref().and_then(|op| ops.get(op)) {
        return *demo;
    }
    !ran_on_a_node(e)
}

/// Redacts an entry's text again as it's read back from disk, so a secret that reached the
/// file (an older build, a hand-edited log) never leaves the backend (P1.46). `redact` is
/// idempotent, so a row already redacted on write is unchanged.
fn redacted(mut e: Entry) -> Entry {
    e.command = redact(&e.command);
    e.title = redact(&e.title);
    e.error = e.error.as_deref().map(redact);
    e.stdout_tail = e.stdout_tail.as_deref().map(redact);
    e.stderr_tail = e.stderr_tail.as_deref().map(redact);
    e
}

/// A "running" entry read back after a restart never got its end.
fn unfinished(mut e: Entry) -> Entry {
    if e.state == "running" {
        e.state = "unfinished".into();
        e.error.get_or_insert_with(|| UNFINISHED.into());
    }
    e
}

/// A row from before round 4 (no `demo` stamp) where SparkBox ran its own sparkjob.py read (a
/// listing, a file's backups) was classed a change then. Read back, it gets the class `record`
/// gives such a row now: a read, and SparkBox's own when outside an operation. Typed and root
/// rows keep theirs. An evidence capture's row from before round 4's fix (an app row, so under
/// Changes) reads back as the read it is now.
fn read_again(mut e: Entry) -> Entry {
    if e.node == "mac" && e.kind == "app" && e.class == "app" && e.title == "Evidence captured" {
        e.class = "read".into();
        e.changes = false;
        return e;
    }
    let sparkjob = e.command.trim_start().starts_with("python3 ~/.sparkbox/sparkjob.py ");
    if e.demo.is_none() && sparkjob && e.kind == "command" && !e.escalated && e.class == "change" && matches!(classify(&e.command), Kind::Read | Kind::Probe) {
        e.class = "read".into();
        e.changes = false;
        if e.op_id.is_none() {
            e.source = "system".into();
            e.actor = actor_of(&e.source).into();
        }
    }
    e
}

fn outcome(e: &Entry) -> &'static str {
    if e.error.is_some() {
        return "failed";
    }
    match (e.kind.as_str(), e.code) {
        ("op", None) => "running",
        (_, Some(0)) | (_, None) => "ok",
        _ => "failed",
    }
}

fn csv_field(s: &str) -> String {
    // A leading = + - @ would run as a formula in a spreadsheet.
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// "2026-10-28T14:32:05Z"
pub fn iso_time(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// The current file and every rotated one (activity.<ms>.jsonl, or round 1's activity.<n>.jsonl),
/// newest first, each with the time of its newest entry (its last write).
fn log_files(path: &Path) -> Vec<(u64, PathBuf)> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "activity".into());
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut files: Vec<(u64, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            name.strip_prefix(&format!("{stem}.")).and_then(|r| r.strip_suffix(".jsonl")).is_some_and(|mid| !mid.is_empty() && mid.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|p| (modified_ms(&p), p))
        .collect();
    if path.exists() {
        files.push((u64::MAX, path.to_path_buf()));
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
}

/// Deletes rotated files whose newest entry is older than `cutoff_ms`.
fn prune(path: &Path, cutoff_ms: u64) {
    for (newest, file) in log_files(path) {
        if newest < cutoff_ms && file != path {
            let _ = std::fs::remove_file(&file);
        }
    }
}

/// Only you can read the log.
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Starts a new file when the current one is from an earlier day or has reached 10 MB; true
/// when it did.
fn rotate_if_due(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let day = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0);
    let stale_day = meta.modified().map(|m| day(m) < day(SystemTime::now())).unwrap_or(false);
    if meta.len() < ROTATE_BYTES && !(stale_day && meta.len() > 0) {
        return false;
    }
    let mut stamp = modified_ms(path).max(1);
    let rotated = loop {
        let candidate = path.with_file_name(format!("{}.{stamp}.jsonl", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
        if !candidate.exists() {
            break candidate;
        }
        stamp += 1;
    };
    let _ = std::fs::rename(path, &rotated);
    // Renaming changes nothing the age is read from (the modification time stays).
    true
}

/// Appends one line; true when the file was rotated first.
fn append(path: &Path, entry: &Entry) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let rotated = rotate_if_due(path);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path) {
        let _ = writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default());
    }
    rotated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sparkbox-audit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("activity.jsonl")
    }

    /// The same cases src/lib/redact.test.ts checks, so the UI and the log agree.
    #[test]
    fn secrets_are_redacted_by_the_shared_cases() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/redact_cases.json");
        let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let redacted = cases["redact"].as_array().unwrap();
        assert!(redacted.len() >= 20);
        for case in redacted {
            let (input, want) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
            assert_eq!(redact(input), want, "{input}");
        }
        // Ordinary words that only look a bit like keys are left alone.
        for keep in cases["keep"].as_array().unwrap() {
            let keep = keep.as_str().unwrap();
            assert_eq!(redact(keep), keep);
        }
    }

    #[test]
    fn output_tails_keep_the_last_64_kib_and_commands_are_whole() {
        let long = "x".repeat(100_000) + "END";
        let t = tail(&long).unwrap();
        assert!(t.len() <= TAIL_BYTES + 4);
        assert!(t.ends_with("END"));
        assert!(t.starts_with('…'));
        assert_eq!(tail("  \n"), None);

        let log = Log::new();
        let cmd = format!("docker run --rm img python3 -c '{}' && echo done", "print(1);".repeat(200));
        log.record(Entry { node: "cell1".into(), kind: "command".into(), command: cmd.clone(), code: Some(0), ..Default::default() });
        assert_eq!(log.entries()[0].command, cmd, "not cut at 600 characters");
    }

    #[tokio::test]
    async fn entries_in_one_operation_share_its_id_and_title() {
        let log = Log::new();
        let op = with_op_id("op-test".into(), OpMeta { title: "Restart 2 containers".into(), source: "ui".into() }, async {
            let id = log.begin_op("cell1", "Restart 2 containers", "", false);
            log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "docker restart abc".into(), code: Some(0), ..Default::default() });
            log.record(Entry { node: "cell2".into(), kind: "sudo".into(), command: "docker restart def".into(), code: Some(1), ..Default::default() });
            log.attach_output("cell2", "docker restart def", "", "Error: no such container");
            log.end_op(&id, false, Some("1 of 2 restarted".into()), Some("cell2 failed".into()));
            current_op().unwrap()
        })
        .await;
        let entries = log.entries();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.op_id.as_deref() == Some(op.as_str())));
        assert!(entries.iter().all(|e| e.title == "Restart 2 containers"));
        let failed = entries.iter().find(|e| e.node == "cell2").unwrap();
        assert_eq!((failed.state.as_str(), failed.escalated), ("failed", true));
        assert_eq!(failed.stderr_tail.as_deref(), Some("Error: no such container"));
        let head = entries.iter().find(|e| e.kind == "op").unwrap();
        assert_eq!(head.state, "failed");
        assert_eq!(head.class, "change");

        // Outside any operation: no op id, and reads aren't changes.
        log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "uptime".into(), code: Some(0), ..Default::default() });
        let e = &log.entries()[0];
        assert_eq!((e.op_id.clone(), e.class.as_str(), e.changes), (None, "read", false));
    }

    #[tokio::test]
    async fn probes_are_tagged_where_they_run_not_guessed_from_the_text() {
        let log = Log::new();
        as_probe(async {
            log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "python3 ~/.sparkbox/sparkjob.py list".into(), code: Some(0), ..Default::default() });
        })
        .await;
        // The same text from you is a read like any other, and anything appended counts on its own.
        log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "python3 ~/.sparkbox/sparkjob.py list; rm -rf ~/models".into(), code: Some(0), ..Default::default() });
        log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "curl -s -o /dev/null -m 3 http://x; dd if=/dev/zero of=/dev/nvme0n1".into(), code: Some(0), ..Default::default() });
        let e = log.entries();
        assert_eq!((e[2].class.as_str(), e[2].changes, e[2].source.as_str()), ("probe", false, "system"));
        assert_eq!((e[1].class.as_str(), e[1].changes), ("destructive", true), "the whole model folder is data loss (r4)");
        assert_eq!((e[0].class.as_str(), e[0].changes), ("destructive", true));
    }

    #[test]
    fn write_ahead_a_started_command_is_on_disk_before_it_ends_and_unfinished_after_a_restart() {
        let path = temp("ahead");
        let log = Log::new();
        log.set_path(path.clone());
        let id = log.begin(Entry { node: "cell2".into(), kind: "sudo".into(), command: "systemctl restart sparkbox-telemetry".into(), ..Default::default() });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&id) && text.contains("\"state\":\"running\""), "written before it runs");

        // SparkBox quits here. Next start:
        let again = Log::new();
        again.set_path(path.clone());
        let e = &again.entries()[0];
        assert_eq!((e.id.as_str(), e.state.as_str(), e.error.as_deref()), (id.as_str(), "unfinished", Some(UNFINISHED)));
        assert!(e.escalated && e.changes);

        // And one that ends: the outcome is appended and wins.
        let ok = again.begin(Entry { node: "cell2".into(), kind: "command".into(), command: "docker restart vllm-head".into(), ..Default::default() });
        again.end(&ok, Ending { code: Some(1), stderr_tail: Some("Error response from daemon: boom".into()), duration_ms: Some(1234), ..Default::default() });
        let third = Log::new();
        third.set_path(path);
        let e = third.entries().into_iter().find(|e| e.id == ok).unwrap();
        assert_eq!((e.state.as_str(), e.code, e.ms), ("failed", Some(1), 1234));
        assert_eq!(e.stderr_tail.as_deref(), Some("Error response from daemon: boom"));
    }

    #[test]
    fn a_guarded_row_dropped_before_it_ends_is_cancelled_never_left_running() {
        let log = Log::new();
        let cmd = |c: &str| Entry { node: "cell1".into(), kind: "command".into(), command: c.into(), ..Default::default() };
        // Dropped before the command went out: nothing ran.
        let id = log.begin_guarded(cmd("sleep 1")).id().to_string();
        let e = log.entries().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!((e.state.as_str(), e.error.as_deref()), ("cancelled", Some(STOPPED_BEFORE_SENT)));
        // Dropped after: TERM went with the channel's close.
        let row = log.begin_guarded(cmd("sleep 2"));
        row.sent();
        let id = row.id().to_string();
        assert_eq!(log.entries().into_iter().find(|e| e.id == id).unwrap().state, "running");
        drop(row);
        let e = log.entries().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!((e.state.as_str(), e.error.as_deref()), ("cancelled", Some(STOPPED_WAITING)));
        // Handed over and ended by its caller: the drop changes nothing.
        let id = log.begin_guarded(cmd("sleep 3")).into_id();
        log.end(&id, Ending { code: Some(0), ..Default::default() });
        assert_eq!(log.entries().into_iter().find(|e| e.id == id).unwrap().state, "ok");
        // Ended meanwhile by something else (a reset connection): left as that said.
        let row = log.begin_guarded(cmd("sleep 4"));
        log.end(row.id(), Ending { error: Some("Interrupted: the connection was reset".into()), ..Default::default() });
        let id = row.id().to_string();
        drop(row);
        let e = log.entries().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!((e.state.as_str(), e.error.as_deref()), ("failed", Some("Interrupted: the connection was reset")));
        assert!(!log.end_running(&id, Ending::ok(None)));
    }

    #[tokio::test]
    async fn a_poll_is_one_operation_run_by_sparkbox() {
        let log = Log::new();
        with_op_id("poll-1".into(), OpMeta { title: "Check updates".into(), source: "poll".into() }, as_probe(async {
            for n in ["cell1", "cell2"] {
                log.record(Entry { node: n.into(), kind: "command".into(), command: "apt list --upgradable".into(), code: Some(0), ..Default::default() });
            }
        }))
        .await;
        let rows = log.entries();
        assert!(rows.iter().all(|e| e.op_id.as_deref() == Some("poll-1") && e.title == "Check updates" && e.source == "poll"));
        assert!(rows.iter().all(|e| (e.class.as_str(), e.actor.as_str(), e.changes) == ("probe", "system", false)), "{rows:?}");
    }

    #[test]
    fn failures_before_anything_ran_are_recorded() {
        let log = Log::new();
        log.record_failed("cell3", "Restart docker on cell3", "sudo systemctl restart docker", "cell3 isn't connected.");
        let e = &log.entries()[0];
        assert_eq!((e.state.as_str(), e.class.as_str(), e.changes), ("failed", "change", true));
        assert_eq!(e.error.as_deref(), Some("cell3 isn't connected."));
    }

    #[test]
    fn demo_and_app_entries() {
        let log = Log::new();
        log.record_simulated("cell2", "Restart docker on cell2", "sudo systemctl restart -- 'docker.service'");
        log.record_app("Admin mode unlocked", "Demo data: no password needed");
        let e = log.entries();
        assert_eq!((e[0].node.as_str(), e[0].class.as_str(), e[0].changes), ("mac", "app", true));
        assert!(e[1].simulated && e[1].escalated && e[1].state == "ok" && e[1].class == "change");
    }

    #[test]
    fn the_file_is_private_survives_a_restart_and_updates_replace_earlier_copies() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("persist");
        let log = Log::new();
        log.set_path(path.clone());
        let id = log.begin_op("cell3", "Restart cell3", "systemctl reboot", false);
        log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "TOKEN=s3cr3t-value docker ps".into(), code: Some(0), ..Default::default() });
        log.end_op(&id, true, Some("Back after 2m14s".into()), None);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        // (Not just "abc": random hex ids can contain that.)
        assert!(!text.contains("s3cr3t") && text.contains("TOKEN=‹redacted›"), "redacted before it reaches the disk");

        let again = Log::new();
        again.set_path(path);
        let e = again.entries();
        assert_eq!(e.len(), 2, "the running copy was replaced by the finished one");
        let reboot = e.iter().find(|e| e.id == id).unwrap();
        assert_eq!(reboot.state, "ok");
        assert_eq!(reboot.stdout_tail.as_deref(), Some("Back after 2m14s"));
    }

    fn set_age(path: &Path, days: u64) {
        let t = SystemTime::now() - std::time::Duration::from_secs(days * 86_400 + 3_600);
        std::fs::File::options().append(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn retention_is_by_age_with_as_many_files_as_that_needs() {
        let path = temp("retain");
        let dir = path.parent().unwrap();
        // Forty days of rotated files, one a day, plus round 1's numbered ones.
        // Keep the fixtures an hour beyond each day boundary. A timestamp equal
        // to the cutoff is correctly retained if setup finishes in the same ms.
        for d in 1..=40u64 {
            let ts = now_ms() - d * 86_400_000 - 3_600_000;
            let f = dir.join(format!("activity.{ts}.jsonl"));
            let e = Entry { id: format!("d{d}"), ts_ms: ts, node: "cell1".into(), command: "uptime".into(), ..Default::default() };
            std::fs::write(&f, serde_json::to_string(&e).unwrap() + "\n").unwrap();
            set_age(&f, d);
        }
        let legacy = dir.join("activity.5.jsonl");
        std::fs::write(&legacy, "{}\n").unwrap();
        set_age(&legacy, 200);

        let log = Log::new();
        log.set_retention_days(30);
        log.set_path(path.clone());
        let left = log_files(&path).len();
        assert_eq!(left, 29, "days 1–29 kept (more than round 1's five files), day 30 and older deleted");
        assert!(!legacy.exists());
        assert_eq!(log.entries().len(), 29);

        log.set_retention_days(365);
        log.set_retention_days(7);
        assert_eq!(log_files(&path).len(), 6, "a shorter setting deletes at once");
        assert_eq!(log.entries().len(), 6);
    }

    #[test]
    fn big_files_rotate_and_reading_back_spans_every_file() {
        let path = temp("rotate");
        let log = Log::new();
        log.set_path(path.clone());
        let first = log.record_app("Inventory saved", "4 nodes");
        // Past 10 MB the file moves to activity.<time>.jsonl and a new one starts.
        let f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.set_len(ROTATE_BYTES + 1).unwrap();
        let second = log.record_app("Inventory saved", "5 nodes");
        assert_eq!(log_files(&path).len(), 2);
        assert!(std::fs::metadata(&path).unwrap().len() < 4096);

        let again = Log::new();
        again.set_path(path);
        let ids: Vec<String> = again.entries().into_iter().map(|e| e.id).collect();
        assert!(ids.contains(&first) && ids.contains(&second));
    }

    #[test]
    fn export_reads_the_requested_range_from_disk() {
        let path = temp("export");
        let log = Log::new();
        log.set_path(path.clone());
        let base = now_ms() - 3 * 86_400_000;
        for (i, cmd) in ["echo \"a,b\"", "uptime", "df -h"].iter().enumerate() {
            log.record(Entry { node: "cell1".into(), kind: "command".into(), command: cmd.to_string(), code: Some(0), ts_ms: base + i as u64 * 86_400_000, ..Default::default() });
        }
        log.record_app("=cmd|' /C calc'!A0", "");
        // A fresh log (nothing in memory but what loading kept) still exports the range.
        let fresh = Log::new();
        fresh.set_path(path);
        let csv = fresh.export("csv", base, base + 86_400_000, None).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("time,node,"));
        assert_eq!(lines.len(), 3, "two days of the four: {csv}");
        assert!(lines[1].contains("\"echo \"\"a,b\"\"\""));
        let all = fresh.export("csv", 0, u64::MAX, None).unwrap();
        assert!(all.contains(",'=cmd|"), "no spreadsheet formulas");
        let json: Vec<Entry> = serde_json::from_str(&fresh.export("json", 0, u64::MAX, None).unwrap()).unwrap();
        assert_eq!(json.len(), 4);
        assert!(fresh.export("xml", 0, u64::MAX, None).is_err());
    }

    /// A tour like the critics' (shells, log tabs, broadcast, a listing, SparkBox's probes, a
    /// sudo read, Fleet, a restart, a setting): the Changes view shows only what changed.
    #[tokio::test]
    async fn changes_view_has_no_read_only_entries() {
        use crate::term::{Emit, Target, Terminals};
        let dir = std::env::temp_dir().join(format!("sparkbox-audit-tour-{}", std::process::id()));
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let live = super::super::Live::new(vpn, dir.join("kh.json"), || Ok(None));
        let cluster = std::sync::Arc::new(std::sync::Mutex::new(crate::demo::DemoCluster::new(&crate::inventory::Inventory::example())));
        let terminals = Terminals::with_demo(live.clone(), cluster.clone(), std::sync::Arc::new(std::sync::Mutex::new(crate::files::demo::DemoFs::new())));
        let quiet = || -> Emit { Box::new(|_| {}) };
        let shell = terminals.open(true, "cell1", Target::Shell, 80, 24, quiet()).await.unwrap();
        terminals.authorize_shell(&shell).unwrap();
        let logs = terminals.open(true, "cell1", Target::ContainerLogs { container: "vllm-head".into(), since: None }, 80, 24, quiet()).await.unwrap();
        let journal = terminals.open(true, "cell2", Target::Journal { unit: "docker.service".into(), since: None }, 80, 24, quiet()).await.unwrap();
        terminals.note_broadcast(true, true, std::slice::from_ref(&shell));
        terminals.note_broadcast(true, false, &[]);
        for id in [&shell, &logs, &journal] {
            terminals.close(id);
        }
        let log = &live.audit;
        as_probe(async { log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "true".into(), code: Some(0), ..Default::default() }) }).await;
        log.record(Entry { node: "cell2".into(), kind: "command".into(), command: "cat /proc/loadavg".into(), code: Some(0), ..Default::default() });
        log.record(Entry { node: "cell2".into(), kind: "sudo".into(), command: "ss -tlnpH".into(), code: Some(0), ..Default::default() });
        as_typed(async { log.record(Entry { node: "cell3".into(), kind: "command".into(), command: "uptime".into(), code: Some(0), ..Default::default() }) }).await;
        log.record(Entry { node: "cell3".into(), kind: "command".into(), command: "docker restart vllm-worker".into(), code: Some(0), ..Default::default() });
        log.record_app("Security settings changed", "Auto-lock 15 → 60 min");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let all = log.entries();
        let changes: Vec<&Entry> = all.iter().filter(|e| e.changes).collect();
        let titles: Vec<String> = changes.iter().map(|e| format!("{} | {} | {}", e.class, e.title, e.command)).collect();
        assert!(changes.iter().all(|e| matches!(e.class.as_str(), "change" | "destructive" | "app")), "{titles:#?}");
        assert!(!changes.iter().any(|e| e.kind == "terminal" || e.title.contains("roadcast")), "no shell, log or broadcast rows: {titles:#?}");
        assert_eq!(changes.len(), 4, "the sudo read, the Fleet command, the restart and the setting: {titles:#?}");
        // Everything is still there for the Everything view, each tab once.
        let tabs: Vec<&Entry> = all.iter().filter(|e| e.kind == "terminal").collect();
        assert_eq!(tabs.len(), 3, "{tabs:#?}");
        assert_eq!(tabs.iter().filter(|e| e.class == "session").count(), 1);
        assert_eq!(tabs.iter().filter(|e| e.class == "read").count(), 2);
        assert!(all.iter().any(|e| e.title == "Open vllm-head's logs on cell1"));
    }

    /// Every multi-step task (a service rollout, a bulk restart, a reboot, Fleet ok and failed)
    /// has exactly one head line, and it ends with the task's own outcome.
    #[tokio::test]
    async fn every_multi_step_task_has_one_head_with_its_outcome() {
        use crate::ops::Ops;
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("sparkbox-audit-heads-{}", std::process::id()));
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let live = super::super::Live::new(vpn, dir.join("kh.json"), || Ok(None));
        let demo = std::sync::Arc::new(std::sync::Mutex::new(crate::demo::DemoCluster::new(&crate::inventory::Inventory::example())));
        let ops = Ops::with_poll(live.clone(), demo.clone(), crate::tasks::Tasks::new(None), Duration::from_millis(1));
        let src = demo.clone();
        ops.set_snapshot_source(move || {
            let mut d = src.lock().unwrap();
            d.advance();
            Some(d.snapshot())
        });
        let nodes: Vec<String> = ["cell1", "cell2"].map(String::from).to_vec();
        let llama: Vec<crate::ops::containers::Target> = {
            let snap = demo.lock().unwrap().snapshot();
            snap.nodes.iter().flat_map(|n| n.containers.iter().filter(|c| c.project.as_deref() == Some("llama-70b")).map(move |c| crate::ops::containers::Target { node: n.id.clone(), id: c.id.clone(), name: c.name.clone() })).collect()
        };
        let fleet = |command: &str| crate::ops::fleet::FleetSpec { nodes: nodes.clone(), command: command.into(), timeout_s: 10, ..Default::default() };
        let ids = vec![
            ops.services_apply(true, nodes.clone(), "docker.service".into(), "restart".into()).unwrap(),
            ops.containers_bulk(true, llama, "restart".into(), false, None).unwrap(),
            ops.power(true, "cell3", "reboot", 0).unwrap(),
            ops.fleet_start(true, fleet("uptime")).unwrap(),
            ops.fleet_start(true, fleet("frobnicate")).unwrap(),
        ];
        for id in &ids {
            let started = std::time::Instant::now();
            let task = loop {
                let t = ops.tasks().get(id).unwrap();
                if t.finished() {
                    break t;
                }
                assert!(started.elapsed() < Duration::from_secs(30), "{} still running", t.title);
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let heads: Vec<Entry> = live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind == "op").collect();
            assert_eq!(heads.len(), 1, "{}: {heads:#?}", task.title);
            let outcome = task.outcome.expect("finished tasks say how");
            assert_eq!(heads[0].state, outcome.as_str(), "{}: the head says what Tasks says", task.title);
        }
        let failed = ops.tasks().get(&ids[4]).unwrap();
        assert_eq!(failed.outcome, Some(Outcome::Failed), "an unknown command fails in Demo");
    }

    /// r4 F4, as found in Live: two `sparkjob.py backups` rows written that morning under the
    /// old classifier were the only "changes" in the log. Read back now, they're SparkBox's own
    /// reads; a typed command, a root one and a sparkjob write keep their class.
    #[test]
    fn rows_from_before_round_4_read_back_with_todays_classes() {
        let path = temp("legacy-reads");
        let row = |id: &str, op: Option<&str>, command: &str, escalated: bool| {
            let mut v = serde_json::to_value(Entry {
                id: id.into(), ts_ms: now_ms() - 36_000_000, node: "cell1".into(), kind: "command".into(), command: command.into(), op_id: op.map(Into::into),
                class: "change".into(), changes: true, source: "ui".into(), escalated, code: Some(0), state: "ok".into(), ..Default::default()
            })
            .unwrap();
            v.as_object_mut().unwrap().remove("demo");
            v.to_string()
        };
        let lines = [
            row("b1", None, "python3 ~/.sparkbox/sparkjob.py backups ~/.sparkbox/backups '/home/sparkuser/glm53/inner.sh'", false),
            row("b2", None, "python3 ~/.sparkbox/sparkjob.py ls '/etc/netplan'", false),
            row("t1", Some("fleet-1"), "cat /etc/hosts", false),
            row("r1", None, "sudo -k -S -p '' python3 ~/.sparkbox/sparkjob.py read '/etc/netplan/40-cx7.yaml'", true),
            row("w1", None, "python3 ~/.sparkbox/sparkjob.py commit '/home/sparkuser/notes.txt'", false),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let log = Log::new();
        log.set_path(path.clone());
        let got = |id: &str| log.entries().into_iter().find(|e| e.id == id).unwrap();
        for id in ["b1", "b2"] {
            let e = got(id);
            assert_eq!((e.class.as_str(), e.changes, e.source.as_str()), ("read", false, "system"), "{id}");
        }
        for id in ["t1", "r1", "w1"] {
            let e = got(id);
            assert_eq!((e.class.as_str(), e.changes, e.source.as_str()), ("change", true, "ui"), "{id}");
        }
        assert_eq!(log.entries_for(false).iter().filter(|e| e.changes).count(), 3, "Changes in Live: the typed, root and write rows only");
        let exported: Vec<Entry> = serde_json::from_str(&log.export("json", 0, u64::MAX, Some(false)).unwrap()).unwrap();
        assert!(exported.iter().filter(|e| e.id.starts_with('b')).all(|e| !e.changes), "an export reads them the same way");
    }

    /// L27: an evidence capture written as an app row (round 4 before its fix) was the one row
    /// under Changes in a look-only session. Read back, it's a read; other app rows keep theirs.
    #[test]
    fn an_evidence_capture_written_as_an_app_row_reads_back_as_a_read() {
        let path = temp("legacy-evidence");
        let row = |id: &str, title: &str| {
            serde_json::to_value(Entry {
                id: id.into(), ts_ms: now_ms() - 3_600_000, node: "mac".into(), kind: "app".into(), title: title.into(), command: "r4-agent · 4 nodes · /x".into(),
                class: "app".into(), changes: true, source: "ui".into(), code: Some(0), state: "ok".into(), demo: Some(false), ..Default::default()
            })
            .unwrap()
            .to_string()
        };
        std::fs::write(&path, [row("e1", "Evidence captured"), row("a1", "Admin unlocked")].join("\n") + "\n").unwrap();
        let log = Log::new();
        log.set_path(path.clone());
        let got = |id: &str| log.entries().into_iter().find(|e| e.id == id).unwrap();
        let e = got("e1");
        assert_eq!((e.class.as_str(), e.changes), ("read", false));
        let a = got("a1");
        assert_eq!((a.class.as_str(), a.changes), ("app", true));
        assert_eq!(log.entries_for(false).iter().filter(|e| e.changes).map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a1"]);
    }

    /// r4 F4: each data source sees only its own rows (what the window and Endpoint History
    /// read), rows from before round 4 are placed by their operation, sparkjob reads are reads,
    /// and reads nobody asked for come from SparkBox.
    #[tokio::test]
    async fn each_data_source_sees_only_its_own_rows() {
        let log = Log::new();
        // Before round 4: a Demo operation (its command simulated), a real command, a Mac row.
        let legacy = |e: Entry| Entry { demo: None, ..e };
        let mut old = vec![
            legacy(Entry { id: "d1".into(), op_id: Some("op-demo".into()), node: "cell1".into(), kind: "op".into(), title: "Push alert settings to cell1".into(), ..Default::default() }),
            legacy(Entry { id: "d2".into(), op_id: Some("op-demo".into()), node: "cell1".into(), kind: "command".into(), command: "cat > /etc/sparkbox-agent/alerts.json".into(), simulated: true, ..Default::default() }),
            legacy(Entry { id: "l1".into(), node: "cell2".into(), kind: "command".into(), command: "systemctl list-units --type=service --all --no-pager --output=json".into(), ..Default::default() }),
            legacy(Entry { id: "m1".into(), node: "mac".into(), kind: "app".into(), title: "Admin mode unlocked".into(), ..Default::default() }),
        ];
        for e in old.drain(..) {
            log.store(Entry { ts_ms: 1, updated_ms: 1, class: "change".into(), ..e }, true);
        }
        log.set_demo(true);
        log.record_simulated("cell3", "Restart vllm-worker on cell3", "docker restart vllm-worker");
        log.set_demo(false);
        let live_change = log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "docker restart vllm-head".into(), code: Some(0), ..Default::default() });
        log.record_app("Look-only mode on", "SPARKBOX_LOOK_ONLY=1");
        let ids = |v: Vec<Entry>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
        let live = ids(log.entries_for(false));
        let demo = ids(log.entries_for(true));
        assert!(live.contains(&"l1".to_string()) && live.contains(&live_change), "{live:?}");
        assert!(!live.iter().any(|i| i == "d1" || i == "d2" || i == "m1"), "no Demo rows in Live: {live:?}");
        assert!(demo.contains(&"d1".to_string()) && demo.contains(&"d2".to_string()) && demo.contains(&"m1".to_string()), "{demo:?}");
        assert!(!demo.contains(&live_change));
        assert_eq!(log.since_for(false, 0, 100).len(), live.len());
        assert_eq!(log.entries_for(false).iter().filter(|e| e.title == "Look-only mode on").count(), 1, "a Mac row goes with the source in use");

        // sparkjob's reads are reads, and a read outside any operation is SparkBox's own.
        for verb in ["ls '~'", "read '/etc/netplan/40-cx7.yaml'", "stat x", "du x", "df x", "summary x", "tree x y", "backups ~/.sparkbox/backups '/home/sparkuser/glm53/inner.sh'", "scan '~/models'", "list", "hash x"] {
            let id = log.record(Entry { node: "cell1".into(), kind: "command".into(), command: format!("python3 ~/.sparkbox/sparkjob.py {verb}"), code: Some(1), ..Default::default() });
            let e = log.entries().into_iter().find(|e| e.id == id).unwrap();
            assert_eq!((e.class.as_str(), e.changes, e.source.as_str()), ("read", false, "system"), "{verb}");
        }
        let id = log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "python3 ~/.sparkbox/sparkjob.py commit x".into(), ..Default::default() });
        assert_eq!(log.entries().into_iter().find(|e| e.id == id).unwrap().class, "change");
        // Escalation is found anywhere in the command, not only a leading "sudo ".
        let id = log.record(Entry { node: "cell1".into(), kind: "command".into(), command: "uptime && sudo reboot".into(), ..Default::default() });
        assert!(log.entries().into_iter().find(|e| e.id == id).unwrap().escalated);
    }

    /// P1.46: snapshot process and container command lines are masked before they leave the
    /// backend (what the UI and "Copy command line" see), and an old raw activity row is masked
    /// again when it's read back and when it's exported.
    #[test]
    fn secrets_in_snapshots_and_old_rows_are_masked() {
        // Process and container command lines with a secret-looking flag.
        let mut procs = vec![
            crate::model::Process { pid: 7, cmd: "vllm serve /models/m --api-key sk-abc1234567890 --port 8000".into(), ..Default::default() },
            crate::model::Process { pid: 8, cmd: "python3 train.py".into(), ..Default::default() },
        ];
        redact_processes(&mut procs);
        assert_eq!(procs[0].cmd, "vllm serve /models/m --api-key ‹redacted› --port 8000");
        assert_eq!(procs[1].cmd, "python3 train.py", "a plain command is untouched");
        let mut containers = vec![crate::model::Container { command: "serve --api-key sk-xyz0987654321".into(), ..Default::default() }];
        redact_containers(&mut containers);
        assert_eq!(containers[0].command, "serve --api-key ‹redacted›");

        // An old row written raw (an older build) is masked on read and on export.
        let path = temp("redact-old");
        let raw = Entry {
            id: "old1".into(),
            ts_ms: now_ms() - 3_600_000,
            node: "cell1".into(),
            kind: "sudo".into(),
            command: "vllm serve --api-key sk-raw9876543210 --port 8000".into(),
            class: "change".into(),
            changes: true,
            escalated: true,
            state: "ok".into(),
            code: Some(0),
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_string(&raw).unwrap() + "\n").unwrap();
        let log = Log::new();
        log.set_path(path);
        let on_read = &log.entries()[0];
        assert!(!on_read.command.contains("sk-raw") && on_read.command.contains("--api-key ‹redacted›"), "masked on read: {}", on_read.command);
        let json: Vec<Entry> = serde_json::from_str(&log.export("json", 0, u64::MAX, None).unwrap()).unwrap();
        assert!(json.iter().all(|e| !e.command.contains("sk-raw")), "masked on export");
    }

    #[test]
    fn since_returns_new_and_changed_entries() {
        let log = Log::new();
        let id = log.begin_op("cell1", "Fleet: uptime", "uptime", false);
        let mark = now_ms() + 1;
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert!(log.since_for(true, mark, 10).is_empty());
        log.end_op(&id, true, None, None);
        assert_eq!(log.since_for(true, mark, 10).len(), 1);
        assert!(log.since_for(false, 0, 10).is_empty(), "a Demo row isn't Live's");
    }

    /// When the fixture's day starts: a day ago.
    fn day_ago() -> u64 {
        now_ms() - 86_400_000
    }

    /// A restart of docker you asked for a day ago, then the update check's polls since (4 nodes
    /// each): `n` probe rows in poll operations, poll `i` at `day_ago() + 1 + i` ms.
    async fn a_change_then_polls(log: &Log, n: usize) -> (String, String) {
        let t0 = day_ago();
        log.set_demo(false);
        let restart = OpMeta { title: "Restart docker on cell2".into(), source: "ui".into() };
        let (change, read) = with_op_id("op-restart".into(), restart, async {
            let change = log.record(Entry { ts_ms: t0, node: "cell2".into(), kind: "sudo".into(), command: "systemctl restart docker".into(), code: Some(0), ..Default::default() });
            // The operation's own read stays with it.
            let read = log.record(Entry { ts_ms: t0, node: "cell2".into(), kind: "command".into(), command: "docker ps --format json".into(), code: Some(0), ..Default::default() });
            (change, read)
        })
        .await;
        for i in 0..n / 4 {
            let poll = OpMeta { title: "Check for update changes".into(), source: "poll".into() };
            with_op_id(format!("poll-stamps-{i}"), poll, as_probe(async {
                for node in ["cell1", "cell2", "cell3", "gx10-0000"] {
                    log.record(Entry { ts_ms: t0 + 1 + i as u64, node: node.into(), kind: "command".into(), command: "stat -c %Y /var/lib/dpkg/status".into(), code: Some(0), ..Default::default() });
                }
            }))
            .await;
        }
        (change, read)
    }

    #[tokio::test]
    async fn a_day_of_polls_never_pushes_a_change_out_of_the_window() {
        let log = Log::new();
        // A window's listing outside any operation is a check too.
        let listing = log.record(Entry { ts_ms: day_ago() - 1, node: "cell1".into(), kind: "command".into(), command: "ls -la /home/sparkuser".into(), code: Some(0), ..Default::default() });
        let (change, read) = a_change_then_polls(&log, 6000).await;
        let rows = log.entries_for(false);
        let ids: Vec<&str> = rows.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&change.as_str()) && ids.contains(&read.as_str()), "the change and its operation's read stay");
        assert!(!ids.contains(&listing.as_str()), "the oldest checks went first");
        assert_eq!(rows.iter().filter(|e| e.changes).map(|e| e.command.as_str()).collect::<Vec<_>>(), ["systemctl restart docker"], "Changes still has it");
        assert_eq!(rows.iter().filter(|e| is_check(e)).count(), KEEP_CHECKS, "the newest polls");
        let polls: HashSet<usize> = rows.iter().filter(|e| is_check(e)).filter_map(|e| e.op_id.as_deref()?.strip_prefix("poll-stamps-")?.parse().ok()).collect();
        assert_eq!(polls, ((6000 - KEEP_CHECKS) / 4..6000 / 4).collect(), "the newest 500 polls, whole");
        // The rest keeps 5000: past that its oldest go, the checks staying as they are.
        for i in 0..KEEP {
            log.record(Entry { node: "cell1".into(), kind: "command".into(), command: format!("touch /tmp/f{i}"), ..Default::default() });
        }
        let rows = log.entries_for(false);
        assert!(!rows.iter().any(|e| e.id == change), "the oldest change goes once 5000 newer ones came");
        assert_eq!((rows.len() - rows.iter().filter(|e| is_check(e)).count(), rows.iter().filter(|e| is_check(e)).count()), (KEEP, KEEP_CHECKS));
    }

    #[tokio::test]
    async fn after_a_restart_the_window_has_its_changes_back_from_under_a_day_of_polls() {
        let path = temp("polls");
        let log = Log::new();
        log.set_path(path.clone());
        let (change, read) = a_change_then_polls(&log, 6000).await;
        let again = Log::new();
        again.set_path(path);
        let rows = again.entries_for(false);
        assert!(rows.iter().any(|e| e.id == change && e.changes && e.state == "ok"));
        assert!(rows.iter().any(|e| e.id == read));
        assert_eq!(rows.iter().filter(|e| is_check(e)).count(), KEEP_CHECKS);
        assert_eq!(rows.len(), KEEP_CHECKS + 2);
    }

    /// The loader skips the lines `check_line` names without parsing them: it must agree with
    /// `is_check` on every kind of row as written.
    #[tokio::test]
    async fn a_check_is_told_the_same_from_its_line() {
        let log = Log::new();
        let row = |c: &str| Entry { node: "cell1".into(), kind: "command".into(), command: c.into(), code: Some(0), stdout_tail: Some(r#"{"class":"probe","opId":null,"source":"poll"}"#.into()), ..Default::default() };
        log.record(row("ls /"));
        log.record(row("touch /tmp/x"));
        as_probe(async { log.record(row("cat /proc/loadavg")) }).await;
        with_op_id("poll-9".into(), OpMeta { title: "Check updates".into(), source: "poll".into() }, as_probe(async { log.record(row("apt list --upgradable")) })).await;
        with_op_id("op-9".into(), OpMeta { title: "Upgrade cell2".into(), source: "ui".into() }, async {
            log.record(row("apt list --upgradable"));
            as_probe(async { log.record(row("dpkg -l")) }).await;
        })
        .await;
        let rows = log.entries();
        let told: Vec<(bool, bool)> = rows.iter().map(|e| (is_check(e), check_line(&serde_json::to_string(e).unwrap()))).collect();
        assert!(told.iter().all(|(a, b)| a == b), "{told:?}");
        assert_eq!(told.iter().filter(|t| t.0).count(), 3, "the listing, the probe and the poll's row; not the upgrade's");
    }
}
