"""Tests for the node agent (sparkbox_agent.py, gate.py, peers.py): sampling from fixture
/proc and /sys trees, the rules (the same parity fixtures the app's engine runs), the
notification gate (fixtures/gate-parity.json, the same cases as notify.rs), peers that
tell a split from an outage, the persisted outbox and its workers, the watchdog, the
listeners, maintenance windows and silences, endpoint probes against r3-serving's fake
engines, and delivery to local ntfy, Pushover, webhook, heartbeat and SMTP servers.

Nothing here touches a real Spark: the nodes are fixture trees, fake servers and
simulated peers.

Run: python3 -I -m unittest discover -s src-tauri/src/agent -p '*_test.py'
"""
import hashlib
import hmac
import http.server
import importlib
import itertools
import json
import os
import shutil
import signal
import socket
import sqlite3
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
LIVE = os.path.join(HERE, "..", "live")
sys.path.insert(0, LIVE)
sys.path.insert(0, HERE)
import collector  # noqa: E402
import collector_test  # noqa: E402
import gate  # noqa: E402
import peers  # noqa: E402
import roce  # noqa: E402
import serving_probe_test  # noqa: E402
import sparkbox_agent as agent  # noqa: E402

DEFAULT_RULES = os.path.join(HERE, "..", "health", "fixtures", "default-rules.json")
PARITY = os.path.join(HERE, "..", "health", "fixtures", "parity.json")
ROCE_IFACE = os.path.join(HERE, "..", "health", "fixtures", "roce-iface.json")
GATE_PARITY = os.path.join(HERE, "fixtures", "gate-parity.json")
DOCKER_STATES = os.path.join(HERE, "fixtures", "docker-states.json")
GPU_THROTTLE = os.path.join(HERE, "..", "health", "fixtures", "gpu-throttle-cases.json")
CONFIG_HASH = os.path.join(HERE, "fixtures", "config-hash")
JOURNAL = os.path.join(HERE, "fixtures", "journal-kernel.jsonl")
GIB = 1024 ** 3
NODES = ("cell1", "cell2", "cell3", "gx10-0000")
#: The C-agent shape: what /healthz says, exactly, and what `status --json` starts with.
HEALTHZ = {"node", "version", "lastCycleMs", "configMac", "addresses", "peers", "missing", "isolated", "leader", "gateway",
           "fabricDown", "switchDown", "outboxPending", "lastDelivery", "heartbeatOkMs", "endpointsProbed", "quietTz"}
#: The request key every test agent gets in notify.json (the credential).
KEY = "3f" * 32


def default_rules():
    with open(DEFAULT_RULES) as f:
        return json.load(f)["rules"]


def cfg(node="cell1", index=0, peers=None, **extra):
    return agent.Config(dict({"node": node, "name": node, "index": index, "peers": peers or []}, **extra))


def frame(mem_pct=31.25, disk_pct=25.0, gpu_temp=50.0, clock=0.0, **extra):
    total, disk = 128 * GIB, 4000 * GIB
    f = {
        "cpu": {"totalPct": 4.0, "perCore": [], "load": [0.1, 0.1, 0.1], "tempC": 40.0},
        "mem": {"total": total, "used": int(total * mem_pct / 100), "available": 0, "cached": 6 * GIB, "gpuAlloc": 0, "swapTotal": 0, "swapUsed": 0},
        "gpu": {"utilPct": 0.0, "tempC": gpu_temp, "powerW": 11.0, "smClockMhz": 600, "throttle": []},
        "disks": [{"mount": "/", "device": "/dev/nvme0n1p2", "fsType": "ext4", "total": disk, "used": int(disk * disk_pct / 100), "readBps": 0.0, "writeBps": 0.0}],
        "nets": [{"iface": "enp1s0f1np1", "role": "fabric", "up": True, "ip": "192.168.100.11", "mtu": 9000, "rxBps": 1e6, "txBps": 2e6,
                  "rxErrors": 0, "txErrors": 0, "carrierChanges": 2, "rdmaDev": "rocep1s0f1", "rdmaRxBps": 8e6, "rdmaTxBps": 4e6},
                 {"iface": "enP7s7", "role": "lan", "up": True, "ip": "192.168.50.10", "mtu": 1500, "rxBps": 3e5, "txBps": 1e5}],
        "system": {"clockOffsetMs": clock, "ntpSynced": True},
        "containers": [], "failedUnits": [], "kernelEvents": [],
    }
    f.update(extra)
    return f


def signed(kind, body, key=KEY):
    """A request as SparkBox writes it (agent/mod.rs signed_request)."""
    text = json.dumps(body, separators=(",", ":"))
    return {"v": 1, "kind": kind, "body": text, "mac": agent.sign(key, kind, text)}


class Dirs(unittest.TestCase):
    """A state and a config folder of our own for every test, and the request key."""

    def setUp(self):
        self.state = tempfile.mkdtemp()
        self.config = tempfile.mkdtemp()
        agent.STATE, agent.CONFIG = self.state, self.config
        with open(os.path.join(self.config, "rules.json"), "w") as f:
            json.dump({"version": 1, "rules": default_rules()}, f)
        with open(os.path.join(self.config, "notify.json"), "w") as f:
            json.dump({"channels": [], "requestKey": KEY}, f)

    def tearDown(self):
        shutil.rmtree(self.state, ignore_errors=True)
        shutil.rmtree(self.config, ignore_errors=True)

    def make(self, *args, **kw):
        """An Agent that's closed (workers, listeners, files) when the test ends."""
        a = agent.Agent(*args, **kw)
        self.addCleanup(a.close)
        return a


class Parity(unittest.TestCase):
    """health/fixtures/parity.json: the app's engine runs the same file (health/tests.rs)."""

    def test_the_agent_raises_the_same_events_as_the_app(self):
        with open(PARITY) as f:
            fixture = json.load(f)
        for case in fixture["cases"]:
            if case.get("macOnly"):
                continue  # about this Mac (its path to the cluster, its login): the agents can't see it
            rules = default_rules()
            if case["rule"] == "roce-iface" and not any(r["id"] == "roce-iface" for r in rules):
                # The app's rule list has it (health/roce_rules.rs); this snapshot of it may not yet.
                rules.append(dict(agent.CODED_DEFAULTS["roce-iface"], id="roce-iface", kind="coded", entity="net", enabled=True))
            e, t, c = agent.Engine(), agent.Trackers(), cfg(fixture["node"])
            got, step, base = [], 0, 1_800_000_000_000
            counters = {}
            for seg in case["segments"]:
                for _ in range(seg["secs"]):
                    step += 1
                    if case["metric"] == "roce":
                        for k, v in seg["perS"].items():
                            counters[k] = counters.get(k, 0) + v
                        fr = frame()
                        fr["nets"][0]["roce"] = {d["key"]: counters.get(d["key"], 0) for d in roce.IFACE_TABLE["counters"]}
                    else:
                        v = seg["value"]
                        fr = frame(**{"memPct": {"mem_pct": v}, "diskPct": {"disk_pct": v}, "gpuTemp": {"gpu_temp": v}, "clockOffsetMs": {"clock": v}}[case["metric"]])
                    now = base + step * 1000
                    conds = agent.evaluate(fr, rules, c, t, set(e.active), now)
                    for ev in e.step(conds, now):
                        if ev["alert"]["rule"] == case["rule"]:
                            got.append([step, ev["kind"], ev["alert"]["severity"]])
            self.assertEqual(got, case["expect"], case["name"])

    def test_the_agent_evaluates_the_listed_rules(self):
        with open(os.path.join(HERE, "fixtures", "agent-rules.json")) as f:
            listed = json.load(f)
        self.assertEqual(list(agent.EVALUATES), listed["coded"])
        self.assertEqual(list(agent.GENERIC_ENTITIES), listed["genericEntities"])
        self.assertEqual(listed["peerRule"], "peer-unreachable")
        self.assertIn(listed["peerRule"], agent.PEER_RULES)
        # Every rule has a definition: in the shipped rules, or the agent's own defaults for a
        # rules.json from a SparkBox that doesn't list it yet.
        ids = {r["id"] for r in default_rules()} | set(agent.CODED_DEFAULTS)
        self.assertTrue(set(agent.EVALUATES) <= ids, set(agent.EVALUATES) - ids)
        self.assertTrue(set(listed["endpointRules"]) <= set(agent.EVALUATES) | {r["id"] for r in default_rules()})


class Summaries(unittest.TestCase):
    def test_acronyms_stay_and_the_node_leads_like_rules_rs(self):
        self.assertEqual(agent.first_lower("RoCE ACK timeouts"), "RoCE ACK timeouts")
        self.assertEqual(agent.first_lower("Unified memory"), "unified memory")
        self.assertEqual(agent.short_label({"label": "enP2p1s0f1np1 on cell2", "node": "cell2"}), "cell2 enP2p1s0f1np1")
        self.assertEqual(agent.short_label({"label": "qsfp56-dd-1-2 (cell2) on CRS804 DDQ", "node": "cell2"}), "qsfp56-dd-1-2 (cell2)")


