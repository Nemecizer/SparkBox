//! Errors the UI can act on. Commands whose callers we own (ops, power, admin, fleet)
//! return `SbError`, serialised as `{kind, message, node?, detail?}`; the front end
//! (src/lib/errors.ts) turns the kind into a plain sentence and a button that fixes it.
//! Commands that return plain strings get the same shape from their gate: `admin::require…`
//! refuses with `SbError::to_json`, and the UI (src/lib/api.ts) turns that string back into
//! the object, so a locked call looks the same whatever the command returns. `SbError::from`
//! reads both the JSON and the Live layer's plain sentences.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// Admin mode is locked.
    Locked,
    /// Admin mode is unlocked without Touch ID (Demo), and this changes something on the Mac:
    /// confirm it's you first (`admin_authenticate`).
    #[serde(rename = "needs_auth")]
    NeedsAuth,
    /// The node refused the SSH login.
    AuthRejected,
    /// sudo refused the cluster password, or the user can't use sudo.
    SudoRejected,
    /// No cluster password saved yet.
    NoPassword,
    /// The node didn't answer in time.
    Timeout,
    /// The node isn't connected (offline, or the VPN is down).
    Unreachable,
    /// The action needs Live data.
    DemoOnly,
    /// A name, signal or action SparkBox won't pass on.
    Invalid,
    /// The process or container is already gone.
    Gone,
    /// Stopped by the user.
    Cancelled,
    /// Ran, but the node reported a failure.
    Failed,
    /// Look-only mode (SPARKBOX_LOOK_ONLY=1, or Settings › Security › Look only): SparkBox starts
    /// no change on the cluster or this Mac, and nothing asks for Touch ID.
    LookOnly,
    /// The write exit refused a change outside the open fence (none open, another node or
    /// class, expired, failed its check, for the other build).
    Fenced,
    /// The other SparkBox (release or dev) holds the node's controller lease, or made the container.
    OtherController,
    /// The machine answering isn't the one SparkBox pinned as this node.
    WrongNode,
}

