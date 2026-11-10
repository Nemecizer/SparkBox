use std::sync::atomic::Ordering as AO;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;

use super::fake::{self, Fake, GID, SWITCH_IP, TOKEN};
use super::labels::{LabelKind, PortLabel};
use super::msp::*;
use super::*;

const NOW: u64 = 1_800_000_000_000;
const MIN: u64 = 60_000;
const HOUR: u64 = 60 * MIN;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sparkbox-firewalla-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make(dir: &std::path::Path, token: Option<&'static str>) -> Arc<Firewalla> {
    Firewalla::new(dir.join("firewalla_ports.json"), dir.join("firewalla_msp.json"), move || Ok(token.map(String::from)), || false)
}

/// A Firewalla with a domain saved, talking to a fake.
async fn rig(name: &str) -> (Arc<Firewalla>, Fake, PathBuf) {
    let dir = temp(name);
    let (base, fake) = fake::start().await;
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(base);
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    (fw, fake, dir)
}

fn want() -> Wanted {
    Wanted { host: SWITCH_IP.into(), macs: Vec::new() }
}

fn label(kind: LabelKind, node: Option<&str>, text: Option<&str>) -> PortLabel {
    PortLabel { kind, node: node.map(String::from), text: text.map(String::from), set_at_ms: 0 }
}

// ---- The client's limits

#[test]
fn the_domain_must_be_https_under_firewalla_net() {
    assert_eq!(check_domain("dn-abc123.firewalla.net").unwrap(), "dn-abc123.firewalla.net");
    assert_eq!(check_domain(" https://DN-abc123.Firewalla.net/ ").unwrap(), "dn-abc123.firewalla.net");
    for bad in [
        "http://dn-abc123.firewalla.net",
        "ftp://dn-abc123.firewalla.net",
        "firewalla.net",
        ".firewalla.net",
        "evil.com",
        "firewalla.net.evil.com",
        "notfirewalla.net",
        "dn-abc123.firewalla.net.evil.com",
        "dn-abc123.firewalla.net:8443",
        "dn-abc123.firewalla.net/v2",
        "user@dn-abc123.firewalla.net",
        "evil.com#.firewalla.net",
        "evil.com/.firewalla.net",
        "-bad.firewalla.net",
        "a..firewalla.net",
        "",
    ] {
        assert!(check_domain(bad).is_err(), "{bad:?} must be refused");
    }
    assert!(Client::new("evil.com", TOKEN).is_err(), "the client itself refuses it");
}

#[test]
fn only_the_two_paths_can_be_named() {
    assert_eq!(Request::Boxes.path(), "/v2/boxes");
    let gid = Gid::new(GID).unwrap();
    assert_eq!(Request::Topology(gid).path(), format!("/v2/boxes/{GID}/topology"));
    // A box id goes into the path, so it can't carry one of its own.
    for bad in ["", "../switches", "a/b", "a?b=c", "a b", "x%2Fy", "a.b"] {
        assert!(Gid::new(bad).is_err(), "{bad:?} must be refused");
    }
    assert!(Gid::new(&"a".repeat(65)).is_err());
}

