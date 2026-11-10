//! Menu bar icon: every node's health and inference status at a glance, plus quick actions.
//! SparkBox keeps running here (server, VPN, monitoring) when its window is closed.
//!
//! Status is drawn, not spelled with emoji: each node line carries a small shape (a filled
//! circle when healthy, a triangle for a warning, a diamond when critical, a ring when
//! offline), and the menu bar icon itself gains a warning mark or fades when nodes are in
//! trouble or unreachable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::health::{Health, HealthView, Level};
use crate::model::{gpu_readable, ClusterSnapshot, LinkStatus, NodeSnapshot};
use crate::models::plan::Stage;
use crate::models::Deployment;
use crate::server::ServerView;
use crate::vpn::{VpnState, VpnStatus};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// What each node is serving: running containers holding GPU memory, else the
/// processes using the GPU.
pub fn engines(n: &NodeSnapshot) -> Vec<String> {
    let containers: Vec<String> = n
        .containers
        .iter()
        .filter(|c| c.state == "running" && c.gpu_mem > 0)
        .map(|c| c.name.clone())
        .collect();
    if !containers.is_empty() {
        return containers;
    }
    let mut procs: Vec<String> = n.gpu.procs.iter().map(|p| p.name.clone()).collect();
    procs.dedup();
    procs
}

/// The shape a node gets in the menu: the same states as the app's status dots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dot {
    Online,
    Idle,
    Connecting,
    Offline,
    Stale,
    Warn,
    Crit,
}

pub fn dot_for(n: &NodeSnapshot, level: Option<Level>) -> Dot {
    match n.status {
        LinkStatus::Offline => Dot::Offline,
        LinkStatus::Error => Dot::Crit,
        LinkStatus::Connecting => Dot::Connecting,
        LinkStatus::Online if n.stale => Dot::Stale,
        LinkStatus::Online => match level {
            Some(Level::Crit) => Dot::Crit,
            Some(Level::Warn) => Dot::Warn,
            _ if engines(n).is_empty() => Dot::Idle,
            _ => Dot::Online,
        },
    }
}

/// Why a node isn't reporting, in the app's words (src/lib/nodeStatus.ts offlineReason):
/// planned work first, then the error's kind, then its text cut short.
pub fn offline_reason(n: &NodeSnapshot) -> String {
    match n.power_state.as_deref() {
        Some("rebooting") => return "restarting".into(),
        Some("shutting-down") => return "shutting down".into(),
        Some("off") => return "off (planned)".into(),
        Some("waking") => return "waking".into(),
        _ => {}
    }
    let kind = n.error_kind.as_deref().or_else(|| n.error.as_deref().and_then(crate::live::error_kind_of));
    match kind {
        Some("hostKeyUnknown") => "host key not trusted yet".into(),
        Some("hostKeyChanged") => "host key changed".into(),
        Some("authFailed") => "login failed".into(),
        Some("timeout") => "no SSH reply".into(),
        Some("refused") => "SSH refused the connection".into(),
        Some("unreachable") => "unreachable on the LAN".into(),
        Some("dns") => "hostname not found".into(),
        _ => n.error.as_deref().unwrap_or("offline").chars().take(60).collect(),
    }
}

/// "GPU 73%", or "GPU —" when nvidia-smi gave no numbers.
fn gpu_text(n: &NodeSnapshot) -> String {
    if gpu_readable(n) {
        format!("GPU {:.0}%", n.gpu.util_pct)
    } else {
        "GPU —".into()
    }
}

