//! The Connect wizard's checks for one node: can SparkBox find it, reach it, trust it,
//! log in (and how: an SSH key from the agent or a file, or the password), use sudo, run
//! what the collector and Models need, and wake it remotely? Each step says what it found
//! and, when it fails, what to do about it.
//!
//! Nothing is saved, host keys included: trusting a key is a separate, explicit step
//! (`known_hosts_trust`). A node that isn't in the inventory yet is logged in to only once
//! its key has been trusted that way; for one that is, a key ~/.ssh/known_hosts vouches
//! for is accepted for the check without being pinned.
//!
//! The checks after the login run on the wizard's own connection but leave through Live's write
//! exit like every command, and go into the activity log: the reads as SparkBox's checks
//! (probes), the sudo check (`sudo true`, which changes nothing) as a `sudo-probe`. That one runs
//! only for a node in the inventory and only where the exit lets it: in the owner's session,
//! under, in the test build, a fence that allows sudo checks on the node. Look-only, no fence, or another class
//! leaves it unchecked, and the step says so.
//!
//! A connection that drops part-way (a node that flaps) fails the step it was on with that,
//! in plain words, never russh's "Channel send error" or a missing python3 for a command cut
//! off; the steps after it wait for the next run, and the drop goes in the SSH log.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::errors::{ErrorKind, SbError};
use crate::inventory::NodeConfig;
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::ssh::{AuthMethod, Trust};
use crate::live::{Live, SshError};
use crate::{admin, secrets, AppState};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckStep {
    pub id: String,
    pub label: String,
    /// ok | warn | fail | skip
    pub status: String,
    pub detail: String,
    pub hint: Option<String>,
    /// For an unknown host key: what to compare and trust.
    pub fingerprint: Option<String>,
    pub address: Option<String>,
    /// How the login went: agent | key | password (the auth and method steps).
    pub auth_method: Option<AuthMethod>,
    /// Skipped because an earlier step stopped the run (not a check that couldn't tell).
    pub blocked: bool,
}

fn step(id: &str, label: &str, status: &str, detail: impl Into<String>, hint: Option<&str>) -> CheckStep {
    CheckStep {
        id: id.into(),
        label: label.into(),
        status: status.into(),
        detail: detail.into(),
        hint: hint.map(Into::into),
        fingerprint: None,
        address: None,
        auth_method: None,
        blocked: false,
    }
}

const LABELS: [(&str, &str); 11] = [
    ("resolve", "Find the node"),
    ("tcp", "Reach SSH"),
    ("hostkey", "Host key"),
    ("auth", "Log in"),
    ("method", "Login method"),
    ("sudo", "sudo"),
    ("python", "Python 3"),
    ("docker", "Docker"),
    ("gpu", "GPU driver"),
    ("fabric", "Fabric addresses"),
    ("wol", "Wake-on-LAN"),
];

/// "SSH key (agent)", "SSH key", "Password".
pub fn method_label(m: AuthMethod) -> &'static str {
    match m {
        AuthMethod::Agent => "SSH key (agent)",
        AuthMethod::Key => "SSH key",
        AuthMethod::Password => "Password",
    }
}

/// The login-method step. The shared cluster password is a choice, not a problem: it reads
/// as plainly as a key does (key login stays one click away in Settings › Cluster login).
fn method_step(method: AuthMethod, detail: &str) -> CheckStep {
    let mut s = match method {
        AuthMethod::Password => step("method", "Login method", "ok", "Password (your shared cluster password, from the Keychain)", None),
        m => step("method", "Login method", "ok", format!("{} · {detail}", method_label(m)), None),
    };
    s.auth_method = Some(method);
    s
}

/// The LAN port (the default route's, else enP7s7), its MAC and its Wake-on setting, read-only.
/// Written without variable assignments, so the write exit reads every part of it as a read.
const WOL_PROBE: &str = "for i in $(ip -o route show default 2>/dev/null | awk '{print $5; exit}' | grep . || echo enP7s7); do \
echo \"$i\"; cat /sys/class/net/\"$i\"/address 2>/dev/null; ethtool \"$i\" 2>/dev/null | grep 'Wake-on:'; done";

/// ethtool shows Wake-on only to root on DGX OS, so NetworkManager's setting for the port's
/// connection follows ("NM-WOL: …"; nothing when NetworkManager doesn't manage the port). A
/// command of its own: if the exit refuses it, the step reads ethtool's lines alone.
fn nm_wol_probe(iface: &str) -> String {
    let i = crate::live::sh_quote(iface);
    format!(
        "nmcli -g GENERAL.CONNECTION device show {i} 2>/dev/null | grep -q . && \
         echo \"NM-WOL: $(nmcli -g 802-3-ethernet.wake-on-lan connection show \"$(nmcli -g GENERAL.CONNECTION device show {i} 2>/dev/null)\" 2>/dev/null)\"; true"
    )
}

/// The wizard's reads after the login, by step: what the collector, Containers and Models need.
const PYTHON_PROBE: &str = "python3 --version";
const DOCKER_PROBE: &str = "docker version --format '{{.Server.Version}}'";
/// Never a reinstall: this also fires for a stopped daemon, Docker is in the GPU stack, and `apt
/// install -y` skips apt's plan and every check SparkBox makes. The heading it names is in Help
/// (errors-connection).
const DOCKER_HINT: &str = "Don't reinstall it. Check the docker service in Services. See Help: Docker isn't available.";
const GPU_PROBE: &str = "nvidia-smi --query-gpu=name,driver_version --format=csv,noheader";
/// Never a repair: a driver package installed by hand can stop a Spark starting, and only someone
/// at the machine can bring it back. The heading it names is in Help (check-your-nodes).
const GPU_HINT: &str = "Don't load a model on this node. SparkBox can't repair a driver. See Help: If the GPU driver doesn't answer.";
const FABRIC_PROBE: &str = "ip -br -4 addr";

/// The sudo step from what the write exit and sudo said. A check the exit held back (look-only,
/// no fence that allows sudo checks here, another SparkBox's lease, a machine that isn't the
/// pinned one) is left unchecked, not failed.
fn sudo_step(name: &str, result: Result<crate::live::ssh::Output, String>) -> CheckStep {
    match result {
        Ok(o) if o.code == Some(0) => step("sudo", "sudo", "ok", "Works with the cluster password", None),
        Ok(o) => step("sudo", "sudo", "warn", o.stderr.trim().to_string(), Some("Services, power and updates need sudo for this user.")),
        Err(e) => {
            let e = SbError::classify(&e);
            match e.kind {
                ErrorKind::LookOnly => step("sudo", "sudo", "skip", "Not checked: SparkBox is in look-only mode", Some("The sudo check runs in your own session: look-only off and Admin unlocked.")),
                ErrorKind::Fenced => step("sudo", "sudo", "skip", format!("Not checked: no open fence allows sudo checks on {name}"), Some(&e.message)),
                ErrorKind::OtherController | ErrorKind::WrongNode => step("sudo", "sudo", "skip", format!("Not checked: {}", e.message), None),
                _ => step("sudo", "sudo", "fail", e.message, Some("Use the same password for SSH and sudo, or add the user to the sudo group.")),
            }
        }
    }
}