/// A guard on the source: this client's one request is a GET, so a mutating call can't be
/// expressed by a later edit that forgets the rule. The API has reset, reboot and power-cycle.
#[test]
fn the_client_has_no_verb_but_get() {
    let src = include_str!("msp.rs");
    let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    assert_eq!(code.matches(".get(url)").count(), 1, "one GET");
    for verb in [".post(", ".put(", ".patch(", ".delete(", ".head(", "Method::", ".request("] {
        assert!(!code.contains(verb), "the client must not use {verb}");
    }
    assert!(code.contains("Policy::none()"), "redirects aren't followed");
    assert!(code.contains(".https_only(https_only)"));
    assert!(code.contains(r#"Client::build(format!("https://{host}"), token, true)"#), "the real client is the HTTPS-only one");
}

#[tokio::test]
async fn an_https_only_client_sends_nothing_over_plain_http() {
    let (base, fake) = fake::start().await;
    let e = Client::strict(&base, TOKEN).unwrap().boxes().await.unwrap_err();
    assert!(matches!(e, MspError::Unreachable(_)), "{e:?}");
    assert!(fake.requests().is_empty(), "not one byte left");
    assert!(Client::new("dn-abc123.firewalla.net", TOKEN).is_ok(), "the real client builds");
}

#[tokio::test]
async fn a_token_with_a_line_break_goes_nowhere_and_is_not_repeated() {
    let dir = temp("crlf");
    let (base, fake) = fake::start().await;
    let fw = make(&dir, Some("secret-part-one\r\nX-Evil: secret-part-two"));
    *fw.fake_base.lock().unwrap() = Some(base);
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let t = fw.test(&want(), NOW).await;
    assert!(!t.ok && !t.message.contains("secret-part"), "{t:?}");
    assert!(t.message.contains("can't be sent"), "{t:?}");
    assert!(fake.requests().is_empty());
}

/// Whatever `check_domain` accepts must parse to a URL whose host is under firewalla.net.
#[test]
fn nothing_check_domain_accepts_can_leave_firewalla_net() {
    let parts = ["a", "dn-abc", "firewalla", "net", "firewalla.net", ".firewalla.net", ".", "..", "/", "//", ":", "@", "?", "#", "\\", "%2f", "%00", "https://", "http://", "evil.com", "user", "443", "-", " ", "\t", "\n", "\u{3002}", "\u{ff0e}", "\u{212a}", "xn--", "[::1]", "127.0.0.1", ";", ",", "&", "=", "+"];
    let mut rng = fastrand::Rng::with_seed(7);
    for _ in 0..60_000 {
        let mut s = String::new();
        for _ in 0..rng.usize(1..7) {
            s.push_str(parts[rng.usize(..parts.len())]);
        }
        if rng.bool() {
            s.push_str(".firewalla.net");
        }
        if let Ok(host) = check_domain(&s) {
            // A name the URL parser refuses never leaves the Mac.
            let Ok(url) = reqwest::Url::parse(&format!("https://{host}")) else { continue };
            assert_eq!(url.host_str(), Some(host.as_str()), "{s:?}");
            assert!(host.ends_with(".firewalla.net") && url.username().is_empty() && url.port().is_none() && url.path() == "/", "{s:?} -> {url}");
        }
    }
}

// ---- A good round against the fake

#[tokio::test]
async fn a_poll_finds_the_switch_and_keeps_the_token_out_of_everything_but_the_header() {
    let (fw, fake, dir) = rig("good").await;
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(fake.requests(), ["GET /v2/boxes", &format!("GET /v2/boxes/{GID}/topology")], "the two paths, GET only");
    assert!(fake.auth().iter().all(|a| a == &format!("Token {TOKEN}")), "{:?}", fake.auth());

    let st = fw.status(NOW + 1000);
    assert_eq!((st.state.as_str(), st.message.as_deref(), st.last_ok_ms), ("ok", None, Some(NOW)));
    let sw = st.sw.unwrap();
    assert_eq!((sw.name.as_deref(), sw.model.as_deref(), sw.ports.len(), sw.skipped), (Some("Firewalla.Switch.X"), Some("fwsw-X"), 12, 0));
    let p1 = &sw.ports[0];
    assert_eq!((p1.port, p1.link_up, p1.link_speed_mbps, p1.uplink), (1, Some(true), Some(10_000), true), "uplink is the switch's, by localPort");
    assert_eq!((p1.rx_bytes, p1.tx_bytes, p1.rx_errors, p1.tx_errors, p1.rx_discards, p1.tx_discards), (Some(3519353730267), Some(900277046548), Some(3), Some(2), Some(1), Some(0)));
    assert_eq!((p1.stp_state.as_deref(), p1.stp_role.as_deref(), p1.stats_since_s), (Some("forwarding"), Some("designated"), Some(1785318062)));
    let p2 = &sw.ports[1];
    assert_eq!((p2.link_up, p2.link_speed_mbps, p2.uplink), (Some(false), None, false), "a down port has no speed");
    let p3 = &sw.ports[2];
    assert_eq!((p3.link_speed_mbps, p3.poe_status.as_deref(), p3.poe_mode.as_deref(), p3.poe_watts), (Some(2500), Some("delivering"), Some("802.3at"), Some(5.9)));
    let sfp = sw.ports[8].sfp.clone().unwrap();
    assert_eq!((sfp.present, sfp.bit_rate.as_deref()), (Some(true), Some("10.3 Gbps")));

    // Nothing on disk holds the token.
    for f in std::fs::read_dir(&dir).unwrap() {
        let text = std::fs::read_to_string(f.unwrap().path()).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
    }
}

#[tokio::test]
async fn the_schedule_holds_it_to_one_poll_in_fifteen_minutes() {
    let (fw, fake, _dir) = rig("schedule").await;
    fw.poll_if_due(&want(), NOW).await;
    let first = fake.requests().len();
    fw.poll_if_due(&want(), NOW + 30_000).await;
    fw.poll_if_due(&want(), NOW + 14 * MIN).await;
    assert_eq!(fake.requests().len(), first, "not yet");
    fw.poll_if_due(&want(), NOW + 15 * MIN).await;
    assert_eq!(fake.requests().len(), first + 1, "and then one call: the box is remembered");
    assert_eq!(fake.requests().last().unwrap(), &format!("GET /v2/boxes/{GID}/topology"));
}

#[tokio::test]
async fn a_restart_loop_with_the_request_killed_each_time_asks_once() {
    let dir = temp("kill");
    let srv = fake::gated().await;
    let first = make(&dir, Some(TOKEN));
    first.set_domain("dn-test.firewalla.net", NOW).unwrap();
    drop(first);
    for i in 0..10u64 {
        let fw = make(&dir, Some(TOKEN));
        *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
        let f = fw.clone();
        let t = NOW + i * 2_000;
        let h = tokio::spawn(async move { f.poll_if_due(&want(), t).await });
        // Long enough for the request to arrive, if one's coming: the first one must.
        for _ in 0..if i == 0 { 100 } else { 10 } {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if srv.hits.load(AO::SeqCst) as u64 > i.min(1) {
                break;
            }
        }
        h.abort();
        let _ = h.await;
    }
    assert_eq!(srv.hits.load(AO::SeqCst), 1, "one poll in 15 minutes, whatever restarts");
    assert!(make(&dir, Some(TOKEN)).status(NOW + 30_000).next_poll_ms >= Some(NOW + 15 * MIN), "the wait was saved before the request");
}

#[tokio::test]
async fn a_failure_waits_the_full_fifteen_minutes() {
    for fail in [500, 404] {
        let (fw, fake, _dir) = rig("failed").await;
        fake.set_status(fail, None);
        fw.poll_if_due(&want(), NOW).await;
        let asked = fake.requests().len();
        fw.poll_if_due(&want(), NOW + MIN).await;
        fw.poll_if_due(&want(), NOW + 14 * MIN).await;
        assert_eq!(fake.requests().len(), asked, "no retry at once");
        fw.poll_if_due(&want(), NOW + 15 * MIN).await;
        assert_eq!(fake.requests().len(), asked + 1);
    }
    // Nobody listening counts the same.
    let (fw, fake, _dir) = rig("failed-down").await;
    *fw.fake_base.lock().unwrap() = Some("http://127.0.0.1:9".into());
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(fw.status(NOW + 1).state, "unreachable");
    *fw.fake_base.lock().unwrap() = Some(fake.base());
    fw.poll_if_due(&want(), NOW + MIN).await;
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn the_poll_is_held_with_demo_data_and_in_look_only_mode() {
    let (fw, fake, _dir) = rig("held").await;
    fw.tick(false, false, Some(want()), NOW).await;
    assert!(fake.requests().is_empty(), "not with Demo data");
    fw.tick(true, true, Some(want()), NOW).await;
    assert!(fake.requests().is_empty(), "not in look-only mode");
    fw.tick(false, true, Some(want()), NOW).await;
    fw.tick(true, false, None, NOW).await;
    assert!(fake.requests().is_empty(), "not with nothing to look for");
    fw.tick(true, false, Some(want()), NOW).await;
    assert_eq!(fake.requests().len(), 2, "Live, outside look-only: one poll");
}

/// A guard on the source: the loop passes the real data source and look-only state to `tick`, and
/// the domain, which says where the token goes, confirms every time as the token does.
#[test]
fn the_loop_and_the_domain_save_keep_their_guards() {
    let code = include_str!("mod.rs");
    assert!(code.contains("fw.tick(crate::is_live(&state), crate::admin::look_only(), want, commands::now_ms()).await;"));
    let commands = include_str!("commands.rs");
    let save = &commands[commands.find("pub fn firewalla_msp_set_domain").unwrap()..];
    assert!(save.lines().nth(1).unwrap().trim() == r#"admin::require_mac_confirmed(&app, "firewalla_msp_set_domain")?;"#, "{save}");
    assert!(commands.contains("fw.test_connection(live, want, now_ms()).await"));
}

/// Waits for the gated cloud to have seen `n` requests.
async fn seen(srv: &fake::Gated, n: u32) {
    for _ in 0..200 {
        if srv.hits.load(AO::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the request never arrived");
}

#[tokio::test]
async fn look_only_coming_on_mid_round_stops_the_next_request() {
    let dir = temp("held-mid");
    let srv = fake::gated().await;
    let on = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = on.clone();
    let fw = Firewalla::new(dir.join("p.json"), dir.join("m.json"), || Ok(Some(TOKEN.to_string())), move || flag.load(AO::SeqCst));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.test(&want(), NOW).await });
    seen(&srv, 1).await;
    on.store(true, AO::SeqCst);
    srv.gate.add_permits(10);
    let t = h.await.unwrap();
    assert_eq!((t.ok, t.kind.as_deref()), (false, Some("held")), "{t:?}");
    assert_eq!(srv.hits.load(AO::SeqCst), 1, "the box list went out before look-only; its topology must not");
    assert_eq!(fw.status(NOW + 1).state, "waiting", "nothing went wrong, so there's nothing to report");
    // Look-only on from the start: not one request, whatever the schedule says.
    let fw2 = Firewalla::new(dir.join("p2.json"), dir.join("m2.json"), || Ok(Some(TOKEN.to_string())), || true);
    *fw2.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw2.set_domain("dn-test.firewalla.net", NOW).unwrap();
    fw2.poll_if_due(&want(), NOW).await;
    assert_eq!(srv.hits.load(AO::SeqCst), 1);
}

#[tokio::test]
async fn the_held_poll_goes_out_when_look_only_turns_off() {
    let (fw, fake, _dir) = rig("resume").await;
    for i in 0..5 {
        fw.tick(true, true, Some(want()), NOW + i * HOUR).await;
    }
    assert!(fake.requests().is_empty());
    assert!(fw.status_for(true, true, NOW).held);
    fw.tick(true, false, Some(want()), NOW + 5 * HOUR).await;
    assert!(!fake.requests().is_empty(), "the held poll went out");
    assert!(!fw.status_for(true, false, NOW + 5 * HOUR + 1).held);
    assert_eq!(fw.status(NOW + 5 * HOUR + 1).state, "ok");
}

#[test]
fn the_panel_is_told_when_look_only_holds_the_cloud_and_demo_shows_only_samples() {
    let dir = temp("held-status");
    let fw = make(&dir, Some(TOKEN));
    assert!(!fw.status_for(true, true, NOW).held, "no domain, nothing to hold");
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    assert!(fw.status_for(true, true, NOW).held);
    assert!(!fw.status_for(true, false, NOW).held);
    let demo = fw.status_for(false, true, NOW);
    assert!(!demo.held && demo.domain.as_deref() == Some("dn-demo.firewalla.net"), "{demo:?}");
}

#[tokio::test]
async fn test_connection_sends_nothing_with_demo_data() {
    let (fw, fake, _dir) = rig("demo-test").await;
    let t = fw.test_connection(false, Some(want()), NOW).await;
    assert!(t.ok && t.message.starts_with("Demo data: nothing was sent to Firewalla."), "{t:?}");
    assert!(fake.requests().is_empty());
    let t = fw.test_connection(true, None, NOW).await;
    assert_eq!((t.ok, t.kind.as_deref()), (false, Some("not-configured")));
    assert!(fake.requests().is_empty());
    assert!(fw.test_connection(true, Some(want()), NOW).await.ok);
    assert!(!fake.requests().is_empty());
}

#[tokio::test]
async fn nothing_is_asked_without_a_domain_and_no_keychain_read_happens_either() {
    let dir = temp("off");
    let (base, fake) = fake::start().await;
    let read = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let seen = read.clone();
    let fw = Firewalla::new(
        dir.join("p.json"),
        dir.join("m.json"),
        move || {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(TOKEN.to_string()))
        },
        || false,
    );
    *fw.fake_base.lock().unwrap() = Some(base);
    fw.poll_if_due(&want(), NOW).await;
    assert!(fake.requests().is_empty());
    assert_eq!(read.load(std::sync::atomic::Ordering::SeqCst), 0, "the token isn't touched until there's a domain");
    assert_eq!(fw.status(NOW).state, "off");
    let t = fw.test(&want(), NOW).await;
    assert_eq!((t.ok, t.kind.as_deref()), (false, Some("not-configured")));
}

#[tokio::test]
async fn no_token_says_so_and_sends_nothing() {
    let dir = temp("no-token");
    let (base, fake) = fake::start().await;
    let fw = make(&dir, None);
    *fw.fake_base.lock().unwrap() = Some(base);
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    fw.poll_if_due(&want(), NOW).await;
    assert!(fake.requests().is_empty());
    assert_eq!(fw.status(NOW).state, "no-token");
}

// ---- What can go wrong

#[tokio::test]
async fn html_on_a_200_is_not_an_answer() {
    let (fw, _fake, _dir) = rig("html").await;
    // Every path under this base is unknown, so each answers with the web app's page.
    let (base, html) = fake::start().await;
    *fw.fake_base.lock().unwrap() = Some(format!("{base}/not-the-api"));
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(html.requests(), ["GET /not-the-api/v2/boxes"]);
    let st = fw.status(NOW + 1);
    assert_eq!(st.state, "not-json");
    assert!(st.message.unwrap().contains("web page"));
    assert!(st.sw.is_none(), "nothing was guessed");
}

#[tokio::test]
async fn a_shape_it_does_not_know_is_a_plain_message_not_a_guess() {
    for (bad, words) in [
        (json!({ "devices": [] }), "no list of devices"),
        (json!({ "nodes": "many" }), "no list of devices"),
        (json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP }] }), "no list of ports"),
        (json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP, "ports": [] }] }), "no list of ports"),
        (json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP, "ports": [{ "name": "gi1" }] }] }), "has a number"),
        (json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP, "ports": [{ "port": "1", "up": true }] }] }), "whether it has a link"),
    ] {
        let (fw, fake, _dir) = rig("shape").await;
        fake.set_topology(bad);
        fw.poll_if_due(&want(), NOW).await;
        let st = fw.status(NOW + 1);
        assert_eq!(st.state, "shape");
        let m = st.message.unwrap();
        assert!(m.contains(words) && m.starts_with("SparkBox doesn't recognize"), "{m}");
        assert!(st.sw.is_none());
    }
}

