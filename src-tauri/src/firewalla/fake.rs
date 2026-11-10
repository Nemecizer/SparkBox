//! Test-only fake of Firewalla's cloud (plain HTTP on this Mac), in the shape of the fixtures in
//! github.com/kdesch5000/firewalla-snmp-proxy: `Authorization: Token <token>`, a list of boxes
//! with their `gid`, and a topology of `nodes`. Like the real one, an unknown path answers 200
//! with the web app's HTML. Knobs make it refuse the token, rate-limit, or answer in another shape.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};

pub const TOKEN: &str = "fake-msp-token-4f3c2a1b0e9d8c7b6a5f";
pub const GID: &str = "00000000-1111-2222-3333-444444444444";
pub const SWITCH_IP: &str = "192.168.50.238";
pub const SWITCH_MAC: &str = "20:6d:31:be:00:15";

pub struct FakeState {
    /// Every request: "GET /v2/boxes".
    pub requests: Vec<String>,
    /// The Authorization header of each.
    pub auth: Vec<String>,
    /// Answer every request with this status, and this Retry-After.
    pub status: Option<(u16, Option<String>)>,
    /// Answer the topology with this instead of `topology()`.
    pub topology: Option<Value>,
    /// Answer `/v2/boxes` with a redirect to this URL.
    pub redirect: Option<String>,
    /// Answer `/v2/boxes` with this many bytes of JSON, a chunk at a time, with no length up front.
    pub endless: Option<u64>,
    pub base: String,
}

#[derive(Clone)]
pub struct Fake(pub Arc<Mutex<FakeState>>);

impl Fake {
    pub fn requests(&self) -> Vec<String> {
        self.0.lock().unwrap().requests.clone()
    }

    pub fn set_status(&self, status: u16, retry_after: Option<&str>) {
        self.0.lock().unwrap().status = Some((status, retry_after.map(String::from)));
    }

    pub fn clear_status(&self) {
        self.0.lock().unwrap().status = None;
    }

    pub fn set_topology(&self, v: Value) {
        self.0.lock().unwrap().topology = Some(v);
    }

    pub fn base(&self) -> String {
        self.0.lock().unwrap().base.clone()
    }

    pub fn set_endless(&self, bytes: u64) {
        self.0.lock().unwrap().endless = Some(bytes);
    }

    pub fn auth(&self) -> Vec<String> {
        self.0.lock().unwrap().auth.clone()
    }
}

fn port(n: u32, up: bool, speed: Option<u32>) -> Value {
    let mut p = json!({
        "port": n.to_string(), "linkUp": up,
        "stp": { "portRole": if up { "designated" } else { "disabled" }, "portState": if up { "forwarding" } else { "discarding" } },
        "txBytes": 900277046548u64, "rxBytes": 3519353730267u64,
        "txDiscardFrames": 0, "rxDiscardFrames": 1, "txErrorFrames": 2, "rxErrorFrames": 3,
        "statsSinceTs": 1785318062, "settings": { "type": "trunk" },
    });
    if let Some(s) = speed {
        p["linkSpeed"] = json!(s);
    }
    p
}

/// A box and a 12-port switch, port 1 its uplink, with the PoE and SFP fields of the fixtures.
pub fn topology() -> Value {
    let mut ports: Vec<Value> = (1..=12).map(|n| port(n, false, None)).collect();
    ports[0] = port(1, true, Some(10_000));
    ports[2] = port(3, true, Some(2_500));
    ports[2]["poe"] = json!(true);
    ports[2]["poePower"] = json!(5.9);
    ports[2]["poeMode"] = json!("802.3at");
    ports[2]["poeStatus"] = json!("delivering");
    ports[8]["sfpInfo"] = json!({ "present": true, "connectorType": "LC  [ 0x07 ]", "nominalBitRate": "10.3 Gbps", "vendorOui": "0x00 0x00 0x00" });
    json!({ "nodes": [
        { "type": "box", "id": "20:6d:31:91:49:b6", "name": "Gold", "ip": "198.51.100.7", "online": true },
        { "type": "switch", "id": SWITCH_MAC, "mac": SWITCH_MAC, "name": "Firewalla.Switch.X", "ip": SWITCH_IP, "online": true, "model": "fwsw-X", "version": "0.6.27",
          "uplink": { "mac": "20:6d:31:91:49:b6", "type": "box", "port": "eth1", "localPort": "1", "connectionType": "ethernet" },
          "ports": ports },
    ] })
}