/// The Wake-on-LAN step from `WOL_PROBE`'s output. Wake-on-LAN isn't something SparkBox needs,
/// and what can't be read without sudo is said plainly: it never reads as a warning.
pub fn wol_step(out: &str) -> CheckStep {
    let mut lines = out.lines().map(str::trim);
    let iface = lines.next().filter(|l| !l.is_empty()).unwrap_or("enP7s7").to_string();
    let mac = lines.next().filter(|l| l.contains(':') && !l.contains(' ')).map(String::from);
    let mut supports = None;
    let mut setting = None;
    let mut nm = None;
    for l in out.lines().map(str::trim) {
        if let Some(v) = l.strip_prefix("Supports Wake-on:") {
            supports = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("Wake-on:") {
            setting = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("NM-WOL:") {
            nm = Some(v.trim().to_lowercase());
        }
    }
    let fix = format!(
        "Run sudo ethtool -s {iface} wol g on the node, and add `wakeonlan: true` under {iface} in its netplan file (/etc/netplan/*.yaml) so it stays on after a restart."
    );
    let on = |mac: &Option<String>| format!("Enabled on {iface}{} (magic packet)", mac.as_ref().map(|m| format!(", {m}")).unwrap_or_default());
    let nm_flags: Vec<&str> = nm.as_deref().map(|v| v.split([',', '|', ' ']).filter(|f| !f.is_empty()).collect()).unwrap_or_default();
    match (setting.as_deref(), supports.as_deref()) {
        (Some(w), _) if w.contains('g') => step("wol", "Wake-on-LAN", "ok", on(&mac), None),
        (Some(_), Some(s)) if !s.contains('g') => step("wol", "Wake-on-LAN", "skip", format!("{iface} can't wake on a magic packet"), None),
        (Some(w), _) => step("wol", "Wake-on-LAN", "skip", format!("Off on {iface} (Wake-on: {w})"), Some(&fix)),
        (None, _) if nm_flags.contains(&"magic") => step("wol", "Wake-on-LAN", "ok", "On (NetworkManager: magic packet)", None),
        (None, _) if nm_flags.iter().any(|f| matches!(*f, "none" | "disable" | "disabled" | "off")) => {
            step("wol", "Wake-on-LAN", "skip", format!("Off on {iface} (NetworkManager)"), Some(&fix))
        }
        (None, _) if nm.is_some() => step("wol", "Wake-on-LAN", "skip", "Not readable without sudo. NetworkManager leaves it to the driver default.", None),
        (None, _) => step("wol", "Wake-on-LAN", "skip", "Not readable without sudo (ethtool needs root)", None),
    }
}

fn skipped_from(id: &str, why: &str) -> Vec<CheckStep> {
    let from = LABELS.iter().position(|(i, _)| *i == id).unwrap_or(LABELS.len());
    LABELS[from..].iter().map(|(i, l)| CheckStep { blocked: true, ..step(i, l, "skip", why, None) }).collect()
}

/// russh's words for a connection that's gone ("Channel send error", "Disconnected", …), and
/// SparkBox's for a login the connection closed under (live/ssh.rs).
fn says_dropped(err: &str) -> bool {
    let e = err.to_lowercase();
    ["channel send error", "disconnected", "connection closed by the remote side", "channel not open", "unable to receive more messages", "could not reach the event loop", "closed during the login"]
        .iter()
        .any(|m| e.contains(m))
}

/// Whether the wizard's connection dropped under `result`: russh says so; the command came back
/// with no exit status (cut off with its channel); or the connection has ended and the command
/// failed anywhere but at the write exit (whose refusals are JSON).
fn dropped(ended: bool, result: &Result<crate::live::ssh::Output, String>) -> bool {
    match result {
        Ok(o) => o.code.is_none() && o.signal.is_none() && !o.timed_out && !o.cancelled,
        Err(e) => says_dropped(e) || (ended && SbError::from_json(e).is_none()),
    }
}

/// The step the connection dropped on, in plain words, naming the SSH log it's written to.
fn dropped_step(id: &str, name: &str) -> CheckStep {
    let label = LABELS.iter().find(|(i, _)| *i == id).map_or(id, |(_, l)| l);
    let log = crate::live::sshlog::path();
    let file = log.file_name().and_then(|f| f.to_str()).unwrap_or("ssh.log");
    step(
        id,
        label,
        "fail",
        format!("The connection to {name} dropped during this check. Run it again."),
        Some(&format!("When and why it dropped is in ~/Library/Logs/SparkBox/{file}.")),
    )
}

/// Ends the run at step `id`, whose connection dropped: that step says so, the ones after it wait
/// for the next run, and the drop goes in the SSH log (the wizard's connection is otherwise kept
/// out of it).
fn cut_off(live: &Live, cfg: &NodeConfig, ended: Option<String>, mut out: Vec<CheckStep>, id: &str) -> Vec<CheckStep> {
    let why = ended.unwrap_or_else(|| "a command came back without an exit status".into());
    live.sshlog.event(&cfg.id, "check_dropped", &[("step", &id), ("reason", &why)]);
    out.push(dropped_step(id, &cfg.name));
    let next = LABELS.iter().position(|(i, _)| *i == id).map_or(LABELS.len(), |p| p + 1);
    if let Some((next, _)) = LABELS.get(next) {
        out.extend(skipped_from(next, "Not checked: the connection dropped. Run the check again."));
    }
    out
}

/// Runs every check against `cfg`, stopping at the first one later checks depend on.
/// `known` says the node is in the inventory (see the module notes on host keys).
pub async fn probe_node(live: &Live, cfg: &NodeConfig, password: Option<String>, socks: Option<(String, String, String)>, known: bool) -> Vec<CheckStep> {
    let mut out = Vec::new();
    let addr = cfg.ip.clone().filter(|ip| !ip.trim().is_empty()).unwrap_or_else(|| cfg.host.clone());
    let address = format!("{addr}:{}", cfg.port);

    // 1. Resolve.
    let resolved = if cfg.ip.as_deref().is_some_and(|ip| !ip.trim().is_empty()) {
        Ok(format!("Uses its address, {addr}"))
    } else {
        match timeout(Duration::from_secs(4), tokio::net::lookup_host((addr.as_str(), cfg.port))).await {
            Ok(Ok(mut it)) => it.next().map(|a| format!("{} is {}", cfg.host, a.ip())).ok_or_else(|| format!("{} didn't resolve", cfg.host)),
            Ok(Err(e)) => Err(format!("{} didn't resolve: {e}", cfg.host)),
            Err(_) => Err(format!("{} didn't resolve within 4 s", cfg.host)),
        }
    };
    match resolved {
        Ok(d) => out.push(step("resolve", "Find the node", "ok", d, None)),
        Err(e) => {
            out.push(step("resolve", "Find the node", "fail", e, Some("Enter the node's IP address: .local names don't resolve over the VPN.")));
            out.extend(skipped_from("tcp", "Needs the node's address"));
            return out;
        }
    }

    // 2. TCP to SSH.
    let tcp = match &socks {
        Some((proxy, u, p)) => timeout(Duration::from_secs(5), tokio_socks::tcp::Socks5Stream::connect_with_password(proxy.as_str(), (addr.as_str(), cfg.port), u, p))
            .await
            .map(|r| r.map(drop).map_err(|e| e.to_string())),
        None => timeout(Duration::from_secs(5), TcpStream::connect((addr.as_str(), cfg.port))).await.map(|r| r.map(drop).map_err(|e| e.to_string())),
    };
    match tcp {
        Ok(Ok(())) => out.push(step("tcp", "Reach SSH", "ok", format!("{address} answers{}", if socks.is_some() { " through the VPN" } else { "" }), None)),
        Ok(Err(e)) => {
            out.push(step("tcp", "Reach SSH", "fail", format!("{address}: {e}"), Some("Check the node is on and SSH is running: sudo systemctl enable --now ssh")));
            out.extend(skipped_from("hostkey", "Needs SSH to answer"));
            return out;
        }
        Err(_) => {
            out.push(step(
                "tcp",
                "Reach SSH",
                "fail",
                format!("No reply from {address} in 5 s"),
                Some("Away from home? Import the VPN profile first. At home, check the cable and the IP."),
            ));
            out.extend(skipped_from("hostkey", "Needs SSH to answer"));
            return out;
        }
    }

    // 3 and 4. Host key, then the login: SSH keys first, the password last.
    let trust = if known { Trust::CheckOnly } else { Trust::PinnedOnly };
    let session = match live.connect_with(cfg, password.as_deref(), trust).await {
        Ok(s) => {
            out.push(step("hostkey", "Host key", "ok", "Matches the key SparkBox trusts", None));
            let mut auth = step("auth", "Log in", "ok", format!("Logged in as {}", cfg.user), None);
            auth.auth_method = Some(s.login.method);
            out.push(auth);
            out.push(method_step(s.login.method, &s.login.detail));
            s
        }
        Err(SshError::HostKeyUnknown { address, fingerprint }) => {
            let mut s = step(
                "hostkey",
                "Host key",
                "warn",
                format!("Not trusted yet (first connection). The node's key is {fingerprint}"),
                Some("Compare it on the node: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub. If it shows the same, choose Trust…"),
            );
            s.fingerprint = Some(fingerprint);
            s.address = Some(address);
            out.push(s);
            out.extend(skipped_from("auth", "After you trust the host key"));
            return out;
        }
        Err(e @ SshError::HostKeyChanged { .. }) => {
            let SshError::HostKeyChanged { ref address, ref fingerprint, .. } = e else { unreachable!() };
            let mut s = step("hostkey", "Host key", "fail", e.to_string(), Some("If you reinstalled the node, trust its new key; otherwise don't connect."));
            s.fingerprint = Some(fingerprint.clone());
            s.address = Some(address.clone());
            out.push(s);
            out.extend(skipped_from("auth", "Needs a trusted host key"));
            return out;
        }
        // Dropped during the handshake or the login: nothing about the key or the password yet.
        Err(SshError::Failed(e)) if says_dropped(&e) => return cut_off(live, cfg, Some(e), out, "hostkey"),
        Err(SshError::Failed(e)) if e.contains("host key") || e.contains("known_hosts") => {
            out.push(step("hostkey", "Host key", "fail", e, Some("If you reinstalled the node, use Settings › Diagnostics › Forget host keys.")));
            out.extend(skipped_from("auth", "Needs a trusted host key"));
            return out;
        }
        Err(SshError::Failed(e)) => {
            out.push(step("hostkey", "Host key", "ok", "Matches the key SparkBox trusts", None));
            out.push(step("auth", "Log in", "fail", e, Some("Check the user name here, and your SSH key or the cluster password in Settings › Cluster login.")));
            out.extend(skipped_from("method", "Needs to log in"));
            return out;
        }
    };

    // 5. sudo, prompt-safe (the password is still what sudo takes), as a sudo check the write
    // exit admits only under a fence that allows it on this node. A node that isn't in the
    // inventory yet isn't in any fence: its sudo is checked once it's saved.
    let session = Arc::new(session);
    match &password {
        None => out.push(step("sudo", "sudo", "warn", "No cluster password saved", Some("Services, power and updates need sudo: save the password in Settings › Cluster login."))),
        Some(_) if !known => out.push(step("sudo", "sudo", "skip", "Checked once the node is saved", Some("Sudo is checked on saved nodes only; test the node again from Settings › Nodes."))),
        Some(password) => {
            let result = as_class(FenceClass::SudoProbe, live.sudo_on(&cfg.id, &session, "true", password, Duration::from_secs(10))).await;
            if dropped(session.ended().is_some(), &result) {
                let ended = session.ended();
                session.close().await;
                return cut_off(live, cfg, ended, out, "sudo");
            }
            out.push(sudo_step(&cfg.name, result))
        }
    }

    // 6-9. What the collector, Containers and Models need: reads, through the write exit and in
    // the activity log as SparkBox's checks.
    let run = |cmd: String| {
        let (session, id) = (&session, &cfg.id);
        async move { live.check_on(id, session, &cmd, Duration::from_secs(15)).await }
    };
    // A read the connection dropped under ends the run there (`cut_off`).
    macro_rules! read {
        ($id:literal, $cmd:expr) => {{
            let result = run($cmd).await;
            if dropped(session.ended().is_some(), &result) {
                let ended = session.ended();
                session.close().await;
                return cut_off(live, cfg, ended, out, $id);
            }
            result
        }};
    }
    let python = read!("python", PYTHON_PROBE.into());
    out.push(match python {
        Ok(o) if o.code == Some(0) => step("python", "Python 3", "ok", format!("{}{}", o.stdout.trim(), o.stderr.trim()), None),
        _ => step("python", "Python 3", "fail", "python3 isn't installed", Some("Install it: sudo apt install -y python3")),
    });
    let docker = read!("docker", DOCKER_PROBE.into());
    out.push(match docker {
        Ok(o) if o.code == Some(0) => step("docker", "Docker", "ok", format!("Docker {}", o.stdout.trim()), None),
        Ok(o) if o.stderr.contains("permission denied") => {
            step("docker", "Docker", "warn", "Installed, but this user can't use it", Some(&format!("Run sudo usermod -aG docker {} and log in again.", cfg.user)))
        }
        _ => step("docker", "Docker", "warn", "Docker isn't available", Some(DOCKER_HINT)),
    });
    let gpu = read!("gpu", GPU_PROBE.into());
    out.push(match gpu {
        Ok(o) if o.code == Some(0) && !o.stdout.trim().is_empty() => {
            let line = o.stdout.lines().next().unwrap_or("").to_string();
            let (name, driver) = line.split_once(',').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())).unwrap_or((line.clone(), String::new()));
            step("gpu", "GPU driver", "ok", format!("{name}, driver {driver}"), None)
        }
        _ => step("gpu", "GPU driver", "fail", "nvidia-smi didn't answer", Some(GPU_HINT)),
    });
    let fabric = read!("fabric", FABRIC_PROBE.into());
    out.push(match fabric {
        Ok(o) if o.code == Some(0) => {
            let found: Vec<String> = o
                .stdout
                .lines()
                .filter_map(|l| {
                    let mut it = l.split_whitespace();
                    let iface = it.next()?;
                    let ip = it.find(|w| w.starts_with("192.168.100.") || w.starts_with("192.168.101."))?;
                    Some(format!("{iface} {ip}"))
                })
                .collect();
            let both = found.iter().any(|f| f.contains("192.168.100.")) && found.iter().any(|f| f.contains("192.168.101."));
            if both {
                step("fabric", "Fabric addresses", "ok", found.join(", "), None)
            } else {
                step(
                    "fabric",
                    "Fabric addresses",
                    "warn",
                    if found.is_empty() { "No address on 192.168.100.x or 192.168.101.x".to_string() } else { format!("Only {}", found.join(", ")) },
                    Some("Give enp1s0f1np1 an address in 192.168.100.0/24 and enP2p1s0f1np1 one in 192.168.101.0/24 (MTU 9000), as in NVIDIA's guide."),
                )
            }
        }
        _ => step("fabric", "Fabric addresses", "warn", "Couldn't list addresses", None),
    });
    // 10. Can it be woken from afar?
    let wol = match read!("wol", WOL_PROBE.into()) {
        Ok(o) => {
            let iface = o.stdout.lines().next().unwrap_or("enP7s7").trim().to_string();
            let nm = read!("wol", nm_wol_probe(&iface)).map(|n| n.stdout).unwrap_or_default();
            wol_step(&format!("{}{nm}", o.stdout))
        }
        Err(e) => step("wol", "Wake-on-LAN", "warn", SbError::classify(&e).message, None),
    };
    out.push(wol);
    session.close().await;
    out
}

