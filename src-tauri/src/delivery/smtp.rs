//! Email over SMTP with lettre: STARTTLS (usually port 587) or implicit TLS
//! ("SSL", usually 465), checked against the Mac's trusted certificates. The
//! password comes from the Keychain. A server on this Mac's loopback may be
//! plain SMTP (the tests' server); nothing else is ever sent unencrypted.

use std::time::Duration;

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Email, Tokio1Executor};

use super::{Channel, Failure, Message};

pub fn loopback(host: &str) -> bool {
    matches!(host.trim(), "127.0.0.1" | "::1" | "localhost")
}

/// "[SparkBox] gx10-0000 GPU Xid 79".
pub fn subject(msg: &Message) -> String {
    match msg.state.as_str() {
        "test" => format!("[SparkBox] {}", msg.title),
        "resolved" => format!("[SparkBox] Resolved: {}", msg.title.trim_start_matches("Resolved: ")),
        _ => format!("[SparkBox] {}", msg.title),
    }
}

pub fn build(ch: &Channel, msg: &Message) -> Result<Email, Failure> {
    let bad = |what: &str, v: &str| Failure::new(format!("\"{v}\" isn't a usable {what} address"), false);
    let from = ch.from.as_deref().unwrap_or("").trim();
    let mut b = Email::builder().from(from.parse::<Mailbox>().map_err(|_| bad("From", from))?).subject(subject(msg));
    for to in ch.to.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()) {
        b = b.to(to.parse::<Mailbox>().map_err(|_| bad("To", to))?);
    }
    let text = format!("{}\n\n{}\n\n— SparkBox", msg.title, msg.body);
    b.header(ContentType::TEXT_PLAIN).body(text).map_err(|e| Failure::new(e.to_string(), false))
}

fn failure(host: &str, e: lettre::transport::smtp::Error) -> Failure {
    if let Some(code) = e.status() {
        let code = code.to_string();
        let hint = match code.as_str() {
            "535" | "534" | "530" => "check the user and password",
            "550" | "553" | "554" => "the server refused the address",
            c if c.starts_with('4') => "the server is busy; it will retry",
            _ => "the server refused it",
        };
        return Failure::new(format!("SMTP {code} — {hint}"), code.starts_with('4'));
    }
    if e.is_tls() {
        return Failure::new(format!("TLS with {host} failed — check STARTTLS or SSL and the port"), false);
    }
    if e.is_timeout() {
        return Failure::new(format!("Couldn't reach {host}: no answer in 20 s"), true);
    }
    Failure::new(format!("Couldn't reach {host}: {e}"), true)
}

pub async fn send(ch: &Channel, password: Option<String>, msg: &Message) -> Result<(), Failure> {
    let host = ch.host.as_deref().unwrap_or("").trim().to_string();
    let email = build(ch, msg)?;
    let security = ch.security.as_deref().unwrap_or("starttls");
    let mut t = match security {
        "ssl" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host).map_err(|e| failure(&host, e))?.port(ch.port.unwrap_or(465)),
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host).map_err(|e| failure(&host, e))?.port(ch.port.unwrap_or(587)),
        _ if loopback(&host) => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host).port(ch.port.unwrap_or(25)).tls(Tls::None),
        _ => return Err(Failure::new("SparkBox won't send a password unencrypted: choose STARTTLS or SSL", false)),
    };
    if security == "starttls" && loopback(&host) {
        // A local relay with a self-made certificate still gets the name checked against "localhost".
        if let Ok(p) = TlsParameters::new(host.clone()) {
            t = t.tls(Tls::Required(p));
        }
    }
    if let Some(user) = ch.user.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        t = t.credentials(Credentials::new(user.to_string(), password.unwrap_or_default()));
    }
    let mailer = t.timeout(Some(Duration::from_secs(20))).build();
    mailer.send(email).await.map(|_| ()).map_err(|e| failure(&host, e))
}
