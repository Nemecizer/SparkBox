//! The demo world agrees with itself, its operations behave like the real thing,
//! and every scenario breaks something visible and then recovers.

use super::*;

fn cluster() -> DemoCluster {
    DemoCluster::new(&Inventory::example())
}

fn run(c: &mut DemoCluster, ticks: u64) -> ClusterSnapshot {
    for _ in 0..ticks {
        c.advance();
    }
    c.snapshot()
}

fn container_id(c: &DemoCluster, node: usize, name: &str) -> String {
    c.nodes[node].containers.iter().find(|x| x.c.name == name).unwrap().c.id.clone()
}

fn net<'a>(n: &'a NodeSnapshot, iface: &str) -> &'a NetStats {
    n.nets.iter().find(|x| x.iface == iface).unwrap_or_else(|| panic!("{} has no {iface}", n.id))
}

fn port<'a>(s: &'a ClusterSnapshot, name: &str) -> &'a SwitchPort {
    s.switches.iter().find(|s| s.id == "crs804").unwrap().ports.iter().find(|p| p.name == name).unwrap()
}

fn fabric_tx(n: &NodeSnapshot) -> f64 {
    n.nets.iter().filter(|x| x.role == NetRole::Fabric).map(|x| x.tx_bps + x.rdma_tx_bps.unwrap_or(0.0)).sum()
}

fn fabric_rx(n: &NodeSnapshot) -> f64 {
    n.nets.iter().filter(|x| x.role == NetRole::Fabric).map(|x| x.rx_bps + x.rdma_rx_bps.unwrap_or(0.0)).sum()
}

// ---- M1: one coherent world

#[test]
fn every_view_agrees_on_addresses_interfaces_images_and_rates() {
    let mut c = cluster();
    for _ in 0..40 {
        let snap = run(&mut c, 1);
        for (i, n) in snap.nodes.iter().enumerate() {
            assert_eq!(n.status, LinkStatus::Online);
            let nccl = net(n, NCCL_IFACE);
            assert!(nccl.up, "{}: the NCCL interface is up", n.id);
            assert_eq!(nccl.ip.as_deref(), Some(fabric_ip(i, 0).as_str()));
            assert_eq!(nccl.mtu, Some(9000));
            assert_eq!(nccl.rdma_dev.as_deref(), Some("rocep1s0f1"));
            assert_eq!(net(n, "enP2p1s0f1np1").ip.as_deref(), Some(format!("192.168.101.{}", i + 1).as_str()));
            assert_eq!(nccl.switch_port.as_deref(), Some(format!("crs804/{}", crs_port(i)).as_str()));
            for unused in ["enp1s0f0np0", "enP2p1s0f0np0"] {
                assert!(!net(n, unused).up && net(n, unused).ip.is_none(), "{unused} is present, down and unaddressed");
            }
            assert_eq!(net(n, LAN_IFACE).ip, Inventory::example().nodes[i].ip, "LAN uses the inventory address");

            // Inspect env agrees with the Network tab.
            for ct in n.containers.iter().filter(|x| x.image.contains("vllm")) {
                assert_eq!(ct.image, VLLM_IMAGE, "one image tag everywhere");
                let inspect = c.container_inspect(&n.id, &ct.id).unwrap();
                let env: Vec<String> = serde_json::from_value(inspect["Config"]["Env"].clone()).unwrap();
                assert!(env.contains(&format!("VLLM_HOST_IP={}", nccl.ip.as_ref().unwrap())), "{env:?}");
                assert!(env.contains(&format!("NCCL_SOCKET_IFNAME={NCCL_IFACE}")));
                assert_eq!(ct.id, world::container_id(&n.id, &ct.name), "ids are stable");
            }

            // The switch port mirrors the node, within 5%.
            let p = port(&snap, &crs_port(i));
            assert_eq!(p.node.as_deref(), Some(n.id.as_str()));
            let (tx, rx) = (fabric_tx(n), fabric_rx(n));
            assert!((p.rx_bps - tx).abs() <= tx * 0.05 + 1.0, "{}: switch rx {} vs node tx {tx}", n.id, p.rx_bps);
            assert!((p.tx_bps - rx).abs() <= rx * 0.05 + 1.0, "{}: switch tx {} vs node rx {rx}", n.id, p.tx_bps);

            // Fleet's docker ps lists exactly what the Containers window shows as up.
            let (code, out, _) = c.fleet_reply(&n.id, "docker ps --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}'");
            assert_eq!(code, 0);
            let listed: Vec<&str> = out.lines().skip(1).map(|l| l.split_whitespace().next().unwrap()).collect();
            let up: Vec<&str> = n.containers.iter().filter(|x| x.state != "exited").map(|x| x.name.as_str()).collect();
            assert_eq!(listed, up, "{out}");
            // Plain docker ps has Docker's columns, published ports included.
            let (_, plain, _) = c.fleet_reply(&n.id, "docker ps");
            let head: Vec<&str> = plain.lines().next().unwrap().split("  ").map(str::trim).filter(|h| !h.is_empty()).collect();
            assert_eq!(head, ["CONTAINER ID", "IMAGE", "COMMAND", "CREATED", "STATUS", "PORTS", "NAMES"], "{plain}");
            for x in n.containers.iter().filter(|x| x.state != "exited") {
                assert!(x.ports.iter().all(|p| plain.contains(p.as_str())), "{plain}");
            }
        }
    }
    // The model folders are on the nodes that serve them.
    let served = MODEL_FOLDERS.iter().find(|f| f.repo == SERVED_REPO).unwrap();
    let ray: Vec<&str> = c.nodes.iter().filter(|n| n.role != Role::Ollama).map(|n| n.cfg.id.as_str()).collect();
    assert_eq!(served.nodes, ray.as_slice());
    let head = c.nodes[0].containers.iter().find(|x| x.c.name == "vllm-head").unwrap();
    assert!(head.c.command.contains(&format!("/models/{SERVED_REPO}")));
}

#[test]
fn pipeline_traffic_is_mostly_rdma_at_decode_rates_with_prefill_bursts() {
    let mut c = cluster();
    let (mut peak, mut decode_ticks) = (0.0f64, 0);
    for _ in 0..300 {
        let snap = run(&mut c, 1);
        let head = &snap.nodes[0];
        let rdma: f64 = head.nets.iter().filter_map(|x| x.rdma_tx_bps).sum();
        let ip: f64 = head.nets.iter().filter(|x| x.role == NetRole::Fabric).map(|x| x.tx_bps).sum();
        peak = peak.max(rdma);
        if trace::prefill_at(c.sim_ms()) == 0.0 && rdma > 0.0 {
            decode_ticks += 1;
            assert!((15e6..=90e6).contains(&rdma), "decode on a pipeline link: {rdma}");
        }
        assert!(ip < 3e6, "IP bytes on the fabric stay small: {ip}");
        assert!(rdma <= 3.1e9 + 90e6);
        // gx10-0000 isn't in the pipeline.
        assert!(snap.nodes[3].nets.iter().filter_map(|x| x.rdma_tx_bps).sum::<f64>() < 1.0);
    }
    assert!(decode_ticks > 100);
    // Bursts come about once a minute (demo::trace::prefill_at); this stretch had one if the trace says so.
    let now = c.sim_ms();
    if (0..300).any(|k| trace::prefill_at(now - k * 1000) > 0.0) {
        assert!(peak >= 0.5e9, "prefill bursts reach Gb/s: {peak}");
    }
    let day = (0..86_400u64).filter(|k| trace::prefill_at(now + k * 1000) > 0.0).count();
    assert!((600..3_000).contains(&day), "a burst about once a minute: {day} burst seconds in a day");

    // A fabric copy from Models runs at several Gb/s, over TCP.
    c.set_external_fabric_load("cell2", 9.4e9, 0.0).unwrap();
    c.set_external_fabric_load("cell1", 0.0, 9.4e9).unwrap();
    let snap = run(&mut c, 1);
    assert!(net(&snap.nodes[1], NCCL_IFACE).rx_bps > 9e9);
    assert!(port(&snap, &crs_port(1)).tx_bps > 8.9e9, "the switch sees it too");
    assert!(snap.nodes[1].disks[0].write_bps > 1e9, "and it lands on disk");
}

#[test]
fn disk_io_is_bursty_and_gigabytes_only_while_weights_load() {
    let mut c = cluster();
    let used = c.snapshot().nodes[0].disks[0].used;
    let mut reads = Vec::new();
    for _ in 0..400 {
        reads.push(run(&mut c, 1).nodes[1].disks[0].read_bps);
    }
    assert!(reads.iter().all(|r| *r < 0.6e9), "no GB/s without a load");
    let quiet = reads.iter().filter(|r| **r < 1e6).count();
    let bursts = reads.iter().filter(|r| **r > 10e6).count();
    assert!(quiet > 250 && bursts > 5, "mostly quiet with bursts: quiet {quiet}, bursts {bursts}");
    assert_eq!(c.snapshot().nodes[0].disks[0].used, used, "disks don't grow by themselves");

    // Restarting the head reloads the model: several GB/s from every Ray node's disk.
    let head = container_id(&c, 0, "vllm-head");
    c.container_action("cell1", &head, "restart").unwrap();
    let snap = run(&mut c, 3);
    assert!(snap.nodes[2].disks[0].read_bps > 2e9, "{}", snap.nodes[2].disks[0].read_bps);
    assert_eq!(snap.nodes[0].containers[0].health.as_deref(), Some("starting"));
    let snap = run(&mut c, 30);
    assert!(snap.nodes[2].disks[0].read_bps < 0.6e9);
    assert_eq!(snap.nodes[0].containers[0].health.as_deref(), Some("healthy"));
}

#[test]
fn nodes_differ_and_carry_the_details_the_ui_shows() {
    let mut c = cluster();
    // Ollama's chat sessions come and go: over a day it works much less than a pipeline stage.
    let day: Vec<u64> = (0..1440).map(|m| 1_800_000_000_000 + m * 60_000).collect();
    let ollama: f64 = day.iter().map(|t| trace::util_at("gx10-0000", *t)).sum();
    let head: f64 = day.iter().map(|t| trace::util_at("cell1", *t)).sum();
    assert!(ollama < head * 0.6, "{ollama} vs {head}");
    let snap = run(&mut c, 60);
    let temps: Vec<f32> = snap.nodes.iter().map(|n| n.gpu.temp_c).collect();
    assert!(temps.windows(2).any(|w| (w[0] - w[1]).abs() >= 1.0), "{temps:?}");
    for n in &snap.nodes {
        assert!(n.gpu.clock_max_mhz.is_some() && n.gpu.pstate.is_some());
        assert_eq!(n.gpu.power_limit_w, None, "nvidia-smi reports GB10's power limit as N/A: never invented");
        assert_eq!(n.system.cx7_fw.as_deref(), Some("28.50.1002"));
        assert_eq!(n.system.docker.as_deref(), Some("29.6.2"));
        assert!(n.system.bios.is_some() && n.system.ntp_synced == Some(true) && n.system.clock_offset_ms.is_some());
        assert!(n.processes.iter().all(|p| p.state.is_some() && p.ppid.is_some() && p.threads.is_some() && p.start_time.is_some()));
        assert!(n.containers.iter().all(|x| x.restart_policy.is_some()));
        assert!(!n.stale && n.frame_age_ms.is_some());
    }
    assert_eq!(snap.nodes[0].failed_units, vec!["nvidia-dgx-telemetry.service".to_string()]);
    assert!(snap.nodes[1].failed_units.is_empty());
    assert!(snap.nodes.iter().all(|n| n.system.driver == DRIVER && n.system.kernel == KERNEL), "the same software on all four");
    let sglang = snap.nodes[3].containers.iter().find(|x| x.name == "sglang-test").unwrap();
    assert_eq!((sglang.state.as_str(), sglang.exit_code), ("exited", 137));
    let webui = snap.nodes[0].containers.iter().find(|x| x.name == "open-webui").unwrap();
    assert_eq!(webui.restart_policy.as_deref(), Some("unless-stopped"));
    assert_eq!(webui.labels.get("com.docker.compose.project").map(String::as_str), Some("webui"));
}

