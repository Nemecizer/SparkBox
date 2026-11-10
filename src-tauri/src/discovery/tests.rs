use super::*;
use crate::wol::iface;
fn run() -> Run {
    Run {
        id: "test".into(),
        demo: true,
        scope: demo_scope(),
        started_ms: now(),
        status: "complete".into(),
        checked: 253,
        total: 253,
        message: String::new(),
        verifying: None,
        candidates: demo_candidates(),
    }
}
fn verified() -> Run {
    let mut r = run();
    for c in &mut r.candidates {
        if let Ok(f) = demo_facts(&c.address) {
            c.status = "spark".into();
            c.facts = Some(f);
            c.fingerprint = Some("shared-key".into());
        }
    }
    r
}
#[test]
fn scope_never_expands_beyond_local_private_twenty_four_or_includes_this_mac() {
    let all = vec![
        iface("en0", "192.168.50.10", "255.255.254.0"),
        iface("en5", "192.168.50.11", "255.255.255.0"),
        iface("utun4", "10.1.0.2", "255.255.255.0"),
        iface("bridge100", "192.168.64.1", "255.255.255.0"),
        iface("en2", "203.0.113.2", "255.255.255.0"),
        iface("en3", "192.168.5.1", "255.255.255.128"),
        iface("en6", "10.1.1.2", "255.0.255.0"),
    ];
    let scopes = scopes(&all);
    assert_eq!(scopes.iter().filter(|s| s.eligible).count(), 2);
    let addresses = addresses(&scopes[0], &all).unwrap();
    assert_eq!(addresses.len(), 252);
    assert_eq!(addresses.first().unwrap().to_string(), "192.168.50.1");
    assert_eq!(addresses.last().unwrap().to_string(), "192.168.50.254");
    assert!(!addresses.contains(&all[0].addr) && !addresses.contains(&all[1].addr));
    let mut changed = all.clone();
    changed[0].addr = "192.168.46.10".parse().unwrap();
    assert!(super::addresses(&scopes[0], &changed).is_err());
}
#[test]
fn lifecycle_refuses_overlap_throttles_and_drops_other_source_or_expired_results() {
    let d = Discovery::default();
    let r = d.begin(demo_scope(), false, 253).unwrap();
    assert!(d.begin(demo_scope(), false, 253).is_err());
    d.edit(&r.id, |r| r.status = "cancelled".into());
    assert!(d
        .begin(demo_scope(), false, 253)
        .unwrap_err()
        .contains("30 seconds"));
    assert!(d.view(true).is_none());
    let r = d.begin(demo_scope(), true, 253).unwrap();
    d.edit(&r.id, |r| r.started_ms = now() - TTL_MS);
    assert_eq!(d.view(true).unwrap().status, "expired");
    assert!(!d.current(&r.id, true));
    assert!(!d.current("old-live-run", false));
    assert!(d.view(false).is_none());
    assert!(
        d.view(true).is_some(),
        "an old worker must not erase the new source run"
    );
    d.edit("obsolete", |r| r.status = "complete".into());
    assert_eq!(d.view(true).unwrap().status, "expired");
}
#[test]
fn generic_ssh_and_spoofed_model_name_are_not_sparks() {
    let r = run();
    assert!(propose(
        &r,
        &["192.168.50.51".into()],
        &Inventory {
            nodes: vec![],
            switches: vec![]
        },
        &BTreeMap::new(),
        "owner"
    )
    .is_err());
    let mut f = demo_facts("192.168.50.51").unwrap();
    f.model = "NVIDIA DGX Spark".into();
    f.gpu = "NVIDIA H100".into();
    assert!(!f.spark());
    f.gpu = "GB100".into();
    assert!(!f.spark());
    f.gpu = "NVIDIA GB10".into();
    assert!(f.spark());
    let mut no_mac = f.clone();
    no_mac.interfaces[0].mac.clear();
    assert!(!no_mac.valid("192.168.50.51"), "a GB10 and IP alone do not establish LAN identity");
    f.machine_id = "bad".into();
    assert!(!f.valid("192.168.50.51"));
}
#[test]
fn proposal_preserves_ids_names_unselected_nodes_and_does_not_invent_cables() {
    let r = verified();
    let mut inv = Inventory::example();
    inv.nodes[3].name = "cell4".into();
    let p = propose(
        &r,
        &["192.168.50.51".into(), "192.168.50.181".into()],
        &inv,
        &BTreeMap::new(),
        "owner",
    )
    .unwrap();
    assert_eq!(p.inventory.nodes.len(), 4);
    assert_eq!(p.inventory.nodes[3].id, "gx10-0000");
    assert_eq!(p.inventory.nodes[3].name, "cell4");
    assert_eq!(p.inventory.nodes[1], inv.nodes[1]);
    assert_eq!(p.inventory.switches, inv.switches);
    assert_eq!(p.links.len(), 1);
    assert_eq!(p.links[0].port, "qsfp56-dd-1-1");
    assert!(p.warnings.iter().any(|w| w.contains("cell4: no LLDP")));
    assert_eq!(p.networks["192.168.100.0/24"].len(), 2);
}
#[test]
fn moved_addresses_match_unique_pins_conflicts_fail_and_shared_keys_do_not_merge() {
    let mut r = verified();
    let mut inv = Inventory::example();
    let f = r.candidates[0].facts.clone().unwrap();
    inv.nodes[0].ip = Some("192.168.50.52".into());
    inv.nodes[0].host = "old-name.local".into();
    let pin = Pin {
        hostname: f.hostname.clone(),
        machine_id: Some(f.machine_id.clone()),
        lan_mac: f.lan_mac("192.168.50.51"),
        ..Default::default()
    };
    let mut pins = BTreeMap::from([("cell1".into(), pin)]);
    let p = propose(&r, &["192.168.50.51".into()], &inv, &pins, "owner").unwrap();
    assert_eq!(p.changes[0].id, "cell1");
    assert_eq!(p.inventory.nodes[0].ip.as_deref(), Some("192.168.50.51"));
    pins.get_mut("cell1").unwrap().lan_mac = Some("02:00:00:99:00:01".into());
    assert!(propose(&r, &["192.168.50.51".into()], &inv, &pins, "owner")
        .unwrap_err()
        .contains("different pinned"));
    let empty = Inventory {
        nodes: vec![],
        switches: vec![],
    };
    let p = propose(
        &r,
        &["192.168.50.51".into(), "192.168.50.154".into()],
        &empty,
        &BTreeMap::new(),
        "owner",
    )
    .unwrap();
    assert_ne!(p.changes[0].id, p.changes[1].id);
    r.candidates[1].facts.as_mut().unwrap().interfaces[0].mac = f.interfaces[0].mac.clone();
    assert!(propose(
        &r,
        &["192.168.50.51".into()],
        &empty,
        &BTreeMap::new(),
        "owner"
    )
    .unwrap_err()
    .contains("same LAN MAC"));
}
#[test]
fn expired_partial_and_duplicate_proposals_are_refused() {
    let mut r = verified();
    let inv = Inventory::example();
    let select = vec!["192.168.50.51".into()];
    for status in ["running", "cancelled", "timedOut", "expired"] {
        r.status = status.into();
        assert!(propose(&r, &select, &inv, &BTreeMap::new(), "owner").is_err());
    }
    r.status = "complete".into();
    r.started_ms = now() - TTL_MS;
    assert!(propose(&r, &select, &inv, &BTreeMap::new(), "owner").is_err());
    r.started_ms = now();
    assert!(propose(
        &r,
        &[select[0].clone(), select[0].clone()],
        &inv,
        &BTreeMap::new(),
        "owner"
    )
    .is_err());
}
#[test]
fn only_the_exact_compiled_read_program_is_read_classified() {
    use crate::live::{
        exit::ExecKind,
        fence_class::{class_of, Ctx, Shape},
    };
    use crate::ops::classify::{classify, read_only, Kind};
    assert_eq!(
        class_of(ExecKind::Run, read_command(), &Ctx::default()),
        Shape::Read
    );
    assert_ne!(
        class_of(
            ExecKind::Run,
            &format!("{}; touch /tmp/no", read_command()),
            &Ctx::default()
        ),
        Shape::Read
    );
    assert_eq!(classify(read_command()), Kind::Read);
    assert!(read_only(read_command()));
    assert!(!read_only(&format!("{}; touch /tmp/no", read_command())));
    assert_ne!(
        classify(&format!("{}; sudo reboot", read_command())),
        Kind::Read
    );
}
#[tokio::test]
async fn anonymous_http_is_bounded_does_not_follow_redirects_or_send_credentials() {
    use tokio::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = l.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut b = [0; 512];
        let n = s.read(&mut b).await.unwrap();
        let request = String::from_utf8_lossy(&b[..n]);
        assert!(request.starts_with("GET / HTTP/1.0"));
        assert!(!request.contains("Authorization"));
        s.write_all(b"HTTP/1.0 200 OK\r\n\r\nMikroTik RouterOS WebFig")
            .await
            .unwrap();
    });
    let hint = http_hint(
        TcpStream::connect(address).await.unwrap(),
        Ipv4Addr::LOCALHOST,
    )
    .await;
    assert!(hint.contains("identity unverified"));
    task.await.unwrap();
}
#[tokio::test]
async fn fake_ssh_requires_trust_before_login_and_identifies_without_sudo_install_or_writes() {
    use crate::fake_node::{fake_node, host_fingerprint, temp};
    let f = demo_facts("192.168.50.51").unwrap();
    let reply = serde_json::to_string(&f).unwrap();
    let (node, port) = fake_node(
        &temp("discovery-node"),
        Arc::new(move |cmd, _| {
            if cmd.contains("python3 -I -c") {
                Some((reply.clone(), 0))
            } else {
                None
            }
        }),
    )
    .await;
    let dir = temp("discovery-app");
    std::fs::create_dir_all(&dir).unwrap();
    let known = dir.join("known_hosts.json");
    let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let live = crate::live::Live::new(vpn, known.clone(), || Ok(None));
    let cfg = NodeConfig {
        id: "discovery-fixture".into(),
        name: "Fixture".into(),
        host: "192.168.50.51".into(),
        ip: Some("192.168.50.51".into()),
        port: 22,
        user: "sparkuser".into(),
        model: String::new(),
        fabric: None,
    };
    crate::secrets::set("ssh", "pw").unwrap();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let error = identify(&live, &cfg, stream).await.unwrap_err();
    assert!(matches!(error, SshError::HostKeyUnknown { .. }));
    assert!(node.knobs.logins.lock().unwrap().is_empty());
    assert!(node.log.lock().unwrap().is_empty());
    crate::live::trust_host_key(
        &known,
        "192.168.50.51:22",
        &crate::fake_node::Profile::spark("gx10-0000").fingerprint(),
    )
    .unwrap();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    assert!(matches!(
        identify(&live, &cfg, stream).await,
        Err(SshError::HostKeyChanged { .. })
    ));
    assert!(node.knobs.logins.lock().unwrap().is_empty());
    assert!(node.log.lock().unwrap().is_empty());
    crate::live::trust_host_key(&known, "192.168.50.51:22", &host_fingerprint()).unwrap();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let (f, _) = identify(&live, &cfg, stream).await.unwrap();
    assert!(f.spark());
    assert!(
        node.log
            .lock()
            .unwrap()
            .iter()
            .all(|c| c.contains("python3 -I -c")),
        "only the compiled read was admitted"
    );
    live.stop();
    crate::secrets::clear("ssh").unwrap();
}

