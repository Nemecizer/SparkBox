//! RDP checks and an explicitly reviewed xrdp repair; no password or firewall changes.
use super::Ops;
use crate::{
    admin,
    errors::SbError,
    is_live,
    live::{
        fence_class::{FenceClass, Shape},
        sh_quote,
    },
    AppState,
};
use serde::{Deserialize, Serialize};
use std::{sync::OnceLock, time::Duration};
use tauri::{AppHandle, State};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub label: String,
    pub ok: Option<bool>,
    pub detail: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub port: Option<u16>,
    pub user: String,
    pub profile: String,
    pub fingerprint: Option<String>,
    pub checks: Vec<Check>,
    pub changes: Vec<String>,
    pub blockers: Vec<String>,
    pub digest: String,
    pub backup: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub node: String,
    pub address: String,
    pub report: Report,
    pub mac: Check,
    pub demo: bool,
}

fn script() -> &'static str {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        include_str!("../rdp/rdp.py")
            .replace(
                "__XFCE_LAUNCHER__",
                &serde_json::to_string(include_str!("../rdp/xfce.sh")).unwrap(),
            )
            .replace(
                "__GNOME_LAUNCHER__",
                &serde_json::to_string(include_str!("../rdp/gnome.sh")).unwrap(),
            )
    })
}
fn valid(mode: &str, profile: &str, digest: &str, instance: &str) -> bool {
    matches!(mode, "check" | "repair")
        && matches!(profile, "keep" | "gnome" | "xfce")
        && matches!(instance, "dev" | "release")
        && ((mode == "check" && digest == "-")
            || (mode == "repair"
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))))
}
pub(crate) fn command(
    mode: &str,
    profile: &str,
    digest: &str,
    instance: &str,
) -> Result<String, String> {
    if !valid(mode, profile, digest, instance) {
        return Err("Invalid RDP request.".into());
    }
    Ok(format!(
        "/usr/bin/python3 -I -c {} {} {} {} {}",
        sh_quote(script()),
        sh_quote(mode),
        sh_quote(profile),
        sh_quote(digest),
        sh_quote(instance)
    ))
}
/// Recognize only the exact embedded helper and validated literal arguments. An appended
/// command, modified helper or arbitrary Python body stays `other`, even under a tag.
pub(crate) fn shape(cmd: &str) -> Option<Shape> {
    let prefix = format!("/usr/bin/python3 -I -c {} ", sh_quote(script()));
    let args: Vec<_> = cmd.strip_prefix(&prefix)?.split(' ').collect();
    let [mode, profile, digest, instance] = args.as_slice() else {
        return None;
    };
    let unquote = |s: &str| {
        s.strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .map(str::to_owned)
    };
    let (mode, profile, digest, instance) = (
        unquote(mode)?,
        unquote(profile)?,
        unquote(digest)?,
        unquote(instance)?,
    );
    (command(&mode, &profile, &digest, &instance).ok()?.as_str() == cmd).then(|| {
        Shape::Change(
            if mode == "check" {
                FenceClass::SudoProbe
            } else {
                FenceClass::RdpRepair
            },
            vec![],
        )
    })
}

fn demo_report(profile: &str) -> Report {
    Report {
        port: Some(3390),
        user: "sparkuser".into(),
        profile: if profile == "xfce" { "xfce" } else { "gnome" }.into(),
        fingerprint: Some("a1".repeat(32)),
        checks: vec![
            Check {
                label: "RDP negotiation on the node".into(),
                ok: Some(true),
                detail: "Simulated TLS 1.3; desktop login not tested".into(),
            },
            Check {
                label: "Desktop and NVIDIA software rendering".into(),
                ok: Some(false),
                detail: "Demo needs the software display settings".into(),
            },
        ],
        changes: vec![
            "Configure the xrdp software display".into(),
            "Restart xrdp and xrdp-sesman".into(),
        ],
        blockers: vec![],
        digest: "d".repeat(64),
        backup: None,
    }
}