/// One menu line per node, e.g. "cell1 — vllm-head · GPU 73% · 99/120 GiB · 65 W".
pub fn node_line(n: &NodeSnapshot) -> String {
    match n.status {
        LinkStatus::Offline | LinkStatus::Error => format!("{} — {}", n.name, offline_reason(n)),
        LinkStatus::Connecting => format!("{} — connecting…", n.name),
        LinkStatus::Online if n.stale => format!("{} — not updating ({} s)", n.name, n.frame_age_ms.unwrap_or(0) / 1000),
        LinkStatus::Online => {
            let mem = format!("{:.0}/{:.0} GiB", n.mem.used as f64 / GIB, n.mem.total as f64 / GIB);
            let engines = engines(n);
            if engines.is_empty() {
                format!("{} — no model loaded · {mem}{}", n.name, if gpu_readable(n) { String::new() } else { " · GPU —".into() })
            } else if gpu_readable(n) {
                format!("{} — {} · {} · {mem} · {:.0} W", n.name, engines.join(", "), gpu_text(n), n.gpu.power_w)
            } else {
                format!("{} — {} · GPU — · {mem}", n.name, engines.join(", "))
            }
        }
    }
}

/// Text beside the icon: nodes online and their average GPU load, plus active alerts.
pub fn title(s: &ClusterSnapshot, health: Option<&HealthView>) -> String {
    let online: Vec<&NodeSnapshot> = s.nodes.iter().filter(|n| n.status == LinkStatus::Online).collect();
    let alerts = health.map(|h| match (h.summary.crit, h.summary.warn) {
        (0, 0) => String::new(),
        (0, w) => format!(" · {w} warning{}", if w == 1 { "" } else { "s" }),
        (c, _) => format!(" · {c} critical"),
    });
    if online.is_empty() {
        return format!("0/{} online", s.nodes.len());
    }
    // A GPU that gave no numbers isn't 0%: it's left out of the average.
    let readable: Vec<&&NodeSnapshot> = online.iter().filter(|n| gpu_readable(n)).collect();
    let gpu = if readable.is_empty() {
        "GPU —".to_string()
    } else {
        format!("{:.0}%", readable.iter().map(|n| n.gpu.util_pct as f64).sum::<f64>() / readable.len() as f64)
    };
    format!("{}/{} · {gpu}{}", online.len(), s.nodes.len(), alerts.unwrap_or_default())
}

/// Models SparkBox loaded, e.g. "Models: qwen3.8-27b on cell1+cell2 ready · glm-5.3 loading 40%".
pub fn models_line(deployments: &[Deployment]) -> String {
    let active: Vec<String> = deployments
        .iter()
        .filter(|d| d.state == "starting" || d.state == "ready" || d.state == "failed")
        .map(|d| {
            let state = match (d.state.as_str(), &d.stage) {
                ("ready", _) => "ready".to_string(),
                ("failed", _) => "failed".to_string(),
                (_, Some(Stage::Loading { percent })) => format!("loading {percent}%"),
                (_, Some(Stage::Joining { have, need })) => format!("joining {have}/{need}"),
                (_, Some(Stage::Warming)) => "warming up".to_string(),
                (_, Some(Stage::Compiling { .. })) => "compiling".to_string(),
                _ => "starting".to_string(),
            };
            format!("{} on {} {state}", d.spec.served_name, d.spec.nodes.join("+"))
        })
        .collect();
    if active.is_empty() {
        "Models: none loaded by SparkBox".into()
    } else {
        format!("Models: {}", active.join(" · "))
    }
}

fn vpn_line(v: &VpnStatus) -> String {
    let proto = match v.protocol.as_deref() {
        Some("amneziawg") => "AmneziaWG",
        Some("wireguard") => "WireGuard",
        _ => "",
    };
    match v.state {
        VpnState::Off => "VPN: off".into(),
        VpnState::Direct => "VPN: not needed (direct)".into(),
        VpnState::Connecting => "VPN: connecting…".into(),
        VpnState::Connected => match v.latency_ms {
            Some(ms) => format!("VPN: connected ({proto}, {ms:.0} ms)"),
            None => format!("VPN: connected ({proto})"),
        },
        VpnState::Error => "VPN: problem — open SparkBox for details".into(),
    }
}

// ---- Drawn icons

