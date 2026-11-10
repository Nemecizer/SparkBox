//! Local port forwards over a node's SSH connection, so a web page that only listens on the
//! node itself (the DGX Dashboard on 127.0.0.1:11000) opens in the Mac's browser at
//! http://127.0.0.1:<local port>.
//!
//! A tunnel opens only to a port the node itself says it listens on (its latest collector
//! frame), never SSH's or DNS's, and goes to the address the port is bound to there. A page on
//! the node can do what a shell there does (JupyterLab is a shell, a Ray job runs code, Ollama
//! pulls and deletes models), so the write exit judges each forward a `shell` change
//! (`as_class`): the first one when the tunnel opens and one for every connection through it.
//! Look-only holds them, and the test build's fence must name the node and allow shells. Any
//! program on the Mac can reach a local port, so each
//! tunnel checks every request: the Host must be 127.0.0.1:<port> or localhost:<port> (no DNS
//! rebinding from a web page), and it must carry the tunnel's random token — in the URL
//! SparkBox opens (`?sbt=…`, swapped for a cookie on the first load) or in that cookie. The
//! cookie is named after the local port (`sbt_<port>`): browsers share cookies between the
//! ports of one host, so two tunnels open at once each keep their own. It's taken out of the
//! request before it goes to the node, and each request gets a connection of its own
//! (`Connection: close`), so every one is checked. Tunnels close after 15 minutes without
//! use, when Admin mode relocks (for any reason) and when SparkBox quits, connections open
//! through them (a WebSocket) included; Settings › Security & privacy lists the open ones with
//! Close.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::Live;
use crate::model::Listener;

pub const DGX_DASHBOARD: u16 = 11000;
const IDLE: Duration = Duration::from_secs(15 * 60);
/// The most a request's head may be before the tunnel gives up on it.
const HEAD_LIMIT: usize = 16 * 1024;
/// Ports that are never a web page: SSH and DNS.
const NOT_PAGES: [u16; 2] = [22, 53];
/// The class the write exit judges every forward as (a page on the node is a shell's worth of access).
const CLASS: FenceClass = FenceClass::Shell;

/// The pages SparkBox knows by name, by port on the node.
pub fn known_port(port: u16) -> Option<&'static str> {
    match port {
        DGX_DASHBOARD => Some("DGX Dashboard"),
        _ => None,
    }
}

/// What a tunnel's page is called: "DGX Dashboard", or "page on port 8000".
pub fn page_label(port: u16) -> String {
    known_port(port).map(String::from).unwrap_or_else(|| format!("page on port {port}"))
}

/// Where on the node a tunnel to `port` goes, from the ports the node says it listens on: the
/// node itself (127.0.0.1, or ::1 for an IPv6-only socket) for a port bound to every address
/// or to loopback, else the one address it's bound to. Refused for a port it doesn't listen
/// on, and for SSH and DNS.
pub fn target(node: &str, port: u16, listeners: Option<&[Listener]>) -> Result<String, String> {
    if NOT_PAGES.contains(&port) {
        return Err(format!("Port {port} on {node} is {}, not a web page.", if port == 22 { "SSH" } else { "DNS" }));
    }
    let Some(listeners) = listeners else {
        return Err(format!("SparkBox hasn't had {node}'s listening ports yet; try again once it shows data."));
    };
    let Some(l) = listeners.iter().find(|l| l.port == port) else {
        return Err(format!("{node} isn't listening on port {port}, so there's no page to open."));
    };
    let v6_only = l.family.as_deref() == Some("v6") || (l.family.is_none() && l.addr.contains(':'));
    Ok(match l.addr.as_str() {
        "0.0.0.0" | "*" | "" => "127.0.0.1".into(),
        "::" if v6_only => "::1".into(),
        "::" => "127.0.0.1".into(),
        a => a.trim_start_matches('[').trim_end_matches(']').to_string(),
    })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct Opened {
    pub local_port: u16,
    /// Where on the node it goes: 127.0.0.1, ::1, or the one address the port is bound to.
    pub host: String,
    /// The fence class it was let through as ("shell"), for its "Opened …" row.
    pub fence_class: &'static str,
    token: String,
}

impl Opened {
    /// The address to open: it carries the token once; the page then keeps it as a cookie.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?sbt={}", self.local_port, self.token)
    }
}

struct Tunnel {
    local: u16,
    token: String,
    task: tauri::async_runtime::JoinHandle<()>,
    use_: Arc<Usage>,
    opened_ms: u64,
}