impl Ops {
    async fn rdp_report(
        &self,
        node: &str,
        profile: &str,
        digest: Option<&str>,
    ) -> Result<Report, SbError> {
        let mode = if digest.is_some() { "repair" } else { "check" };
        let cmd = command(
            mode,
            profile,
            digest.unwrap_or("-"),
            crate::identity::instance_name(),
        )
        .map_err(SbError::invalid)?;
        let output = self
            .live
            .sudo(
                node,
                &cmd,
                Duration::from_secs(if digest.is_some() { 240 } else { 60 }),
            )
            .await
            .map_err(|e| SbError::classify(&e))?;
        if output.code != Some(0) {
            return Err(SbError::failed(if output.stderr.trim().is_empty() {
                "RDP inspection or repair did not complete; check again before retrying.".into()
            } else {
                output.stderr.trim().to_string()
            }));
        }
        serde_json::from_str(&output.stdout)
            .map_err(|_| SbError::failed("The node returned an incomplete RDP report."))
    }

    pub async fn rdp_check(
        &self,
        demo: bool,
        node: &str,
        host: &str,
        profile: &str,
    ) -> Result<View, SbError> {
        command("check", profile, "-", crate::identity::instance_name())
            .map_err(SbError::invalid)?;
        let report = if demo {
            let mut r = demo_report(profile);
            if self
                .rdp_demo
                .lock()
                .unwrap()
                .contains(&format!("{node}:{profile}"))
            {
                r.changes.clear();
                for c in &mut r.checks {
                    c.ok = Some(true);
                }
            }
            r
        } else {
            self.rdp_report(node, profile, None).await?
        };
        view(node, host, report, demo).await
    }

    pub async fn rdp_repair(
        &self,
        demo: bool,
        node: &str,
        host: &str,
        profile: &str,
        digest: &str,
    ) -> Result<View, SbError> {
        command("repair", profile, digest, crate::identity::instance_name())
            .map_err(SbError::invalid)?;
        if let Some(r) = self.system_status(demo).map_err(SbError::failed)? {
            if r.node == node && !matches!(r.phase.as_str(), "verified" | "refused") {
                return Err(SbError::invalid(
                    "Finish this node's protected system-update verification before RDP repair.",
                ));
            }
        }
        self.one("rdp", format!("Repair RDP on {}", self.name_of(node)), node, async {
            let report = if demo {
                if digest != "d".repeat(64) { return Err(SbError::invalid("Check RDP again before repairing.")); }
                self.simulated(node, "Repair RDP (Demo)", "Simulated xrdp repair");
                let mut r = demo_report(profile); r.changes.clear();
                for c in &mut r.checks { c.ok = Some(true); }
                r.backup = Some("/var/backups/sparkbox-rdp-demo".into());
                self.rdp_demo.lock().unwrap().insert(format!("{node}:{profile}")); r
            } else { self.rdp_report(node, profile, Some(digest)).await? };
            let v = view(node, host, report, demo).await?;
            Ok((v, "RDP configuration checked after repair. Confirm the desktop login in your RDP client.".into()))
        }).await
    }
}

async fn view(node: &str, host: &str, report: Report, demo: bool) -> Result<View, SbError> {
    let address = report
        .port
        .map(|p| {
            if host.contains(':') {
                format!("[{host}]:{p}")
            } else {
                format!("{host}:{p}")
            }
        })
        .unwrap_or_else(|| host.into());
    let mac = if demo {
        Check {
            label: "RDP from this Mac".into(),
            ok: Some(true),
            detail: "Simulated encrypted connection; no real network or login tested".into(),
        }
    } else if let (Some(port), Some(pin)) = (report.port, report.fingerprint.as_ref()) {
        let result = crate::rdp_probe::probe(host, port, pin).await;
        Check { label: "RDP from this Mac".into(), ok: Some(result.is_ok()), detail: result.unwrap_or_else(|e| format!("{e}. SparkBox's built-in VPN does not route an external RDP client; use a direct LAN or system VPN route.")) }
    } else {
        Check {
            label: "RDP from this Mac".into(),
            ok: None,
            detail: "A usable port and authenticated certificate are needed before testing.".into(),
        }
    };
    Ok(View {
        node: node.into(),
        address,
        report,
        mac,
        demo,
    })
}

