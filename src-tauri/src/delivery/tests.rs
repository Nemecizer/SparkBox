//! Every channel against a local server that records what arrived: the method,
//! path, headers and body for the HTTP ones, the whole conversation for SMTP.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use super::*;

#[derive(Clone, Debug, Default)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// An HTTP/1.1 server on 127.0.0.1 that answers every request with `status` and `reply`.
async fn http_server(status: u16, reply: &'static str) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let log = log.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).await.unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
                let mut body = vec![0; len];
                r.read_exact(&mut body).await.unwrap();
                log.lock().unwrap().push(Request { method, path, headers, body });
                let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
                w.write_all(resp.as_bytes()).await.unwrap();
                w.shutdown().await.ok();
            });
        }
    });
    (addr, seen)
}

fn message(severity: Severity, state: &str) -> Message {
    Message {
        title: "gx10-0000 GPU Xid 79".into(),
        body: "GPU on gx10-0000 — Xid 79 · the GPU stopped answering on PCIe".into(),
        severity,
        state: state.into(),
        node: Some("gx10-0000".into()),
        rule: Some("kernel-xid".into()),
        alert_id: Some("kernel-xid:gpu:gx10-0000:xid-79".into()),
        ts_ms: 1_793_196_600_000,
    }
}

fn channel(kind: Kind) -> Channel {
    Channel::empty("phone", kind, "Phone")
}

fn none(_: &str) -> Option<String> {
    None
}

