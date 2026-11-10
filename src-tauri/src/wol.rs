//! Wake-on-LAN: a node that's switched off is woken by a magic packet (6 × 0xFF, then its
//! LAN MAC 16 times) broadcast on UDP port 9 of the cluster LAN.
//!
//! The packet goes from a node that's still on, over SSH (`python3 -I -c`, to the LAN's
//! broadcast address and 255.255.255.255), so it works from anywhere the VPN reaches.
//! Without an online node, it goes from this Mac when the Mac is on the cluster LAN. A
//! node's MAC comes from its collector frames and is remembered (wol-macs.json next to
//! the known hosts), because an off node sends none. Whether a node will wake at all is
//! its LAN port's Wake-on setting: "g" (magic packet) is on. On DGX OS only root can read it
//! with ethtool, so the collector reads NetworkManager's instead; when neither says, it's
//! unknown ("Unknown · needs sudo"), not off, and no fix is offered.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::live::{sh_quote, Live};
use crate::model::{ClusterSnapshot, LinkStatus, NetRole, WolInfo};
use crate::AppState;

pub const PORT: u16 = 9;

/// How a wake was (or can be) sent.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WakeRoute {
    /// From an online node over SSH, on the LAN.
    Peer { node: String, name: String },
    /// From this Mac, which is on the cluster LAN.
    Mac { address: String },
}