#[test]
fn switches_are_set_up_with_sensors_optics_and_never_online_without_an_address() {
    let mut c = cluster();
    let snap = run(&mut c, 2);
    let crs = snap.switches.iter().find(|s| s.id == "crs804").unwrap();
    assert!(crs.configured && crs.status == LinkStatus::Online);
    assert_eq!(crs.host, "192.168.50.2");
    assert_eq!(crs.ports.len(), 5, "4 occupied physical sockets and ether1");
    assert_eq!(crs.sensors.iter().filter(|s| s.unit == "RPM").count(), 2);
    assert!(crs.sensors.iter().any(|s| s.name == "PSU1") && crs.sensors.iter().any(|s| s.name == "PSU2"));
    let p = &crs.ports[0];
    assert_eq!((p.mtu, p.capacity_mbps, p.lane_rx_power_dbm.len()), (Some(9216), 200_000, 4));
    assert!(p.sfp_vendor.is_some() && p.sfp_part.is_some());
    assert_eq!(crs.ports.iter().filter(|p| p.name.starts_with("qsfp") && p.running && p.node.is_some()).count(), 4, "all four physical fabric sockets are occupied");
    for (id, ip) in [("firewalla", "192.168.50.3"), ("gold", "192.168.50.1")] {
        let s = snap.switches.iter().find(|s| s.id == id).unwrap();
        assert!(s.configured && s.status == LinkStatus::Online && s.latency_ms.is_some());
        assert_eq!(s.host, ip);
    }

    let mut inv = Inventory::example();
    inv.switches.push(SwitchConfig { id: "lab".into(), name: "Lab switch".into(), kind: SwitchKind::RouterOs, host: String::new(), user: None });
    let snap = DemoCluster::new(&inv).snapshot();
    let lab = snap.switches.iter().find(|s| s.id == "lab").unwrap();
    assert!(!lab.configured && lab.status == LinkStatus::Offline, "not set up is never Online");
}

// ---- M2: stateful operations

#[test]
fn the_failed_telemetry_service_reads_like_systemd() {
    let mut c = cluster();
    let status = c.service_status("cell1", "nvidia-dgx-telemetry.service").unwrap();
    assert!(status.contains("Active: failed (Result: exit-code) since "), "{status}");
    assert!(status.contains("Main process exited, code=exited, status=1/FAILURE"));
    assert!(status.contains("telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied"));
    let journal = c.journal("cell1", "nvidia-dgx-telemetry.service", 20).unwrap();
    assert!(journal.trim_end().ends_with("Failed with result 'exit-code'."), "{journal}");
    assert!(journal.contains("cell1-example"));

    // Starting it fails again, the way systemctl says so.
    let err = c.service_action("cell1", "nvidia-dgx-telemetry.service", "start").unwrap_err();
    assert!(err.contains("failed because the control process exited with error code"), "{err}");

    // A healthy one: stop, then start with a new main PID.
    let before = c.service_status("cell2", "dgx-dashboard.service").unwrap();
    assert!(before.contains("Active: active (running)") && before.contains("Main PID: "));
    c.service_action("cell2", "dgx-dashboard.service", "stop").unwrap();
    assert!(c.services("cell2").unwrap().iter().any(|u| u.unit == "dgx-dashboard.service" && u.active == "inactive"));
    assert!(!run(&mut c, 1).nodes[1].listeners.iter().any(|l| l.port == 11000), "its port closes");
    c.service_action("cell2", "dgx-dashboard.service", "start").unwrap();
    assert!(run(&mut c, 1).nodes[1].listeners.iter().any(|l| l.port == 11000));
    assert!(c.journal("cell2", "dgx-dashboard.service", 5).unwrap().contains("Started dgx-dashboard.service"));
    assert!(c.service_action("cell2", "nope.service", "start").unwrap_err().contains("not found"));
}

#[test]
fn signals_end_pause_and_renice_processes_and_containers() {
    let mut c = cluster();
    let snap = c.snapshot();
    let cron = snap.nodes[1].processes.iter().find(|p| p.name == "cron").unwrap().pid;
    c.signal("cell2", cron, "TERM").unwrap();
    assert!(!c.snapshot().nodes[1].processes.iter().any(|p| p.pid == cron), "TERM removes it");
    assert!(c.services("cell2").unwrap().iter().any(|u| u.unit == "cron.service" && u.active == "inactive"));
    assert!(c.signal("cell2", cron, "TERM").unwrap_err().contains("No such process"));

    let dash = snap.nodes[1].processes.iter().find(|p| p.name == "dashboard-service").unwrap().pid;
    c.signal("cell2", dash, "STOP").unwrap();
    assert_eq!(c.snapshot().nodes[1].processes.iter().find(|p| p.pid == dash).unwrap().state.as_deref(), Some("T"));
    c.signal("cell2", dash, "CONT").unwrap();
    assert_eq!(c.snapshot().nodes[1].processes.iter().find(|p| p.pid == dash).unwrap().state.as_deref(), Some("S"));
    c.renice("cell2", dash, 10).unwrap();
    assert_eq!(c.snapshot().nodes[1].processes.iter().find(|p| p.pid == dash).unwrap().nice, 10);

    // Killing a worker's main process: the container exits 137 and the head follows.
    let worker = snap.nodes[2].containers.iter().find(|x| x.name == "vllm-worker").unwrap();
    let pid = snap.nodes[2].processes.iter().find(|p| p.container.as_deref() == Some("vllm-worker")).unwrap().pid;
    c.signal("cell3", pid, "KILL").unwrap();
    let snap = run(&mut c, 5);
    let w = snap.nodes[2].containers.iter().find(|x| x.id == worker.id).unwrap();
    assert_eq!((w.state.as_str(), w.exit_code), ("exited", 137));
    let head = snap.nodes[0].containers.iter().find(|x| x.name == "vllm-head").unwrap();
    assert_eq!((head.state.as_str(), head.exit_code), ("exited", 1), "the Ray cascade");
    assert!(snap.nodes[1].gpu.procs.is_empty(), "the model is down everywhere");

    // TERM on Ollama: exits 143 and Docker restarts it (unless-stopped).
    let ollama = snap.nodes[3].processes.iter().find(|p| p.container.as_deref() == Some("ollama")).unwrap().pid;
    c.signal("gx10-0000", ollama, "TERM").unwrap();
    let o = c.snapshot().nodes[3].containers.iter().find(|x| x.name == "ollama").unwrap().clone();
    assert_eq!((o.state.as_str(), o.restart_count), ("running", 1));
}

#[test]
fn a_reboot_plays_out_over_ticks_and_restart_policies_decide_what_comes_back() {
    let mut c = cluster();
    let uptime = c.snapshot().nodes[3].system.uptime_s;
    c.power("gx10-0000", "reboot").unwrap();
    assert!(c.power("gx10-0000", "reboot").is_err(), "one at a time");
    let s = run(&mut c, 1);
    let n = &s.nodes[3];
    assert_eq!((n.status, n.error.as_deref(), n.power_state.as_deref()), (LinkStatus::Connecting, Some("Rebooting…"), Some("rebooting")));
    run(&mut c, 2);
    let s = run(&mut c, 1);
    assert_eq!(s.nodes[3].status, LinkStatus::Offline);
    assert!(!port(&s, &crs_port(3)).running, "the NIC is down while it reboots");
    run(&mut c, 19);
    assert_eq!(run(&mut c, 1).nodes[3].status, LinkStatus::Connecting);
    let s = run(&mut c, 3);
    let n = &s.nodes[3];
    assert_eq!(n.status, LinkStatus::Online);
    assert!(n.power_state.is_none() && n.system.uptime_s < uptime && n.system.uptime_s < 60);
    let ollama = n.containers.iter().find(|x| x.name == "ollama").unwrap();
    assert_eq!(ollama.state, "running", "unless-stopped comes back");

    // A Ray node: vLLM's policy is "no", so the model stays down after the reboot.
    c.power("cell2", "reboot").unwrap();
    let s = run(&mut c, 30);
    assert_eq!(s.nodes[1].status, LinkStatus::Online);
    assert_eq!(s.nodes[1].containers.iter().find(|x| x.name == "vllm-worker").unwrap().state, "exited");
    assert!(s.nodes[0].gpu.procs.is_empty());

    // Power off stays off until woken.
    c.power("cell3", "poweroff").unwrap();
    let s = run(&mut c, 40);
    assert_eq!((s.nodes[2].status, s.nodes[2].power_state.as_deref()), (LinkStatus::Offline, Some("off")));
    assert!(c.power("cell3", "reboot").is_err());
    c.power("cell3", "wake").unwrap();
    assert_eq!(run(&mut c, 26).nodes[2].status, LinkStatus::Online);
}

#[test]
fn container_actions_cover_kill_pause_and_remove() {
    let mut c = cluster();
    let webui = container_id(&c, 0, "open-webui");
    c.container_action("cell1", &webui, "pause").unwrap();
    assert_eq!(c.snapshot().nodes[0].containers[1].state, "paused");
    assert!(c.container_action("cell1", &webui, "remove").unwrap_err().contains("container is running"));
    c.container_action("cell1", &webui, "unpause").unwrap();
    c.container_action("cell1", &webui, "kill").unwrap();
    let w = c.snapshot().nodes[0].containers[1].clone();
    assert_eq!((w.state.as_str(), w.exit_code), ("exited", 137));
    c.container_action("cell1", &webui, "remove").unwrap();
    assert!(!c.snapshot().nodes[0].containers.iter().any(|x| x.id == webui));
    assert!(c.container_action("cell1", &webui, "start").is_err());
    assert!(c.container_action("cell1", &container_id(&c, 0, "vllm-head"), "explode").is_err());
}

#[test]
fn stopping_the_ray_head_takes_the_model_down_and_starting_it_reloads() {
    let mut c = cluster();
    assert!(c.ray_serving());
    let head = container_id(&c, 0, "vllm-head");
    c.container_action("cell1", &head, "stop").unwrap();
    let snap = run(&mut c, 30);
    assert!(snap.nodes[0].gpu.procs.is_empty(), "head's GPU memory is released");
    assert!(snap.nodes[1].gpu.util_pct < 1.0 && snap.nodes[1].gpu.procs.is_empty(), "workers go idle without the head");
    assert!(!snap.nodes[0].listeners.iter().any(|l| l.port == 8000));
    c.container_action("cell1", &head, "start").unwrap();
    let snap = run(&mut c, 30);
    assert!(c.ray_serving());
    assert!(snap.nodes[0].listeners.iter().any(|l| l.port == 8000));
}