#[tokio::test]
async fn every_field_is_optional_and_odd_ports_are_counted_not_placed() {
    let (fw, fake, _dir) = rig("sparse").await;
    fake.set_topology(json!({ "nodes": [{ "type": "SWITCH", "ip": SWITCH_IP, "ports": [
        { "port": 2, "linkUp": true },
        { "port": "1", "linkUp": false, "linkSpeed": "1000", "rxBytes": "12" },
        { "port": "sfp-1", "linkUp": true },
        { "port": "13", "linkUp": true },
        { "port": "0", "linkUp": true },
        { "port": "10", "linkUp": true, "linkSpeed": null, "stp": "on", "settings": 4, "sfpInfo": [] },
    ] }] }));
    fw.poll_if_due(&want(), NOW).await;
    let sw = fw.status(NOW + 1).sw.expect("understood");
    assert_eq!(sw.ports.iter().map(|p| p.port).collect::<Vec<_>>(), [1, 2, 10], "by number, whatever the order");
    assert_eq!(sw.skipped, 3, "no number, and numbers the panel doesn't have");
    assert_eq!((sw.ports[0].link_speed_mbps, sw.ports[0].rx_bytes), (Some(1000), Some(12)), "digits in a string count");
    assert_eq!((sw.ports[1].link_speed_mbps, sw.ports[1].rx_bytes, sw.ports[1].poe_status.as_deref()), (None, None, None), "absent stays absent, never zero");
    assert_eq!((sw.ports[2].stp_state.as_deref(), sw.ports[2].sfp.is_none()), (None, true));
}