impl WakeRoute {
    pub fn describe(&self) -> String {
        match self {
            WakeRoute::Peer { name, .. } => format!("from {name}"),
            WakeRoute::Mac { address } => format!("from this Mac ({address})"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WolStatus {
    pub node: String,
    /// The LAN port's MAC, from the collector (now or remembered).
    pub mac: Option<String>,
    /// The LAN interface, e.g. enP7s7.
    pub iface: Option<String>,
    /// Wake-on "g" on the LAN port (None: not known).
    pub enabled: Option<bool>,
    /// What's known, in words: "On (NetworkManager: magic packet)", "Not readable without sudo (ethtool needs root)".
    pub detail: Option<String>,
    /// The ways a wake could go right now, preferred first.
    pub routes: Vec<WakeRoute>,
    /// What to run on the node to turn it on, when it's off.
    pub fix: Option<String>,
    /// How to keep it on across restarts.
    pub persist: Option<String>,
}

pub fn parse_mac(mac: &str) -> Result<[u8; 6], String> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let parts = mac.split([':', '-']).count();
    if hex.len() != 12 || !(parts == 6 || parts == 1) || mac.chars().any(|c| !(c.is_ascii_hexdigit() || c == ':' || c == '-')) {
        return Err(format!("“{mac}” isn't a MAC address."));
    }
    let mut out = [0u8; 6];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    Ok(out)
}

/// 6 × 0xFF followed by the MAC 16 times (102 bytes).
pub fn magic_packet(mac: &str) -> Result<Vec<u8>, String> {
    let m = parse_mac(mac)?;
    let mut p = vec![0xFF; 6];
    for _ in 0..16 {
        p.extend_from_slice(&m);
    }
    Ok(p)
}

/// The script a peer runs: sends the packet to each target (and to its LAN's own broadcast
/// address, from `ip`), then prints where it went. Arguments: mac, LAN interface, port, targets….
const PEER_SCRIPT: &str = r#"import json, socket, subprocess, sys
mac, iface, port, targets = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4:]
pkt = b"\xff" * 6 + bytes.fromhex(mac.replace(":", "").replace("-", "")) * 16
try:
    for a in json.loads(subprocess.run(["ip", "-j", "-4", "addr", "show", "dev", iface], capture_output=True, text=True).stdout or "[]"):
        targets += [i["broadcast"] for i in a.get("addr_info", []) if i.get("broadcast")]
except (OSError, ValueError):
    pass
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
sent = []
for t in dict.fromkeys(targets):
    try:
        s.sendto(pkt, (t, port))
        sent.append(t)
    except OSError as e:
        print("%s: %s" % (t, e), file=sys.stderr)
print(" ".join(sent))
sys.exit(0 if sent else 1)
"#;

/// The command a peer runs to wake `mac`.
pub fn peer_command(mac: &str, iface: &str, port: u16, targets: &[String]) -> Result<String, String> {
    parse_mac(mac)?;
    let iface_ok = !iface.is_empty() && iface.len() < 32 && iface.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let iface = if iface_ok { iface } else { "lo" };
    let targets: Vec<String> = targets.iter().filter(|t| t.parse::<Ipv4Addr>().is_ok()).map(|t| sh_quote(t)).collect();
    Ok(format!("python3 -I -c {} {} {} {port} {}", sh_quote(PEER_SCRIPT), sh_quote(mac), sh_quote(iface), targets.join(" ")))
}

/// x.y.z.255 for a LAN address (home LANs are /24; the peer adds its real broadcast too).
fn lan_broadcast(ip: &str) -> Option<String> {
    let a: Ipv4Addr = ip.parse().ok()?;
    let o = a.octets();
    Some(Ipv4Addr::new(o[0], o[1], o[2], 255).to_string())
}

/// A node's LAN interface: (iface, mac, ip, wake-on).
type Lan = (String, Option<String>, Option<String>, Option<WolInfo>);

/// The node's LAN interface in a snapshot.
fn lan_of(snap: &ClusterSnapshot, node: &str) -> Option<Lan> {
    let n = snap.nodes.iter().find(|n| n.id == node)?;
    let lan = n.nets.iter().find(|i| i.role == NetRole::Lan && i.mac.is_some()).or_else(|| n.nets.iter().find(|i| i.role == NetRole::Lan))?;
    Some((lan.iface.clone(), lan.mac.clone(), lan.ip.clone(), lan.lan_wol.clone()))
}

/// MACs remembered from earlier frames: node → (iface, mac, ip).
pub type MacBook = BTreeMap<String, (String, String, Option<String>)>;

pub fn load_book(path: &Path) -> MacBook {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Notes every node's LAN MAC that the snapshot shows (saved only when something changed).
pub fn remember(path: &Path, snap: &ClusterSnapshot) -> MacBook {
    let mut book = load_book(path);
    let before = book.clone();
    for n in &snap.nodes {
        if let Some((iface, Some(mac), ip, _)) = lan_of(snap, &n.id) {
            if parse_mac(&mac).is_ok() {
                book.insert(n.id.clone(), (iface, mac.to_lowercase(), ip));
            }
        }
    }
    if book != before {
        let _ = crate::util::save_json(path, &book);
    }
    book
}

/// One of this Mac's IPv4 addresses, with the interface it's on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MacIface {
    /// en0, bridge100, utun4…
    pub name: String,
    pub addr: Ipv4Addr,
    pub mask: Ipv4Addr,
    /// A tunnel (utun, ppp, ipsec…) or another point-to-point link: never the LAN itself,
    /// whatever address another VPN app gives it.
    pub point_to_point: bool,
}

impl MacIface {
    /// The interface's broadcast address (`addr | !mask`).
    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) | !u32::from(self.mask))
    }

    /// Whether `ip` is on this interface's own network (`addr & mask == ip & mask`). A tunnel
    /// never is, and neither is a mask of 0, which would match every address.
    pub fn holds(&self, ip: Ipv4Addr) -> bool {
        let m = u32::from(self.mask);
        !self.point_to_point && m != 0 && u32::from(self.addr) & m == u32::from(ip) & m
    }
}

/// Interface names macOS gives tunnels, also when the point-to-point flag isn't set.
const TUNNEL_PREFIXES: &[&str] = &["utun", "ppp", "ipsec", "gif", "stf", "tun"];

/// Tests: an interface as getifaddrs would list it ("utun…" counts as a tunnel, as above).
#[cfg(test)]
pub(crate) fn iface(name: &str, addr: &str, mask: &str) -> MacIface {
    MacIface { name: name.into(), addr: addr.parse().unwrap(), mask: mask.parse().unwrap(), point_to_point: TUNNEL_PREFIXES.iter().any(|t| name.starts_with(t)) }
}