#[test]
fn disabling_a_fabric_port_cuts_that_node_off() {
    let mut c = cluster();
    c.set_port_disabled("crs804", "qsfp56-dd-2-1", true).unwrap();
    let snap = run(&mut c, 1);
    assert!(!c.ray_serving());
    assert!(!net(&snap.nodes[1], NCCL_IFACE).up);
    assert!(port(&snap, "qsfp56-dd-2-1").disabled);
    c.set_port_disabled("crs804", "qsfp56-dd-2-1", false).unwrap();
    run(&mut c, 30);
    assert!(c.ray_serving());
    assert!(c.set_port_disabled("crs804", "ether1", true).is_err(), "the management uplink stays up");
}

#[test]
fn every_built_in_fleet_snippet_answers_from_the_node() {
    const BUILT_IN: [&str; 8] = [
        "nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader",
        "uptime",
        "df -h / /home 2>/dev/null",
        "free -h",
        "docker ps --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}'",
        "apt list --upgradable 2>/dev/null",
        "uname -srm",
        "for i in /sys/class/net/enp1s0f*np* /sys/class/net/enP2p1s0f*np*; do [ -e $i ] && s=$(cat $i/operstate) && echo \"$(basename $i) $s$([ \"$s\" = up ] && echo \" $(cat $i/speed)Mb/s\")\"; done",
    ];
    let mut c = cluster();
    run(&mut c, 5);
    for cmd in BUILT_IN {
        let outs: Vec<String> = ["cell1", "gx10-0000"]
            .iter()
            .map(|n| {
                let (code, out, err) = c.fleet_reply(n, cmd);
                assert_eq!(code, 0, "{cmd} on {n}: {err}");
                assert!(!out.trim().is_empty(), "{cmd}");
                out
            })
            .collect();
        // Healthy links and the kernel print the same everywhere; everything else is the node's own.
        if !cmd.contains("/sys/class/net") && !cmd.starts_with("uname") {
            assert_ne!(outs[0], outs[1], "{cmd} differs per node");
        }
    }
    c.set_port_disabled("crs804", "qsfp56-dd-4-1", true).unwrap();
    run(&mut c, 1);
    assert!(c.fleet_reply("gx10-0000", BUILT_IN[7]).1.contains("enp1s0f1np1 down"), "the links follow the node's state");
    c.set_port_disabled("crs804", "qsfp56-dd-4-1", false).unwrap();
    run(&mut c, 1);
    let (_, gpu, _) = c.fleet_reply("gx10-0000", BUILT_IN[0]);
    assert!(gpu.starts_with(&format!("NVIDIA GB10, {DRIVER}, ")), "{gpu}");
    let (_, links, _) = c.fleet_reply("cell1", BUILT_IN[7]);
    assert!(links.contains("enp1s0f1np1 up 200000Mb/s") && links.contains("enp1s0f0np0 down\n") && !links.contains("down Mb/s"), "{links}");
    let (_, df, _) = c.fleet_reply("gx10-0000", BUILT_IN[2]);
    assert_eq!(df.lines().count(), 3, "{df}");
    let (_, apt, _) = c.fleet_reply("gx10-0000", BUILT_IN[5]);
    assert!(apt.contains("libbrotli1/noble-updates,noble-security 1.1.0-2ubuntu0.1 arm64 [upgradable from: 1.1.0-2build2]"), "{apt}");

    for cmd in ["hostname", "cat /etc/hostname", "ip -br addr", "ip -br link", "systemctl --failed", "nproc", "nvidia-smi", "docker version", "journalctl -u nvidia-dgx-telemetry -n 5"] {
        let (code, out, err) = c.fleet_reply("cell1", cmd);
        assert_eq!(code, 0, "{cmd}: {err}");
        assert!(!out.is_empty());
    }
    assert!(c.fleet_reply("cell1", "ip -br addr").1.contains("192.168.100.1/24"));
    assert!(c.fleet_reply("cell1", "systemctl --failed").1.contains("nvidia-dgx-telemetry.service"));
    assert_eq!(c.fleet_reply("cell1", "systemctl status nvidia-dgx-telemetry").0, 3);
    assert_eq!(c.fleet_reply("cell1", "rm -rf /tmp/x").0, 127);
}

#[test]
fn updates_are_the_real_ones_and_installing_them_empties_the_list() {
    let mut c = cluster();
    let names = |n: &str| c.updates(n).unwrap().packages.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    assert_eq!(names("cell1"), ["libbrotli1", "nginx", "nginx-common", "google-chrome-stable"]);
    for n in ["cell2", "cell3", "gx10-0000"] {
        assert_eq!(names(n), ["libbrotli1", "google-chrome-stable"], "{n}");
    }
    let cell1 = c.updates("cell1").unwrap();
    let nginx = cell1.packages.iter().find(|p| p.name == "nginx").unwrap();
    assert_eq!((nginx.from.as_str(), nginx.to.as_str(), nginx.security), ("1.24.0-2ubuntu7.18", "1.24.0-2ubuntu7.19", true));
    assert!(!cell1.packages.iter().find(|p| p.name == "google-chrome-stable").unwrap().security);
    assert!(cell1.packages.iter().all(|p| !p.held && !p.reboot));
    assert!(!cell1.unattended, "auto-updates off, as on the real nodes");
    assert!(c.apply_updates("cell2", &["docker-ce".into()]).unwrap_err().contains("Unable to locate package docker-ce"));
    c.apply_updates("cell1", &[]).unwrap();
    let after = c.updates("cell1").unwrap();
    assert!(after.packages.is_empty() && !after.reboot_required);
    assert!(!run(&mut c, 1).nodes[0].system.reboot_required);
}