/// An open tunnel, as Settings › Security & privacy lists it.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub node: String,
    pub port: u16,
    /// "DGX Dashboard"
    pub page: String,
    pub local_port: u16,
    pub opened_ms: u64,
    pub last_used_ms: u64,
    /// Connections open through it right now.
    pub active: usize,
}

#[derive(Default)]
struct Usage {
    last_ms: AtomicU64,
    active: AtomicUsize,
}

pub struct Tunnels {
    live: Arc<Live>,
    /// (node, remote port) -> tunnel
    open: Mutex<HashMap<(String, u16), Tunnel>>,
    /// How many times every tunnel was closed (`close_all`), changed with `open` locked: a
    /// tunnel still opening when they were doesn't outlive it.
    closes: AtomicU64,
}

/// What an open that was still under way when every tunnel closed says.
const CLOSED_WHILE_OPENING: &str = "SparkBox closed its tunnels (Admin mode relocked) while this one was opening. Open it again.";

impl Tunnels {
    pub fn new(live: Arc<Live>) -> Arc<Tunnels> {
        Arc::new(Tunnels { live, open: Mutex::new(HashMap::new()), closes: AtomicU64::new(0) })
    }

    /// Forwards a local port to `port` on the node (reusing an existing forward), once the node's
    /// own listener list shows it listens there (`target`) and the write exit lets a shell's
    /// worth of access through. Two opens of one page at once (a double-click) get one tunnel.
    pub async fn open(&self, node: &str, port: u16) -> Result<Opened, String> {
        let host = target(node, port, self.live.listeners(node).as_deref())?;
        let k = (node.to_string(), port);
        if let Some(opened) = self.reuse(&self.open.lock().unwrap(), &k, &host) {
            return Ok(opened);
        }
        let closes = self.closes.load(Ordering::SeqCst);
        // Fail now, not on the first browser request, when the node isn't connected, nothing
        // answers on the port, or the exit refuses it.
        as_class(CLASS, self.live.direct_tcpip(node, &host, port, 0)).await.map(drop)?;
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
        let local = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = new_token();
        let use_ = Arc::new(Usage { last_ms: AtomicU64::new(now_ms()), active: AtomicUsize::new(0) });
        let target_host = host.clone();
        let (live, node_id, key, u) = (self.live.clone(), node.to_string(), token.clone(), use_.clone());
        let task = tauri::async_runtime::spawn(async move {
            // The connections it serves live in this set: when the tunnel closes (this task is
            // aborted) they end with it, a WebSocket's too.
            let mut conns = JoinSet::new();
            loop {
                let sock = tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((sock, _)) => sock,
                        Err(_) => break,
                    },
                    Some(_) = conns.join_next() => continue,
                };
                let (live, node_id, key, u, host) = (live.clone(), node_id.clone(), key.clone(), u.clone(), host.clone());
                conns.spawn(async move {
                    u.active.fetch_add(1, Ordering::SeqCst);
                    u.last_ms.store(now_ms(), Ordering::SeqCst);
                    let _ = serve(sock, &live, &node_id, &host, port, local, &key).await;
                    u.last_ms.store(now_ms(), Ordering::SeqCst);
                    u.active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        let mut open = self.open.lock().unwrap();
        if self.closes.load(Ordering::SeqCst) != closes {
            task.abort();
            return Err(CLOSED_WHILE_OPENING.into());
        }
        // Another open of the same page finished first: keep that one.
        if let Some(opened) = self.reuse(&open, &k, &target_host) {
            task.abort();
            return Ok(opened);
        }
        let opened = Opened { local_port: local, host: target_host, fence_class: CLASS.id(), token: token.clone() };
        if let Some(old) = open.insert(k, Tunnel { local, token, task, use_, opened_ms: now_ms() }) {
            old.task.abort();
        }
        Ok(opened)
    }

    /// The tunnel already open to `k`, as `open` returns it (marked used), if it's still serving.
    fn reuse(&self, open: &HashMap<(String, u16), Tunnel>, k: &(String, u16), host: &str) -> Option<Opened> {
        let t = open.get(k).filter(|t| !t.task.inner().is_finished())?;
        t.use_.last_ms.store(now_ms(), Ordering::SeqCst);
        Some(Opened { local_port: t.local, host: host.to_string(), fence_class: CLASS.id(), token: t.token.clone() })
    }

    /// The open tunnels, oldest first.
    pub fn list(&self) -> Vec<TunnelView> {
        let mut out: Vec<TunnelView> = self
            .open
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| !t.task.inner().is_finished())
            .map(|((node, port), t)| TunnelView {
                node: node.clone(),
                port: *port,
                page: known_port(*port).map(String::from).unwrap_or_else(|| format!("Port {port}")),
                local_port: t.local,
                opened_ms: t.opened_ms,
                last_used_ms: t.use_.last_ms.load(Ordering::SeqCst),
                active: t.use_.active.load(Ordering::SeqCst),
            })
            .collect();
        out.sort_by_key(|t| t.opened_ms);
        out
    }

    /// Closes the tunnel to one page (Close in Settings › Security & privacy); true when one was open.
    pub fn close(&self, node: &str, port: u16) -> bool {
        match self.open.lock().unwrap().remove(&(node.to_string(), port)) {
            Some(t) => {
                t.task.abort();
                true
            }
            None => false,
        }
    }

    /// Closes every tunnel (Admin mode relocked, SparkBox quitting), and any still opening.
    pub fn close_all(&self) {
        let mut open = self.open.lock().unwrap();
        self.closes.fetch_add(1, Ordering::SeqCst);
        for (_, t) in open.drain() {
            t.task.abort();
        }
    }

    /// Closes the tunnels nobody has used for `idle` and that have no connection open; returns
    /// how many.
    fn reap(&self, now_ms: u64, idle: Duration) -> usize {
        let mut open = self.open.lock().unwrap();
        let stale: Vec<(String, u16)> = open
            .iter()
            .filter(|(_, t)| t.use_.active.load(Ordering::SeqCst) == 0 && now_ms.saturating_sub(t.use_.last_ms.load(Ordering::SeqCst)) >= idle.as_millis() as u64)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            if let Some(t) = open.remove(k) {
                t.task.abort();
            }
        }
        stale.len()
    }

    /// Checks every 30 seconds for tunnels left idle for 15 minutes. Spawned once at start.
    pub fn start_idle_reaper(self: &Arc<Self>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(30));
            loop {
                every.tick().await;
                me.reap(now_ms(), IDLE);
            }
        });
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