#[tokio::test]
async fn the_switch_is_found_by_address_or_mac_and_never_by_being_the_only_one() {
    let (fw, _fake, _dir) = rig("match").await;
    let mut other = want();
    other.host = "192.168.50.9".into();
    fw.poll_if_due(&other, NOW).await;
    let st = fw.status(NOW + 1);
    assert_eq!(st.state, "not-listed");
    assert!(st.message.unwrap().contains("Firewalla lists 1 switch, and none has the address 192.168.50.9"));
    assert!(st.sw.is_none());

    // By the chassis LLDP hears, in any case and separator.
    let (fw, _fake, _dir) = rig("match-mac").await;
    fw.poll_if_due(&Wanted { host: "192.168.50.9".into(), macs: vec!["20-6d-31-be-00-15".into()] }, NOW).await;
    assert_eq!(fw.status(NOW + 1).state, "ok");
}

#[test]
fn the_address_outranks_a_mac_heard_from_another_switch() {
    let ports = |speed: u64| json!([{ "port": "1", "linkUp": true, "linkSpeed": speed }]);
    let v = json!({ "nodes": [
        { "type": "switch", "mac": "AA:AA:AA:AA:AA:AA", "ip": "192.168.50.9", "name": "other", "ports": ports(100) },
        { "type": "switch", "mac": "BB:BB:BB:BB:BB:BB", "ip": SWITCH_IP, "name": "mine", "ports": ports(10_000) },
    ]});
    let w = Wanted { host: SWITCH_IP.into(), macs: vec!["aa:aa:aa:aa:aa:aa".into()] };
    assert_eq!(parse_topology(&v, &w).unwrap().name.as_deref(), Some("mine"));
    let by_mac = Wanted { host: String::new(), macs: vec!["aa-aa-aa-aa-aa-aa".into()] };
    assert_eq!(parse_topology(&v, &by_mac).unwrap().name.as_deref(), Some("other"), "with no address, the MAC decides");
    let both = Wanted { host: SWITCH_IP.into(), macs: vec!["aa:aa:aa:aa:aa:aa".into(), "bb:bb:bb:bb:bb:bb".into()] };
    assert_eq!(parse_topology(&v, &both).unwrap().name.as_deref(), Some("mine"), "LLDP hearing two Switch Xs changes nothing");
}

#[test]
fn a_port_listed_twice_that_disagrees_on_the_link_is_not_lit() {
    let of = |ports: serde_json::Value| parse_topology(&json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP, "ports": ports }] }), &want()).unwrap();
    for ports in [
        json!([{ "port": "1", "linkUp": true, "linkSpeed": 1000 }, { "port": "1", "linkUp": false, "linkSpeed": 1000 }, { "port": "2", "linkUp": false }]),
        json!([{ "port": "1", "linkUp": false, "linkSpeed": 1000 }, { "port": "1", "linkUp": true, "linkSpeed": 1000 }, { "port": "2", "linkUp": false }]),
    ] {
        let sw = of(ports);
        assert_eq!((sw.ports[0].link_up, sw.ports[0].link_speed_mbps), (None, None), "neither is believed");
    }
}

#[test]
fn a_port_listed_twice_keeps_one_entry_and_no_speed_it_can_not_trust() {
    let of = |ports: serde_json::Value| parse_topology(&json!({ "nodes": [{ "type": "switch", "ip": SWITCH_IP, "ports": ports }] }), &want()).unwrap();
    let sw = of(json!([{ "port": "1", "linkUp": true, "linkSpeed": 100 }, { "port": 1, "linkUp": true, "linkSpeed": 10_000 }, { "port": "2", "linkUp": false }]));
    assert_eq!(sw.ports.iter().map(|p| p.port).collect::<Vec<_>>(), [1, 2]);
    assert_eq!((sw.ports[0].link_speed_mbps, sw.skipped), (None, 1), "they disagree, so neither speed stands");
    let sw = of(json!([{ "port": "1", "linkUp": true, "linkSpeed": 100 }, { "port": 1, "linkUp": true, "linkSpeed": 100 }]));
    assert_eq!((sw.ports.len(), sw.ports[0].link_speed_mbps), (1, Some(100)), "the same twice is still the same");
}

#[tokio::test]
async fn an_answer_that_may_not_be_this_switch_is_not_shown() {
    // Another address: the last answer was some switch's, and this one says it isn't ours.
    let (fw, _fake, _dir) = rig("notlisted").await;
    fw.poll_if_due(&want(), NOW).await;
    let mut other = want();
    other.host = "192.168.50.9".into();
    fw.poll_if_due(&other, NOW + 15 * MIN).await;
    let st = fw.status(NOW + 15 * MIN + 1);
    assert_eq!((st.state.as_str(), st.sw.is_none()), ("not-listed", true));
    // "It shows nothing from the cloud" means that, and a good poll brings it back.
    let (fw, fake, _dir) = rig("shape-after").await;
    fw.poll_if_due(&want(), NOW).await;
    fake.set_topology(json!({ "devices": [] }));
    fw.poll_if_due(&want(), NOW + 15 * MIN).await;
    let st = fw.status(NOW + 15 * MIN + 1);
    assert_eq!((st.state.as_str(), st.sw.is_none()), ("shape", true));
    fake.0.lock().unwrap().topology = None;
    fw.poll_if_due(&want(), NOW + 30 * MIN).await;
    assert!(fw.status(NOW + 30 * MIN + 1).sw.is_some(), "a good poll brings a new reading");
}

/// What each kind of untrusted answer does to the last reading: `poison` turns the rig's next poll
/// into that answer.
async fn untrusted(name: &str, state: &str, poison: impl FnOnce(&Arc<Firewalla>, &Fake)) -> (Arc<Firewalla>, Fake, PathBuf) {
    let (fw, fake, dir) = rig(name).await;
    fw.poll_if_due(&want(), NOW).await;
    assert!(fw.status(NOW + 1).sw.is_some(), "a good answer first");
    poison(&fw, &fake);
    fw.poll_if_due(&want(), NOW + 15 * MIN).await;
    let st = fw.status(NOW + 15 * MIN + 1);
    assert_eq!((st.state.as_str(), st.sw.is_none(), st.last_ok_ms), (state, true, None), "{name}");
    (fw, fake, dir)
}