/// Coverage (0..1) of a shape at a pixel, sampled 4×4 for smooth edges.
fn coverage(inside: impl Fn(f32, f32) -> bool, x: u32, y: u32) -> f32 {
    let mut hits = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                hits += 1;
            }
        }
    }
    hits as f32 / 16.0
}

/// A 32×32 (16 pt at 2×) status shape in its colour, on transparent.
pub fn dot_icon(dot: Dot) -> Image<'static> {
    const S: u32 = 32;
    let (c, r) = (S as f32 / 2.0, 11.0f32);
    let rgb: [u8; 3] = match dot {
        Dot::Online => [52, 199, 89],
        Dot::Idle => [142, 142, 147],
        Dot::Connecting | Dot::Stale | Dot::Warn => [255, 176, 32],
        Dot::Offline => [142, 142, 147],
        Dot::Crit => [255, 69, 58],
    };
    let shape = move |x: f32, y: f32| -> bool {
        let (dx, dy) = (x - c, y - c);
        let d = (dx * dx + dy * dy).sqrt();
        match dot {
            Dot::Online | Dot::Idle => d <= r * if dot == Dot::Idle { 0.62 } else { 0.8 },
            Dot::Connecting | Dot::Offline => d <= r * 0.8 && d >= r * 0.8 - 3.2,
            Dot::Stale => (d <= r * 0.8 && d >= r * 0.8 - 3.0) || (d <= r * 0.8 && dx <= 0.0),
            Dot::Warn => {
                // An upward triangle.
                let top = c - r;
                let h = 2.0 * r * 0.9;
                y >= top && y <= top + h && dx.abs() <= (y - top) / h * r * 1.05
            }
            Dot::Crit => dx.abs() + dy.abs() <= r,
        }
    };
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let a = coverage(shape, x, y);
            let i = ((y * S + x) * 4) as usize;
            rgba[i..i + 3].copy_from_slice(&rgb);
            rgba[i + 3] = (a * 255.0).round() as u8;
        }
    }
    Image::new_owned(rgba, S, S)
}

/// How the menu bar icon looks: as designed, with a warning mark, or faded when nothing answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconVariant {
    Normal,
    Warning,
    Offline,
}

/// Derives a variant from the template icon: a small triangle cut-and-filled into the
/// bottom-right corner for warnings, 40% opacity when no node is online.
pub fn icon_variant(base: &Image<'_>, v: IconVariant) -> Image<'static> {
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    match v {
        IconVariant::Normal => {}
        IconVariant::Offline => {
            for px in rgba.chunks_mut(4) {
                px[3] = (px[3] as f32 * 0.4) as u8;
            }
        }
        IconVariant::Warning => {
            let s = w.min(h) as f32;
            let (cx, top, side) = (w as f32 - s * 0.22, h as f32 - s * 0.44, s * 0.42);
            let tri = |x: f32, y: f32, grow: f32| {
                let hgt = side * 0.9 + grow * 2.0;
                let t = top - grow;
                y >= t && y <= t + hgt && (x - cx).abs() <= (y - t) / hgt * (side / 2.0 + grow)
            };
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let inner = coverage(|a, b| tri(a, b, 0.0), x, y);
                    let ring = coverage(|a, b| tri(a, b, s * 0.06), x, y);
                    // Clear a margin round the mark so it reads against the glyph, then draw it.
                    let alpha = (rgba[i + 3] as f32 * (1.0 - ring)).max(inner * 255.0);
                    rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                    rgba[i + 3] = alpha.round() as u8;
                }
            }
        }
    }
    Image::new_owned(rgba, w, h)
}

// ---- The menu

struct NodeItems {
    menu: Submenu<Wry>,
    ip: MenuItem<Wry>,
    dot: Option<Dot>,
}