/// A simulated checklist: the demo nodes pass, anything else isn't there.
fn demo_checks(cfg: &NodeConfig, app: &AppHandle) -> Vec<CheckStep> {
    let demo = app.state::<AppState>().demo.clone();
    let mut d = demo.lock().unwrap();
    let ip = cfg.ip.clone().unwrap_or_else(|| cfg.host.clone());
    let (code, out, _) = d.fleet_reply(&cfg.id, "hostname");
    if code != 0 {
        let mut v = vec![step("resolve", "Find the node", "ok", format!("Uses its address, {ip}"), None)];
        v.push(step("tcp", "Reach SSH", "fail", format!("No reply from {ip}:{} in 5 s", cfg.port), Some("This node isn't part of the simulated cluster.")));
        v.extend(skipped_from("hostkey", "Needs SSH to answer"));
        return v;
    }
    let (_, gpu, _) = d.fleet_reply(&cfg.id, "nvidia-smi --query-gpu=name,driver_version --format=csv,noheader");
    let (_, addrs, _) = d.fleet_reply(&cfg.id, "ip -br -4 addr");
    let fabric: Vec<String> = addrs
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let iface = it.next()?;
            let ip = it.find(|w| w.starts_with("192.168.10"))?;
            Some(format!("{iface} {ip}"))
        })
        .collect();
    // Every Spark logs in with the shared cluster password, as the real ones do.
    let method = AuthMethod::Password;
    let mut auth = step("auth", "Log in", "ok", format!("Logged in as {}", cfg.user), None);
    auth.auth_method = Some(method);
    let lan = d.snapshot().nodes.into_iter().find(|n| n.id == cfg.id).and_then(|n| n.nets.into_iter().find(|i| i.role == crate::model::NetRole::Lan));
    let (lan_iface, mac) = lan.map(|i| (i.iface, i.mac.unwrap_or_default())).unwrap_or_else(|| ("enP7s7".into(), String::new()));
    vec![
        step("resolve", "Find the node", "ok", format!("Uses its address, {ip}"), None),
        step("tcp", "Reach SSH", "ok", format!("{ip}:{} answers", cfg.port), None),
        step("hostkey", "Host key", "ok", format!("Matches the key SparkBox trusts ({} is simulated)", out.trim()), None),
        auth,
        method_step(method, "~/.ssh/id_ed25519 via agent"),
        step("sudo", "sudo", "ok", "Works with the cluster password", None),
        step("python", "Python 3", "ok", "Python 3.12.3", None),
        step("docker", "Docker", "ok", format!("Docker {}", crate::demo::world::DOCKER), None),
        step("gpu", "GPU driver", "ok", gpu.split(',').take(2).map(str::trim).collect::<Vec<_>>().join(", driver "), None),
        step("fabric", "Fabric addresses", "ok", fabric.join(", "), None),
        // As on the real Sparks: ethtool needs root, NetworkManager leaves it to the driver.
        wol_step(&format!("{lan_iface}\n{mac}\nNM-WOL: default\n")),
    ]
}