/// An answer that says the last reading may not be this switch's drops it for good.
#[tokio::test]
async fn a_dropped_answer_stays_gone_after_a_restart_a_429_and_a_token_save() {
    type Poison = fn(&Arc<Firewalla>, &Fake);
    let not_listed: Poison = |_, f| f.set_topology(json!({ "nodes": [{ "type": "switch", "ip": "192.168.50.9", "ports": [{ "port": "1", "linkUp": true }] }] }));
    let shape: Poison = |_, f| f.set_topology(json!({ "devices": [] }));
    // Every path answers with the web app's page.
    let not_json: Poison = |fw, f| *fw.fake_base.lock().unwrap() = Some(format!("{}/not-the-api", f.base()));
    for (state, poison) in [("not-listed", not_listed), ("shape", shape), ("not-json", not_json)] {
        // A restart: nothing of it is left in the file.
        let (fw, _fake, dir) = untrusted(&format!("drop-restart-{state}"), state, poison).await;
        drop(fw);
        let again = make(&dir, Some(TOKEN));
        let st = again.status(NOW + 15 * MIN + 2);
        assert_eq!((st.state.as_str(), st.sw.is_none(), st.last_ok_ms), ("waiting", true, None), "{state} after a restart");

        // A 429 replaces the state that was hiding it.
        let (fw, fake, _dir) = untrusted(&format!("drop-429-{state}"), state, poison).await;
        fake.set_status(429, None);
        fw.poll_if_due(&want(), NOW + 30 * MIN).await;
        let st = fw.status(NOW + 30 * MIN + 1);
        assert_eq!((st.state.as_str(), st.sw.is_none()), ("rate-limited", true), "{state} after a 429");

        // Saving the token again clears the problem that was hiding it.
        let (fw, _fake, _dir) = untrusted(&format!("drop-token-{state}"), state, poison).await;
        fw.token_changed(true, NOW + 16 * MIN);
        let st = fw.status(NOW + 16 * MIN + 1);
        assert_eq!((st.sw.is_none(), st.last_ok_ms), (true, None), "{state} after a token save");
    }
}

/// `set_domain` and `token_changed` run on their own threads, and a Keychain read can block on a
/// macOS prompt: a change landing during it must still count against this round.
fn changing_during_the_keychain_read(dir: &std::path::Path, change: fn(&Firewalla)) -> (Arc<Firewalla>, Arc<Mutex<Option<Arc<Firewalla>>>>) {
    let slot: Arc<Mutex<Option<Arc<Firewalla>>>> = Arc::new(Mutex::new(None));
    let during = slot.clone();
    let fw = Firewalla::new(
        dir.join("p.json"),
        dir.join("m.json"),
        move || {
            if let Some(f) = during.lock().unwrap().as_ref() {
                change(f);
            }
            Ok(Some(TOKEN.to_string()))
        },
        || false,
    );
    (fw, slot)
}

#[tokio::test]
async fn a_change_during_the_keychain_read_sets_the_answer_aside() {
    let changes: [fn(&Firewalla); 2] = [
        |f| {
            let _ = f.set_domain("dn-new.firewalla.net", NOW + 1);
        },
        |f| f.token_changed(false, NOW + 1),
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let dir = temp(&format!("keychain-{i}"));
        let (base, _fake) = fake::start().await;
        let (fw, slot) = changing_during_the_keychain_read(&dir, change);
        *fw.fake_base.lock().unwrap() = Some(base);
        fw.set_domain("dn-old.firewalla.net", NOW).unwrap();
        *slot.lock().unwrap() = Some(fw.clone());
        fw.poll_if_due(&want(), NOW).await;
        *slot.lock().unwrap() = None;
        let st = fw.status(NOW + 2);
        assert_eq!((st.sw.is_none(), st.last_ok_ms), (true, None), "change {i}: an answer for the old settings was kept");
    }
}

#[tokio::test]
async fn a_set_aside_answer_does_not_lose_the_poll() {
    let dir = temp("set-aside");
    let srv = fake::gated().await;
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.poll_if_due(&want(), NOW).await });
    seen(&srv, 1).await;
    fw.token_changed(true, NOW + 1);
    srv.gate.add_permits(100);
    h.await.unwrap();
    let next = fw.status(NOW + 2).next_poll_ms.unwrap();
    assert!(next <= NOW + 15 * MIN, "rescheduled");
    let asked = srv.hits.load(AO::SeqCst);
    fw.poll_if_due(&want(), next).await;
    assert!(srv.hits.load(AO::SeqCst) > asked, "the poll came round");
    assert_eq!(fw.status(next + 1).state, "ok");
}

#[tokio::test]
async fn a_killed_round_leaves_the_lock_free() {
    let dir = temp("lock");
    let srv = fake::gated().await;
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.poll_if_due(&want(), NOW).await });
    seen(&srv, 1).await;
    h.abort();
    let _ = h.await;
    srv.gate.add_permits(100);
    let t = tokio::time::timeout(Duration::from_secs(3), fw.test(&want(), NOW + 6 * MIN)).await.expect("Test isn't stuck behind the dead round");
    assert!(t.ok, "{t:?}");
}

/// `asking` must only ever push the next poll out: a killed Test can't shorten a longer wait.
#[tokio::test]
async fn a_request_that_dies_does_not_shorten_a_longer_wait() {
    let mut s = Schedule { next_ms: NOW + HOUR, ..Default::default() };
    s.asking(NOW);
    assert_eq!((s.next_ms, s.last_try_ms), (NOW + HOUR, NOW));
    s.next_ms = 0;
    s.asking(NOW);
    assert_eq!(s.next_ms, NOW + 15 * MIN);

    // The same through a round: a refused token waits an hour, and a Test dies part-way.
    let (fw, fake, _dir) = rig("asking").await;
    fake.set_status(401, None);
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(fw.status(NOW + 1).next_poll_ms, Some(NOW + HOUR));
    let srv = fake::gated().await;
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    let f = fw.clone();
    let h = tokio::spawn(async move { f.test(&want(), NOW + 6 * MIN).await });
    seen(&srv, 1).await;
    h.abort();
    let _ = h.await;
    assert_eq!(fw.status(NOW + 7 * MIN).next_poll_ms, Some(NOW + HOUR), "the hour stands");
}

async fn stubborn_server(hang: bool) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n[").await;
                if hang {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
        }
    });
    base
}

/// Headers arrive, then the body stalls: the client must give up by itself.
#[tokio::test]
async fn a_body_that_stalls_ends_in_a_timeout_and_a_normal_wait() {
    let dir = temp("stall");
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(stubborn_server(true).await);
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let t = tokio::time::timeout(Duration::from_secs(30), fw.test(&want(), NOW)).await.expect("gives up by itself");
    assert_eq!(t.kind.as_deref(), Some("unreachable"), "{t:?}");
    assert_eq!(fw.status(NOW + 1).next_poll_ms, Some(NOW + 15 * MIN));
}