#[test]
fn containers_from_models_count_on_the_node_and_go_by_label() {
    let mut c = cluster();
    let before = run(&mut c, 1).nodes[3].mem.used;
    let id = c
        .add_container(
            "gx10-0000",
            DemoContainerSpec {
                name: "sparkbox-qwen".into(),
                image: VLLM_IMAGE.into(),
                ports: vec![8001],
                gpu_mem: 30 * 1024 * 1024 * 1024,
                rss: 6 * 1024 * 1024 * 1024,
                labels: [("sparkbox.deployment".to_string(), "qwen".to_string())].into(),
                command: "vllm serve /models/Qwen/Qwen3.8-27B-FP8 --port 8001".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(id, world::container_id("gx10-0000", "sparkbox-qwen"));
    let n = run(&mut c, 1).nodes[3].clone();
    assert!(n.mem.used > before + 30 * 1024 * 1024 * 1024);
    assert!(n.listeners.iter().any(|l| l.port == 8001));
    assert!(n.gpu.procs.iter().any(|p| p.container.as_deref() == Some("sparkbox-qwen")));
    assert!(c.add_container("gx10-0000", DemoContainerSpec { name: "sparkbox-qwen".into(), ..Default::default() }).unwrap_err().contains("already in use"));
    assert_eq!(c.remove_containers("sparkbox.deployment", "qwen"), 1);
    assert!(!run(&mut c, 1).nodes[3].listeners.iter().any(|l| l.port == 8001));
}

// ---- M3: scenarios

fn play(id: &str) -> DemoCluster {
    let mut c = cluster();
    c.set_scenario(Some(id)).unwrap();
    assert_eq!(c.snapshot().scenario.as_deref(), Some(id));
    c
}

#[test]
fn scenarios_are_listed_and_one_plays_at_a_time() {
    let mut c = cluster();
    let ids: Vec<String> = c.scenarios().into_iter().map(|s| s.id).collect();
    for id in [
        "as-found", "default", "healthy", "node-offline", "disk-filling", "thermal-throttle", "oom-kill", "crash-loop", "fabric-flap", "collector-stall", "optic-degraded",
        "switch-port-down", "sudo-rejected", "slow-node", "model-oom", "gated-401", "ray-join-timeout", "copy-disk-full", "endpoint-slow", "agent-offline",
        "gpu-xid", "fabric-congestion", "roce-loss", "pfc-storm", "miscabled", "mtu-mismatch", "endpoint-stall", "paging-drift", "fabric-partition",
    ] {
        assert!(ids.contains(&id.to_string()), "{id}");
    }
    assert!(c.set_scenario(Some("meteor")).is_err());
    c.set_scenario(Some("healthy")).unwrap();
    assert!(run(&mut c, 1).nodes.iter().all(|n| n.failed_units.is_empty()), "healthy has no issues");
    c.set_scenario(Some("default")).unwrap();
    assert_eq!(c.scenario(), None);
    assert_eq!(run(&mut c, 1).nodes[0].failed_units.len(), 1, "back to the everyday world");
}

#[test]
fn node_offline_drops_cell3_for_a_minute_then_reconnects() {
    let mut c = play("node-offline");
    assert_eq!(run(&mut c, 1).nodes[2].status, LinkStatus::Connecting);
    let s = run(&mut c, 5);
    assert_eq!(s.nodes[2].status, LinkStatus::Offline);
    assert_eq!(s.nodes[2].error.as_deref(), Some("SSH timeout: no reply from 192.168.50.98:22 in 15 s"));
    assert!(s.nodes[2].nets.is_empty(), "no numbers from a node SparkBox can't reach");
    assert!(port(&s, "qsfp56-dd-3-1").running, "its fabric stays up");
    // The link is up but the stage is hung: the pipeline carries nothing until cell3 is back.
    let rdma = |s: &ClusterSnapshot, i: usize| s.nodes[i].nets.iter().filter_map(|x| x.rdma_tx_bps).sum::<f64>();
    assert_eq!(rdma(&s, 0), 0.0, "cell1 sends nothing down a stalled pipeline");
    let s = run(&mut c, 60);
    assert_eq!(s.nodes[2].status, LinkStatus::Online);
    let s = run(&mut c, 3);
    assert!(rdma(&s, 0) > 1e6, "the pipeline runs again once cell3 is back");
}

#[test]
fn disk_filling_reaches_97_percent_then_is_cleaned_up() {
    let mut c = play("disk-filling");
    let pct = |s: &ClusterSnapshot| s.nodes[3].disks[0].used as f64 / s.nodes[3].disks[0].total as f64;
    let start = pct(&run(&mut c, 1));
    let mid = run(&mut c, 299);
    assert!(pct(&mid) > start + 0.04 && mid.nodes[3].disks[0].write_bps > 50e6);
    let full = run(&mut c, 300);
    assert!((pct(&full) - 0.97).abs() < 0.002, "{}", pct(&full));
    let after = run(&mut c, 100);
    assert!((pct(&after) - start).abs() < 0.01, "cleaned up");
}

#[test]
fn thermal_throttle_heats_cell1_and_slows_its_clock() {
    let mut c = play("thermal-throttle");
    let normal = c.snapshot().nodes[0].gpu.clone();
    let s = run(&mut c, 60);
    let g = &s.nodes[0].gpu;
    assert!((88.0..=91.0).contains(&g.temp_c), "{}", g.temp_c);
    assert_eq!(g.throttle, vec!["HW thermal slowdown".to_string(), "SW power cap".to_string()]);
    assert!(g.sm_clock_mhz < 1_800, "clock down 30%: {} (was {})", g.sm_clock_mhz, normal.sm_clock_mhz);
    let s = run(&mut c, 100);
    assert!(s.nodes[0].gpu.throttle.is_empty() && s.nodes[0].gpu.temp_c < 85.0);
}

#[test]
fn oom_kill_fills_memory_then_ollama_dies_and_comes_back() {
    let mut c = play("oom-kill");
    let s = run(&mut c, 41);
    let m = &s.nodes[3].mem;
    assert!(m.used as f64 / m.total as f64 > 0.95, "{}", m.used as f64 / m.total as f64);
    let s = run(&mut c, 2);
    let o = s.nodes[3].containers.iter().find(|x| x.name == "ollama").unwrap();
    assert_eq!((o.state.as_str(), o.exit_code, o.oom_killed), ("exited", 137, true));
    assert!(s.nodes[3].mem.used as f64 / (s.nodes[3].mem.total as f64) < 0.5);
    let s = run(&mut c, 13);
    let o = s.nodes[3].containers.iter().find(|x| x.name == "ollama").unwrap();
    assert_eq!((o.state.as_str(), o.restart_count), ("running", 1));
}

#[test]
fn crash_loop_restarts_the_worker_and_keeps_the_model_down_until_it_stops() {
    let mut c = play("crash-loop");
    let mut states = Vec::new();
    for t in 0..240 {
        let s = run(&mut c, 1);
        states.push(s.nodes[1].containers[0].state.clone());
        // (The head's GPU takes a few seconds to wind down.)
        assert!(t < 10 || s.nodes[0].gpu.util_pct < 20.0, "the model is down");
    }
    assert!(states.iter().any(|s| s == "restarting") && states.iter().any(|s| s == "running"));
    let w = &c.snapshot().nodes[1].containers[0];
    assert!(w.restart_count >= 7, "{}", w.restart_count);
    assert_eq!(w.restart_policy.as_deref(), Some("on-failure"));
    run(&mut c, 40);
    assert!(c.ray_serving(), "it stays up and the model reloads");
}

#[test]
fn fabric_flap_counts_carrier_changes_link_downs_and_errors() {
    let mut c = play("fabric-flap");
    let before = run(&mut c, 0);
    let (cc0, ld0, fcs0) = (net(&before.nodes[1], NCCL_IFACE).carrier_changes.unwrap(), port(&before, "qsfp56-dd-2-1").link_downs, port(&before, "qsfp56-dd-2-1").fcs_errors);
    let down = run(&mut c, 2);
    assert!(!net(&down.nodes[1], NCCL_IFACE).up && !port(&down, "qsfp56-dd-2-1").running);
    let up = run(&mut c, 10);
    assert!(net(&up.nodes[1], NCCL_IFACE).up);
    let s = run(&mut c, 110);
    assert!(net(&s.nodes[1], NCCL_IFACE).carrier_changes.unwrap() >= cc0 + 12);
    assert!(port(&s, "qsfp56-dd-2-1").link_downs >= ld0 + 6);
    assert!(port(&s, "qsfp56-dd-2-1").fcs_errors > fcs0 && net(&s.nodes[1], NCCL_IFACE).rx_errors > 0);
    let calm = run(&mut c, 40);
    assert!(net(&calm.nodes[1], NCCL_IFACE).up);
}

#[test]
fn collector_stall_freezes_cell1_with_a_growing_age() {
    let mut c = play("collector-stall");
    let a = run(&mut c, 2);
    let b = run(&mut c, 10);
    assert!(a.nodes[0].stale && b.nodes[0].stale);
    assert_eq!(a.nodes[0].cpu.total_pct, b.nodes[0].cpu.total_pct, "the numbers don't move");
    assert!(b.nodes[0].frame_age_ms.unwrap() >= a.nodes[0].frame_age_ms.unwrap() + 9_000);
    assert_eq!(a.nodes[0].last_seen_ms, b.nodes[0].last_seen_ms);
    assert_eq!(b.nodes[0].status, LinkStatus::Online, "connected, just stale");
    let c2 = run(&mut c, 20);
    assert!(!c2.nodes[0].stale && c2.nodes[0].frame_age_ms.unwrap() < 2_000);
}

#[test]
fn optic_degraded_fades_lane_3_on_cell2s_port() {
    let mut c = play("optic-degraded");
    let s = run(&mut c, 60);
    let p = port(&s, "qsfp56-dd-2-1");
    assert!((p.lane_rx_power_dbm[2] + 9.5).abs() < 0.2, "{:?}", p.lane_rx_power_dbm);
    assert!(p.lane_rx_power_dbm.iter().enumerate().filter(|(i, _)| *i != 2).all(|(_, v)| (-2.8..=-1.4).contains(v)));
    assert!((p.rx_power_dbm.unwrap() + 9.5).abs() < 0.2, "the worst lane is reported");
    let s = run(&mut c, 150);
    assert!(port(&s, "qsfp56-dd-2-1").lane_rx_power_dbm[2] > -3.0, "reseated");
}

#[test]
fn switch_port_down_takes_cell3_and_the_model_down() {
    let mut c = play("switch-port-down");
    let s = run(&mut c, 3);
    assert!(!port(&s, "qsfp56-dd-3-1").running);
    assert!(!net(&s.nodes[2], NCCL_IFACE).up);
    assert!(!c.ray_serving());
    run(&mut c, 60);
    run(&mut c, 30);
    assert!(c.ray_serving(), "back after the port returns");
}

#[test]
fn sudo_rejected_fails_privileged_operations() {
    let mut c = play("sudo-rejected");
    let e = c.service_action("cell2", "cron.service", "restart").unwrap_err();
    assert!(e.contains("sudo: incorrect password"), "{e}");
    assert!(c.power("cell2", "reboot").unwrap_err().contains("sudo: incorrect password"));
    assert!(c.apply_updates("cell2", &[]).unwrap_err().contains("sudo: incorrect password"));
    assert_eq!(c.fleet_reply("cell2", "sudo apt list --upgradable").0, 1);
    assert_eq!(c.fleet_reply("cell2", "uptime").0, 0, "plain commands still work");
}

#[test]
fn slow_node_answers_in_seconds() {
    let mut c = play("slow-node");
    let s = run(&mut c, 5);
    let l = s.nodes[1].latency_ms.unwrap();
    assert!((1_800.0..=2_400.0).contains(&l), "{l}");
    assert!(run(&mut c, 100).nodes[1].latency_ms.unwrap() < 100.0);
}

/// Writes the everyday Demo world's snapshot for UI tests that need a serving cluster
/// (src/dev/fixtures/demo-snapshot.json). The browser preview's snapshot.json is the real
/// cluster at rest instead (the Live tour's, see the as-found tests below).
/// Run with: SPARKBOX_WRITE_FIXTURE=1 cargo test write_ui_fixture -- --ignored
#[test]
#[ignore]
fn write_ui_fixture() {
    if std::env::var("SPARKBOX_WRITE_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let mut c = cluster();
    let snap = run(&mut c, 40);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/dev/fixtures/demo-snapshot.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&snap).unwrap() + "\n").unwrap();
}

// ---- r2: scenario phases, engine load, RoCE counters and the switch's tables

#[test]
fn each_scenario_says_where_it_is_in_its_cycle() {
    let mut c = cluster();
    assert_eq!(c.snapshot().scenario_phase, None, "the everyday world has no phase");
    c.set_scenario(Some("node-offline")).unwrap();
    let p = run(&mut c, 10).scenario_phase.unwrap();
    assert_eq!((p.phase.as_str(), p.label.as_str()), ("fault", "Node offline · fault"));
    assert_eq!(p.next_change_s, Some(53), "fault until 63 s in");
    let p = run(&mut c, 54).scenario_phase.unwrap();
    assert_eq!((p.phase.as_str(), p.label.as_str(), p.next_change_s), ("recovering", "Node offline · recovering", Some(2)));
    let p = run(&mut c, 10).scenario_phase.unwrap();
    assert_eq!(p.phase, "healthy");
    assert!(p.next_change_s.unwrap() > 0);
    // A steady scenario stays in its fault with no countdown.
    c.set_scenario(Some("miscabled")).unwrap();
    let p = run(&mut c, 5).scenario_phase.unwrap();
    assert_eq!((p.phase.as_str(), p.next_change_s), ("fault", None));
    c.set_scenario(Some("healthy")).unwrap();
    assert_eq!(run(&mut c, 1).scenario_phase.unwrap().phase, "healthy");
}

#[test]
fn engine_load_drives_the_gpu_and_falls_idle_without_calls() {
    let mut c = cluster();
    run(&mut c, 5);
    // Models says gx10-0000's engine is generating at 80%, 61 W.
    for _ in 0..12 {
        c.set_engine_load("gx10-0000", EngineLoad { util_pct: 80.0, power_w: Some(61.0), active: true, steady: false });
        c.advance();
    }
    let s = c.snapshot();
    let g = &s.nodes[3].gpu;
    assert!((70.0..=90.0).contains(&g.util_pct), "{}", g.util_pct);
    assert!((58.0..=64.0).contains(&g.power_w), "{}", g.power_w);
    assert!(g.sm_clock_mhz > 2_000, "boost clock while generating");
    assert_eq!(g.power_limit_w, None);
    // Not generating (or no call for 10 s): idle, whatever the old synthetic load would have been.
    let s = run(&mut c, 25);
    let g = &s.nodes[3].gpu;
    assert_eq!(g.util_pct, 0.0);
    assert!(g.power_w < 12.0 && g.sm_clock_mhz < 500, "{} W {} MHz", g.power_w, g.sm_clock_mhz);
    // A node Models never drove keeps the world's own load (cell1 serves the pipeline).
    assert!(s.nodes.iter().take(3).any(|n| n.gpu.util_pct > 0.0) || run(&mut c, 30).nodes.iter().take(3).any(|n| n.gpu.util_pct > 0.0));
}

#[test]
fn a_healthy_fabric_counts_a_little_ecn_and_no_loss() {
    let mut c = cluster();
    let a = run(&mut c, 2);
    let b = run(&mut c, 120);
    for (na, nb) in a.nodes.iter().zip(&b.nodes) {
        for iface in ["enp1s0f1np1", "enP2p1s0f1np1"] {
            let (ra, rb) = (net(na, iface).roce.clone().unwrap(), net(nb, iface).roce.clone().unwrap());
            assert_eq!(ra.out_of_sequence, rb.out_of_sequence, "{} {iface}: nothing lost", na.id);
            assert_eq!((rb.rx_crc_errors_phy, rb.local_ack_timeout_err), (Some(0), Some(0)));
            assert!(rb.ecn_marked >= ra.ecn_marked);
            assert_eq!(rb.pfc_prios.as_deref(), Some(&[3u8][..]), "PFC on RoCE's priority");
            assert_eq!(rb.roce_v2, Some(true));
        }
        assert_eq!(net(nb, LAN_IFACE).lan_wol.as_deref(), Some("g"));
        assert!(net(nb, LAN_IFACE).roce.is_none(), "the LAN has no RDMA counters");
    }
    // The Ray containers carry their NCCL settings for the transport check.
    let head = b.nodes[0].containers.iter().find(|x| x.name == "vllm-head").unwrap();
    assert_eq!(head.env_flags.get("NCCL_SOCKET_IFNAME").map(String::as_str), Some(NCCL_IFACE));
    assert!(head.env_flags.contains_key("NCCL_IB_HCA") && head.env_flags.contains_key("VLLM_HOST_IP"));
    assert!(!head.env_flags.contains_key("PATH") && !head.env_flags.contains_key("HF_HOME"), "only NCCL_* and VLLM_HOST_IP");
}

#[test]
fn the_default_world_keeps_exactly_its_one_documented_warning() {
    let mut c = cluster();
    let s = run(&mut c, 30);
    // Telemetry failed on cell1: nothing else is off (gx10-0000's disk fills only in disk-filling).
    let failed: Vec<(&str, &String)> = s.nodes.iter().flat_map(|n| n.failed_units.iter().map(move |u| (n.id.as_str(), u))).collect();
    assert_eq!(failed, [("cell1", &"nvidia-dgx-telemetry.service".to_string())]);
    let full: Vec<&str> = s.nodes.iter().filter(|n| n.disks[0].used as f64 / n.disks[0].total as f64 >= 0.80).map(|n| n.id.as_str()).collect();
    assert!(full.is_empty(), "{full:?}");
    assert!(s.nodes.iter().all(|n| n.kernel_events.is_empty() && n.gpu.xid_errors == 0));
    assert!(s.nodes.iter().all(|n| n.nets.iter().filter(|x| x.ip.is_some() && x.role == NetRole::Fabric).all(|x| x.mtu == Some(9000) && x.up)));
}

#[test]
fn roce_loss_climbs_on_cell2_half_2_and_flaps_once() {
    let mut c = play("roce-loss");
    let a = run(&mut c, 5);
    let b = run(&mut c, 20);
    let r = |s: &ClusterSnapshot, iface: &str| net(&s.nodes[1], iface).roce.clone().unwrap();
    let (ra, rb) = (r(&a, "enP2p1s0f1np1"), r(&b, "enP2p1s0f1np1"));
    assert!(rb.out_of_sequence.unwrap() >= ra.out_of_sequence.unwrap() + 20 * 40);
    assert!(rb.local_ack_timeout_err > ra.local_ack_timeout_err && rb.rx_crc_errors_phy > ra.rx_crc_errors_phy);
    assert_eq!(r(&b, "enp1s0f1np1").out_of_sequence, r(&a, "enp1s0f1np1").out_of_sequence, "half 1 stays as it was");
    assert_eq!(r(&a, "enp1s0f1np1").out_of_sequence, Some(342), "what the GLM run left");
    assert!(port(&b, "qsfp56-dd-2-1").fcs_errors > port(&a, "qsfp56-dd-2-1").fcs_errors, "the switch sees the CRC errors too");
    let downs = rb.link_downed.unwrap();
    let s = run(&mut c, 80);
    assert_eq!(r(&s, "enP2p1s0f1np1").link_downed, Some(downs + 1), "one flap at 100 s");
}

#[test]
fn pfc_storm_sends_50k_pause_frames_a_second_from_gx10() {
    let mut c = play("pfc-storm");
    let a = run(&mut c, 2);
    let b = run(&mut c, 10);
    let tx = |s: &ClusterSnapshot| net(&s.nodes[3], "enp1s0f1np1").roce.clone().unwrap().tx_pause[3].unwrap();
    let per_s = (tx(&b) - tx(&a)) as f64 / 10.0;
    assert!((48_000.0..=52_000.0).contains(&per_s), "{per_s}");
    assert!(port(&b, "qsfp56-dd-4-1").rx_pause > port(&a, "qsfp56-dd-4-1").rx_pause + 400_000);
}

#[test]
fn congestion_marks_ecn_without_loss() {
    let mut c = play("fabric-congestion");
    let a = run(&mut c, 1);
    let b = run(&mut c, 5);
    let r = |s: &ClusterSnapshot| net(&s.nodes[1], "enp1s0f1np1").roce.clone().unwrap();
    assert!(r(&b).ecn_marked.unwrap() >= r(&a).ecn_marked.unwrap() + 5 * 4_000);
    assert!(r(&b).cnp_handled.unwrap() > r(&a).cnp_handled.unwrap());
    assert_eq!(r(&b).out_of_sequence, r(&a).out_of_sequence, "nothing lost");
    assert!(fabric_tx(&b.nodes[1]) > 50e9, "an all-reduce burst: {}", fabric_tx(&b.nodes[1]));
}

#[test]
fn miscabled_swaps_cell2_and_cell3_by_mac_but_not_by_comment() {
    let mut c = play("miscabled");
    let s = run(&mut c, 2);
    let t = c.switch_tables("crs804");
    let on = |port: &str| t.hosts.iter().filter(|h| h.port == port).map(|h| h.mac.clone()).collect::<Vec<_>>();
    assert!(on("qsfp56-dd-2-1").contains(&mac("cell3", "enp1s0f1np1").to_uppercase()));
    assert!(on("qsfp56-dd-3-1").contains(&mac("cell2", "enP2p1s0f1np1").to_uppercase()));
    assert_eq!(port(&s, "qsfp56-dd-2-1").comment, "cell2", "the comment still names the old node");
    assert_eq!(port(&s, "qsfp56-dd-2-1").node.as_deref(), Some("cell3"), "placed by MAC");
    assert_eq!(net(&s.nodes[1], NCCL_IFACE).switch_port.as_deref(), Some("crs804/qsfp56-dd-3-1"));
    assert!(t.neighbors.iter().any(|n| n.port == "qsfp56-dd-2-1" && n.identity == "cell3-example"));
    c.set_scenario(None).unwrap();
    assert_eq!(port(&run(&mut c, 1), "qsfp56-dd-2-1").node.as_deref(), Some("cell2"), "put back");
}

#[test]
fn mtu_mismatch_sets_gx10_half_2_to_1500_and_jumbo_pings_fail() {
    let mut c = play("mtu-mismatch");
    let s = run(&mut c, 2);
    assert_eq!(net(&s.nodes[3], "enP2p1s0f1np1").mtu, Some(1500));
    assert_eq!(net(&s.nodes[3], "enp1s0f1np1").mtu, Some(9000));
    let (code, out, err) = c.fleet_reply("gx10-0000", "ping -M do -s 8972 -c 3 -W 1 -I 'enP2p1s0f1np1' '192.168.101.1'");
    assert_eq!(code, 1);
    assert!(err.contains("ping: local error: message too long, mtu=1500") && out.contains("100% packet loss"), "{out}{err}");
    let (code, out, _) = c.fleet_reply("cell1", "ping -M do -s 8972 -c 3 -W 1 -I 'enP2p1s0f1np1' '192.168.101.4'");
    assert!(code == 1 && out.contains("0 received"), "jumbo frames to gx10's half 2 are dropped: {out}");
    let (code, out, _) = c.fleet_reply("cell1", "ping -c 2 -W 1 -I 'enP2p1s0f1np1' '192.168.101.4'");
    assert!(code == 0 && out.contains("2 received"), "small ones get through: {out}");
}

#[test]
fn gpu_xid_logs_xid_79_on_gx10_and_oom_kill_logs_the_kill() {
    let mut c = play("gpu-xid");
    let s = run(&mut c, 3);
    let e = &s.nodes[3].kernel_events;
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].kind.as_str(), e[0].code, e[0].severity.as_str()), ("xid", Some(79), "crit"));
    assert!(e[0].text.contains("GPU has fallen off the bus"));
    assert_eq!(s.nodes[3].gpu.xid_errors, 1);
    assert_eq!(s.nodes[3].gpu.util_pct, 0.0);
    c.set_scenario(None).unwrap();
    assert!(run(&mut c, 1).nodes[3].kernel_events.is_empty(), "the scenario's events leave with it");

    let mut c = play("oom-kill");
    let s = run(&mut c, 44);
    let e = s.nodes[3].kernel_events.last().expect("the kill is logged");
    assert_eq!((e.kind.as_str(), e.process.as_deref()), ("oom", Some("ollama")));
    assert!(e.text.starts_with("Out of memory: Killed process "), "{}", e.text);
}