fn new_token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the system's random source");
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// The cookie a tunnel keeps its token in: one per local port, since browsers share cookies
/// between the ports of a host.
fn cookie_name(local: u16) -> String {
    format!("sbt_{local}")
}

/// What to do with a connection, from its first request's head.
#[derive(Debug, PartialEq)]
enum Check {
    /// Forward it to the node (the head as rewritten by `forwarded_head`).
    Pass,
    /// First load with `?sbt=`: set the cookie and send the browser to the same page without it.
    SetCookie(String),
    Reject(&'static str),
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn check(head: &str, local: u16, token: &str) -> Check {
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or("");
    let target = request.split(' ').nth(1).unwrap_or("/");
    let header = |name: &str| {
        head.split("\r\n").skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
    };
    let host = header("host").unwrap_or_default();
    if host != format!("127.0.0.1:{local}") && host != format!("localhost:{local}") {
        return Check::Reject("This page only opens from SparkBox (wrong Host).");
    }
    let name = format!("{}=", cookie_name(local));
    let cookie_ok = header("cookie").is_some_and(|c| c.split(';').any(|kv| kv.trim().strip_prefix(name.as_str()).is_some_and(|v| same(v, token))));
    if cookie_ok {
        return Check::Pass;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut rest = Vec::new();
    let mut found = false;
    for kv in query.split('&').filter(|kv| !kv.is_empty()) {
        match kv.strip_prefix("sbt=") {
            Some(v) if same(v, token) => found = true,
            _ => rest.push(kv),
        }
    }
    if found {
        let location = if rest.is_empty() { path.to_string() } else { format!("{path}?{}", rest.join("&")) };
        return Check::SetCookie(location);
    }
    Check::Reject("Open this page from SparkBox: its address carries a one-time key.")
}

/// The request head as the node gets it: without any tunnel's cookie (the browser sends every
/// tunnel's to every port; other cookies stay, and a Cookie header left empty goes) and, unless
/// it's a WebSocket upgrade, with `Connection: close`, so the browser's next request comes on a
/// new, checked connection.
fn forwarded_head(head: &str) -> String {
    let upgrade = head.split("\r\n").any(|l| l.split_once(':').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("upgrade")));
    let mut out: Vec<String> = Vec::new();
    for (i, line) in head.split("\r\n").enumerate() {
        let key = line.split_once(':').map(|(k, _)| k.trim().to_ascii_lowercase());
        match key.as_deref() {
            Some("cookie") if i > 0 => {
                let (_, value) = line.split_once(':').unwrap_or_default();
                let kept: Vec<&str> = value.split(';').map(str::trim).filter(|kv| !kv.is_empty() && !kv.starts_with("sbt_") && !kv.starts_with("sbt=")).collect();
                if !kept.is_empty() {
                    out.push(format!("Cookie: {}", kept.join("; ")));
                }
            }
            Some("connection") | Some("keep-alive") if i > 0 && !upgrade => {}
            _ => out.push(line.to_string()),
        }
    }
    if !upgrade {
        out.push("Connection: close".into());
    }
    out.join("\r\n")
}

/// Reads up to the end of the request head; None if the client closes or sends too much.
async fn read_head(sock: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(buf);
        }
        if buf.len() > HEAD_LIMIT {
            return None;
        }
    }
}

