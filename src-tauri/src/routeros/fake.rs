//! Test-only fake RouterOS 7 REST API (plain HTTP): Basic auth, string-typed JSON like
//! the real thing, byte counters that advance on every read, and knobs for a wrong
//! password and a counter reset. Writes (disable, comment, blink, reset) change its state.
//!
//! It also has the tables port mapping reads (bridge hosts, neighbors, addresses, ARP),
//! and runs what SparkBox leaves on a switch: `/system/script` (add, run, remove) and
//! `/system/scheduler` entries, whose `on-event` fires when a test calls `fire()`
//! (as if the interval had passed). Scripts are interpreted for the commands SparkBox
//! writes: `/interface ethernet disable|enable [find name="…"]`, `:delay`, and removing
//! a script or scheduler entry by name. `refuse_scheduler` makes adding an entry fail.
//!
//! The scheduler behaves like RouterOS's: `/system/clock` gives the switch's own time (in
//! RouterOS 7.10's ISO dates, or the older oct/29/2026 with `old_dates`); an entry added
//! without a start time starts now, and with an interval it runs at once (the hazard a
//! one-shot revert must avoid); `next-run` reads back what the switch will do. `skew_next_run`
//! makes it report a different time.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct FakePort {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub disabled: bool,
    pub running: bool,
    pub rx_byte: u64,
    pub tx_byte: u64,
    pub rx_error: u64,
    pub link_downs: u64,
    pub lanes: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct FakeEntry {
    pub id: String,
    pub name: String,
    /// A script's source, or a scheduler entry's on-event.
    pub source: String,
    pub interval: String,
    /// Scheduler entries: what they were given, and the next run the switch reports.
    pub start_date: String,
    pub start_time: String,
    pub next_run: String,
}

pub struct FakeState {
    pub user: String,
    pub password: String,
    pub ports: Vec<FakePort>,
    /// /interface/bridge/host: (MAC, port).
    pub hosts: Vec<(String, String)>,
    /// /ip/neighbor: (port, identity, MAC).
    pub neighbors: Vec<(String, String, String)>,
    /// /ip/address and /ip/arp: (address, interface) and (address, MAC, interface).
    pub addresses: Vec<(String, String)>,
    pub arp: Vec<(String, String, String)>,
    pub scripts: Vec<FakeEntry>,
    pub scheduler: Vec<FakeEntry>,
    pub refuse_scheduler: bool,
    /// The switch's clock: (date, time) as /system/clock reports them.
    pub clock: (String, String),
    pub old_dates: bool,
    /// Report a next-run 5 minutes off what was asked.
    pub skew_next_run: bool,
    /// On-events the switch ran the moment their entry was added.
    pub ran_at_creation: Vec<String>,
    /// Every port state change scripts and writes made, in order: "disable qsfp56-dd-1-1".
    pub transitions: Vec<String>,
    next_id: u32,
    /// Bytes each running port's counters advance per `/interface` read: (rx, tx).
    pub step: (u64, u64),
    pub blinks: Vec<String>,
    pub requests: Vec<String>,
    /// Fail just this read, including after earlier reads in the poll succeeded.
    pub fail_path: Option<(String, u16)>,
}

pub struct FakeRos {
    pub port: u16,
    pub state: Arc<Mutex<FakeState>>,
}

fn port(i: usize, name: &str, comment: &str, running: bool) -> FakePort {
    FakePort {
        id: format!("*{:X}", i + 1),
        name: name.into(),
        comment: comment.into(),
        disabled: false,
        running,
        rx_byte: 1_000_000 * (i as u64 + 1),
        tx_byte: 2_000_000 * (i as u64 + 1),
        rx_error: if i == 1 { 3 } else { 0 },
        link_downs: 1,
        lanes: vec![-1.9, -2.0, -2.2, -2.1],
    }
}

pub async fn fake_routeros() -> FakeRos {
    let state = Arc::new(Mutex::new(FakeState {
        user: "sparkbox".into(),
        password: "secret".into(),
        ports: vec![
            port(0, "qsfp56-dd-1-1", "cell1", true),
            port(1, "qsfp56-dd-1-2", "cell2 (fabric)", true),
            port(2, "qsfp56-dd-2-1", "", false),
            port(3, "ether1", "management uplink to Firewalla", true),
        ],
        step: (25_000_000, 50_000_000),
        blinks: Vec::new(),
        requests: Vec::new(),
        fail_path: None,
        hosts: Vec::new(),
        neighbors: Vec::new(),
        addresses: vec![("192.168.50.2/24".into(), "bridge".into())],
        arp: Vec::new(),
        scripts: Vec::new(),
        scheduler: Vec::new(),
        refuse_scheduler: false,
        // Half a minute before midnight, so a 60 s revert lands on the next day.
        clock: ("2026-10-29".into(), "23:59:40".into()),
        old_dates: false,
        skew_next_run: false,
        ran_at_creation: Vec::new(),
        transitions: Vec::new(),
        next_id: 0x40,
    }));
    let app = axum::Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    FakeRos { port, state }
}