#[test]
fn model_and_monitoring_scenarios_are_named_for_the_packages_that_play_them() {
    for id in ["model-oom", "gated-401", "ray-join-timeout", "copy-disk-full", "endpoint-slow", "agent-offline"] {
        let mut c = play(id);
        let s = run(&mut c, 3);
        assert_eq!(s.scenario.as_deref(), Some(id));
        let p = s.scenario_phase.unwrap();
        assert_eq!(p.phase, "fault", "{id}");
        // The simulator itself leaves the world as it was: Models and the health engine react to the id.
        assert!(s.nodes.iter().all(|n| n.status == LinkStatus::Online), "{id}");
    }
}

#[test]
fn a_disabled_port_comes_back_on_its_own_unless_kept_and_a_bounce_is_3_seconds() {
    let mut c = cluster();
    let at = c.set_port_disabled_for("crs804", "qsfp56-dd-4-1", true, 60).unwrap();
    assert!(at.is_some());
    let s = run(&mut c, 1);
    assert!(port(&s, "qsfp56-dd-4-1").disabled && port(&s, "qsfp56-dd-4-1").revert_at_ms.is_some());
    assert!(!run(&mut c, 60).switches[0].ports.iter().find(|p| p.name == "qsfp56-dd-4-1").unwrap().disabled, "the switch re-enabled it");
    c.set_port_disabled_for("crs804", "qsfp56-dd-4-1", true, 60).unwrap();
    c.keep_port_disabled("crs804", "qsfp56-dd-4-1").unwrap();
    assert!(port(&run(&mut c, 90), "qsfp56-dd-4-1").disabled, "kept disabled");
    c.set_port_disabled_for("crs804", "qsfp56-dd-4-1", false, 0).unwrap();

    // Disabling the management uplink (typed out in the UI) cuts SparkBox off until the revert.
    c.set_port_disabled_for("crs804", "ether1", true, 60).unwrap();
    let s = run(&mut c, 2);
    let crs = s.switches.iter().find(|x| x.id == "crs804").unwrap();
    assert_eq!(crs.status, LinkStatus::Offline);
    assert!(crs.error.as_deref().unwrap().starts_with("No reply from 192.168.50.2"));
    assert_eq!(run(&mut c, 60).switches.iter().find(|x| x.id == "crs804").unwrap().status, LinkStatus::Online);

    c.bounce_port("crs804", "qsfp56-dd-1-1").unwrap();
    assert!(!port(&run(&mut c, 1), "qsfp56-dd-1-1").running);
    assert!(port(&run(&mut c, 3), "qsfp56-dd-1-1").running, "back by itself");
}

// ---- Each node's own voice (trace)

fn corr(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    cov / (va.sqrt() * vb.sqrt())
}