/// Tauri command: checks one node (from the wizard or Settings › Nodes) without saving
/// anything. It logs in with the Mac's keys and the saved password, so it's Mac scope.
#[tauri::command]
pub async fn node_probe(app: AppHandle, node: NodeConfig) -> Result<Vec<CheckStep>, String> {
    admin::require_mac(&app)?;
    let state = app.state::<AppState>();
    if !crate::is_live(&state) {
        tokio::time::sleep(Duration::from_millis(350)).await;
        return Ok(demo_checks(&node, &app));
    }
    let live: Arc<Live> = state.live.clone();
    let password = secrets::get("ssh")?;
    let socks = state.vpn.socks().await;
    let known = state.inventory.lock().unwrap().nodes.iter().any(|n| n.id == node.id && n.host == node.host && n.ip == node.ip && n.port == node.port);
    Ok(probe_node(&live, &node, password, socks, known).await)
}

/// Pins `fingerprint` for `address` (the node `node`: its id and name, when it's in the
/// inventory). When the address already had another key pinned, the node's name must be typed
/// (`typed_name`): the shared cluster password, which is also the sudo password, goes to whatever
/// machine holds the new key. A replaced key is logged old → new and the node's identity pin is
/// cleared, so it's pinned again at its next connection. A scripted unlock (`scripted`:
/// SPARKBOX_ADMIN_NOAUTH) never replaces a key: that takes a real Touch ID.
pub fn trust_host_key_checked(live: &Live, known_hosts: &Path, address: &str, fingerprint: &str, node: Option<(&str, &str)>, typed_name: Option<&str>, scripted: bool) -> Result<(), String> {
    let address = address.trim();
    let old = crate::live::ssh::load_known(known_hosts)?.get(address).cloned().filter(|old| old != fingerprint.trim());
    let name = node.map(|(_, name)| name).unwrap_or(address);
    if old.is_some() {
        crate::live::exit::trust_needs_touch_id(scripted).map_err(|e| e.to_json())?;
    }
    if old.is_some() && typed_name.map(str::trim) != Some(name) {
        return Err(format!("{name}'s host key changed. Type {name} to trust the new key."));
    }
    crate::live::trust_host_key(known_hosts, address, fingerprint)?;
    match old {
        Some(old) => {
            live.audit.record_app(&format!("Host key for {name} replaced {old} → {}", fingerprint.trim()), address);
            if let Some((id, _)) = node {
                live.forget_identity(id)?;
            }
        }
        None => {
            live.audit.record_app(&format!("Trusted the SSH host key of {address}"), fingerprint);
        }
    }
    Ok(())
}