/// This Mac's IPv4 addresses (loopback left out), each with its interface's netmask.
pub(crate) fn mac_interfaces() -> Vec<MacIface> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut p = ifap;
    while !p.is_null() {
        let ifa = unsafe { &*p };
        if !ifa.ifa_addr.is_null() && unsafe { (*ifa.ifa_addr).sa_family } as i32 == libc::AF_INET && !ifa.ifa_netmask.is_null() {
            let addr = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
            let mask = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in) };
            let a = u32::from_be(addr.sin_addr.s_addr);
            let m = u32::from_be(mask.sin_addr.s_addr);
            let name = if ifa.ifa_name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_string_lossy().into_owned() };
            let point_to_point = ifa.ifa_flags & libc::IFF_POINTOPOINT as u32 != 0 || TUNNEL_PREFIXES.iter().any(|t| name.starts_with(t));
            if a >> 24 != 127 {
                out.push(MacIface { name, addr: Ipv4Addr::from(a), mask: Ipv4Addr::from(m), point_to_point });
            }
        }
        p = ifa.ifa_next;
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

/// This Mac's interface on `lan_ip`'s network, if the Mac is on that LAN: the interface's own
/// netmask decides (a /16 or /22 home LAN too), and tunnels don't count.
pub(crate) fn mac_on_lan<'a>(lan_ip: Option<&str>, ifaces: &'a [MacIface]) -> Option<&'a MacIface> {
    let ip: Ipv4Addr = lan_ip?.parse().ok()?;
    ifaces.iter().find(|i| i.holds(ip))
}

/// The ways a wake could go now, preferred first: online peers with a LAN port, then the Mac.
pub fn routes(snap: &ClusterSnapshot, node: &str, lan_ip: Option<&str>, ifaces: &[MacIface]) -> Vec<WakeRoute> {
    let mut out: Vec<WakeRoute> = snap
        .nodes
        .iter()
        .filter(|n| n.id != node && n.status == LinkStatus::Online && !n.stale && n.nets.iter().any(|i| i.role == NetRole::Lan))
        .map(|n| WakeRoute::Peer { node: n.id.clone(), name: n.name.clone() })
        .collect();
    if let Some(mac) = mac_on_lan(lan_ip, ifaces) {
        out.push(WakeRoute::Mac { address: mac.addr.to_string() });
    }
    out
}

/// Sends the packet from this Mac to its LAN broadcast and 255.255.255.255.
pub fn send_from_mac(mac: &str, targets: &[Ipv4Addr], port: u16) -> Result<(), String> {
    let packet = magic_packet(mac)?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("Couldn't open a UDP socket: {e}"))?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;
    for t in targets {
        sock.send_to(&packet, (*t, port)).map_err(|e| format!("Couldn't send the wake packet to {t}: {e}"))?;
    }
    Ok(())
}

/// Wakes `node` from the first route that works. `port` is 9 except in tests. The write exit
/// admits the wake for `node` first (look-only, the fence's `power-one`); the peer that relays
/// the packet runs it as `as_use`, since it changes nothing there.
pub async fn send_with(live: &Live, snap: &ClusterSnapshot, book: &MacBook, node: &str, port: u16, extra: &[String]) -> Result<WakeRoute, String> {
    let name = snap.nodes.iter().find(|n| n.id == node).map(|n| n.name.clone()).unwrap_or_else(|| node.to_string());
    live.admit_from_mac(node, crate::live::fence_class::FenceClass::PowerOne, &format!("Wake-on-LAN for {name}")).map_err(|e| e.to_json())?;
    let (iface, mac, ip) = match (lan_of(snap, node), book.get(node)) {
        (Some((iface, Some(mac), ip, _)), _) => (iface, mac, ip),
        (_, Some((iface, mac, ip))) => (iface.clone(), mac.clone(), ip.clone()),
        _ => return Err(format!("SparkBox doesn't know {name}'s MAC address yet. It learns it while the node is on; wake it by its power button this time.")),
    };
    let ifaces = mac_interfaces();
    let mut errors = Vec::new();
    for route in routes(snap, node, ip.as_deref(), &ifaces) {
        match &route {
            WakeRoute::Peer { node: peer, name: peer_name } => {
                let mut targets: Vec<String> = ip.as_deref().and_then(lan_broadcast).into_iter().collect();
                targets.push("255.255.255.255".into());
                targets.extend(extra.iter().cloned());
                let cmd = peer_command(&mac, &iface, port, &targets)?;
                match crate::live::exit::as_use(live.run(peer, &cmd, None, Duration::from_secs(15))).await {
                    Ok(o) if o.code == Some(0) => return Ok(route),
                    Ok(o) => errors.push(format!("{peer_name}: {}", o.stderr.trim().lines().last().unwrap_or("it failed"))),
                    Err(e) => errors.push(format!("{peer_name}: {e}")),
                }
            }
            WakeRoute::Mac { .. } => {
                let bcast = mac_on_lan(ip.as_deref(), &ifaces).unwrap().broadcast();
                match send_from_mac(&mac, &[bcast, Ipv4Addr::BROADCAST], port) {
                    Ok(()) => return Ok(route),
                    Err(e) => errors.push(e),
                }
            }
        }
    }
    if errors.is_empty() {
        Err(format!("No node is online to send the wake packet, and this Mac isn't on {name}'s LAN. Wake another node first, or use its power button."))
    } else {
        Err(format!("Couldn't wake {name}: {}", errors.join("; ")))
    }
}