#[test]
fn no_two_nodes_draw_the_same_gpu_line() {
    // Fifteen minutes of the everyday world, as the GPU tiles and charts read it, at the same
    // three stretches as demo::trace's test. The stretch is fixed, not whenever the test runs:
    // one with minutes of lull in it can lift two stages past 0.95, because every stage goes
    // quiet together, as stages serving one pipeline do.
    for start in [1_800_000_000_000u64, 1_800_000_000_000 + 4 * 3_600_000, 1_800_000_000_000 + 13 * 3_600_000] {
        let mut c = DemoCluster::starting_at(&Inventory::example(), start);
        let mut series: Vec<Vec<f64>> = vec![Vec::new(); 4];
        for _ in 0..900 {
            let snap = run(&mut c, 1);
            for (i, n) in snap.nodes.iter().enumerate() {
                series[i].push(n.gpu.util_pct as f64);
            }
        }
        for i in 0..4 {
            for j in i + 1..4 {
                let r = corr(&series[i], &series[j]);
                assert!(r.is_nan() || r < 0.95, "{} and {} correlate {r:.3} from {start}", c.nodes[i].cfg.id, c.nodes[j].cfg.id);
            }
        }
    }
    // (That the pipeline stages still move together is checked at fixed times in demo::trace.)
    // Each node splits its fabric traffic across the halves its own way (and runs its own TCP level).
    let mixes: Vec<(f64, f64, f64)> = ["cell1", "cell2", "cell3"].iter().map(|n| trace::fabric_mix(n)).collect();
    for i in 0..3 {
        for j in i + 1..3 {
            assert!((mixes[i].0 - mixes[j].0).abs() > 0.005 && (mixes[i].2 - mixes[j].2).abs() > 0.005, "{mixes:?}");
        }
    }
    // And the live readings carry that split whenever a stage sends.
    let mut c = DemoCluster::starting_at(&Inventory::example(), 1_800_000_000_000);
    for _ in 0..30 {
        let snap = run(&mut c, 1);
        for (n, mix) in snap.nodes.iter().take(3).zip(&mixes) {
            let halves: Vec<f64> = n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).map(|x| x.rdma_tx_bps.unwrap_or(0.0)).collect();
            let total = halves[0] + halves[1];
            if total > 0.0 {
                assert!((halves[0] / total - mix.0).abs() < 1e-9, "{}: {halves:?}", n.id);
            }
        }
    }
}

#[test]
fn the_live_gpu_meets_its_history_at_now() {
    // What the history synth draws for "now" is what the simulator reads now.
    let mut c = cluster();
    for _ in 0..30 {
        let snap = run(&mut c, 1);
        let at = c.sim_ms();
        for n in snap.nodes.iter().filter(|n| n.id != "gx10-0000") {
            let want = trace::value_at(&n.id, "gpuUtil", at).unwrap();
            assert!((n.gpu.util_pct - want).abs() <= 2.5, "{} reads {} where its trace says {want}", n.id, n.gpu.util_pct);
        }
    }
    // Simulated time is wall time: the first reading after the warm-up is "now".
    let now = unix_now_ms();
    assert!(cluster().sim_ms().abs_diff(now) < 5_000);
}

#[test]
fn every_node_has_a_boot_id_and_a_reboot_changes_it() {
    let mut c = cluster();
    let snap = run(&mut c, 1);
    let ids: Vec<String> = snap.nodes.iter().map(|n| n.boot_id.clone().expect("a boot_id")).collect();
    assert!(ids.iter().all(|id| id.len() == 36 && id.matches('-').count() == 4), "{ids:?}");
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "each node its own");
    c.power("cell2", "reboot").unwrap();
    let snap = run(&mut c, 40);
    assert_eq!(snap.nodes[1].status, LinkStatus::Online);
    assert_ne!(snap.nodes[1].boot_id.as_ref(), Some(&ids[1]), "a new boot");
    assert_eq!(snap.nodes[0].boot_id.as_ref(), Some(&ids[0]));
}

#[test]
fn the_head_container_has_a_token_for_inspect_to_mask() {
    let c = cluster();
    let head = c.nodes[0].containers.iter().find(|x| x.c.name == "vllm-head").unwrap();
    assert!(head.env.iter().any(|e| e.starts_with("HF_TOKEN=hf_demo")), "{:?}", head.env);
    // It never reaches the NCCL settings the snapshot carries.
    assert!(env_flags(&head.env).keys().all(|k| k != "HF_TOKEN"));
}

#[test]
fn gpu_xid_takes_the_gpu_until_a_restart() {
    let mut c = cluster();
    c.set_scenario(Some("gpu-xid")).unwrap();
    let snap = run(&mut c, 5);
    let gx = &snap.nodes[3];
    assert_eq!(gx.error.as_deref(), Some("GPU has fallen off the bus (Xid 79)"));
    assert_eq!(gx.gpu.util_pct, 0.0);
    assert!(gx.kernel_events.iter().any(|e| e.code == Some(79)));
    let boot = gx.boot_id.clone();
    c.power("gx10-0000", "reboot").unwrap();
    let snap = run(&mut c, 40);
    let gx = &snap.nodes[3];
    assert_eq!(gx.status, LinkStatus::Online);
    assert_eq!(gx.error, None, "the restart brought the GPU back");
    assert!(gx.kernel_events.is_empty(), "a new boot's kernel log");
    assert_ne!(gx.boot_id, boot);
}

#[test]
fn fabric_partition_cuts_both_halves_and_keeps_the_lan() {
    let mut c = cluster();
    c.set_scenario(Some("fabric-partition")).unwrap();
    let snap = run(&mut c, 5);
    let cell2 = &snap.nodes[1];
    assert_eq!(cell2.status, LinkStatus::Online);
    assert!(cell2.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).all(|x| !x.up), "both halves down");
    assert!(cell2.nets.iter().any(|x| x.role == NetRole::Lan && x.up));
    assert!(!port(&snap, "qsfp56-dd-2-1").running);
    // Two minutes on, it's back.
    let snap = run(&mut c, 125);
    assert!(snap.nodes[1].nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).all(|x| x.up));
}

// ---- r4: the real machines, and the cluster as found on Oct 29

/// The Live tour's snapshot of Oct 29, 07:43 (the browser preview's fixture).
fn live_snapshot() -> ClusterSnapshot {
    serde_json::from_str(include_str!("../../../src/dev/fixtures/snapshot.json")).expect("the Live fixture parses")
}

fn found() -> DemoCluster {
    let mut c = DemoCluster::as_found();
    run(&mut c, 5);
    c
}

#[test]
fn as_found_matches_the_live_snapshot() {
    let live = live_snapshot();
    assert!(!live.demo, "the fixture is a Live snapshot");
    let mut c = found();
    let demo = run(&mut c, 3);
    assert_eq!(demo.scenario.as_deref(), Some("as-found"));
    for real in &live.nodes {
        let n = demo.nodes.iter().find(|n| n.id == real.id).unwrap_or_else(|| panic!("no {}", real.id));
        let id = n.id.as_str();
        assert_eq!(n.system.hostname, real.system.hostname);
        assert_eq!(n.system.model, real.system.model, "{id}");
        for (a, b) in [(&n.system.os, &real.system.os), (&n.system.kernel, &real.system.kernel), (&n.system.driver, &real.system.driver), (&n.system.cuda, &real.system.cuda)] {
            assert_eq!(a, b, "{id}");
        }
        assert_eq!((&n.system.bios, &n.system.cx7_fw, &n.system.docker), (&real.system.bios, &real.system.cx7_fw, &real.system.docker), "{id}");
        assert_eq!(n.system.clock_offset_ms, None, "{id}: Live can't read it");

        // NICs: the same interfaces with the same addresses, MACs, links and MTUs.
        let nics = |s: &NodeSnapshot| {
            let mut v: Vec<(String, Option<String>, Option<String>, bool, Option<u32>, u32)> =
                s.nets.iter().map(|x| (x.iface.clone(), x.ip.clone(), x.mac.clone(), x.up, x.mtu, if x.role == NetRole::Wifi { 0 } else { x.speed_mbps })).collect();
            v.sort();
            v
        };
        assert_eq!(nics(n), nics(real), "{id}");
        for x in &real.nets {
            let d = net(n, &x.iface);
            assert_eq!((d.carrier_changes, d.tx_drops), (x.carrier_changes, x.tx_drops), "{id} {}", x.iface);
            if let (Some(a), Some(b)) = (&d.roce, &x.roce) {
                assert_eq!((a.out_of_sequence, a.packet_seq_err, a.tx_pause_ctrl), (b.out_of_sequence, b.packet_seq_err, b.tx_pause_ctrl), "{id} {}", x.iface);
                assert_eq!((a.trust.as_deref(), a.pfc_prios.as_deref()), (Some("pcp"), Some(&[][..])), "PFC off");
            }
        }

        // Memory and disks.
        assert_eq!(n.mem.total, real.mem.total, "{id}");
        assert_eq!(n.mem.swap_total, real.mem.swap_total);
        assert!((n.mem.used as f64 - real.mem.used as f64).abs() < 0.3 * GIB, "{id}: {} vs {}", n.mem.used, real.mem.used);
        let pct = |s: &NodeSnapshot| s.disks[0].used as f64 / s.disks[0].total as f64 * 100.0;
        assert_eq!(n.disks[0].total, real.disks[0].total, "{id}");
        assert!((pct(n) - pct(real)).abs() < 0.01, "{id}: {:.1}% vs {:.1}%", pct(n), pct(real));
        assert_eq!(n.disks[1].used, real.disks[1].used, "{id} /boot/efi");

        // The GPU at rest.
        let (g, r) = (&n.gpu, &real.gpu);
        assert_eq!((g.util_pct, g.sm_clock_mhz, g.clock_max_mhz, g.power_limit_w, g.pstate.as_deref()), (r.util_pct, r.sm_clock_mhz, r.clock_max_mhz, r.power_limit_w, r.pstate.as_deref()), "{id}");
        assert!((g.power_w - r.power_w).abs() < 0.8 && (g.temp_c - r.temp_c).abs() <= 2.0, "{id}: {} W {} °C vs {} W {} °C", g.power_w, g.temp_c, r.power_w, r.temp_c);
        assert!(g.procs.is_empty() && n.mem.gpu_alloc == 0);

        // Containers: the same names, images, commands, states, exit codes and run lengths.
        let ctrs = |s: &NodeSnapshot| {
            let mut v: Vec<(String, String, String, String, i32, String, u64)> =
                s.containers.iter().map(|x| (x.name.clone(), x.id.clone(), x.image.clone(), x.state.clone(), x.exit_code, x.command.clone(), x.finished_at - x.started_at)).collect();
            v.sort();
            v
        };
        assert_eq!(ctrs(n), ctrs(real), "{id}");
        for (d, r) in n.containers.iter().zip(&real.containers) {
            let d = n.containers.iter().find(|x| x.name == r.name).unwrap_or(d);
            assert_eq!(d.labels, r.labels, "{id} {}", r.name);
            assert_eq!(d.env_flags, r.env_flags, "{id} {}", r.name);
        }

        // Listening sockets, with no owners (ss without root).
        let ports = |s: &NodeSnapshot| {
            let mut v: Vec<(u16, String)> = s.listeners.iter().map(|l| (l.port, l.addr.clone())).collect();
            v.sort();
            v
        };
        assert_eq!(ports(n), ports(real), "{id}");
        assert!(n.listeners.iter().all(|l| l.pid.is_none() && l.process.is_none()), "{id}");

        // The kernel log: the recabling.
        let texts = |s: &NodeSnapshot| s.kernel_events.iter().map(|e| e.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(n), texts(real), "{id}");

        // Services: as many units as the services matrix lists for the node.
        let units = c.services(id).unwrap();
        let want = include_str!("fixtures/services-matrix.txt").lines().filter(|l| l.contains(&format!(" on {id}: "))).count();
        assert_eq!(units.len(), want, "{id}");
        assert!(n.failed_units.is_empty(), "{id}: every unit is healthy");
    }
    // 92 units across the four.
    let mut all: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].iter().flat_map(|n| c.services(n).unwrap().into_iter().map(|u| u.unit)).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 92);
    // The switches aren't set up, as in Live.
    assert!(demo.switches.iter().all(|s| !s.configured && s.status == LinkStatus::Offline));
    assert!(demo.nodes.iter().all(|n| n.nets.iter().all(|x| x.switch_port.is_none())), "Switch port unknown");
}