#[tokio::test]
async fn a_body_cut_short_is_not_an_answer() {
    let dir = temp("short");
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(stubborn_server(false).await);
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let t = fw.test(&want(), NOW).await;
    assert_eq!((t.ok, t.kind.as_deref()), (false, Some("unreachable")), "{t:?}");
}

#[tokio::test]
async fn an_endless_answer_stops_at_the_cap_and_is_not_read_to_its_end() {
    let (fw, fake, _dir) = rig("endless").await;
    fake::ENDLESS_SENT.store(0, AO::SeqCst);
    fake.set_endless(64 * 1024 * 1024);
    fw.poll_if_due(&want(), NOW).await;
    let st = fw.status(NOW + 1);
    assert_eq!(st.state, "shape");
    assert!(st.message.unwrap().contains("far too long"));
    let sent = fake::ENDLESS_SENT.load(AO::SeqCst);
    assert!(sent < 16 * 1024 * 1024, "stopped reading near the 4 MiB cap, after {sent} bytes of 64 MiB");
}

#[tokio::test]
async fn an_answer_arriving_after_the_token_goes_is_set_aside() {
    let dir = temp("race-token");
    let srv = fake::gated().await;
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.poll_if_due(&want(), NOW).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    fw.token_changed(false, NOW + 1);
    srv.gate.add_permits(10);
    h.await.unwrap();
    assert!(fw.status(NOW + 2).sw.is_none(), "removing the token drops what it read, even an answer still on its way");
}

#[tokio::test]
async fn a_429_arriving_after_the_token_changes_still_counts() {
    let dir = temp("race-429");
    let srv = fake::gated().await;
    srv.limit.store(7200, AO::SeqCst);
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-test.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.poll_if_due(&want(), NOW).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    fw.token_changed(true, NOW + 1);
    srv.gate.add_permits(10);
    h.await.unwrap();
    let st = fw.status(NOW + 2);
    assert_eq!((st.state.as_str(), st.next_poll_ms), ("rate-limited", Some(NOW + 2 * HOUR)), "the quota doesn't care what changed");
}

