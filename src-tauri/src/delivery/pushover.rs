//! Pushover (https://pushover.net): the app token and the user key from the
//! Keychain, optionally one device. Priority: critical 1 (high, bypasses quiet
//! hours on the phone), warning 0, information, tests and resolved alerts -1.

use super::{http_failure, network_failure, Channel, Failure, Message};
use crate::health::Severity;

pub const API: &str = "https://api.pushover.net/1/messages.json";

pub fn priority(msg: &Message) -> i8 {
    match (msg.state.as_str(), msg.severity) {
        ("test" | "resolved", _) => -1,
        (_, Severity::Crit) => 1,
        (_, Severity::Warn) => 0,
        (_, Severity::Info) => -1,
    }
}

pub fn form(ch: &Channel, app: &str, user: &str, msg: &Message) -> Vec<(&'static str, String)> {
    let mut f = vec![
        ("token", app.to_string()),
        ("user", user.to_string()),
        ("title", msg.title.clone()),
        ("message", msg.body.clone()),
        ("priority", priority(msg).to_string()),
        ("timestamp", (msg.ts_ms / 1000).to_string()),
    ];
    if let Some(d) = ch.device.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        f.push(("device", d.to_string()));
    }
    f
}

pub async fn send(ch: &Channel, app: Option<String>, user: Option<String>, msg: &Message, http: &reqwest::Client, api: &str) -> Result<(), Failure> {
    let (Some(app), Some(user)) = (app.filter(|s| !s.is_empty()), user.filter(|s| !s.is_empty())) else {
        return Err(Failure::new("Pushover needs the app token and your user key", false));
    };
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form(ch, &app, &user, msg)).finish();
    let resp = http
        .post(api)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| network_failure("api.pushover.net", &e))?;
    let status = resp.status().as_u16();
    // {"status":1,"request":"…"} on success; {"status":0,"errors":["application token is invalid"]} otherwise.
    let v = resp.json::<serde_json::Value>().await.unwrap_or_default();
    if (200..300).contains(&status) && v.get("status").and_then(|s| s.as_i64()) == Some(1) {
        return Ok(());
    }
    let detail = v.get("errors").and_then(|e| e.get(0)).and_then(|e| e.as_str()).map(String::from);
    Err(http_failure(status, detail.as_deref(), "app token and user key"))
}