#[test]
fn every_world_wears_the_real_identities() {
    for (world, mut c) in [("everyday", cluster()), ("as-found", found())] {
        let s = run(&mut c, 2);
        let want = [
            ("cell1", "cell1-example", "MSI EdgeXpert (MS-C931)", "5.36_1.8.0", 130_661_117_952u64, 30.08),
            ("cell2", "cell2-example", "MSI EdgeXpert (MS-C931)", "5.36_1.8.0", 130_661_117_952, 23.86),
            ("cell3", "cell3-example", "MSI EdgeXpert (MS-C931)", "5.36_1.8.0", 130_661_122_048, 26.17),
            ("gx10-0000", "gx10-0000", "ASUS Ascent GX10 (GX10)", "GX10DGX.0106.2026.0707.0731", 130_594_021_376, 38.04),
        ];
        for (n, (id, host, model, bios, mem, disk)) in s.nodes.iter().zip(want) {
            assert_eq!(n.id, id);
            let sys = &n.system;
            assert_eq!((sys.hostname.as_str(), sys.model.as_str(), sys.bios.as_deref()), (host, model, Some(bios)), "{world}");
            assert_eq!((sys.os.as_str(), sys.kernel.as_str(), sys.driver.as_str(), sys.cuda.as_str()), ("NVIDIA DGX Spark (Ubuntu 24.04.5 LTS)", "7.0.0-1019-nvidia", "580.178.04", "13.0"), "{world} {id}");
            assert_eq!((sys.cx7_fw.as_deref(), sys.docker.as_deref()), (Some("28.50.1002"), Some("29.6.2")));
            assert_eq!(n.mem.total, mem, "{world} {id}");
            assert_eq!((n.gpu.clock_max_mhz, n.gpu.power_limit_w), (Some(3003), None));
            let pct = n.disks[0].used as f64 / n.disks[0].total as f64 * 100.0;
            assert!((pct - disk).abs() < 0.05, "{world} {id}: {pct:.2}%");
            assert_eq!(n.disks[0].total, if id == "gx10-0000" { 982_819_848_192 } else { 4_030_802_149_376 });
            // Port 1 uncabled at 1500, port 2 at 200G and 9000, the Wi-Fi radio, the 1G LAN.
            for unused in ["enp1s0f0np0", "enP2p1s0f0np0"] {
                assert!(!net(n, unused).up && net(n, unused).mtu == Some(1500));
            }
            assert_eq!(net(n, LAN_IFACE).speed_mbps, 1000);
            assert_eq!(net(n, LAN_IFACE).mac.as_deref(), Some(world::real(id, 0).macs[0]));
            assert_eq!(net(n, "wlP9s9").up, id == "cell3", "{world} {id}: only cell3's Wi-Fi joined the LAN");
            assert_eq!(n.host_key.as_ref().unwrap().shared_with.len(), if id == "gx10-0000" { 0 } else { 2 }, "the cloned image");
        }
        assert_eq!(net(&s.nodes[2], "wlP9s9").ip.as_deref(), Some("192.168.50.229"));
        assert_eq!(net(&s.nodes[2], "wlP9s9").carrier_changes, Some(154));
        assert_eq!(c.fleet_reply("cell1", "python3 --version").1, "Python 3.12.3\n");
        assert!(c.fleet_reply("gx10-0000", "cat /etc/dgx-release").1.contains("DGX_PLATFORM=\"GX10\""));
    }
}

#[test]
fn idle_gpus_read_like_the_real_ones() {
    let mut c = found();
    let (mut with_cap, mut without, mut flips) = (0, 0, 0);
    let mut last: Vec<bool> = Vec::new();
    for _ in 0..120 {
        let s = run(&mut c, 1);
        let caps: Vec<bool> = s.nodes.iter().map(|n| !n.gpu.throttle.is_empty()).collect();
        for n in &s.nodes {
            let g = &n.gpu;
            assert_eq!((g.util_pct, g.sm_clock_mhz, g.pstate.as_deref()), (0.0, 208, Some("P8")), "{}", n.id);
            assert!((3.7..=5.4).contains(&g.power_w), "{}: {} W", n.id, g.power_w);
            assert!((42.0..=50.0).contains(&g.temp_c), "{}: {} °C", n.id, g.temp_c);
            assert!(g.throttle.is_empty() || g.throttle == ["SW power cap"], "{:?}", g.throttle);
            assert!((0.0..=1.6).contains(&n.cpu.total_pct), "{}: CPU {}%", n.id, n.cpu.total_pct);
            assert!((0.05..=0.6).contains(&n.cpu.load[0]), "{}: load {:?}", n.id, n.cpu.load);
        }
        with_cap += caps.iter().filter(|x| **x).count();
        without += caps.iter().filter(|x| !**x).count();
        flips += caps.iter().zip(&last).filter(|(a, b)| a != b).count();
        last = caps;
    }
    // GB10 at P8 says "SW power cap" every other sample or so.
    assert!(with_cap > 120 && without > 120 && flips > 300, "{with_cap} {without} {flips}");
    // The everyday world's gx10-0000 between chat sessions is the same idle GPU.
    let mut e = cluster();
    let idle = (0..600).map(|_| run(&mut e, 1).nodes[3].gpu.clone()).find(|g| g.util_pct == 0.0).expect("an idle stretch");
    assert_eq!((idle.sm_clock_mhz, idle.pstate.as_deref()), (208, Some("P8")));
    assert!(idle.power_w < 6.0, "{}", idle.power_w);
}

#[test]
fn idle_traffic_is_broadcasts_and_the_collector() {
    let mut c = found();
    let mut fabric_seconds = Vec::new();
    for _ in 0..90 {
        let s = run(&mut c, 1);
        for n in &s.nodes {
            let lan = net(n, LAN_IFACE);
            assert!((3e3..=30e3).contains(&lan.rx_bps) && (95e3..=126e3).contains(&lan.tx_bps), "{}: {} / {}", n.id, lan.rx_bps, lan.tx_bps);
            let (a, b) = (net(n, "enp1s0f1np1"), net(n, "enP2p1s0f1np1"));
            assert_eq!(a.rx_bps, b.rx_bps, "both halves hear the same broadcasts");
            assert!(a.rx_bps <= 3_800.0 && a.tx_bps == 0.0 && a.rdma_tx_bps == Some(0.0), "{}: {} / {}", n.id, a.rx_bps, a.tx_bps);
            if n.id == "gx10-0000" {
                fabric_seconds.push(a.rx_bps);
            }
        }
    }
    // Per second, not a few segments: most seconds differ from the one before.
    let changes = fabric_seconds.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(changes > 40, "{changes} of 89");
    assert!(fabric_seconds.iter().any(|v| (250.0..=860.0).contains(v)));
}

#[test]
fn as_found_has_the_failed_glm53_run_and_nothing_serving() {
    let mut c = found();
    let s = run(&mut c, 2);
    assert!(s.nodes.iter().all(|n| n.containers.iter().all(|x| x.state == "exited")));
    assert!(s.nodes.iter().all(|n| !n.listeners.iter().any(|l| l.port == 8000)), "nothing serving");
    let glm: Vec<&Container> = s.nodes.iter().flat_map(|n| n.containers.iter().filter(|x| x.name == "glm53")).collect();
    assert_eq!(glm.len(), 4);
    for (rank, x) in glm.iter().enumerate() {
        assert_eq!((x.image.as_str(), x.exit_code, x.restart_policy.as_deref()), ("vllm/vllm-openai:glm53-flash-arm64-cu130", 1, Some("no")));
        assert_eq!(x.command, format!("/bin/bash /glm53/inner.sh {rank}"));
        assert!((53..=56).contains(&(x.finished_at - x.started_at)), "after 54 s");
        // Moved by whole days (world::found): it finished 2.2 to 3.3 days before now, so docker
        // says "2 days ago" until 6:21 PM and "3 days ago" after, as on the real nodes.
        let ago = unix_now().saturating_sub(x.finished_at);
        assert!((86_400 * 22 / 10..86_400 * 33 / 10).contains(&ago), "{ago}");
    }
    let days = ((unix_now().saturating_sub(glm[0].finished_at) as f64 / 3_600.0 + 0.5) as u64) / 24;
    let inspect = c.container_inspect("cell1", "glm53").unwrap();
    assert_eq!(inspect["Id"], "7da4d848a41aefe02d3543e6c8cc46326426cadaa673f0c6cd707e95b0ceb135");
    assert_eq!((inspect["HostConfig"]["NetworkMode"].as_str(), inspect["HostConfig"]["IpcMode"].as_str(), inspect["HostConfig"]["ShmSize"].as_u64()), (Some("host"), Some("host"), Some(17_179_869_184)));
    assert_eq!(inspect["NetworkSettings"]["Networks"].as_object().unwrap().keys().collect::<Vec<_>>(), ["host"]);
    let mounts: Vec<(String, bool)> = inspect["Mounts"].as_array().unwrap().iter().map(|m| (m["Destination"].as_str().unwrap().to_string(), m["RW"].as_bool().unwrap())).collect();
    assert_eq!(mounts, [("/root/.cache/flashinfer".into(), true), ("/root/.triton".into(), true), ("/glm53".into(), false), ("/models".into(), false), ("/root/.cache/vllm".into(), true)]);
    let env: Vec<String> = serde_json::from_value(inspect["Config"]["Env"].clone()).unwrap();
    for want in ["GPU_MEM_UTIL=0.87", "MAX_MODEL_LEN=524288", "NNODES=4", "SERVED_NAME=glm-5.3-flash", "MODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4", "HEAD_IP=192.168.100.1", "MASTER_PORT=29501"] {
        assert!(env.iter().any(|e| e == want), "{want}");
    }
    assert!(env.contains(&format!("API_KEY={}", found::API_KEY_PLACEHOLDER)), "a placeholder, never the owner's key");
    assert!(env.iter().find(|e| e.starts_with("NVIDIA_REQUIRE_CUDA=")).unwrap().len() > 2_400, "the long one-line requirement");
    // docker logs: the head's real log, ending where the engine gave up; the cause is in the middle.
    let (log, finished) = c.container_log("cell1", "glm53").unwrap();
    assert!(log.trim_end().ends_with("RuntimeError: Engine core initialization failed. See root cause above. Failed core proc(s): {}"), "{}", &log[log.len() - 200..]);
    assert!(log.contains("kv_cache_dtype=bfloat16"));
    assert_eq!(finished, glm[0].finished_at);
    assert!(c.container_log("cell2", "glm53").unwrap().0.contains("from node_rank 1"), "each rank its own");
    // The other exited containers, and their images.
    let gx = &s.nodes[3];
    let names: Vec<&str> = gx.containers.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["glm53", "vigilant_hofstadter", "adoring_northcutt", "jovial_elgamal", "gracious_babbage"]);
    let old = unix_now() - gx.containers[1].finished_at;
    assert!((185 * 86_400..188 * 86_400).contains(&old), "{}", old / 86_400);
    let proxy = s.nodes[2].containers.iter().find(|x| x.name == "workbench-proxy").unwrap();
    assert_eq!((proxy.image.as_str(), proxy.exit_code), ("docker.io/traefik:v3.6.1", 0));
    assert!(c.container_log("cell3", "workbench-proxy").unwrap().0.contains("Traefik version 3.6.1"));
    let images = c.images("gx10-0000").unwrap();
    assert_eq!(images.iter().map(|i| format!("{}:{}", i.repository, i.tag)).collect::<Vec<_>>(), ["vllm/vllm-openai:glm53-flash-arm64-cu130", "nvcr.io/nvidia/vllm:26.03.post1-py3", "nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04"]);
    let (_, ls, _) = c.fleet_reply("cell3", "docker image ls --format '{{json .}}'");
    assert_eq!(ls.lines().count(), 2);
    assert!(serde_json::from_str::<serde_json::Value>(ls.lines().next().unwrap()).unwrap()["Repository"] == "vllm/vllm-openai");
    let (_, ps, _) = c.fleet_reply("cell1", "docker ps -a");
    assert!(ps.contains(&format!("Exited (1) {days} days ago")) && ps.contains("7da4d848a41a"), "{ps}");
}

