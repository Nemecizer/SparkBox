//! The RouterOS client and poller against the fake REST API.

use std::time::Duration;

use super::commands::test_routeros;
use super::fake::fake_routeros;
use super::*;
use crate::model::NodeSnapshot;

fn switches(name: &str, password: &'static str) -> Arc<Switches> {
    let dir = std::env::temp_dir().join(format!("sparkbox-ros-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    Switches::new_plain_http(dir.join("switch_certs.json"), vpn, move |_| Ok(Some(password.to_string())))
}

fn crs(port: u16) -> SwitchConfig {
    SwitchConfig { id: "crs804".into(), name: "CRS804 DDQ".into(), kind: SwitchKind::RouterOs, host: format!("127.0.0.1:{port}"), user: Some("sparkbox".into()) }
}

fn nodes() -> Vec<NodeSnapshot> {
    ["cell1", "cell2", "cell3"].iter().map(|n| NodeSnapshot { id: n.to_string(), name: n.to_string(), ..Default::default() }).collect()
}

#[tokio::test]
async fn changing_only_the_user_rebuilds_the_authenticated_client() {
    let ros = fake_routeros().await;
    let sw = switches("changed-user", "secret");
    let mut cfg = crs(ros.port);
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Online);
    ros.state.lock().unwrap().user = "new-reader".into();
    cfg.user = Some("new-reader".into());
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Online, "same password, different account");
    cfg.user = Some("not-authorized".into());
    let refused = sw.poll(&cfg, &nodes()).await;
    assert_eq!(refused.status, LinkStatus::Error, "never use the previous account's access");
    assert!(refused.error.unwrap().contains("not-authorized"));
}

#[tokio::test]
async fn optional_poll_reads_cannot_hide_a_login_refusal() {
    for path in ["/system/health", "/interface/ethernet/monitor", "/interface/ethernet/print", "/interface/bridge/host", "/interface/ethernet/switch/host", "/ip/neighbor", "/ip/address", "/ip/arp"] {
        let ros = fake_routeros().await;
        let sw = switches(&format!("optional-{}", path.replace('/', "-")), "secret");
        ros.state.lock().unwrap().fail_path = Some((format!("/rest{path}"), 401));
        let snap = sw.poll(&crs(ros.port), &nodes()).await;
        assert_eq!(snap.status, LinkStatus::Error, "{path} must not look Online");
        assert!(snap.error.unwrap().contains("401"));
        assert_eq!(ros.state.lock().unwrap().requests.last().unwrap(), &format!("{} /rest{path}", if path.ends_with("monitor") || path.ends_with("print") { "POST" } else { "GET" }), "stop at the refusal");
    }
}

#[tokio::test]
async fn login_refusal_waits_without_hiding_error_and_explicit_test_can_recover() {
    let ros = fake_routeros().await;
    let sw = switches("auth-wait", "secret");
    let cfg = crs(ros.port);
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Online);
    ros.state.lock().unwrap().fail_path = Some(("/rest/system/resource".into(), 401));
    let failed = sw.poll(&cfg, &nodes()).await;
    assert_eq!(failed.status, LinkStatus::Error);
    assert!(failed.error.as_ref().unwrap().contains("30 seconds"));
    assert!(!failed.ports.is_empty());
    assert!(failed.ports.iter().all(|p| p.rx_bps == 0.0 && p.tx_bps == 0.0));
    let calls = ros.state.lock().unwrap().requests.len();
    let client = sw.client(&cfg).await.unwrap();
    assert!(client.auth_retry_pending(Instant::now()));
    assert!(!client.auth_retry_pending(Instant::now() + Duration::from_secs(30)));
    for _ in 0..10 {
        let waiting = sw.poll(&cfg, &nodes()).await;
        assert_eq!(waiting.status, LinkStatus::Error);
        assert_eq!(waiting.error, failed.error);
    }
    assert_eq!(ros.state.lock().unwrap().requests.len(), calls, "no automatic requests during cooldown");
    assert!(!test_routeros(&sw, &cfg).await.ok, "manual test makes one attempt, not an auth bypass");
    assert_eq!(ros.state.lock().unwrap().requests.len(), calls + 1);
    ros.state.lock().unwrap().fail_path = None;
    assert!(test_routeros(&sw, &cfg).await.ok);
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Online);
}