async fn serve(mut sock: TcpStream, live: &Live, node: &str, host: &str, port: u16, local: u16, token: &str) -> Result<(), String> {
    let Some(head) = read_head(&mut sock).await else { return Ok(()) };
    let end = head.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(head.len());
    let text = String::from_utf8_lossy(&head[..end]).into_owned();
    let reply = |status: &str, extra: &str, body: &str| format!("HTTP/1.1 {status}\r\n{extra}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    match check(&text, local, token) {
        Check::Reject(why) => {
            let _ = sock.write_all(reply("403 Forbidden", "", why).as_bytes()).await;
        }
        Check::SetCookie(location) => {
            let cookie = format!("Set-Cookie: {}={token}; Path=/; HttpOnly; SameSite=Strict\r\nLocation: {location}\r\nCache-Control: no-store\r\n", cookie_name(local));
            let _ = sock.write_all(reply("302 Found", &cookie, "").as_bytes()).await;
        }
        Check::Pass => {
            // Each connection is asked for again: a fence that closed or ran out ends the page.
            let mut stream = match as_class(CLASS, live.direct_tcpip(node, host, port, local)).await {
                Ok(s) => s,
                Err(e) => {
                    let why = crate::errors::SbError::classify(&e).message;
                    let _ = sock.write_all(reply("502 Bad Gateway", "", &why).as_bytes()).await;
                    return Err(e);
                }
            };
            // The head without the token, then whatever followed it (a request body).
            let mut sent = forwarded_head(&text).into_bytes();
            sent.extend_from_slice(&head[end..]);
            stream.write_all(&sent).await.map_err(|e| e.to_string())?;
            let _ = tokio::io::copy_bidirectional(&mut sock, &mut stream).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{connect, fake_node, real_shell, temp};

    async fn get(local: u16, host: &str, path: &str, cookie: Option<&str>) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", local)).await.unwrap();
        let cookie = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
        c.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: keep-alive\r\n{cookie}\r\n").as_bytes()).await.unwrap();
        let mut body = String::new();
        c.read_to_string(&mut body).await.unwrap();
        body
    }

    /// "A DGX Dashboard": answers with its name and the request head it was given.
    async fn page(name: &'static str) -> u16 {
        let page = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = page.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = page.accept().await {
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = s.write_all(format!("HTTP/1.0 200 OK\r\n\r\n{name}\n{head}").as_bytes()).await;
            }
        });
        port
    }

    /// The node's collector frame says it listens on these (port, address).
    fn listening(node: &crate::fake_node::Node, ports: &[(u16, &str)]) {
        let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
        frame["listeners"] = ports.iter().map(|(p, a)| serde_json::json!({ "port": p, "addr": a, "pid": null, "process": null })).collect();
        *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
    }

    #[tokio::test]
    async fn a_node_only_web_page_opens_through_the_ssh_connection_with_its_key() {
        let page_port = page("DGX Dashboard").await;
        let (node, port) = fake_node(&temp("tunnel-home"), real_shell()).await;
        listening(&node, &[(page_port, "127.0.0.1")]);
        let (live, _) = connect(&temp("tunnel-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());

        let opened = tunnels.open("cell1", page_port).await.unwrap();
        let local = opened.local_port;
        let token = opened.token.clone();
        assert!(opened.url().starts_with(&format!("http://127.0.0.1:{local}/?sbt=")));
        assert_eq!(tunnels.open("cell1", page_port).await.unwrap().local_port, local, "reused");

        // First load: the key in the address becomes a cookie named after the port, and the browser comes back.
        let first = get(local, &format!("127.0.0.1:{local}"), &format!("/?sbt={token}&tab=gpu"), None).await;
        assert!(first.starts_with("HTTP/1.1 302"), "{first}");
        assert!(first.contains(&format!("Set-Cookie: sbt_{local}={token}; Path=/; HttpOnly; SameSite=Strict")), "{first}");
        assert!(first.contains("Location: /?tab=gpu\r\n"));
        for host in [format!("127.0.0.1:{local}"), format!("localhost:{local}")] {
            let body = get(local, &host, "/", Some(&format!("theme=dark; sbt_{local}={token}"))).await;
            assert!(body.contains("DGX Dashboard"), "{body}");
            // The node's page never sees the key, and each request gets its own checked connection.
            assert!(!body.contains(&token) && !body.contains("sbt_"), "{body}");
            assert!(body.contains("Cookie: theme=dark\r\n") && body.contains("Connection: close\r\n") && !body.contains("keep-alive"), "{body}");
        }
        assert!(node.log.lock().unwrap().iter().any(|c| c == &format!("direct-tcpip 127.0.0.1:{page_port}")));

        // Another program on the Mac, or a web page rebinding its own name to 127.0.0.1, gets nothing.
        assert!(get(local, &format!("127.0.0.1:{local}"), "/", None).await.starts_with("HTTP/1.1 403"));
        assert!(get(local, &format!("127.0.0.1:{local}"), "/?sbt=0000", None).await.starts_with("HTTP/1.1 403"));
        assert!(get(local, &format!("127.0.0.1:{local}"), "/", Some(&format!("sbt_{local}=wrong"))).await.starts_with("HTTP/1.1 403"));
        assert!(get(local, &format!("127.0.0.1:{local}"), "/", Some(&format!("sbt={token}"))).await.starts_with("HTTP/1.1 403"), "the old shared name isn't accepted");
        assert!(get(local, &format!("evil.example:{local}"), "/", Some(&format!("sbt_{local}={token}"))).await.starts_with("HTTP/1.1 403"));

        assert_eq!(tunnels.open("cell9", 11000).await.err().unwrap(), "SparkBox hasn't had cell9's listening ports yet; try again once it shows data.");

        // Listed for Settings › Security & privacy.
        let listed = tunnels.list();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].node.as_str(), listed[0].port, listed[0].local_port, listed[0].page.as_str()), ("cell1", page_port, local, format!("Port {page_port}").as_str()));

        // Idle tunnels close; used ones stay.
        assert_eq!(tunnels.reap(now_ms(), IDLE), 0);
        assert_eq!(tunnels.reap(now_ms() + IDLE.as_millis() as u64, IDLE), 1);
        assert_eq!(tunnels.count(), 0);

        // Relock and quit close everything; Close in Settings closes one.
        tunnels.open("cell1", page_port).await.unwrap();
        assert!(tunnels.close("cell1", page_port));
        assert!(!tunnels.close("cell1", page_port));
        assert_eq!(tunnels.count(), 0);
        tunnels.open("cell1", page_port).await.unwrap();
        tunnels.close_all();
        assert_eq!(tunnels.count(), 0);
        live.stop();
    }

    /// Two dashboards open at once: the browser sends both cookies to both ports (cookies are
    /// per host), and each tunnel takes its own and passes the page nothing.
    #[tokio::test]
    async fn two_tunnels_keep_their_own_keys_and_both_pages_load() {
        let (one, two) = (page("cell1's dashboard").await, page("cell2's dashboard").await);
        let (n1, p1) = fake_node(&temp("tunnel2-a"), real_shell()).await;
        let (n2, p2) = fake_node(&temp("tunnel2-b"), real_shell()).await;
        listening(&n1, &[(one, "127.0.0.1")]);
        listening(&n2, &[(two, "0.0.0.0")]);
        let (live, _) = connect(&temp("tunnel2-app"), &[("cell1", p1), ("cell2", p2)]).await;
        let tunnels = Tunnels::new(live.clone());
        let a = tunnels.open("cell1", one).await.unwrap();
        let b = tunnels.open("cell2", two).await.unwrap();
        assert_ne!(a.local_port, b.local_port);
        // The browser's jar for 127.0.0.1 after both first loads: one cookie per tunnel.
        for o in [&a, &b] {
            let first = get(o.local_port, &format!("127.0.0.1:{}", o.local_port), &format!("/?sbt={}", o.token), None).await;
            assert!(first.contains(&format!("Set-Cookie: sbt_{}=", o.local_port)), "{first}");
        }
        let jar = format!("sbt_{}={}; sbt_{}={}", a.local_port, a.token, b.local_port, b.token);
        let page_a = get(a.local_port, &format!("127.0.0.1:{}", a.local_port), "/", Some(&jar)).await;
        let page_b = get(b.local_port, &format!("127.0.0.1:{}", b.local_port), "/", Some(&jar)).await;
        assert!(page_a.contains("cell1's dashboard"), "{page_a}");
        assert!(page_b.contains("cell2's dashboard"), "{page_b}");
        // Neither page sees either key.
        for p in [&page_a, &page_b] {
            assert!(!p.contains(&a.token) && !p.contains(&b.token) && !p.contains("sbt_"), "{p}");
        }
        assert_eq!(tunnels.list().len(), 2);
        live.stop();
    }

    /// A tunnel opens only to a port the node's own frame lists (P1.18): the dashboard on 11000,
    /// a model server on 8000. A port it doesn't listen on, SSH and DNS are refused before
    /// anything is forwarded.
    #[tokio::test]
    async fn a_tunnel_opens_only_to_a_port_the_node_listens_on() {
        let served = page("vLLM's API").await;
        let quiet = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (node, port) = fake_node(&temp("tunnel-ports"), real_shell()).await;
        listening(&node, &[(22, "0.0.0.0"), (53, "127.0.0.53"), (served, "0.0.0.0")]);
        let (live, _) = connect(&temp("tunnel-ports-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());

        // Listened on: forwarded, through the node's own SSH connection, to the node itself.
        let opened = tunnels.open("cell1", served).await.unwrap();
        assert_eq!(opened.host, "127.0.0.1");
        let first = get(opened.local_port, &format!("127.0.0.1:{}", opened.local_port), &format!("/?sbt={}", opened.token), None).await;
        assert!(first.starts_with("HTTP/1.1 302"), "{first}");
        let body = get(opened.local_port, &format!("127.0.0.1:{}", opened.local_port), "/", Some(&format!("sbt_{}={}", opened.local_port, opened.token))).await;
        assert!(body.contains("vLLM's API"), "{body}");
        assert!(node.ran("direct-tcpip").iter().all(|c| c == &format!("direct-tcpip 127.0.0.1:{served}")), "{:?}", node.ran("direct-tcpip"));

        // Not listened on, or not a web page: refused, and nothing is forwarded.
        let forwards = node.ran("direct-tcpip").len();
        assert_eq!(tunnels.open("cell1", quiet).await.err().unwrap(), format!("cell1 isn't listening on port {quiet}, so there's no page to open."));
        assert_eq!(tunnels.open("cell1", DGX_DASHBOARD).await.err().unwrap(), "cell1 isn't listening on port 11000, so there's no page to open.");
        assert_eq!(tunnels.open("cell1", 22).await.err().unwrap(), "Port 22 on cell1 is SSH, not a web page.");
        assert_eq!(tunnels.open("cell1", 53).await.err().unwrap(), "Port 53 on cell1 is DNS, not a web page.");
        assert_eq!(node.ran("direct-tcpip").len(), forwards, "no forward for a refused port");
        assert_eq!(tunnels.list().len(), 1);
        live.stop();
    }

    /// The verifier's double-click: two opens of the same page at once, then a relock
    /// (`close_all`). One tunnel, listed, and nothing still serving after the relock.
    #[tokio::test]
    async fn v_two_opens_at_once_then_relock() {
        let served = page("vLLM's API").await;
        let (node, port) = fake_node(&temp("v-tunnel-race"), real_shell()).await;
        listening(&node, &[(served, "0.0.0.0")]);
        let (live, _) = connect(&temp("v-tunnel-race-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());
        let mut orphaned = 0;
        let mut races = 0;
        for _ in 0..10 {
            let (a, b) = tokio::join!(tunnels.open("cell1", served), tunnels.open("cell1", served));
            let (a, b) = (a.unwrap(), b.unwrap());
            if a.local_port != b.local_port || a.token != b.token {
                races += 1;
            }
            assert_eq!(tunnels.list().len(), 1);
            tunnels.close_all();
            tokio::time::sleep(Duration::from_millis(150)).await;
            for o in [&a, &b] {
                if let Ok(mut c) = TcpStream::connect(("127.0.0.1", o.local_port)).await {
                    let _ = c.write_all(format!("GET /?sbt={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", o.token, o.local_port).as_bytes()).await;
                    let mut body = String::new();
                    let _ = tokio::time::timeout(Duration::from_secs(3), c.read_to_string(&mut body)).await;
                    if body.starts_with("HTTP/1.1 302") {
                        orphaned += 1;
                    }
                }
            }
        }
        assert_eq!((races, orphaned), (0, 0), "both opens got one tunnel, and the relock closed it");
        live.stop();
    }

    /// A relock while a tunnel is still opening: the open fails instead of leaving a tunnel the
    /// relock missed.
    #[tokio::test]
    async fn a_relock_while_a_tunnel_opens_leaves_no_tunnel() {
        let served = page("DGX Dashboard").await;
        let (node, port) = fake_node(&temp("tunnel-relock-opening"), real_shell()).await;
        listening(&node, &[(served, "127.0.0.1")]);
        let (live, _) = connect(&temp("tunnel-relock-opening-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());
        // The open runs first, up to its forward to the node; the relock comes while it waits.
        let (opened, ()) = tokio::join!(biased; tunnels.open("cell1", served), async { tunnels.close_all() });
        assert_eq!(opened.err().as_deref(), Some(CLOSED_WHILE_OPENING));
        assert_eq!((tunnels.count(), tunnels.list().len()), (0, 0));
        // The next open works.
        let o = tunnels.open("cell1", served).await.unwrap();
        let first = get(o.local_port, &format!("127.0.0.1:{}", o.local_port), &format!("/?sbt={}", o.token), None).await;
        assert!(first.starts_with("HTTP/1.1 302"), "{first}");
        live.stop();
    }

    /// A WebSocket (or any connection) open through a tunnel ends when the tunnel closes, so a
    /// relock leaves nothing reaching the node.
    #[tokio::test]
    async fn closing_a_tunnel_ends_the_connections_open_through_it() {
        // "A Jupyter terminal": switches to a WebSocket and keeps the connection open.
        let ws = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_port = ws.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = ws.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let _ = s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await;
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let (node, port) = fake_node(&temp("tunnel-ws"), real_shell()).await;
        listening(&node, &[(ws_port, "127.0.0.1")]);
        let (live, _) = connect(&temp("tunnel-ws-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());
        for close in ["relock", "Close in Settings"] {
            let o = tunnels.open("cell1", ws_port).await.unwrap();
            let mut c = TcpStream::connect(("127.0.0.1", o.local_port)).await.unwrap();
            let head = format!("GET /terminals/websocket/1 HTTP/1.1\r\nHost: 127.0.0.1:{0}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nCookie: sbt_{0}={1}\r\n\r\n", o.local_port, o.token);
            c.write_all(head.as_bytes()).await.unwrap();
            let mut buf = [0u8; 2048];
            let n = c.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&buf[..n]));
            c.write_all(b"ls\n").await.unwrap();
            let n = c.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"ls\n", "the shell answers through the tunnel");
            if close == "relock" {
                tunnels.close_all();
            } else {
                assert!(tunnels.close("cell1", ws_port));
            }
            // The connection ends: EOF or a reset, within seconds, not whenever the page lets go.
            let after = tokio::time::timeout(Duration::from_secs(5), async {
                let _ = c.write_all(b"id\n").await;
                c.read(&mut buf).await
            })
            .await
            .unwrap_or_else(|_| panic!("{close}: the WebSocket outlived its tunnel"));
            assert!(after.is_err() || after.unwrap() == 0, "{close}: the WebSocket outlived its tunnel");
        }
        live.stop();
    }

    /// A page on a node can do what a shell there does, so a tunnel needs a fence that names
    /// the node and allows shells: the T2 preset (cell2; test containers, process signals,
    /// files) opens none, a shell fence on cell1 opens cell1's, and each connection is asked
    /// again, so the page stops when the fence closes. Look-only holds it too.
    #[tokio::test]
    async fn a_tunnel_needs_a_fence_that_allows_shells_on_its_node() {
        use crate::live::fence_class::FenceClass::{ContainerTest, FilesTest, ProcessTest, Shell};
        let served = page("Ray's dashboard").await;
        let (n1, p1) = fake_node(&temp("tunnel-fence-1"), real_shell()).await;
        let (n2, p2) = fake_node(&temp("tunnel-fence-2"), real_shell()).await;
        listening(&n1, &[(served, "0.0.0.0")]);
        listening(&n2, &[(served, "0.0.0.0")]);
        let message = |e: String| crate::errors::SbError::classify(&e).message;

        let base = temp("tunnel-fence-t2-app");
        let (live, _) = connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        crate::live::tests::open_fence(&live, &base, &["cell2"], &[ContainerTest, ProcessTest, FilesTest]);
        let tunnels = Tunnels::new(live.clone());
        assert_eq!(message(tunnels.open("cell1", served).await.err().unwrap()), "cell1 isn't in the open fence.");
        assert_eq!(message(tunnels.open("cell2", served).await.err().unwrap()), "The fence doesn't allow shells.");
        assert!(n1.ran("direct-tcpip").is_empty() && n2.ran("direct-tcpip").is_empty(), "nothing was forwarded");
        let refused: Vec<(String, String)> = live.audit.entries().into_iter().filter(|e| e.title == "Refused").map(|e| (e.node, e.command)).collect();
        assert_eq!(refused, [("cell2".to_string(), format!("127.0.0.1:{served}")), ("cell1".to_string(), format!("127.0.0.1:{served}"))]);
        assert!(tunnels.list().is_empty());
        live.stop();

        // A shell fence on cell1: cell1's page opens and loads; cell2's is still refused.
        let base = temp("tunnel-fence-shell-app");
        let (live, _) = connect(&base, &[("cell1", p1), ("cell2", p2)]).await;
        crate::live::tests::open_fence(&live, &base, &["cell1"], &[Shell]);
        let tunnels = Tunnels::new(live.clone());
        let o = tunnels.open("cell1", served).await.unwrap();
        // Its "Opened …" row names the class the exit let it through as.
        assert_eq!(o.fence_class, "shell");
        assert_eq!(tunnels.open("cell1", served).await.unwrap().fence_class, "shell", "a reused tunnel too");
        assert_eq!(message(tunnels.open("cell2", served).await.err().unwrap()), "cell2 isn't in the open fence.");
        let at = format!("127.0.0.1:{}", o.local_port);
        assert!(get(o.local_port, &at, &format!("/?sbt={}", o.token), None).await.starts_with("HTTP/1.1 302"));
        let cookie = format!("sbt_{}={}", o.local_port, o.token);
        assert!(get(o.local_port, &at, "/", Some(&cookie)).await.contains("Ray's dashboard"));
        assert_eq!(n1.ran("direct-tcpip").len(), 2, "the open's check and the page's connection");

        // The fence closes: the next connection is refused at the exit, with the reason.
        crate::live::fence::close(&base.join("SparkBox Fence"), &base.join("config-dev"), now_ms()).unwrap();
        let after = get(o.local_port, &at, "/", Some(&cookie)).await;
        assert!(after.starts_with("HTTP/1.1 502") && after.contains("No fence is open"), "{after}");
        assert_eq!(n1.ran("direct-tcpip").len(), 2, "nothing more was forwarded");
        live.stop();

        // Look-only: refused before any fence is asked.
        let (live, _) = connect(&temp("tunnel-fence-look-app"), &[("cell1", p1)]).await;
        live.set_write_policy(crate::live::tests::TestPolicy::look_only());
        let tunnels = Tunnels::new(live.clone());
        assert!(message(tunnels.open("cell1", served).await.err().unwrap()).contains("look-only mode"));
        assert_eq!(n1.ran("direct-tcpip").len(), 2);
        live.stop();
    }

    /// The node forwards only to something that answers: a port it listed whose server has
    /// gone fails when the tunnel opens, not on the browser's first request.
    #[tokio::test]
    async fn a_listed_port_nothing_answers_on_fails_when_the_tunnel_opens() {
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (node, port) = fake_node(&temp("tunnel-gone"), real_shell()).await;
        listening(&node, &[(gone, "127.0.0.1")]);
        let (live, _) = connect(&temp("tunnel-gone-app"), &[("cell1", port)]).await;
        let tunnels = Tunnels::new(live.clone());
        let e = tunnels.open("cell1", gone).await.err().unwrap();
        assert!(e.starts_with(&format!("the node refused the forward to 127.0.0.1:{gone}: ")), "{e}");
        assert_eq!(node.ran("direct-tcpip"), [format!("direct-tcpip 127.0.0.1:{gone}")]);
        assert!(tunnels.list().is_empty());
        live.stop();
    }

    #[test]
    fn a_tunnel_goes_where_the_port_is_bound_and_known_pages_keep_their_name() {
        let l = |port: u16, addr: &str, family: Option<&str>| Listener { port, addr: addr.into(), pid: None, process: None, scope: None, family: family.map(String::from) };
        let list = [
            l(11000, "127.0.0.1", Some("v4")),
            l(8000, "0.0.0.0", Some("both")),
            l(8265, "*", None),
            l(3000, "::", Some("v6")),
            l(631, "::1", Some("v6")),
            l(9000, "192.168.100.1", Some("v4")),
        ];
        let at = |port| target("cell1", port, Some(&list));
        assert_eq!(at(11000).unwrap(), "127.0.0.1");
        assert_eq!(at(8000).unwrap(), "127.0.0.1");
        assert_eq!(at(8265).unwrap(), "127.0.0.1");
        assert_eq!(at(3000).unwrap(), "::1", "an IPv6-only socket is reached on ::1");
        assert_eq!(at(631).unwrap(), "::1");
        assert_eq!(at(9000).unwrap(), "192.168.100.1", "a port bound to one address is reached there");
        assert!(at(6379).unwrap_err().contains("isn't listening on port 6379"));
        assert!(target("cell1", 11000, None).unwrap_err().contains("hasn't had cell1's listening ports yet"));
        assert_eq!(page_label(11000), "DGX Dashboard");
        assert_eq!(known_port(11000), Some("DGX Dashboard"));
        for port in [80, 8000, 6379, 11001] {
            assert_eq!((known_port(port), page_label(port)), (None, format!("page on port {port}")));
        }
    }
}
