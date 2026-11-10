//! The dead-man's switch: a URL at a service that pages the owner when it stops
//! hearing from SparkBox (healthchecks.io, an Uptime Kuma push monitor, Cronitor).
//! The leader node agent GETs it every 60 s (sparkbox_agent.py); if the house loses
//! power or internet, the pings stop and that service pages on its own. It is never
//! sent alerts. The Mac only pings it once for the Test button.

use std::time::Instant;

use super::{http_url, network_failure, Failure};

/// "healthchecks.io" for its ping host, otherwise the URL's host.
pub fn service(url: &str) -> String {
    let host = url::Url::parse(url.trim()).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
    match host.as_str() {
        "hc-ping.com" | "healthchecks.io" => "healthchecks.io".into(),
        h if h.ends_with("cronitor.link") || h.ends_with("cronitor.io") => "Cronitor".into(),
        h => h.to_string(),
    }
}

/// One GET, as an agent pings it. Returns the HTTP status and how long it took.
pub async fn ping(url: &str, http: &reqwest::Client) -> Result<(u16, u64), Failure> {
    let u = http_url(url).map_err(|e| Failure::new(e, false))?;
    let host = u.host_str().unwrap_or_default().to_string();
    let started = Instant::now();
    let resp = http.get(u).send().await.map_err(|e| network_failure(&host, &e))?;
    let status = resp.status().as_u16();
    let ms = started.elapsed().as_millis() as u64;
    if (200..300).contains(&status) {
        Ok((status, ms))
    } else {
        Err(super::http_failure(status, None, "check's address"))
    }
}