#[test]
fn a_cloned_machine_id_without_a_mac_cannot_reidentify_a_saved_node() {
    let r = verified();
    let inv = Inventory::example();
    let f = demo_facts("192.168.50.51").unwrap();
    let pins = BTreeMap::from([(
        "cell1".into(),
        Pin {
            machine_id: Some(f.machine_id),
            cloned_machine_id: true,
            ..Default::default()
        },
    )]);
    assert!(propose(&r, &["192.168.50.51".into()], &inv, &pins, "owner")
        .unwrap_err()
        .contains("no comparable identity pin"));
}

#[test]
fn damaged_pin_storage_and_hostname_conflicts_cannot_downgrade_identity() {
    let dir = crate::fake_node::temp("discovery-pins");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("node_pins.json");
    assert!(load_pins(&path).unwrap().is_empty());
    std::fs::write(&path, b"{broken").unwrap();
    assert!(load_pins(&path).is_err());
    let f = demo_facts("192.168.50.51").unwrap();
    let pin = Pin {
        hostname: "another-spark".into(),
        lan_mac: f.lan_mac("192.168.50.51"),
        ..Default::default()
    };
    assert!(propose(
        &verified(),
        &["192.168.50.51".into()],
        &Inventory::example(),
        &BTreeMap::from([("cell1".into(), pin)]),
        "owner"
    )
    .unwrap_err()
    .contains("different pinned hostname"));
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn actual_bound_socket_uses_only_injected_loopback_interface() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let scope = Scope {
        id: "loopback-fixture".into(),
        interface: "lo0".into(),
        address: "127.0.0.1".into(),
        network: "127.0.0.0/24".into(),
        eligible: true,
        reason: String::new(),
    };
    let stream = connect(
        &scope,
        Ipv4Addr::LOCALHOST,
        listener.local_addr().unwrap().port(),
    )
    .await
    .unwrap();
    assert!(stream.local_addr().unwrap().ip().is_loopback());
    assert!(stream.peer_addr().unwrap().ip().is_loopback());
    let (_, remote) = listener.accept().await.unwrap();
    assert!(remote.ip().is_loopback());
    let mut missing = scope;
    missing.interface = "not-a-real-interface".into();
    assert!(connect(
        &missing,
        Ipv4Addr::LOCALHOST,
        listener.local_addr().unwrap().port()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn anonymous_http_ignores_redirects_and_content_beyond_eight_kib() {
    let trap = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let redirect = trap.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 512];
        stream.read(&mut request).await.unwrap();
        let body = format!(
            "HTTP/1.0 302 Found\r\nLocation: http://{redirect}/\r\n\r\n{}MikroTik RouterOS WebFig",
            "x".repeat(9000)
        );
        let _ = stream.write_all(body.as_bytes()).await;
    });
    assert_eq!(
        http_hint(
            TcpStream::connect(address).await.unwrap(),
            Ipv4Addr::LOCALHOST
        )
        .await,
        "Web service; equipment unidentified"
    );
    task.await.unwrap();
    assert!(timeout(Duration::from_millis(30), trap.accept())
        .await
        .is_err());
}

#[test]
fn ethernet_and_wifi_of_one_spark_cannot_be_proposed_as_two_nodes() {
    let mut r = verified();
    let mut alias = r.candidates[0].facts.clone().unwrap();
    alias.interfaces[0].mac = "02:00:00:45:99:01".into();
    alias.interfaces[0].addresses = vec!["192.168.50.154/24".into()];
    r.candidates[1].facts = Some(alias);
    let empty = Inventory {
        nodes: vec![],
        switches: vec![],
    };
    assert!(propose(
        &r,
        &["192.168.50.51".into(), "192.168.50.154".into()],
        &empty,
        &BTreeMap::new(),
        "owner"
    )
    .unwrap_err()
    .contains("Select only one address"));
    assert_eq!(
        propose(
            &r,
            &["192.168.50.51".into()],
            &empty,
            &BTreeMap::new(),
            "owner"
        )
        .unwrap()
        .inventory
        .nodes
        .len(),
        1
    );
}
