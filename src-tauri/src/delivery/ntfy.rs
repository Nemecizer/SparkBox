//! ntfy (https://ntfy.sh or a self-hosted server): one JSON publish per alert, so
//! titles with "·" and "°C" arrive intact. Priority: critical 5 (urgent, breaks
//! through Do Not Disturb on the phone), warning 3, information and tests 2.

use serde_json::json;

use super::{http_failure, network_failure, Channel, Failure, Message};
use crate::health::Severity;

pub const DEFAULT_SERVER: &str = "https://ntfy.sh";

pub fn priority(msg: &Message) -> u8 {
    match (msg.state.as_str(), msg.severity) {
        ("test", _) => 2,
        ("resolved", _) => 3,
        (_, Severity::Crit) => 5,
        (_, Severity::Warn) => 3,
        (_, Severity::Info) => 2,
    }
}

fn tags(msg: &Message) -> Vec<&'static str> {
    match (msg.state.as_str(), msg.severity) {
        ("test", _) => vec!["test_tube"],
        ("resolved", _) => vec!["white_check_mark"],
        (_, Severity::Crit) => vec!["rotating_light"],
        _ => vec!["warning"],
    }
}

pub fn body(ch: &Channel, msg: &Message) -> serde_json::Value {
    json!({
        "topic": ch.topic.as_deref().unwrap_or("").trim(),
        "title": msg.title,
        "message": msg.body,
        "priority": priority(msg),
        "tags": tags(msg),
    })
}

pub async fn send(ch: &Channel, token: Option<String>, msg: &Message, http: &reqwest::Client) -> Result<(), Failure> {
    let server = ch.server.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SERVER).trim_end_matches('/').to_string();
    let host = super::http_url(&server).map_err(|e| Failure::new(e, false))?.host_str().unwrap_or_default().to_string();
    let mut req = http.post(&server).json(&body(ch, msg));
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| network_failure(&host, &e))?;
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    // ntfy explains itself: {"code":40101,"http":401,"error":"unauthorized"}.
    let detail = resp.json::<serde_json::Value>().await.ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from));
    Err(http_failure(status, detail.as_deref(), "token"))
}