/// What the Connect wizard and node view show about waking a node.
pub fn status_from(snap: &ClusterSnapshot, book: &MacBook, node: &str, ifaces: &[MacIface]) -> WolStatus {
    let now = lan_of(snap, node);
    let known = book.get(node);
    let iface = now.as_ref().map(|l| l.0.clone()).or_else(|| known.map(|k| k.0.clone()));
    let mac = now.as_ref().and_then(|l| l.1.clone()).or_else(|| known.map(|k| k.1.clone()));
    let ip = now.as_ref().and_then(|l| l.2.clone()).or_else(|| known.and_then(|k| k.2.clone()));
    let wol = now.as_ref().and_then(|l| l.3.clone());
    let enabled = wol.as_ref().and_then(|w| match w.state.as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    });
    let i = iface.clone().unwrap_or_else(|| "enP7s7".into());
    // The fix only for a setting that's really off: unknown isn't off.
    WolStatus {
        node: node.into(),
        routes: routes(snap, node, ip.as_deref(), ifaces),
        fix: (enabled == Some(false)).then(|| format!("sudo ethtool -s {i} wol g")),
        persist: (enabled == Some(false)).then(|| {
            format!("Add `wakeonlan: true` under {i} in the node's netplan file (/etc/netplan/*.yaml), then run `sudo netplan apply`.")
        }),
        detail: wol.map(|w| w.detail).filter(|d| !d.is_empty()),
        mac,
        iface,
        enabled,
    }
}

fn book_path(state: &AppState) -> PathBuf {
    state.known_hosts_path.with_file_name("wol-macs.json")
}

fn latest(state: &AppState) -> ClusterSnapshot {
    state.latest.read().unwrap().clone().unwrap_or_default()
}

/// Wakes a node: Demo powers the simulated one on; Live sends the magic packet. Part of C-live.
pub async fn send(app: &AppHandle, node: &str) -> Result<WakeRoute, String> {
    let state = app.state::<AppState>();
    let snap = latest(&state);
    if !crate::is_live(&state) {
        let route = routes(&snap, node, None, &[]).into_iter().next().ok_or("No other node is on to send the wake packet.")?;
        state.demo.lock().unwrap().power(node, "wake")?;
        state.live.audit.record_simulated(node, &format!("Wake {node}"), "python3 -I -c '<send a magic packet>' (Wake-on-LAN)");
        return Ok(route);
    }
    let book = remember(&book_path(&state), &snap);
    send_with(&state.live, &snap, &book, node, PORT, &[]).await
}

/// MAC, Wake-on setting and the routes a wake could take. Part of C-live.
pub async fn status(app: &AppHandle, node: &str) -> WolStatus {
    let state = app.state::<AppState>();
    let snap = latest(&state);
    let book = if crate::is_live(&state) { remember(&book_path(&state), &snap) } else { MacBook::new() };
    // The collector reads Wake-on as far as a normal user can (ethtool, then NetworkManager).
    status_from(&snap, &book, node, &mac_interfaces())
}