#[test]
fn as_found_services_ollama_and_timers_are_the_real_ones() {
    let mut c = found();
    let has = |c: &DemoCluster, node: &str, unit: &str| c.services(node).unwrap().into_iter().find(|u| u.unit == unit);
    assert!(has(&c, "cell1", "nginx.service").is_some() && has(&c, "cell2", "nginx.service").is_none());
    for (n, ollama, earlyoom) in [("cell1", true, true), ("cell2", false, true), ("cell3", true, false), ("gx10-0000", false, false)] {
        assert_eq!(has(&c, n, "ollama.service").is_some(), ollama, "{n}");
        assert_eq!(has(&c, n, "earlyoom.service").is_some(), earlyoom, "{n}");
    }
    assert_eq!(has(&c, "gx10-0000", "networkd-dispatcher.service").unwrap().active, "inactive");
    assert_eq!(has(&c, "gx10-0000", "containerd.service").unwrap().enabled, "disabled");
    assert_eq!(has(&c, "cell1", "apparmor.service").unwrap().sub, "exited", "a oneshot stays active (exited)");
    // Native Ollama: user ollama on loopback; ss can't name it; its journal says where it listens.
    let s = run(&mut c, 1);
    let p = s.nodes[0].processes.iter().find(|p| p.name == "ollama").unwrap();
    assert_eq!((p.user.as_str(), p.cmd.as_str(), p.unit.as_deref()), ("ollama", "/usr/local/bin/ollama serve", Some("ollama.service")));
    let l = s.nodes[0].listeners.iter().find(|l| l.port == 11434).unwrap();
    assert_eq!((l.addr.as_str(), l.scope.as_deref(), l.process.as_deref()), ("127.0.0.1", Some("local"), None));
    let status = c.service_status("cell1", "ollama.service").unwrap();
    assert!(status.contains("Loaded: loaded (/etc/systemd/system/ollama.service; enabled; preset: enabled)") && status.contains("Memory: 20.1M (peak: 554.8M)"), "{status}");
    let journal = c.journal("cell1", "ollama.service", 40).unwrap();
    assert!(journal.contains("msg=\"Listening on 127.0.0.1:11434 (version 0.21.0)\"") && journal.contains("\"/api/tags\""), "{journal}");
    // Full names, the kernel's 15-character comm, SparkBox's own marked.
    let dash = s.nodes[0].processes.iter().find(|p| p.name == "dashboard-service").unwrap();
    assert_eq!((dash.comm.as_deref(), dash.user.as_str()), (Some("dashboard-servi"), "dgx-dashboard-service-user"));
    assert!(s.nodes[0].processes.iter().filter(|p| p.sparkbox).count() == 2);
    assert!(s.nodes[0].processes.iter().any(|p| p.kernel && p.name == "kthreadd"));
    assert!(s.nodes[0].processes.iter().filter(|p| !p.kernel).count() >= 30, "a full process list");
    assert!(s.nodes[0].proc_total as usize > s.nodes[0].processes.len());
    // anacron comes and goes with its timer, as a healthy one does.
    let mut states = Vec::new();
    for _ in 0..12 {
        run(&mut c, 60);
        states.push(has(&c, "cell2", "anacron.service").unwrap().active);
    }
    assert!(states.iter().any(|s| s == "active") && states.iter().any(|s| s == "inactive"), "{states:?}");
    assert!(run(&mut c, 1).nodes.iter().all(|n| n.failed_units.is_empty()), "inactive isn't failed");
    // Updates as listed: 4 on cell1, 2 elsewhere, checked half an hour ago.
    assert_eq!(c.updates("cell1").unwrap().packages.len(), 4);
    assert!((29 * 60..=31 * 60).contains(&(unix_now() - c.updates("cell1").unwrap().checked_at)));
}

#[test]
fn as_found_kernel_log_is_the_recabling_three_days_ago() {
    let mut c = found();
    let s = run(&mut c, 1);
    let all: Vec<&KernelEvent> = s.nodes.iter().flat_map(|n| n.kernel_events.iter()).collect();
    assert_eq!(all.len(), 58);
    assert_eq!(s.nodes.iter().map(|n| n.kernel_events.len()).collect::<Vec<_>>(), [14, 14, 14, 16]);
    assert!(all.iter().all(|e| e.kind == "mlx5" && e.text.starts_with("mlx5_core") || e.text.contains("Link") || e.text.contains("Port")));
    let off = crate::health::maintenance::local_offset_s();
    let today = (unix_now() as i64 + off).div_euclid(86_400);
    let mut recabling = 0;
    for (n, e) in s.nodes.iter().flat_map(|n| n.kernel_events.iter().map(move |e| (n, e))) {
        let local = (e.ts_ms / 1000) as i64 + off;
        // world::found moves it by whole days, so it keeps its time of day in UTC: 2:16 PM EDT,
        // when the tour saw it, whatever offset the Mac is on now.
        let minute = (e.ts_ms / 1000 % 86_400) as i64 / 60;
        let boot = unix_now() - n.system.uptime_s;
        if (e.ts_ms / 1000).abs_diff(boot) < 60 {
            // The uncabled port 1 reports its empty cage at boot.
            assert!(e.text.contains("Cable unplugged") || e.text.contains("Link down") || e.text.contains("DOWN"), "{}", e.text);
            continue;
        }
        recabling += 1;
        assert_eq!(today - local.div_euclid(86_400), 3, "three days ago: {}", e.text);
        assert!((18 * 60 + 16..=18 * 60 + 29).contains(&minute), "2:16–2:29 PM EDT: {}:{:02} UTC", minute / 60, minute % 60);
    }
    assert_eq!(recabling, 42, "the recabling, after the 16 lines at boot");
}

#[test]
fn as_found_and_back_to_everyday_serving() {
    let mut c = found();
    assert_eq!(c.world(), world::World::AsFound);
    assert!(!c.ray_serving());
    let phase = c.snapshot().scenario_phase.unwrap();
    assert_eq!((phase.phase.as_str(), phase.label.as_str()), ("healthy", "Your cluster at rest (Oct 29)"));
    c.set_scenario(None).unwrap();
    assert_eq!(c.world(), world::World::Everyday);
    let s = run(&mut c, 30);
    assert!(c.ray_serving(), "the showcase serves again");
    assert!(s.nodes[0].containers.iter().any(|x| x.name == "vllm-head" && x.state == "running"));
    assert!(s.nodes.iter().all(|n| n.kernel_events.is_empty()) && s.switches.iter().all(|x| x.configured));
    // The scenario list starts with the cluster at rest, then the everyday world.
    let list = c.scenarios();
    assert_eq!((list[0].id.as_str(), list[0].title.as_str()), ("as-found", "Your cluster at rest (Oct 29)"));
    assert_eq!((list[1].id.as_str(), list[1].title.as_str()), ("default", "Everyday serving"));
    // Another scenario from the as-found world starts from the everyday one.
    let mut c = found();
    c.set_scenario(Some("crash-loop")).unwrap();
    assert_eq!(c.world(), world::World::Everyday);
}

#[test]
fn the_roce_counters_keep_the_ring_the_glm_run_left() {
    for mut c in [found(), cluster()] {
        let s = run(&mut c, 2);
        let half2 = |i: usize| net(&s.nodes[i], "enP2p1s0f1np1").roce.clone().unwrap();
        // Node N's sequence errors are node N+1's out-of-sequence count, all the way round.
        for i in 0..4 {
            assert_eq!(half2(i).packet_seq_err, half2((i + 1) % 4).out_of_sequence, "{i}");
        }
        assert_eq!(half2(0).out_of_sequence, Some(83));
    }
    // A fault on top adds to them; leaving it, or the as-found world, never zeroes them.
    let mut c = cluster();
    c.set_scenario(Some("roce-loss")).unwrap();
    run(&mut c, 30);
    c.set_scenario(None).unwrap();
    let s = run(&mut c, 2);
    assert!(net(&s.nodes[1], "enP2p1s0f1np1").roce.clone().unwrap().out_of_sequence.unwrap() > 341);
    let mut c = found();
    c.power("cell2", "reboot").unwrap();
    let s = run(&mut c, 30);
    assert_eq!(net(&s.nodes[1], "enP2p1s0f1np1").roce.clone().unwrap().out_of_sequence, Some(341));
}

#[test]
fn switch_ports_move_every_second_each_its_own_way() {
    // D5: each port's per-second rate is its own line, not a shared shape.
    let mut c = cluster();
    let ports = ["qsfp56-dd-1-1", "qsfp56-dd-2-1", "qsfp56-dd-3-1", "qsfp56-dd-4-1"];
    let mut series: Vec<Vec<f64>> = vec![Vec::new(); 4];
    for _ in 0..84 {
        let s = run(&mut c, 1);
        for (k, p) in ports.iter().enumerate() {
            let p = port(&s, p);
            series[k].push(p.rx_bps.max(p.tx_bps));
        }
    }
    for (k, v) in series.iter().enumerate() {
        let distinct = v.windows(2).filter(|w| (w[0] - w[1]).abs() > 1e-6 * w[0].max(1.0)).count();
        // The three pipeline ports carry traffic every second. gx10's port only hears broadcasts,
        // in about 4 seconds of 10 and in no set pattern, so it reads 0 b/s between them, as on the real
        // switch: it changes about 53 times in 84 s, rarely fewer than 40.
        let least = if k == 3 { 21 } else { 80 };
        assert!(distinct >= least, "{}: {distinct} changes in 84 s", ports[k]);
    }
    for i in 0..3 {
        for j in i + 1..3 {
            assert_ne!(series[i], series[j], "{} and {} draw the same line", ports[i], ports[j]);
        }
    }
}