#[tokio::test]
async fn changed_or_missing_saved_password_never_reuses_old_access() {
    let ros = fake_routeros().await;
    let saved = Arc::new(Mutex::new(Ok(Some("wrong".to_string()))));
    let password = saved.clone();
    let dir = std::env::temp_dir().join(format!("sparkbox-ros-password-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let sw = Switches::new_plain_http(dir.join("pins.json"), vpn, move |_| password.lock().unwrap().clone());
    let cfg = crs(ros.port);
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Error);
    *saved.lock().unwrap() = Ok(Some("secret".into()));
    assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Online, "changed credential retries now");
    let calls = ros.state.lock().unwrap().requests.len();
    for unavailable in [Ok(None), Err("Keychain unavailable".into())] {
        *saved.lock().unwrap() = unavailable;
        assert_eq!(sw.poll(&cfg, &nodes()).await.status, LinkStatus::Error);
        assert_eq!(ros.state.lock().unwrap().requests.len(), calls, "no empty/stale credential fallback");
    }
}

#[tokio::test]
async fn unsupported_optional_data_still_allows_monitoring() {
    let ros = fake_routeros().await;
    let sw = switches("optional-unsupported", "secret");
    ros.state.lock().unwrap().fail_path = Some(("/rest/system/health".into(), 404));
    assert_eq!(sw.poll(&crs(ros.port), &nodes()).await.status, LinkStatus::Online);
    for e in [ApiError::Auth, ApiError::TlsUntrusted { fingerprint: "new".into() }, ApiError::TlsChanged { pinned: "old".into(), fingerprint: "new".into() }] {
        assert_eq!(client::optional_read::<Vec<()>>(Err(e.clone())), Err(e));
    }
}

#[tokio::test]
async fn polling_reads_health_ports_optics_and_rates() {
    let ros = fake_routeros().await;
    let sw = switches("poll", "secret");
    let first = sw.poll(&crs(ros.port), &nodes()).await;
    assert_eq!((first.status, first.configured), (LinkStatus::Online, true));
    assert_eq!(first.board.as_deref(), Some("CRS804-4DDQ"));
    assert_eq!(first.version.as_deref(), Some("7.20.2 (stable)"));
    assert_eq!(first.uptime_s, Some(6 * 604_800 + 86_400 + 2 * 3_600 + 55 * 60 + 14));
    assert_eq!(first.sensors.iter().filter(|s| s.unit == "RPM").count(), 2);
    assert!(first.sensors.iter().any(|s| s.name == "PSU2" && s.ok));
    assert_eq!(first.temp_c, Some(58.0));

    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = sw.poll(&crs(ros.port), &nodes()).await;
    let p = s.ports.iter().find(|p| p.name == "qsfp56-dd-1-1").unwrap();
    // 25 MB in ~0.5 s ≈ 400 Mb/s received, twice that sent.
    assert!((300e6..=450e6).contains(&p.rx_bps), "{}", p.rx_bps);
    assert!(p.tx_bps > p.rx_bps * 1.8);
    assert_eq!(p.node.as_deref(), Some("cell1"), "mapped by comment");
    assert_eq!(s.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap().node.as_deref(), Some("cell2"), "a comment can say more than the name");
    assert_eq!((p.mtu, p.capacity_mbps, p.speed.as_str()), (Some(9216), 200_000, "200G"));
    assert_eq!(p.lane_rx_power_dbm, vec![-1.9, -2.0, -2.2, -2.1]);
    assert_eq!(p.rx_power_dbm, Some(-2.2), "the weakest lane");
    assert_eq!((p.sfp_vendor.as_deref(), p.module_temp_c), (Some("MikroTik"), Some(45.0)));
    assert_eq!(s.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap().fcs_errors, 5);
    assert!(p.last_link_up_ms.is_some());
    let idle = s.ports.iter().find(|p| p.name == "qsfp56-dd-2-1").unwrap();
    assert!(!idle.running && idle.node.is_none() && idle.lane_rx_power_dbm.is_empty());
}