impl FakeState {
    /// The switch's clock, in the date format its RouterOS version uses.
    fn clock_now(&self) -> (String, String) {
        let t = super::client::SwitchTime::parse(&self.clock.0, &self.clock.1).expect("a valid fake clock");
        if self.old_dates {
            let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
            let (y, m, d) = (&self.clock.0[0..4], self.clock.0[5..7].parse::<usize>().unwrap(), &self.clock.0[8..10]);
            return (format!("{}/{d}/{y}", months[m - 1]), t.time());
        }
        (t.date(), t.time())
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("*{:X}", self.next_id)
    }

    fn set_port(&mut self, name: &str, disabled: bool) {
        if let Some(p) = self.ports.iter_mut().find(|p| p.name == name) {
            if disabled && !p.disabled {
                p.link_downs += 1;
            }
            p.disabled = disabled;
            self.transitions.push(format!("{} {name}", if disabled { "disable" } else { "enable" }));
        }
    }

    /// Runs a RouterOS script the way the switch would, for the commands SparkBox writes.
    pub fn exec(&mut self, source: &str) {
        for stmt in source.split(';').map(str::trim) {
            let name = stmt.split("name=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("").to_string();
            if stmt.starts_with("/interface ethernet disable") {
                self.set_port(&name, true);
            } else if stmt.starts_with("/interface ethernet enable") {
                self.set_port(&name, false);
            } else if stmt.starts_with("/system scheduler remove") {
                self.scheduler.retain(|e| e.name != name);
            } else if stmt.starts_with("/system script remove") {
                self.scripts.retain(|e| e.name != name);
            }
        }
    }

    /// The interval of scheduler entry `name` passed: its on-event runs on the switch.
    pub fn fire(&mut self, name: &str) -> bool {
        let Some(e) = self.scheduler.iter().find(|e| e.name == name).cloned() else { return false };
        self.exec(&e.source);
        true
    }
}

fn authorized(s: &FakeState, headers: &HeaderMap) -> bool {
    let want = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", s.user, s.password)));
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(want.as_str())
}

fn names(body: &Value) -> Vec<String> {
    body.get("numbers").and_then(Value::as_str).unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

async fn handle(State(state): State<Arc<Mutex<FakeState>>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let mut s = state.lock().unwrap();
    let path = uri.path().replace("%2A", "*").replace("%2a", "*");
    s.requests.push(format!("{method} {path}"));
    if !authorized(&s, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": 401, "message": "Unauthorized" }))).into_response();
    }
    if let Some((fail_path, status)) = &s.fail_path {
        if path == *fail_path {
            return (StatusCode::from_u16(*status).unwrap(), Json(json!({ "error": status, "message": "fixture refusal" }))).into_response();
        }
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match (method.as_str(), path.as_str()) {
        ("GET", "/rest/system/resource") => Json(json!({
            "board-name": "CRS804-4DDQ", "version": "7.20.2 (stable)", "cpu-load": "4", "uptime": "6w1d02:55:14",
            "architecture-name": "arm64", "free-memory": "1834893312", "total-memory": "2147483648",
        }))
        .into_response(),
        ("GET", "/rest/system/health") => Json(json!([
            { ".id": "*1", "name": "cpu-temperature", "value": "47", "type": "C" },
            { ".id": "*2", "name": "switch-temperature", "value": "58", "type": "C" },
            { ".id": "*3", "name": "fan1-speed", "value": "5820", "type": "RPM" },
            { ".id": "*4", "name": "fan2-speed", "value": "5760", "type": "RPM" },
            { ".id": "*5", "name": "psu1-state", "value": "ok" },
            { ".id": "*6", "name": "psu2-state", "value": "ok" },
        ]))
        .into_response(),
        ("GET", "/rest/interface") => {
            let step = s.step;
            for p in s.ports.iter_mut().filter(|p| p.running && !p.disabled) {
                p.rx_byte += step.0;
                p.tx_byte += step.1;
            }
            Json(Value::Array(
                s.ports
                    .iter()
                    .map(|p| {
                        json!({
                            ".id": p.id, "name": p.name, "type": "ether", "comment": p.comment,
                            "running": (p.running && !p.disabled).to_string(), "disabled": p.disabled.to_string(),
                            "mtu": "1500", "l2mtu": if p.name.starts_with("qsfp") { "9216" } else { "1584" },
                            "rx-byte": p.rx_byte.to_string(), "tx-byte": p.tx_byte.to_string(),
                            "rx-error": p.rx_error.to_string(), "tx-error": "0", "rx-drop": "0", "tx-drop": "0",
                            "link-downs": p.link_downs.to_string(), "last-link-up-time": "2026-10-23 08:41:12",
                        })
                    })
                    .collect(),
            ))
            .into_response()
        }
        ("POST", "/rest/interface/ethernet/monitor") => {
            let wanted = names(&body);
            Json(Value::Array(
                s.ports
                    .iter()
                    .filter(|p| wanted.contains(&p.name))
                    .map(|p| {
                        let lanes: Vec<String> = p.lanes.iter().map(|d| format!("{d}dBm")).collect();
                        if p.name.starts_with("qsfp") {
                            json!({ "name": p.name, "status": "link-ok", "rate": "200Gbps", "full-duplex": "true", "fec": "fec91",
                                    "sfp-vendor-name": "MikroTik", "sfp-vendor-part-number": "XQ+DD85-2Q56-SR4", "sfp-temperature": "45C",
                                    "sfp-tx-power": "0.7dBm,0.8dBm,0.6dBm,0.7dBm", "sfp-rx-power": lanes.join(",") })
                        } else {
                            json!({ "name": p.name, "status": "link-ok", "rate": "1Gbps", "full-duplex": "true" })
                        }
                    })
                    .collect(),
            ))
            .into_response()
        }
        ("POST", "/rest/interface/ethernet/print") => Json(Value::Array(
            s.ports.iter().map(|p| json!({ "name": p.name, "rx-fcs-error": if p.name.ends_with("1-2") { "5" } else { "0" }, "rx-pause": "120" })).collect(),
        ))
        .into_response(),
        ("GET", "/rest/interface/bridge/host") => Json(Value::Array(
            s.hosts.iter().map(|(mac, port)| json!({ ".id": "*1", "mac-address": mac, "on-interface": port, "bridge": "bridge", "dynamic": "true", "local": "false" })).collect(),
        ))
        .into_response(),
        // A CRS804 bridges in hardware: its switch chip has no separate host menu here.
        ("GET", "/rest/interface/ethernet/switch/host") => (StatusCode::BAD_REQUEST, Json(json!({ "error": 400, "message": "Bad Request", "detail": "no such command" }))).into_response(),
        ("GET", "/rest/ip/neighbor") => Json(Value::Array(
            s.neighbors
                .iter()
                .map(|(port, identity, mac)| json!({ ".id": "*1", "interface": format!("{port},bridge"), "identity": identity, "mac-address": mac, "platform": "Linux", "discovered-by": "lldp" }))
                .collect(),
        ))
        .into_response(),
        ("GET", "/rest/ip/address") => Json(Value::Array(s.addresses.iter().map(|(a, i)| json!({ ".id": "*1", "address": a, "interface": i, "network": "192.168.50.0" })).collect())).into_response(),
        ("GET", "/rest/ip/arp") => Json(Value::Array(s.arp.iter().map(|(a, m, i)| json!({ ".id": "*1", "address": a, "mac-address": m, "interface": i, "dynamic": "true" })).collect())).into_response(),
        ("PUT", "/rest/system/script") => {
            let id = s.id();
            let e = FakeEntry {
                id: id.clone(),
                name: body.get("name").and_then(Value::as_str).unwrap_or("").into(),
                source: body.get("source").and_then(Value::as_str).unwrap_or("").into(),
                ..Default::default()
            };
            s.scripts.push(e.clone());
            Json(json!({ ".id": id, "name": e.name, "source": e.source })).into_response()
        }
        ("POST", "/rest/system/script/run") => {
            let id = body.get(".id").and_then(Value::as_str).unwrap_or("").to_string();
            let Some(e) = s.scripts.iter().find(|e| e.id == id || e.name == id).cloned() else {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": 400, "message": "Bad Request", "detail": "no such item" }))).into_response();
            };
            s.exec(&e.source);
            Json(json!([])).into_response()
        }
        ("DELETE", p) if p.starts_with("/rest/system/script/") => {
            let id = &p["/rest/system/script/".len()..];
            let before = s.scripts.len();
            s.scripts.retain(|e| e.id != id);
            if before == s.scripts.len() {
                return (StatusCode::NOT_FOUND, Json(json!({ "error": 404, "message": "Not Found" }))).into_response();
            }
            Json(json!([])).into_response()
        }
        ("PUT", "/rest/system/scheduler") => {
            if s.refuse_scheduler {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": 400, "message": "Bad Request", "detail": "not enough permissions (9)" }))).into_response();
            }
            let id = s.id();
            let text = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let (date, time) = s.clock_now();
            let start_date = Some(text("start-date")).filter(|d| !d.is_empty()).unwrap_or(date);
            let start_time = Some(text("start-time")).filter(|t| !t.is_empty()).unwrap_or(time);
            let interval = text("interval");
            let one_shot = matches!(interval.as_str(), "" | "0s" | "00:00:00");
            let started_now = text("start-time").is_empty();
            let mut next = super::client::SwitchTime::parse(&start_date, &start_time).map(|t| format!("{} {}", t.date(), t.time())).unwrap_or_default();
            if started_now && !one_shot {
                // No start time: RouterOS starts it now, and with an interval it runs at once.
                s.ran_at_creation.push(text("on-event"));
                let secs = interval.trim_end_matches('s').parse::<u32>().unwrap_or(0);
                next = super::client::SwitchTime::parse(&start_date, &start_time).map(|t| t.plus(secs)).map(|t| format!("{} {}", t.date(), t.time())).unwrap_or_default();
            }
            if s.skew_next_run {
                next = super::client::SwitchTime::parse_stamp(&next).map(|t| t.plus(300)).map(|t| format!("{} {}", t.date(), t.time())).unwrap_or_default();
            }
            let e = FakeEntry { id: id.clone(), name: text("name"), source: text("on-event"), interval, start_date, start_time, next_run: next };
            s.scheduler.push(e.clone());
            if started_now && !one_shot {
                let src = e.source.clone();
                s.exec(&src);
            }
            Json(json!({ ".id": id, "name": e.name, "on-event": e.source, "interval": e.interval })).into_response()
        }
        ("GET", "/rest/system/clock") => {
            let (date, time) = s.clock_now();
            Json(json!({ "date": date, "time": time, "time-zone-name": "Europe/Prague", "gmt-offset": "+01:00", "dst-active": "false" })).into_response()
        }
        ("GET", p) if p.starts_with("/rest/system/scheduler/") => {
            let id = &p["/rest/system/scheduler/".len()..];
            match s.scheduler.iter().find(|e| e.id == id) {
                Some(e) => Json(json!({
                    ".id": e.id, "name": e.name, "on-event": e.source, "interval": e.interval,
                    "start-date": e.start_date, "start-time": e.start_time, "next-run": e.next_run, "disabled": "false"
                }))
                .into_response(),
                None => (StatusCode::NOT_FOUND, Json(json!({ "error": 404, "message": "Not Found" }))).into_response(),
            }
        }
        ("GET", "/rest/system/scheduler") => {
            let want = uri.query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("name="))).map(String::from);
            Json(Value::Array(
                s.scheduler
                    .iter()
                    .filter(|e| want.as_deref().is_none_or(|w| e.name == w))
                    .map(|e| json!({ ".id": e.id, "name": e.name, "on-event": e.source, "interval": e.interval, "next-run": e.next_run }))
                    .collect(),
            ))
            .into_response()
        }
        ("DELETE", p) if p.starts_with("/rest/system/scheduler/") => {
            let id = &p["/rest/system/scheduler/".len()..];
            s.scheduler.retain(|e| e.id != id);
            Json(json!([])).into_response()
        }
        ("POST", "/rest/interface/ethernet/blink") => {
            let n = names(&body);
            s.blinks.extend(n);
            Json(json!([])).into_response()
        }
        ("POST", "/rest/interface/reset-counters") => {
            for n in names(&body) {
                if let Some(p) = s.ports.iter_mut().find(|p| p.name == n) {
                    p.rx_error = 0;
                    p.link_downs = 0;
                    p.rx_byte = 0;
                    p.tx_byte = 0;
                }
            }
            Json(json!([])).into_response()
        }
        ("PATCH", p) if p.starts_with("/rest/interface/ethernet/") => {
            let id = &p["/rest/interface/ethernet/".len()..];
            let Some(port) = s.ports.iter_mut().find(|x| x.id == id) else {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": 400, "message": "Bad Request", "detail": "no such item" }))).into_response();
            };
            if let Some(c) = body.get("comment").and_then(Value::as_str) {
                port.comment = c.into();
            }
            let (name, reply) = (port.name.clone(), json!({ ".id": port.id, "name": port.name, "comment": port.comment }));
            if let Some(d) = body.get("disabled").and_then(Value::as_str) {
                s.set_port(&name, d == "true" || d == "yes");
            }
            Json(reply).into_response()
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": 404, "message": "Not Found" }))).into_response(),
    }
}