struct Items {
    node_ids: Vec<String>,
    header: MenuItem<Wry>,
    nodes: Vec<NodeItems>,
    models: MenuItem<Wry>,
    vpn: MenuItem<Wry>,
    vpn_toggle: MenuItem<Wry>,
    server: MenuItem<Wry>,
    server_toggle: MenuItem<Wry>,
    demo: CheckMenuItem<Wry>,
    live: CheckMenuItem<Wry>,
    admin: MenuItem<Wry>,
    alerts: MenuItem<Wry>,
}

pub struct Tray {
    icon: TrayIcon,
    base: Image<'static>,
    variant: Mutex<IconVariant>,
    items: Mutex<Option<Items>>,
    icons: Mutex<HashMap<Dot, Image<'static>>>,
}

/// Brings the window back (and the Dock icon), optionally opening a window inside it.
pub fn show_main(app: &AppHandle, open: Option<(&str, serde_json::Value)>) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some((kind, params)) = open {
        let _ = app.emit("tray://open", json!({ "kind": kind, "params": params }));
    }
}

/// Hides the window and the Dock icon; SparkBox lives on in the menu bar.
pub fn hide_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

/// Asks the window to do something with a node that needs the UI (a terminal, a confirmation).
fn node_action(app: &AppHandle, node: &str, action: &str) {
    show_main(app, None);
    let _ = app.emit("tray://node-action", json!({ "node": node, "action": action }));
}

/// The node's LAN address, as the inventory has it.
fn node_ip(app: &AppHandle, id: &str) -> Option<String> {
    let state = app.state::<crate::AppState>();
    let inv = state.inventory.lock().unwrap();
    inv.nodes.iter().find(|n| n.id == id).map(|n| n.ip.clone().filter(|ip| !ip.is_empty()).unwrap_or_else(|| n.host.clone()))
}