/// Wake-on-LAN facts for a node (read-only).
#[tauri::command]
pub async fn node_wol_status(app: AppHandle, _state: State<'_, AppState>, node: String) -> Result<WolStatus, String> {
    Ok(status(&app, &node).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{connect, fake_node, real_shell, temp};
    use crate::model::{NetStats, NodeSnapshot};

    fn snap(nodes: &[(&str, LinkStatus, Option<&str>)]) -> ClusterSnapshot {
        ClusterSnapshot {
            nodes: nodes
                .iter()
                .map(|(id, status, mac)| NodeSnapshot {
                    id: id.to_string(),
                    name: id.to_string(),
                    status: *status,
                    nets: vec![NetStats {
                        iface: "enP7s7".into(),
                        role: NetRole::Lan,
                        ip: Some("192.168.50.51".into()),
                        mac: mac.map(String::from),
                        lan_wol: Some("g".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_magic_packet_is_six_ff_then_sixteen_macs() {
        let p = magic_packet("4c:bb:47:0a:1b:2c").unwrap();
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &[0x4c, 0xbb, 0x47, 0x0a, 0x1b, 0x2c]);
        }
        assert_eq!(magic_packet("4C-BB-47-0A-1B-2C").unwrap(), p);
        for bad in ["4c:bb:47:0a:1b", "zz:bb:47:0a:1b:2c", "4c:bb:47:0a:1b:2c; reboot", ""] {
            assert!(magic_packet(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_peer_is_preferred_then_the_mac_on_the_same_lan() {
        let s = snap(&[("cell1", LinkStatus::Online, Some("aa:aa:aa:aa:aa:01")), ("cell3", LinkStatus::Offline, None), ("cell2", LinkStatus::Error, None)]);
        let mac_lan = [iface("en0", "192.168.50.20", "255.255.255.0")];
        assert_eq!(
            routes(&s, "cell3", Some("192.168.50.53"), &mac_lan),
            vec![WakeRoute::Peer { node: "cell1".into(), name: "cell1".into() }, WakeRoute::Mac { address: "192.168.50.20".into() }],
            "an offline or failing node can't send it"
        );
        let away = [iface("en0", "10.0.0.5", "255.255.255.0")];
        assert_eq!(routes(&s, "cell3", Some("192.168.50.53"), &away).len(), 1, "a Mac on another network can't broadcast to the LAN");
        assert!(routes(&snap(&[("cell3", LinkStatus::Offline, None)]), "cell3", Some("192.168.50.53"), &away).is_empty());

        // Status: remembered MAC for an off node, the fix while Wake-on is off.
        let book: MacBook = [("cell3".to_string(), ("enP7s7".to_string(), "aa:aa:aa:aa:aa:03".to_string(), Some("192.168.50.53".to_string())))].into();
        let st = status_from(&s, &book, "cell3", &away);
        assert_eq!((st.mac.as_deref(), st.enabled), (Some("aa:aa:aa:aa:aa:03"), Some(true)));
        let mut off = s.clone();
        off.nodes[0].nets[0].lan_wol = Some("d".into());
        let st = status_from(&off, &book, "cell1", &away);
        assert_eq!((st.enabled, st.fix.as_deref()), (Some(false), Some("sudo ethtool -s enP7s7 wol g")));
        assert!(st.persist.unwrap().contains("wakeonlan: true"));
    }

    #[test]
    fn the_mac_is_on_a_lan_by_its_interface_netmask_and_never_through_a_tunnel() {
        let on = |ip: &str, ifaces: &[MacIface]| mac_on_lan(Some(ip), ifaces).map(|i| i.name.clone());
        // A /24: the same network, not the neighbouring one.
        let home = [iface("en0", "192.168.50.113", "255.255.255.0")];
        assert_eq!(on("192.168.50.51", &home).as_deref(), Some("en0"));
        assert_eq!(on("192.168.51.51", &home), None);
        // A /22 home LAN holds 192.168.48.0–51.255: a node at .50 is on it although the third
        // octets differ (comparing three octets said no).
        let wide = [iface("en0", "192.168.48.20", "255.255.252.0")];
        assert_eq!(on("192.168.50.51", &wide).as_deref(), Some("en0"));
        assert_eq!(on("192.168.52.1", &wide), None);
        assert_eq!(wide[0].broadcast(), Ipv4Addr::new(192, 168, 51, 255), "the wake goes to the real broadcast");
        // A /25 splits a third octet: 192.168.50.130 isn't on .0/25 (three octets said yes).
        let narrow = [iface("en0", "192.168.50.20", "255.255.255.128")];
        assert_eq!(on("192.168.50.130", &narrow), None);
        assert_eq!(on("192.168.50.100", &narrow).as_deref(), Some("en0"));
        // Another VPN app's utun with an address in the LAN's range isn't the LAN; nor is a
        // point-to-point link by its flag, whatever it's called.
        let away = [iface("en0", "10.0.0.23", "255.255.255.0"), iface("utun4", "192.168.50.7", "255.255.255.0")];
        assert_eq!(on("192.168.50.51", &away), None);
        let ppp = [MacIface { point_to_point: true, ..iface("en5", "192.168.50.9", "255.255.255.0") }];
        assert_eq!(on("192.168.50.51", &ppp), None);
        // A zero mask would match everything; it's never taken for the LAN. Names aren't addresses.
        assert_eq!(on("192.168.50.51", &[iface("en0", "192.168.50.2", "0.0.0.0")]), None);
        assert_eq!(mac_on_lan(Some("cell1.local"), &home), None);
        assert_eq!(mac_on_lan(None, &home), None);
    }

    #[test]
    fn this_macs_own_interfaces_carry_their_netmask() {
        // Read-only: getifaddrs on this Mac. Loopback is left out; every mask is a real prefix.
        for i in mac_interfaces() {
            assert_ne!(i.addr.octets()[0], 127, "{i:?}");
            let m = u32::from(i.mask);
            assert_eq!(m.leading_ones() + m.trailing_zeros(), 32, "{i:?} has a contiguous mask");
            if i.name.starts_with("utun") {
                assert!(i.point_to_point, "{i:?}");
            }
        }
    }

    #[test]
    fn unreadable_wake_on_lan_is_unknown_with_no_fix() {
        // What the collector sends on a real Spark: ethtool refused, NetworkManager at the driver default.
        let mut s = snap(&[("cell1", LinkStatus::Online, Some("fc:9d:05:d5:54:96"))]);
        s.nodes[0].nets[0].lan_wol = Some(WolInfo {
            state: "unknown".into(),
            source: "networkmanager".into(),
            detail: "Not readable without sudo. NetworkManager leaves it to the driver default.".into(),
            raw: None,
        });
        let st = status_from(&s, &MacBook::new(), "cell1", &[]);
        assert_eq!((st.enabled, st.fix, st.persist), (None, None, None));
        assert_eq!(st.detail.as_deref(), Some("Not readable without sudo. NetworkManager leaves it to the driver default."));
        s.nodes[0].nets[0].lan_wol = Some(WolInfo { state: "on".into(), source: "networkmanager".into(), detail: "On (NetworkManager: magic packet)".into(), raw: None });
        assert_eq!(status_from(&s, &MacBook::new(), "cell1", &[]).enabled, Some(true));
    }

    #[tokio::test]
    async fn a_peer_sends_the_packet_over_ssh() {
        let (peer, port) = fake_node(&temp("wol-peer"), real_shell()).await;
        let (live, _) = connect(&temp("wol-app"), &[("cell1", port)]).await;
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let udp = listener.local_addr().unwrap().port();
        let s = snap(&[("cell1", LinkStatus::Online, Some("aa:aa:aa:aa:aa:01")), ("cell3", LinkStatus::Offline, None)]);
        let book: MacBook = [("cell3".to_string(), ("enP7s7".to_string(), "4c:bb:47:0a:1b:2c".to_string(), Some("127.0.0.3".to_string())))].into();
        // In the test the packet also goes to 127.0.0.1 so it can be caught.
        let route = send_with(&live, &s, &book, "cell3", udp, &["127.0.0.1".into()]).await.unwrap();
        assert_eq!(route, WakeRoute::Peer { node: "cell1".into(), name: "cell1".into() });
        let mut buf = [0u8; 200];
        let (n, _) = listener.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], magic_packet("4c:bb:47:0a:1b:2c").unwrap().as_slice());
        let ran = peer.ran("python3 -I -c ");
        assert_eq!(ran.len(), 1);
        assert!(ran[0].ends_with(&format!("'4c:bb:47:0a:1b:2c' 'enP7s7' {udp} '127.0.0.255' '255.255.255.255' '127.0.0.1'")), "{}", ran[0]);
        assert_eq!(live.audit.entries().iter().filter(|e| e.command.starts_with("python3 -I -c")).count(), 1, "logged");

        let unknown = send_with(&live, &s, &MacBook::new(), "cell3", udp, &[]).await.unwrap_err();
        assert!(unknown.contains("doesn't know cell3's MAC address"), "{unknown}");
        live.stop();
    }
}
