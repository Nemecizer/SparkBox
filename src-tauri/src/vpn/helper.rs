//! Runs the bundled `sparkbox-tunnel` helper (see /tunnel) and speaks its
//! line-delimited JSON protocol. The helper exits when its stdin closes, and
//! the child is killed if this handle is dropped, so it never outlives SparkBox.

use std::path::PathBuf;
use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum HelperEvent {
    Ready {
        socks: String,
    },
    #[serde(rename_all = "camelCase")]
    Stats {
        handshake_age_sec: i64,
        rx_bytes: u64,
        tx_bytes: u64,
        #[serde(default)]
        udp_sent: u64,
        #[serde(default)]
        udp_received: u64,
        #[serde(default)]
        udp_send_errors: u64,
    },
    Probe {
        id: String,
        ok: bool,
        ms: f32,
        #[serde(default)]
        error: Option<String>,
    },
    Forward {
        id: String,
        ok: bool,
        listen: String,
        #[serde(default)]
        error: Option<String>,
    },
    Error {
        message: String,
    },
    Log {
        message: String,
    },
    /// Not sent by the helper: emitted here when its output ends.
    #[serde(skip)]
    Exited,
}

pub struct Helper {
    commands: mpsc::UnboundedSender<String>,
    _child: Child,
}

/// The helper sits next to SparkBox's own executable (Tauri's externalBin puts
/// it in Contents/MacOS, and copies it into target/<profile> for dev builds).
pub fn helper_path() -> PathBuf {
    std::env::current_exe().map(|exe| exe.with_file_name("sparkbox-tunnel")).unwrap_or_default()
}

impl Helper {
    pub async fn spawn(
        path: &std::path::Path,
        config: &Value,
        events: mpsc::UnboundedSender<HelperEvent>,
    ) -> Result<Helper, String> {
        if !path.exists() {
            return Err(format!("tunnel helper missing at {}; run `npm run build:tunnel`", path.display()));
        }
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("couldn't start the tunnel helper: {e}"))?;

        let mut stdin = child.stdin.take().ok_or("helper stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("helper stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("helper stderr unavailable")?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tx.send(config.to_string()).map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
            // Dropping stdin tells the helper to shut down.
        });

        // Whatever the helper prints on stderr goes to the connection log, line by line.
        let log = events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    let _ = log.send(HelperEvent::Log { message: line });
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<HelperEvent>(&line) {
                    Ok(ev) => {
                        let _ = events.send(ev);
                    }
                    Err(_) => { let _ = events.send(HelperEvent::Log { message: "The tunnel helper sent an unreadable event.".into() }); },
                }
            }
            let _ = events.send(HelperEvent::Exited);
        });

        Ok(Helper { commands: tx, _child: child })
    }

    pub fn send(&self, cmd: Value) {
        let _ = self.commands.send(cmd.to_string());
    }
}

#[cfg(test)]
pub(super) mod tests {
    //! Two real helper processes: one plays the Firewalla, the other connects to it.
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    // Throwaway keys used only by this test.
    pub const SERVER: (&str, &str) =
        ("CYvKnzUoAAz+AJ+Hh3t4grqjtPdO8ys9ho3sW4wRboc=", "ytirFS10hAhWMTlnOkKcpKWfCGB/UKxA8eqHOgfTkyg=");
    pub const CLIENT: (&str, &str) =
        ("9BIiJFKyL9vHTAvgb9IB2e+POIF+H8W9JHPxwAAVG+U=", "hqaVJEYEiY3eNg4REZ2hgZeU5v5/D1osGvrLGg9cj3Y=");

    pub fn built_helper() -> std::path::PathBuf {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/sparkbox-tunnel-aarch64-apple-darwin");
        assert!(p.exists(), "run `npm run build:tunnel` first");
        p
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<HelperEvent>, want: impl Fn(&HelperEvent) -> bool) -> HelperEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ev = rx.recv().await.expect("helper exited");
                if want(&ev) {
                    return ev;
                }
            }
        })
        .await
        .expect("timed out waiting for helper event")
    }

    #[tokio::test]
    async fn rust_drives_the_helper_through_a_real_handshake() {
        let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let obf = json!({ "jc": "3", "jmin": "30", "jmax": "60", "s1": "12", "s2": "17",
                          "h1": "111111", "h2": "222222", "h3": "333333", "h4": "444444" });

        let (stx, mut srx) = mpsc::unbounded_channel();
        let _server = Helper::spawn(&built_helper(), &json!({
            "privateKey": SERVER.0, "addresses": ["10.98.0.1/24"], "peerPublicKey": CLIENT.1,
            "allowedIps": ["10.98.0.2/32"], "obfuscation": obf, "listenPort": port,
        }), stx).await.unwrap();
        next(&mut srx, |e| matches!(e, HelperEvent::Ready { .. })).await;

        let (ctx, mut crx) = mpsc::unbounded_channel();
        let client = Helper::spawn(&built_helper(), &json!({
            "privateKey": CLIENT.0, "addresses": ["10.98.0.2/32"], "peerPublicKey": SERVER.1,
            "endpoint": format!("127.0.0.1:{port}"), "allowedIps": ["10.98.0.0/24"], "keepalive": 25,
            "obfuscation": obf, "socksUser": "u", "socksPass": "p",
        }), ctx).await.unwrap();

        match next(&mut crx, |e| matches!(e, HelperEvent::Ready { .. })).await {
            HelperEvent::Ready { socks } => assert!(socks.starts_with("127.0.0.1:")),
            _ => unreachable!(),
        }

        // Nothing listens at 10.98.0.1:22, so the probe is refused — but only after
        // packets crossed the tunnel, which forces the handshake.
        client.send(json!({ "cmd": "probe", "id": "1", "target": "10.98.0.1:22" }));
        next(&mut crx, |e| matches!(e, HelperEvent::Probe { .. })).await;
        next(&mut crx, |e| matches!(e, HelperEvent::Stats { handshake_age_sec, .. } if *handshake_age_sec >= 0)).await;

        client.send(json!({ "cmd": "forward", "id": "fw", "listen": "127.0.0.1:0", "target": "10.98.0.1:80" }));
        match next(&mut crx, |e| matches!(e, HelperEvent::Forward { .. })).await {
            HelperEvent::Forward { id, ok, listen, .. } => {
                assert_eq!(id, "fw");
                assert!(ok);
                assert!(listen.starts_with("127.0.0.1:"));
            }
            _ => unreachable!(),
        }
    }
}