fn host(state: &AppState, node: &str) -> Result<String, String> {
    state
        .inventory
        .lock()
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.id == node)
        .map(|n| {
            n.ip.as_ref()
                .filter(|ip| !ip.is_empty())
                .unwrap_or(&n.host)
                .clone()
        })
        .ok_or_else(|| "Unknown inventory node.".into())
}
#[tauri::command]
pub async fn rdp_check(
    app: AppHandle,
    state: State<'_, AppState>,
    node: String,
    profile: String,
) -> Result<View, String> {
    admin::require(&app)?;
    let host = host(&state, &node)?;
    state
        .ops
        .rdp_check(!is_live(&state), &node, &host, &profile)
        .await
        .map_err(|e| e.to_json())
}
#[tauri::command]
pub async fn rdp_repair(
    app: AppHandle,
    state: State<'_, AppState>,
    node: String,
    profile: String,
    digest: String,
) -> Result<View, String> {
    admin::require(&app)?;
    let host = host(&state, &node)?;
    state
        .ops
        .rdp_repair(!is_live(&state), &node, &host, &profile, &digest)
        .await
        .map_err(|e| e.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{
        exit::ExecKind,
        fence_class::{class_of, Ctx},
    };
    use std::sync::Arc;
    #[test]
    fn only_literal_helper_and_valid_args_get_rdp_privileges() {
        for mode in ["check", "repair"] {
            let digest = "f".repeat(64);
            let cmd = command(
                mode,
                "keep",
                if mode == "check" { "-" } else { &digest },
                "dev",
            )
            .unwrap();
            assert_eq!(
                class_of(ExecKind::Sudo, &cmd, &Ctx::default()),
                Shape::Change(
                    if mode == "check" {
                        FenceClass::SudoProbe
                    } else {
                        FenceClass::RdpRepair
                    },
                    vec![]
                )
            );
            assert_eq!(
                class_of(
                    ExecKind::Sudo,
                    &format!("{cmd}; chmod 777 /etc/shadow"),
                    &Ctx::default()
                ),
                Shape::Change(FenceClass::Other, vec![])
            );
            assert!(shape(&cmd.replace("systemctl", "malicious")).is_none());
            assert_eq!(
                class_of(ExecKind::Run, &cmd, &Ctx::default()),
                Shape::Change(FenceClass::Other, vec![])
            );
        }
        assert!(command("repair", "gnome;reboot", &"0".repeat(64), "release").is_err());
        assert!(command("repair", "keep", "old", "release").is_err());
    }
    #[tokio::test]
    async fn fake_ssh_checks_and_repair_use_sudo_and_keep_failures_visible() {
        use crate::fake_node::{connect, fake_node, temp};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        };
        let fail = Arc::new(AtomicBool::new(false));
        let failing = fail.clone();
        let report = serde_json::to_string(&demo_report("gnome")).unwrap();
        let (node, port) = fake_node(
            &temp("rdp-home"),
            Arc::new(move |cmd, _| {
                if cmd.contains("Bounded xrdp inspection") {
                    assert!(cmd.starts_with("sudo -k -S -p '' "));
                    return Some(if failing.load(Ordering::SeqCst) {
                        ("!stderr:Configuration changed; check again".into(), 1)
                    } else {
                        (report.clone(), 0)
                    });
                }
                None
            }),
        )
        .await;
        let (live, inv) = connect(&temp("rdp-app"), &[("cell1", port)]).await;
        let ops = Ops::new(
            live.clone(),
            Arc::new(Mutex::new(crate::demo::DemoCluster::new(&inv))),
            crate::tasks::Tasks::new(None),
        );
        assert_eq!(
            ops.rdp_report("cell1", "keep", None).await.unwrap().port,
            Some(3390)
        );
        assert!(ops
            .rdp_report("cell1", "keep", Some(&"d".repeat(64)))
            .await
            .is_ok());
        fail.store(true, Ordering::SeqCst);
        assert!(ops
            .rdp_report("cell1", "keep", Some(&"d".repeat(64)))
            .await
            .unwrap_err()
            .message
            .contains("changed"));
        assert_eq!(
            node.log
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.contains("Bounded xrdp inspection"))
                .count(),
            3
        );
        live.stop();
    }
}