#[tokio::test]
async fn an_answer_arriving_after_the_domain_changes_is_set_aside() {
    let dir = temp("race-domain");
    let srv = fake::gated().await;
    let fw = make(&dir, Some(TOKEN));
    *fw.fake_base.lock().unwrap() = Some(srv.base.clone());
    fw.set_domain("dn-old.firewalla.net", NOW).unwrap();
    let f = fw.clone();
    let h = tokio::spawn(async move { f.poll_if_due(&want(), NOW).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    fw.set_domain("dn-new.firewalla.net", NOW + 1).unwrap();
    srv.gate.add_permits(10);
    h.await.unwrap();
    let st = fw.status(NOW + 2);
    assert_eq!((st.sw.is_none(), st.last_ok_ms), (true, None), "an answer from the old domain isn't the new domain's");
}

#[tokio::test]
async fn a_failed_poll_keeps_the_last_answer_and_says_why() {
    let (fw, fake, _dir) = rig("keep").await;
    fw.poll_if_due(&want(), NOW).await;
    fake.set_status(500, None);
    fw.poll_if_due(&want(), NOW + 15 * MIN).await;
    let st = fw.status(NOW + 15 * MIN + 1);
    assert_eq!((st.state.as_str(), st.last_ok_ms), ("http", Some(NOW)));
    assert!(st.sw.is_some(), "the panel judges its age");
    assert!(st.message.unwrap().contains("HTTP 500"));
}

#[tokio::test]
async fn a_refused_token_waits_an_hour_or_for_a_new_token() {
    let (fw, fake, _dir) = rig("auth").await;
    fake.set_status(401, None);
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(fw.status(NOW + 1).state, "auth");
    let asked = fake.requests().len();
    fw.poll_if_due(&want(), NOW + 59 * MIN).await;
    assert_eq!(fake.requests().len(), asked);
    fake.clear_status();
    fw.token_changed(true, NOW + 59 * MIN);
    fw.poll_if_due(&want(), NOW + 59 * MIN).await;
    assert_eq!(fw.status(NOW + 59 * MIN + 1).state, "ok", "a new token gets its own try");
}

#[tokio::test]
async fn a_redirect_is_not_followed_so_the_token_stays_home() {
    let (fw, fake, _dir) = rig("redirect").await;
    let (elsewhere, other) = fake::start().await;
    fake.0.lock().unwrap().redirect = Some(format!("{elsewhere}/v2/boxes"));
    fw.poll_if_due(&want(), NOW).await;
    assert!(other.requests().is_empty(), "the other host saw nothing");
    assert_eq!(fw.status(NOW + 1).state, "http");
}

// ---- 429

#[test]
fn a_429_backs_off_at_least_an_hour_doubling_and_retry_after_can_only_add() {
    let mut s = Schedule::default();
    s.rate_limited(NOW, None);
    assert_eq!(s.next_ms, NOW + HOUR, "at least an hour");
    s.rate_limited(NOW, None);
    assert_eq!(s.next_ms, NOW + 2 * HOUR);
    s.rate_limited(NOW, None);
    assert_eq!(s.next_ms, NOW + 4 * HOUR);
    for _ in 0..20 {
        s.rate_limited(NOW, None);
    }
    assert_eq!(s.next_ms, NOW + BACKOFF_CAP_MS, "up to a day, the quota's likely reset");
    assert!(s.limited(NOW + 1) && !s.limited(NOW + BACKOFF_CAP_MS));

    let mut s = Schedule::default();
    s.rate_limited(NOW, Some(60));
    assert_eq!(s.next_ms, NOW + HOUR, "a short Retry-After doesn't shorten the hour");
    let mut s = Schedule::default();
    s.rate_limited(NOW, Some(7 * 3600));
    assert_eq!(s.next_ms, NOW + 7 * HOUR, "a long one is honoured");
    let mut s = Schedule::default();
    s.rate_limited(NOW, Some(u64::MAX));
    assert_eq!(s.next_ms, NOW + 7 * 24 * HOUR, "a silly one can't silence it for good");
}

#[test]
fn retry_after_reads_seconds_and_dates() {
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert_eq!(parse_retry_after("3600", now), Some(3600));
    assert_eq!(parse_retry_after(" 120 ", now), Some(120));
    let later = httpdate::fmt_http_date(now + Duration::from_secs(5400));
    assert_eq!(parse_retry_after(&later, now), Some(5400));
    let earlier = httpdate::fmt_http_date(now - Duration::from_secs(60));
    assert_eq!(parse_retry_after(&earlier, now), Some(0));
    assert_eq!(parse_retry_after("soon", now), None);
    assert_eq!(parse_retry_after("-5", now), None);
}

#[tokio::test]
async fn a_lockout_stops_every_request_until_it_ends() {
    let (fw, fake, _dir) = rig("429").await;
    fake.set_status(429, Some("7200"));
    fw.poll_if_due(&want(), NOW).await;
    let st = fw.status(NOW + 1);
    assert_eq!(st.state, "rate-limited");
    assert!(st.message.unwrap().starts_with("Firewalla's cloud asked SparkBox to wait"));
    assert_eq!(st.next_poll_ms, Some(NOW + 2 * HOUR), "Retry-After wins over the hour");
    let asked = fake.requests().len();

    // Neither the poll, nor a new token, nor Test connection may ask.
    fw.poll_if_due(&want(), NOW + HOUR).await;
    fw.token_changed(true, NOW + HOUR);
    fw.poll_if_due(&want(), NOW + HOUR + 1).await;
    let t = fw.test(&want(), NOW + HOUR).await;
    assert_eq!((t.ok, t.kind.as_deref()), (false, Some("rate-limited")));
    assert_eq!(fake.requests().len(), asked, "not one request during the lockout");

    // Past it, one try. Still locked out: the wait doubles.
    fw.poll_if_due(&want(), NOW + 2 * HOUR).await;
    assert_eq!(fake.requests().len(), asked + 1);
    assert_eq!(fw.status(NOW + 2 * HOUR + 1).next_poll_ms, Some(NOW + 4 * HOUR));

    // It recovers by itself, and the next wait is the normal one.
    fake.clear_status();
    fw.poll_if_due(&want(), NOW + 4 * HOUR).await;
    let st = fw.status(NOW + 4 * HOUR + 1);
    assert_eq!((st.state.as_str(), st.next_poll_ms), ("ok", Some(NOW + 4 * HOUR + 15 * MIN)));
}

#[tokio::test]
async fn a_503_counts_as_a_429() {
    let (fw, fake, _dir) = rig("503").await;
    fake.set_status(503, None);
    fw.poll_if_due(&want(), NOW).await;
    assert_eq!(fw.status(NOW + 1).state, "rate-limited");
}

#[tokio::test]
async fn a_restart_can_not_ask_sooner() {
    let (fw, fake, dir) = rig("restart").await;
    fake.set_status(429, Some("3600"));
    fw.poll_if_due(&want(), NOW).await;
    let asked = fake.requests().len();
    drop(fw);
    let again = make(&dir, Some(TOKEN));
    *again.fake_base.lock().unwrap() = Some(fake.base());
    again.poll_if_due(&want(), NOW + 5 * MIN).await;
    assert_eq!(fake.requests().len(), asked, "the backoff survives");
    assert_eq!(again.status(NOW + 5 * MIN).state, "rate-limited");

    // And the 15 minutes do too.
    let (fw, fake, dir) = rig("restart-15").await;
    fw.poll_if_due(&want(), NOW).await;
    let asked = fake.requests().len();
    let last = fw.status(NOW + 1).sw;
    drop(fw);
    let again = make(&dir, Some(TOKEN));
    *again.fake_base.lock().unwrap() = Some(fake.base());
    again.poll_if_due(&want(), NOW + MIN).await;
    assert_eq!(fake.requests().len(), asked);
    assert_eq!(again.status(NOW + MIN).sw, last, "and the panel keeps the last answer");
}

#[tokio::test]
async fn test_connection_reports_and_counts_as_a_poll_but_is_limited_to_one_in_5_minutes() {
    let (fw, fake, _dir) = rig("test").await;
    let t = fw.test(&want(), NOW).await;
    assert!(t.ok, "{t:?}");
    assert_eq!(t.message, "Connected. Firewalla reports 12 ports on Firewalla.Switch.X, 2 with a link.");
    let asked = fake.requests().len();
    for wait in [30_000, MIN, 4 * MIN, 5 * MIN - 1] {
        let t = fw.test(&want(), NOW + wait).await;
        assert_eq!((t.ok, t.kind.as_deref()), (false, Some("wait")), "{wait}");
    }
    assert_eq!(fake.requests().len(), asked);
    fw.poll_if_due(&want(), NOW + 2 * MIN).await;
    assert_eq!(fake.requests().len(), asked, "a test resets the 15 minutes");
    assert!(fw.test(&want(), NOW + 5 * MIN).await.ok);
    // An hour of clicking once a minute: 12 tests at most, on the one box already found.
    let (fw, fake, _dir) = rig("test-hour").await;
    for i in 0..60 {
        fw.test(&want(), NOW + i * MIN).await;
    }
    assert_eq!(fake.requests().len(), 13, "{:?}", fake.requests());
}

#[tokio::test]
async fn a_new_domain_forgets_the_old_answer_but_not_a_backoff() {
    let (fw, fake, _dir) = rig("domain").await;
    fw.poll_if_due(&want(), NOW).await;
    assert!(fw.status(NOW + 1).sw.is_some());
    fw.set_domain("dn-other.firewalla.net", NOW + 1).unwrap();
    let st = fw.status(NOW + 2);
    assert_eq!((st.state.as_str(), st.sw.is_none(), st.domain.as_deref()), ("waiting", true, Some("dn-other.firewalla.net")));
    assert_eq!(fw.set_domain("evil.com", NOW + 2).unwrap_err(), "That isn't an MSP domain. It looks like dn-abc123.firewalla.net.");
    assert_eq!(fw.domain().as_deref(), Some("dn-other.firewalla.net"), "a refused domain changes nothing");

    // A new domain gets its own try, within the 5-minute floor.
    fake.set_status(429, None);
    fw.poll_if_due(&want(), NOW + 4 * MIN).await;
    assert_eq!(fw.status(NOW + 4 * MIN).state, "waiting", "not yet");
    fw.poll_if_due(&want(), NOW + 5 * MIN).await;
    fw.set_domain("dn-third.firewalla.net", NOW + 6 * MIN).unwrap();
    assert_eq!(fw.status(NOW + 6 * MIN).state, "rate-limited", "the quota doesn't care what the domain is called");
    fw.set_domain("", NOW + 6 * MIN).unwrap();
    assert_eq!((fw.domain(), fw.status(NOW).state.as_str()), (None, "off"));
}

#[tokio::test]
async fn removing_the_token_drops_what_it_read() {
    let (fw, _fake, _dir) = rig("forget").await;
    fw.poll_if_due(&want(), NOW).await;
    fw.token_changed(false, NOW + 1);
    assert!(fw.status(NOW + 2).sw.is_none());
}

// ---- Labels

#[test]
fn labels_are_checked_stamped_saved_and_read_back() {
    let dir = temp("labels");
    let fw = make(&dir, None);
    let nodes = vec!["cell1".to_string()];
    let set = |port, l: PortLabel| fw.set_label("firewalla", port, Some(&l), &nodes, NOW, true);

    let saved = set(3, label(LabelKind::Node, Some("cell1"), Some("stray words"))).unwrap().unwrap();
    assert_eq!((saved.kind, saved.node.as_deref(), saved.text.as_deref(), saved.set_at_ms), (LabelKind::Node, Some("cell1"), None, NOW));
    assert_eq!(set(12, label(LabelKind::Empty, Some("cell1"), Some("x"))).unwrap().unwrap().node, None, "only what the kind uses is kept");
    let typed = set(4, label(LabelKind::Text, None, Some("  NAS\u{7} in the closet  "))).unwrap().unwrap();
    assert_eq!(typed.text.as_deref(), Some("NAS in the closet"));
    for kind in [LabelKind::Crs804, LabelKind::Gold, LabelKind::Mac] {
        assert_eq!(set(5, label(kind, None, None)).unwrap().unwrap().kind, kind);
    }

    assert_eq!(set(3, label(LabelKind::Node, Some("cell9"), None)).unwrap_err(), "That node isn't in the inventory.");
    assert_eq!(set(3, label(LabelKind::Node, None, None)).unwrap_err(), "That node isn't in the inventory.");
    assert_eq!(set(3, label(LabelKind::Text, None, Some("   "))).unwrap_err(), "Type what's plugged in.");
    assert!(set(3, label(LabelKind::Text, None, Some(&"x".repeat(61)))).is_err());
    assert_eq!(set(0, label(LabelKind::Empty, None, None)).unwrap_err(), "The Switch X has no port 0.");
    assert_eq!(set(13, label(LabelKind::Empty, None, None)).unwrap_err(), "The Switch X has no port 13.");

    let again = make(&dir, None);
    let back = again.labels_of("firewalla", true);
    assert_eq!(back.keys().copied().collect::<Vec<_>>(), [3, 4, 5, 12]);
    assert_eq!(back[&3].node.as_deref(), Some("cell1"), "the refused change left it");

    fw.set_label("firewalla", 3, None, &nodes, NOW, true).unwrap();
    assert!(!make(&dir, None).labels_of("firewalla", true).contains_key(&3), "cleared on disk too");
}

#[test]
fn demo_labels_stay_in_memory_and_the_samples_are_valid() {
    let dir = temp("demo-labels");
    let fw = make(&dir, None);
    assert!(fw.labels_of("firewalla", true).is_empty(), "the owner's file is empty");
    let sample = fw.labels_of("firewalla", false);
    assert_eq!(sample[&5].node.as_deref(), Some("gx10-0000"));
    fw.set_label("firewalla", 8, Some(&label(LabelKind::Empty, None, None)), &[], NOW, false).unwrap();
    assert!(fw.labels_of("firewalla", false).contains_key(&8));
    assert!(!dir.join("firewalla_ports.json").exists(), "Demo writes no file");
}

#[test]
fn the_demo_sample_shows_every_state_the_panel_has() {
    let st = demo::sample_status(NOW);
    assert_eq!((st.configured, st.state.as_str(), st.last_ok_ms), (true, "ok", Some(NOW - 4 * MIN)));
    let sw = st.sw.unwrap();
    assert_eq!(sw.ports.len(), 12);
    let speeds: Vec<Option<u32>> = sw.ports.iter().map(|p| p.link_speed_mbps).collect();
    for s in [10_000, 5_000, 2_500, 1_000, 100] {
        assert!(speeds.contains(&Some(s)), "{s}");
    }
    assert!(sw.ports.iter().any(|p| p.link_up == Some(true) && p.link_speed_mbps.is_none()), "the neutral state");
    assert!(sw.ports.iter().any(|p| p.link_up == Some(false)));
    assert!(sw.ports.iter().any(|p| p.poe_watts.is_some()) && sw.ports.iter().any(|p| p.sfp.is_some()));
}

#[test]
fn the_token_is_a_credential_the_ui_may_save_and_the_masking_hides_its_form() {
    assert_eq!(crate::named_credential(TOKEN_ACCOUNT), Some("Firewalla MSP token"));
    assert_eq!(crate::named_credential("firewalla:other"), None);
    let header = format!("Authorization: Token {TOKEN}");
    let bare = format!("sent Token {TOKEN} to the cloud");
    let env = format!("FIREWALLA_MSP_TOKEN={TOKEN}");
    for text in [header, bare, env] {
        assert!(!crate::live::audit::redact(&text).contains(TOKEN), "{text}");
    }
    assert_eq!(crate::live::audit::redact("Token limit reached"), "Token limit reached");
}

/// A failure's sentence goes to the panel and the log: it must never carry the token.
#[tokio::test]
async fn no_failure_message_holds_the_token() {
    for status in [401, 403, 429, 500, 503] {
        let (fw, fake, _dir) = rig("words").await;
        fake.set_status(status, None);
        let t = fw.test(&want(), NOW).await;
        assert!(!t.ok && !t.message.contains(TOKEN), "{t:?}");
        assert!(!format!("{:?}", fw.status(NOW + 1)).contains(TOKEN));
    }
    // Nobody listening: the transport's own words.
    let (fw, _fake, _dir) = rig("words-down").await;
    *fw.fake_base.lock().unwrap() = Some("http://127.0.0.1:9".into());
    let t = fw.test(&want(), NOW).await;
    assert_eq!(t.kind.as_deref(), Some("unreachable"));
    assert!(!t.message.contains(TOKEN), "{t:?}");
}

#[test]
fn the_switch_x_is_known_by_its_lldp_system_name() {
    assert!(is_firewalla_switch("Firewalla-Switch-X"));
    assert!(is_firewalla_switch("firewalla switch x"));
    assert!(!is_firewalla_switch("Firewalla-Gold"));
    assert!(!is_firewalla_switch("CRS804"));
    assert!(!is_firewalla_switch("Firewalla-Switch-SE"), "another Firewalla switch is another switch");
    assert!(!is_firewalla_switch(""));
}

#[test]
fn only_the_switch_x_s_chassis_is_looked_for() {
    use crate::model::{LldpNeighbor, NetStats};
    let peer = |system: &str, mac: &str| LldpNeighbor { system: Some(system.into()), chassis_id: Some(mac.into()), ..Default::default() };
    let nodes = |peers: Vec<LldpNeighbor>| vec![NodeSnapshot { nets: vec![NetStats { lldp_peers: peers, ..Default::default() }], ..Default::default() }];
    let lan = nodes(vec![peer("Firewalla-Switch-X", "20:6d:31:be:00:15"), peer("CRS804", "38:32:7a:79:9b:9e"), peer("Firewalla-Switch-SE", "20:6d:31:6f:a9:f6"), peer("cell2-example", "aa:bb:cc:00:00:02")]);
    assert_eq!(heard_macs(&lan), ["20:6d:31:be:00:15"]);
    let twice = [lan.clone(), lan].concat();
    assert_eq!(heard_macs(&twice).len(), 1, "every node hears it, and it's one MAC");
}
