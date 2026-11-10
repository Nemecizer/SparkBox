//! Explicit local-LAN survey. Anonymous TCP/HTTP hints never establish a Spark identity.
//! Owner-selected verification uses pinned SSH keys and the ordinary read/write exit.
use crate::{
    admin,
    inventory::{Inventory, NodeConfig},
    live::{identity_pin::Pin, SshError},
    wol::MacIface,
    AppState,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv4Addr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    time::timeout,
};

const TTL_MS: u64 = 10 * 60_000;
const PORTS: [u16; 3] = [22, 80, 443];
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn read_command() -> &'static str {
    static COMMAND: OnceLock<String> = OnceLock::new();
    COMMAND.get_or_init(|| {
        format!(
            "python3 -I -c {}",
            crate::live::sh_quote(include_str!("read.py"))
        )
    })
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub id: String,
    pub interface: String,
    pub address: String,
    pub network: String,
    pub eligible: bool,
    pub reason: String,
}
fn scopes(ifaces: &[MacIface]) -> Vec<Scope> {
    ifaces.iter().map(|i| {
        let mask = u32::from(i.mask); let prefix = mask.leading_ones();
        let valid_mask = prefix > 0 && mask == u32::MAX.checked_shl(32-prefix).unwrap_or(0);
        let eligible = !i.point_to_point && i.name.starts_with("en") && i.addr.is_private() && valid_mask && prefix <= 24;
        let [a,b,c,_] = i.addr.octets();
        Scope { id: format!("{}:{}/{}",i.name,i.addr,i.mask), interface:i.name.clone(),address:i.addr.to_string(),network:format!("{a}.{b}.{c}.0/24"),eligible,
            reason: if eligible { "Local private LAN; only this /24 will be surveyed" } else { "Needs a local Ethernet or Wi-Fi private IPv4 network with a /24 or larger netmask; tunnels and virtual bridges are excluded" }.into() }
    }).collect()
}
fn addresses(scope: &Scope, ifaces: &[MacIface]) -> Result<Vec<Ipv4Addr>, String> {
    if !scope.eligible || !scopes(ifaces).contains(scope) {
        return Err(
            "The selected local interface changed. Refresh networks and survey again.".into(),
        );
    }
    let ip: Ipv4Addr = scope.address.parse().map_err(|_| "Invalid local address")?;
    let [a, b, c, _] = ip.octets();
    Ok((1..=254)
        .map(|d| Ipv4Addr::new(a, b, c, d))
        .filter(|ip| !ifaces.iter().any(|i| i.addr == *ip))
        .collect())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interface {
    pub name: String,
    pub mac: String,
    pub addresses: Vec<String>,
    pub mtu: Option<u32>,
    pub speed_mbps: Option<u32>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbor {
    pub iface: String,
    pub system: String,
    pub chassis: String,
    pub port: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Facts {
    pub hostname: String,
    pub machine_id: String,
    pub model: String,
    pub gpu: String,
    pub interfaces: Vec<Interface>,
    pub neighbors: Vec<Neighbor>,
}
impl Facts {
    fn lan_mac(&self, address: &str) -> Option<String> {
        self.interfaces
            .iter()
            .find(|i| {
                i.addresses
                    .iter()
                    .any(|a| a.split('/').next() == Some(address))
            })
            .and_then(|i| {
                crate::wol::parse_mac(&i.mac).ok().map(|m| {
                    m.iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(":")
                })
            })
    }
    fn spark(&self) -> bool {
        self.gpu
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "GB10")
    }
    fn valid(&self, address: &str) -> bool {
        !self.hostname.is_empty()
            && self.hostname.len() <= 160
            && self
                .hostname
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
            && self.machine_id.len() == 32
            && self.machine_id.chars().all(|c| c.is_ascii_hexdigit())
            && self.model.len() <= 512
            && self.gpu.len() <= 160
            && self.interfaces.len() <= 64
            && self.neighbors.len() <= 64
            && self.lan_mac(address).is_some()
            && self.interfaces.iter().all(|i| {
                i.name.len() <= 64
                    && i.addresses.len() <= 8
                    && i.addresses.iter().all(|a| cidr(a).is_some())
            })
            && self.neighbors.iter().all(|n| {
                n.iface.len() <= 64
                    && [&n.system, &n.chassis, &n.port]
                        .iter()
                        .all(|s| s.len() <= 160)
            })
    }
}
fn cidr(value: &str) -> Option<String> {
    let (ip, prefix) = value.split_once('/')?;
    let ip = ip.parse::<Ipv4Addr>().ok()?;
    let prefix = prefix.parse::<u32>().ok()?;
    if !(1..=32).contains(&prefix) || ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() {
        return None;
    }
    Some(format!(
        "{}/{}",
        Ipv4Addr::from(u32::from(ip) & (u32::MAX << (32 - prefix))),
        prefix
    ))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub address: String,
    pub ports: Vec<u16>,
    pub hint: String,
    pub status: String,
    pub message: String,
    pub fingerprint: Option<String>,
    pub changed_key: bool,
    pub facts: Option<Facts>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub demo: bool,
    pub scope: Scope,
    pub started_ms: u64,
    pub status: String,
    pub checked: usize,
    pub total: usize,
    pub message: String,
    pub verifying: Option<String>,
    pub candidates: Vec<Candidate>,
}
#[derive(Default)]
struct Store {
    run: Option<Run>,
    last_live: Option<Instant>,
}
#[derive(Default)]
pub struct Discovery {
    store: Mutex<Store>,
}
impl Discovery {
    fn view(&self, demo: bool) -> Option<Run> {
        let mut s = self.store.lock().unwrap();
        if s.run.as_ref().is_some_and(|r| r.demo != demo) {
            // A late read from an old source must not erase a newer source's run.
            return None;
        }
        if let Some(r) = &mut s.run {
            if now().saturating_sub(r.started_ms) >= TTL_MS {
                r.status = "expired".into();
                r.message = "Survey expired. Survey again before proposing changes.".into();
            }
        }
        s.run.clone()
    }
    fn current(&self, id: &str, demo: bool) -> bool {
        self.store.lock().unwrap().run.as_ref().is_some_and(|r| {
            r.id == id
                && r.demo == demo
                && now().saturating_sub(r.started_ms) < TTL_MS
                && matches!(r.status.as_str(), "running" | "complete")
        })
    }
    fn edit(&self, id: &str, f: impl FnOnce(&mut Run)) {
        if let Some(r) = &mut self.store.lock().unwrap().run {
            if r.id == id {
                f(r);
            }
        }
    }
    fn begin(&self, scope: Scope, demo: bool, total: usize) -> Result<Run, String> {
        let mut s = self.store.lock().unwrap();
        if s.run
            .as_ref()
            .is_some_and(|r| r.demo == demo && (r.status == "running" || r.verifying.is_some()))
        {
            return Err("Stop the current survey or verification first.".into());
        }
        if !demo
            && s.last_live
                .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return Err("Wait 30 seconds between LAN surveys.".into());
        }
        if !demo {
            s.last_live = Some(Instant::now());
        }
        let r = Run {
            id: format!("{:016x}", fastrand::u64(..)),
            demo,
            scope,
            started_ms: now(),
            status: "running".into(),
            checked: 0,
            total,
            message: String::new(),
            verifying: None,
            candidates: Vec::new(),
        };
        s.run = Some(r.clone());
        Ok(r)
    }
}
/// Every production socket is bound to the selected address AND macOS interface, so a VPN
/// route or SSH HostName/ProxyJump cannot redirect discovery. Tests inject loopback streams.
async fn connect(scope: &Scope, address: Ipv4Addr, port: u16) -> Result<TcpStream, String> {
    let socket = TcpSocket::new_v4().map_err(|e| e.to_string())?;
    socket
        .bind(
            (
                scope
                    .address
                    .parse::<Ipv4Addr>()
                    .map_err(|_| "Invalid local address")?,
                0,
            )
                .into(),
        )
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let name =
            std::ffi::CString::new(scope.interface.as_str()).map_err(|_| "Invalid interface")?;
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0
            || unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::IPPROTO_IP,
                    libc::IP_BOUND_IF,
                    &index as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&index) as libc::socklen_t,
                )
            } != 0
        {
            return Err("Could not bind discovery to the selected interface".into());
        }
    }
    timeout(
        Duration::from_millis(700),
        socket.connect((address, port).into()),
    )
    .await
    .map_err(|_| "No reply".to_string())?
    .map_err(|e| e.to_string())
}
async fn http_hint(mut stream: TcpStream, address: Ipv4Addr) -> String {
    let read = async {
        stream
            .write_all(
                format!("GET / HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .ok()?;
        let mut bytes = Vec::new();
        stream.take(8192).read_to_end(&mut bytes).await.ok()?;
        let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
        Some(if text.contains("routeros") && (text.contains("webfig")||text.contains("mikrotik")) {"RouterOS page reported; device identity unverified"} else {"Web service; equipment unidentified"}.to_string())
    };
    timeout(Duration::from_millis(700), read)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Web service; equipment unidentified".into())
}
async fn survey_host(scope: &Scope, address: Ipv4Addr) -> Option<Candidate> {
    let mut ports = Vec::new();
    let mut hint = String::new();
    for port in PORTS {
        if let Ok(s) = connect(scope, address, port).await {
            ports.push(port);
            if port == 80 {
                hint = http_hint(s, address).await;
            }
        }
    }
    if ports.is_empty() {
        return None;
    }
    if hint.is_empty() {
        hint = if ports.contains(&22) {
            "SSH answers; hardware unidentified"
        } else {
            "HTTPS answers; hardware unidentified"
        }
        .into();
    }
    Some(Candidate {
        address: address.to_string(),
        ports,
        hint,
        status: "unidentified".into(),
        message:
            "Select Verify to identify an SSH candidate; no credentials were sent by the survey."
                .into(),
        fingerprint: None,
        changed_key: false,
        facts: None,
    })
}
async fn stopped(app: &AppHandle, service: &Discovery, id: &str, demo: bool, scope: &Scope) {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let state = app.state::<AppState>();
        if !service.current(id, demo)
            || crate::is_live(&state) == demo
            || admin::look_only()
            || (!demo && addresses(scope, &crate::wol::mac_interfaces()).is_err())
        {
            break;
        }
    }
}
fn demo_scope() -> Scope {
    Scope {
        id: "demo-lan".into(),
        interface: "Demo Ethernet".into(),
        address: "192.168.50.10".into(),
        network: "192.168.50.0/24".into(),
        eligible: true,
        reason: "Fixture only; no network traffic".into(),
    }
}
fn demo_candidates() -> Vec<Candidate> {
    serde_json::from_str(include_str!("fixture.json")).expect("discovery fixture")
}

#[tauri::command]
pub fn discovery_networks(app: AppHandle) -> Vec<Scope> {
    if crate::is_live(&app.state::<AppState>()) {
        scopes(&crate::wol::mac_interfaces())
    } else {
        vec![demo_scope()]
    }
}
#[tauri::command]
pub fn discovery_status(app: AppHandle) -> Option<Run> {
    app.state::<Discovery>()
        .view(!crate::is_live(&app.state::<AppState>()))
}
#[tauri::command]
pub fn discovery_cancel(app: AppHandle, id: String) {
    app.state::<Discovery>().edit(&id, |r| {
        r.status = "cancelled".into();
        r.message = "Stopped. Partial results cannot be used for an inventory proposal.".into();
    });
}
#[tauri::command]
pub fn discovery_start(
    app: AppHandle,
    scope_id: String,
    expected_demo: bool,
) -> Result<Run, String> {
    admin::require_mac(&app)?;
    if admin::look_only() {
        return Err("LAN discovery is unavailable in look-only mode.".into());
    }
    let demo = !crate::is_live(&app.state::<AppState>());
    if demo != expected_demo {
        return Err("The data source changed. Reopen discovery before surveying.".into());
    }
    let scope = discovery_networks(app.clone())
        .into_iter()
        .find(|s| s.id == scope_id && s.eligible)
        .ok_or("Choose an eligible local network.")?;
    let targets = if demo {
        Vec::new()
    } else {
        addresses(&scope, &crate::wol::mac_interfaces())?
    };
    let run = app.state::<Discovery>().begin(
        scope.clone(),
        demo,
        if demo { 253 } else { targets.len() },
    )?;
    let id = run.id.clone();
    app.state::<AppState>().live.audit.record_app(
        "LAN discovery started",
        &format!(
            "{} via {}{}",
            scope.network,
            scope.interface,
            if demo { " (Demo)" } else { "" }
        ),
    );
    tauri::async_runtime::spawn(async move {
        let service = app.state::<Discovery>();
        let work = async {
            if demo {
                tokio::time::sleep(Duration::from_millis(250)).await;
                service.edit(&id, |r| {
                    r.candidates = demo_candidates();
                    r.checked = r.total;
                });
            } else {
                // Eight hosts at a time, one batch per second at most; at most eight sockets.
                for batch in targets.chunks(8) {
                    let started = Instant::now();
                    let results =
                        stream::iter(batch.iter().copied().map(|a| survey_host(&scope, a)))
                            .buffer_unordered(8)
                            .collect::<Vec<_>>()
                            .await;
                    service.edit(&id, |r| {
                        r.checked += batch.len();
                        r.candidates.extend(results.into_iter().flatten());
                        r.candidates
                            .sort_by_key(|c| c.address.parse::<Ipv4Addr>().ok());
                    });
                    if started.elapsed() < Duration::from_secs(1) {
                        tokio::time::sleep(Duration::from_secs(1) - started.elapsed()).await;
                    }
                }
            }
        };
        let result = tokio::select! {_=stopped(&app,&service,&id,demo,&scope)=>"cancelled",r=timeout(Duration::from_secs(120),work)=>if r.is_ok(){"complete"}else{"timedOut"}};
        let result = if !service.current(&id, demo)
            || crate::is_live(&app.state::<AppState>()) == demo
            || admin::look_only()
            || (!demo && addresses(&scope, &crate::wol::mac_interfaces()).is_err())
        {
            "cancelled"
        } else {
            result
        };
        service.edit(&id, |r| {
            r.status = result.into();
            r.message = match result {
                "complete" => {
                    "Survey complete. Verify selected SSH candidates to identify GB10 Sparks."
                }
                "timedOut" => {
                    "Survey reached its two-minute limit. Partial results cannot be proposed."
                }
                _ => "Stopped: source, interface, permission or survey changed.",
            }
            .into();
        });
    });
    Ok(run)
}

async fn identify(
    live: &crate::live::Live,
    cfg: &NodeConfig,
    stream: TcpStream,
) -> Result<(Facts, Option<String>), SshError> {
    let session = Arc::new(live.connect_for_discovery(cfg, stream).await?);
    let result = live
        .check_on(&cfg.id, &session, read_command(), Duration::from_secs(15))
        .await;
    let fp = session.host_key.clone();
    session.close().await;
    let out = result.map_err(SshError::Failed)?;
    if out.code != Some(0) || out.stdout.len() > 131072 {
        return Err(SshError::Failed("Identification did not return a bounded report. Python 3, ip and nvidia-smi must be available.".into()));
    }
    let facts: Facts = serde_json::from_str(&out.stdout)
        .map_err(|_| SshError::Failed("Identification returned an invalid report.".into()))?;
    if !facts.valid(cfg.ip.as_deref().unwrap_or("")) {
        return Err(SshError::Failed("Identification lacks a valid hostname, machine identity or MAC for the surveyed address.".into()));
    }
    Ok((facts, fp))
}
#[tauri::command]
pub async fn discovery_verify(
    app: AppHandle,
    id: String,
    address: String,
    user: String,
) -> Result<Run, String> {
    admin::require_mac(&app)?;
    if admin::look_only() {
        return Err("Verification is unavailable in look-only mode.".into());
    }
    if user.is_empty()
        || user.len() > 64
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return Err("Enter a valid login user in Account first.".into());
    }
    let demo = !crate::is_live(&app.state::<AppState>());
    let service = app.state::<Discovery>();
    let run = service
        .view(demo)
        .filter(|r| r.id == id && r.status == "complete" && r.verifying.is_none())
        .ok_or("Finish the current survey before verifying one candidate at a time.")?;
    if !run
        .candidates
        .iter()
        .any(|c| c.address == address && c.ports.contains(&22))
    {
        return Err("Choose an SSH candidate from this survey.".into());
    }
    // Reservation is atomic even when two windows ask at once.
    {
        let mut s = service.store.lock().unwrap();
        let r = s
            .run
            .as_mut()
            .filter(|r| r.id == id && r.verifying.is_none() && r.status == "complete")
            .ok_or("Another verification is running.")?;
        r.verifying = Some(address.clone());
    }
    struct Release<'a>(&'a Discovery, String);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.edit(&self.1, |r| r.verifying = None);
        }
    }
    let _release = Release(&service, id.clone());
    let work = async {
        if demo {
            tokio::time::sleep(Duration::from_millis(150)).await;
            return demo_facts(&address)
                .map(|f| (f, Some("Demo key — no real trust".into())))
                .map_err(SshError::Failed);
        }
        let ip: Ipv4Addr = address
            .parse()
            .map_err(|_| SshError::Failed("Invalid address".into()))?;
        if !addresses(&run.scope, &crate::wol::mac_interfaces())
            .map_err(SshError::Failed)?
            .contains(&ip)
        {
            return Err(SshError::Failed("Address is outside this survey.".into()));
        }
        let stream = connect(&run.scope, ip, 22)
            .await
            .map_err(SshError::Failed)?;
        let cfg = NodeConfig {
            id: format!("discovery-{address}"),
            name: address.clone(),
            host: address.clone(),
            ip: Some(address.clone()),
            model: String::new(),
            port: 22,
            user,
            fabric: None,
        };
        identify(&app.state::<AppState>().live, &cfg, stream).await
    };
    let result = tokio::select! {_=stopped(&app,&service,&id,demo,&run.scope)=>return Err("Verification stopped; survey again before using these results.".into()),r=timeout(Duration::from_secs(30),work)=>r.unwrap_or_else(|_|Err(SshError::Failed("Verification reached its 30-second limit.".into())))};
    if !service.current(&id, demo)
        || crate::is_live(&app.state::<AppState>()) == demo
        || admin::look_only()
        || (!demo && addresses(&run.scope, &crate::wol::mac_interfaces()).is_err())
    {
        return Err("The survey changed; the reply was discarded.".into());
    }
    service.edit(&id,|r|if let Some(c)=r.candidates.iter_mut().find(|c|c.address==address) {
        c.facts=None;c.fingerprint=None;c.changed_key=false;
        match result {
            Ok((f,fp))=>{c.status=if f.spark(){"spark"}else{"other"}.into();c.message=if f.spark(){"GB10 reported through trusted SSH. Review identity and topology before using the proposal."}else{"SSH verified, but no GB10 GPU was reported; this is not a confirmed Spark."}.into();c.fingerprint=fp;c.facts=Some(f);},
            Err(SshError::HostKeyUnknown{fingerprint,..})=>{c.status="trust".into();c.message="Compare the SSH host key on the device, then Trust and verify again. No login was attempted.".into();c.fingerprint=Some(fingerprint);},
            Err(SshError::HostKeyChanged{fingerprint,..})=>{c.status="trust".into();c.changed_key=true;c.message="The SSH host key changed. Review the device before replacing trust.".into();c.fingerprint=Some(fingerprint);},
            Err(e)=>{c.status="error".into();c.message=crate::live::audit::redact(&e.to_string());}
        }
    });
    let mut view = service.view(demo).ok_or("Survey ended")?;
    view.verifying = None;
    Ok(view)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub name: String,
    pub address: String,
    pub action: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub node: String,
    pub iface: String,
    pub peer: String,
    pub port: String,
    pub evidence: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub inventory: Inventory,
    pub changes: Vec<Change>,
    pub links: Vec<Link>,
    pub networks: BTreeMap<String, Vec<String>>,
    pub warnings: Vec<String>,
}
fn hostname(s: &str) -> String {
    s.trim_end_matches('.')
        .trim_end_matches(".local")
        .to_ascii_lowercase()
}
fn propose(
    run: &Run,
    selected: &[String],
    inventory: &Inventory,
    pins: &BTreeMap<String, Pin>,
    user: &str,
) -> Result<Proposal, String> {
    if run.status != "complete"
        || run.verifying.is_some()
        || now().saturating_sub(run.started_ms) >= TTL_MS
    {
        return Err("Complete a fresh survey and verification first.".into());
    }
    if selected.is_empty()
        || selected.len() > 4
        || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
    {
        return Err("Select one to four distinct verified Sparks.".into());
    }
    let mut p=Proposal{inventory:inventory.clone(),changes:Vec::new(),links:Vec::new(),networks:BTreeMap::new(),warnings:vec!["Shared network addresses establish network membership, not physical cabling. LLDP neighbors are reports, not verified switch configuration.".into()]};
    let mut ids = BTreeSet::new();
    for address in selected {
        let c = run
            .candidates
            .iter()
            .find(|c| &c.address == address && c.status == "spark")
            .ok_or("Only verified GB10 Sparks can be proposed.")?;
        let f = c
            .facts
            .as_ref()
            .filter(|f| f.valid(address) && f.spark())
            .ok_or("Missing verified identity")?;
        let mac = f.lan_mac(address).unwrap();
        // A Spark can answer on both Ethernet and Wi-Fi. Shared machine-ids across
        // differently named clones are allowed, but selecting two attachments of
        // the same named machine would invent an extra node.
        if run
            .candidates
            .iter()
            .filter(|other| selected.contains(&other.address))
            .filter_map(|other| other.facts.as_ref())
            .filter(|other| {
                other.machine_id == f.machine_id
                    && hostname(&other.hostname) == hostname(&f.hostname)
            })
            .count()
            > 1
        {
            return Err("Two selected addresses report the same hostname and machine-id. Select only one address or resolve the duplicate identity first.".into());
        }
        if run
            .candidates
            .iter()
            .filter_map(|c| c.facts.as_ref().map(|f| (c, f)))
            .filter(|(c, f)| f.lan_mac(&c.address).as_deref() == Some(&mac))
            .count()
            > 1
        {
            return Err("Two addresses report the same LAN MAC. Review duplicate identities before using this proposal.".into());
        }
        let matches: Vec<_> = inventory
            .nodes
            .iter()
            .filter(|n| {
                pins.get(&n.id).is_some_and(|pin| {
                    pin.lan_mac
                        .as_deref()
                        .is_some_and(|m| m.eq_ignore_ascii_case(&mac))
                        || (!pin.cloned_machine_id
                            && pin.machine_id.as_deref() == Some(&f.machine_id))
                }) || n.ip.as_deref() == Some(address)
                    || n.host == *address
                    || hostname(&n.host) == hostname(&f.hostname)
            })
            .collect();
        if matches.len() > 1 {
            return Err(format!(
                "{address} matches more than one saved node. Review the inventory first."
            ));
        }
        let old = matches.first().copied();
        if let Some(old) = old {
            if let Some(pin) = pins.get(&old.id) {
                if !pin.hostname.is_empty() && hostname(&pin.hostname) != hostname(&f.hostname) {
                    return Err(format!("{} reports a different pinned hostname. Review its identity in Settings first.", old.name));
                }
                if pin
                    .lan_mac
                    .as_deref()
                    .is_some_and(|m| !m.eq_ignore_ascii_case(&mac))
                    || (!pin.cloned_machine_id
                        && pin.machine_id.as_deref().is_some_and(|m| m != f.machine_id))
                {
                    return Err(format!(
                        "{} has a different pinned identity. Discovery cannot re-pin it.",
                        old.name
                    ));
                }
                if pin.lan_mac.is_none() && (pin.machine_id.is_none() || pin.cloned_machine_id) {
                    return Err(format!(
                        "{} has no comparable identity pin; review it in Settings first.",
                        old.name
                    ));
                }
            } else if hostname(&old.host) != hostname(&f.hostname) {
                return Err(format!("{address} overlaps {} but its hostname differs and there is no identity pin. Review it before changing the saved node.",old.name));
            }
        }
        let id = old
            .map(|n| n.id.clone())
            .unwrap_or_else(|| format!("spark-{}", &crate::live::sha256_hex(mac.as_bytes())[..12]));
        if !ids.insert(id.clone()) || (old.is_none() && inventory.nodes.iter().any(|n| n.id == id))
        {
            return Err("The proposal contains a duplicate stable node identity.".into());
        }
        let node = NodeConfig {
            id: id.clone(),
            name: old
                .map(|n| n.name.clone())
                .unwrap_or_else(|| f.hostname.clone()),
            host: format!("{}.local", hostname(&f.hostname)),
            ip: Some(address.clone()),
            model: f.model.clone(),
            port: 22,
            user: user.into(),
            fabric: old.and_then(|n| n.fabric.clone()),
        };
        p.changes.push(Change {
            id: id.clone(),
            name: node.name.clone(),
            address: address.clone(),
            action: if old.is_some() {
                "Update address; preserve saved ID and expectations"
            } else {
                "Add verified Spark"
            }
            .into(),
        });
        for i in &f.interfaces {
            for a in &i.addresses {
                if let Some(net) = cidr(a) {
                    p.networks.entry(net).or_default().push(format!(
                        "{} · {} · {}{}",
                        node.name,
                        i.name,
                        a,
                        i.speed_mbps
                            .map(|s| format!(" · {s} Mb/s"))
                            .unwrap_or_default()
                    ));
                }
            }
        }
        for n in &f.neighbors {
            p.links.push(Link {
                node: node.name.clone(),
                iface: n.iface.clone(),
                peer: if n.system.is_empty() {
                    n.chassis.clone()
                } else {
                    n.system.clone()
                },
                port: n.port.clone(),
                evidence: format!(
                    "LLDP chassis {}",
                    if n.chassis.is_empty() {
                        "unknown"
                    } else {
                        &n.chassis
                    }
                ),
            });
        }
        if f.neighbors.is_empty() {
            p.warnings.push(format!(
                "{}: no LLDP neighbors were reported; cabling is unknown.",
                node.name
            ));
        }
        if run
            .candidates
            .iter()
            .filter_map(|c| c.facts.as_ref())
            .filter(|other| other.machine_id == f.machine_id)
            .count()
            > 1
        {
            p.warnings.push(format!("{} shares a machine-id with another candidate; separate LAN MACs identify these nodes. Host keys alone never merge nodes.",node.name));
        }
        if let Some(index) = p.inventory.nodes.iter().position(|n| n.id == id) {
            p.inventory.nodes[index] = node;
        } else {
            p.inventory.nodes.push(node);
        }
    }
    Ok(p)
}
// A broken pin file must never downgrade a known device to an unpinned hostname match.
fn load_pins(path: &std::path::Path) -> Result<BTreeMap<String, Pin>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Saved node identity pins cannot be read. Resolve them in Settings before proposing changes.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(_) => Err("Saved node identity pins are unavailable. Resolve them before proposing changes.".into()),
    }
}
#[tauri::command]
pub fn discovery_propose(
    app: AppHandle,
    id: String,
    selected: Vec<String>,
    inventory: Inventory,
    user: String,
) -> Result<Proposal, String> {
    if user.is_empty()
        || user.len() > 64
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return Err("Enter a valid account user.".into());
    }
    let state = app.state::<AppState>();
    let demo = !crate::is_live(&state);
    let r = app
        .state::<Discovery>()
        .view(demo)
        .filter(|r| r.id == id)
        .ok_or("Survey changed or expired")?;
    if !demo {
        addresses(&r.scope, &crate::wol::mac_interfaces())?;
    }
    let pins = if demo {
        BTreeMap::new()
    } else {
        load_pins(&state.live.data_path("node_pins.json"))?
    };
    propose(&r, &selected, &inventory, &pins, &user)
}
fn demo_facts(address: &str) -> Result<Facts, String> {
    let mut facts: BTreeMap<String, Facts> =
        serde_json::from_str(include_str!("facts.json")).expect("discovery identity fixture");
    facts
        .remove(address)
        .ok_or_else(|| "Demo: this device is a generic SSH host, not a Spark.".into())
}
#[cfg(test)]
mod tests;