/// Tauri command: pins a node's SSH host key once the user has compared the fingerprint. A
/// changed key also needs the node's name typed (`typed_name`).
#[tauri::command]
pub fn known_hosts_trust(app: AppHandle, address: String, fingerprint: String, typed_name: Option<String>) -> Result<(), String> {
    admin::require_mac(&app)?;
    let state = app.state::<AppState>();
    let node = state.inventory.lock().unwrap().nodes.iter().find(|n| format!("{}:{}", n.ip.clone().filter(|ip| !ip.trim().is_empty()).unwrap_or_else(|| n.host.clone()), n.port) == address.trim()).map(|n| (n.id.clone(), n.name.clone()));
    trust_host_key_checked(&state.live, &state.known_hosts_path, &address, &fingerprint, node.as_ref().map(|(id, name)| (id.as_str(), name.as_str())), typed_name.as_deref(), crate::live::exit::scripted_unlock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{fake_node, host_fingerprint, temp, Script};
    use crate::vpn::Vpn;

    fn gb10() -> Script {
        Arc::new(|cmd: &str, _: &[u8]| match cmd {
            c if c.starts_with("python3 --version") => Some(("Python 3.12.3\n".into(), 0)),
            c if c.starts_with("docker version") => Some(("28.3.2\n".into(), 0)),
            c if c.starts_with("nvidia-smi") => Some(("NVIDIA GB10, 580.95.05\n".into(), 0)),
            c if c.starts_with("ip -br -4 addr") => Some(("lo UNKNOWN 127.0.0.1/8\nenp1s0f1np1 UP 192.168.100.2/24\nenP2p1s0f1np1 UP 192.168.101.2/24\n".into(), 0)),
            _ => None,
        })
    }

    fn cfg(port: u16) -> NodeConfig {
        NodeConfig { id: "cell2".into(), name: "cell2".into(), host: "cell2.local".into(), ip: Some("127.0.0.1".into()), model: "MSI EdgeXpert".into(), port, user: "sparkuser".into(), fabric: None }
    }

    fn live(name: &str) -> (Arc<Live>, std::path::PathBuf) {
        let dir = temp(name);
        let known = dir.join("known_hosts.json");
        (Live::new(Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}), known.clone(), || Ok(None)), known)
    }

    fn status(steps: &[CheckStep], id: &str) -> String {
        steps.iter().find(|s| s.id == id).unwrap().status.clone()
    }

    #[tokio::test]
    async fn a_gpu_driver_that_doesnt_answer_points_to_help_and_never_to_a_reinstall() {
        let healthy = gb10();
        let broken: Script = Arc::new(move |cmd: &str, stdin: &[u8]| if cmd.starts_with("nvidia-smi") { Some((String::new(), 127)) } else { healthy(cmd, stdin) });
        let (_node, port) = fake_node(&temp("onboard-gpu-home"), broken).await;
        let (live, known) = live("onboard-gpu-app");
        let first = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        let key = first.iter().find(|s| s.id == "hostkey").unwrap();
        crate::live::trust_host_key(&known, key.address.as_ref().unwrap(), key.fingerprint.as_ref().unwrap()).unwrap();
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        let gpu = steps.iter().find(|s| s.id == "gpu").unwrap();
        assert_eq!((gpu.status.as_str(), gpu.detail.as_str()), ("fail", "nvidia-smi didn't answer"));
        assert_eq!(gpu.hint.as_deref(), Some("Don't load a model on this node. SparkBox can't repair a driver. See Help: If the GPU driver doesn't answer."));
    }

    #[tokio::test]
    async fn a_stopped_docker_points_to_services_and_never_to_a_reinstall() {
        let healthy = gb10();
        let broken: Script = Arc::new(move |cmd: &str, stdin: &[u8]| if cmd.starts_with("docker version") { Some(("Client: 28.3.2\nCannot connect to the Docker daemon\n".into(), 1)) } else { healthy(cmd, stdin) });
        let (_node, port) = fake_node(&temp("onboard-docker-home"), broken).await;
        let (live, known) = live("onboard-docker-app");
        let first = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        let key = first.iter().find(|s| s.id == "hostkey").unwrap();
        crate::live::trust_host_key(&known, key.address.as_ref().unwrap(), key.fingerprint.as_ref().unwrap()).unwrap();
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        let docker = steps.iter().find(|s| s.id == "docker").unwrap();
        assert_eq!((docker.status.as_str(), docker.detail.as_str()), ("warn", "Docker isn't available"));
        let hint = docker.hint.as_deref().unwrap();
        assert_eq!(hint, "Don't reinstall it. Check the docker service in Services. See Help: Docker isn't available.");
        assert!(!hint.contains("apt"), "{hint}");
    }

    #[tokio::test]
    async fn a_new_node_needs_its_key_trusted_then_passes_every_check() {
        let (node, port) = fake_node(&temp("onboard-home"), gb10()).await;
        let (live, known) = live("onboard-app");
        let first = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        let key = first.iter().find(|s| s.id == "hostkey").unwrap();
        assert_eq!(key.status, "warn");
        assert_eq!(key.fingerprint.as_deref(), Some(host_fingerprint().as_str()));
        assert_eq!(key.address.as_deref(), Some(format!("127.0.0.1:{port}").as_str()));
        assert_eq!(status(&first, "auth"), "skip");
        // "Not finished: trust the host key to run the other 8 checks."
        assert_eq!(first.iter().filter(|s| s.blocked).count(), 8, "{first:?}");

        crate::live::trust_host_key(&known, key.address.as_ref().unwrap(), key.fingerprint.as_ref().unwrap()).unwrap();
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        for id in ["resolve", "tcp", "hostkey", "auth", "python", "docker", "gpu", "fabric"] {
            assert_eq!(status(&steps, id), "ok", "{id}: {:?}", steps.iter().find(|s| s.id == id));
        }
        // No fence names a node that isn't saved yet: its sudo is checked once it is.
        let sudo = steps.iter().find(|s| s.id == "sudo").unwrap();
        assert_eq!((sudo.status.as_str(), sudo.detail.as_str(), sudo.blocked), ("skip", "Checked once the node is saved", false));
        assert!(node.ran("sudo").is_empty(), "nothing ran with sudo");
        assert!(steps.iter().find(|s| s.id == "fabric").unwrap().detail.contains("192.168.101.2"));
        // The shared password is the owner's choice: it reads as plainly as a key.
        let method = steps.iter().find(|s| s.id == "method").unwrap();
        assert_eq!((method.status.as_str(), method.auth_method, method.hint.as_deref()), ("ok", Some(AuthMethod::Password), None));
        assert_eq!(method.detail, "Password (your shared cluster password, from the Keychain)");
        // The fake's ethtool reports Wake-on d: off. Not a warning (nothing SparkBox needs), but the fix is there.
        let wol = steps.iter().find(|s| s.id == "wol").unwrap();
        assert_eq!((wol.status.as_str(), wol.blocked), ("skip", false), "{wol:?}");
        assert!(wol.hint.as_deref().unwrap().contains("sudo ethtool -s"), "{wol:?}");
    }

    #[tokio::test]
    async fn probing_never_pins_a_host_key() {
        let (_node, port) = fake_node(&temp("onboard-pin-home"), gb10()).await;
        let (live, known) = live("onboard-pin-app");
        // An unknown key stops the check at the host key, and nothing is saved.
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        assert_eq!(status(&steps, "hostkey"), "warn");
        assert!(!known.exists() || !std::fs::read_to_string(&known).unwrap().contains(&format!("127.0.0.1:{port}")), "nothing pinned");
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, false).await;
        assert_eq!(status(&steps, "auth"), "skip", "a new node isn't logged in to before its key is trusted");
        assert!(!known.exists() || !std::fs::read_to_string(&known).unwrap().contains(&format!("127.0.0.1:{port}")));
    }

    #[tokio::test]
    async fn a_key_login_and_wake_on_lan_pass_the_checks() {
        let (node, port) = fake_node(&temp("onboard-key-home"), gb10()).await;
        let dir = temp("onboard-key-app");
        let key = dir.join("id_ed25519");
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status().unwrap().success());
        node.authorize_key(russh::keys::PublicKey::from_openssh(std::fs::read_to_string(key.with_extension("pub")).unwrap().trim()).unwrap());
        std::fs::write(node.state_dir.join("wol"), "g").unwrap();
        let (live, known) = live("onboard-key-live");
        crate::fake_node::trust_ports(&known, &[port]);
        live.set_login_settings(&crate::live::sshconfig::LoginSettings { identity_file: Some(key.display().to_string()), ..Default::default() }).unwrap();
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        let method = steps.iter().find(|s| s.id == "method").unwrap();
        assert_eq!((method.status.as_str(), method.auth_method), ("ok", Some(AuthMethod::Key)), "{method:?}");
        assert!(method.detail.starts_with("SSH key · "), "{}", method.detail);
        let wol = steps.iter().find(|s| s.id == "wol").unwrap();
        assert_eq!((wol.status.as_str(), wol.label.as_str()), ("ok", "Wake-on-LAN"));
        assert!(wol.detail.starts_with("Enabled on "), "{}", wol.detail);
    }

    #[test]
    fn the_wake_on_lan_step_reads_ethtool() {
        let on = wol_step("enP7s7\n4c:bb:47:0a:1b:2c\n\tSupports Wake-on: pumbg\n\tWake-on: g\n");
        assert_eq!((on.status.as_str(), on.detail.as_str()), ("ok", "Enabled on enP7s7, 4c:bb:47:0a:1b:2c (magic packet)"));
        let off = wol_step("enP7s7\n4c:bb:47:0a:1b:2c\n\tSupports Wake-on: pumbg\n\tWake-on: d\n");
        assert_eq!(off.status, "skip");
        assert!(off.hint.unwrap().contains("sudo ethtool -s enP7s7 wol g"));
        let unread = wol_step("eth0\n");
        assert_eq!((unread.status.as_str(), unread.detail.as_str(), unread.hint), ("skip", "Not readable without sudo (ethtool needs root)", None));
    }

    /// Runs the Test against cell1 as the tour found it, with NetworkManager's Wake-on-LAN set as `nm`.
    async fn spark_wol(home: &str, set: &[(&str, &str)]) -> Vec<CheckStep> {
        let (node, port) = crate::fake_node::fake_spark(&temp(home), "cell1").await;
        for (file, value) in set {
            std::fs::write(node.state_dir.join(file), value).unwrap();
        }
        let (live, known) = live(&format!("{home}-app"));
        crate::live::trust_host_key(&known, &format!("127.0.0.1:{port}"), &crate::fake_node::Profile::spark("cell1").fingerprint()).unwrap();
        probe_node(&live, &cfg(port), Some("pw".into()), None, true).await
    }

    #[tokio::test]
    async fn a_real_spark_passes_and_reads_wake_on_lan_without_root() {
        let steps = spark_wol("onboard-spark-default", &[]).await;
        for id in ["resolve", "tcp", "hostkey", "auth", "method", "sudo", "python", "docker", "gpu", "fabric"] {
            assert_eq!(status(&steps, id), "ok", "{id}: {:?}", steps.iter().find(|s| s.id == id));
        }
        let detail = |steps: &[CheckStep], id: &str| steps.iter().find(|s| s.id == id).unwrap().detail.clone();
        assert_eq!(detail(&steps, "python"), "Python 3.12.3");
        assert_eq!(detail(&steps, "gpu"), "NVIDIA GB10, driver 580.178.04");
        assert_eq!(detail(&steps, "fabric"), "enp1s0f1np1 192.168.100.1/24, enP2p1s0f1np1 192.168.101.1/24");
        // ethtool refuses a normal user; NetworkManager leaves Wake-on to the driver: unknown, not a problem.
        let wol = steps.iter().find(|s| s.id == "wol").unwrap();
        assert_eq!((wol.status.as_str(), wol.detail.as_str(), wol.hint.as_deref()), ("skip", "Not readable without sudo. NetworkManager leaves it to the driver default.", None));
        let on = spark_wol("onboard-spark-magic", &[("nm-wol", "magic")]).await;
        assert_eq!((status(&on, "wol"), detail(&on, "wol")), ("ok".to_string(), "On (NetworkManager: magic packet)".to_string()));
        let none = spark_wol("onboard-spark-nonm", &[("nmcli", "missing")]).await;
        assert_eq!((status(&none, "wol"), detail(&none, "wol")), ("skip".to_string(), "Not readable without sudo (ethtool needs root)".to_string()));
        let off = spark_wol("onboard-spark-off", &[("nm-wol", "none")]).await;
        let off = off.iter().find(|s| s.id == "wol").unwrap();
        assert_eq!((off.status.as_str(), off.detail.as_str()), ("skip", "Off on enP7s7 (NetworkManager)"));
        assert!(off.hint.as_deref().unwrap().contains("sudo ethtool -s enP7s7 wol g"), "the fix only for an explicit off");
    }

    #[test]
    fn a_changed_host_key_needs_the_nodes_name_typed_and_is_logged_old_to_new() {
        let (live, known) = live("onboard-retrust");
        let old = crate::fake_node::host_fingerprint();
        let new = crate::fake_node::Profile::spark("gx10-0000").fingerprint();
        let cell2 = Some(("cell2", "cell2"));
        // First trust: nothing to type.
        trust_host_key_checked(&live, &known, "192.168.50.154:22", &old, cell2, None, false).unwrap();
        // Its pin, as the exit made it at the first connection.
        let pins = live.exit_config_dir().join(crate::live::identity_pin::FILE);
        std::fs::write(&pins, r#"{"cell2":{"hostname":"cell2-example","lanMac":"fc:9d:05:9b:e9:14","clonedMachineId":true,"pinnedMs":1}}"#).unwrap();
        // A different key at the same address: refused without the name, or with the wrong one.
        for typed in [None, Some("cell3"), Some("")] {
            let e = trust_host_key_checked(&live, &known, "192.168.50.154:22", &new, cell2, typed, false).unwrap_err();
            assert_eq!(e, "cell2's host key changed. Type cell2 to trust the new key.");
        }
        assert_eq!(crate::live::ssh::load_known(&known).unwrap()["192.168.50.154:22"], old, "nothing changed");
        assert!(std::fs::read_to_string(&pins).unwrap().contains("cell2-example"));
        // A scripted unlock (SPARKBOX_ADMIN_NOAUTH) can't replace it, name typed or not.
        let e = crate::errors::SbError::from(trust_host_key_checked(&live, &known, "192.168.50.154:22", &new, cell2, Some("cell2"), true).unwrap_err());
        assert_eq!(e.message, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.");
        assert_eq!(crate::live::ssh::load_known(&known).unwrap()["192.168.50.154:22"], old, "nothing changed");
        // Typed: trusted, logged old → new, and the identity pin is gone (pinned again on connect).
        trust_host_key_checked(&live, &known, "192.168.50.154:22", &new, cell2, Some("cell2"), false).unwrap();
        assert_eq!(crate::live::ssh::load_known(&known).unwrap()["192.168.50.154:22"], new);
        let row = live.audit.entries().into_iter().find(|e| e.title.starts_with("Host key for cell2 replaced")).unwrap();
        assert_eq!(row.title, format!("Host key for cell2 replaced {old} → {new}"));
        assert_eq!((row.node.as_str(), row.class.as_str()), ("mac", "app"));
        assert!(!std::fs::read_to_string(&pins).unwrap().contains("cell2"));
        // The same key again needs nothing typed, and a first trust needs no Touch ID of its own.
        trust_host_key_checked(&live, &known, "192.168.50.154:22", &new, cell2, None, true).unwrap();
        trust_host_key_checked(&live, &known, "192.168.50.98:22", &new, Some(("cell3", "cell3")), None, true).unwrap();
    }

    /// cell2 in the inventory and connected, as Settings › Nodes › Test checks it.
    async fn connected(name: &str) -> (crate::fake_node::Node, u16, Arc<Live>, std::path::PathBuf) {
        let (node, port) = fake_node(&temp(&format!("{name}-home")), gb10()).await;
        let base = temp(&format!("{name}-app"));
        let (live, _) = crate::fake_node::connect(&base, &[("cell2", port)]).await;
        (node, port, live, base)
    }

    fn rows(live: &Live) -> Vec<crate::live::audit::Entry> {
        live.audit.entries().into_iter().filter(|e| e.node == "cell2").collect()
    }

    /// The wizard's checks go to the activity log: its reads as SparkBox's checks, its sudo `true`
    /// as one sudo line the fence let through as a sudo check (P1.19).
    #[tokio::test]
    async fn the_wizards_checks_are_logged_and_its_sudo_is_a_fenced_sudo_check() {
        let (node, port, live, base) = connected("onboard-logged").await;
        crate::live::tests::open_fence(&live, &base, &["cell2"], &[FenceClass::SudoProbe]);
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        for id in ["auth", "sudo", "python", "docker", "gpu", "fabric"] {
            assert_eq!(status(&steps, id), "ok", "{id}: {:?}", steps.iter().find(|s| s.id == id));
        }
        let probes: Vec<String> = rows(&live).into_iter().filter(|e| e.class == "probe").map(|e| e.command).collect();
        for cmd in [PYTHON_PROBE, DOCKER_PROBE, GPU_PROBE, FABRIC_PROBE, WOL_PROBE] {
            assert!(probes.iter().any(|p| p == cmd), "{cmd} logged as a probe: {probes:?}");
        }
        assert!(probes.iter().all(|p| !p.contains("sudo")), "{probes:?}");
        let sudo: Vec<crate::live::audit::Entry> = rows(&live).into_iter().filter(|e| e.kind == "sudo").collect();
        assert_eq!(sudo.len(), 1, "{sudo:?}");
        assert_eq!((sudo[0].command.as_str(), sudo[0].fence.as_deref(), sudo[0].state.as_str(), sudo[0].escalated), ("true", Some("sudo-probe"), "ok", true));
        assert_eq!(node.ran("sudo -k -S -p '' true").len(), 1);
        // The password went only to sudo's own prompt, on stdin.
        let (_, stdin) = node.stdin.lock().unwrap().iter().find(|(c, _)| c == "sudo -k -S -p '' true").cloned().unwrap();
        assert_eq!(stdin, b"pw\n");
        live.stop();
    }

    /// In look-only, and without a fence that allows sudo checks, the sudo check is held back by
    /// the write exit and the step says so; the reads still run.
    #[tokio::test]
    async fn look_only_or_no_fence_leaves_sudo_unchecked_and_says_why() {
        let (node, port, live, base) = connected("onboard-held").await;
        live.set_write_policy(crate::live::tests::TestPolicy::look_only());
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        let sudo = steps.iter().find(|s| s.id == "sudo").unwrap();
        assert_eq!((sudo.status.as_str(), sudo.detail.as_str()), ("skip", "Not checked: SparkBox is in look-only mode"));
        for id in ["python", "docker", "gpu", "fabric"] {
            assert_eq!(status(&steps, id), "ok", "{id}: reads pass in look-only");
        }
        assert!(node.ran("sudo").is_empty(), "sudo never reached the node");
        let refused: Vec<_> = rows(&live).into_iter().filter(|e| e.title == "Refused").collect();
        assert_eq!(refused.iter().filter(|e| e.command == "true").count(), 1, "{refused:?}");
        // Nothing else: NetworkManager's read is a read too.
        assert_eq!(refused.len(), 1, "{refused:?}");

        // Look-only off, the exit enforced, but no fence open: refused as fenced.
        live.set_write_policy(Arc::new(crate::live::tests::TestPolicy(false.into())));
        crate::live::tests::open_fence(&live, &base, &["cell2"], &[FenceClass::Service]);
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        let sudo = steps.iter().find(|s| s.id == "sudo").unwrap();
        assert_eq!((sudo.status.as_str(), sudo.detail.as_str()), ("skip", "Not checked: no open fence allows sudo checks on cell2"));
        assert_eq!(sudo.hint.as_deref(), Some("The fence doesn't allow sudo checks."));
        assert!(node.ran("sudo").is_empty());
        live.stop();
    }

    /// Every read the wizard sends the write exit is a read, so look-only and an enforced exit let
    /// them through, NetworkManager's (nmcli) included.
    #[test]
    fn the_wizards_reads_are_reads_to_the_write_exit() {
        use crate::live::exit::ExecKind;
        use crate::live::fence_class::{class_of, Ctx, Shape};
        for cmd in [PYTHON_PROBE, DOCKER_PROBE, GPU_PROBE, FABRIC_PROBE, WOL_PROBE] {
            assert_eq!(class_of(ExecKind::Run, cmd, &Ctx::default()), Shape::Read, "`{cmd}`");
        }
        assert!(nm_wol_probe("enP7s7").contains("nmcli -g GENERAL.CONNECTION device show 'enP7s7'"));
        assert_eq!(class_of(ExecKind::Run, &nm_wol_probe("enP7s7"), &Ctx::default()), Shape::Read);
    }

    /// A connection that drops part-way through the checks (here while Docker's version is
    /// read) fails that step in plain words, naming the SSH log the drop goes in; the steps after
    /// it wait for the next run, and nothing says "Channel send error" or blames a missing tool.
    #[tokio::test]
    async fn a_connection_that_drops_mid_check_says_so_in_plain_words() {
        let answers = gb10();
        // Docker's version comes from the node's own shell, which takes its time.
        let script: Script = Arc::new(move |cmd: &str, stdin: &[u8]| if cmd.starts_with("docker version") { None } else { answers(cmd, stdin) });
        let (node, port) = fake_node(&temp("onboard-drop-home"), script).await;
        node.bin("docker", "sleep 5; echo 28.3.2");
        let base = temp("onboard-drop-app");
        let (live, _) = crate::fake_node::connect(&base, &[("cell2", port)]).await;
        let log = base.join("ssh-dev.log");
        live.sshlog.set_path(log.clone());
        let n = node.clone();
        let cut = async move {
            for _ in 0..400 {
                if !n.ran("docker version").is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            n.drop_connections().await;
        };
        let node_cfg = cfg(port);
        let (steps, ()) = tokio::join!(probe_node(&live, &node_cfg, Some("pw".into()), None, true), cut);
        for id in ["auth", "sudo", "python"] {
            assert_eq!(status(&steps, id), "ok", "{id}: {:?}", steps.iter().find(|s| s.id == id));
        }
        let docker = steps.iter().find(|s| s.id == "docker").unwrap();
        assert_eq!((docker.status.as_str(), docker.detail.as_str()), ("fail", "The connection to cell2 dropped during this check. Run it again."));
        let file = crate::live::sshlog::path().file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(docker.hint.as_deref(), Some(format!("When and why it dropped is in ~/Library/Logs/SparkBox/{file}.").as_str()));
        for id in ["gpu", "fabric", "wol"] {
            let s = steps.iter().find(|s| s.id == id).unwrap();
            assert_eq!((s.status.as_str(), s.blocked), ("skip", true), "{id}: {s:?}");
        }
        assert!(steps.iter().all(|s| !s.detail.contains("Channel send error") && !s.detail.contains("isn't available")), "{steps:?}");
        assert!(std::fs::read_to_string(&log).unwrap().contains(" cell2 check_dropped step=docker reason="), "the drop is in the SSH log");
        live.stop();
    }

    /// Every step after the login says a drop the same way: russh's words for a connection that's
    /// gone, a command cut off without an exit status, or a failure once the connection ended.
    /// A refusal at the write exit, or an ordinary failure, is never a drop.
    #[test]
    fn a_dropped_connection_reads_the_same_on_every_step() {
        use crate::live::ssh::Output;
        let refused = SbError::new(ErrorKind::LookOnly, "SparkBox is in look-only mode.").to_json();
        let slow = "`python3 --version` took longer than 15s".to_string();
        let cases: [(Result<Output, String>, bool, bool); 9] = [
            (Err("Channel send error".into()), false, true),
            (Err("Disconnected".into()), false, true),
            (Err("Connection closed by the remote side".into()), false, true),
            (Ok(Output::default()), false, true),
            (Err(slow.clone()), true, true),
            (Err(slow), false, false),
            (Err(refused), true, false),
            (Ok(Output { code: Some(1), stderr: "permission denied".into(), ..Default::default() }), true, false),
            (Ok(Output { signal: Some("KILL".into()), ..Default::default() }), false, false),
        ];
        for (result, ended, want) in &cases {
            assert_eq!(dropped(*ended, result), *want, "{result:?} ended={ended}");
        }
        for (id, label) in &LABELS[2..] {
            let s = dropped_step(id, "cell2");
            assert_eq!((s.label.as_str(), s.status.as_str(), s.detail.as_str()), (*label, "fail", "The connection to cell2 dropped during this check. Run it again."), "{id}");
            assert!(s.hint.as_deref().unwrap().contains("Library/Logs/SparkBox/ssh"), "{id}: {s:?}");
        }
    }

    #[tokio::test]
    async fn a_bad_password_and_an_unreachable_node_say_what_to_do() {
        let (_node, port) = fake_node(&temp("onboard-pw-home"), gb10()).await;
        let (live, known) = live("onboard-pw-app");
        crate::fake_node::trust_ports(&known, &[port]);
        let steps = probe_node(&live, &cfg(port), Some("wrong".into()), None, true).await;
        assert_eq!(status(&steps, "auth"), "fail");
        assert!(steps.iter().find(|s| s.id == "auth").unwrap().hint.as_deref().unwrap().contains("Cluster login"));
        assert_eq!(status(&steps, "sudo"), "skip");

        // A port that's taken but not listening (macOS drops the SYN: no reply in 5 s), so no test
        // running alongside can start listening on it meanwhile.
        let held = tokio::net::TcpSocket::new_v4().unwrap();
        held.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let closed = held.local_addr().unwrap().port();
        let steps = probe_node(&live, &cfg(closed), Some("pw".into()), None, true).await;
        assert_eq!(status(&steps, "resolve"), "ok");
        assert_eq!(status(&steps, "tcp"), "fail");
        assert!(steps.iter().find(|s| s.id == "tcp").unwrap().hint.is_some());
        assert!(steps.iter().skip(2).all(|s| s.status == "skip"));

        let steps = probe_node(&live, &cfg(port), None, None, true).await;
        assert_eq!(status(&steps, "auth"), "fail");
    }

    /// The steps from `from` on wait for the next run, and nothing reads as russh's words or a
    /// missing tool.
    fn cut_off_from(steps: &[CheckStep], from: &str) {
        let at = LABELS.iter().position(|(i, _)| *i == from).unwrap();
        for (id, _) in &LABELS[at..] {
            let s = steps.iter().find(|s| s.id == *id).unwrap_or_else(|| panic!("{id}: {steps:?}"));
            assert_eq!((s.status.as_str(), s.blocked), ("skip", true), "{id}: {s:?}");
            assert_eq!(s.detail, "Not checked: the connection dropped. Run the check again.", "{id}");
        }
        assert!(steps.iter().all(|s| !s.detail.contains("Channel send error") && !s.detail.contains("Disconnected") && !s.detail.contains("isn't available")), "{steps:?}");
    }

    /// A connection that drops while sudo checks the password (`sudo -k -S -p '' true`, still
    /// running on the node) fails the sudo step in plain words; Python and the rest wait.
    #[tokio::test]
    async fn a_connection_that_drops_during_the_sudo_check_says_so_in_plain_words() {
        let (node, port, live, base) = connected("onboard-drop-sudo").await;
        let log = base.join("ssh-dev.log");
        live.sshlog.set_path(log.clone());
        // sudo takes the password, then takes its time to run `true` (the shim runs it as
        // `env LC_ALL=C sh -c 'true'`, under SB_FAKE_ROOT).
        node.bin("env", "[ \"$SB_FAKE_ROOT\" = 1 ] && sleep 5\nexec /usr/bin/env \"$@\"");
        let n = node.clone();
        let cut = async move {
            for _ in 0..400 {
                if !n.ran("sudo -k -S -p '' true").is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            n.drop_connections().await;
        };
        let node_cfg = cfg(port);
        let (steps, ()) = tokio::join!(probe_node(&live, &node_cfg, Some("pw".into()), None, true), cut);
        for id in ["hostkey", "auth"] {
            assert_eq!(status(&steps, id), "ok", "{id}: {:?}", steps.iter().find(|s| s.id == id));
        }
        let sudo = steps.iter().find(|s| s.id == "sudo").unwrap();
        assert_eq!((sudo.label.as_str(), sudo.status.as_str(), sudo.detail.as_str()), ("sudo", "fail", "The connection to cell2 dropped during this check. Run it again."));
        cut_off_from(&steps, "python");
        assert!(node.ran("python3 --version").is_empty(), "nothing after sudo was sent");
        assert!(std::fs::read_to_string(&log).unwrap().contains(" cell2 check_dropped step=sudo reason="), "the drop is in the SSH log");
        live.stop();
    }

    /// A connection that drops while the node is being logged in to (the key matched, the login
    /// went unanswered) says so at the connection's step, not as a rejected login.
    #[tokio::test]
    async fn a_connection_that_drops_during_the_login_says_so_in_plain_words() {
        let (node, port) = fake_node(&temp("onboard-drop-login-home"), gb10()).await;
        let (live, known) = live("onboard-drop-login-app");
        crate::fake_node::trust_ports(&known, &[port]);
        node.drop_next_login();
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        assert_eq!(status(&steps, "tcp"), "ok");
        let key = steps.iter().find(|s| s.id == "hostkey").unwrap();
        assert_eq!((key.status.as_str(), key.detail.as_str()), ("fail", "The connection to cell2 dropped during this check. Run it again."), "{steps:?}");
        cut_off_from(&steps, "auth");
        assert!(node.knobs.logins.lock().unwrap().is_empty(), "no login went through");
        // The next run logs in.
        let steps = probe_node(&live, &cfg(port), Some("pw".into()), None, true).await;
        assert_eq!(status(&steps, "auth"), "ok", "{steps:?}");
    }
}