class Rules(unittest.TestCase):
    def run_frames(self, frames, rules=None, c=None, step_ms=1000):
        e, t, c = agent.Engine(), agent.Trackers(), c or cfg()
        rules = rules or default_rules()
        events, now = [], 1_800_000_000_000
        for fr in frames:
            now += step_ms
            events += e.step(agent.evaluate(fr, rules, c, t, e.active, now), now)
        return e, events

    def test_a_healthy_frame_raises_nothing(self):
        e, events = self.run_frames([frame()] * 120)
        self.assertEqual(events, [])

    def test_kernel_events_become_alerts_with_their_severity(self):
        now = 1_800_000_000_000
        xid = {"tsMs": now, "kind": "xid", "code": 79, "text": "NVRM: Xid (PCI:000f:01:00): 79, GPU has fallen off the bus.", "severity": "crit", "process": None}
        oom = {"tsMs": now, "kind": "oom", "code": None, "text": "Out of memory: Killed process 4242 (ollama)", "severity": "crit", "process": "ollama"}
        _, events = self.run_frames([frame(kernelEvents=[xid, oom])] * 2)
        raised = {(ev["alert"]["rule"], ev["alert"]["severity"]): ev for ev in events if ev["kind"] == "raised"}
        self.assertIn(("kernel-xid", "crit"), raised)
        self.assertEqual(raised[("kernel-xid", "crit")]["alert"]["summary"], "cell1 GPU Xid 79")
        self.assertEqual(raised[("kernel-xid", "crit")]["alert"]["message"], "Xid 79 · GPU fallen off the bus · until the node restarts")
        self.assertEqual(raised[("kernel-oom", "crit")]["alert"]["summary"], "ollama OOM-killed on cell1")

    def test_failed_units_exits_and_fabric_links(self):
        half2 = {"iface": "enP2p1s0f1np1", "role": "fabric", "up": True, "ip": "192.168.101.11", "mtu": 9000, "rxBps": 1e6, "txBps": 2e6,
                 "rxErrors": 0, "txErrors": 0, "carrierChanges": 2}
        up = frame()
        up["nets"].append(half2)
        down = frame()
        down["nets"][0] = dict(down["nets"][0], up=False, ip=None)
        down["nets"].append(half2)
        down["failedUnits"] = ["nvidia-dgx-telemetry.service"]
        down["containers"] = [{"name": "sglang-test", "state": "exited", "exitCode": 1, "finishedAt": 0, "restartCount": 0}]
        _, events = self.run_frames([up] + [down] * 5)
        rules = sorted({ev["alert"]["rule"] for ev in events if ev["kind"] == "raised"})
        self.assertEqual(rules, ["container-exited", "fabric-down", "unit-failed"], "one half down pages at once")
        fd = next(ev for ev in events if ev["alert"]["rule"] == "fabric-down")
        self.assertEqual(fd["alert"]["message"], "Fabric link enp1s0f1np1 down (192.168.100.11)", "the address is remembered")

    def test_both_halves_down_wait_for_the_switch_verdict(self):
        both = frame()
        both["nets"][0] = dict(both["nets"][0], up=False)
        _, events = self.run_frames([frame()] + [both] * 60)
        fd = [(ev["kind"], ev["alert"]["severity"], (ev["tsMs"] - 1_800_000_000_000) // 1000) for ev in events if ev["alert"]["rule"] == "fabric-down"]
        self.assertEqual(fd, [("raised", "crit", 2 + 45)], "every fabric link here down: 45 s for the others' views first, then critical")
        view = {"seen": ["cell1"], "minority": False, "switch": True}
        e, t, c = agent.Engine(), agent.Trackers(), cfg(switchMgmt={"id": "crs804", "name": "CRS804 DDQ", "host": "192.168.88.1"})
        conds = []
        for i, fr in enumerate([frame()] + [both] * 30):
            conds = agent.evaluate(fr, default_rules(), c, t, e.active, 1_800_000_000_000 + i * 2000, view=view)
            e.step(conds, 1_800_000_000_000 + i * 2000)
        fd = next(a for a in e.active.values() if a["rule"] == "fabric-down")
        self.assertEqual((fd["severity"], fd["incident"], fd["message"]), ("warn", "switch:crs804", "Fabric link enp1s0f1np1 down (192.168.100.11): CRS804 DDQ is down"),
                         "under the switch verdict: a warning in the switch's incident")

    def test_a_crash_loop_clears_once_the_container_stays_up(self):
        now_s = 1_800_000_000
        def c(restarts, state, started):
            return frame(containers=[{"name": "vllm-worker", "state": state, "restartCount": restarts, "startedAt": started}])
        frames = [c(0, "running", now_s)] + [c(1, "restarting", 0)] * 5 + [c(2, "running", now_s + 6)] * 700
        _, events = self.run_frames(frames)
        kinds = [(ev["kind"], ev["alert"]["severity"], (ev["tsMs"] - 1_800_000_000_000) // 1000) for ev in events if ev["alert"]["rule"] == "crash-loop"]
        self.assertEqual(kinds, [("raised", "crit", 2), ("resolved", "crit", 7 + 600)],
                         "raised at the first restart; it holds, never easing, until 10 min after the last restart seen (t=7, its count rose)")

    def test_generic_rate_rules_on_roce_counters(self):
        rule = {"id": "roce-oos", "area": "fabric", "title": "Out-of-sequence packets", "entity": "net", "kind": "rate", "metric": "oos",
                "op": ">", "warn": 10, "unit": "/s", "windowS": 30, "forS": 0, "clearS": 0, "enabled": True, "message": "{value} out of sequence"}
        frames = []
        for i in range(10):
            fr = frame()
            fr["nets"][0]["roce"] = {"outOfSequence": i * 50}
            frames.append(fr)
        _, events = self.run_frames(frames, rules=default_rules() + [rule])
        ev = next(ev for ev in events if ev["alert"]["rule"] == "roce-oos")
        self.assertEqual(ev["alert"]["message"], "50.0/s out of sequence")
        self.assertEqual(ev["alert"]["entity"]["id"], "enp1s0f1np1")

    def test_overrides_and_switches(self):
        rules = default_rules()
        mem = next(r for r in rules if r["id"] == "mem-high")
        mem["overrides"] = {"cell1": {"warn": 70}}
        _, events = self.run_frames([frame(mem_pct=75)] * 62, rules=rules)
        self.assertEqual([(ev["kind"], ev["alert"]["severity"]) for ev in events], [("raised", "warn")])
        mem["overrides"] = {"cell1": {"enabled": False}}
        _, events = self.run_frames([frame(mem_pct=99)] * 10, rules=rules)
        self.assertEqual(events, [])


class ServingMemory(unittest.TestCase):
    def test_shared_memory_streams(self):
        with open(os.path.join(HERE, "..", "health", "fixtures", "serving-memory.json")) as f:
            fixture = json.load(f)
        for case in fixture["cases"]:
            with self.subTest(case=case["name"]):
                e, t, config = agent.Engine(), agent.Trackers(), cfg()
                rules = default_rules()
                if case.get("headroomEnabled") is False:
                    next(r for r in rules if r["id"] == "mem-available")["enabled"] = False
                start, step, got = 1_800_000_000_000, 0, []
                for segment in case["segments"]:
                    for _ in range(segment["secs"]):
                        step += 1
                        available = int(segment.get("availableGiB", 6) * GIB)
                        fr = frame()
                        fr["mem"] = {"total": 256 * GIB, "used": 256 * GIB - available,
                                     "available": 0 if segment.get("invalidAvailable") else available,
                                     "gpuAlloc": int(segment.get("gpuGiB", 224) * GIB),
                                     "psiFull10": segment.get("psi", 0), "swapTotal": 16 * GIB,
                                     "swapUsed": int(segment.get("swapUsedGiB", 0) * GIB),
                                     "swapInBps": None if segment.get("swapMiBs", 0) is None else segment.get("swapMiBs", 0) * 1048576}
                        fr["gpu"]["readable"] = segment.get("gpuReadable", True)
                        if "share" in segment:
                            fr["containers"] = [{"name": "model-rank", "state": segment.get("state", "running"), "labels": {
                                "sparkbox.instance": "release", "sparkbox.deployment": "d", "sparkbox.path": "/model",
                                "sparkbox.role": segment.get("role", "single"), "sparkbox.gpu-share": segment["share"]}}]
                        if segment.get("oom"):
                            fr["kernelEvents"] = [{"tsMs": start + 1000, "kind": "oom", "severity": "crit", "process": "vllm", "text": "Out of memory: Killed process vllm"}]
                        now = start + step * 1000
                        conds = agent.evaluate(fr, rules, config, t, e.active, now)
                        for ev in e.step(conds, now):
                            if ev["alert"]["rule"] in case.get("focus", ["mem-high", "mem-available", "mem-pressure", "mem-swap-in", "kernel-oom"]):
                                got.append([step, ev["alert"]["rule"], ev["kind"], ev["alert"]["severity"]])
                self.assertEqual(got, case["expect"])


class GateParity(unittest.TestCase):
    """fixtures/gate-parity.json, run the way its comment says (notify.rs runs the same file)."""

    def run_case(self, case):
        g = gate.Gate(gate.Settings(case["settings"]))
        got = []
        for t, events in itertools.groupby(case["events"], key=lambda e: e["t"]):
            events = [dict(e, untilMs=e["until"] * 1000) if e["kind"] == "maintenance" else e for e in events]
            now = t * 1000
            if any(e["kind"] == "tick" for e in events):
                g.tick(now)
            outcomes = g.decide([e for e in events if e["kind"] != "tick"], now)
            messages = g.flush(now + gate.GROUP_WAIT_MS)
            got.append({"t": t, "outcomes": [{k: o[k] for k in ("rule", "entity", "outcome")} for o in outcomes],
                        "messages": messages})
        return got

    def test_every_case_in_gate_parity_json(self):
        with open(GATE_PARITY) as f:
            fixture = json.load(f)
        self.assertEqual((fixture["groupWaitS"] * 1000, fixture["duplicateS"] * 1000, fixture["coalesceS"] * 1000),
                         (gate.GROUP_WAIT_MS, gate.DUPLICATE_MS, gate.COALESCE_MS))
        names = [c["name"].split()[0] for c in fixture["cases"]]
        for g in ["G%d" % i for i in range(1, 18)]:
            self.assertTrue(any(n == g or n.startswith(g) and not n[len(g):].isdigit() for n in names), g)
        passed = 0
        for case in fixture["cases"]:
            with self.subTest(case["name"]):
                got = self.run_case(case)
                self.assertEqual([x["t"] for x in got], [x["t"] for x in case["expect"]])
                for have, want in zip(got, case["expect"]):
                    self.assertEqual(have["outcomes"], want["outcomes"], "t=%s outcomes" % want["t"])
                    self.assertEqual([(m["title"], m["severity"]) for m in have["messages"]],
                                     [(m["title"], m["severity"]) for m in want["messages"]], "t=%s messages" % want["t"])
                    for m, w in zip(have["messages"], want["messages"]):
                        if "lines" in w:
                            self.assertEqual(m["lines"], w["lines"], "t=%s lines" % want["t"])
                passed += 1
        self.assertEqual(passed, len(fixture["cases"]))
        print("\ngate-parity.json: %d/%d cases (G1–G17 and C1–C4) pass" % (passed, len(fixture["cases"])), file=sys.stderr)

    def test_quiet_hours_without_a_zone_follow_the_node_and_say_so(self):
        self.assertEqual(gate.Settings({"quiet": {"start": "22:00", "end": "07:00"}}).quiet_tz(), "missing")
        self.assertEqual(gate.Settings({"quiet": {"start": "22:00", "end": "07:00", "tz": "Mars/Olympus"}}).quiet_tz(), "missing")
        self.assertEqual(gate.Settings({"quiet": {"start": "22:00", "end": "07:00", "tz": "Europe/Paris"}}).quiet_tz(), "Europe/Paris")
        self.assertIsNone(gate.Settings({}).quiet_tz())
        # r2's notify.json policy reads too.
        old = gate.Settings({"quietHours": True, "quietStart": "22:00", "quietEnd": "07:00", "criticalOnly": True, "disk": False})
        self.assertEqual((old.quiet["start"], old.critical_only, old.category_on("disk"), old.category_on("gpu")), ("22:00", True, False, True))

    def test_a_restart_keeps_what_was_paged(self):
        g = gate.Gate()
        ev = {"kind": "raised", "rule": "node-offline", "entity": "node:cell3", "severity": "crit", "incident": "node:cell3", "title": "cell3 offline"}
        self.assertEqual(g.decide([ev], 0)[0]["outcome"], "sent")
        again = gate.Gate(state=json.loads(json.dumps(g.state())))
        self.assertEqual(len(again.flush(gate.GROUP_WAIT_MS)), 1, "a message still in its window goes out after a restart")
        self.assertEqual(again.decide([dict(ev, kind="resolved", title="cell3 is back online")], 60_000)[0]["outcome"], "sent",
                         "the resolve of what was paged before the restart is sent at once")
        later = gate.Gate(state=json.loads(json.dumps(again.state())))
        self.assertEqual([m["title"] for m in later.flush(60_000 + gate.GROUP_WAIT_MS)], ["cell3 is back online"],
                         "it goes out with its window, after another restart too")
        self.assertEqual(later.tick(60_000 + gate.FLAP_MS), [], "nothing waits to be sent later")

    def test_incident_roots_json(self):
        with open(os.path.join(HERE, "..", "health", "fixtures", "incident-roots.json")) as f:
            fixture = json.load(f)
        for case in fixture["cases"]:
            if case.get("macOnly"):
                continue  # this Mac's own path to the cluster (health/reach.rs): the agents can't see it
            with self.subTest(case["name"]):
                roots = gate.Roots(fixture["ifaces"], fixture["gpuFault"], fixture["endpoints"], case["active"])
                self.assertEqual(roots.root_of(case["alert"]), case["root"])
        self.assertTrue(gate.same_entity("disk:gx10-0000:/", gate.entity_key({"kind": "disk", "id": "/", "node": "gx10-0000"})))
        self.assertTrue(gate.same_entity("container:cell1:vllm-head", "container:cell1/vllm-head"))


class RoceIface(unittest.TestCase):
    """roce-iface: health/fixtures/roce-iface.json's cases (health/roce_rules.rs runs them too),
    and a whole loss-then-recovery run through the agent's own rate windows."""

    def test_the_table_is_the_apps(self):
        with open(ROCE_IFACE) as f:
            table = json.load(f)
        self.assertEqual({k: table[k] for k in roce.IFACE_TABLE}, roce.IFACE_TABLE)

    def test_the_cases_in_roce_iface_json(self):
        with open(ROCE_IFACE) as f:
            table = json.load(f)
        for case in table["cases"]:
            with self.subTest(case["name"]):
                rates = {k: (v[0], v[1]) for k, v in case["rates"].items()}
                j = roce.judge(None, rates, case.get("pauseStormS"), case["active"])
                self.assertEqual(None if j is None else {"severity": j[0], "message": j[1]}, case["expect"])

    def counters(self, oos=0, crc=0):
        return {"outOfSequence": oos, "packetSeqErr": 0, "localAckTimeoutErr": 0, "rxCrcErrorsPhy": crc, "rxSymbolErrPhy": 0,
                "ecnMarked": oos * 3, "cnpSent": oos * 2, "txPauseCtrl": 0, "rxPauseCtrl": 0}

    def test_one_alert_per_interface_and_it_clears_once_the_counters_stop(self):
        e, t, c = agent.Engine(), agent.Trackers(), cfg("cell2")
        rules = default_rules() + [{"id": "roce-iface", "kind": "coded", "entity": "net", "area": "fabric", "forS": 0, "clearS": 30, "enabled": True}]
        now, oos, crc, events = 1_800_000_000_000, 0, 0, []

        def step(oos_rate, crc_rate):
            nonlocal now, oos, crc
            now += 2000
            oos += oos_rate * 2
            crc += crc_rate * 2
            fr = frame()
            fr["nets"][0] = dict(fr["nets"][0], iface="enP2p1s0f1np1", ip="192.168.101.12", roce=self.counters(oos, crc))
            out = e.step(agent.evaluate(fr, rules, c, t, set(e.active), now), now)
            events.extend((now, ev) for ev in out)
            return out
        for _ in range(30):            # a minute clean
            step(0, 0)
        for _ in range(45):            # 90 s of loss with CRC errors (the roce-loss scenario)
            step(160, 18)
        for _ in range(60):            # then the counters stop dead
            step(0, 0)
        roce_events = [(ts, ev) for ts, ev in events if ev["alert"]["rule"] == "roce-iface"]
        kinds = [(ev["kind"], ev["alert"]["severity"]) for _, ev in roce_events]
        self.assertEqual(kinds[:2], [("raised", "warn"), ("escalated", "crit")], "CRC errors at once; critical as the 10 s rates fill")
        self.assertEqual(kinds[-1][0], "resolved")
        self.assertEqual(sum(1 for k, _ in kinds if k == "raised"), 1, "one alert for the interface, not one per counter")
        raised = roce_events[0][1]["alert"]
        self.assertEqual(raised["summary"], "cell2 half 2: RoCE retransmitting · CRC errors rising · check the optic or cable")
        self.assertEqual(raised["incident"], "link:cell2/enP2p1s0f1np1")
        stopped = 1_800_000_000_000 + (30 + 45) * 2000
        resolved_at = roce_events[-1][0]
        self.assertLessEqual(resolved_at - stopped, (30 + 30) * 1000, "resolves within clear_s + 30 s of the counters stopping")
        self.assertNotIn("roce-out-of-sequence", {ev["alert"]["rule"] for _, ev in events}, "the per-counter rules are folded in")


class MoreRules(Dirs):
    def run_frames(self, frames, rules=None, c=None, step_ms=2000, start=1_800_000_000_000):
        e, t, c = agent.Engine(), agent.Trackers(), c or cfg()
        events, now = [], start
        for fr in frames:
            now += step_ms
            events += e.step(agent.evaluate(fr, rules or default_rules(), c, t, e.active, now), now)
        return e, events

    def test_an_unreadable_gpu_is_critical_after_30_s_and_its_numbers_arent_zero(self):
        dead = frame()
        dead["gpu"] = {"utilPct": None, "tempC": None, "powerW": None, "smClockMhz": None, "procs": [], "throttle": [],
                       "error": "nvidia-smi didn't answer in 10 s"}
        e, events = self.run_frames([dead] * 14)
        self.assertEqual(events, [], "not for the first 30 s")
        e, events = self.run_frames([dead] * 17)
        a = next(ev["alert"] for ev in events if ev["kind"] == "raised")
        self.assertEqual((a["rule"], a["severity"], a["message"]), ("gpu-unreadable", "crit", "Can't read the GPU: nvidia-smi didn't answer in 10 s"))
        self.assertNotIn("gpu-hot", {ev["alert"]["rule"] for ev in events}, "null isn't 0 °C either")

    def test_a_fatal_xid_from_an_hour_before_connecting_is_still_firing_hours_later(self):
        # The collector read this boot's kernel log from the start: the Xid 79 an hour before
        # the agent (or SparkBox) connected is in the frame with its own time.
        with open(JOURNAL) as f:
            events = [collector.classify_kernel(msg, ts) for ts, msg in map(collector.parse_journal_line, f)]
        events = [x for x in events if x]
        xid79 = next(x for x in events if x.get("code") == 79)
        self.assertEqual(xid79["tsMs"], 1761660000000)
        fr = frame(kernelEvents=events)
        start = xid79["tsMs"] + 3_600_000
        e, got = self.run_frames([fr] * 2, start=start)
        raised = {ev["alert"]["id"] for ev in got if ev["kind"] == "raised"}
        self.assertIn("kernel-xid:gpu:cell1:xid-79", raised, "an Xid 79 from an hour before is reported")
        self.assertNotIn("kernel-xid:gpu:cell1:xid-13", raised, "a warning Xid an hour old has passed its window")
        e, got = self.run_frames([fr] * 2, start=start + 2 * 3_600_000)
        self.assertIn("kernel-xid:gpu:cell1:xid-79", {ev["alert"]["id"] for ev in got if ev["kind"] == "raised"},
                      "it holds until the node restarts (a new boot reads a new log)")


def addresses(node):
    i = NODES.index(node) + 1
    return {"lan": "192.168.1.10%d" % i, "fabric": ["192.168.100.1%d" % i, "192.168.101.1%d" % i]}


def roster():
    """agent.json peers as SparkBox writes them: every node, this one included."""
    return [dict({"id": n, "name": n, "index": i}, **addresses(n)) for i, n in enumerate(NODES)]


class Net:
    """A simulated cluster: which node reaches which address, which nodes and agents are up.
    Each agent's /healthz answers with the view it computed in the last round."""

    def __init__(self):
        self.owner = {a: n for n in NODES for a in [addresses(n)["lan"]] + addresses(n)["fabric"]}
        self.cut = set()            # (from node, address) that doesn't connect
        self.off = set()            # nodes that are powered off
        self.agent_down = set()     # nodes up, agent not running (port refused)
        self.wedged = set()         # agent up, says 503 wedged
        self.hung = set()           # out of memory: the kernel accepts connections, nothing answers on them
        self.no_gateway = set()     # nodes that can't reach their gateway
        self.views = {}

    def reach(self, src, addr):
        return src not in self.off and self.owner[addr] not in self.off and (src, addr) not in self.cut

    def fetch_for(self, src):
        def fetch(addr, port):
            if not self.reach(src, addr):
                raise OSError("timed out")
            dst = self.owner[addr]
            if dst in self.hung:
                raise OSError("timed out")
            if dst in self.agent_down:
                raise ConnectionRefusedError("refused")
            if dst in self.wedged:
                return 503, {"node": dst, "version": agent.VERSION, "lastCycleMs": 1, "wedged": True}
            return 200, self.views.get(dst) or {"node": dst}
        return fetch

    def tcp_for(self, src):
        # ssh_answers: a hung node accepts in the kernel but never greets.
        return lambda addr, port, timeout=None: self.reach(src, addr) and self.owner[addr] not in self.hung

    def gateway_for(self, src):
        return lambda: src not in self.no_gateway and src not in self.off

    def sever(self, a, b, kinds=("lan", "fabric")):
        """No path of these kinds between nodes a and b, either way."""
        for x, y in ((a, b), (b, a)):
            for kind in kinds:
                for addr in ([addresses(y)["lan"]] if kind == "lan" else addresses(y)["fabric"]):
                    self.cut.add((x, addr))

    def watches(self, ticks=range(0, 91, 15)):
        ws = {n: peers.PeerWatch(n, i, roster(), 9469, fetch=self.fetch_for(n), tcp=self.tcp_for(n), gateway=self.gateway_for(n), start=0.0)
              for i, n in enumerate(NODES) if n not in self.off}
        for t in ticks:
            for w in ws.values():
                w.poll(float(t), t * 1000)
            for n, w in ws.items():
                lead, iso, _ = w.judge(float(t))
                self.views[n] = {"node": n, "peers": w.view(float(t)), "isolated": iso, "leader": lead, "gateway": w.gateway_ok(float(t))}
        return ws, float(ticks[-1])

    def findings(self, ticks=range(0, 91, 15)):
        """{node: (leader, isolated, [(rule, severity, about)])} after 90 s (hung and powered-off
        nodes run no agent)."""
        ws, t = self.watches(ticks)
        out = {}
        for n, w in ws.items():
            if n in self.hung:
                continue
            lead, iso, found = w.judge(t)
            out[n] = (lead, iso, sorted((r, s, p.id if p else None) for r, s, p, _ in found))
        return out


class SplitVsOutage(unittest.TestCase):
    """Every agent polls every path of every peer; the lowest-index agent that reaches its
    gateway and has no healthy lower-index peer pages about peers (no quorum), and what it
    pages tells a split from an outage. Deaths, not just severs."""

    def test_a_healthy_cluster_has_one_leader_and_nothing_to_say(self):
        f = Net().findings()
        self.assertEqual({n: v[0] for n, v in f.items()}, {"cell1": True, "cell2": False, "cell3": False, "gx10-0000": False})
        self.assertTrue(all(v[2] == [] for v in f.values()))

    def test_a_node_that_is_off_is_unreachable_and_paged_once(self):
        net = Net()
        net.off.add("cell3")
        f = net.findings()
        self.assertEqual(f["cell1"][2], [("peer-unreachable", "crit", "cell3")])
        self.assertEqual([v[2] for n, v in f.items() if n != "cell1"], [[], []])

    def test_full_partition_pages_no_peer_unreachable(self):
        net = Net()
        for a, b in itertools.combinations(NODES, 2):
            net.sever(a, b)
        net.no_gateway.update(NODES)  # the switches are gone: no gateway either
        f = net.findings()
        for n, (lead, iso, found) in f.items():
            self.assertFalse(lead, n)
            self.assertEqual(iso, "network", n)
            self.assertEqual(found, [("fabric-lost", "crit", None)], "%s says it's cut off, never blames its peers" % n)

    def test_one_node_losing_the_fabric_says_so_itself_and_the_others_only_warn(self):
        net = Net()
        for other in NODES:
            if other != "cell2":
                net.sever("cell2", other, kinds=("fabric",))
        f = net.findings()
        self.assertEqual(f["cell2"], (False, "fabric", [("fabric-lost", "crit", None)]), "cell2 lost the fabric (critical, about itself)")
        self.assertEqual(f["cell1"], (True, None, [("fabric-lost", "warn", "cell2")]), "the leader: cell2 isn't reachable over the fabric")
        self.assertEqual((f["cell3"][2], f["gx10-0000"][2]), ([], []))
        self.assertNotIn("peer-unreachable", {r for v in f.values() for r, _, _ in v[2]})

    def test_the_leader_losing_the_fabric_hands_over(self):
        net = Net()
        for other in NODES[1:]:
            net.sever("cell1", other, kinds=("fabric",))
        f = net.findings()
        self.assertEqual(f["cell1"], (False, "fabric", [("fabric-lost", "crit", None)]))
        self.assertEqual(f["cell2"], (True, None, [("fabric-lost", "warn", "cell1")]), "cell2 leads: cell1 says it's cut off from the fabric")

    def test_a_crashed_agent_with_ssh_up_is_a_warning_only(self):
        net = Net()
        net.agent_down.add("cell3")
        self.assertEqual(net.findings()["cell1"][2], [("agent-wedged", "warn", "cell3")])
        net = Net()
        net.wedged.add("cell3")
        f = net.findings()
        self.assertEqual(f["cell1"][2], [("agent-wedged", "warn", "cell3")], "a 503 wedged /healthz is the same warning")
        self.assertTrue(f["cell1"][0])

    def test_two_dead_including_the_lowest_are_paged_by_the_next(self):
        net = Net()
        net.off.update({"cell1", "cell2"})
        f = net.findings()
        self.assertEqual(f["cell3"], (True, None, [("peer-unreachable", "crit", "cell1"), ("peer-unreachable", "crit", "cell2")]),
                         "no quorum needed: cell3 reaches its gateway and nobody lower answers")
        self.assertEqual(f["gx10-0000"], (False, None, []), "cell3 answers and is healthy, so gx10-0000 leaves it to cell3")

    def test_three_dead_are_paged_by_the_last_one_standing(self):
        net = Net()
        net.off.update({"cell1", "cell2", "cell3"})
        f = net.findings()
        self.assertEqual(f, {"gx10-0000": (True, None, [("peer-unreachable", "crit", n) for n in ("cell1", "cell2", "cell3")])})
        net = Net()
        net.off.update({"cell2", "cell3", "gx10-0000"})
        self.assertEqual(net.findings()["cell1"][2], [("peer-unreachable", "crit", n) for n in ("cell2", "cell3", "gx10-0000")])

    def test_the_pp3_trio_hung_out_of_memory_is_three_criticals(self):
        # llama-3.3-70b's three pipeline stages at 95-98 GiB: a KV spike hangs them together. Their
        # kernels still accept TCP, so only SSH's greeting tells them from a crashed agent.
        net = Net()
        net.hung.update({"cell1", "cell2", "cell3"})
        f = net.findings()
        self.assertEqual(f, {"gx10-0000": (True, None, [("peer-unreachable", "crit", n) for n in ("cell1", "cell2", "cell3")])})

    def test_a_node_cut_off_from_everyone_but_its_gateway_pages_them_as_a_minority(self):
        net = Net()
        for other in NODES:
            if other != "cell2":
                net.sever("cell2", other)
        ws, t = net.watches()
        lead, iso, found = ws["cell2"].judge(t)
        self.assertEqual((lead, iso), (True, None), "its gateway answers: it isn't the one cut off as far as it can tell")
        self.assertEqual(sorted(p.id for r, _, p, _ in found if r == "peer-unreachable"), ["cell1", "cell3", "gx10-0000"])
        self.assertTrue(ws["cell2"].minority(t), "and its page says it's the only one seeing it")
        self.assertEqual(net.findings()["cell1"][2], [("peer-unreachable", "crit", "cell2")], "the other side pages cell2: a duplicate, never silence")

    def test_a_two_two_split_where_one_side_lost_the_gateway_pages_from_the_other_side_only(self):
        net = Net()
        for a in ("cell1", "cell2"):
            for b in ("cell3", "gx10-0000"):
                net.sever(a, b)
        net.no_gateway.update({"cell1", "cell2"})
        f = net.findings()
        self.assertEqual(f["cell3"], (True, None, [("peer-unreachable", "crit", "cell1"), ("peer-unreachable", "crit", "cell2")]))
        self.assertEqual([f[n][2] for n in ("cell1", "cell2", "gx10-0000")], [[], [], []],
                         "the side without its gateway can't tell a split from its own outage, so it doesn't lead")
        # With the gateway on both sides it's a true split: each side pages the other (the Mac dedupes by rule and entity).
        net.no_gateway.clear()
        f = net.findings()
        self.assertEqual((f["cell1"][0], f["cell3"][0]), (True, True))

    def test_a_leader_that_loses_its_gateway_stops_leading(self):
        net = Net()
        for other in NODES[1:]:
            net.sever("cell1", other, kinds=("lan",))
        net.no_gateway.add("cell1")
        net.off.add("cell3")
        f = net.findings()
        self.assertFalse(f["cell1"][0], "cell1 still has the fabric, but its pages couldn't leave the house")
        self.assertEqual(f["cell2"], (True, None, [("peer-unreachable", "crit", "cell3")]), "cell2 takes over: cell1 says it has no gateway")

    def test_a_dead_switch_is_one_switch_page_and_every_node_a_warning_child(self):
        net = Net()
        for a, b in itertools.combinations(NODES, 2):
            net.sever(a, b, kinds=("fabric",))
        f = net.findings(range(0, 106, 15))
        self.assertEqual(f["cell1"], (True, "fabric", [("fabric-lost", "warn", None), ("fabric-switch-down", "crit", None)]),
                         "the lowest-index node with its gateway pages the switch; nobody blames a peer")
        for n in NODES[1:]:
            self.assertEqual(f[n], (False, "fabric", [("fabric-lost", "warn", None)]), n)

    def test_a_dead_switch_is_paged_by_the_next_node_when_the_first_has_no_gateway(self):
        net = Net()
        for a, b in itertools.combinations(NODES, 2):
            net.sever(a, b, kinds=("fabric",))
        net.no_gateway.add("cell1")
        f = net.findings(range(0, 106, 15))
        self.assertEqual([r for r, _, _ in f["cell1"][2]], ["fabric-lost"])
        self.assertEqual(f["cell2"], (True, "fabric", [("fabric-lost", "warn", None), ("fabric-switch-down", "crit", None)]))

    def test_ssh_greeting_tells_a_live_node_from_a_hung_one(self):
        def server(greet):
            srv = socket.socket()
            srv.bind(("127.0.0.1", 0))
            srv.listen(4)
            held = []
            def serve():
                while True:
                    try:
                        conn, _ = srv.accept()
                    except OSError:
                        return
                    if greet:
                        conn.sendall(b"SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5\r\n")
                    held.append(conn)
            threading.Thread(target=serve, daemon=True).start()
            self.addCleanup(lambda: [c.close() for c in held + [srv]])
            return srv.getsockname()[1]
        self.assertTrue(peers.ssh_answers("127.0.0.1", server(True), timeout=1.0))
        self.assertFalse(peers.ssh_answers("127.0.0.1", server(False), timeout=0.5), "accepted but silent: hung")
        self.assertTrue(peers.ssh_answers("127.0.0.1", free_port(), timeout=0.5), "refused: the kernel answers, the node is up")

    def test_the_view_has_every_path(self):
        net = Net()
        for other in NODES:
            if other != "cell2":
                net.sever("cell2", other, kinds=("fabric",))
        ws, t = net.watches()
        v = ws["cell1"].view(t)
        self.assertEqual({k: v["cell2"][k] for k in ("lan", "fabric", "wedged")}, {"lan": True, "fabric": False, "wedged": False})
        self.assertEqual(v["cell3"]["lan"], True)
        self.assertIsInstance(v["cell3"]["lastOkMs"], int)
        self.assertEqual(ws["cell1"].missing(t), [])

    def test_the_view_says_what_was_heard_not_the_benefit_of_the_doubt(self):
        net = Net()
        net.off.add("cell3")
        w = peers.PeerWatch("cell1", 0, roster(), 9469, fetch=net.fetch_for("cell1"), tcp=net.tcp_for("cell1"), gateway=net.gateway_for("cell1"), start=0.0)
        self.assertEqual({k: w.view(0.0)["cell3"][k] for k in ("lan", "fabric", "ssh")}, {"lan": None, "fabric": None, "ssh": None}, "not polled yet")
        w.poll(0.0, 0)
        self.assertEqual({k: w.view(0.0)["cell3"][k] for k in ("lan", "fabric", "ssh")}, {"lan": False, "fabric": False, "ssh": False},
                         "nothing answered, even though it isn't called missing for a minute")
        self.assertEqual(w.judge(0.0)[2], [], "a minute's grace before anything is said")
        self.assertEqual(w.view(0.0)["cell2"]["lan"], True)

    def test_the_gateway_comes_from_proc_net_route(self):
        route = ("Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n"
                 "enP7s7\t00000000\t0132A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n"
                 "enP7s7\t0032A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n")
        self.assertEqual(peers.default_gateway(route), "192.168.50.1")
        self.assertIsNone(peers.default_gateway(route.splitlines()[0]))


class LeaderPages(Dirs):
    """The whole path in one agent: the peer watch, the rules, the gate and the outbox."""

    def agent_for(self, node, net, sent):
        index = NODES.index(node)
        c = agent.Config({"node": node, "name": node, "index": index, "peers": roster(), "repeatMinutes": 60})
        w = peers.PeerWatch(node, index, roster(), 9469, fetch=net.fetch_for(node), tcp=net.tcp_for(node), gateway=net.gateway_for(node), start=0.0)
        a = self.make(c, collect=lambda: frame(), outbox_senders={"ntfy": lambda ch, m: sent.append((node, m["title"], m["severity"]))}, watch=w)
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        return a

    def cluster(self, net, nodes, ticks, sent, frames=None, before=None, extra=None):
        """Full agents on `nodes`, each with its own state folder, polled and cycled every 15 s;
        each publishes its /healthz after its cycle, as the real one does every 2 s."""
        base = 1_800_000_000_000
        agents = {}
        for n in nodes:
            agent.STATE = os.path.join(self.state, n)
            index = NODES.index(n)
            c = agent.Config(dict({"node": n, "name": n, "index": index, "peers": roster(), "repeatMinutes": 60, "tz": "UTC"}, **(extra or {})))
            w = peers.PeerWatch(n, index, roster(), 9469, fetch=net.fetch_for(n), tcp=net.tcp_for(n), gateway=net.gateway_for(n), start=0.0)
            a = self.make(c, collect=(lambda n=n: (frames or {}).get(n, frame)()), outbox_senders={"ntfy": lambda ch, m, n=n: sent.append((n, m["title"], m["severity"]))},
                          watch=w)
            a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
            agents[n] = a
        for t in ticks:
            if before:
                before(t)
            for n, a in agents.items():
                agent.STATE = os.path.join(self.state, n)
                a.watch.poll(float(t), base + t * 1000)
            for n, a in agents.items():
                agent.STATE = os.path.join(self.state, n)
                a.cycle(float(t), base + t * 1000)
                net.views[n] = a.health(float(t))
        for n, a in agents.items():
            agent.STATE = os.path.join(self.state, n)
            a.outbox.drain()
        return agents

    def outcomes(self, node, rule):
        with open(os.path.join(self.state, node, "deliveries.jsonl")) as f:
            return [l["outcome"] for l in map(json.loads, f) if l.get("rule") == rule]

    def test_three_nodes_dead_together_are_one_page_from_the_survivor(self):
        net, sent = Net(), []
        net.off.update({"cell1", "cell2", "cell3"})
        self.cluster(net, ["gx10-0000"], range(0, 106, 15), sent)
        self.assertEqual(sent, [("gx10-0000", "cell1, cell2 and cell3 aren't answering on any path (seen from gx10-0000 only)", "crit")])

    def test_two_dead_including_the_lowest_are_one_page_from_the_next(self):
        net, sent = Net(), []
        net.off.update({"cell1", "cell2"})
        self.cluster(net, ["cell3", "gx10-0000"], range(0, 106, 15), sent)
        self.assertEqual(sent, [("cell3", "cell1 and cell2 aren't answering on any path (seen from cell3 and gx10-0000 only)", "crit")])

    def test_a_dead_switch_is_one_page(self):
        # The CRS804 loses power at t=30: every fabric link on every node goes down, and nothing
        # crosses the fabric. The LAN and the gateway still work.
        net, sent, dead = Net(), [], [False]
        def fr():
            f = frame()
            f["nets"][0] = dict(f["nets"][0], up=not dead[0])
            return f
        def at(t):
            if t >= 30 and not dead[0]:
                dead[0] = True
                for a, b in itertools.combinations(NODES, 2):
                    net.sever(a, b, kinds=("fabric",))
        sw = {"switchMgmt": {"id": "crs804", "name": "CRS804 DDQ", "host": "192.168.88.1"}}
        agents = self.cluster(net, list(NODES), range(0, 241, 15), sent, frames={n: fr for n in NODES}, before=at, extra=sw)
        self.assertEqual(sent, [("cell1", "CRS804 DDQ isn't answering: all 4 nodes lost the fabric at %s" % gate.clock(1_800_000_030_000, "UTC"), "crit")],
                         "one page from the lowest-index node with its gateway; no node blames itself or a peer")
        for n in NODES:
            self.assertEqual(self.outcomes(n, "fabric-down"), ["coalesced"], "%s: its link is the switch page's child" % n)
            self.assertEqual(self.outcomes(n, "fabric-lost"), ["coalesced"], n)
        self.assertEqual(agents["cell1"].health()["switchDown"]["incident"], "switch:crs804")

    def test_a_peer_off_is_paged_by_the_leader_only_and_a_crashed_agent_is_a_warning(self):
        net, sent = Net(), []
        net.off.add("cell3")
        net.agent_down.add("gx10-0000")
        agents = [self.agent_for(n, net, sent) for n in ("cell1", "cell2")]
        base = 1_800_000_000_000
        for t in range(0, 106, 15):
            for a in agents:
                a.watch.poll(float(t), base + t * 1000)
                net.views[a.cfg.node] = a.health(float(t))
            for a in agents:
                a.cycle(float(t), base + t * 1000)
        for a in agents:
            a.outbox.drain()
        self.assertEqual(sorted(sent), [("cell1", "The agent on gx10-0000 isn't answering", "warn"), ("cell1", "cell3 isn't answering on any path", "crit")])
        st = agents[0].health()
        self.assertEqual((st["leader"], st["missing"], st["isolated"]), (True, ["cell3"], None))
        self.assertEqual(agents[1].health()["missing"], [], "only the agent that pages reports a node missing")


class CrashLoops(unittest.TestCase):
    """fixtures/docker-states.json: one crash loop is one raise and one resolve, and at most two
    messages through the gate (health/tests.rs runs the same file)."""

    @staticmethod
    def samples(fixture, case):
        def expand(phases):
            for p in phases:
                if "repeat" in p:
                    for _ in range(p["repeat"]):
                        yield from expand(p["phases"])
                else:
                    yield p
        out, t, restarts, step = [], 0, 0, fixture["sampleS"]
        for p in expand(case["phases"]):
            began, end = t, t + p["forS"]
            while t < end:
                if t == began and (p["state"] == "restarting" or p.get("restarted")):
                    restarts += 1
                started = fixture["startS"] + began if p["state"] == "running" else 0
                out.append((t, {"name": case["container"], "state": p["state"], "restartCount": restarts, "startedAt": started}))
                t += step
        return out

    def run_case(self, fixture, case):
        e, tr, c = agent.Engine(), agent.Trackers(), cfg(crash={"recentS": fixture["crashRecentS"], "windowS": fixture["crashWindowS"]})
        g = gate.Gate(gate.Settings({"criticalOnly": False, "resolved": True, "repeatMinutes": 60}))
        endpoints = {"ep": {"result": {"health": "ready"}, "endpoint": {"id": "ep", "containers": [case["container"]]}}} \
            if (case.get("endpoint") or {}).get("answering") else None
        events, messages = [], []
        for t, container in self.samples(fixture, case):
            now = (fixture["startS"] + t) * 1000
            evs = [ev for ev in e.step(agent.evaluate(frame(containers=[container]), default_rules(), c, tr, e.active, now, None, endpoints), now)
                   if ev["alert"]["rule"] == "crash-loop"]
            events += [(ev["kind"], ev["alert"]["severity"], t) for ev in evs]
            g.decide([agent.Agent.gate_event(None, ev) for ev in evs], now)
            g.tick(now)
            messages += g.flush(now)
        return events, messages

    def test_every_case_in_docker_states_json(self):
        with open(DOCKER_STATES) as f:
            fixture = json.load(f)
        self.assertEqual((fixture["crashRecentS"], fixture["crashWindowS"]), (agent.CRASH_RECENT_S, agent.CRASH_WINDOW_S))
        exact = {"D1": 898, "D2": 640, "D3": 1098, "D4": None, "D5": 700, "D6": 1198}
        for case in fixture["cases"]:
            with self.subTest(case["name"]):
                events, messages = self.run_case(fixture, case)
                want = case["expect"]
                self.assertEqual([(s, t) for k, s, t in events if k == "raised"], [(r["severity"], r["t"]) for r in want["raised"]])
                self.assertEqual([k for k, _, _ in events if k not in ("raised", "resolved")], [], "never escalates or eases")
                resolved = [t for k, _, t in events if k == "resolved"]
                if want["resolvedBetween"] is None:
                    self.assertEqual(resolved, [])
                else:
                    lo, hi = want["resolvedBetween"]
                    self.assertEqual(len(resolved), 1)
                    self.assertTrue(lo <= resolved[0] <= hi, (resolved, lo, hi))
                # The agent's own timing: exactly 10 min after the last restart it saw.
                self.assertEqual(resolved[0] if resolved else None, exact[case["name"].split()[0]])
                self.assertLessEqual(len(messages), want["messagesAtMost"], [m["title"] for m in messages])
        print("\ndocker-states.json: %d/%d crash-loop cases pass" % (len(fixture["cases"]), len(fixture["cases"])), file=sys.stderr)

    def test_the_round_three_loop_sends_two_messages_not_fifteen(self):
        # The critic's probe: 8 s running, 12 s restarting, sampled every second for 5 minutes, then up.
        fixture = {"sampleS": 1, "startS": 1_800_000_000, "crashRecentS": 60, "crashWindowS": 600}
        case = {"container": "vllm-worker", "phases": [{"repeat": 15, "phases": [{"state": "running", "forS": 8}, {"state": "restarting", "forS": 12}]},
                                                       {"state": "running", "forS": 1300}]}
        events, messages = self.run_case(fixture, case)
        self.assertEqual([(k, s) for k, s, _ in events], [("raised", "crit"), ("resolved", "crit")])
        self.assertEqual([m["title"] for m in messages], ["vllm-worker crash-looping on cell1", "Resolved: vllm-worker crash-looping on cell1"])


class GpuThrottle(unittest.TestCase):
    """health/fixtures/gpu-throttle-cases.json (alerts' file): an idle GB10 alternating [] and
    ["SW power cap"] never throttles; busy and capped, or a hardware slowdown, does."""

    def setUp(self):
        with open(GPU_THROTTLE) as f:
            self.fixture = json.load(f)

    def test_the_verdicts(self):
        for v in self.fixture["verdicts"]:
            with self.subTest(v["name"]):
                got = agent.throttle_verdict(v["gpu"])
                self.assertEqual((got is not None, got[1] if got else None), (v["throttled"], v["text"]))

    def test_the_cases_through_the_rule(self):
        rule = next(r for r in default_rules() if r["id"] == "gpu-throttle")
        self.assertEqual((rule["forS"], rule["clearS"]), (self.fixture["rule"]["forS"], self.fixture["rule"]["clearS"]))
        for case in self.fixture["cases"]:
            with self.subTest(case["name"]):
                readings = [seg["cycle"][i % len(seg["cycle"])] for seg in case["segments"] for i in range(seg["secs"])]
                e, tr, c = agent.Engine(), agent.Trackers(), cfg()
                raised = resolved = None
                for t, gpu in enumerate(readings):
                    now = 1_800_000_000_000 + t * 1000
                    for ev in e.step(agent.evaluate(frame(gpu=dict(gpu, procs=[])), default_rules(), c, tr, e.active, now), now):
                        if ev["alert"]["rule"] == "gpu-throttle":
                            if ev["kind"] == "raised":
                                raised = t
                            if ev["kind"] == "resolved":
                                resolved = t
                self.assertEqual((raised, resolved), (case["raisedAtS"], case["resolvedAtS"]))


class AcksFromTheMac(Dirs):
    """An alert acknowledged on the Mac reaches the agent that pages it (a signed
    requests/ack-*.json): no more "Still firing", until it gets worse."""

    def scenario(self, ack):
        agent.STATE = os.path.join(self.state, "acked" if ack else "not-acked")
        net, sent = Net(), []
        net.off.add("cell3")
        c = agent.Config({"node": "cell1", "name": "cell1", "index": 0, "peers": roster(), "repeatMinutes": 60})
        w = peers.PeerWatch("cell1", 0, roster(), 9469, fetch=net.fetch_for("cell1"), tcp=net.tcp_for("cell1"), gateway=net.gateway_for("cell1"), start=0.0)
        a = self.make(c, collect=lambda: frame(), outbox_senders={"ntfy": lambda ch, m: sent.append(m["title"])}, watch=w)
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        base = 1_800_000_000_000
        for t in list(range(0, 121, 15)) + list(range(180, 2 * 3600 + 1, 60)):
            a.watch.poll(float(t), base + t * 1000)
            a.cycle(float(t), base + t * 1000)
            if t == 120 and ack:
                write_request(agent.STATE, "ack-1.json", signed("ack", {"rule": "peer-unreachable", "entity": "node:cell3", "incident": "node:cell3",
                                                                       "untilEscalated": True, "ackedMs": base + 120_000, "by": "the Mac"}))
                a.handle_requests(now=base + 125_000)
        a.outbox.drain()
        return a, sent

    def test_a_mac_ack_stops_the_repeats_two_hours_on(self):
        a, sent = self.scenario(ack=False)
        self.assertEqual(sent[:2], ["cell3 isn't answering on any path", "Still firing: cell3 isn't answering on any path"],
                         "without the ack it repeats every hour")
        a, sent = self.scenario(ack=True)
        self.assertEqual(sent, ["cell3 isn't answering on any path"], "acknowledged on the Mac: the agent ticks for 2 h and sends no repeat")
        with open(os.path.join(self.state, "acked", "requests", "ack-1.result.json")) as f:
            self.assertEqual(json.load(f), {"ok": True, "error": None})
        self.assertEqual(a.status()["acks"], 1)

    def test_the_macs_signature_checks_here(self):
        # fixtures/signed-request.json: tests.rs checks agent/mod.rs signs it this way.
        with open(os.path.join(HERE, "fixtures", "signed-request.json")) as f:
            fx = json.load(f)
        self.assertEqual(agent.sign(fx["key"], fx["kind"], fx["body"]), fx["mac"])
        req = {"v": 1, "kind": fx["kind"], "body": fx["body"], "mac": fx["mac"]}
        body, why = agent.verify_request(req, "maintenance", fx["key"], json.loads(fx["body"])["startedMs"] + 1000, {})
        self.assertEqual((body["nodes"], why), (["cell3"], None))
        self.assertEqual(agent.verify_request(req, "ack", fx["key"], 0, {})[1], "it isn't signed", "an ack can't pass as a maintenance window or back")

    def test_an_unsigned_ack_is_refused(self):
        a = self.make(cfg(), collect=frame)
        write_request(self.state, "ack-2.json", {"rule": "peer-unreachable", "entity": "node:cell3", "ackedMs": agent.now_ms()})
        a.handle_requests()
        with open(os.path.join(self.state, "requests", "ack-2.result.json")) as f:
            self.assertEqual(json.load(f), {"ok": False, "error": "it isn't signed"})


class HistoryStore(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.h = agent.History(os.path.join(self.dir, "history.db"))

    def tearDown(self):
        self.h.db.close()
        shutil.rmtree(self.dir)

    def test_wal_and_tiers(self):
        self.assertEqual(self.h.db.execute("PRAGMA journal_mode").fetchone()[0], "wal")
        base = 1_800_000_000_000
        for i in range(120):  # two minutes of 2 s samples
            self.h.add(base + i * 2000, "node:cell1", {"cpu": float(i % 10), "gpu": None})
        self.h.flush()
        tiers = dict(self.h.db.execute("SELECT tier, COUNT(*) FROM points GROUP BY tier").fetchall())
        self.assertEqual(tiers, {0: 24, 1: 4, 2: 1}, "four minutes of 2 s samples: 24 raw 10 s buckets, 4 minutes, one 10-minute bucket")
        avg, lo, hi, n = self.h.db.execute("SELECT avg, min, max, n FROM points WHERE tier = 1 ORDER BY ts LIMIT 1").fetchone()
        self.assertEqual((lo, hi, n), (0.0, 9.0, 6))
        self.assertAlmostEqual(avg, 4.5)
        self.assertEqual(self.h.db.execute("SELECT COUNT(*) FROM points WHERE metric = 'gpu'").fetchone()[0], 0, "missing values stay gaps")

    def test_retention_by_tier(self):
        now = 1_800_000_000_000
        for days in (1, 3, 20, 200):
            self.h.add(now - days * 86_400_000, "node:cell1", {"cpu": float(days)})
            self.h.flush()
        self.h.prune(now)
        kept = sorted(self.h.db.execute("SELECT tier, avg FROM points").fetchall())
        self.assertEqual(kept, [(0, 1.0), (1, 1.0), (1, 3.0), (2, 1.0), (2, 3.0), (2, 20.0)])

    def test_export_takes_the_finest_tier_and_filters(self):
        now = 1_800_000_000_000
        for i in range(30):
            self.h.add(now - 600_000 + i * 2000, "node:cell1", {"cpu": 10.0, "gpu": 20.0})
            self.h.add(now - 600_000 + i * 2000, "container:cell1/vllm-head", {"cpu": 5.0})
        self.h.flush()
        rows = list(self.h.export(now - 3_600_000, now=now))
        self.assertTrue(all(r["s"] == 10 for r in rows), "raw 10 s buckets while raw still covers the range")
        cell = [r for r in rows if r["e"] == "node:cell1"]
        self.assertEqual(len(cell), 6)
        self.assertEqual(cell[0]["v"], {"cpu": 10.0, "gpu": 20.0})
        only = list(self.h.export(now - 3_600_000, entity="container:cell1/vllm-head", now=now))
        self.assertEqual({r["e"] for r in only}, {"container:cell1/vllm-head"})
        # Older than 48 h: the minute tier answers.
        self.h.add(now - 3 * 86_400_000, "node:cell1", {"cpu": 1.0})
        self.h.flush()
        old = list(self.h.export(now - 4 * 86_400_000, now - 2 * 86_400_000, now=now))
        self.assertEqual([r["s"] for r in old], [60])

    def test_the_cap_drops_the_oldest_raw_data_first(self):
        now = 1_800_000_000_000
        for i in range(4000):
            self.h.add(now - (4000 - i) * 10_000, "node:cell1", {m: float(i) for m in agent.NODE_METRICS})
        self.h.flush()
        before = self.h.size()
        coarse = self.h.db.execute("SELECT COUNT(*) FROM points WHERE tier = 2").fetchone()[0]
        self.h.cap = before // 2
        self.assertTrue(self.h.enforce_cap())
        self.assertLessEqual(self.h.size(), before // 2 + 64 * 1024)
        lo = self.h.db.execute("SELECT MIN(ts) FROM points WHERE tier = 0").fetchone()[0]
        self.assertGreater(lo, now - 4000 * 10_000, "the oldest raw buckets went")
        self.assertEqual(self.h.db.execute("SELECT COUNT(*) FROM points WHERE tier = 2").fetchone()[0], coarse, "the 10-minute tier is untouched")


def parse_prometheus(text):
    """Text format 0.0.4 back to {(name, labels): value}, checking HELP/TYPE come first."""
    out, typed = {}, set()
    for line in text.splitlines():
        if line.startswith("# TYPE "):
            typed.add(line.split()[2])
            continue
        if not line or line.startswith("#"):
            continue
        name_labels, value = line.rsplit(" ", 1)
        name, _, labels = name_labels.partition("{")
        assert name in typed, name
        pairs = tuple(sorted(tuple(p.split("=", 1)) for p in labels.rstrip("}").split(",") if p))
        out[(name, tuple((k, v.strip('"')) for k, v in pairs))] = float(value)
    return out


def get(url, headers=None):
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return r.status, r.headers.get("Content-Type"), r.read().decode()
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.headers.get("Content-Type"), e.read().decode()


class Listening(Dirs):
    def start(self, metrics_enabled=False, token=None, metrics_bind="127.0.0.1"):
        port = free_port()
        c = cfg("cell1", 0, port=port, bind="127.0.0.1", metrics={"enabled": metrics_enabled, "port": port, "bind": metrics_bind})
        fr = frame()
        fr["nets"][0]["roce"] = {"ecnMarked": 12, "cnpSent": None, "outOfSequence": 3}
        a = self.make(c, collect=lambda: fr)
        a.metrics_token = token
        a.cycle()
        a.listeners.sync(a.frame)
        self.addCleanup(a.listeners.close)
        return a, "http://127.0.0.1:%d" % port

    def test_healthz_says_only_the_c_agent_fields(self):
        a, base = self.start()
        status, ctype, body = get(base + "/healthz")
        v = json.loads(body)
        self.assertEqual((status, ctype), (200, "application/json"))
        self.assertEqual(set(v), HEALTHZ, "the C-agent shape, nothing else")
        self.assertEqual((v["node"], v["version"]), ("cell1", agent.VERSION))
        self.assertEqual(v["configMac"], hmac.new(KEY.encode(), a.hash.encode(), hashlib.sha256).hexdigest(),
                         "the settings hash only as an HMAC keyed by the request key: nothing to test secret guesses against")
        self.assertNotIn(a.hash, body)
        self.assertEqual(a.status()["configHash"], a.hash, "status over SSH keeps the hash itself")
        self.assertEqual(v["addresses"], {"lan": "192.168.50.10", "fabric": ["192.168.100.11"]})
        self.assertNotIn("Phone", body)
        self.assertEqual(get(base + "/metrics")[0], 404, "metrics are off by default")

    def test_a_stale_cycle_answers_503_wedged(self):
        a, base = self.start()
        a.last_cycle_ms = agent.now_ms() - agent.WEDGED_MS - 1000
        status, _, body = get(base + "/healthz")
        self.assertEqual((status, json.loads(body)["wedged"]), (503, True))
        a.cycle()
        self.assertEqual(get(base + "/healthz")[0], 200, "a completed cycle and it's fine again")

    def test_metrics_parse_back_and_need_the_token(self):
        a, base = self.start(metrics_enabled=True, token="t0ken")
        self.assertEqual(get(base + "/metrics")[0], 401)
        status, ctype, body = get(base + "/metrics", {"Authorization": "Bearer t0ken"})
        self.assertEqual((status, ctype), (200, "text/plain; version=0.0.4; charset=utf-8"))
        m = parse_prometheus(body)
        self.assertEqual(m[("sparkbox_mem_total_bytes", (("node", "cell1"),))], 128 * GIB)
        self.assertEqual(m[("sparkbox_disk_used_ratio", (("mount", "/"), ("node", "cell1")))], 0.25)
        self.assertEqual(m[("sparkbox_net_bits_per_second", (("dir", "rx"), ("iface", "enp1s0f1np1"), ("node", "cell1")))], 9e6, "IP plus RDMA")
        self.assertEqual(m[("sparkbox_roce_counter_total", (("counter", "ecnMarked"), ("iface", "enp1s0f1np1"), ("node", "cell1")))], 12)
        self.assertNotIn(("sparkbox_roce_counter_total", (("counter", "cnpSent"), ("iface", "enp1s0f1np1"), ("node", "cell1"))), m, "a missing counter isn't 0")
        self.assertEqual(m[("sparkbox_alerts_firing", (("node", "cell1"), ("severity", "crit")))], 0)
        self.assertEqual(m[("sparkbox_agent_up", (("node", "cell1"),))], 1)
        for name in ("sparkbox_cpu_percent", "sparkbox_gpu_util_percent", "sparkbox_gpu_temp_celsius", "sparkbox_gpu_power_watts", "sparkbox_mem_used_bytes"):
            self.assertIn((name, (("node", "cell1"),)), m)
        # The models it serves, as gauges.
        a.endpoints.latest["llama"] = {"ms": agent.now_ms(), "endpoint": {"id": "llama"},
                                       "result": {"ok": True, "genTps": 31.5, "running": 2, "waiting": 0, "kvPct": 17.0, "ttftP95": 420.0, "canaryMs": 84}}
        m = parse_prometheus(get(base + "/metrics", {"Authorization": "Bearer t0ken"})[2])
        ep = lambda n: m[(n, (("endpoint", "llama"), ("node", "cell1")))]
        self.assertEqual((ep("sparkbox_endpoint_up"), ep("sparkbox_endpoint_generation_tokens_per_second"), ep("sparkbox_endpoint_kv_cache_ratio"),
                          ep("sparkbox_endpoint_ttft_p95_seconds")), (1, 31.5, 0.17, 0.42))
        self.assertEqual(m[("sparkbox_agent_leader", (("node", "cell1"),))], 1)

    def test_metrics_beyond_loopback_need_a_token(self):
        a, base = self.start(metrics_enabled=True)
        self.assertEqual(get(base + "/metrics")[0], 200, "on loopback alone no token is needed")
        srv = next(iter(a.listeners.servers.values()))
        srv.server_address = ("192.168.50.10", srv.server_address[1])  # as if bound on the LAN
        status, _, body = get(base + "/metrics")
        self.assertEqual(status, 403)
        self.assertIn("need a token", body)

    def test_each_client_gets_20_requests_a_second(self):
        a, base = self.start()
        codes = [get(base + "/healthz")[0] for _ in range(30)]
        self.assertEqual(codes.count(429), 30 - codes.count(200))
        self.assertGreaterEqual(codes.count(429), 5)
        self.assertLessEqual(codes.count(200), 25)
        limit = agent.RateLimit(rate=20, clock=lambda: 100.0)
        self.assertEqual(sum(limit.allow("10.0.0.1") for _ in range(25)), 20)
        self.assertTrue(limit.allow("10.0.0.2"), "per client")

    def test_never_0_0_0_0_and_rebinds_when_addresses_change(self):
        made, closed = [], []

        class Fake:
            def __init__(self, addr):
                self.server_address, self.roles = addr, None
                made.append(addr)

            def serve_forever(self):
                pass

            def shutdown(self):
                closed.append(self.server_address)

            def server_close(self):
                pass
        a = self.make(cfg("cell1", 0, port=9469), collect=frame)
        lst = agent.Listeners(a, factory=Fake)
        self.assertTrue(lst.sync({}))
        self.assertEqual(made, [("127.0.0.1", 9469)], "no address known yet: loopback only, never 0.0.0.0")
        fr = frame()
        self.assertTrue(lst.sync(fr))
        self.assertEqual(sorted(lst.servers), [("127.0.0.1", 9469), ("192.168.100.11", 9469), ("192.168.50.10", 9469)])
        self.assertFalse(lst.sync(fr), "nothing changed")
        fr["nets"][0]["ip"] = "192.168.100.21"  # the fabric address changed
        self.assertTrue(lst.sync(fr))
        self.assertEqual(closed, [("192.168.100.11", 9469)])
        self.assertIn(("192.168.100.21", 9469), lst.servers)
        fr["nets"][1]["ip"] = "0.0.0.0"
        lst.sync(fr)
        self.assertNotIn(("0.0.0.0", 9469), lst.servers)


class Watchdog(Dirs):
    def listen(self):
        d = tempfile.mkdtemp(dir="/tmp" if os.path.isdir("/tmp") else None)
        self.addCleanup(shutil.rmtree, d)
        path = os.path.join(d, "notify")
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.bind(path)
        sock.settimeout(2)
        self.addCleanup(sock.close)
        return path, sock

    def test_sd_notify_sends_one_datagram_to_notify_socket(self):
        path, sock = self.listen()
        self.assertTrue(agent.sd_notify("READY=1", {"NOTIFY_SOCKET": path}))
        self.assertEqual(sock.recv(256), b"READY=1")
        self.assertFalse(agent.sd_notify("READY=1", {}), "not under systemd: nothing to do")
        self.assertEqual(agent.notify_address("@/org/freedesktop/systemd1/notify"), "\0/org/freedesktop/systemd1/notify")
        self.assertEqual(agent.notify_address("/run/systemd/notify"), "/run/systemd/notify")

    def test_run_keeps_notify_socket_from_the_children_it_starts(self):
        import subprocess
        path, sock = self.listen()
        os.environ["NOTIFY_SOCKET"] = path
        self.addCleanup(os.environ.pop, "NOTIFY_SOCKET", None)
        self.addCleanup(setattr, agent, "NOTIFY_SOCKET", None)
        agent.keep_notify_socket()
        self.assertNotIn("NOTIFY_SOCKET", os.environ)
        child = subprocess.run([sys.executable, "-c", "import os; print(os.environ.get('NOTIFY_SOCKET', ''))"], capture_output=True, text=True)
        self.assertEqual(child.stdout.strip(), "", "a child systemd tool gets no NOTIFY_SOCKET")
        self.assertTrue(agent.sd_notify("READY=1"))
        self.assertEqual(sock.recv(256), b"READY=1", "the agent itself still tells systemd")

    @unittest.skipUnless(sys.platform.startswith("linux"), "abstract sockets are Linux only")
    def test_sd_notify_to_an_abstract_socket(self):
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.bind("\0sparkbox-test-%d" % os.getpid())
        self.addCleanup(sock.close)
        self.assertTrue(agent.sd_notify("WATCHDOG=1", {"NOTIFY_SOCKET": "@sparkbox-test-%d" % os.getpid()}))
        self.assertEqual(sock.recv(64), b"WATCHDOG=1")

    def test_every_completed_cycle_pings_the_watchdog(self):
        path, sock = self.listen()
        os.environ["NOTIFY_SOCKET"] = path
        self.addCleanup(os.environ.pop, "NOTIFY_SOCKET", None)
        a = self.make(cfg(), collect=frame)
        a.cycle()
        self.assertEqual(sock.recv(64), b"WATCHDOG=1")
        a.collect = lambda: None
        self.assertIsNone(a.cycle(), "no frame: not a completed cycle")
        with self.assertRaises(socket.timeout):
            sock.settimeout(0.2)
            sock.recv(64)


class SlowSenderDoesNotStall(Dirs):
    """A channel that takes 30 s (a dead SMTP server) holds up its own worker, never the watch."""

    def test_samples_beat_and_last_cycle_keep_advancing(self):
        release = threading.Event()
        started = threading.Event()
        self.addCleanup(release.set)

        def slow(ch, msg):
            started.set()
            release.wait(30)
        wait, gate.GROUP_WAIT_MS = gate.GROUP_WAIT_MS, 0
        self.addCleanup(setattr, gate, "GROUP_WAIT_MS", wait)
        rules = default_rules()
        next(r for r in rules if r["id"] == "mem-high")["critForS"] = 0
        with open(os.path.join(self.config, "rules.json"), "w") as f:
            json.dump({"version": 1, "rules": rules}, f)
        with open(os.path.join(self.config, "notify.json"), "w") as f:
            json.dump({"channels": [{"id": "mail", "kind": "email", "name": "Email", "enabled": True}]}, f)
        a = self.make(cfg(), collect=lambda: frame(mem_pct=97), outbox_senders={"email": slow})
        a.outbox.start()
        beats, cycles, durations = [], [], []
        for _ in range(12):
            t0 = time.monotonic()
            a.cycle()
            durations.append(time.monotonic() - t0)
            cycles.append(a.last_cycle_ms)
            with open(os.path.join(self.state, "beat.json")) as f:
                beats.append(json.load(f)["lastBeatMs"])
            time.sleep(0.05)
        self.assertTrue(started.wait(2), "the page went to the slow channel")
        self.assertLess(max(durations), 0.5, "no cycle waited on the channel")
        self.assertEqual(cycles, sorted(cycles))
        self.assertGreater(cycles[-1], cycles[0])
        self.assertGreater(beats[-1], beats[0], "beat.json keeps advancing")
        self.assertEqual(a.health()["outboxPending"], 1, "the page waits in the outbox while the channel is stuck")
        release.set()


class NothingDropped(Dirs):
    def test_a_message_the_outbox_cant_take_waits_for_the_next_cycle(self):
        sent = []
        a = self.make(cfg(), collect=lambda: frame(mem_pct=97), outbox_senders={"ntfy": lambda ch, m: sent.append(m["title"])})
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        next(r for r in a.rules if r["id"] == "mem-high")["critForS"] = 0
        real = a.outbox.enqueue

        def broken(msg, *args, **kw):
            raise sqlite3.OperationalError("disk I/O error")
        a.outbox.enqueue = broken
        now = agent.now_ms()
        a.cycle(now=now)
        a.cycle(now=now + gate.GROUP_WAIT_MS)
        self.assertEqual([m["title"] for m in a.unqueued], ["cell1 memory 97%"])
        with open(os.path.join(self.state, "state.json")) as f:
            self.assertEqual(len(json.load(f)["unqueued"]), 1, "kept across a restart too")
        a.outbox.enqueue = real
        a.cycle(now=now + gate.GROUP_WAIT_MS + 2000)
        a.outbox.drain()
        self.assertEqual((sent, a.unqueued), (["cell1 memory 97%"], []))


class TheSwitch(Dirs):
    def test_the_leader_pages_a_switch_that_stops_answering(self):
        sent = []
        a = self.make(cfg(switchMgmt={"id": "crs804", "name": "CRS804 DDQ", "host": "192.168.88.1"}), collect=frame,
                        outbox_senders={"ntfy": lambda ch, m: sent.append((m["title"], m["severity"]))})
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        mono = time.monotonic()
        a.cycle(mono)
        self.assertTrue(a.leading, "alone in its cluster, it leads")
        a.switch_ok_mono = mono - peers.MISSING_S - 1
        now = agent.now_ms()
        for i in range(8):
            a.cycle(mono + i * 2, now + i * 2000)
        a.outbox.drain()
        self.assertEqual(sent, [("CRS804 DDQ isn't answering", "crit")])
        self.assertTrue(a.status()["switchWatched"])
        self.assertEqual(a.engine.firing()[0]["incident"], "switch:crs804")


class PersistedOutbox(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)

    def box(self, senders, clock=time.time, channels=None):
        chans = channels or [{"id": "phone", "name": "Phone", "kind": "ntfy", "enabled": True}]
        box = agent.Outbox(os.path.join(self.dir, "outbox.db"), os.path.join(self.dir, "deliveries.jsonl"), "cell1", chans, senders, clock=clock)
        self.addCleanup(box.close)
        return box

    def test_a_pending_message_survives_a_restart(self):
        def down(ch, msg):
            raise agent.Failure("Couldn't reach ntfy.sh: no route to host", True)
        clock = [1000.0]
        box = self.box({"ntfy": down}, clock=lambda: clock[0])
        box.enqueue(dict(MSG, messageId="m-1"))
        box.drain()
        self.assertEqual(box.pending(), 1)
        box.close()
        sent = []
        clock[0] += 31
        again = self.box({"ntfy": lambda ch, m: sent.append(m["title"])}, clock=lambda: clock[0])
        self.assertEqual(again.pending(), 1, "still queued after a restart")
        again.drain()
        self.assertEqual((sent, again.pending()), ([MSG["title"]], 0))
        with open(os.path.join(self.dir, "deliveries.jsonl")) as f:
            lines = [json.loads(l) for l in f]
        self.assertEqual([(l["outcome"], l["attempts"], l["messageId"]) for l in lines], [("failed", 1, "m-1"), ("sent", 2, "m-1")])

    def retries(self, severity, until_s):
        clock, calls = [1000.0], []

        def flaky(ch, msg):
            calls.append(clock[0] - 1000)
            raise agent.Failure("HTTP 503 — the server had a problem; it will retry", True)
        box = self.box({"ntfy": flaky}, clock=lambda: clock[0])
        box.enqueue(dict(MSG, severity=severity))
        for step in range(0, until_s, 5):
            clock[0] = 1000.0 + step
            box.drain()
        return calls, box

    def test_retries_after_30_s_2_min_and_10_min_then_every_30_min_for_a_critical(self):
        calls, box = self.retries("warn", 4000)
        self.assertEqual(calls, [0, 30, 150, 750], "a warning: 30 s, 2 min, 10 min, then it stops")
        self.assertEqual(box.pending(), 0)
        calls, box = self.retries("crit", 2 * 86400)
        self.assertEqual(calls[:6], [0, 30, 150, 750, 2550, 4350], "a critical: then every 30 min")
        self.assertLessEqual(calls[-1], 86400, "for a day")
        self.assertGreater(calls[-1], 86400 - 1800)

    def test_a_config_error_isnt_retried_and_critical_only_channels_skip_warnings(self):
        calls = []

        def bad(ch, msg):
            calls.append(msg["severity"])
            raise agent.Failure("HTTP 401 — check the token", False)
        box = self.box({"ntfy": bad}, channels=[{"id": "p", "name": "P", "kind": "ntfy", "enabled": True, "criticalOnly": True},
                                                {"id": "beat", "name": "Dead-man's switch", "kind": "heartbeat", "enabled": True, "url": "https://hc"}])
        box.enqueue(dict(MSG, severity="warn"))
        box.enqueue(dict(MSG))
        box.drain()
        self.assertEqual(calls, ["crit"], "and the heartbeat channel isn't a place to page")
        self.assertEqual(box.pending(), 0)

    def test_each_channel_has_its_own_worker(self):
        release = threading.Event()
        self.addCleanup(release.set)
        sent = []
        box = self.box({"email": lambda ch, m: release.wait(10), "ntfy": lambda ch, m: sent.append(m["title"])},
                       channels=[{"id": "mail", "name": "Email", "kind": "email", "enabled": True},
                                 {"id": "phone", "name": "Phone", "kind": "ntfy", "enabled": True}])
        self.addCleanup(box.stop)
        box.start()
        box.enqueue(dict(MSG))
        for _ in range(50):
            if sent:
                break
            time.sleep(0.02)
        self.assertEqual(sent, [MSG["title"]], "the phone isn't held up by a stuck mail server")
        release.set()


class DeadMansSwitch(unittest.TestCase):
    def test_the_leader_pings_each_heartbeat_url(self):
        srv, h, url = http_server(status=200)
        self.addCleanup(srv.shutdown)
        leading = [False]
        hb = agent.Heartbeat([{"id": "hc", "kind": "heartbeat", "name": "healthchecks.io", "enabled": True, "url": url + "/ping/abc"},
                              {"id": "phone", "kind": "ntfy"}], lambda: leading[0])
        self.assertFalse(hb.beat(), "not the leader: it doesn't ping")
        leading[0] = True
        self.assertTrue(hb.beat())
        self.assertEqual([(m, p) for m, p, _, _ in h.seen], [("GET", "/ping/abc")])
        self.assertIsNotNone(hb.ok_ms)
        started = agent.Heartbeat([{"id": "hc", "kind": "heartbeat", "url": url + "/ping/new"}], lambda: True)
        started.start()
        for _ in range(100):
            if started.ok_ms:
                break
            time.sleep(0.02)
        self.assertIsNotNone(started.ok_ms, "a new leader pings at once, not a minute later")
        kept = agent.Heartbeat([{"id": "hc", "kind": "heartbeat", "secrets": {"url": url + "/ping/kept"}}], lambda: True)
        self.assertTrue(kept.beat())
        self.assertEqual(h.seen[-1][1], "/ping/kept", "the URL the Mac keeps in the Keychain comes with the secrets")
        bad = agent.Heartbeat([{"id": "hc", "kind": "heartbeat", "url": "http://127.0.0.1:9/ping"}], lambda: True)
        bad.beat()
        self.assertTrue(bad.error.startswith("Couldn't reach 127.0.0.1"), bad.error)
        self.assertIsNone(bad.ok_ms)


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class Recorder(http.server.BaseHTTPRequestHandler):
    """Records each request; answers with the class's status and body."""
    status, reply, seen = 200, b'{"status":1}', []

    def log_message(self, *a):
        pass

    def do_GET(self):
        type(self).seen.append((self.command, self.path, {k.lower(): v for k, v in self.headers.items()}, b""))
        self.send_response(type(self).status)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"OK")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        type(self).seen.append((self.command, self.path, {k.lower(): v for k, v in self.headers.items()}, body))
        self.send_response(type(self).status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(type(self).reply)))
        self.end_headers()
        self.wfile.write(type(self).reply)


def http_server(status=200, reply=b'{"id":"x"}'):
    handler = type("H", (Recorder,), {"status": status, "reply": reply, "seen": []})
    srv = agent.Server(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    stop = srv.shutdown
    srv.shutdown = lambda: (stop(), srv.server_close())  # tests clean up with srv.shutdown
    return srv, handler, "http://127.0.0.1:%d" % srv.server_address[1]


MSG = {"title": "gx10-0000 GPU Xid 79", "body": "GPU on gx10-0000 — Xid 79 · GPU fallen off the bus", "severity": "crit", "state": "firing",
       "node": "gx10-0000", "rule": "kernel-xid", "alertId": "kernel-xid:gpu:gx10-0000:xid-79", "tsMs": 1_800_000_000_000}


class Delivery(unittest.TestCase):
    def test_ntfy(self):
        srv, h, url = http_server()
        self.addCleanup(srv.shutdown)
        agent.send_ntfy({"server": url, "topic": "sparkbox-alerts", "secrets": {"token": "tk_1"}}, MSG)
        method, path, headers, body = h.seen[0]
        self.assertEqual((method, path, headers["authorization"], headers["content-type"]), ("POST", "/", "Bearer tk_1", "application/json"))
        b = json.loads(body)
        self.assertEqual((b["topic"], b["title"], b["priority"], b["tags"]), ("sparkbox-alerts", MSG["title"], 5, ["rotating_light"]))
        self.assertEqual(agent.ntfy_priority(dict(MSG, severity="warn")), 3)

    def test_pushover(self):
        srv, h, url = http_server(reply=b'{"status":1,"request":"r"}')
        self.addCleanup(srv.shutdown)
        agent.send_pushover({"api": url + "/1/messages.json", "device": "iphone", "secrets": {"appToken": "app", "userKey": "usr"}}, MSG)
        method, path, headers, body = h.seen[0]
        form = dict(urllib.parse.parse_qsl(body.decode()))
        self.assertEqual((method, path, headers["content-type"]), ("POST", "/1/messages.json", "application/x-www-form-urlencoded"))
        self.assertEqual((form["token"], form["user"], form["priority"], form["device"], form["title"]), ("app", "usr", "1", "iphone", MSG["title"]))

    def test_webhook_signature_and_slack(self):
        srv, h, url = http_server(status=204, reply=b"")
        self.addCleanup(srv.shutdown)
        agent.send_webhook({"url": url + "/hook", "secrets": {"secret": "s3cret"}}, MSG)
        method, path, headers, body = h.seen[0]
        expected = "sha256=" + agent.hmac.new(b"s3cret", body, agent.hashlib.sha256).hexdigest()
        self.assertEqual((method, path, headers["x-sparkbox-signature"]), ("POST", "/hook", expected))
        self.assertEqual(json.loads(body)["state"], "firing")
        agent.send_webhook({"url": url + "/hook", "slack": True}, MSG)
        self.assertEqual(json.loads(h.seen[1][3]), {"text": "*%s*\n%s" % (MSG["title"], MSG["body"])})
        self.assertNotIn("x-sparkbox-signature", h.seen[1][2])
        # Since round 4 the Mac sends the URL with the secrets (it's kept in the Keychain).
        agent.send_webhook({"urlHost": "127.0.0.1", "secrets": {"url": url + "/kept", "secret": "s3cret"}}, MSG)
        self.assertEqual(h.seen[2][1], "/kept")

    def test_http_errors_say_what_to_check(self):
        srv, h, url = http_server(status=401, reply=b'{"code":40101,"http":401,"error":"unauthorized"}')
        self.addCleanup(srv.shutdown)
        with self.assertRaises(agent.Failure) as e:
            agent.send_ntfy({"server": url, "topic": "t"}, MSG)
        self.assertEqual((e.exception.message, e.exception.retry), ("HTTP 401 — check the token (unauthorized)", False))
        with self.assertRaises(agent.Failure) as e:
            agent.send_ntfy({"server": "http://127.0.0.1:9", "topic": "t"}, MSG)
        self.assertTrue(e.exception.retry)
        self.assertTrue(e.exception.message.startswith("Couldn't reach 127.0.0.1"), e.exception.message)

    def test_email_through_a_local_smtp_server(self):
        port, transcript = smtp_server()
        agent.send_email({"host": "127.0.0.1", "port": port, "security": "none", "user": "alerts@example.com", "from": "alerts@example.com",
                          "to": "owner@example.com", "secrets": {"password": "pw"}}, MSG)
        for _ in range(50):
            if "QUIT" in "".join(transcript):
                break
            time.sleep(0.02)
        t = "".join(transcript)
        self.assertIn("AUTH PLAIN", t)
        self.assertIn("mail from:<alerts@example.com>", t.lower())
        self.assertIn("rcpt to:<owner@example.com>", t.lower())
        self.assertIn("Subject: [SparkBox] gx10-0000 GPU Xid 79", t)
        with self.assertRaises(agent.Failure) as e:
            agent.send_email({"host": "smtp.example.com", "security": "none", "from": "a@example.com", "to": "b@example.com"}, MSG)
        self.assertIn("won't send a password unencrypted", e.exception.message)


def smtp_server():
    """A minimal SMTP server on 127.0.0.1 for one conversation; returns (port, transcript)."""
    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    transcript = []

    def serve():
        conn, _ = srv.accept()
        f = conn.makefile("rwb")
        f.write(b"220 localhost ESMTP test\r\n")
        f.flush()
        data = False
        for raw in f:
            line = raw.decode()
            transcript.append(line)
            if data:
                if line == ".\r\n":
                    data = False
                    f.write(b"250 queued\r\n")
            elif line.upper().startswith("EHLO"):
                f.write(b"250-localhost\r\n250 AUTH PLAIN LOGIN\r\n")
            elif line.upper().startswith("AUTH"):
                f.write(b"235 ok\r\n")
            elif line.upper().startswith("DATA"):
                data = True
                f.write(b"354 go ahead\r\n")
            elif line.upper().startswith("QUIT"):
                f.write(b"221 bye\r\n")
                f.flush()
                break
            else:
                f.write(b"250 ok\r\n")
            f.flush()
        conn.close()
        srv.close()

    threading.Thread(target=serve, daemon=True).start()
    return srv.getsockname()[1], transcript


def write_request(state, name, body, raw=None):
    folder = os.path.join(state, "requests")
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, name)
    with open(path, "w") as f:
        f.write(raw if raw is not None else json.dumps(body))
    return path


class PlannedWork(Dirs):
    """Maintenance windows SparkBox drops in requests/, and silences from agent.json."""

    def leader(self, net, sent, extra=None):
        c = agent.Config(dict({"node": "cell1", "name": "cell1", "index": 0, "peers": roster(),
                               "quiet": {"start": "22:00", "end": "07:00", "tz": "UTC"}}, **(extra or {})))
        w = peers.PeerWatch("cell1", 0, roster(), 9469, fetch=net.fetch_for("cell1"), tcp=net.tcp_for("cell1"), gateway=net.gateway_for("cell1"), start=0.0)
        a = self.make(c, collect=lambda: frame(), outbox_senders={"ntfy": lambda ch, m: sent.append((m["title"], m["severity"]))}, watch=w)
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        return a

    def run_for(self, a, net, seconds, base, start=0):
        for t in range(start, start + seconds + 1, 15):
            a.watch.poll(float(t), base + t * 1000)
            a.cycle(float(t), base + t * 1000)
        a.outbox.drain()

    def answers(self):
        """{request name: its answer} for the result files waiting in requests/."""
        folder = os.path.join(self.state, "requests")
        out = {}
        for name in sorted(os.listdir(folder)):
            if name.endswith(".result.json"):
                with open(os.path.join(folder, name)) as f:
                    out[name[:-len(".result.json")]] = json.load(f)
        return out

    def test_requests_are_checked_kept_and_deleted(self):
        a = self.make(cfg(peers=roster()), collect=frame)
        now = 1_800_000_000_500
        window = {"nodes": ["cell3"], "untilMs": 1_800_000_600_000, "reason": "restarting", "by": "SparkBox", "startedMs": 1_800_000_000_000,
                  "op": "op-7"}
        good = write_request(self.state, "maintenance-cell3-1.json", signed("maintenance", window))
        big = write_request(self.state, "maintenance-cell2-2.json", None, raw=json.dumps({"node": "cell2", "reason": "x" * 5000}))
        shape = write_request(self.state, "maintenance-cell2-3.json", signed("maintenance", {"node": "cell2; rm -rf /", "untilMs": "soon", "startedMs": now}))
        # What a process running as the login user could write: no key, so no window.
        forged = write_request(self.state, "maintenance-cell2-4.json", {"node": "cell2", "untilMs": None, "reason": "x", "by": "y", "startedMs": now})
        guessed = write_request(self.state, "maintenance-cell2-5.json", signed("maintenance", dict(window, nodes=["cell2"]), key="00" * 32))
        stale = write_request(self.state, "maintenance-gx10-0000-6.json", signed("maintenance", dict(window, nodes=["gx10-0000"], startedMs=now - 25 * 3600_000)))
        ahead = write_request(self.state, "maintenance-gx10-0000-7.json", signed("maintenance", dict(window, nodes=["gx10-0000"], startedMs=now + 120_000)))
        target = os.path.join(self.state, "elsewhere.json")
        with open(target, "w") as f:
            json.dump(signed("maintenance", dict(window, nodes=["gx10-0000"])), f)
        link = os.path.join(self.state, "requests", "maintenance-link-8.json")
        os.symlink(target, link)
        other = write_request(self.state, "notes.txt", None, raw="not mine")
        a.handle_requests(now=now)
        self.assertEqual(sorted(a.maint), ["cell3"], "only the valid, signed window")
        self.assertEqual({k: a.maint["cell3"][k] for k in ("untilMs", "reason", "by", "startedMs", "op")},
                         {"untilMs": 1_800_000_600_000, "reason": "restarting", "by": "SparkBox", "startedMs": 1_800_000_000_000, "op": "op-7"})
        for p in (good, big, shape, forged, guessed, stale, ahead, link):
            self.assertFalse(os.path.lexists(p), "read once, then deleted: %s" % os.path.basename(p))
        self.assertEqual({k: v["error"] for k, v in self.answers().items()}, {
            "maintenance-cell3-1": None,
            "maintenance-cell2-2": "it isn't a small JSON file",
            "maintenance-cell2-3": "it doesn't name a node",
            "maintenance-cell2-4": "it isn't signed",
            "maintenance-cell2-5": "its signature doesn't match",
            "maintenance-gx10-0000-6": "it's more than 24 h old",
            "maintenance-gx10-0000-7": "it starts in the future",
            "maintenance-link-8": "it isn't a small JSON file",
        }, "each answered, so SparkBox knows whether it took")
        self.assertTrue(os.path.exists(target), "a link is removed, never followed")
        self.assertTrue(os.path.exists(other), "files that aren't requests are left alone")
        # The same signed file again (a copy someone kept) is refused.
        write_request(self.state, "maintenance-cell3-9.json", signed("maintenance", window))
        a.maint.clear()
        a.handle_requests(now=now)
        self.assertEqual((a.maint, self.answers()["maintenance-cell3-9"]["error"]), ({}, "it was used before"))
        write_request(self.state, "maintenance-cell3-10.json", signed("maintenance", dict(window, startedMs=1_800_000_000_100)))
        a.handle_requests(now=now)
        a.cycle(0.0, 1_800_000_001_000)
        with open(os.path.join(self.state, "state.json")) as f:
            saved = json.load(f)
        self.assertIn("cell3", saved["maintenance"], "kept in state.json across restarts")
        self.assertEqual(len(saved["requestsSeen"]), 3, "and every signed request it took, so a restart doesn't take one again")
        again = self.make(cfg(peers=roster()), collect=frame)
        self.assertIn("cell3", again.maint)
        write_request(self.state, "maintenance-cell3-11.json", signed("maintenance", {"nodes": ["cell3"], "end": True, "startedMs": now + 1000}))
        again.handle_requests(now=now + 2000)
        self.assertEqual(again.maint, {}, "an end record closes the window")

    def test_windows_are_bounded(self):
        net, sent = Net(), []
        a = self.leader(net, sent)
        base = 1_800_000_000_000
        # A shutdown: open-ended once, now four hours at most.
        write_request(self.state, "maintenance-cell3-1.json", signed("maintenance", {"nodes": ["cell3"], "untilMs": None, "reason": "shut down",
                                                                                     "by": "SparkBox", "startedMs": base}))
        # A restart that was refused: cell2 never goes down (it was given 40 minutes to come back).
        write_request(self.state, "maintenance-cell2-2.json", signed("maintenance", {"nodes": ["cell2"], "untilMs": base + 40 * 60_000, "reason": "restarting",
                                                                                     "by": "SparkBox", "startedMs": base}))
        a.handle_requests(now=base)
        self.assertEqual((a.maint["cell3"]["untilMs"], a.maint["cell3"]["shutdown"]), (base + agent.MAINT_MAX_MS, True))
        net.off.add("cell3")
        self.run_for(a, net, 19 * 60, base)
        self.assertEqual(sorted(a.maint), ["cell2", "cell3"])
        self.run_for(a, net, 120, base, start=19 * 60 + 15)
        self.assertEqual(sorted(a.maint), ["cell3"], "cell2 never went down in 20 min: its window closed")
        self.assertEqual(sent, [], "the shutdown is planned; no node-reboot for a node that was meant to stay off")
        self.run_for(a, net, 30, base, start=4 * 3600 - 15)
        self.assertEqual(a.maint, {}, "four hours on, the shutdown's window is over too")
        self.run_for(a, net, 30, base, start=4 * 3600 + 30)
        self.assertEqual(sent, [("cell3 isn't answering on any path", "crit")], "and cell3 pages like any node that's gone")

    def test_a_planned_restart_never_pages_and_ends_when_the_node_is_back(self):
        net, sent = Net(), []
        a = self.leader(net, sent)
        base = 1_800_000_000_000
        write_request(self.state, "maintenance-cell3-1.json", signed("maintenance", {"node": "cell3", "untilMs": base + 600_000, "reason": "restarting",
                                                                                     "by": "SparkBox", "startedMs": base}))
        a.handle_requests(now=base)
        net.off.add("cell3")
        self.run_for(a, net, 300, base)
        self.assertEqual(a.engine.firing()[0]["paging"], "planned")
        net.off.discard("cell3")
        self.run_for(a, net, 120, base, start=315)
        self.assertEqual(sent, [], "neither the outage nor its end pages")
        self.assertEqual(a.maint, {}, "back, so the window closed")
        with open(os.path.join(self.state, "deliveries.jsonl")) as f:
            outcomes = [(l["rule"], l["outcome"]) for l in map(json.loads, f) if l.get("rule")]
        self.assertEqual(outcomes, [("peer-unreachable", "planned"), ("peer-unreachable", "not-paged")])

    def test_a_restart_that_doesnt_come_back_is_critical(self):
        net, sent = Net(), []
        a = self.leader(net, sent)
        base = 1_800_000_000_000 - (1_800_000_000_000 % 86_400_000) + (10 * 60 + 42) * 60_000  # 10:42 UTC
        write_request(self.state, "maintenance-cell3-1.json", signed("maintenance", {"node": "cell3", "untilMs": base + 300_000, "reason": "restarting",
                                                                                     "by": "SparkBox", "startedMs": base}))
        a.handle_requests(now=base)
        net.off.add("cell3")
        self.run_for(a, net, 300 + 600 + 30, base)
        self.assertEqual(sent, [("cell3 didn't come back after the restart SparkBox started at 10:42", "crit")])

    def test_silences_from_agent_json_are_honoured_until_they_end(self):
        sent = []
        silences = [{"id": "s-1", "rule": "disk-full", "entity": "disk:cell1:/", "untilMs": None, "reason": "expanding the disk on Friday"},
                    {"id": "s-2", "matcher": {"rule": "mem-high"}, "until": {"kind": "at", "ms": 1}, "reason": "over"}]
        rules = default_rules()
        next(r for r in rules if r["id"] == "mem-high")["critForS"] = 0
        with open(os.path.join(self.config, "rules.json"), "w") as f:
            json.dump({"version": 1, "rules": rules}, f)
        a = self.make(cfg(silences=silences), collect=lambda: frame(disk_pct=90, mem_pct=97),
                        outbox_senders={"ntfy": lambda ch, m: sent.append(m["title"])})
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        now = agent.now_ms()
        a.cycle(now=now)
        a.cycle(now=now + gate.GROUP_WAIT_MS)
        a.outbox.drain()
        self.assertEqual(sent, ["cell1 memory 97%"], "the disk is silenced; the expired silence on memory doesn't count")
        with open(os.path.join(self.state, "deliveries.jsonl")) as f:
            lines = [json.loads(l) for l in f]
        disk = next(l for l in lines if l.get("rule") == "disk-full")
        self.assertEqual((disk["outcome"], disk["reason"]), ("silenced", "expanding the disk on Friday"))


class UntilItClears(Dirs):
    def test_a_silence_until_it_clears_ends_with_its_alert(self):
        sent = []
        disk = [90]
        silences = [{"id": "s-9", "rule": "disk-full", "entity": "disk:cell1:/", "incident": None, "untilMs": None, "untilClears": True,
                     "reason": "cleaning up"}]
        a = self.make(cfg(silences=silences), collect=lambda: frame(disk_pct=disk[0]), outbox_senders={"ntfy": lambda ch, m: sent.append(m["title"])})
        a.outbox.channels = {"phone": {"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": True, "topic": "t"}}
        now = agent.now_ms()
        a.cycle(now=now)
        disk[0] = 50
        a.cycle(now=now + 2000)
        disk[0] = 90
        a.cycle(now=now + 4000)
        a.cycle(now=now + 4000 + gate.GROUP_WAIT_MS)
        a.outbox.drain()
        self.assertEqual(sent, ["cell1 disk 90%"], "silenced the first time; it cleared, so the next one pages")
        self.assertEqual(a.silences_done, {"s-9"})


class Settings(Dirs):
    def install(self, src):
        for name in ("agent.json", "rules.json", "notify.json"):
            shutil.copy(os.path.join(src, name), os.path.join(self.config, name))

    def test_config_hash_fixture(self):
        self.install(CONFIG_HASH)
        with open(os.path.join(CONFIG_HASH, "expected.txt")) as f:
            expected = f.read().strip()
        a = self.make(collect=frame)
        self.assertEqual(a.hash, expected)
        a.cycle()
        self.assertNotIn("configHash", a.health())
        self.assertNotIn("configMac", a.health(), "no request key in this notify.json: /healthz serves nothing of the hash")
        self.assertEqual(a.status()["configHash"], expected)
        with open(os.path.join(self.state, "beat.json")) as f:
            self.assertEqual(json.load(f)["configHash"], expected)
        self.assertEqual((a.cfg.node, a.cfg.index, len(a.cfg.peers), a.cfg.switch["host"]), ("cell3", 2, 3, "192.168.88.1"))
        self.assertEqual(a.health()["quietTz"], "America/New_York")

    def test_sighup_rereads_agent_and_rules_but_not_the_channels(self):
        self.install(CONFIG_HASH)
        a = self.make(collect=frame)
        old = signal.getsignal(signal.SIGHUP), signal.getsignal(signal.SIGTERM)
        self.addCleanup(lambda: (signal.signal(signal.SIGHUP, old[0]), signal.signal(signal.SIGTERM, old[1])))
        a.install_signals()
        rules = b'{"version":1,"rules":[]}'
        with open(os.path.join(self.config, "rules.json"), "wb") as f:
            f.write(rules)
        with open(os.path.join(self.config, "agent.json")) as f:
            v = json.load(f)
        v["repeatMinutes"] = 15
        agent_raw = json.dumps(v).encode()
        with open(os.path.join(self.config, "agent.json"), "wb") as f:
            f.write(agent_raw)
        with open(os.path.join(self.config, "notify.json"), "w") as f:
            f.write('{"channels": []}')
        os.kill(os.getpid(), signal.SIGHUP)
        for _ in range(50):
            if a.reload_wanted:
                break
            time.sleep(0.01)
        self.assertTrue(a.reload_wanted)
        a.reload()
        with open(os.path.join(CONFIG_HASH, "notify.json"), "rb") as f:
            notify_raw = f.read()
        self.assertEqual(a.hash, hashlib.sha256(agent_raw + b"\0" + rules + b"\0" + notify_raw).hexdigest(),
                         "agent.json and rules.json as they are now; the channels as read at start")
        self.assertEqual((a.rules, a.gate.settings.repeat_ms), ([], 15 * 60_000))
        self.assertEqual([c["id"] for c in a.notify["channels"]], ["phone"], "the credential is read once, at start")


class ServingWhileTheMacSleeps(Dirs):
    """Endpoints probed on loopback with serving_probe.py against r3-serving's fake vLLM."""

    def setUp(self):
        super().setUp()
        self.fake = serving_probe_test.Fake("vllm")
        self.addCleanup(self.fake.close)
        self.ep = {"id": "llama", "name": "llama-3.3-70b", "engine": "vllm", "port": self.fake.port, "model": "llama-3.3-70b",
                   "headNode": "cell1", "keyRef": "llama"}
        with open(os.path.join(self.config, "notify.json"), "w") as f:
            json.dump({"channels": [], "endpointKeys": {"llama": "sk-local-7f3a"}}, f)
        self.now = [serving_probe_test.NOW]
        self.a = self.make(cfg(endpoints=[self.ep]), collect=frame)
        self.a.endpoints.clock = lambda: self.now[0] / 1000.0

    def step(self, seconds):
        self.now[0] += seconds * 1000
        self.a.endpoints.probe_once("llama")
        return self.a.cycle(now=self.now[0])

    def test_series_alerts_and_the_key_stay_where_they_belong(self):
        timeout, serving_probe_test.sp.CANARY_TIMEOUT_S = serving_probe_test.sp.CANARY_TIMEOUT_S, 1
        self.addCleanup(setattr, serving_probe_test.sp, "CANARY_TIMEOUT_S", timeout)
        events = self.step(0)
        events += self.step(15)
        self.assertEqual(self.a.health()["endpointsProbed"], ["llama"])
        self.fake.stall = True  # it hangs: 3 requests stay running, no tokens come
        when = {}
        for _ in range(5):
            for ev in self.step(15):
                if ev["kind"] == "raised":
                    when[ev["alert"]["rule"]] = ((self.now[0] - serving_probe_test.NOW) // 1000, ev["alert"])
        self.assertEqual(when["endpoint-stalled"][0], 60, "stalled once no token came for 45 s")
        self.assertEqual(when["endpoint-stalled"][1]["severity"], "crit")
        down_for = next(r["forS"] for r in default_rules() if r["id"] == "endpoint-down")
        self.assertEqual(when["endpoint-down"][0], 60 + down_for, "Down by the probe, held for the rule's own time, as the app holds it")
        self.assertEqual(when["endpoint-down"][1]["message"], "Stalled: 3 requests running, no tokens for %d s" % (45 + down_for))
        self.assertEqual(when["endpoint-down"][1]["incident"], "endpoint:llama")
        self.fake.stall = False
        events = []
        for _ in range(3):
            events += self.step(15)
        self.assertIn(("resolved", "endpoint-down"), [(ev["kind"], ev["alert"]["rule"]) for ev in events], "Ready again clears it")
        self.a.history.flush()
        metrics = {m for (m,) in self.a.history.db.execute("SELECT DISTINCT metric FROM points WHERE entity = 'endpoint:llama'")}
        self.assertTrue({"genTps", "running", "kvPct", "canaryMs"} <= metrics, metrics)
        auth = {s[2].get("Authorization") for s in self.fake.seen if s[1] in ("/v1/models", "/v1/completions")}
        self.assertEqual(auth, {"Bearer sk-local-7f3a"}, "the key travels in the header")
        for name in os.listdir(self.state):
            if os.path.isfile(os.path.join(self.state, name)):
                with open(os.path.join(self.state, name), "rb") as f:
                    self.assertNotIn(b"sk-local-7f3a", f.read(), name)
        self.assertNotIn("sk-local-7f3a", json.dumps(self.a.status()))


class CommandLine(Dirs):
    def background(self, a):
        stop = threading.Event()
        self.addCleanup(stop.set)

        def loop():
            while not stop.is_set():
                a.handle_requests()
                stop.wait(0.05)
        threading.Thread(target=loop, daemon=True).start()
        a.outbox.start()

    def test_test_notify_one_channel_and_all(self):
        srv, h, url = http_server(status=200)
        self.addCleanup(srv.shutdown)
        sent = []
        with open(os.path.join(self.config, "notify.json"), "w") as f:
            json.dump({"channels": [{"id": "phone", "kind": "ntfy", "name": "Phone", "enabled": False, "topic": "t"},
                                    {"id": "hook", "kind": "webhook", "name": "Hook", "enabled": True, "url": "x"},
                                    {"id": "hc", "kind": "heartbeat", "name": "healthchecks.io", "enabled": True, "url": url + "/ping"}]}, f)
        a = self.make(cfg(), collect=frame, outbox_senders={"ntfy": lambda ch, m: sent.append(("phone", m["title"])),
                                                              "webhook": lambda ch, m: sent.append(("hook", m["title"]))})
        self.background(a)
        out = json.loads(run_cli(["test-notify", "phone"]))
        self.assertEqual((out["ok"], out["message"]), (True, "Sent from the agent on cell1."))
        self.assertEqual(out["delivery"]["channel"], "phone")
        self.assertEqual(sent, [("phone", "SparkBox test from the agent on cell1")], "a test goes out even on a disabled channel")
        self.assertIn("has no channel", json.loads(run_cli(["test-notify", "nope"]))["message"])
        time.sleep(agent.TEST_EVERY_MS / 1000.0)
        out = json.loads(run_cli(["test-notify", "--all", "--json"]))
        self.assertEqual((out["node"], out["ok"]), ("cell1", True), out)
        self.assertEqual(sorted((r["channel"], r["ok"]) for r in out["results"]), [("hc", True), ("hook", True)],
                         "every enabled channel, and the dead-man's switch answers a GET")
        self.assertTrue(all(isinstance(r["ms"], int) for r in out["results"]))
        out = json.loads(run_cli(["test-notify", "--all", "--json"]))
        self.assertFalse(out["ok"], "a channel tested seconds ago waits")

    def test_status_export_alerts_and_deliveries(self):
        a = self.make(cfg(), collect=lambda: frame(mem_pct=97))
        next(r for r in a.rules if r["id"] == "mem-high")["critForS"] = 0
        for _ in range(4):
            a.cycle()
            time.sleep(0.01)
        a.history.flush()
        agent.write_atomic(agent.state_path("status.json"), json.dumps(a.status()))
        st = json.loads(run_cli(["status", "--json"]))
        self.assertTrue(HEALTHZ <= set(st), HEALTHZ - set(st))
        self.assertEqual((st["node"], st["version"], st["configHash"]), ("cell1", agent.VERSION, a.hash))
        self.assertEqual(st["firing"][0]["summary"], "cell1 memory 97%")
        self.assertEqual(st["firing"][0]["paging"], "sent")
        self.assertIsNotNone(st["lastBeatMs"])
        self.assertEqual(st["historyVacuum"], "incremental")
        lines = [json.loads(l) for l in run_cli(["alerts", "--since", "0"]).splitlines()]
        self.assertEqual((lines[0]["kind"], lines[0]["alert"]["rule"], lines[0]["alert"]["severity"]), ("raised", "mem-high", "crit"))
        rows = [json.loads(l) for l in run_cli(["export", "--since", "0", "--entity", "node:cell1"]).splitlines()]
        self.assertTrue(rows and rows[0]["e"] == "node:cell1" and "memUsed" in rows[0]["v"])
        outcomes = [json.loads(l) for l in run_cli(["deliveries", "--since", "0"]).splitlines()]
        self.assertEqual((outcomes[0]["rule"], outcomes[0]["outcome"]), ("mem-high", "sent"))

    def test_export_reads_a_history_it_cant_write(self):
        a = self.make(cfg(), collect=frame)
        a.cycle()
        a.close()
        os.chmod(self.state, 0o555)
        try:
            rows = [json.loads(l) for l in run_cli(["export", "--since", "0"]).splitlines()]
        finally:
            os.chmod(self.state, 0o755)
        self.assertTrue(rows and rows[0]["e"].startswith("node:"), "read-only, as the login user reads it")


def run_cli(args):
    import io
    import contextlib
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        agent.main(args)
    return buf.getvalue()


class WholeNode(collector_test.WholeFrame):
    """The agent sampling fixture /proc and /sys trees with the collector's own code."""

    test_a_frame_has_the_whole_schema = test_interfaces_map_to_rdma_devices = None
    test_rdma_counters_are_4_byte_words_and_prefer_ext = test_rdma_rate_in_bits_and_through_a_wrap = None
    test_firmware_comes_from_the_rdma_device = None

    def test_a_cycle_on_fixture_trees_records_history_and_beats(self):
        state, config = tempfile.mkdtemp(), tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, state)
        self.addCleanup(shutil.rmtree, config)
        agent.STATE, agent.CONFIG = state, config
        collector_test.reloaded()
        a = agent.Agent(cfg("cell1"))
        self.addCleanup(a.close)
        a.sampler = collector.Collector(stream_gpu=False, kernel=False)
        a.cycle()
        a.history.flush()
        with open(os.path.join(state, "beat.json")) as f:
            beat = json.load(f)
        self.assertEqual((beat["node"], beat["version"]), ("cell1", agent.VERSION))
        metrics = {m for (m,) in a.history.db.execute("SELECT DISTINCT metric FROM points WHERE entity = 'node:cell1'")}
        self.assertTrue({"cpu", "memUsed", "fabricRx", "rdmaRx", "lanRx"} <= metrics, metrics)


class RunAsAService(collector_test.WholeFrame):
    """`sparkbox_agent.py run` as systemd runs it, from a folder laid out like
    /opt/sparkbox-agent, on fixture /proc and /sys trees: READY=1, WATCHDOG=1 every cycle,
    /healthz on loopback, SIGHUP re-reads the settings, SIGTERM stops it."""

    # Not the collector's own tests again.
    test_a_frame_has_the_whole_schema = test_interfaces_map_to_rdma_devices = None
    test_rdma_counters_are_4_byte_words_and_prefer_ext = test_rdma_rate_in_bits_and_through_a_wrap = None
    test_firmware_comes_from_the_rdma_device = None

    def test_the_service_starts_watches_reloads_and_stops(self):
        import subprocess
        opt = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, opt, True)
        for path in ("sparkbox_agent.py", "gate.py", "peers.py", "serving_probe.py", os.path.join(LIVE, "collector.py"), os.path.join(LIVE, "roce.py")):
            shutil.copy(os.path.join(HERE, path), opt)
        state, config = tempfile.mkdtemp(), tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, state, True)
        self.addCleanup(shutil.rmtree, config, True)
        collector_test.write(os.path.join(self.proc, "net/route"), "Iface\tDestination\tGateway\n")
        collector_test.write(os.path.join(self.proc, "sys/kernel/random/boot_id"), "8c0f1e3a-4b5c-6d7e-8f9a-0b1c2d3e4f50\n")
        port = free_port()
        with open(os.path.join(config, "agent.json"), "w") as f:
            json.dump({"node": "cell1", "name": "cell1", "index": 0, "port": port, "peers": [{"id": "cell1", "index": 0}]}, f)
        with open(os.path.join(config, "rules.json"), "w") as f:
            json.dump({"version": 1, "rules": default_rules()}, f)
        with open(os.path.join(config, "notify.json"), "w") as f:
            json.dump({"channels": [], "requestKey": KEY}, f)
        sock_dir = tempfile.mkdtemp(dir="/tmp" if os.path.isdir("/tmp") else None)
        self.addCleanup(shutil.rmtree, sock_dir, True)
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.bind(os.path.join(sock_dir, "notify"))
        sock.settimeout(20)
        self.addCleanup(sock.close)
        env = dict(os.environ, SPARKBOX_AGENT_STATE=state, SPARKBOX_AGENT_CONFIG=config, NOTIFY_SOCKET=os.path.join(sock_dir, "notify"),
                   SPARKBOX_PROC_ROOT=self.proc, SPARKBOX_SYSFS_ROOT=self.root)
        p = subprocess.Popen([sys.executable, "-I", os.path.join(opt, "sparkbox_agent.py"), "run"], env=env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: p.poll() is None and p.kill())
        got = [sock.recv(256).decode() for _ in range(3)]
        self.assertIn("READY=1", got)
        self.assertIn("WATCHDOG=1", got)
        status, _, body = get("http://127.0.0.1:%d/healthz" % port)
        v = json.loads(body)
        self.assertEqual((status, v["node"], v["leader"]), (200, "cell1", True))
        self.assertGreater(v["lastCycleMs"], agent.now_ms() - 10_000)
        first = v["configMac"]
        with open(os.path.join(config, "rules.json"), "w") as f:
            json.dump({"version": 1, "rules": []}, f)
        p.send_signal(signal.SIGHUP)
        for _ in range(40):
            v = json.loads(get("http://127.0.0.1:%d/healthz" % port)[2])
            if v["configMac"] != first:
                break
            time.sleep(0.25)
        self.assertNotEqual(v["configMac"], first, "systemctl reload took the new rules")
        p.send_signal(signal.SIGTERM)
        self.assertEqual(p.wait(10), 0)
        err = p.stderr.read()
        p.stderr.close()
        self.assertIn("settings reloaded", err)
        self.assertNotIn("Traceback", err)
        with open(os.path.join(state, "beat.json")) as f:
            self.assertEqual(json.load(f)["bootId"], "8c0f1e3a-4b5c-6d7e-8f9a-0b1c2d3e4f50")


class Vacuum(unittest.TestCase):
    """No more daily full VACUUM: incremental auto-vacuum, a little every hour."""

    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)

    def test_a_new_history_is_incremental_and_an_r2_one_moves_over_when_small(self):
        h = agent.History(os.path.join(self.dir, "new.db"))
        self.addCleanup(h.db.close)
        self.assertEqual(h.auto_vacuum(), "incremental")
        old = os.path.join(self.dir, "r2.db")
        db = sqlite3.connect(old)
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("CREATE TABLE points (tier INTEGER NOT NULL, entity TEXT NOT NULL, metric TEXT NOT NULL, ts INTEGER NOT NULL, avg REAL, "
                   "min REAL, max REAL, n INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (tier, entity, metric, ts)) WITHOUT ROWID")
        db.execute("INSERT INTO points VALUES (0, 'node:cell1', 'cpu', 1, 1, 1, 1, 1)")
        db.commit()
        db.close()
        h = agent.History(old)
        self.addCleanup(h.db.close)
        self.assertEqual(h.auto_vacuum(), "incremental", "moved over at start (under 50 MB)")
        self.assertEqual(h.db.execute("SELECT COUNT(*) FROM points").fetchone()[0], 1, "nothing lost")

    def test_a_big_r2_history_waits_for_a_later_restart(self):
        old = os.path.join(self.dir, "big.db")
        db = sqlite3.connect(old)
        db.execute("CREATE TABLE t (x)")
        db.commit()
        db.close()
        under, agent.MIGRATE_UNDER = agent.MIGRATE_UNDER, 1
        self.addCleanup(setattr, agent, "MIGRATE_UNDER", under)
        h = agent.History(old)
        self.addCleanup(h.db.close)
        self.assertEqual(h.auto_vacuum(), "none")
        self.assertIn("incremental vacuum starts at a restart", h.note)

    def test_pruning_gives_space_back_a_little_at_a_time(self):
        h = agent.History(os.path.join(self.dir, "h.db"))
        self.addCleanup(h.db.close)
        now = 1_800_000_000_000
        for i in range(3000):
            h.add(now - (3000 - i) * 10_000, "node:cell1", {m: float(i) for m in agent.NODE_METRICS})
        h.flush()
        h.db.execute("DELETE FROM points WHERE tier = 0")
        h.db.commit()
        free = h.db.execute("PRAGMA freelist_count").fetchone()[0]
        self.assertGreater(free, 20)
        h.vacuum(pages=10)
        self.assertEqual(h.db.execute("PRAGMA freelist_count").fetchone()[0], free - 10, "ten pages back, no full rewrite")


class Logs(unittest.TestCase):
    def test_the_logs_keep_their_newest_lines(self):
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d)
        path = os.path.join(d, "alerts.jsonl")
        for i in range(400):
            agent.append_line(path, {"tsMs": i, "kind": "raised", "pad": "x" * 40})
        self.assertFalse(agent.trim_log(path, keep=100_000), "small enough")
        self.assertTrue(agent.trim_log(path, keep=4000))
        with open(path) as f:
            lines = [json.loads(l) for l in f]
        self.assertLessEqual(os.path.getsize(path), 4000)
        self.assertEqual(lines[-1]["tsMs"], 399, "the newest stay, whole lines only")


class Unit(unittest.TestCase):
    """What the agent needs from r3-paging's unit file."""

    def test_the_unit_runs_it_as_a_watched_notify_service(self):
        with open(os.path.join(HERE, "sparkbox-agent.service")) as f:
            unit = f.read()
        for line in ("Type=notify", "NotifyAccess=main", "WatchdogSec=30", "ExecReload=/bin/kill -HUP $MAINPID",
                     "User=sparkbox-agent", "StateDirectory=sparkbox-agent", "SupplementaryGroups=systemd-journal docker",
                     "LoadCredential=notify:/etc/sparkbox-agent/notify.json", "Restart=always",
                     "ExecStart=/usr/bin/python3 -I /opt/sparkbox-agent/sparkbox_agent.py run"):
            self.assertIn(line + "\n", unit)
        self.assertLess(agent.WEDGED_MS, 30_000 + 1, "/healthz says wedged no later than systemd's watchdog fires")


if __name__ == "__main__":
    unittest.main()