/// What `SbError::words` accepts as a refusal in the middle of a sentence: `to_json`'s fields and
/// no others. Any kind will do, so a kind this build doesn't know still doesn't show its JSON,
/// but a field a refusal never has means it's someone else's JSON (Kubernetes also says
/// `{"kind":"Status",…"message":…}`), and the parse stops there.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Written {
    kind: String,
    message: String,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SbError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Extra text for a Details disclosure (stderr, exit code).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SbError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> SbError {
        SbError { kind, message: message.into(), node: None, detail: None }
    }

    pub fn on(mut self, node: &str) -> SbError {
        self.node = Some(node.to_string());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> SbError {
        let d = detail.into();
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }

    pub fn invalid(message: impl Into<String>) -> SbError {
        SbError::new(ErrorKind::Invalid, message)
    }

    pub fn failed(message: impl Into<String>) -> SbError {
        SbError::new(ErrorKind::Failed, message)
    }

    /// The error as one JSON string, for commands that return `Result<_, String>`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| self.message.clone())
    }

    /// An error `to_json` wrote, read back.
    pub fn from_json(text: &str) -> Option<SbError> {
        text.trim_start().starts_with("{\"kind\"").then(|| serde_json::from_str(text).ok()).flatten()
    }

    /// An error as a person reads it: any refusal `to_json` wrote, wherever it sits in the text
    /// ("cell2: {\"kind\":…"), becomes its message, and the rest stays as it is. Every string that
    /// ends up in a load's message, an alert or a task goes through this, so a refusal from the
    /// write exit never shows its JSON.
    pub fn words(text: &str) -> String {
        const MARK: &str = "{\"kind\"";
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find(MARK) {
            let (before, json) = rest.split_at(at);
            out.push_str(before);
            let mut found = serde_json::Deserializer::from_str(json).into_iter::<Written>();
            match found.next() {
                Some(Ok(w)) => {
                    // A wrapped error can hold another inside its message.
                    out.push_str(&SbError::words(&w.message));
                    rest = &json[found.byte_offset()..];
                }
                // Text that only looks like one stays, and the search goes on after it.
                _ => {
                    out.push_str(MARK);
                    rest = &json[MARK.len()..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Reads one of the Live layer's error strings (or a gate's JSON) and works out what kind it is.
    pub fn classify(message: &str) -> SbError {
        if let Some(e) = SbError::from_json(message) {
            return e;
        }
        let message = &SbError::words(message);
        let m = message.to_lowercase();
        let kind = if m.contains("look-only mode") {
            ErrorKind::LookOnly
        } else if m.contains("open fence") || m.contains("the fence ") || m.contains("fence record") || m.contains("fence key") || m.contains("no fence is open") {
            ErrorKind::Fenced
        } else if m.contains("is controlling") || m.contains("managed by the other sparkbox") {
            ErrorKind::OtherController
        } else if m.contains(" answers as ") || m.contains("pinned which machine") {
            ErrorKind::WrongNode
        } else if m.contains("admin mode is locked") {
            ErrorKind::Locked
        } else if m.contains(&crate::admin::NEEDS_AUTH.to_lowercase()) || m.contains(&crate::admin::NEEDS_CONFIRM.to_lowercase()) {
            ErrorKind::NeedsAuth
        } else if m.contains("sudo") && (m.contains("rejected") || m.contains("incorrect password") || m.contains("sudoers") || m.contains("can't use sudo")) {
            ErrorKind::SudoRejected
        } else if m.contains("save the cluster password") {
            ErrorKind::NoPassword
        } else if m.contains("didn't accept the password") || m.contains("authentication failed") || m.contains("password was rejected") {
            ErrorKind::AuthRejected
        } else if m.contains("timed out") || m.contains("timeout") || m.contains("no reply") {
            ErrorKind::Timeout
        } else if m.contains("isn't connected") || m.contains("not connected") || m.contains("unreachable") || m.contains("connection refused") {
            ErrorKind::Unreachable
        } else if m.contains("demo mode") || m.contains("switch to live") {
            ErrorKind::DemoOnly
        } else if m.contains("cancelled") || m.contains("canceled") {
            ErrorKind::Cancelled
        } else if m.contains("already exited") || m.contains("no such process") || m.contains("no such container") {
            ErrorKind::Gone
        } else if m.contains("unsupported") || m.contains("won't pass") || m.contains("bad ") {
            ErrorKind::Invalid
        } else {
            ErrorKind::Failed
        };
        SbError::new(kind, message.trim().trim_start_matches("Error: "))
    }
}

impl std::fmt::Display for SbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for SbError {
    fn from(message: String) -> SbError {
        SbError::classify(&message)
    }
}

impl From<&str> for SbError {
    fn from(message: &str) -> SbError {
        SbError::classify(message)
    }
}

impl From<SbError> for String {
    fn from(e: SbError) -> String {
        e.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_messages_get_a_kind_the_ui_can_act_on() {
        let k = |m: &str| SbError::classify(m).kind;
        assert_eq!(k("sudo on cell2 rejected the cluster password."), ErrorKind::SudoRejected);
        assert_eq!(k("The cluster user can't use sudo on cell2."), ErrorKind::SudoRejected);
        assert_eq!(k("Save the cluster password in Settings first."), ErrorKind::NoPassword);
        assert_eq!(k("cell3 isn't connected. Use Live data (Settings › Data source) and check the node is online."), ErrorKind::Unreachable);
        assert_eq!(k("SSH timeout: no reply from 192.168.50.98:22 in 15 s"), ErrorKind::Timeout);
        assert_eq!(k("Admin mode is locked. Unlock it in the top bar to make changes."), ErrorKind::Locked);
        assert_eq!(k("docker restart failed on cell1: boom"), ErrorKind::Failed);
        assert_eq!(k(crate::admin::NEEDS_AUTH), ErrorKind::NeedsAuth);
        assert_eq!(serde_json::to_value(ErrorKind::NeedsAuth).unwrap(), "needs_auth");
        assert_eq!(k(crate::admin::LOOK_ONLY), ErrorKind::LookOnly);
        assert_eq!(serde_json::to_value(ErrorKind::LookOnly).unwrap(), "lookOnly");
        // The write exit's refusals (live/exit.rs), whether they arrive as JSON or as words.
        assert_eq!(k("cell2 isn't in the open fence."), ErrorKind::Fenced);
        assert_eq!(k("The fence doesn't allow service changes."), ErrorKind::Fenced);
        assert_eq!(k("The fence expired at 14:05."), ErrorKind::Fenced);
        assert_eq!(k("The fence record failed its check: open a new one."), ErrorKind::Fenced);
        assert_eq!(k("SparkBox (dev) is controlling cell2 right now."), ErrorKind::OtherController);
        assert_eq!(k("glm53 on cell2 is managed by the other SparkBox (release)."), ErrorKind::OtherController);
        assert_eq!(k("cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."), ErrorKind::WrongNode);
        for (kind, word) in [(ErrorKind::Fenced, "fenced"), (ErrorKind::OtherController, "otherController"), (ErrorKind::WrongNode, "wrongNode")] {
            assert_eq!(serde_json::to_value(kind).unwrap(), word);
            let e = SbError::new(kind, "x").on("cell2");
            assert_eq!(SbError::from(e.to_json()), e, "{word} reads back from its JSON");
        }
    }

    #[test]
    fn a_gate_refusal_reads_back_from_its_json() {
        let e = SbError::new(ErrorKind::NeedsAuth, crate::admin::NEEDS_AUTH);
        let text = e.to_json();
        assert!(text.starts_with("{\"kind\":\"needs_auth\""), "{text}");
        assert_eq!(SbError::classify(&text), e);
        assert_eq!(SbError::from(text), e, "a String error from require() keeps its kind through `?`");
        assert_eq!(SbError::from_json("Admin mode is locked."), None);
        assert_eq!(SbError::words(&SbError::new(ErrorKind::Fenced, "No fence is open.").on("cell1").to_json()), "No fence is open.");
        assert_eq!(SbError::words("Admin mode is locked."), "Admin mode is locked.");
        assert_eq!(SbError::words("{\"kind\": not json"), "{\"kind\": not json", "text that only looks like it stays");
    }

    /// The text comes from a node's or a server's output, so it can be anything. Every shape here
    /// must be a straight read: the parse stops at the first field that isn't a refusal's.
    #[test]
    fn words_stays_quick_on_long_text_that_never_closes() {
        let size = 250_000;
        let repeated = |unit: &str| unit.repeat(size / unit.len());
        for (name, text) in [
            ("marks", repeated("{\"kind\"")),
            ("open strings", repeated("{\"kind\":\"")),
            ("nesting", repeated("{\"kind\":\"a\",\"x\":")),
            ("arrays", repeated("{\"kind\":\"a\",\"x\":[")),
            ("no message", repeated("{\"kind\":\"a\"}")),
            ("one unclosed string", format!("{{\"kind\":\"a\",\"message\":\"{}", "x".repeat(size))),
        ] {
            let t = std::time::Instant::now();
            let out = SbError::words(&text);
            assert!(t.elapsed() < std::time::Duration::from_secs(2), "{name}: {:?}", t.elapsed());
            assert_eq!(out.len(), text.len(), "{name} isn't a refusal, so it stays");
        }
    }

    #[test]
    fn words_finds_a_refusal_anywhere_in_a_sentence() {
        let e = SbError::new(ErrorKind::Fenced, "No fence is open.").on("cell2").to_json();
        assert_eq!(SbError::words(&format!("cell2: {e}")), "cell2: No fence is open.");
        assert_eq!(SbError::words(&format!("Lost contact with cell1: {e}")), "Lost contact with cell1: No fence is open.");
        assert_eq!(SbError::words(&format!("Engine not healthy: {e} (2 tries)")), "Engine not healthy: No fence is open. (2 tries)", "what follows it stays");
        let look = SbError::new(ErrorKind::LookOnly, "Look only.").to_json();
        assert_eq!(SbError::words(&format!("cell1: {e}; cell2: {look}")), "cell1: No fence is open.; cell2: Look only.", "every one of them");
        // One wrapped in another, a kind this build doesn't know, and a mark that leads nowhere.
        let inner = SbError::new(ErrorKind::Failed, format!("cell2: {e}")).to_json();
        assert_eq!(SbError::words(&inner), "cell2: No fence is open.");
        assert_eq!(SbError::words(r#"cell3: {"kind":"fromTheFuture","message":"Later."}"#), "cell3: Later.");
        let broken = format!(r#"{{"kind": oops then {e}"#);
        assert_eq!(SbError::words(&broken), r#"{"kind": oops then No fence is open."#);
        assert_eq!(SbError::words(r#"{"kind":"Pod","spec":{}}"#), r#"{"kind":"Pod","spec":{}}"#, "JSON that isn't a refusal stays");
        assert_eq!(SbError::words(""), "");
        // Its own fields all count, and any other means it isn't ours.
        let full = SbError::new(ErrorKind::Failed, "It failed.").on("cell2").with_detail("exit 137: killed").to_json();
        assert_eq!(SbError::words(&format!("cell2: {full}")), "cell2: It failed.");
        let k8s = r#"Error from server: {"kind":"Status","apiVersion":"v1","metadata":{},"status":"Failure","message":"pods \"x\" not found","reason":"NotFound","code":404}"#;
        assert_eq!(SbError::words(k8s), k8s, "Kubernetes says kind and message too");
        let spaced = r#"{"kind": "Event", "message": "started", "count": 3}"#;
        assert_eq!(SbError::words(spaced), spaced);
        // A refusal inside a sentence keeps its kind and loses its JSON.
        let c = SbError::classify(&format!("cell2: {e}"));
        assert_eq!((c.kind, c.message.as_str()), (ErrorKind::Fenced, "cell2: No fence is open."));
    }

    #[test]
    fn serialises_as_kind_and_message() {
        let e = SbError::new(ErrorKind::Gone, "Process already exited").on("cell1");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "gone", "message": "Process already exited", "node": "cell1"}));
    }
}