#[tokio::test]
async fn a_counter_reset_never_shows_a_negative_or_huge_rate() {
    let ros = fake_routeros().await;
    let sw = switches("reset", "secret");
    sw.poll(&crs(ros.port), &nodes()).await;
    for p in ros.state.lock().unwrap().ports.iter_mut() {
        p.rx_byte = 0;
        p.tx_byte = 0;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = sw.poll(&crs(ros.port), &nodes()).await;
    let p = s.ports.iter().find(|p| p.name == "qsfp56-dd-1-1").unwrap();
    assert!(p.rx_bps >= 0.0 && p.rx_bps < 600e6, "{}", p.rx_bps);
}

#[tokio::test]
async fn a_wrong_password_and_a_missing_api_are_explained() {
    let ros = fake_routeros().await;
    let sw = switches("auth", "wrong");
    let r = test_routeros(&sw, &crs(ros.port)).await;
    assert_eq!((r.ok, r.error_kind.as_deref()), (false, Some("auth")));
    assert!(r.message.contains("refused user \"sparkbox\" (401)"), "{}", r.message);
    let s = sw.poll(&crs(ros.port), &nodes()).await;
    assert_eq!(s.status, LinkStatus::Error);
    assert!(s.error.unwrap().contains("401"));

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let r = test_routeros(&switches("closed", "secret"), &crs(closed)).await;
    assert_eq!(r.error_kind.as_deref(), Some("rest-disabled"), "{}", r.message);
    assert!(r.message.contains("www-ssl"));

    let ok = test_routeros(&switches("ok", "secret"), &crs(ros.port)).await;
    assert!(ok.ok && ok.message.contains("CRS804-4DDQ"), "{}", ok.message);
}

#[tokio::test]
async fn ports_enable_disable_and_take_comments() {
    let ros = fake_routeros().await;
    let sw = switches("write", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    let c = sw.client(&cfg).await.unwrap();
    let id = sw.port_id(&cfg, "qsfp56-dd-1-2").await.unwrap();
    assert_eq!(id, "*2");

    c.set_disabled(&id, true).await.unwrap();
    let s = sw.poll(&cfg, &nodes()).await;
    let p = s.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap();
    assert!(p.disabled && !p.running);
    assert_eq!(p.link_downs, 2);
    c.set_disabled(&id, false).await.unwrap();
    assert!(!sw.poll(&cfg, &nodes()).await.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap().disabled, "round trip");

    c.set_comment(&id, "cell3").await.unwrap();
    assert_eq!(sw.poll(&cfg, &nodes()).await.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap().node.as_deref(), Some("cell3"));
    c.blink("qsfp56-dd-1-1").await.unwrap();
    c.reset_counters("qsfp56-dd-1-2").await.unwrap();
    let st = ros.state.lock().unwrap();
    assert_eq!(st.blinks, vec!["qsfp56-dd-1-1".to_string()]);
    assert_eq!(st.ports[1].rx_error, 0);
    assert!(st.requests.iter().any(|r| r == "PATCH /rest/interface/ethernet/*2"));
}

#[tokio::test]
async fn an_unreachable_switch_keeps_its_ports_but_not_their_rates() {
    let ros = fake_routeros().await;
    let sw = switches("gone", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sw.poll(&cfg, &nodes()).await.ports[0].rx_bps > 0.0);
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = SwitchConfig { host: format!("127.0.0.1:{closed}"), ..cfg };
    // Same switch id, new (dead) address: an error, the last ports, no rates.
    let s = sw.poll(&gone, &nodes()).await;
    assert_ne!(s.status, LinkStatus::Online);
    assert!(s.error.is_some() && s.ports.iter().all(|p| p.rx_bps == 0.0 && p.tx_bps == 0.0));
}

#[test]
fn unconfigured_switches_are_never_online() {
    let cfg = SwitchConfig { host: String::new(), ..crs(1) };
    let s = placeholder_switch(&cfg);
    assert_eq!((s.status, s.configured), (LinkStatus::Offline, false));
    assert_eq!(placeholder_switch(&SwitchConfig { host: "192.168.50.2".into(), ..cfg }).status, LinkStatus::Connecting);
}

#[test]
fn certificates_are_trusted_by_address_and_checked() {
    let sw = switches("pins", "secret");
    assert!(sw.trust("192.168.50.2", "not a fingerprint").is_err());
    let fp = client::fingerprint(b"cert");
    sw.trust("192.168.50.2", &client::pretty_fingerprint(&fp)).unwrap();
    assert_eq!(sw.pin_for("192.168.50.2").as_deref(), Some(fp.as_str()));
    assert_eq!(sw.pin_for("192.168.50.9"), None);
}

#[test]
fn only_a_certificate_failure_offers_a_certificate() {
    use client::ApiError::*;
    assert_eq!(TlsUntrusted { fingerprint: "ab".into() }.cert_to_trust(), Some("ab"));
    assert_eq!(TlsChanged { pinned: "cd".into(), fingerprint: "ab".into() }.cert_to_trust(), Some("ab"));
    // Past a pinned certificate the client has seen one, and there's still nothing to trust.
    for e in [Auth, RestDisabled("off".into()), Timeout, Unreachable("no route".into()), Other("?".into())] {
        assert_eq!(e.cert_to_trust(), None, "{}", e.kind());
    }
}

#[test]
fn comments_name_nodes_by_name_id_or_hostname() {
    let mut n = nodes();
    n[2].system.hostname = "cell3-example".into();
    assert_eq!(node_for_comment("cell1", &n).as_deref(), Some("cell1"));
    assert_eq!(node_for_comment("to CELL2 port 1", &n).as_deref(), Some("cell2"));
    assert_eq!(node_for_comment("cell3-example.local", &n).as_deref(), Some("cell3"));
    assert_eq!(node_for_comment("cell10", &n), None);
    assert_eq!(node_for_comment("", &n), None);
}

// ---- Port mapping tables and switch-management safety (r2-fabric)

fn fabric_node(id: &str, i: u8) -> NodeSnapshot {
    use crate::model::{NetRole, NetStats};
    NodeSnapshot {
        id: id.into(),
        name: id.into(),
        status: LinkStatus::Online,
        nets: (0..2)
            .map(|h| NetStats {
                iface: if h == 0 { "enp1s0f1np1".into() } else { "enP2p1s0f1np1".into() },
                role: NetRole::Fabric,
                up: true,
                ip: Some(format!("192.168.{}.{i}", 100 + h)),
                mac: Some(format!("94:6d:ae:00:0{h}:0{i}")),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn tables_place_nodes_by_mac_and_find_the_management_port() {
    let ros = fake_routeros().await;
    {
        let mut s = ros.state.lock().unwrap();
        // cell2's cable is in 1-1 now, although 1-1's comment still says cell1.
        s.hosts = vec![
            ("94:6D:AE:00:00:02".into(), "qsfp56-dd-1-1".into()),
            ("94:6D:AE:00:01:02".into(), "qsfp56-dd-1-1".into()),
            ("20:6d:31:c4:59:8c".into(), "ether1".into()),
        ];
        s.neighbors = vec![("qsfp56-dd-1-2".into(), "cell1".into(), "94:6D:AE:00:00:99".into())];
        s.arp = vec![("192.168.50.1".into(), "20:6d:31:c4:59:8c".into(), "bridge".into())];
    }
    let sw = switches("tables", "secret");
    let cfg = crs(ros.port);
    let nodes = vec![fabric_node("cell1", 1), fabric_node("cell2", 2)];
    let snap = sw.poll(&cfg, &nodes).await;
    let t = sw.tables("crs804").expect("tables read on the first poll");
    assert_eq!(t.hosts.len(), 3);
    assert_eq!(t.neighbors[0].port, "qsfp56-dd-1-2", "\"qsfp56-dd-1-2,bridge\" is port 1-2");
    assert_eq!(t.arp, vec![("192.168.50.1".to_string(), "20:6D:31:C4:59:8C".to_string(), "bridge".to_string())]);
    assert!(ros.state.lock().unwrap().requests.iter().any(|r| r == "GET /rest/interface/ethernet/switch/host"), "the switch chip's table is asked for too");

    // The fabric monitor places the nodes and marks the management port.
    let gold = SwitchSnapshot { id: "gold".into(), kind: SwitchKind::Gateway, host: "192.168.50.1".into(), configured: true, ..Default::default() };
    let mut cluster = ClusterSnapshot { ts_ms: 1, nodes, switches: vec![snap, gold], ..Default::default() };
    let mon = crate::fabric::FabricMonitor::new(None);
    let tables = [("crs804".to_string(), t)].into();
    mon.assess(&mut cluster, &tables, None);
    let ports = &cluster.switches[0].ports;
    let p = |n: &str| ports.iter().find(|p| p.name == n).unwrap();
    assert_eq!((p("qsfp56-dd-1-1").node.as_deref(), p("qsfp56-dd-1-1").mapped_by.as_deref()), (Some("cell2"), Some("mac")));
    assert_eq!((p("qsfp56-dd-1-2").node.as_deref(), p("qsfp56-dd-1-2").mapped_by.as_deref()), (Some("cell1"), Some("lldp")));
    assert!(p("ether1").management.as_deref().unwrap().contains("192.168.50.1"), "{:?}", p("ether1").management);
    assert!(p("qsfp56-dd-1-1").management.is_none());
    assert_eq!(cluster.nodes[1].nets[1].switch_port.as_deref(), Some("crs804/qsfp56-dd-1-1"));
}

#[tokio::test]
async fn a_disable_leaves_a_revert_on_the_switch_first() {
    let ros = fake_routeros().await;
    let sw = switches("revert", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    let at = safety::disable(&sw, &cfg, "qsfp56-dd-1-2").await.unwrap();
    assert!(at > crate::demo::unix_now() * 1000 + 55_000);
    {
        let s = ros.state.lock().unwrap();
        let put = s.requests.iter().position(|r| r == "PUT /rest/system/scheduler").expect("revert scheduled");
        let patch = s.requests.iter().position(|r| r == "PATCH /rest/interface/ethernet/*2").expect("port disabled");
        assert!(put < patch, "the revert is on the switch before the port goes down: {:?}", s.requests);
        assert_eq!(s.scheduler.len(), 1);
        // One shot, 60 s ahead by the switch's own clock (23:59:40 there: tomorrow), never at once.
        let e = &s.scheduler[0];
        assert_eq!((e.name.as_str(), e.interval.as_str()), ("sparkbox-revert-qsfp56-dd-1-2", "0s"));
        assert_eq!((e.start_date.as_str(), e.start_time.as_str(), e.next_run.as_str()), ("2026-10-30", "00:00:40", "2026-10-30 00:00:40"));
        assert!(s.ran_at_creation.is_empty(), "the revert didn't run when it was added");
        let clock = s.requests.iter().position(|r| r == "GET /rest/system/clock").expect("the switch's clock was read");
        let back = s.requests.iter().position(|r| r.starts_with("GET /rest/system/scheduler/")).expect("next-run read back");
        assert!(clock < put && put < back && back < patch, "{:?}", s.requests);
        assert!(s.ports[1].disabled);
    }
    // The snapshot says when it comes back.
    let snap = sw.poll(&cfg, &nodes()).await;
    assert_eq!(snap.ports.iter().find(|p| p.name == "qsfp56-dd-1-2").unwrap().revert_at_ms, Some(at));
    // A minute later the switch re-enables it by itself and the entry removes itself.
    let mut s = ros.state.lock().unwrap();
    assert!(s.fire("sparkbox-revert-qsfp56-dd-1-2"));
    assert!(!s.ports[1].disabled);
    assert!(s.scheduler.is_empty());
    assert_eq!(s.transitions, ["disable qsfp56-dd-1-2", "enable qsfp56-dd-1-2"]);
}

#[tokio::test]
async fn keep_disabled_removes_the_revert_and_enable_clears_it() {
    let ros = fake_routeros().await;
    let sw = switches("keep", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    safety::disable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap();
    safety::keep_disabled(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap();
    assert!(ros.state.lock().unwrap().scheduler.is_empty());
    assert!(ros.state.lock().unwrap().ports[0].disabled, "kept");
    assert_eq!(sw.poll(&cfg, &nodes()).await.ports[0].revert_at_ms, None);
    // Disable again, then enable by hand: the waiting revert goes too.
    safety::disable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap();
    safety::enable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap();
    let s = ros.state.lock().unwrap();
    assert!(s.scheduler.is_empty() && !s.ports[0].disabled);
}

#[tokio::test]
async fn the_revert_is_refused_when_the_switch_would_run_it_at_another_time() {
    let ros = fake_routeros().await;
    ros.state.lock().unwrap().skew_next_run = true;
    let sw = switches("skew", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    let e = safety::disable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap_err();
    assert!(e.contains("reported a next run of 2026-10-30 00:05:40 for the revert instead of 2026-10-30 00:00:40, so it was removed"), "{e}");
    let s = ros.state.lock().unwrap();
    assert!(s.scheduler.is_empty(), "the odd entry is gone");
    assert!(!s.ports[0].disabled);
    assert!(!s.requests.iter().any(|r| r.starts_with("PATCH")), "{:?}", s.requests);
}

#[tokio::test]
async fn older_routeros_dates_are_read_and_written_its_way() {
    let ros = fake_routeros().await;
    {
        let mut s = ros.state.lock().unwrap();
        s.old_dates = true;
        s.clock = ("2026-02-28".into(), "12:00:00".into());
    }
    let sw = switches("olddates", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    safety::disable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap();
    let s = ros.state.lock().unwrap();
    assert_eq!((s.scheduler[0].start_date.as_str(), s.scheduler[0].start_time.as_str()), ("feb/28/2026", "12:01:00"));
    assert!(s.ports[0].disabled && s.ran_at_creation.is_empty());
}

#[test]
fn switch_times_roll_over_days_months_and_years() {
    use super::client::SwitchTime;
    let t = |d: &str, h: &str| SwitchTime::parse(d, h).unwrap();
    assert_eq!(t("2026-12-31", "23:59:30").plus(60).date(), "2027-01-01");
    assert_eq!(t("2028-02-28", "23:59:30").plus(60).date(), "2028-02-29", "a leap year");
    assert_eq!(t("dec/31/2026", "23:59:30").plus(60).date(), "jan/01/2027");
    assert_eq!(t("2026-10-29", "10:42:13").plus(60).time(), "10:43:13");
    assert!(SwitchTime::parse("2026-13-01", "00:00:00").is_none() && SwitchTime::parse("2026-02-30", "00:00:00").is_none());
    assert_eq!(SwitchTime::parse_stamp("oct/30/2026 00:00:40"), Some(t("oct/30/2026", "00:00:40")));
    assert_eq!(SwitchTime::parse_stamp("2026-10-30 00:00:40").map(|x| x.time()).as_deref(), Some("00:00:40"));
}

#[tokio::test]
async fn without_a_revert_nothing_is_disabled() {
    let ros = fake_routeros().await;
    ros.state.lock().unwrap().refuse_scheduler = true;
    let sw = switches("norevert", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    let e = safety::disable(&sw, &cfg, "qsfp56-dd-1-1").await.unwrap_err();
    assert!(e.starts_with("Couldn't leave a revert on the switch, so qsfp56-dd-1-1 wasn't disabled"), "{e}");
    let s = ros.state.lock().unwrap();
    assert!(!s.ports[0].disabled);
    assert!(!s.requests.iter().any(|r| r.starts_with("PATCH")), "{:?}", s.requests);
}

#[tokio::test]
async fn a_bounce_is_one_script_that_brings_the_port_back_itself() {
    let ros = fake_routeros().await;
    let sw = switches("bounce", "secret");
    let cfg = crs(ros.port);
    sw.poll(&cfg, &nodes()).await;
    safety::bounce(&sw, &cfg, "ether1", true).await.unwrap();
    let s = ros.state.lock().unwrap();
    assert_eq!(s.transitions, ["disable ether1", "enable ether1"]);
    assert!(s.scripts.is_empty(), "the script removed itself");
    assert!(!s.ports[3].disabled);
    // Never disable, wait and enable from SparkBox's side: no PATCH at all.
    assert!(!s.requests.iter().any(|r| r.starts_with("PATCH")), "{:?}", s.requests);
    assert!(s.requests.iter().any(|r| r == "POST /rest/system/script/run"));
}

#[test]
fn the_management_port_needs_the_exact_phrase() {
    let why = Some("The switch's management address 192.168.50.2 is on this port.");
    let e = safety::check_phrase("ether1", why, "disable", None).unwrap_err();
    assert!(e.contains("type \"disable ether1\""), "{e}");
    assert!(safety::check_phrase("qsfp56-dd-1-1", why, "disable", Some("disable 1-2")).is_err());
    assert!(safety::check_phrase("qsfp56-dd-1-1", why, "disable", Some(" disable 1-1 ")).is_ok());
    assert!(safety::check_phrase("qsfp56-dd-1-1", why, "bounce", Some("disable 1-1")).is_err(), "the verb matters");
    assert!(safety::check_phrase("qsfp56-dd-1-1", None, "disable", None).is_ok(), "other ports need no phrase");
    assert!(!safety::valid_port("1-1\"; /system reset-configuration"));
    assert!(safety::bounce_script("qsfp56-dd-2-1").ends_with("/system script remove [find name=\"sparkbox-bounce-qsfp56-dd-2-1\"]"));
}