/// How much of the endless answer has left the fake.
pub static ENDLESS_SENT: AtomicU64 = AtomicU64::new(0);
const CHUNK: usize = 64 * 1024;

fn endless_body(total: u64) -> Response {
    let frames = futures_util::stream::unfold(0u64, move |sent| async move {
        if sent >= total {
            return None;
        }
        ENDLESS_SENT.fetch_add(CHUNK as u64, Ordering::SeqCst);
        Some((Ok::<_, std::convert::Infallible>(vec![b' '; CHUNK]), sent + CHUNK as u64))
    });
    ([(header::CONTENT_TYPE, "application/json")], axum::body::Body::from_stream(frames)).into_response()
}

async fn handle(state: axum::extract::State<Fake>, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let fake = state.0;
    let mut s = fake.0.lock().unwrap();
    s.requests.push(format!("{method} {}", uri.path()));
    let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    s.auth.push(auth.clone());
    if let Some((status, retry)) = &s.status {
        let code = StatusCode::from_u16(*status).unwrap();
        return match retry {
            Some(r) => (code, [(header::RETRY_AFTER, r.clone())], "no").into_response(),
            None => (code, "no").into_response(),
        };
    }
    if auth != format!("Token {TOKEN}") {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({ "error": "unauthorized" }))).into_response();
    }
    match uri.path() {
        "/v2/boxes" if s.endless.is_some() => endless_body(s.endless.unwrap()),
        "/v2/boxes" => match &s.redirect {
            Some(to) => (StatusCode::FOUND, [(header::LOCATION, to.clone())]).into_response(),
            None => axum::Json(json!([{ "gid": GID, "name": "Gold", "model": "gold", "version": "1.981", "online": true }])).into_response(),
        },
        p if p == format!("/v2/boxes/{GID}/topology") => axum::Json(s.topology.clone().unwrap_or_else(topology)).into_response(),
        // The SPA fallback: a 200 that isn't an answer.
        _ => ([(header::CONTENT_TYPE, "text/html")], "<!doctype html><title>MSP</title>").into_response(),
    }
}

/// Starts the fake on a random port. Returns its base URL.
pub async fn start() -> (String, Fake) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let fake = Fake(Arc::new(Mutex::new(FakeState { requests: Vec::new(), auth: Vec::new(), status: None, topology: None, redirect: None, endless: None, base: base.clone() })));
    let app = Router::new().fallback(handle).with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, fake)
}

/// A cloud that counts requests as they arrive and answers one only when `gate` gives a permit,
/// so a test can act while an answer is on its way, or abort the asker.
pub struct Gated {
    pub base: String,
    pub hits: Arc<AtomicU32>,
    pub gate: Arc<tokio::sync::Semaphore>,
    /// Answer every request with a 429 and this Retry-After, in seconds, instead.
    pub limit: Arc<AtomicU32>,
}

pub async fn gated() -> Gated {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicU32::new(0));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let limit = Arc::new(AtomicU32::new(0));
    let (h, g, l) = (hits.clone(), gate.clone(), limit.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (h, g, l) = (h.clone(), g.clone(), l.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    seen.extend_from_slice(&buf[..n]);
                }
                h.fetch_add(1, Ordering::SeqCst);
                let body = if String::from_utf8_lossy(&seen).starts_with("GET /v2/boxes ") { json!([{ "gid": GID }]).to_string() } else { topology().to_string() };
                let Ok(p) = g.acquire().await else { return };
                p.forget();
                let wait = l.load(Ordering::SeqCst);
                let resp = if wait > 0 {
                    format!("HTTP/1.1 429 Too Many Requests\r\nRetry-After: {wait}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                };
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    Gated { base, hits, gate, limit }
}