fn copy_text(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn set_data_source(app: &AppHandle, live: bool) {
    let source = if live { crate::prefs::DataSource::Live } else { crate::prefs::DataSource::Demo };
    let app = app.clone();
    tauri::async_runtime::spawn(from_menu_bar("Switch data source", async move {
        let state = app.state::<crate::AppState>();
        if let Err(e) = crate::data_source_set(app.clone(), state, source) {
            refused(&app, if live { "switch to Live data" } else { "switch to Demo data" }, &e);
        }
    }));
}

/// The VPN toggle goes through the same gated, logged command as the VPN window.
fn toggle_vpn(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(from_menu_bar("Turn the VPN on or off", async move {
        let vpn = app.state::<crate::AppState>().vpn.clone();
        let connected = matches!(vpn.view().await.status.state, VpnState::Connected | VpnState::Connecting);
        let action = if connected { "disconnect" } else { "connect" };
        if let Err(e) = crate::vpn::commands::vpn_override(app.clone(), Some(action.into())).await {
            refused(&app, if connected { "disconnect the VPN" } else { "connect the VPN" }, &e);
        }
    }));
}

/// Runs a menu bar action as its own operation, so the activity log says it came from there.
pub(crate) fn from_menu_bar<F: std::future::Future>(title: &str, fut: F) -> impl std::future::Future<Output = F::Output> {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    crate::live::audit::with_op_id(format!("tray-{ms}"), crate::live::audit::OpMeta { title: title.into(), source: "tray".into() }, fut)
}

/// A menu bar action Admin mode refused: open the window (where it can be unlocked) and say why.
pub(crate) fn refused(app: &AppHandle, what: &str, error: &str) {
    show_main(app, None);
    let error = crate::errors::SbError::classify(error);
    let body = if matches!(error.kind, crate::errors::ErrorKind::Locked | crate::errors::ErrorKind::NeedsAuth) {
        format!("Unlock Admin mode in the top bar to {what}.")
    } else {
        error.message
    };
    crate::notify::post(app, &format!("Couldn't {what}"), &body, None);
}

impl Tray {
    pub fn build(app: &AppHandle, on_toggle_server: impl Fn(&AppHandle) + Send + Sync + 'static) -> tauri::Result<Tray> {
        let base = Image::from_bytes(include_bytes!("../icons/tray.png"))?.to_owned();
        let icon = TrayIconBuilder::with_id("sparkbox")
            .icon(base.clone())
            .icon_as_template(true)
            .tooltip("SparkBox")
            .show_menu_on_left_click(true)
            .on_menu_event(move |app, event| match event.id.as_ref() {
                "open" => show_main(app, None),
                "chat" => show_main(app, Some(("chat", json!({})))),
                "models" => show_main(app, Some(("models", json!({})))),
                "alerts" => show_main(app, Some(("alerts", json!({})))),
                "settings" => show_main(app, Some(("settings", json!({})))),
                "server-toggle" => on_toggle_server(app),
                "vpn-toggle" => toggle_vpn(app),
                "source-demo" => set_data_source(app, false),
                "source-live" => set_data_source(app, true),
                "admin" => {
                    if let Some(gate) = app.try_state::<Arc<crate::admin::AdminGate>>() {
                        if gate.state().unlocked {
                            crate::admin::lock_because(app, crate::admin::RelockReason::Explicit, "Locked from the menu bar");
                        } else {
                            show_main(app, None);
                        }
                    }
                }
                "quit" => app.exit(0),
                id => {
                    let Some((what, node)) = id.split_once(':') else { return };
                    match what {
                        "node" => show_main(app, Some(("node", json!({ "id": node })))),
                        "node-ip" => {
                            if let Some(ip) = node_ip(app, node) {
                                copy_text(&ip);
                            }
                        }
                        "node-term" => node_action(app, node, "terminal"),
                        "node-files" => node_action(app, node, "files"),
                        "node-dgx" => node_action(app, node, "dgx"),
                        "node-reboot" => node_action(app, node, "reboot"),
                        _ => {}
                    }
                }
            })
            .build(app)?;
        Ok(Tray { icon, base, variant: Mutex::new(IconVariant::Normal), items: Mutex::new(None), icons: Mutex::new(HashMap::new()) })
    }

    fn dot_image(&self, dot: Dot) -> Image<'static> {
        self.icons.lock().unwrap().entry(dot).or_insert_with(|| dot_icon(dot)).clone()
    }

    /// Refreshes the menu; rebuilds it only when the set of nodes changes.
    pub fn update(&self, app: &AppHandle, snap: &ClusterSnapshot, deployments: &[Deployment], vpn: &VpnStatus, server: &ServerView) -> tauri::Result<()> {
        let health = app.try_state::<Arc<Health>>().map(|h| h.view());
        let ids: Vec<String> = snap.nodes.iter().map(|n| n.id.clone()).collect();
        let mut guard = self.items.lock().unwrap();
        if guard.as_ref().is_none_or(|i| i.node_ids != ids) {
            *guard = Some(self.rebuild(app, snap)?);
        }
        let items = guard.as_mut().unwrap();

        let source = if snap.demo { "demo data" } else { "live" };
        items.header.set_text(format!("SparkBox — {source}"))?;
        for (item, node) in items.nodes.iter_mut().zip(&snap.nodes) {
            item.menu.set_text(node_line(node))?;
            let dot = dot_for(node, health.as_ref().and_then(|h| h.nodes.get(&node.id)).map(|e| e.level));
            if item.dot != Some(dot) {
                item.menu.set_icon(Some(self.dot_image(dot)))?;
                item.dot = Some(dot);
            }
            item.ip.set_text(match node_ip(app, &node.id) {
                Some(ip) => format!("Copy IP ({ip})"),
                None => "Copy IP".into(),
            })?;
        }
        items.models.set_text(models_line(deployments))?;
        items.vpn.set_text(vpn_line(vpn))?;
        let vpn_up = matches!(vpn.state, VpnState::Connected | VpnState::Connecting);
        items.vpn_toggle.set_text(if vpn_up { "Disconnect VPN" } else { "Connect VPN" })?;
        match &server.url {
            Some(url) if server.running => items.server.set_text(format!("Server: running at {}", url.trim_start_matches("http://")))?,
            _ => items.server.set_text("Server: stopped")?,
        }
        items.server_toggle.set_text(if server.running { "Stop Server" } else { "Start Server" })?;
        items.demo.set_checked(snap.demo)?;
        items.live.set_checked(!snap.demo)?;
        let admin = app.try_state::<Arc<crate::admin::AdminGate>>().map(|g| g.state());
        match admin {
            Some(a) if a.unlocked => {
                let left = a.expires_at_ms.map(|t| t.saturating_sub(crate::history::now_ms()) / 60_000);
                items.admin.set_text(match left {
                    Some(m) => format!("Lock Admin Mode ({} min left)", m.max(1)),
                    None => "Lock Admin Mode".into(),
                })?;
            }
            _ => items.admin.set_text("Admin Mode: locked — unlock in SparkBox…")?,
        }
        let (warn, crit) = health.as_ref().map(|h| (h.summary.warn, h.summary.crit)).unwrap_or((0, 0));
        items.alerts.set_text(match (crit, warn) {
            (0, 0) => "Alerts: all healthy".to_string(),
            (0, w) => format!("Alerts: {w} warning{}…", if w == 1 { "" } else { "s" }),
            (c, 0) => format!("Alerts: {c} critical…"),
            (c, w) => format!("Alerts: {c} critical, {w} warning{}…", if w == 1 { "" } else { "s" }),
        })?;

        let online = snap.nodes.iter().filter(|n| n.status == LinkStatus::Online).count();
        let variant = if !snap.nodes.is_empty() && online == 0 {
            IconVariant::Offline
        } else if crit + warn > 0 || online < snap.nodes.len() {
            IconVariant::Warning
        } else {
            IconVariant::Normal
        };
        let mut current = self.variant.lock().unwrap();
        if *current != variant {
            self.icon.set_icon(Some(icon_variant(&self.base, variant)))?;
            self.icon.set_icon_as_template(true)?;
            *current = variant;
        }
        self.icon.set_title(Some(title(snap, health.as_ref())))?;
        Ok(())
    }

    fn rebuild(&self, app: &AppHandle, snap: &ClusterSnapshot) -> tauri::Result<Items> {
        let label = |id: &str, text: &str, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
        let sep = || PredefinedMenuItem::separator(app);
        let header = label("header", "SparkBox", false)?;
        let mut nodes = Vec::new();
        for n in &snap.nodes {
            let id = &n.id;
            let ip = label(&format!("node-ip:{id}"), "Copy IP", true)?;
            let open = label(&format!("node:{id}"), &format!("Open {}", n.name), true)?;
            let term = label(&format!("node-term:{id}"), "Terminal", true)?;
            let files = label(&format!("node-files:{id}"), "Files", true)?;
            let dgx = label(&format!("node-dgx:{id}"), "DGX Dashboard", true)?;
            let reboot = label(&format!("node-reboot:{id}"), "Reboot…", true)?;
            let s1 = sep()?;
            let menu = Submenu::with_id_and_items(app, format!("node-menu:{id}"), &n.name, true, &[&open, &term, &files, &dgx, &ip, &s1, &reboot])?;
            nodes.push(NodeItems { menu, ip, dot: None });
        }
        let models = label("models", "Models", true)?;
        let vpn = label("vpn", "VPN", false)?;
        let vpn_toggle = label("vpn-toggle", "Connect VPN", true)?;
        let server = label("server", "Server", false)?;
        let server_toggle = label("server-toggle", "Start Server", true)?;
        let demo = CheckMenuItem::with_id(app, "source-demo", "Demo", true, snap.demo, None::<&str>)?;
        let live = CheckMenuItem::with_id(app, "source-live", "Live", true, !snap.demo, None::<&str>)?;
        let source = Submenu::with_items(app, "Data Source", true, &[&demo, &live])?;
        let admin = label("admin", "Admin Mode", true)?;
        let alerts = label("alerts", "Alerts", true)?;
        let open = label("open", "Open SparkBox", true)?;
        let chat = label("chat", "Chat…", true)?;
        let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
        let about = PredefinedMenuItem::about(app, Some("About SparkBox"), None)?;
        let quit = MenuItem::with_id(app, "quit", "Quit SparkBox", true, Some("CmdOrCtrl+Q"))?;
        let seps = (0..5).map(|_| sep()).collect::<tauri::Result<Vec<_>>>()?;

        let mut entries: Vec<&dyn IsMenuItem<Wry>> = vec![&header, &alerts, &seps[0]];
        entries.extend(nodes.iter().map(|n| &n.menu as &dyn IsMenuItem<Wry>));
        entries.extend([
            &seps[1] as &dyn IsMenuItem<Wry>,
            &models,
            &vpn,
            &vpn_toggle,
            &server,
            &server_toggle,
            &seps[2],
            &source,
            &admin,
            &seps[3],
            &open,
            &chat,
            &settings,
            &seps[4],
            &about,
            &quit,
        ]);
        let menu = Menu::with_items(app, &entries)?;
        self.icon.set_menu(Some(menu))?;
        Ok(Items { node_ids: snap.nodes.iter().map(|n| n.id.clone()).collect(), header, nodes, models, vpn, vpn_toggle, server, server_toggle, demo, live, admin, alerts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoCluster;
    use crate::inventory::Inventory;

    #[test]
    fn menu_lines_show_what_each_node_serves_without_emoji() {
        let mut demo = DemoCluster::new(&Inventory::example());
        let mut snap = demo.snapshot();
        let line = node_line(&snap.nodes[0]);
        assert!(line.starts_with("cell1 — vllm-head · GPU "), "{line}");
        assert!(line.contains("GiB") && line.ends_with(" W"), "{line}");
        assert!(node_line(&snap.nodes[3]).contains("ollama"));

        snap.nodes[1].status = LinkStatus::Error;
        snap.nodes[1].error = Some("The password for sparkuser@192.168.50.154 was rejected. Check it in Settings.".into());
        assert_eq!(node_line(&snap.nodes[1]), "cell2 — login failed");
        snap.nodes[1].error = Some("SparkBox hasn't connected to 192.168.50.154:22 before. Its host key is SHA256:x.".into());
        snap.nodes[1].error_kind = Some("hostKeyUnknown".into());
        assert_eq!(node_line(&snap.nodes[1]), "cell2 — host key not trusted yet");
        snap.nodes[1].error_kind = Some("hostKeyChanged".into());
        assert_eq!(node_line(&snap.nodes[1]), "cell2 — host key changed");
        snap.nodes[1].error_kind = None;
        snap.nodes[1].error = Some("weird failure".into());
        assert_eq!(node_line(&snap.nodes[1]), "cell2 — weird failure");
        assert!(title(&snap, None).starts_with("3/4 · "));

        snap.nodes[2].containers.clear();
        snap.nodes[2].gpu.procs.clear();
        assert!(node_line(&snap.nodes[2]).starts_with("cell3 — no model loaded"));
        for n in &snap.nodes {
            assert!(node_line(n).chars().all(|c| (c as u32) < 0x2600 || "—…·".contains(c)), "no emoji or pictographs: {}", node_line(n));
        }
    }

    #[test]
    fn an_unreadable_gpu_reads_as_a_dash_and_stays_out_of_the_average() {
        let mut demo = DemoCluster::new(&Inventory::example());
        let mut snap = demo.snapshot();
        for (i, n) in snap.nodes.iter_mut().enumerate() {
            n.status = LinkStatus::Online;
            n.stale = false;
            n.gpu.util_pct = if i == 0 { 90.0 } else { 10.0 };
            n.gpu.readable = true;
        }
        snap.nodes[0].gpu.readable = false;
        snap.nodes[0].gpu.util_pct = 0.0;
        assert!(node_line(&snap.nodes[0]).contains("GPU —"), "{}", node_line(&snap.nodes[0]));
        assert!(title(&snap, None).starts_with("4/4 · 10%"), "{}", title(&snap, None));
        for n in &mut snap.nodes {
            n.gpu.readable = false;
        }
        assert!(title(&snap, None).starts_with("4/4 · GPU —"), "{}", title(&snap, None));
    }

    #[test]
    fn dots_follow_link_state_then_health() {
        let mut demo = DemoCluster::new(&Inventory::example());
        let mut snap = demo.snapshot();
        assert_eq!(dot_for(&snap.nodes[0], None), Dot::Online);
        assert_eq!(dot_for(&snap.nodes[0], Some(Level::Warn)), Dot::Warn);
        assert_eq!(dot_for(&snap.nodes[0], Some(Level::Crit)), Dot::Crit);
        snap.nodes[0].stale = true;
        assert_eq!(dot_for(&snap.nodes[0], Some(Level::Crit)), Dot::Stale);
        snap.nodes[1].status = LinkStatus::Offline;
        assert_eq!(dot_for(&snap.nodes[1], None), Dot::Offline);
    }

    #[test]
    fn drawn_icons_have_distinct_shapes() {
        let filled = |d: Dot| dot_icon(d).rgba().chunks(4).filter(|p| p[3] > 128).count();
        assert!(filled(Dot::Online) > filled(Dot::Offline), "a ring has less ink than a disc");
        assert_ne!(filled(Dot::Warn), filled(Dot::Crit));
        let base = Image::new_owned([0u8, 0, 0, 255].repeat(44 * 44), 44, 44);
        let faded = icon_variant(&base, IconVariant::Offline);
        assert_eq!(faded.rgba()[3], 102);
        let warned = icon_variant(&base, IconVariant::Warning);
        assert_eq!(warned.width(), 44);
        assert!(warned.rgba().chunks(4).any(|p| p[3] < 255), "the mark clears a margin round itself");
    }

    #[test]
    fn the_title_counts_alerts() {
        let mut demo = DemoCluster::new(&Inventory::example());
        let snap = demo.snapshot();
        let mut view = HealthView::default();
        view.summary.warn = 2;
        assert!(title(&snap, Some(&view)).ends_with(" · 2 warnings"));
        view.summary.crit = 1;
        assert!(title(&snap, Some(&view)).ends_with(" · 1 critical"));
    }

    #[test]
    fn the_models_line_shows_each_load_and_its_stage() {
        assert_eq!(models_line(&[]), "Models: none loaded by SparkBox");
        let d: Deployment = serde_json::from_value(json!({
            "spec": { "id": "q", "servedName": "qwen3.8-27b", "modelPath": "/m", "nodes": ["cell1", "cell2"], "tensorParallel": 2,
                      "pipelineParallel": 1, "expertParallel": false, "maxModelLen": null, "gpuMemoryUtilization": 0.8,
                      "port": 8000, "image": "i", "extraArgs": [] },
            "endpoint": "http://x/v1", "state": "starting", "stage": { "kind": "loading", "percent": 40 }, "message": "",
            "logTail": "", "startedAt": 0, "providerId": "", "demo": true
        }))
        .unwrap();
        let mut compiling = d.clone();
        compiling.stage = Some(Stage::Compiling { what: "FlashInfer kernels".into() });
        assert_eq!(models_line(&[compiling]), "Models: qwen3.8-27b on cell1+cell2 compiling");
        let mut stopped = d.clone();
        stopped.state = "stopped".into();
        assert_eq!(models_line(&[d, stopped]), "Models: qwen3.8-27b on cell1+cell2 loading 40%");
    }
}
