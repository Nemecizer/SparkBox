//! A webhook: a JSON POST describing the alert, signed with HMAC-SHA256 of the
//! exact body when a secret is set (`X-SparkBox-Signature: sha256=<hex>`), so the
//! receiver can check it came from SparkBox. "Slack-compatible" sends
//! `{"text": …}` instead, which Slack, Mattermost and Discord's /slack take.

use serde_json::json;

use super::{hmac_sha256_hex, http_failure, http_url, network_failure, Channel, Failure, Message};

pub const SIGNATURE_HEADER: &str = "X-SparkBox-Signature";

pub fn body(ch: &Channel, msg: &Message) -> Vec<u8> {
    let v = if ch.slack.unwrap_or(false) {
        json!({ "text": format!("*{}*\n{}", msg.title, msg.body) })
    } else {
        json!({
            "source": "sparkbox",
            "title": msg.title,
            "body": msg.body,
            "severity": msg.severity,
            "state": msg.state,
            "node": msg.node,
            "rule": msg.rule,
            "alertId": msg.alert_id,
            "tsMs": msg.ts_ms,
        })
    };
    serde_json::to_vec(&v).unwrap_or_default()
}

/// `url` is the webhook's URL from the Keychain (it's a credential); `secret` its signing secret.
pub async fn send(ch: &Channel, url: Option<String>, secret: Option<String>, msg: &Message, http: &reqwest::Client) -> Result<(), Failure> {
    let url = http_url(url.as_deref().unwrap_or("")).map_err(|e| Failure::new(e, false))?;
    let host = url.host_str().unwrap_or_default().to_string();
    let bytes = body(ch, msg);
    let mut req = http.post(url).header("Content-Type", "application/json");
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        req = req.header(SIGNATURE_HEADER, format!("sha256={}", hmac_sha256_hex(s.as_bytes(), &bytes)));
    }
    let resp = req.body(bytes).send().await.map_err(|e| network_failure(&host, &e))?;
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    let detail = text.trim().chars().take(120).collect::<String>();
    Err(http_failure(status, Some(&detail), "URL and secret"))
}