#[tokio::test]
async fn ntfy_publishes_json_with_priority_tags_and_the_token() {
    let (addr, seen) = http_server(200, r#"{"id":"abc"}"#).await;
    let ch = Channel { server: Some(addr), topic: Some("sparkbox-alerts".into()), ..channel(Kind::Ntfy) };
    let token = |f: &str| (f == "token").then(|| "tk_secret".to_string());
    send(&ch, &token, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap();
    let r = seen.lock().unwrap()[0].clone();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/"));
    assert_eq!(r.header("authorization"), Some("Bearer tk_secret"));
    let b = r.json();
    assert_eq!(b["topic"], "sparkbox-alerts");
    assert_eq!(b["title"], "gx10-0000 GPU Xid 79");
    assert_eq!((b["priority"].as_u64(), b["tags"][0].as_str()), (Some(5), Some("rotating_light")));
    // A warning is priority 3 and goes without a token when none is saved.
    send(&ch, &none, &message(Severity::Warn, "firing"), &client(), &Endpoints::default()).await.unwrap();
    let r = seen.lock().unwrap()[1].clone();
    assert_eq!(r.json()["priority"], 3);
    assert_eq!(r.header("authorization"), None);
}

#[tokio::test]
async fn a_rejected_token_says_what_to_check_and_isnt_retried() {
    let (addr, _) = http_server(401, r#"{"code":40101,"http":401,"error":"unauthorized"}"#).await;
    let ch = Channel { server: Some(addr), topic: Some("t".into()), ..channel(Kind::Ntfy) };
    let e = send(&ch, &none, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap_err();
    assert_eq!(e.message, "HTTP 401 — check the token (unauthorized)");
    assert!(!e.retry);
    let (addr, _) = http_server(503, "{}").await;
    let ch = Channel { server: Some(addr), ..ch };
    let e = send(&ch, &none, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap_err();
    assert!(e.retry, "a server error may pass: {e:?}");
    // Nobody listening at all.
    let ch = Channel { server: Some("http://127.0.0.1:9".into()), ..ch };
    let e = send(&ch, &none, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap_err();
    assert!(e.message.starts_with("Couldn't reach 127.0.0.1"), "{}", e.message);
    assert!(e.retry);
}

#[tokio::test]
async fn pushover_posts_the_form_with_priority_and_device() {
    let (addr, seen) = http_server(200, r#"{"status":1,"request":"5042853c"}"#).await;
    let ch = Channel { device: Some("iphone".into()), ..channel(Kind::Pushover) };
    let keys = |f: &str| match f {
        "appToken" => Some("azGDORePK8gMaC0QOYAMyEEuzJnyUi".to_string()),
        "userKey" => Some("uQiRzpo4DXghDmr9QzzfQu27cmVRsG".to_string()),
        _ => None,
    };
    let ep = Endpoints { pushover: format!("{addr}/1/messages.json") };
    send(&ch, &keys, &message(Severity::Crit, "firing"), &client(), &ep).await.unwrap();
    let r = seen.lock().unwrap()[0].clone();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/1/messages.json"));
    assert_eq!(r.header("content-type"), Some("application/x-www-form-urlencoded"));
    let form: Vec<(String, String)> = url::form_urlencoded::parse(&r.body).into_owned().collect();
    let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    assert_eq!(get("token"), Some("azGDORePK8gMaC0QOYAMyEEuzJnyUi"));
    assert_eq!(get("user"), Some("uQiRzpo4DXghDmr9QzzfQu27cmVRsG"));
    assert_eq!((get("priority"), get("device"), get("title")), (Some("1"), Some("iphone"), Some("gx10-0000 GPU Xid 79")));
    assert_eq!(pushover::priority(&message(Severity::Warn, "firing")), 0);

    let (addr, _) = http_server(400, r#"{"token":"invalid","errors":["application token is invalid"],"status":0}"#).await;
    let ep = Endpoints { pushover: addr };
    let e = send(&ch, &keys, &message(Severity::Crit, "firing"), &client(), &ep).await.unwrap_err();
    assert_eq!(e.message, "HTTP 400 — application token is invalid (application token is invalid)");
}

#[tokio::test]
async fn the_webhook_is_signed_over_the_exact_body() {
    let (addr, seen) = http_server(204, "").await;
    let ch = Channel { url: Some(format!("{addr}/hooks/sparkbox")), ..channel(Kind::Webhook) };
    let secret = |f: &str| (f == "secret").then(|| "s3cret".to_string());
    send(&ch, &secret, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap();
    let r = seen.lock().unwrap()[0].clone();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/hooks/sparkbox"));
    assert_eq!(r.header("content-type"), Some("application/json"));
    let sig = r.header("x-sparkbox-signature").unwrap();
    assert_eq!(sig, format!("sha256={}", hmac_sha256_hex(b"s3cret", &r.body)));
    let b = r.json();
    assert_eq!((b["source"].as_str(), b["severity"].as_str(), b["state"].as_str()), (Some("sparkbox"), Some("crit"), Some("firing")));
    assert_eq!(b["node"], "gx10-0000");

    // Slack-compatible: {"text": …} only, and no signature without a secret.
    let ch = Channel { slack: Some(true), ..ch };
    send(&ch, &none, &message(Severity::Warn, "firing"), &client(), &Endpoints::default()).await.unwrap();
    let r = seen.lock().unwrap()[1].clone();
    assert_eq!(r.json(), serde_json::json!({ "text": "*gx10-0000 GPU Xid 79*\nGPU on gx10-0000 — Xid 79 · the GPU stopped answering on PCIe" }));
    assert_eq!(r.header("x-sparkbox-signature"), None);
}

#[test]
fn hmac_matches_rfc_4231() {
    // Test case 2 of RFC 4231.
    assert_eq!(hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    // Test case 6: a key longer than the block is hashed first.
    let key = [0xaa_u8; 131];
    assert_eq!(hmac_sha256_hex(&key, b"Test Using Larger Than Block-Size Key - Hash Key First"), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
}

/// A minimal SMTP server: greets, takes EHLO, AUTH PLAIN, MAIL, RCPT and DATA, and keeps the transcript.
async fn smtp_server(auth_ok: bool) -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let transcript = Arc::new(Mutex::new(String::new()));
    let t = transcript.clone();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (r, mut w) = sock.into_split();
        let mut r = BufReader::new(r);
        w.write_all(b"220 localhost ESMTP test\r\n").await.unwrap();
        let mut in_data = false;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            t.lock().unwrap().push_str(&line);
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    w.write_all(b"250 2.0.0 queued as 1A2B\r\n").await.unwrap();
                }
                continue;
            }
            let upper = line.to_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") {
                b"250-localhost\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
            } else if upper.starts_with("AUTH") {
                if auth_ok {
                    b"235 2.7.0 Authentication successful\r\n"
                } else {
                    b"535 5.7.8 Authentication credentials invalid\r\n"
                }
            } else if upper.starts_with("MAIL") || upper.starts_with("RCPT") {
                b"250 2.1.0 Ok\r\n"
            } else if upper.starts_with("DATA") {
                in_data = true;
                b"354 End data with <CR><LF>.<CR><LF>\r\n"
            } else if upper.starts_with("QUIT") {
                w.write_all(b"221 2.0.0 Bye\r\n").await.unwrap();
                return;
            } else {
                b"250 Ok\r\n"
            };
            w.write_all(reply).await.unwrap();
        }
    });
    (port, transcript)
}

fn email(port: u16) -> Channel {
    Channel {
        host: Some("127.0.0.1".into()),
        port: Some(port),
        security: Some("none".into()),
        user: Some("alerts@example.com".into()),
        from: Some("SparkBox <alerts@example.com>".into()),
        to: Some("owner@example.com, oncall@example.com".into()),
        ..channel(Kind::Email)
    }
}

#[tokio::test]
async fn email_goes_through_smtp_with_auth_and_every_recipient() {
    let (port, transcript) = smtp_server(true).await;
    let pw = |f: &str| (f == "password").then(|| "app-password".to_string());
    send(&email(port), &pw, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap();
    let t = transcript.lock().unwrap().clone();
    use base64::Engine;
    let plain = base64::engine::general_purpose::STANDARD.encode("\0alerts@example.com\0app-password");
    assert!(t.contains(&format!("AUTH PLAIN {plain}")), "{t}");
    assert!(t.contains("MAIL FROM:<alerts@example.com>"), "{t}");
    assert!(t.contains("RCPT TO:<owner@example.com>") && t.contains("RCPT TO:<oncall@example.com>"), "{t}");
    assert!(t.contains("Subject: [SparkBox] gx10-0000 GPU Xid 79"), "{t}");
    assert!(t.contains("the GPU stopped answering on PCIe"), "{t}");
}

#[tokio::test]
async fn a_refused_smtp_login_says_to_check_the_password() {
    let (port, _) = smtp_server(false).await;
    let pw = |f: &str| (f == "password").then(|| "wrong".to_string());
    let e = send(&email(port), &pw, &message(Severity::Crit, "firing"), &client(), &Endpoints::default()).await.unwrap_err();
    assert_eq!(e.message, "SMTP 535 — check the user and password");
    assert!(!e.retry);
}

#[test]
fn channels_are_checked_before_they_are_saved() {
    let none_saved = |_: &str| false;
    let lan: &[String] = &[];
    assert!(Channel { topic: Some("t".into()), ..channel(Kind::Ntfy) }.validate(none_saved, lan).is_ok());
    assert_eq!(channel(Kind::Ntfy).validate(none_saved, lan).unwrap_err(), "Phone: enter the ntfy topic.");
    assert!(channel(Kind::Pushover).validate(none_saved, lan).unwrap_err().contains("app token and your user key"));
    assert!(channel(Kind::Pushover).validate(|_| true, lan).is_ok());
    assert!(Channel { url: Some("ftp://x".into()), ..channel(Kind::Webhook) }.validate(none_saved, lan).unwrap_err().contains("http or https"));
    let mail = Channel { host: Some("smtp.example.com".into()), from: Some("a@example.com".into()), to: Some("b@example.com".into()), security: Some("none".into()), ..channel(Kind::Email) };
    assert!(mail.validate(none_saved, lan).unwrap_err().contains("won't send a password unencrypted"));
    assert!(Channel { security: Some("starttls".into()), ..mail.clone() }.validate(none_saved, lan).is_ok());
    assert!(Channel { to: Some("not an address".into()), security: Some("ssl".into()), ..mail }.validate(none_saved, lan).unwrap_err().contains("isn't an email address"));
    assert!(Channel { id: "a b".into(), ..channel(Kind::Mac) }.validate(none_saved, lan).is_err());
}

#[test]
fn ntfy_requires_https_off_lan() {
    let none_saved = |_: &str| false;
    let nodes = ["cell1.lab".to_string(), "10.20.0.11".to_string()];
    let ntfy = |server: &str| Channel { server: Some(server.into()), topic: Some("t".into()), ..channel(Kind::Ntfy) }.validate(none_saved, &nodes);
    assert_eq!(ntfy("http://ntfy.example.com").unwrap_err(), "Phone: Use https for ntfy.example.com: plain http would send the token and every alert unencrypted.");
    assert!(ntfy("https://ntfy.example.com").is_ok());
    // This Mac, a node, a private or link-local address, a local name: plain http is fine.
    for ok in ["http://127.0.0.1:2586", "http://localhost", "http://cell1.lab:8080", "http://192.168.1.20", "http://10.20.0.11", "http://172.20.1.5", "http://169.254.3.3", "http://ntfy.local", "http://ntfybox", "http://[fd12::1]:80"] {
        assert!(ntfy(ok).is_ok(), "{ok}");
    }
    for bad in ["http://8.8.8.8", "http://172.32.0.1", "http://[2001:db8::1]"] {
        assert!(ntfy(bad).unwrap_err().contains("Use https"), "{bad}");
    }
    // Webhooks and the dead-man's switch follow the same rule.
    let hook = Channel { url: Some("http://hooks.example.com/x".into()), ..channel(Kind::Webhook) };
    assert!(hook.validate(none_saved, &nodes).unwrap_err().contains("plain http would send every alert unencrypted"));
    let hb = Channel { url: Some("http://hc-ping.com/abc".into()), ..channel(Kind::Heartbeat) };
    assert!(hb.validate(none_saved, &nodes).unwrap_err().contains("Use https for hc-ping.com"));
    let hb = Channel { url: Some("https://hc-ping.com/abc".into()), mac_only: true, ..channel(Kind::Heartbeat) };
    assert!(hb.validate(none_saved, &nodes).unwrap_err().contains("can't be this Mac only"));
}

#[tokio::test]
async fn heartbeat_get() {
    let (addr, seen) = http_server(200, "OK").await;
    let ch = Channel { url: Some(format!("{addr}/ping/5c1e")), ..channel(Kind::Heartbeat) };
    let (status, _ms) = heartbeat::ping(ch.url.as_deref().unwrap(), &client()).await.unwrap();
    assert_eq!(status, 200);
    let r = seen.lock().unwrap()[0].clone();
    assert_eq!((r.method.as_str(), r.path.as_str(), r.body.len()), ("GET", "/ping/5c1e", 0), "one plain GET, no alert in it");
    // The channel's Test sends the same GET through `send`.
    send(&ch, &none, &message(Severity::Crit, "test"), &client(), &Endpoints::default()).await.unwrap();
    assert_eq!(seen.lock().unwrap()[1].method, "GET");
    let (addr, _) = http_server(404, "not found").await;
    let e = heartbeat::ping(&format!("{addr}/ping/gone"), &client()).await.unwrap_err();
    assert_eq!(e.message, "HTTP 404 — check the address");
    assert_eq!(heartbeat::service("https://hc-ping.com/5c1e"), "healthchecks.io");
    assert_eq!(heartbeat::service("https://kuma.example.com/api/push/x"), "kuma.example.com");
    assert!(!Kind::Heartbeat.pages() && !Kind::Mac.pages() && Kind::Ntfy.pages(), "never a notification target");
}

#[test]
fn retries_back_off_then_keep_going_for_a_day_when_critical() {
    assert_eq!(max_attempts(false), 4);
    assert_eq!(retry_after(1, false), Some(std::time::Duration::from_secs(30)));
    assert_eq!(retry_after(3, false), Some(std::time::Duration::from_secs(600)));
    assert_eq!(retry_after(4, false), None);
    // 30 s + 2 min + 10 min, then every 30 min to 24 h: 47 more.
    assert_eq!(max_attempts(true), 51);
    assert_eq!(retry_after(4, true), Some(CRIT_RETRY_EVERY));
    assert_eq!(retry_after(51, true), None);
}

#[test]
fn the_log_keeps_the_last_500_and_a_retry_replaces_its_entry() {
    let dir = std::env::temp_dir().join(format!("sb-deliveries-{}-{}", std::process::id(), fastrand::u32(..)));
    let path = dir.join("deliveries.jsonl");
    let log = Log::open(Some(path.clone()));
    let ch = channel(Kind::Ntfy);
    let d = |i: u32, ok: bool| Delivery {
        id: format!("d{i}"),
        ts_ms: i as u64,
        error: (!ok).then(|| "HTTP 503 — the server had a problem; it will retry".into()),
        ..Delivery::row(Some(&ch), &format!("t{i}"), Severity::Warn, if ok { Outcome::Sent } else { Outcome::Failed })
    };
    let n = LOG_KEEP as u32 + 20;
    for i in 0..n {
        log.record(d(i, true));
    }
    assert_eq!(log.list(10_000).len(), LOG_KEEP);
    assert_eq!(log.list(1)[0].id, format!("d{}", n - 1));
    log.record(Delivery { attempts: 2, ..d(n - 1, false) });
    // A held event (no channel) never counts as a channel's last delivery.
    log.record(Delivery { id: "held".into(), ts_ms: n as u64, ..Delivery::row(None, "cell2 GPU 88 °C", Severity::Warn, Outcome::Held) });
    let again = Log::open(Some(path.clone()));
    let list = again.list(10_000);
    assert_eq!(list.len(), LOG_KEEP);
    assert_eq!((list[1].id.as_str(), list[1].attempts, list[1].ok, list[1].outcome), (format!("d{}", n - 1).as_str(), 2, false, Outcome::Failed));
    assert_eq!(list[0].outcome, Outcome::Held);
    assert_eq!(again.last_by_channel()["phone"].id, format!("d{}", n - 1));
    assert_eq!(again.last_failure(0).map(|d| d.id), Some(format!("d{}", n - 1)));
    // Once the channel delivers again (a fixed token, a test), the failure no longer counts.
    again.record(Delivery { id: "later".into(), ts_ms: n as u64 + 1, ..d(n + 1, true) });
    assert_eq!(again.last_failure(0), None);
    // A round-two row without an outcome reads as what it was.
    std::fs::write(&path, r#"{"id":"old","tsMs":5,"channel":"phone","channelName":"Phone","kind":"ntfy","title":"t","body":"","severity":"warn","ok":false,"error":"x","via":"mac"}
{"id":"demo","tsMs":6,"channel":"phone","channelName":"Phone","kind":"ntfy","title":"t","body":"","severity":"warn","ok":true,"error":null,"via":"mac","demo":true}
"#).unwrap();
    let old = Log::open(Some(path));
    assert_eq!(old.list(10).iter().map(|d| d.outcome).collect::<Vec<_>>(), [Outcome::Demo, Outcome::Failed]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Sends exactly what a channel's Test button sends from this Mac to the ntfy server at
/// $SB_NTFY_URL. Run by hand against a local ntfy-compatible server (ignored otherwise):
/// `SB_NTFY_URL=http://127.0.0.1:18089 cargo test --lib -- --ignored send_a_test_to_the_ntfy_server`.
#[tokio::test]
#[ignore]
async fn send_a_test_to_the_ntfy_server_in_the_environment() {
    let url = std::env::var("SB_NTFY_URL").expect("set SB_NTFY_URL");
    let ch = Channel { server: Some(url), topic: Some("sparkbox-alerts".into()), name: "Phone (ntfy)".into(), ..channel(Kind::Ntfy) };
    let msg = crate::notify::test_message(&ch, crate::history::now_ms());
    send(&ch, &none, &msg, &client(), &Endpoints::default()).await.unwrap();
}

/// Seen in Live on Oct 29: the Deliveries tab listed a Demo session's muted roce-loss resolves
/// among Live's. Each source lists its own; a Test (a real send) shows in both.
#[test]
fn live_and_demo_deliveries_are_listed_apart() {
    let log = Log::open(None);
    let ch = channel(Kind::Ntfy);
    let row = |id: &str, demo: bool, test: bool| Delivery { id: id.into(), demo, test, ..Delivery::row(Some(&ch), id, Severity::Warn, if demo { Outcome::Demo } else { Outcome::Sent }) };
    log.record_all(vec![row("live-page", false, false), row("demo-page", true, false), row("test", false, true)]);
    let ids = |v: Vec<Delivery>| v.into_iter().map(|d| d.id).collect::<Vec<_>>();
    assert_eq!(ids(log.list_in(10, false)), ["test", "live-page"]);
    assert_eq!(ids(log.list_in(10, true)), ["test", "demo-page"]);
    log.record(row("demo-later", true, false));
    assert_eq!(log.last_by_channel_in(false)["phone"].id, "test", "Live never shows a Demo row as a channel's last");
    assert_eq!(log.last_by_channel_in(true)["phone"].id, "demo-later");
}
