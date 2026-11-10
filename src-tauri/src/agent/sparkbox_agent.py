#!/usr/bin/env python3
"""SparkBox agent: keeps watching a Spark while SparkBox is closed.

SparkBox installs it in /opt/sparkbox-agent with gate.py, peers.py, collector.py, roce.py
and serving_probe.py, and systemd runs it as the sparkbox-agent service user
(sparkbox-agent.service: Type=notify, WatchdogSec=30, reload on SIGHUP). It samples the
node with the collector's own code, keeps history in SQLite, evaluates the same alert
rules as the app (/etc/sparkbox-agent/rules.json), probes the models this node serves,
watches the other agents on every path, and pages the owner (ntfy, Pushover, a webhook
or email) through the channels systemd hands it as a credential
(LoadCredential=notify:/etc/sparkbox-agent/notify.json, root 0600: this service and root
read it, and through Docker anyone in the docker group). It never changes anything on the
node.

Threads: the main loop samples, evaluates and queues every 2 s and never waits on the
network; one delivery worker per channel drains the outbox (kept in SQLite, so a restart
or a settings push loses nothing); the peer watch, the endpoint probes, the dead-man's
switch pings and the HTTP listeners run on their own. After each completed cycle the
loop tells systemd WATCHDOG=1, and /healthz answers 503 when no cycle has completed for
30 s, so a wedged agent is restarted by systemd and reported by its peers.

Files:
  /etc/sparkbox-agent/agent.json   this node, its peers, endpoints, silences, quiet hours (from SparkBox)
  /etc/sparkbox-agent/rules.json   the rule set (the same JSON as the app's)
  /var/lib/sparkbox-agent/         history.db, outbox.db (WAL), state.json, alerts.jsonl,
                                   deliveries.jsonl, beat.json, status.json; requests/ is
                                   where SparkBox drops acknowledgements, maintenance windows
                                   and test requests

Acknowledgements and maintenance windows are signed: HMAC-SHA256 with the request key that
comes in notify.json (the credential), over "<kind>\n<body>". An unsigned, wrongly signed,
replayed or stale one (over 24 h old, or more than a minute ahead of this clock) is refused
and logged, and the answer is left next to it (<name>.result.json) for SparkBox to read.
Writing into requests/ is not enough to silence paging: that takes the key, which only root
and this service read (and, through Docker, anyone in the docker group).

Commands: run | once | status [--json] | export --since MS [--until MS] [--entity E] |
alerts --since MS | deliveries --since MS | test-notify CHANNEL_ID | test-notify --all --json | stream

Python 3.10 or newer, standard library only.
"""
import collections
import hashlib
import hmac
import http.server
import json
import math
import os
import re
import signal
import smtplib
import socket
import socketserver
import sqlite3
import ssl
import stat
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from email.message import EmailMessage
from email.utils import formatdate

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import collector  # noqa: E402
import gate  # noqa: E402
import peers  # noqa: E402

try:
    import serving_probe  # noqa: E402 - r3-serving's prober; endpoints aren't probed without it
except ImportError:
    serving_probe = None
#: The per-interface RoCE verdict lives next to the counter reader (roce.py, shipped with the collector).
roce_iface = collector.roce if getattr(collector.roce, "judge", None) else None

VERSION = "1.3.0"
STATE = os.environ.get("SPARKBOX_AGENT_STATE", "/var/lib/sparkbox-agent")
CONFIG = os.environ.get("SPARKBOX_AGENT_CONFIG", "/etc/sparkbox-agent")

#: Sampling cadences (seconds): core numbers, GPU, Docker, slow facts.
CORE_EVERY, GPU_EVERY, DOCKER_EVERY, SLOW_EVERY = 2.0, 2.0, 5.0, 30.0
#: /healthz says wedged (503) when no cycle has completed for this long.
WEDGED_MS = max(30_000, int(3 * CORE_EVERY * 1000))
#: History tiers: (tier, bucket seconds, kept seconds).
TIERS = ((0, 10, 48 * 3600), (1, 60, 14 * 86400), (2, 600, 180 * 86400))
HISTORY_CAP = 300 * 1024 * 1024
#: An older history.db moves to incremental vacuum at start only while it's smaller than this.
MIGRATE_UNDER = 50 * 1024 * 1024
DEFAULT_PORT = 9469
#: Own addresses are checked this often, and the listeners rebound when they change.
REBIND_EVERY = 30.0
#: Delivery retries after a failure that may pass (seconds after the previous try); then a
#: critical is tried every 30 min until it's a day old.
RETRIES = (30, 120, 600)
RETRY_CRIT_EVERY, RETRY_CRIT_FOR = 1800, 24 * 3600
#: The dead-man's switch is pinged this often (by the leader).
HEARTBEAT_EVERY = 60.0
#: Endpoint probes (serving_probe decides when a probe includes the one-token canary).
ENDPOINT_EVERY = 15.0
#: Requests in requests/ are small JSON files; a test per channel at most this often.
MAX_REQUEST, TEST_EVERY_MS = 4096, 10_000
#: The HTTP listeners answer each client at most this often per second.
RATE_PER_S = 20
#: Signed requests older than this, or more than REQUEST_SKEW_MS ahead of this clock, are refused.
REQUEST_MAX_AGE_MS, REQUEST_SKEW_MS = 24 * 3600_000, 60_000
#: Maintenance windows last at most this long, shutdowns included; one whose node never went
#: down (a restart refused or called off) closes after MAINT_NEVER_DOWN_MS.
MAINT_MAX_MS, MAINT_NEVER_DOWN_MS = 4 * 3600_000, 20 * 60_000
#: Result files nobody collected are removed after this long.
RESULT_KEEP_MS = 3600_000
#: The crash-loop rule (health/rules.rs CRASH_RECENT_S, CRASH_WINDOW_S; agent.json "crash"): a
#: container running this long has settled, and a raised crash loop holds this long after the
#: last restart, so one loop is one raise and one resolve.
CRASH_RECENT_S, CRASH_WINDOW_S = 60, 600
#: GPU throttling (health/rules.rs throttle_verdict, health/fixtures/gpu-throttle-cases.json):
#: hardware reasons always count; the driver's own (SW power cap, SW thermal, sync boost and
#: anything newer) only while the GPU is busy; idle and application clocks never.
THROTTLE_ALWAYS = ("hw slowdown", "hw thermal slowdown", "hw power brake slowdown")
THROTTLE_NEVER = ("gpu idle", "applications clocks setting", "display clock setting", "none")
THROTTLE_BUSY_UTIL_PCT, THROTTLE_CLOCK_BELOW, THROTTLE_POWER_AT = 50.0, 0.9, 0.8
#: alerts.jsonl and deliveries.jsonl keep about this much each.
LOG_KEEP = 8 * 1024 * 1024

NODE_METRICS = ("cpu", "gpu", "memUsed", "gpuAlloc", "power", "gpuTemp", "fabricRx", "fabricTx", "rdmaRx", "rdmaTx",
                "lanRx", "lanTx", "diskRead", "diskWrite", "memCache", "smClock")
NET_METRICS = ("ecn", "cnpSent", "cnpHandled", "oos", "seqErr", "timeouts", "pauseRx", "pauseTx", "discards", "crc")
ENDPOINT_METRICS = ("genTps", "promptTps", "running", "waiting", "kvPct", "ttftP95", "canaryMs", "preemptions")
#: Rules this agent evaluates itself (fixtures/agent-rules.json; SparkBox leaves these to it
#: while it's healthy and on the current settings).
EVALUATES = ("mem-high", "disk-full", "gpu-hot", "gpu-throttle", "ntp-unsynced",
             "kernel-xid", "kernel-mlx5", "kernel-oom", "kernel-hung",
             "fabric-down", "fabric-flap", "net-errors", "fabric-mtu",
             "crash-loop", "container-exited", "unit-failed",
             "endpoint-down", "endpoint-stalled", "roce-iface", "gpu-unreadable", "fabric-lost", "agent-wedged")
GENERIC_ENTITIES = ("node", "net", "container", "endpoint")
#: Raised from what the agents see of each other (the leader only), not from a frame.
PEER_RULES = ("peer-unreachable", "node-reboot", "switch-unreachable", "fabric-switch-down")
#: Defaults for coded rules a rules.json from an older SparkBox doesn't list yet.
CODED_DEFAULTS = {
    "gpu-unreadable": {"area": "gpu", "forS": 30, "clearS": 0},
    "roce-iface": {"area": "fabric", "forS": 0, "clearS": 30},
    "endpoint-down": {"area": "serving", "forS": 0, "clearS": 0},
    "endpoint-stalled": {"area": "serving", "forS": 45, "clearS": 0},
    "fabric-lost": {"area": "fabric", "forS": 0, "clearS": 30},
    "agent-wedged": {"area": "node", "forS": 0, "clearS": 30},
    "peer-unreachable": {"area": "node", "forS": 0, "clearS": 0},
    "node-reboot": {"area": "node", "forS": 600, "clearS": 0},
    "switch-unreachable": {"area": "fabric", "forS": 10, "clearS": 30},
    "fabric-switch-down": {"area": "fabric", "forS": 0, "clearS": 30},
}
#: r2's per-counter RoCE rules, folded into roce-iface (health/roce_rules.rs OLD_RULES).
ROCE_R2 = ("roce-out-of-sequence", "roce-seq-errors", "roce-ack-timeouts", "roce-crc", "roce-symbol", "roce-discards",
           "roce-out-of-buffer", "roce-pause-tx", "roce-pause-rx")
CATEGORY = {"disk": "disk", "gpu": "gpu", "fabric": "fabric", "roce": "fabric", "containers": "containers", "serving": "containers"}
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
REQUEST_ID = re.compile(r"^[A-Za-z0-9_-]{1,40}$")


def now_ms():
    return int(time.time() * 1000)


def state_path(name):
    return os.path.join(STATE, name)


def load_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def read_bytes(path):
    try:
        with open(path, "rb") as f:
            return f.read()
    except OSError:
        return b""


def parse_json(raw, default):
    """JSON from bytes, or `default` when it's missing, unreadable or not the same kind of
    value as `default` (a dict for {}); with None for `default`, any JSON."""
    try:
        v = json.loads(raw.decode("utf-8")) if raw else default
    except (ValueError, UnicodeDecodeError):
        return default
    return v if default is None or isinstance(v, type(default)) else default


def write_atomic(path, data):
    tmp = "%s.%d.tmp" % (path, os.getpid())
    with open(tmp, "w") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def append_line(path, obj):
    with open(path, "a") as f:
        f.write(json.dumps(obj, separators=(",", ":")) + "\n")


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def trim_log(path, keep=LOG_KEEP):
    """Keeps a JSON-lines log under twice `keep` bytes: past that, its newest `keep` bytes
    (whole lines) stay."""
    try:
        if os.path.getsize(path) <= 2 * keep:
            return False
        with open(path, "rb") as f:
            f.seek(-keep, os.SEEK_END)
            tail = f.read()
    except OSError:
        return False
    tail = tail[tail.find(b"\n") + 1:]
    tmp = "%s.%d.tmp" % (path, os.getpid())
    with open(tmp, "wb") as f:
        f.write(tail)
    os.replace(tmp, path)
    return True


# ---- Configuration

def config_hash(agent_raw, rules_raw, notify_raw):
    """sha256(agent.json ‖ 0x00 ‖ rules.json ‖ 0x00 ‖ notify.json), hex, over the bytes as
    installed (fixtures/config-hash): SparkBox compares it with the hash of what it pushed."""
    return hashlib.sha256(agent_raw + b"\0" + rules_raw + b"\0" + notify_raw).hexdigest()


class Config:
    """agent.json as SparkBox writes it (agent/mod.rs documents the schema):
    {node, name, index, port, peers: [{id, name, index, lan, fabric: [ip]}] (this node
    included), switchMgmt, endpoints: [{id, name, engine, port, model, headNode, keyRef}],
    silences, quiet: {start, end, tz}, repeatMinutes, criticalOnly, resolved, categories,
    metrics: {enabled, port, bind}}. r2's file (peers with one addr, the notify policy
    in notify.json) reads too."""

    def __init__(self, data, raw=b""):
        self.raw = raw
        self.node = data.get("node") or socket.gethostname()
        self.name = data.get("name") or self.node
        self.nodes = [p for p in data.get("peers") or [] if isinstance(p, dict) and isinstance(p.get("id"), str)]
        position = next((i for i, p in enumerate(self.nodes) if p["id"] == self.node), 0)
        self.index = int(data["index"]) if isinstance(data.get("index"), int) else position
        self.peers = [p for p in self.nodes if p["id"] != self.node]
        self.me = next((p for p in self.nodes if p["id"] == self.node), {})
        self.port = int(data.get("port", DEFAULT_PORT))
        bind = data.get("bind")
        self.bind = [bind] if isinstance(bind, str) else [b for b in bind or [] if isinstance(b, str)]
        m = data.get("metrics") or {}
        self.metrics_enabled = bool(m.get("enabled"))
        self.metrics_port = int(m.get("port", self.port))
        self.metrics_bind = m.get("bind")
        sw = data.get("switchMgmt")
        if isinstance(sw, str) and sw:
            sw = {"host": sw}
        self.switch = dict({"id": "switch", "name": "The switch"}, **sw) if isinstance(sw, dict) and sw.get("host") else None
        self.endpoints = [e for e in data.get("endpoints") or [] if isinstance(e, dict) and isinstance(e.get("id"), str)]
        self.silences = [s for s in data.get("silences") or [] if isinstance(s, dict)]
        self.policy = {k: data[k] for k in ("criticalOnly", "resolved", "repeatMinutes", "quiet", "categories", "enabled") if k in data}
        # The Mac's IANA zone, for times in messages ("at 10:42"); else the node's clock.
        self.tz = data.get("tz") if isinstance(data.get("tz"), str) and data.get("tz") else None
        crash = data.get("crash") if isinstance(data.get("crash"), dict) else {}
        self.crash_recent_s = int(crash.get("recentS") or CRASH_RECENT_S)
        self.crash_window_s = int(crash.get("windowS") or CRASH_WINDOW_S)

    @staticmethod
    def load():
        raw = read_bytes(os.path.join(CONFIG, "agent.json"))
        return Config(parse_json(raw, {}), raw)


def load_rules():
    """(rules, the file's bytes)."""
    raw = read_bytes(os.path.join(CONFIG, "rules.json"))
    rules = parse_json(raw, {}).get("rules")
    return (rules if isinstance(rules, list) else []), raw


def load_notify():
    """(channels and secrets, the bytes): the systemd credential, else (tests) the config
    file. Read once at start; a change of channels restarts the agent."""
    creds = os.environ.get("CREDENTIALS_DIRECTORY")
    for path in ([os.path.join(creds, "notify")] if creds else []) + [os.path.join(CONFIG, "notify.json")]:
        raw = read_bytes(path)
        data = parse_json(raw, None) if raw else None
        if isinstance(data, dict):
            return data, raw
    return {"channels": []}, b""


def silence_active(s, now, done):
    """A silence from agent.json still applies: {rule, entity, incident, untilMs,
    untilClears} as SparkBox writes it, or {matcher: …, until: {kind: at|resolved|never}}."""
    if s.get("untilClears") and s.get("id") in done:
        return False
    until = s.get("until")
    if isinstance(until, dict):
        if until.get("kind") == "at" and now >= (until.get("ms") or 0):
            return False
        if until.get("kind") == "resolved" and s.get("id") in done:
            return False
    elif isinstance(s.get("untilMs"), (int, float)) and now >= s["untilMs"]:
        return False
    return True


def silence_matches(s, ev):
    m = s.get("matcher") if isinstance(s.get("matcher"), dict) else s
    if not (m.get("rule") or m.get("entity") or m.get("incident")):
        return False
    return ((not m.get("rule") or m["rule"] == ev.get("rule"))
            and (not m.get("entity") or gate.same_entity(m["entity"], ev.get("entity")))
            and (not m.get("incident") or m["incident"] == ev.get("incident")))


# ---- Readings

def pct(part, whole):
    return part / whole * 100.0 if whole else 0.0


def root_disk(frame):
    disks = frame.get("disks") or []
    for d in disks:
        if d.get("mount") == "/":
            return d
    return disks[0] if disks else None


def node_metrics(frame):
    """The 16 node series in the app's definitions (src-tauri/src/history/mod.rs sample())."""
    nets = frame.get("nets") or []
    def total(role, key):
        return sum(n.get(key) or 0 for n in nets if n.get("role") == role)
    seen, rdma_rx, rdma_tx = set(), 0.0, 0.0
    for n in nets:
        if n.get("role") != "fabric":
            continue
        dev = n.get("rdmaDev") or n.get("iface")
        if dev in seen or (n.get("rdmaRxBps") is None and n.get("rdmaTxBps") is None):
            continue
        seen.add(dev)
        rdma_rx += n.get("rdmaRxBps") or 0.0
        rdma_tx += n.get("rdmaTxBps") or 0.0
    disk = root_disk(frame) or {}
    cpu, mem, gpu = frame.get("cpu") or {}, frame.get("mem") or {}, frame.get("gpu") or {}
    return {
        "cpu": cpu.get("totalPct"), "gpu": gpu.get("utilPct"), "memUsed": mem.get("used"), "gpuAlloc": mem.get("gpuAlloc"),
        "power": gpu.get("powerW"), "gpuTemp": gpu.get("tempC"),
        "fabricRx": total("fabric", "rxBps") + rdma_rx, "fabricTx": total("fabric", "txBps") + rdma_tx,
        "rdmaRx": rdma_rx, "rdmaTx": rdma_tx, "lanRx": total("lan", "rxBps"), "lanTx": total("lan", "txBps"),
        "diskRead": disk.get("readBps", 0.0), "diskWrite": disk.get("writeBps", 0.0),
        "memCache": mem.get("cached"), "smClock": gpu.get("smClockMhz"),
    }


def roce_value(roce, metric):
    """A RoCE counter by the app's name (ecn, oos…) or its snapshot name; None when absent."""
    if not roce:
        return None
    def f(k):
        v = roce.get(k)
        return float(v) if isinstance(v, (int, float)) else None
    def total(keys):
        vals = [f(k) for k in keys if f(k) is not None]
        return sum(vals) if vals else None
    def pause(ctrl, prios):
        if f(ctrl) is not None:
            return f(ctrl)
        vals = [float(v) for v in roce.get(prios) or [] if isinstance(v, (int, float))]
        return sum(vals) if vals else None
    names = {"ecn": "ecnMarked", "cnpSent": "cnpSent", "cnpHandled": "cnpHandled", "oos": "outOfSequence",
             "seqErr": "packetSeqErr", "timeouts": "localAckTimeoutErr"}
    if metric in names:
        return f(names[metric])
    if metric == "pauseRx":
        return pause("rxPauseCtrl", "rxPause")
    if metric == "pauseTx":
        return pause("txPauseCtrl", "txPause")
    if metric == "discards":
        return total(("rxDiscardsPhy", "portXmitDiscards"))
    if metric == "crc":
        return total(("rxCrcErrorsPhy", "rxIcrcErr"))
    return f(metric)


def memory_available_gib(frame):
    mem = frame.get("mem") or {}
    total, used, available = (mem.get(k) for k in ("total", "used", "available"))
    if not all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in (total, used, available)):
        return None
    if total <= 0 or not (0 <= available <= total and 0 <= used <= total) or abs(used - (total - available)) > 1048576:
        return None
    return available / 1073741824.0


def memory_reserved(frame):
    mem, gpu = frame.get("mem") or {}, frame.get("gpu") or {}
    total, used, measured = mem.get("total", 0), mem.get("used", 0), mem.get("gpuAlloc", 0)
    if gpu.get("readable") is False or gpu.get("error") or not isinstance(measured, (int, float)) or not math.isfinite(measured) or not 0 <= measured <= total:
        measured = 0
    declared = 0
    for c in frame.get("containers") or []:
        l = c.get("labels") or {}
        if c.get("state") != "running" or l.get("sparkbox.instance") not in ("release", "dev") or not l.get("sparkbox.deployment") or not l.get("sparkbox.path") or l.get("sparkbox.role") not in ("single", "head", "worker"):
            continue
        try:
            share = float(l.get("sparkbox.gpu-share", ""))
        except (ValueError, TypeError):
            continue
        if math.isfinite(share) and 0 < share < 1:
            declared += share * total
    return min(max(measured, min(int(declared), total)), used)


def memory_used_pct(frame, headroom_enabled):
    mem = frame.get("mem") or {}
    pool = memory_reserved(frame) if headroom_enabled and memory_available_gib(frame) is not None else 0
    return pct(max(mem.get("used", 0) - pool, 0), mem.get("total", 0)), pool


def node_metric(frame, metric):
    """A node-level value by rule metric name (rules.rs node_metric), or None."""
    mem, gpu, cpu, sysinfo = frame.get("mem") or {}, frame.get("gpu") or {}, frame.get("cpu") or {}, frame.get("system") or {}
    if metric == "memPct":
        return pct(mem.get("used", 0), mem.get("total", 0))
    if metric == "diskPct":
        d = root_disk(frame)
        return pct(d.get("used", 0), d.get("total", 0)) if d else None
    if metric == "memAvailableGiB":
        return memory_available_gib(frame)
    if metric in ("memPsiFull", "swapInMiBs"):
        value = mem.get("psiFull10" if metric == "memPsiFull" else "swapInBps")
        if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value) or value < 0:
            return None
        return float(value) if metric == "memPsiFull" else value / 1048576.0
    if metric == "swapPct":
        return pct(mem.get("swapUsed", 0), mem.get("swapTotal", 0)) if mem.get("swapTotal") else None
    if metric == "load1":
        return (cpu.get("load") or [None])[0]
    simple = {"gpuTemp": gpu.get("tempC"), "gpuUtil": gpu.get("utilPct"), "gpuPowerW": gpu.get("powerW"),
              "cpuPct": cpu.get("totalPct"), "cpuTempC": cpu.get("tempC"), "clockOffsetMs": sysinfo.get("clockOffsetMs")}
    v = simple.get(metric)
    return float(v) if isinstance(v, (int, float)) else None


def net_metric(n, metric):
    if metric == "rxBps":
        return n.get("rxBps")
    if metric == "txBps":
        return n.get("txBps")
    if metric == "ifErrors":
        return float((n.get("rxErrors") or 0) + (n.get("txErrors") or 0))
    if metric == "rxDrops":
        return float(n.get("rxDrops") or 0)
    return roce_value(n.get("roce"), metric)


def container_metric(c, metric):
    return {"cpu": c.get("cpuPct"), "mem": c.get("mem"), "restarts": c.get("restartCount")}.get(metric)


# ---- History (SQLite)

class History:
    """Raw 10 s buckets for 48 h, 1 min for 14 days, 10 min for 180 days, each with
    min/avg/max. Readings are averaged into the current 10 s bucket in memory and
    written when it closes; the coarser tiers are updated as each bucket is written.

    The file uses incremental auto-vacuum: space freed by pruning goes back a little
    every hour (PRAGMA incremental_vacuum), never in one long full VACUUM."""

    def __init__(self, path, cap=HISTORY_CAP, readonly=False):
        self.path, self.cap = path, cap
        self.lock = threading.Lock()
        self.bucket = None
        self.acc = {}
        self.note = None
        if readonly:
            # `export` runs as the login user, who may read the file but not write it.
            uri = "file:%s?%s" % (urllib.parse.quote(path), "immutable=1" if readonly == "immutable" else "mode=ro")
            self.db = sqlite3.connect(uri, uri=True, timeout=10, check_same_thread=False)
            return
        fresh = not os.path.exists(path) or os.path.getsize(path) == 0
        self.db = sqlite3.connect(path, timeout=10, check_same_thread=False)
        if fresh:
            self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
        elif self.db.execute("PRAGMA auto_vacuum").fetchone()[0] != 2:
            self.migrate()
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=NORMAL")
        self.db.execute("CREATE TABLE IF NOT EXISTS points (tier INTEGER NOT NULL, entity TEXT NOT NULL, metric TEXT NOT NULL, "
                        "ts INTEGER NOT NULL, avg REAL, min REAL, max REAL, n INTEGER NOT NULL DEFAULT 1, "
                        "PRIMARY KEY (tier, entity, metric, ts)) WITHOUT ROWID")
        self.db.execute("CREATE INDEX IF NOT EXISTS points_ts ON points (tier, ts)")
        self.db.commit()

    def migrate(self):
        """An r2 file (no auto-vacuum): switching needs one full VACUUM, done at start while
        the file is small. A bigger one waits for a restart when pruning has shrunk it."""
        size = os.path.getsize(self.path)
        if size >= MIGRATE_UNDER:
            self.note = "history.db is %d MB, so incremental vacuum starts at a restart once it's under %d MB" % (
                size // 1048576, MIGRATE_UNDER // 1048576)
            log(self.note)
            return
        self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
        self.db.execute("VACUUM")

    def auto_vacuum(self):
        with self.lock:
            return {0: "none", 1: "full", 2: "incremental"}.get(self.db.execute("PRAGMA auto_vacuum").fetchone()[0])

    def add(self, ts_ms, entity, values):
        """One reading of several metrics; None and NaN are skipped (an outage stays a gap)."""
        b = ts_ms // 10_000 * 10_000
        if self.bucket is not None and b != self.bucket:
            self.flush()
        self.bucket = b
        for m, v in values.items():
            if v is None or isinstance(v, bool) or (isinstance(v, float) and math.isnan(v)):
                continue
            a = self.acc.setdefault((entity, m), [0.0, 0, math.inf, -math.inf])
            a[0] += v
            a[1] += 1
            a[2] = min(a[2], v)
            a[3] = max(a[3], v)

    def flush(self):
        if self.bucket is None or not self.acc:
            self.acc = {}
            return
        rows = [(e, m, a[0] / a[1], a[2], a[3]) for (e, m), a in self.acc.items()]
        with self.lock:
            cur = self.db.cursor()
            cur.executemany("INSERT OR REPLACE INTO points VALUES (0, ?, ?, ?, ?, ?, ?, 1)",
                            [(e, m, self.bucket, avg, lo, hi) for e, m, avg, lo, hi in rows])
            for tier, step, _ in TIERS[1:]:
                ts = self.bucket // (step * 1000) * step * 1000
                cur.executemany("INSERT INTO points VALUES (?, ?, ?, ?, ?, ?, ?, 1) ON CONFLICT (tier, entity, metric, ts) DO UPDATE SET "
                                "avg = (avg * n + excluded.avg) / (n + 1), min = MIN(min, excluded.min), "
                                "max = MAX(max, excluded.max), n = n + 1",
                                [(tier, e, m, ts, avg, lo, hi) for e, m, avg, lo, hi in rows])
            self.db.commit()
        self.acc = {}

    def prune(self, now=None):
        now = now or now_ms()
        with self.lock:
            for tier, _, keep in TIERS:
                self.db.execute("DELETE FROM points WHERE tier = ? AND ts < ?", (tier, now - keep * 1000))
            self.db.commit()

    def size(self):
        with self.lock:
            pages = self.db.execute("PRAGMA page_count").fetchone()[0]
            page = self.db.execute("PRAGMA page_size").fetchone()[0]
            free = self.db.execute("PRAGMA freelist_count").fetchone()[0]
        return (pages - free) * page

    def enforce_cap(self, beat=None):
        """Over the cap: drop the oldest raw data first, then the oldest minutes, then the
        oldest 10-minute buckets, a tenth at a time (`beat` is called between steps, so a
        long trim never looks like a wedged agent)."""
        if self.size() <= self.cap:
            return False
        for tier, _, _ in TIERS:
            for _ in range(10):
                if self.size() <= self.cap:
                    break
                with self.lock:
                    lo, hi = self.db.execute("SELECT MIN(ts), MAX(ts) FROM points WHERE tier = ?", (tier,)).fetchone()
                    if lo is None:
                        break
                    cut = lo + max((hi - lo) // 10, 10_000)
                    self.db.execute("DELETE FROM points WHERE tier = ? AND ts < ?", (tier, cut))
                    self.db.commit()
                if beat:
                    beat()
            if self.size() <= self.cap:
                break
        self.vacuum()
        return True

    def vacuum(self, pages=2000):
        """Give up to `pages` free pages back to the file system (a no-op on an r2 file
        that hasn't moved to incremental vacuum yet)."""
        with self.lock:
            self.db.execute("PRAGMA incremental_vacuum(%d)" % int(pages)).fetchall()

    def span(self):
        """The oldest sample held: one index lookup per tier (a beat asks every cycle)."""
        with self.lock:
            firsts = [self.db.execute("SELECT MIN(ts) FROM points WHERE tier = ?", (tier,)).fetchone()[0] for tier, _, _ in TIERS]
        firsts = [t for t in firsts if t is not None]
        return min(firsts) if firsts else None

    def export(self, since, until=None, entity=None, now=None):
        """Points from `since` to `until`, finest tier first: raw where raw still exists,
        then minutes, then 10 minutes. Yields {"e", "t", "s", "v": {metric: avg}}."""
        now = now or now_ms()
        until = until or now
        covered_from = until + 1
        with self.lock:
            for tier, step, keep in TIERS:
                lo = max(since, now - keep * 1000)
                hi = min(until, covered_from - 1)
                if hi < lo:
                    continue
                q = "SELECT entity, ts, metric, avg FROM points WHERE tier = ? AND ts >= ? AND ts <= ?"
                args = [tier, lo, hi]
                if entity:
                    q += " AND entity = ?"
                    args.append(entity)
                rows = self.db.execute(q + " ORDER BY entity, ts", args).fetchall()
                first = None
                group, key = {}, None
                for e, ts, m, avg in rows:
                    if (e, ts) != key:
                        if key:
                            yield {"e": key[0], "t": key[1], "s": step, "v": group}
                        key, group = (e, ts), {}
                    group[m] = avg
                    first = ts if first is None else min(first, ts)
                if key:
                    yield {"e": key[0], "t": key[1], "s": step, "v": group}
                if first is not None:
                    covered_from = min(covered_from, first)


# ---- Rules

def shown(v):
    """Percent rules are judged on the whole percent the app shows (half away from zero, like Rust)."""
    return math.floor(v + 0.5) if v >= 0 else -math.floor(-v + 0.5)


def past(op, v, line):
    return v > line if op == ">" else v < line if op == "<" else abs(v) > line


def reached(op, v, line):
    return v >= line if op == ">" else v <= line if op == "<" else abs(v) >= line


class Th:
    """A rule's lines and durations for this node, overrides applied (rule.rs RuleSet::th)."""

    def __init__(self, rule, node):
        o = (rule.get("overrides") or {}).get(node) or {}
        self.enabled = o.get("enabled", rule.get("enabled", True)) is not False
        self.op = rule.get("op", ">")
        self.warn = o.get("warn", rule.get("warn"))
        self.crit = o.get("crit", rule.get("crit"))
        self.clear = rule.get("clear")
        self.for_ms = int(rule.get("forS", 0)) * 1000
        self.crit_for_ms = int(rule.get("critForS") if rule.get("critForS") is not None else rule.get("forS", 0)) * 1000
        self.clear_ms = int(rule.get("clearS", 0)) * 1000
        self.window_ms = int(rule.get("windowS") or 0) * 1000

    def judge(self, v, raw, inclusive, active):
        hit = (lambda line: reached(self.op, v, line)) if inclusive else (lambda line: past(self.op, v, line))
        if self.crit is not None and hit(self.crit):
            return "crit"
        if self.warn is not None and hit(self.warn):
            return "warn"
        line = self.clear if self.clear is not None else (self.warn if self.warn is not None else self.crit)
        if active and line is not None and reached(self.op, raw, line):
            return "warn" if self.warn is not None else "crit"
        return None


def fmt_value(v, unit):
    if unit == "%":
        return "%d%%" % shown(v)
    if unit == "°C":
        return "%d °C" % shown(v)
    if unit == "ms":
        return "%.1f s" % (v / 1000) if abs(v) >= 1000 else "%d ms" % shown(v)
    if unit == "s":
        return "%.1f s" % v
    if unit == "dBm":
        return "%.1f dBm" % v
    if unit in ("GiB", "MiB/s"):
        return "%.2f %s" % (v, unit)
    if unit == "/s":
        # Real precision for slow rates ("0.004/s"), never a rounded "0.0/s".
        return "%.3f/s" % v if 0 < abs(v) < 0.1 else "%.1f/s" % v
    return "%d" % v if float(v).is_integer() else "%.1f" % v


def short_label(ent):
    # "enP2p1s0f1np1 on cell2" -> "cell2 enP2p1s0f1np1": led by the node when that's where it is (as rules.rs does).
    what, sep, place = ent["label"].partition(" on ")
    if sep and place == ent.get("node"):
        return "%s %s" % (place, what)
    return what


def first_lower(s):
    # "Unified memory" -> "unified memory"; a first word like "RoCE", "CRC" or "GPU" keeps its capitals (as rules.rs does).
    word = (s.split() or [""])[0]
    if any(ch.isupper() for ch in word[1:]):
        return s
    return s[:1].lower() + s[1:]


class Cond:
    def __init__(self, rule, severity, entity, message, summary, for_ms, clear_ms, value=None, category="node", detail=None):
        self.rule, self.severity, self.entity = rule, severity, entity
        self.message, self.summary, self.for_ms, self.clear_ms = message, summary, for_ms, clear_ms
        self.value, self.category, self.detail = value, category, detail
        self.incident = None  # set by evaluate() (gate.Roots)
        self.root = None      # an incident given outright (a node's fabric-lost under the switch)
        e = entity
        if e.get("kind") == "node":
            self.key = "%s:node:%s" % (rule, e["id"])
        else:
            self.key = "%s:%s:%s:%s" % (rule, e["kind"], e.get("node") or "", e["id"])


class Trackers:
    """Counters between samples, as rules.rs Trackers: rate windows, flaps, restarts."""

    def __init__(self):
        self.series = {}   # key -> deque[(ms, value)]
        self.known_ip = {}
        self.roce = {}     # interface -> RoCE counter history (roce.py judge)
        self.last_restart = {}  # restarts key -> ms of the last restart seen

    def push(self, key, now, value, window_ms):
        q = self.series.setdefault(key, collections.deque())
        if q and value < q[-1][1]:
            q.clear()  # a counter that went back was reset
        q.append((now, value))
        while len(q) > 2 and now - q[0][0] > window_ms:
            q.popleft()

    def growth(self, key, per_second=False):
        q = self.series.get(key)
        if not q or q[-1][0] <= q[0][0]:
            return None
        delta = q[-1][1] - q[0][1]
        return delta / ((q[-1][0] - q[0][0]) / 1000.0) if per_second else delta

    def restarts(self, key, now, c, window_ms):
        """A container's restarts within the window (its restartCount rising), noting the last
        restart seen: a sample while it's restarting, or one where the count rose."""
        q = self.series.get(key)
        count = float(c.get("restartCount") or 0)
        if c.get("state") == "restarting" or (q and count > q[-1][1]):
            self.last_restart[key] = now
        self.push(key, now, count, window_ms)
        return self.changes(key)

    def changes(self, key):
        """How many times a counter rose within its window (carrier changes: flaps)."""
        q = self.series.get(key)
        if not q:
            return 0
        return sum(1 for a, b in zip(q, list(q)[1:]) if b[1] > a[1])


def node_entity(cfg):
    return {"kind": "node", "id": cfg.node, "node": cfg.node, "switch": None, "label": cfg.name}


def on_node(cfg, kind, ident):
    return {"kind": kind, "id": ident, "node": cfg.node, "switch": None, "label": "%s on %s" % (ident, cfg.name)}


def peer_entity(p):
    return {"kind": "node", "id": p.id, "node": p.id, "switch": None, "label": p.name}


class Conditions(list):
    """Unknown memory readings hold firing alerts, but cannot confirm a pending one."""
    def __init__(self):
        super().__init__()
        self.frozen = set()


def evaluate(frame, rules, cfg, trackers, active, now, findings=None, endpoints=None, view=None):
    """Every rule this agent evaluates over one frame (rules.rs evaluate, node-local part),
    plus what the peer watch found (`findings`, from peers.PeerWatch.judge and the
    maintenance check) and the latest probe of each endpoint this node serves. `active`: the
    engine's alerts (their keys at least); `view`: {seen: [names], minority} from the watch."""
    by_id = {r.get("id"): r for r in rules}
    out = Conditions()

    def th(rid):
        r = by_id.get(rid)
        if r is None and rid in CODED_DEFAULTS:
            r = dict(CODED_DEFAULTS[rid], id=rid)
        return Th(r, cfg.node) if r else None

    def cat(rid):
        return CATEGORY.get((by_id.get(rid) or CODED_DEFAULTS.get(rid) or {}).get("area"), "node")

    for rid, metric in (("mem-available", "memAvailableGiB"), ("mem-pressure", "memPsiFull"), ("mem-swap-in", "swapInMiBs")):
        t = th(rid)
        if t and t.enabled and node_metric(frame, metric) is None:
            out.frozen.add("%s:node:%s" % (rid, cfg.node))

    # Threshold rules with messages of their own.
    t = th("mem-high")
    if t and t.enabled:
        headroom = th("mem-available")
        p, pool = memory_used_pct(frame, bool(headroom and headroom.enabled))
        sev = t.judge(shown(p), p, True, "mem-high:node:%s" % cfg.node in active)
        if sev:
            out.append(Cond("mem-high", sev, node_entity(cfg), "Unified memory %d%% used%s" % (shown(p), " outside GPU reservation" if pool else ""), "%s memory %d%%" % (cfg.name, shown(p)),
                            t.crit_for_ms if sev == "crit" else t.for_ms, t.clear_ms, p, "node"))
    d = root_disk(frame)
    t = th("disk-full")
    if t and t.enabled and d:
        p = pct(d.get("used", 0), d.get("total", 0))
        ent = on_node(cfg, "disk", d.get("mount", "/"))
        if d.get("mount") == "/":
            ent["label"] = "Root disk on %s" % cfg.name
        sev = t.judge(shown(p), p, True, "disk-full:disk:%s:%s" % (cfg.node, d.get("mount")) in active)
        if sev:
            out.append(Cond("disk-full", sev, ent, "Root disk %d%% full" % shown(p), "%s disk %d%%" % (cfg.name, shown(p)), t.for_ms, t.clear_ms, p, "disk"))
    gpu = frame.get("gpu") or {}
    gpu_ent = {"kind": "gpu", "id": "gpu0", "node": cfg.node, "switch": None, "label": "GPU on %s" % cfg.name}
    t = th("gpu-hot")
    if t and t.enabled and isinstance(gpu.get("tempC"), (int, float)):
        temp = float(gpu["tempC"])
        sev = t.judge(temp, temp, False, "gpu-hot:gpu:%s:gpu0" % cfg.node in active)
        if sev:
            out.append(Cond("gpu-hot", sev, gpu_ent, "GPU at %d °C" % shown(temp), "%s GPU %d °C" % (cfg.name, shown(temp)), t.for_ms, t.clear_ms, temp, "gpu"))
    t = th("gpu-throttle")
    verdict = throttle_verdict(gpu)
    if t and t.enabled and verdict:
        out.append(Cond("gpu-throttle", "warn", gpu_ent, verdict[1], "%s GPU throttled" % cfg.name, t.for_ms, t.clear_ms, gpu.get("utilPct"), "gpu"))
    # The GPU stopped answering (nvidia-smi or NVML hung or failing): its readings are null.
    t = th("gpu-unreadable")
    unreadable = gpu.get("error") or (not any(isinstance(gpu.get(k), (int, float)) for k in ("utilPct", "tempC", "powerW")) and "GPU readings missing")
    if t and t.enabled and unreadable:
        out.append(Cond("gpu-unreadable", "crit", gpu_ent, "Can't read the GPU: %s" % unreadable, "%s GPU isn't answering" % cfg.name,
                        t.for_ms, t.clear_ms, None, "gpu"))
    t = th("ntp-unsynced")
    if t and t.enabled and (frame.get("system") or {}).get("ntpSynced") is False:
        out.append(Cond("ntp-unsynced", "warn", node_entity(cfg), "Clock not synchronized with NTP", "%s clock not synchronized" % cfg.name, t.for_ms, t.clear_ms))

    # Fabric links.
    down, flap, errs, mtu = th("fabric-down"), th("fabric-flap"), th("net-errors"), th("fabric-mtu")
    for n in frame.get("nets") or []:
        iface = n.get("iface")
        if n.get("ip"):
            trackers.known_ip[iface] = n["ip"]
        ent = on_node(cfg, "interface", iface)
        fabric = n.get("role") == "fabric"
        if fabric and down and down.enabled and not n.get("up") and (n.get("ip") or trackers.known_ip.get(iface)):
            ip = n.get("ip") or trackers.known_ip.get(iface)
            if (view or {}).get("switch"):
                # Every node lost its fabric links with it: one switch page, this link its child.
                c = Cond("fabric-down", "warn", ent, "Fabric link %s down (%s): %s is down" % (iface, ip, (cfg.switch or {}).get("name") or "the switch"),
                         "%s down on %s" % (iface, cfg.name), max(down.for_ms, int(peers.SWITCH_GRACE_S * 1000)), down.clear_ms, None, "fabric")
                c.root = "switch:%s" % (cfg.switch or {}).get("id", "fabric")
                out.append(c)
            else:
                # Every fabric link here down at once may be the switch: give the others' views time.
                wait = int(peers.SWITCH_GRACE_S * 1000) if fabric_links_down(frame, trackers) else 0
                out.append(Cond("fabric-down", "crit", ent, "Fabric link %s down (%s)" % (iface, ip), "%s down on %s" % (iface, cfg.name),
                                max(down.for_ms, wait), down.clear_ms, None, "fabric"))
        if fabric and flap and n.get("carrierChanges") is not None:
            key = "flap|%s" % iface
            trackers.push(key, now, float(n["carrierChanges"]), flap.window_ms or 300_000)
            times = trackers.changes(key)
            if flap.enabled and flap.judge(times, times, False, False):
                mins = max((flap.window_ms or 300_000) // 60_000, 1)
                msg = ("%s dropped and came back in the last %d min" % (iface, mins)) if times == 1 else ("%s flapped %d times in %d min" % (iface, times, mins))
                out.append(Cond("fabric-flap", "warn", ent, msg, "%s flapping on %s" % (iface, cfg.name), flap.for_ms, flap.clear_ms, times, "fabric"))
        if errs and n.get("role") != "wifi":
            key = "errors|%s" % iface
            trackers.push(key, now, float((n.get("rxErrors") or 0) + (n.get("txErrors") or 0)), errs.window_ms or 120_000)
            delta = trackers.growth(key) or 0
            if errs.enabled and errs.judge(delta, delta, False, False):
                mins = max((errs.window_ms or 120_000) // 60_000, 1)
                out.append(Cond("net-errors", "warn", ent, "%s errors rising (+%d in the last %d min)" % (iface, delta, mins),
                                "%s errors on %s" % (iface, cfg.name), errs.for_ms, errs.clear_ms, delta, "fabric"))
        if fabric and mtu and mtu.enabled and n.get("up") and n.get("ip") and n.get("mtu") and mtu.warn is not None and n["mtu"] != int(mtu.warn):
            out.append(Cond("fabric-mtu", "warn", ent, "%s MTU %d, expected %d" % (iface, n["mtu"], int(mtu.warn)),
                            "%s MTU %d on %s" % (iface, n["mtu"], cfg.name), mtu.for_ms, mtu.clear_ms, n["mtu"], "fabric"))

    # RoCE on each fabric interface: one alert per interface, the worst counter's severity,
    # what the rising counters mean (roce.py judge; the app's roce_rules.rs runs the same
    # table). A rules.json from before it still has the per-counter rules: those run instead.
    t = th("roce-iface")
    if t and t.enabled and roce_iface is not None and roce_on(by_id):
        lines = (by_id.get("roce-iface") or {}).get("counters")
        addressed = [n["iface"] for n in frame.get("nets") or [] if n.get("role") == "fabric" and n.get("ip")]
        for n in frame.get("nets") or []:
            if n.get("role") != "fabric" or not isinstance(n.get("roce"), dict):
                continue  # no counters: no data, so a raised alert resolves after its clear time
            rates = trackers.roce.setdefault(n["iface"], roce_iface.IfaceRates())
            rates.add(now, n["roce"])
            j = roce_iface.judge(lines, rates.rates(now), rates.storm_s(now), "roce-iface:interface:%s:%s" % (cfg.node, n["iface"]) in active)
            if j:
                sev, msg, _, detail, value = j
                label = "%s half %d" % (cfg.name, roce_iface.half(n["iface"], n.get("ip"), addressed))
                ent = {"kind": "interface", "id": n["iface"], "node": cfg.node, "switch": None, "label": label}
                out.append(Cond("roce-iface", sev, ent, msg, "%s: %s" % (label, first_lower(msg)), t.for_ms, t.clear_ms, value, "fabric", detail))

    # Containers and units. A container has settled once it has run CRASH_RECENT_S or its model
    # answers (as rules.rs decides); a raised crash loop holds, never easing, until
    # CRASH_WINDOW_S after the last restart, so one loop is one raise and one resolve
    # (fixtures/docker-states.json).
    loop, exited = th("crash-loop"), th("container-exited")
    now_s = now // 1000
    vouched = answering_containers(endpoints)
    for c in frame.get("containers") or []:
        ent = on_node(cfg, "container", c.get("name"))
        if loop:
            key = "restarts|%s" % c.get("name")
            restarts = trackers.restarts(key, now, c, loop.window_ms or cfg.crash_window_s * 1000)
            running = c.get("state") == "running"
            up_s = now_s - c["startedAt"] if running and (c.get("startedAt") or 0) > 0 else 0
            settled = running and (up_s >= cfg.crash_recent_s or c.get("name") in vouched)
            held = firing_severity(active, "crash-loop:container:%s:%s" % (cfg.node, c.get("name")))
            last = trackers.last_restart.get(key)
            hold = held is not None and last is not None and now - last < cfg.crash_window_s * 1000
            if loop.enabled and (not settled or hold):
                sev = "crit" if c.get("state") == "restarting" else loop.judge(restarts, restarts, False, False)
                if hold and gate.RANK.get(held, 0) > gate.RANK.get(sev, 0):
                    sev = held
                if sev == "crit":
                    msg = ("%s is up again after %d restart%s; it clears %d min after the last one" % (c["name"], restarts, "" if restarts == 1 else "s", cfg.crash_window_s // 60)
                           if settled else "%s crash loop: Docker restarted it %d times in %d min" % (c["name"], restarts, cfg.crash_window_s // 60))
                    out.append(Cond("crash-loop", "crit", ent, msg, "%s crash-looping on %s" % (c["name"], cfg.name), loop.for_ms, 0, restarts, "containers"))
                elif sev:
                    out.append(Cond("crash-loop", "warn", ent, "%s crashed and Docker restarted it" % c["name"],
                                    "%s restarted on %s" % (c["name"], cfg.name), loop.for_ms, 0, restarts, "containers"))
        if exited and exited.enabled and c.get("state") in ("exited", "dead") and (c.get("exitCode") or c.get("oomKilled")):
            fin = c.get("finishedAt") or 0
            if fin == 0 or now_s - fin < (exited.window_ms or 86_400_000) // 1000:
                sev = "crit" if c.get("oomKilled") else "warn"
                msg = "%s exited %d%s" % (c["name"], c.get("exitCode") or 0, " · OOM-killed" if c.get("oomKilled") else "")
                out.append(Cond("container-exited", sev, ent, msg, "%s exited on %s" % (c["name"], cfg.name), exited.for_ms, exited.clear_ms, c.get("exitCode"), "containers"))
    t = th("unit-failed")
    if t and t.enabled:
        for unit in frame.get("failedUnits") or []:
            short = unit[:-8] if unit.endswith(".service") else unit
            out.append(Cond("unit-failed", "warn", on_node(cfg, "service", unit), "%s failed" % unit, "%s failed on %s" % (short, cfg.name), 0, 0, None, "containers"))

    # Kernel events: each stays an alert for its rule's window after the latest event. A
    # fatal Xid (the GPU needs a reset) stays until the node reboots: the collector reads
    # this boot's kernel log, so a new boot starts without it.
    latest = {}
    for e in frame.get("kernelEvents") or []:
        rid = {"xid": "kernel-xid", "mlx5": "kernel-mlx5", "oom": "kernel-oom", "hung": "kernel-hung"}.get(e.get("kind"))
        t = th(rid) if rid else None
        fatal = e.get("kind") == "xid" and e.get("code") in collector.FATAL_XID
        if not t or not t.enabled or (not fatal and now - (e.get("tsMs") or 0) > max(t.window_ms, 60_000)):
            continue
        who = e.get("process") or "a process"
        if e["kind"] == "xid":
            code = e.get("code") or 0
            name = collector.XID.get(code, ("", "an Xid this table doesn't know"))[1]
            ent = {"kind": "gpu", "id": "xid-%d" % code, "node": cfg.node, "switch": None, "label": "GPU on %s" % cfg.name}
            msg, summary = "Xid %d · %s" % (code, name), "%s GPU Xid %d" % (cfg.name, code)
            if fatal:
                msg += " · until the node restarts"
        elif e["kind"] == "oom":
            ent, msg, summary = node_entity(cfg), "Out of memory: the kernel killed %s" % who, "%s OOM-killed on %s" % (who, cfg.name)
        elif e["kind"] == "hung":
            ent, msg, summary = node_entity(cfg), "%s blocked for more than 2 minutes" % who, "Hung task on %s" % cfg.name
        else:
            iface = next((n["iface"] for n in frame.get("nets") or [] if n.get("iface") and n["iface"] in e.get("text", "")), "mlx5")
            ent = on_node(cfg, "interface", iface)
            msg, summary = "mlx5_core: %s" % e.get("text", "").rsplit(": ", 1)[-1][:120], "ConnectX-7 error on %s" % cfg.name
        c = Cond(rid, e.get("severity", "warn"), ent, msg, summary, 0, 0, e.get("code"), cat(rid))
        prev = latest.get(c.key)
        if not prev or e.get("tsMs", 0) >= prev[1] or c.severity == "crit":
            latest[c.key] = (c, e.get("tsMs", 0))
    out.extend(c for c, _ in sorted(latest.values(), key=lambda x: x[0].key))

    # The models this node serves: serving_probe's verdict (Down after two failed probes or
    # one-token checks, an unhealthy engine, or requests running with no tokens for the
    # endpoint-stalled rule's time); Ready again clears it at once.
    stalled_th, down_th = th("endpoint-stalled"), th("endpoint-down")
    stall_s = stall_after(by_id)
    for eid, r in sorted((endpoints or {}).items()):
        res, ep = r.get("result") or {}, r.get("endpoint") or {}
        ent = {"kind": "endpoint", "id": eid, "node": cfg.node, "switch": None, "label": ep.get("name") or eid}
        stalled = (res.get("stalledS") or 0) >= stall_s and (res.get("running") or 0) > 0
        if down_th and down_th.enabled and res.get("health") == "down":
            out.append(Cond("endpoint-down", "crit", ent, res.get("error") or "Not answering", "%s down" % ent["label"],
                            down_th.for_ms, 0, None, "serving"))
        if stalled_th and stalled_th.enabled and stalled:
            n = int(round(res.get("running") or 0))
            out.append(Cond("endpoint-stalled", "crit", ent, "%d request%s running, no tokens for %d s" % (n, "" if n == 1 else "s", res["stalledS"]),
                            "%s stalled" % ent["label"], 0, 0, res.get("stalledS"), "serving"))

    # Generic threshold and rate rules on node, fabric-interface, container and endpoint metrics.
    roce_rules = roce_on(by_id) and roce_iface is not None
    for r in rules:
        rid = r.get("id")
        if rid in EVALUATES or r.get("kind") not in ("threshold", "rate") or r.get("entity") not in GENERIC_ENTITIES:
            continue
        if roce_rules and rid in ROCE_R2:
            continue  # folded into roce-iface
        t = Th(r, cfg.node)
        readings = []
        if r["entity"] == "node":
            v = node_metric(frame, r.get("metric"))
            if v is not None:
                readings.append((node_entity(cfg), v))
        elif r["entity"] == "net":
            for n in frame.get("nets") or []:
                if n.get("role") == "fabric":
                    v = net_metric(n, r.get("metric"))
                    if v is not None:
                        readings.append((on_node(cfg, "interface", n["iface"]), float(v)))
        elif r["entity"] == "endpoint":
            for eid, res in sorted((endpoints or {}).items()):
                v = (res.get("result") or {}).get(r.get("metric"))
                if isinstance(v, (int, float)) and not isinstance(v, bool):
                    label = (res.get("endpoint") or {}).get("name") or eid
                    readings.append(({"kind": "endpoint", "id": eid, "node": cfg.node, "switch": None, "label": label}, float(v)))
        else:
            for c in frame.get("containers") or []:
                v = container_metric(c, r.get("metric"))
                if v is not None:
                    readings.append((on_node(cfg, "container", c["name"]), float(v)))
        for ent, v in readings:
            key = "%s|%s:%s" % (rid, ent["kind"], ent["id"])
            if r["kind"] == "rate":
                trackers.push(key, now, v, max(t.window_ms, 1000))
                v = trackers.growth(key, per_second=r.get("unit") == "/s")
                if v is None:
                    continue
            if not t.enabled:
                continue
            probe = Cond(rid, "warn", ent, "", "", 0, 0)
            inclusive = r.get("unit") == "%"
            sev = t.judge(shown(v) if inclusive else v, v, inclusive, probe.key in active)
            if not sev:
                continue
            if r.get("notify") is False:
                sev = "info"
            line = t.crit if sev == "crit" else t.warn
            value = fmt_value(v, r.get("unit", ""))
            msg = (r.get("message") or "%s {value}" % r.get("title", rid)).replace("{value}", value).replace(
                "{threshold}", fmt_value(line, r.get("unit", "")) if line is not None else "")
            out.append(Cond(rid, sev, ent, msg, "%s %s %s" % (short_label(ent), first_lower(r.get("title", rid)), value),
                            t.crit_for_ms if sev == "crit" else t.for_ms, t.clear_ms, v, CATEGORY.get(r.get("area"), "node"), r.get("help") or None))

    # What the agents see of each other: the leader's findings about peers, and this
    # node's own (lost the fabric, cut off). A node that lost the fabric waits SWITCH_GRACE_S
    # before that fires: when the others lost it too, it's the switch, and this node's alert
    # becomes a warning in the switch's incident (the one switch page names every node).
    sw = cfg.switch or {"id": "fabric", "name": "The fabric switch"}
    names = {p["id"]: p.get("name") or p["id"] for p in cfg.nodes}
    names[cfg.node] = cfg.name
    grace = int(peers.SWITCH_GRACE_S * 1000)
    for rule, sev, who, why in findings or []:
        t = th(rule)
        if t and not t.enabled:
            continue
        hold = t.clear_ms if t else 0
        if rule == "node-reboot":
            out.append(Cond(rule, "crit", peer_entity(who), why, why, 0, 0, None, "node"))
        elif rule == "switch-unreachable":
            ent = {"kind": "switch", "id": who["id"], "node": None, "switch": who["id"], "label": who["name"]}
            out.append(Cond(rule, "crit", ent, "Not answering at %s (HTTPS, HTTP or ping)" % who["host"],
                            "%s isn't answering" % who["name"], 0, hold, None, "fabric"))
        elif rule == "fabric-switch-down":
            ent = {"kind": "switch", "id": sw["id"], "node": None, "switch": sw["id"], "label": sw["name"]}
            lost = why.get("nodes") or []
            every = len(lost) > 1 and len(lost) >= len(cfg.nodes)
            who_lost = "all %d nodes" % len(lost) if every else peers.words([names.get(n, n) for n in lost])
            at = gate.clock(why.get("sinceMs") or now, cfg.tz)
            out.append(Cond(rule, "crit", ent, "%s lost the fabric at %s; the LAN still answers" % ("Every node" if every else who_lost, at),
                            "%s isn't answering: %s lost the fabric at %s" % (sw["name"], who_lost, at), 0, hold, len(lost), "fabric"))
        elif who is None:  # this node
            if why == "network":
                out.append(Cond(rule, "crit", node_entity(cfg), "No other node and not the gateway answers",
                                "%s is cut off from the cluster" % cfg.name, 0, hold, None, "fabric"))
            elif why == "switch":
                c = Cond(rule, "warn", node_entity(cfg), "No node answers over the fabric, here or anywhere: %s, not this node" % sw["name"],
                         "%s lost the fabric" % cfg.name, max(t.for_ms if t else 0, grace), hold, None, "fabric")
                c.root = "switch:%s" % sw["id"]
                out.append(c)
            else:
                out.append(Cond(rule, "crit", node_entity(cfg), "No other node answers over the fabric; the LAN works",
                                "%s lost the fabric" % cfg.name, max(t.for_ms if t else 0, grace), hold, None, "fabric"))
        elif rule == "peer-unreachable":
            seen = (view or {}).get("seen") or []
            tail = " (seen from %s only)" % peers.words(seen) if (view or {}).get("minority") and seen else ""
            out.append(Cond(rule, "crit", peer_entity(who), "Nothing answers on the LAN or the fabric, not even SSH",
                            "%s isn't answering on any path%s" % (who.name, tail), 0, 0, None, "node"))
        elif rule == "agent-wedged":
            msg = ("Its agent says it's stuck: no check completed for over 30 s" if why == "wedged"
                   else "The node answers on SSH, but its agent doesn't")
            out.append(Cond(rule, "warn", peer_entity(who), msg, "The agent on %s isn't answering" % who.name, 0, hold, None, "node"))
        elif rule == "fabric-lost":
            out.append(Cond(rule, "warn", peer_entity(who), "Its agent answers over the LAN but on neither fabric address",
                            "%s isn't reachable over the fabric" % who.name, 0, hold, None, "fabric"))

    # Each alert's incident, the way the app roots it (gate.Roots, incident-roots.json).
    roots = gate.Roots(ifaces=[{"node": cfg.node, "iface": n["iface"], "ip": n.get("ip")} for n in frame.get("nets") or [] if n.get("role") == "fabric"],
                       endpoints=[dict(e, headNode=e.get("headNode") or cfg.node) for e in cfg.endpoints],
                       active=[{"rule": c.rule, "entity": c.entity, "severity": c.severity} for c in out])
    for c in out:
        c.incident = c.root or roots.root_of({"rule": c.rule, "entity": c.entity})
    return out


def fabric_links_down(frame, trackers=None):
    """Every fabric link of this node is down (each one that has or had an address), and it has
    at least one: what a fabric switch that lost power looks like from here."""
    known = trackers.known_ip if trackers is not None else {}
    links = [n for n in frame.get("nets") or [] if n.get("role") == "fabric" and (n.get("ip") or known.get(n.get("iface")))]
    return bool(links) and not any(n.get("up") for n in links)


def throttle_verdict(gpu):
    """(the reasons that count, the message) for one GPU reading, or None (health/rules.rs
    throttle_verdict): "GPU throttled: SW power cap at 98% busy, 1.9 of 3.0 GHz". An idle GB10
    at P8 and 4 W reports SW power cap every other reading: that never counts."""
    if gpu.get("readable") is False or not any(isinstance(gpu.get(k), (int, float)) for k in ("utilPct", "tempC", "powerW")):
        return None
    util, sm, power = float(gpu.get("utilPct") or 0), float(gpu.get("smClockMhz") or 0), float(gpu.get("powerW") or 0)
    top, limit = gpu.get("clockMaxMhz") or 0, gpu.get("powerLimitW") or 0
    clock_low = not top or sm < THROTTLE_CLOCK_BELOW * top
    busy = (util >= THROTTLE_BUSY_UTIL_PCT and clock_low) or (limit > 0 and power >= THROTTLE_POWER_AT * limit)
    reasons = [r for r in gpu.get("throttle") or [] if str(r).strip().lower() not in THROTTLE_NEVER
               and (str(r).strip().lower() in THROTTLE_ALWAYS or busy)]
    if not reasons:
        return None
    ghz = lambda mhz: "%.1f" % (mhz / 1000.0)  # noqa: E731
    clocks = "%s of %s GHz" % (ghz(sm), ghz(top)) if top else "%s GHz" % ghz(sm)
    return reasons, "GPU throttled: %s at %.0f%% busy, %s" % (", ".join(reasons), util, clocks)


def answering_containers(endpoints):
    """The containers whose model answers (its endpoint Ready), from the endpoints' members."""
    out = set()
    for r in (endpoints or {}).values():
        if (r.get("result") or {}).get("health") == "ready":
            out.update((r.get("endpoint") or {}).get("containers") or [])
    return out


def firing_severity(active, key):
    """The severity an alert is firing at, from the engine's alerts (None when it isn't, or when
    only their keys are known)."""
    a = active.get(key) if isinstance(active, dict) else None
    return a.get("severity") if isinstance(a, dict) and a.get("state") == "firing" else None


def merge_outages(messages, title):
    """Peers the leader finds dead together are one page: the messages that only raise
    peer-unreachable become one, titled title(node ids) ("cell1, cell2 and cell3 aren't
    answering on any path (seen from gx10-0000 only)")."""
    picked = [m for m in messages if m.get("kinds") and all(k == "raised" for k in m["kinds"])
              and all(str(p).startswith("peer-unreachable:node:") for p in m.get("parts") or [])]
    ids = [str(p).split(":", 2)[2] for m in picked for p in m.get("parts") or []]
    if len(ids) < 2:
        return messages
    first = picked[0]
    merged = dict(first, title=title(ids), body="Nothing answers on the LAN or the fabric, not even SSH.", lines=[],
                  severity="crit", state="firing", node=None, rule="peer-unreachable", alertId=None,
                  merged=[x for m in picked for x in ([m["messageId"]] if m is not first else []) + list(m.get("merged") or [])],
                  parts=[p for m in picked for p in m["parts"]], kinds=["raised"] * len(ids))
    return [merged] + [m for m in messages if all(m is not p for p in picked)]


def stall_after(by_id):
    """Seconds of running requests with no tokens before an endpoint counts as stalled."""
    r = by_id.get("endpoint-stalled") or CODED_DEFAULTS["endpoint-stalled"]
    return int(r.get("forS") or 45)


def roce_on(by_id):
    """roce-iface runs unless the rules are from before it (they list r2's per-counter rules)."""
    return "roce-iface" in by_id or not any(rid in ROCE_R2 for rid in by_id)


class Engine:
    """Alerts move pending → firing → resolved with the same timing as the app's engine
    (health/mod.rs Engine::step_with): a condition must hold for its rule's duration before
    it fires and stay gone for its clear time before it resolves. A critical eases to a
    warning only once it has stayed below the critical line for the clear time too, so it
    never flaps crit → warn → crit."""

    RANK = {"info": 0, "warn": 1, "crit": 2}

    def __init__(self, active=None):
        self.active = active or {}

    def step(self, conds, now):
        events, seen = [], set()
        frozen = getattr(conds, "frozen", set())
        rank = self.RANK
        for c in conds:
            seen.add(c.key)
            a = self.active.get(c.key)
            if a is None:
                a = self.active[c.key] = {"id": c.key, "rule": c.rule, "category": c.category, "severity": c.severity, "state": "pending",
                                          "entity": c.entity, "incident": c.incident, "message": c.message, "summary": c.summary, "detail": c.detail,
                                          "value": c.value, "sinceMs": now, "firingSinceMs": None, "resolvedMs": None,
                                          "ackedMs": None, "mutedUntilMs": None, "notify": c.severity != "info"}
            a["absentSince"] = None
            a.update({"message": c.message, "summary": c.summary, "detail": c.detail, "value": c.value, "incident": c.incident or a.get("incident")})
            a["entity"]["label"] = c.entity["label"]
            a["clearMs"] = c.clear_ms
            if a["state"] == "pending":
                a["severity"] = c.severity
                if now - a["sinceMs"] >= c.for_ms:
                    a["state"], a["firingSinceMs"] = "firing", now
                    events.append(self.event(now, "raised", a))
            elif rank[c.severity] > rank[a["severity"]]:
                a["severity"], a["easedSince"] = c.severity, None
                events.append(self.event(now, "escalated", a))
            elif rank[c.severity] < rank[a["severity"]]:
                # Below the line, but it only eases once it has stayed there for the clear time.
                if a.get("easedSince") is None:
                    a["easedSince"] = now
                if now - a["easedSince"] >= c.clear_ms:
                    a["severity"], a["easedSince"] = c.severity, None
                    events.append(self.event(now, "eased", a))
            else:
                a["easedSince"] = None
        for key in list(self.active):
            if key in seen:
                continue
            a = self.active[key]
            if key in frozen and a["state"] == "firing":
                a["absentSince"], a["easedSince"] = None, None
                continue
            if a["state"] == "pending":
                del self.active[key]
                continue
            if a.get("absentSince") is None:
                a["absentSince"] = now
            if now - a["absentSince"] >= a.get("clearMs", 0):
                del self.active[key]
                a["state"], a["resolvedMs"] = "resolved", now
                events.append(self.event(now, "resolved", a))
        return events

    @staticmethod
    def event(now, kind, a):
        alert = {k: v for k, v in a.items() if k not in ("absentSince", "clearMs", "easedSince")}
        return {"tsMs": now, "kind": kind, "alert": json.loads(json.dumps(alert))}

    def firing(self):
        return [a for a in self.active.values() if a["state"] == "firing"]


# ---- Delivery

class Failure(Exception):
    def __init__(self, message, retry):
        super().__init__(message)
        self.message, self.retry = message, retry


def http_failure(status, detail, secret_word):
    hints = {401: "check the %s" % secret_word, 403: "check the %s" % secret_word, 404: "check the address",
             413: "the message was too large", 429: "rate limited; it will retry"}
    hint = hints.get(status) or ("the server had a problem; it will retry" if status >= 500 else detail or "the server refused it")
    if detail and status in (400, 401, 403, 422):
        hint = "%s (%s)" % (hint, detail)
    return Failure("HTTP %d — %s" % (status, hint), status == 429 or status >= 500)


def post(url, body, headers, secret_word):
    req = urllib.request.Request(url, data=body, headers=headers, method="POST")
    host = urllib.parse.urlsplit(url).hostname or url
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        raw = e.read()
        e.close()
        try:
            v = json.loads(raw)
            detail = v.get("error") or (v.get("errors") or [None])[0]
        except (ValueError, AttributeError):
            detail = raw.decode("utf-8", "replace").strip()[:120] or None
        raise http_failure(e.code, detail, secret_word) from None
    except (urllib.error.URLError, OSError) as e:
        why = getattr(e, "reason", e)
        raise Failure("Couldn't reach %s: %s" % (host, why), True) from None


def ntfy_priority(msg):
    if msg["state"] == "test":
        return 2
    if msg["state"] == "resolved":
        return 3
    return {"crit": 5, "warn": 3}.get(msg["severity"], 2)


def pushover_priority(msg):
    if msg["state"] in ("test", "resolved"):
        return -1
    return {"crit": 1, "warn": 0}.get(msg["severity"], -1)


def send_ntfy(ch, msg):
    server = (ch.get("server") or "https://ntfy.sh").strip().rstrip("/")
    tags = ["test_tube"] if msg["state"] == "test" else ["white_check_mark"] if msg["state"] == "resolved" else \
        ["rotating_light"] if msg["severity"] == "crit" else ["warning"]
    body = json.dumps({"topic": (ch.get("topic") or "").strip(), "title": msg["title"], "message": msg["body"],
                       "priority": ntfy_priority(msg), "tags": tags}).encode()
    headers = {"Content-Type": "application/json"}
    token = (ch.get("secrets") or {}).get("token")
    if token:
        headers["Authorization"] = "Bearer %s" % token
    post(server, body, headers, "token")


def send_pushover(ch, msg, api="https://api.pushover.net/1/messages.json"):
    s = ch.get("secrets") or {}
    if not s.get("appToken") or not s.get("userKey"):
        raise Failure("Pushover needs the app token and your user key", False)
    form = {"token": s["appToken"], "user": s["userKey"], "title": msg["title"], "message": msg["body"],
            "priority": str(pushover_priority(msg)), "timestamp": str(msg["tsMs"] // 1000)}
    if (ch.get("device") or "").strip():
        form["device"] = ch["device"].strip()
    status, raw = post(ch.get("api") or api, urllib.parse.urlencode(form).encode(),
                       {"Content-Type": "application/x-www-form-urlencoded"}, "app token and user key")
    try:
        if json.loads(raw).get("status") != 1:
            raise Failure("Pushover didn't accept it", False)
    except ValueError:
        raise Failure("Pushover sent an unreadable reply", True) from None


def webhook_body(ch, msg):
    if ch.get("slack"):
        v = {"text": "*%s*\n%s" % (msg["title"], msg["body"])}
    else:
        v = {"source": "sparkbox", "title": msg["title"], "body": msg["body"], "severity": msg["severity"], "state": msg["state"],
             "node": msg.get("node"), "rule": msg.get("rule"), "alertId": msg.get("alertId"), "tsMs": msg["tsMs"]}
    return json.dumps(v, separators=(",", ":")).encode()


def channel_url(ch):
    """A webhook's or dead-man's switch's URL is a credential: the Mac keeps it in the Keychain
    and copies it with the channel's secrets. Settings pushed before round 4 had it in the clear."""
    return ((ch.get("secrets") or {}).get("url") or ch.get("url") or "").strip()


def send_webhook(ch, msg):
    body = webhook_body(ch, msg)
    headers = {"Content-Type": "application/json"}
    secret = (ch.get("secrets") or {}).get("secret")
    if secret:
        headers["X-SparkBox-Signature"] = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    post(channel_url(ch), body, headers, "URL and secret")


def send_email(ch, msg):
    host, security = (ch.get("host") or "").strip(), ch.get("security") or "starttls"
    loopback = host in ("127.0.0.1", "::1", "localhost")
    if security not in ("starttls", "ssl") and not loopback:
        raise Failure("SparkBox won't send a password unencrypted: choose STARTTLS or SSL", False)
    m = EmailMessage()
    m["From"], m["To"], m["Date"] = ch.get("from"), ch.get("to"), formatdate(localtime=True)
    m["Subject"] = "[SparkBox] %s" % msg["title"] if msg["state"] != "resolved" else "[SparkBox] Resolved: %s" % msg["title"].replace("Resolved: ", "", 1)
    m.set_content("%s\n\n%s\n\n— SparkBox agent on %s" % (msg["title"], msg["body"], msg.get("via", "")))
    try:
        if security == "ssl":
            s = smtplib.SMTP_SSL(host, ch.get("port") or 465, local_hostname=socket.gethostname(), timeout=20, context=ssl.create_default_context())
        else:
            s = smtplib.SMTP(host, ch.get("port") or (587 if security == "starttls" else 25), local_hostname=socket.gethostname(), timeout=20)
            if security == "starttls":
                s.starttls(context=ssl.create_default_context())
        with s:
            if (ch.get("user") or "").strip():
                s.login(ch["user"].strip(), (ch.get("secrets") or {}).get("password", ""))
            s.send_message(m)
    except smtplib.SMTPAuthenticationError as e:
        raise Failure("SMTP %d — check the user and password" % e.smtp_code, False) from None
    except smtplib.SMTPResponseException as e:
        raise Failure("SMTP %d — %s" % (e.smtp_code, "the server is busy; it will retry" if 400 <= e.smtp_code < 500 else "the server refused it"),
                      400 <= e.smtp_code < 500) from None
    except ssl.SSLError:
        raise Failure("TLS with %s failed — check STARTTLS or SSL and the port" % host, False) from None
    except (OSError, smtplib.SMTPException) as e:
        raise Failure("Couldn't reach %s: %s" % (host, e), True) from None


def ping_heartbeat(ch):
    """The dead-man's switch: one GET of the check's URL (healthchecks.io, Uptime Kuma push,
    Cronitor). Its absence is what pages."""
    url = channel_url(ch)
    host = urllib.parse.urlsplit(url).hostname or url
    if not url.startswith(("https://", "http://")):
        raise Failure("The heartbeat address must start with https://", False)
    try:
        with urllib.request.urlopen(urllib.request.Request(url, method="GET"), timeout=10) as r:
            r.read(1024)
    except urllib.error.HTTPError as e:
        e.close()
        raise http_failure(e.code, None, "address") from None
    except (urllib.error.URLError, OSError) as e:
        raise Failure("Couldn't reach %s: %s" % (host, getattr(e, "reason", e)), True) from None


SENDERS = {"ntfy": send_ntfy, "pushover": send_pushover, "webhook": send_webhook, "email": send_email}
#: Channel kinds that aren't notification targets.
NOT_TARGETS = ("heartbeat", "mac")


def next_try(attempts, severity, created_ms, now):
    """When to try again after `attempts` failed tries, or None: 30 s, 2 min and 10 min,
    then a critical every 30 min until it's a day old."""
    if attempts <= len(RETRIES):
        return now + RETRIES[attempts - 1] * 1000
    if severity == "crit" and now + RETRY_CRIT_EVERY * 1000 <= created_ms + RETRY_CRIT_FOR * 1000:
        return now + RETRY_CRIT_EVERY * 1000
    return None


class Outbox:
    """Messages waiting to go out: one row per (message, channel) in outbox.db, so a
    restart or a settings push loses nothing. The main loop only inserts rows; one worker
    thread per channel sends them, so a slow or dead channel (a 20 s SMTP timeout, no
    internet) holds up neither the others nor the watch. A failure that may pass is tried
    again (next_try); deliveries.jsonl gets one line per attempt."""

    def __init__(self, path, log_path, node, channels, senders=None, clock=time.time, on_done=None):
        self.path, self.log_path, self.node = path, log_path, node
        self.channels = {c["id"]: c for c in channels if isinstance(c, dict) and c.get("id") and c.get("kind") not in NOT_TARGETS}
        self.senders, self.clock, self.on_done = senders or SENDERS, clock, on_done
        self.db = sqlite3.connect(path, timeout=10, check_same_thread=False)
        self.lock = threading.Lock()
        self.wake = threading.Condition()
        with self.lock:
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("CREATE TABLE IF NOT EXISTS outbox (id TEXT PRIMARY KEY, message_id TEXT, channel TEXT NOT NULL, "
                            "created_ms INTEGER NOT NULL, next_ms INTEGER, attempts INTEGER NOT NULL DEFAULT 0, "
                            "status TEXT NOT NULL, severity TEXT, msg TEXT NOT NULL, error TEXT, test TEXT, done_ms INTEGER)")
            self.db.execute("CREATE INDEX IF NOT EXISTS outbox_due ON outbox (status, channel, next_ms)")
            # A day of finished rows is plenty for status; the delivery log keeps the rest.
            self.db.execute("DELETE FROM outbox WHERE status != 'pending' AND done_ms < ?", (self.now() - 86_400_000,))
            self.db.commit()
        self.last = None
        self.threads = {}
        self.stopping = False

    def now(self):
        return int(self.clock() * 1000)

    def enqueue(self, msg, channels=None, test=None):
        """Queues a message for each channel that takes it; returns the delivery ids."""
        now = self.now()
        rows = []
        for cid in channels if channels is not None else list(self.channels):
            ch = self.channels.get(cid)
            if not ch or ch.get("kind") not in self.senders:
                continue
            if test is None and (not ch.get("enabled") or (ch.get("criticalOnly") and msg.get("severity") != "crit")):
                continue
            rows.append(("d-%s-%s" % (self.node, uuid.uuid4().hex[:10]), msg.get("messageId"), cid, now, now, 0, "pending",
                         msg.get("severity"), json.dumps(msg, separators=(",", ":")), None, test, None))
        if rows:
            with self.lock:
                self.db.executemany("INSERT INTO outbox VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", rows)
                self.db.commit()
            with self.wake:
                self.wake.notify_all()
        return [r[0] for r in rows]

    def pending(self):
        with self.lock:
            return self.db.execute("SELECT COUNT(*) FROM outbox WHERE status = 'pending'").fetchone()[0]

    def due(self, channel, now):
        with self.lock:
            return self.db.execute("SELECT id, message_id, channel, created_ms, attempts, severity, msg, test FROM outbox "
                                   "WHERE status = 'pending' AND channel = ? AND next_ms <= ? ORDER BY next_ms, created_ms LIMIT 1",
                                   (channel, now)).fetchone()

    def next_due(self, channel):
        with self.lock:
            return self.db.execute("SELECT MIN(next_ms) FROM outbox WHERE status = 'pending' AND channel = ?", (channel,)).fetchone()[0]

    def attempt(self, row):
        """One try of one row, in the calling (worker) thread."""
        rid, message_id, cid, created, attempts, severity, raw, test = row
        msg = json.loads(raw)
        ch = self.channels.get(cid)
        attempts += 1
        started = self.now()
        error, retry = None, False
        try:
            if ch is None:
                raise Failure("This channel was removed from the settings", False)
            self.senders[ch["kind"]](ch, msg)
        except Failure as f:
            error, retry = f.message, f.retry
        except Exception as e:  # noqa: BLE001 - a sender bug must not stop the worker
            error = "%s: %s" % (type(e).__name__, e)
        done = self.now()
        nxt = next_try(attempts, severity, created, done) if error and retry else None
        status = "sent" if error is None else "pending" if nxt else "failed"
        with self.lock:
            self.db.execute("UPDATE outbox SET attempts = ?, next_ms = ?, status = ?, error = ?, done_ms = ? WHERE id = ?",
                            (attempts, nxt, status, error, None if status == "pending" else done, rid))
            self.db.commit()
        d = {"id": rid, "tsMs": done, "channel": cid, "channelName": (ch or {}).get("name", cid), "kind": (ch or {}).get("kind"),
             "title": msg.get("title"), "body": msg.get("body"), "severity": msg.get("severity"), "ok": error is None,
             "error": error, "via": "agent:%s" % self.node, "demo": False, "test": bool(test), "attempts": attempts,
             "nextMs": nxt, "outcome": "sent" if error is None else "failed", "messageId": message_id, "ms": done - started}
        self.last = {"ms": done, "ok": error is None, "channel": cid}
        try:
            append_line(self.log_path, d)
        except OSError:
            pass
        if status != "pending" and self.on_done:
            self.on_done(test, d)
        return d

    def drain(self, channel=None):
        """Every due row, in this thread (tests and `once`). Returns the attempts."""
        out = []
        for cid in [channel] if channel else list(self.channels):
            while True:
                row = self.due(cid, self.now())
                if row is None:
                    break
                out.append(self.attempt(row))
        return out

    def work(self, cid):
        while not self.stopping:
            try:
                row = self.due(cid, self.now())
                if row is not None:
                    self.attempt(row)
                    continue
                nxt = self.next_due(cid)
            except sqlite3.ProgrammingError:
                return  # closed
            except sqlite3.Error as e:  # a disk problem: say so and keep trying, never stop the channel
                log("outbox (%s): %s" % (cid, e))
                nxt = self.now() + 5000
            wait = 5.0 if nxt is None else min(max((nxt - self.now()) / 1000.0, 0.05), 5.0)
            with self.wake:
                self.wake.wait(wait)

    def stop(self):
        self.stopping = True
        with self.wake:
            self.wake.notify_all()

    def close(self):
        """Stops the workers and closes the file; a worker mid-send finds it closed and ends."""
        self.stop()
        with self.lock:
            self.db.close()

    def start(self):
        for cid in self.channels:
            if cid not in self.threads:
                t = threading.Thread(target=self.work, args=(cid,), daemon=True, name="deliver-%s" % cid)
                self.threads[cid] = t
                t.start()


class Heartbeat:
    """The dead-man's switch: while this agent is the leader it GETs each heartbeat
    channel's URL every minute. If the house loses power or internet, or every agent is
    gone, the pings stop and that service pages the owner."""

    def __init__(self, channels, leading, ping=None, clock=time.time):
        self.channels = [c for c in channels if isinstance(c, dict) and c.get("kind") == "heartbeat" and c.get("enabled", True)]
        self.leading, self.ping, self.clock = leading, ping or ping_heartbeat, clock
        self.ok_ms, self.error = None, None

    def beat(self):
        if not self.channels or not self.leading():
            return False
        for ch in self.channels:
            try:
                self.ping(ch)
                self.ok_ms, self.error = int(self.clock() * 1000), None
            except Failure as f:
                self.error = f.message
        return True

    def run(self):
        last = None
        while True:
            # Checked every few seconds, so a new leader pings within moments, then once a minute.
            if self.leading() and (last is None or time.monotonic() - last >= HEARTBEAT_EVERY):
                last = time.monotonic()
                self.beat()
            time.sleep(5)

    def start(self):
        if self.channels:
            threading.Thread(target=self.run, daemon=True, name="heartbeat").start()


class Endpoints:
    """Probes the models this node serves, on loopback, with serving_probe.py (r3-serving):
    every 15 s, each in its own thread so a slow one delays nobody; serving_probe adds the
    one-token canary every minute from its state. Keys come from the credential and only
    travel in the request's header."""

    def __init__(self, endpoints, keys, probe=None, every=ENDPOINT_EVERY, clock=time.time):
        self.endpoints = {e["id"]: e for e in endpoints}
        self.keys = keys if isinstance(keys, dict) else {}
        self.probe = probe or (serving_probe.probe if serving_probe else None)
        self.every, self.clock = every, clock
        self.stall_after_s = CODED_DEFAULTS["endpoint-stalled"]["forS"]
        self.lock = threading.Lock()
        self.latest = {}   # id -> {"ms", "result", "endpoint"}
        self.states = {}   # id -> serving_probe's own state
        self.threads = {}

    def target(self, ep):
        t = {k: v for k, v in ep.items() if k != "containers"}
        t.setdefault("host", "127.0.0.1")
        t["stallAfterS"] = self.stall_after_s
        key = self.keys.get(ep["id"]) or self.keys.get(ep.get("keyRef") or "")
        if key:
            t["key"] = key
        return t

    def probe_once(self, eid):
        ep = self.endpoints[eid]
        now = int(self.clock() * 1000)
        state = self.states.setdefault(eid, {})
        try:
            res = self.probe(self.target(ep), state, now)
        except Exception as e:  # noqa: BLE001 - a prober bug is a failed probe, not a dead thread
            res = {"id": eid, "ok": False, "health": "down", "error": "The probe failed: %s: %s" % (type(e).__name__, e)}
        with self.lock:
            self.latest[eid] = {"ms": now, "result": res, "endpoint": ep}
        return res

    def run(self, eid):
        while eid in self.endpoints:
            started = time.monotonic()
            self.probe_once(eid)
            time.sleep(max(self.every - (time.monotonic() - started), 0.5))

    def start(self):
        if self.probe is None:
            return
        for eid in self.endpoints:
            if eid not in self.threads:
                self.threads[eid] = threading.Thread(target=self.run, args=(eid,), daemon=True, name="probe-%s" % eid)
                self.threads[eid].start()

    def update(self, endpoints, keys=None):
        """New endpoints from a reload: probes for removed ones end, new ones start."""
        self.endpoints = {e["id"]: e for e in endpoints}
        if keys is not None:
            self.keys = keys
        with self.lock:
            for gone in [eid for eid in self.latest if eid not in self.endpoints]:
                self.latest.pop(gone, None)
        self.threads = {eid: t for eid, t in self.threads.items() if eid in self.endpoints and t.is_alive()}
        self.start()

    def results(self, now=None):
        """The latest probe of each endpoint, while it's under a minute old."""
        now = now or int(self.clock() * 1000)
        with self.lock:
            return {eid: dict(r) for eid, r in self.latest.items() if now - r["ms"] < 60_000}

    def probed(self, now=None):
        return sorted(self.results(now))


# ---- systemd

def notify_address(addr):
    """$NOTIFY_SOCKET as a socket address: "@name" is in the abstract namespace."""
    return "\0" + addr[1:] if addr.startswith("@") else addr


# $NOTIFY_SOCKET, taken out of the environment when `run` starts (keep_notify_socket).
NOTIFY_SOCKET = None


def keep_notify_socket():
    """Takes $NOTIFY_SOCKET out of the environment and keeps it for sd_notify. The children the
    agent runs (journalctl, systemctl, timedatectl) would otherwise each send systemd an
    EXIT_STATUS on exit, which a Type=notify unit refuses and logs as a warning every time."""
    global NOTIFY_SOCKET
    NOTIFY_SOCKET = os.environ.pop("NOTIFY_SOCKET", None) or NOTIFY_SOCKET


def sd_notify(message, env=None):
    """Tells systemd READY=1, WATCHDOG=1 or STATUS=… (Type=notify): one datagram to
    $NOTIFY_SOCKET (or the one keep_notify_socket kept). Without it (not under systemd) it does
    nothing."""
    addr = env.get("NOTIFY_SOCKET") if env is not None else os.environ.get("NOTIFY_SOCKET") or NOTIFY_SOCKET
    if not addr:
        return False
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as s:
            s.connect(notify_address(addr))
            s.sendall(message.encode())
        return True
    except OSError:
        return False


# ---- The HTTP listeners

def prometheus(node, frame, firing, metrics_up=True, endpoints=None, leader=None):
    """The node and the models it serves in Prometheus text format 0.0.4."""
    lines = []
    def esc(v):
        return str(v).replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    def metric(name, kind, help_, samples):
        lines.append("# HELP %s %s" % (name, help_))
        lines.append("# TYPE %s %s" % (name, kind))
        for labels, v in samples:
            if v is None:
                continue
            labels = dict({"node": node}, **labels)
            lines.append("%s{%s} %s" % (name, ",".join('%s="%s"' % (k, esc(x)) for k, x in labels.items()), repr(float(v))))
    cpu, gpu, mem = frame.get("cpu") or {}, frame.get("gpu") or {}, frame.get("mem") or {}
    metric("sparkbox_cpu_percent", "gauge", "CPU busy, percent.", [({}, cpu.get("totalPct"))])
    metric("sparkbox_gpu_util_percent", "gauge", "GPU utilization, percent.", [({}, gpu.get("utilPct"))])
    metric("sparkbox_gpu_temp_celsius", "gauge", "GPU temperature.", [({}, gpu.get("tempC"))])
    metric("sparkbox_gpu_power_watts", "gauge", "GPU power draw.", [({}, gpu.get("powerW"))])
    metric("sparkbox_mem_used_bytes", "gauge", "Unified memory in use.", [({}, mem.get("used"))])
    metric("sparkbox_mem_total_bytes", "gauge", "Unified memory size.", [({}, mem.get("total"))])
    metric("sparkbox_disk_used_ratio", "gauge", "Filesystem use, 0 to 1.",
           [({"mount": d["mount"]}, d["used"] / d["total"] if d.get("total") else None) for d in frame.get("disks") or []])
    nets = []
    for n in frame.get("nets") or []:
        nets.append(({"iface": n["iface"], "dir": "rx"}, (n.get("rxBps") or 0) + (n.get("rdmaRxBps") or 0)))
        nets.append(({"iface": n["iface"], "dir": "tx"}, (n.get("txBps") or 0) + (n.get("rdmaTxBps") or 0)))
    metric("sparkbox_net_bits_per_second", "gauge", "Interface traffic, IP plus RDMA.", nets)
    roce = []
    for n in frame.get("nets") or []:
        for k, v in sorted((n.get("roce") or {}).items()):
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                roce.append(({"iface": n["iface"], "counter": k}, v))
    metric("sparkbox_roce_counter_total", "counter", "RoCE and PHY counters since boot.", roce)
    eps = sorted((endpoints or {}).items())
    def ep(field, scale=1.0):
        out = []
        for eid, r in eps:
            v = (r.get("result") or {}).get(field)
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                out.append(({"endpoint": eid}, v * scale))
        return out
    metric("sparkbox_endpoint_up", "gauge", "1 when the model answered its last probe and canary.",
           [({"endpoint": eid}, 1 if (r.get("result") or {}).get("ok") else 0) for eid, r in eps])
    metric("sparkbox_endpoint_generation_tokens_per_second", "gauge", "Tokens generated per second.", ep("genTps"))
    metric("sparkbox_endpoint_prompt_tokens_per_second", "gauge", "Prompt tokens processed per second.", ep("promptTps"))
    metric("sparkbox_endpoint_requests_running", "gauge", "Requests being generated.", ep("running"))
    metric("sparkbox_endpoint_requests_waiting", "gauge", "Requests waiting for a slot.", ep("waiting"))
    metric("sparkbox_endpoint_kv_cache_ratio", "gauge", "KV cache in use, 0 to 1.", ep("kvPct", 0.01))
    metric("sparkbox_endpoint_ttft_p95_seconds", "gauge", "Time to first token, 95th percentile.", ep("ttftP95", 0.001))
    metric("sparkbox_endpoint_canary_seconds", "gauge", "How long the one-token check took.", ep("canaryMs", 0.001))
    by = collections.Counter(a["severity"] for a in firing)
    metric("sparkbox_alerts_firing", "gauge", "Alerts firing on this node.", [({"severity": s}, by.get(s, 0)) for s in ("crit", "warn", "info")])
    metric("sparkbox_agent_up", "gauge", "1 while the agent runs.", [({}, 1 if metrics_up else 0)])
    if leader is not None:
        metric("sparkbox_agent_leader", "gauge", "1 when this agent pages about its peers.", [({}, 1 if leader else 0)])
    return "\n".join(lines) + "\n"


class Server(http.server.ThreadingHTTPServer):
    """HTTPServer without its reverse-DNS lookup of the bound address at start (it can
    take half a minute when DNS doesn't answer). `roles` says what it serves: health,
    metrics or both."""

    daemon_threads = True
    roles = frozenset(("health",))

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]

    @property
    def loopback(self):
        return self.server_address[0].startswith("127.")


class RateLimit:
    """At most RATE_PER_S requests a second from each client (a token bucket)."""

    def __init__(self, rate=RATE_PER_S, clock=time.monotonic):
        self.rate, self.clock = float(rate), clock
        self.buckets = {}
        self.lock = threading.Lock()

    def allow(self, client):
        now = self.clock()
        with self.lock:
            tokens, last = self.buckets.get(client, (self.rate, now))
            tokens = min(self.rate, tokens + (now - last) * self.rate)
            if len(self.buckets) > 1024:
                self.buckets = {k: v for k, v in self.buckets.items() if now - v[1] < 10}
            if tokens < 1:
                self.buckets[client] = (tokens, now)
                return False
            self.buckets[client] = (tokens - 1, now)
            return True


class Handler(http.server.BaseHTTPRequestHandler):
    agent = None
    server_version = "sparkbox-agent/" + VERSION
    timeout = 5  # a client that sends nothing is dropped

    def log_message(self, *args):
        pass

    def reply(self, status, body, ctype):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        a, path, roles = self.agent, self.path.split("?")[0], self.server.roles
        if not a.rate.allow(self.client_address[0]):
            self.reply(429, "too many requests\n", "text/plain")
        elif path == "/healthz" and "health" in roles:
            body, wedged = a.healthz()
            self.reply(503 if wedged else 200, json.dumps(body), "application/json")
        elif path == "/metrics" and "metrics" in roles and a.cfg.metrics_enabled:
            token = a.metrics_token
            if not token and not self.server.loopback:
                self.reply(403, "metrics beyond this node need a token: set one in SparkBox\n", "text/plain")
            elif token and not hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer %s" % token):
                self.reply(401, "unauthorized\n", "text/plain")
            else:
                self.reply(200, prometheus(a.cfg.node, a.frame or {}, a.engine.firing(), endpoints=a.endpoints.results(), leader=a.leading),
                           "text/plain; version=0.0.4; charset=utf-8")
        else:
            self.reply(404, "not found\n", "text/plain")


def own_addresses(frame):
    """(LAN address, [fabric addresses]) from a frame's interfaces."""
    nets = frame.get("nets") or []
    fabric = sorted({n["ip"] for n in nets if n.get("role") == "fabric" and n.get("ip")})
    lan = [n["ip"] for n in nets if n.get("role") == "lan" and n.get("ip")]
    return (lan[0] if lan else None), fabric


def usable(addr):
    """An address to listen on: a real one, never a wildcard."""
    return isinstance(addr, str) and addr not in ("", "0.0.0.0", "::", "*") and not addr.startswith("0.")


class Listeners:
    """One listener per own address (LAN, each fabric address, loopback) on the agent
    port, and on the metrics port when that's separate. Never 0.0.0.0: until a frame names
    an address, only loopback answers. sync() starts and stops listeners to match."""

    def __init__(self, agent, factory=None):
        self.agent = agent
        self.factory = factory or (lambda addr: Server(addr, Handler))
        self.servers = {}  # (addr, port) -> Server

    def wanted(self, frame):
        cfg = self.agent.cfg
        lan, fabric = own_addresses(frame or {})
        addrs = [a for a in (cfg.bind or ["127.0.0.1"] + ([lan] if lan else []) + fabric) if usable(a)]
        want = {}
        for a in addrs:
            want.setdefault((a, cfg.port), set()).add("health")
        if cfg.metrics_enabled:
            mb = cfg.metrics_bind
            maddrs = [mb] if isinstance(mb, str) else list(mb or []) or (cfg.bind or ["127.0.0.1"] + ([lan] if lan else []))
            for a in maddrs:
                if usable(a):
                    want.setdefault((a, cfg.metrics_port), set()).add("metrics")
        return {k: frozenset(v) for k, v in want.items()}

    def sync(self, frame):
        """Listen where wanted; returns True when anything changed."""
        want = self.wanted(frame)
        changed = False
        for addr, srv in list(self.servers.items()):
            if want.get(addr) != srv.roles:
                srv.shutdown()
                srv.server_close()
                del self.servers[addr]
                changed = True
        for addr, roles in want.items():
            if addr in self.servers:
                continue
            try:
                srv = self.factory(addr)
            except OSError as e:
                log("couldn't listen on %s:%d: %s" % (addr[0], addr[1], e))
                continue
            srv.roles = roles
            self.servers[addr] = srv
            threading.Thread(target=srv.serve_forever, daemon=True, name="http-%s" % addr[0]).start()
            changed = True
        return changed

    def close(self):
        for srv in self.servers.values():
            srv.shutdown()
            srv.server_close()
        self.servers = {}


# ---- Requests from SparkBox (requests/, written by the login user)

def read_request(path):
    """A request file's JSON object, or None: a regular file (not a link, not a FIFO) of at
    most 4 KiB."""
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    except OSError:
        return None
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_size > MAX_REQUEST:
            return None
        data = os.read(fd, MAX_REQUEST + 1)
    except OSError:
        return None
    finally:
        os.close(fd)
    v = parse_json(data, None) if len(data) <= MAX_REQUEST else None
    return v if isinstance(v, dict) else None


def is_ms(v):
    return isinstance(v, int) and not isinstance(v, bool) and v >= 0


def sign(key, kind, body):
    """A request's signature: HMAC-SHA256 keyed by the request key over "<kind>\n<body>", hex
    (agent/mod.rs signed_request writes the same)."""
    return hmac.new(key.encode(), ("%s\n%s" % (kind, body)).encode(), hashlib.sha256).hexdigest()


def verify_request(req, kind, key, now, seen):
    """(body, None) for a request {kind, body, mac} signed with `key` whose time (ackedMs or
    startedMs) is under 24 h old and at most a minute ahead, and that wasn't used before
    (`seen`: sha256 of each body taken, with its time); else (None, why)."""
    if not key:
        return None, "this agent has no request key: push the settings again"
    body, mac = req.get("body"), req.get("mac")
    if req.get("kind") != kind or not isinstance(body, str) or not isinstance(mac, str):
        return None, "it isn't signed"
    if not hmac.compare_digest(sign(key, kind, body), mac):
        return None, "its signature doesn't match"
    v = parse_json(body.encode("utf-8", "replace"), {})
    at = v.get("ackedMs" if kind == "ack" else "startedMs")
    if not is_ms(at):
        return None, "it has no time"
    if now - at > REQUEST_MAX_AGE_MS:
        return None, "it's more than 24 h old"
    if at - now > REQUEST_SKEW_MS:
        return None, "it starts in the future"
    nonce = hashlib.sha256(body.encode()).hexdigest()
    if nonce in seen:
        return None, "it was used before"
    seen[nonce] = at
    return v, None


def maintenance_entries(v):
    """The windows of a maintenance request {nodes: [id] (or node), untilMs|null, reason, by,
    startedMs, op} (or an end record {nodes, end: true}), checked field by field and bounded:
    a window lasts at most MAINT_MAX_MS from its start, a shutdown's (untilMs null) too.
    ([entries], None) or ([], why)."""
    nodes = v.get("nodes") if isinstance(v.get("nodes"), list) else [v.get("node")]
    if not nodes or len(nodes) > 64 or not all(isinstance(n, str) and NAME.match(n) for n in nodes):
        return [], "it doesn't name a node"
    if v.get("end") is True:
        return [{"node": n, "end": True} for n in nodes], None
    until, reason, by, started, op = v.get("untilMs"), v.get("reason"), v.get("by"), v.get("startedMs"), v.get("op")
    if not (until is None or is_ms(until)) or not is_ms(started):
        return [], "its times aren't readable"
    if not isinstance(reason, str) or not 0 < len(reason) <= 300 or not isinstance(by, str) or not 0 < len(by) <= 64:
        return [], "its reason isn't readable"
    cap = started + MAINT_MAX_MS
    return [{"node": n, "untilMs": cap if until is None else min(until, cap), "shutdown": until is None, "reason": reason, "by": by,
             "startedMs": started, "op": op if isinstance(op, str) and len(op) <= 64 else None} for n in nodes], None


# ---- The agent

class Agent:
    def __init__(self, cfg=None, collect=None, outbox_senders=None, watch=None, probe=None, heartbeat_ping=None):
        os.makedirs(STATE, exist_ok=True)
        self.started, self.started_ms = time.time(), now_ms()
        self.collect = collect
        self.notify, self.notify_raw = load_notify()
        self.metrics_token = (self.notify.get("metrics") or {}).get("token")
        # Signs acknowledgements and maintenance windows (and /healthz's settings hash).
        self.request_key = self.notify.get("requestKey") if isinstance(self.notify.get("requestKey"), str) else None
        self.cfg = cfg or Config.load()
        self.rules, self.rules_raw = load_rules()
        self.hash = config_hash(self.cfg.raw, self.rules_raw, self.notify_raw)
        saved = load_json(state_path("state.json"), {})
        self.engine = Engine(saved.get("active") or {})
        self.gate = gate.Gate(gate.Settings(self.policy()), ctx=self, state=saved.get("gate"))
        self.maint = saved.get("maintenance") or {}
        self.silences_done = set(saved.get("silencesDone") or [])
        self.unqueued = saved.get("unqueued") or []
        self.requests_seen = saved.get("requestsSeen") or {}  # sha256 of each signed body taken -> its time
        self.switch_noted = None  # the switch page another agent sent, already told to the gate
        self.trackers = Trackers()
        self.watch = watch or self.make_watch()
        self.leading, self.isolated, self.missing, self.links_down = False, None, [], False
        self.switch_ok_mono = time.monotonic()
        channels = self.notify.get("channels") or []
        self.outbox = Outbox(state_path("outbox.db"), state_path("deliveries.jsonl"), self.cfg.node, channels, outbox_senders,
                             on_done=self.test_done)
        self.heartbeat = Heartbeat(channels, lambda: self.leading, heartbeat_ping)
        self.endpoints = Endpoints(self.cfg.endpoints, self.notify.get("endpointKeys"), probe)
        self.endpoints.stall_after_s = stall_after({r.get("id"): r for r in self.rules})
        self.history = History(state_path("history.db"))
        self.frame = None
        self.last_sample = None
        self.last_cycle_ms = None
        self.listeners = Listeners(self)
        self.rate = RateLimit()
        self.reload_wanted = False
        self.tests = {}         # request id -> {"path", "waiting": {channel ids}, "results": [...]}
        self.last_test = {}     # channel -> ms
        self.snapshot = None    # the last /healthz body, published by the main loop
        self.endpoint_seen = {}  # endpoint -> the probe last recorded
        self.lock = threading.Lock()
        Handler.agent = self

    # -- settings

    def policy(self):
        """When to notify: agent.json (r3), else r2's notify.json policy."""
        return self.cfg.policy if self.cfg.policy else (self.notify.get("policy") or {})

    def make_watch(self, start=None):
        route = collector.proc("net/route")
        return peers.PeerWatch(self.cfg.node, self.cfg.index, self.cfg.nodes, self.cfg.port, start=start,
                               gateway=lambda: peers.gateway_answers(peers.default_gateway(collector.read(route)), ping=ping_once))

    def reload(self):
        """SIGHUP: agent.json and rules.json again (the channels are read only at start)."""
        self.reload_wanted = False
        old = self.cfg
        self.cfg = Config.load()
        self.rules, self.rules_raw = load_rules()
        self.hash = config_hash(self.cfg.raw, self.rules_raw, self.notify_raw)
        self.gate.settings = gate.Settings(self.policy())
        if [(p["id"], p.get("lan") or p.get("addr"), p.get("fabric")) for p in old.nodes] != \
                [(p["id"], p.get("lan") or p.get("addr"), p.get("fabric")) for p in self.cfg.nodes] or old.index != self.cfg.index:
            self.watch = self.make_watch()
        self.endpoints.stall_after_s = stall_after({r.get("id"): r for r in self.rules})
        self.endpoints.update(self.cfg.endpoints)
        self.listeners.sync(self.frame)
        log("settings reloaded (%d rules, settings %s)" % (len(self.rules), self.hash[:12]))

    # -- what the gate asks (gate.Gate ctx)

    def silenced(self, ev):
        now = now_ms()
        for s in self.cfg.silences:
            if silence_active(s, now, self.silences_done) and silence_matches(s, ev):
                return s.get("reason") or "silenced"
        return None

    def planned(self, ev, now):
        """Alerts about a node in a maintenance window SparkBox opened (a restart or a
        shutdown), until it's back or overdue."""
        nodes = {gate.node_of(ev)}
        inc = ev.get("incident") or ""
        if inc.startswith("node:"):
            nodes.add(inc[5:])
        if inc.startswith("endpoint:"):
            ep = next((e for e in self.cfg.endpoints if e["id"] == inc[9:]), None)
            nodes |= {ep.get("headNode") or self.cfg.node} | set(ep.get("nodes") or []) if ep else set()
        for node in nodes:
            m = self.maint.get(node or "")
            if m and now < self.window_end(m):
                return "%s %s (%s, %s)" % (self.name_of(node), m["reason"], m["by"], gate.clock(m["startedMs"], self.quiet_zone()))
        return None

    def window_end(self, m):
        """When a maintenance window stops covering its node's alerts: its end and the restart's
        grace, and never past MAINT_MAX_MS from its start."""
        cap = m["startedMs"] + MAINT_MAX_MS
        return cap if m.get("untilMs") is None else min(m["untilMs"] + (0 if m.get("shutdown") else self.reboot_grace_ms()), cap)

    def name_of(self, node):
        if node == self.cfg.node:
            return self.cfg.name
        return next((p.get("name") or p["id"] for p in self.cfg.nodes if p["id"] == node), node)

    def quiet_zone(self):
        q = self.gate.settings.quiet or self.policy().get("quiet") or {}
        return (q.get("tz") if isinstance(q, dict) else None) or self.cfg.tz

    def reboot_grace_ms(self):
        r = next((r for r in self.rules if r.get("id") == "node-reboot"), CODED_DEFAULTS["node-reboot"])
        return int(r.get("forS") or 600) * 1000

    # -- maintenance windows

    def add_maintenance(self, entry, now):
        if entry.get("end"):
            if self.maint.pop(entry["node"], None):
                log("maintenance on %s ended" % entry["node"])
            return
        entry = dict(entry, seenDown=False, receivedMs=now)
        self.maint[entry["node"]] = entry
        log("maintenance on %s: %s until %s" % (entry["node"], entry["reason"], entry["untilMs"]))

    def tend_maintenance(self, now, mono):
        """Ends windows whose node is back, that have run their 4 hours, or whose node never
        went down in 20 minutes (a restart refused or called off); finds the restarts overdue."""
        findings = []
        by_id = {p.id: p for p in self.watch.peers}
        for node, m in list(self.maint.items()):
            if now >= m["startedMs"] + MAINT_MAX_MS:
                log("the maintenance window on %s ended: it ran its 4 hours" % node)
                self.maint.pop(node)
                continue
            if node == self.cfg.node:
                # This agent started after the window opened (the node came back), the window
                # is over, or the node never went down.
                if self.started_ms > m["startedMs"] or now >= self.window_end(m) or now - m["startedMs"] >= MAINT_NEVER_DOWN_MS:
                    self.maint.pop(node)
                continue
            p = by_id.get(node)
            if p is None:
                self.maint.pop(node)
                continue
            up = p.agent_ok(mono)
            if not p.reachable(mono):
                m["seenDown"] = True
            if up and (m["seenDown"] or (not m.get("shutdown") and m["untilMs"] is not None and now >= m["untilMs"])):
                log("%s is back; its maintenance window ended" % node)
                self.maint.pop(node)
                continue
            if not m["seenDown"] and now - m["startedMs"] >= MAINT_NEVER_DOWN_MS:
                log("%s never went down; its maintenance window ended" % node)
                self.maint.pop(node)
                continue
            overdue = not m.get("shutdown") and m["untilMs"] is not None and now >= m["untilMs"] + self.reboot_grace_ms()
            if overdue and not p.reachable(mono) and self.leading:
                what = "the restart" if re.search(r"restart|reboot", m["reason"], re.I) else "the maintenance"
                findings.append(("node-reboot", "crit", p, "%s didn't come back after %s %s started at %s" % (
                    p.name, what, m["by"], gate.clock(m["startedMs"], self.quiet_zone()))))
        return findings

    # -- one cycle

    def record(self, frame, ts):
        self.history.add(ts, "node:%s" % self.cfg.node, node_metrics(frame))
        for c in frame.get("containers") or []:
            if c.get("state") == "running":
                self.history.add(ts, "container:%s/%s" % (self.cfg.node, c["name"]), {"cpu": c.get("cpuPct"), "mem": c.get("mem")})
        for n in frame.get("nets") or []:
            if not n.get("roce"):
                continue
            key = "roce|%s" % n["iface"]
            prev = self.trackers.series.get(key)
            cur = {m: roce_value(n["roce"], m) for m in NET_METRICS}
            if prev and prev[0] < ts:
                dt = (ts - prev[0]) / 1000.0
                rates = {m: (cur[m] - prev[1][m]) / dt for m in NET_METRICS if cur[m] is not None and prev[1].get(m) is not None and cur[m] >= prev[1][m]}
                self.history.add(ts, "net:%s/%s" % (self.cfg.node, n["iface"]), rates)
            self.trackers.series[key] = (ts, cur)

    def record_endpoints(self, results, ts):
        """endpoint:<id> series (the Mac's backfill merges them), once per new probe."""
        for eid, r in results.items():
            if self.endpoint_seen.get(eid) == r["ms"]:
                continue
            self.endpoint_seen[eid] = r["ms"]
            res = r.get("result") or {}
            values = {m: res.get(m) for m in ENDPOINT_METRICS if m != "canaryMs"}
            canary = res.get("canary") or {}
            values["canaryMs"] = canary.get("ms") if canary.get("ok") else None
            self.history.add(ts, "endpoint:%s" % eid, values)

    def has_fabric(self, frame):
        return bool(own_addresses(frame)[1] or self.cfg.me.get("fabric"))

    def judge_switch(self, mono, found=()):
        """The leader checks the fabric switch's management address; the switch page names it
        already when every node lost the fabric."""
        sw = self.cfg.switch
        if not sw or not self.leading or mono - self.switch_ok_mono < peers.MISSING_S or any(f[0] == "fabric-switch-down" for f in found):
            return []
        return [("switch-unreachable", "crit", sw, "unreachable")]

    def note_switch_page(self, found, now, under_switch):
        """Another agent paged the switch this node lost the fabric through: its incident counts
        as paged here, so this node's fabric-lost and fabric-down warnings are coalesced into it
        (gate G17)."""
        if not under_switch or any(f[0] == "fabric-switch-down" for f in found):
            return
        for p in self.watch.peers:
            sd = (p.view or {}).get("switchDown")
            if isinstance(sd, dict) and isinstance(sd.get("incident"), str) and self.switch_noted != (sd["incident"], sd.get("sinceMs")):
                self.switch_noted = (sd["incident"], sd.get("sinceMs"))
                self.gate.decide([{"kind": "paged", "incident": sd["incident"], "severity": "crit", "by": p.id}], now)

    def cycle(self, mono=None, now=None):
        """One sample: record, evaluate, decide what to page, queue it, save. Never waits on
        the network. Returns the alert events, or None when there was no frame (then the
        cycle doesn't count as completed)."""
        now = now if now is not None else now_ms()
        mono = mono if mono is not None else time.monotonic()
        frame = self.collect() if self.collect else collector.sample(self.sampler)
        if frame is None:
            return None
        self.frame, self.last_sample = frame, now
        self.record(frame, now)
        results = self.endpoints.results(now)
        self.record_endpoints(results, now)
        self.links_down = fabric_links_down(frame, self.trackers)
        self.leading, self.isolated, found = self.watch.judge(mono, self.has_fabric(frame), self.links_down)
        self.missing = [f[2].id for f in found if f[0] == "peer-unreachable"]
        under_switch = self.watch.switch_down(mono, self.has_fabric(frame), self.links_down) is not None
        self.note_switch_page(found, now, under_switch)
        found = found + self.tend_maintenance(now, mono) + self.judge_switch(mono, found)
        view = {"seen": [self.name_of(n) for n in self.watch.seen(mono)], "minority": self.watch.minority(mono), "switch": under_switch}
        conds = evaluate(frame, self.rules, self.cfg, self.trackers, self.engine.active, now, found, results, view)
        events = self.engine.step(conds, now)
        for e in events:
            append_line(state_path("alerts.jsonl"), e)
        self.page(events, now, view)
        self.save_state(now)
        self.last_cycle_ms = now_ms()
        self.snapshot = self.health(mono)
        sd_notify("WATCHDOG=1")
        return events

    def gate_event(self, e):
        a = e["alert"]
        ent = a["entity"]
        kind = {"eased": "deescalated"}.get(e["kind"], e["kind"])
        title = a["summary"]
        if kind == "resolved":
            title = "%s is back online" % ent["label"] if a["rule"] in ("node-offline", "peer-unreachable") else "Resolved: %s" % a["summary"]
        return {"kind": kind, "rule": a["rule"], "entity": gate.entity_key(ent), "severity": a["severity"], "incident": a.get("incident"),
                "title": title, "body": "%s — %s" % (ent["label"], a["message"]), "node": ent.get("node"), "label": ent["label"],
                "category": a.get("category"), "notify": a.get("notify"), "alertId": a["id"]}

    def page(self, events, now, view=None):
        """Every event through the gate (one outcome each); what it lets out goes to the outbox.
        Peers found dead together go out as one page."""
        evs = [self.gate_event(e) for e in events] + self.unplanned(now)
        for ev in evs:
            if ev["kind"] == "resolved":
                for s in self.cfg.silences:
                    clears = s.get("untilClears") or (isinstance(s.get("until"), dict) and s["until"].get("kind") == "resolved")
                    if clears and silence_matches(s, ev):
                        self.silences_done.add(s.get("id"))  # "until it clears": it has
        outcomes = self.gate.decide(evs, now)
        for o, ev in zip(outcomes, evs):
            a = self.engine.active.get(ev["alertId"])
            if a is not None:
                a["paging"] = o["outcome"]
            self.record_outcome(o, ev, now)
        for o in self.gate.tick(now):
            self.record_outcome(o, None, now)
        seen = (view or {}).get("seen") or [self.cfg.name]
        title = lambda ids: peers.outage_title([self.name_of(n) for n in ids], seen, bool((view or {}).get("minority")))  # noqa: E731
        for msg in merge_outages(self.gate.flush(now), title):
            msg["via"] = self.cfg.name
            self.unqueued.append(msg)
        # A message the gate let out is never dropped: one the outbox couldn't take (a disk
        # error) waits here, and in state.json, for the next cycle.
        while self.unqueued:
            try:
                self.outbox.enqueue(self.unqueued[0])
            except sqlite3.Error as e:
                log("couldn't queue \"%s\": %s" % (self.unqueued[0]["title"], e))
                break
            self.unqueued.pop(0)
        return outcomes

    def unplanned(self, now):
        """Alerts raised while planned work ran that still fire once its window is over: raised
        again through the gate, since now they're an outage (a shutdown past its 4 hours)."""
        out = []
        for a in self.engine.firing():
            if a.get("paging") == "planned":
                ev = self.gate_event({"kind": "raised", "alert": a})
                if self.planned(ev, now) is None:
                    out.append(ev)
        return out

    def record_outcome(self, o, ev, now):
        """deliveries.jsonl gets one line per alert event, sent or not, and why."""
        line = {"id": "o-%s-%s" % (self.cfg.node, uuid.uuid4().hex[:10]), "tsMs": now, "outcome": o["outcome"], "rule": o["rule"],
                "entity": o["entity"], "severity": o.get("severity"), "event": o.get("kind"), "reason": o.get("reason"),
                "messageId": o.get("messageId"), "title": (ev or {}).get("title") or ("Still firing" if o.get("kind") == "repeat" else None),
                "alertId": (ev or {}).get("alertId"), "via": "agent:%s" % self.cfg.node, "channel": None, "demo": False}
        try:
            append_line(state_path("deliveries.jsonl"), line)
        except OSError:
            pass

    def save_state(self, now):
        firing = self.engine.firing()
        beat = {"node": self.cfg.node, "version": VERSION, "lastBeatMs": now, "lastCycleMs": now, "historyFromMs": self.history.span(),
                "firing": len(firing), "configHash": self.hash, "bootId": (self.frame or {}).get("bootId")}
        write_atomic(state_path("beat.json"), json.dumps(beat))
        self.requests_seen = {k: t for k, t in self.requests_seen.items() if now - t < REQUEST_MAX_AGE_MS + REQUEST_SKEW_MS}
        write_atomic(state_path("state.json"), json.dumps({"active": self.engine.active, "gate": self.gate.state(), "maintenance": self.maint,
                                                           "silencesDone": sorted(self.silences_done), "unqueued": self.unqueued,
                                                           "requestsSeen": self.requests_seen, "savedMs": now}))

    # -- what others see

    def health(self, mono=None):
        """/healthz and the core of `status`: what SparkBox and the other agents need, and
        nothing more (no channel names, no secrets). The settings hash is there only as an
        HMAC keyed by the request key (configMac), so nobody on the network can test guesses
        of a secret against it; `status` over SSH adds the hash itself."""
        mono = mono if mono is not None else time.monotonic()
        lan, fabric = own_addresses(self.frame or {})
        last = self.outbox.last
        sd = next((a for a in self.engine.firing() if a["rule"] == "fabric-switch-down"), None)
        body = {"node": self.cfg.node, "version": VERSION, "lastCycleMs": self.last_cycle_ms,
                "addresses": {"lan": lan or self.cfg.me.get("lan"), "fabric": fabric or list(self.cfg.me.get("fabric") or [])},
                "peers": self.watch.view(mono), "missing": list(self.missing), "isolated": self.isolated, "leader": self.leading,
                "gateway": self.watch.gateway_ok(mono), "fabricDown": self.links_down, "switchDown": {"incident": sd.get("incident"), "sinceMs": sd.get("firingSinceMs")} if sd else None,
                "outboxPending": self.outbox.pending(), "lastDelivery": dict(last) if last else None,
                "heartbeatOkMs": self.heartbeat.ok_ms, "endpointsProbed": self.endpoints.probed(), "quietTz": self.gate.settings.quiet_tz()}
        if self.request_key:
            body["configMac"] = hmac.new(self.request_key.encode(), self.hash.encode(), hashlib.sha256).hexdigest()
        return body

    def wedged(self, now=None):
        now = now or now_ms()
        last = self.last_cycle_ms if self.last_cycle_ms is not None else self.started_ms
        return now - last > WEDGED_MS

    def healthz(self):
        """(body, wedged): 503 {wedged: true} once no cycle has completed for 30 s."""
        snap = self.snapshot or {"node": self.cfg.node, "version": VERSION, "lastCycleMs": self.last_cycle_ms}
        if self.wedged():
            return {"node": self.cfg.node, "version": VERSION, "lastCycleMs": self.last_cycle_ms, "wedged": True}, True
        return snap, False

    def status(self):
        """`status --json`: /healthz plus what the Settings window shows."""
        s = dict(self.snapshot or self.health())
        firing = self.engine.firing()
        frame = self.frame or {}
        s.update({"configHash": self.hash, "uptimeS": int(time.time() - self.started), "lastSampleMs": self.last_sample, "lastBeatMs": self.last_cycle_ms,
                  "bootId": frame.get("bootId"), "historyFromMs": self.history.span(), "historyBytes": self.history.size(),
                  "historyVacuum": self.history.auto_vacuum(), "rules": len(self.rules),
                  "firing": [{"id": a["id"], "rule": a["rule"], "severity": a["severity"], "summary": a["summary"],
                              "incident": a.get("incident"), "paging": a.get("paging")} for a in firing],
                  "channels": [{"id": c.get("id"), "name": c.get("name"), "kind": c.get("kind"), "enabled": bool(c.get("enabled", True))}
                               for c in self.notify.get("channels") or []],
                  "heartbeatError": self.heartbeat.error, "silences": len(self.cfg.silences),
                  "maintenance": [{"node": n, "untilMs": m["untilMs"], "reason": m["reason"], "by": m["by"], "startedMs": m["startedMs"],
                                   "op": m.get("op")} for n, m in sorted(self.maint.items())],
                  "acks": sum(1 for k in self.gate.keys.values() if k.get("acked")),
                  "switchWatched": bool(self.cfg.switch and self.leading), "errors": frame.get("errors") or [],
                  "metrics": {"enabled": self.cfg.metrics_enabled, "port": self.cfg.metrics_port}})
        return s

    # -- requests

    def handle_requests(self, now=None):
        """requests/: signed ack-*.json and maintenance-*.json from SparkBox (each answered in
        <name>.result.json), and test requests from `test-notify` (the login user can't read
        the channels, so it asks)."""
        now = now or now_ms()
        folder = state_path("requests")
        try:
            names = sorted(os.listdir(folder))
        except OSError:
            return
        for name in names:
            if not name.endswith(".json") or name.endswith(".result.json") or name.endswith(".tmp"):
                continue
            path = os.path.join(folder, name)
            req = read_request(path)
            try:
                os.remove(path)
            except OSError:
                pass
            base = name[:-5]
            signed = name.startswith(("ack-", "maintenance-"))
            if req is None:
                log("ignored %s: not a small JSON file" % name)
                if signed:
                    self.answer(base, "it isn't a small JSON file")
                continue
            if signed:
                kind = "ack" if name.startswith("ack-") else "maintenance"
                body, why = verify_request(req, kind, self.request_key, now, self.requests_seen)
                if body is not None:
                    why = self.take_ack(body, now) if kind == "ack" else self.take_maintenance(body, now)
                if why:
                    log("refused %s: %s" % (name, why))
                self.answer(base, why)
            elif REQUEST_ID.match(base):
                self.start_test(base, req, now)

    def answer(self, base, why):
        """<name>.result.json: {ok} or {ok: false, error}, for SparkBox to read and remove."""
        try:
            write_atomic(state_path(os.path.join("requests", "%s.result.json" % base)), json.dumps({"ok": why is None, "error": why}))
        except OSError as e:
            log("couldn't answer %s: %s" % (base, e))

    def take_ack(self, v, now):
        """Acknowledged on the Mac: no more repeats from here, until it gets worse."""
        if not isinstance(v.get("rule"), str) or not isinstance(v.get("entity"), str):
            return "it doesn't name an alert"
        self.gate.decide([{"kind": "ack", "rule": v["rule"], "entity": v["entity"], "ackedMs": v["ackedMs"]}], now)
        k = self.gate.find(v)
        for a in self.engine.active.values():
            if a["rule"] == v["rule"] and gate.same_entity(gate.entity_key(a["entity"]), v["entity"]):
                a["ackedMs"] = v["ackedMs"]
        log("acknowledged on the Mac by %s: %s on %s%s" % (v.get("by") or "the owner", v["rule"], v["entity"], "" if k else " (not firing here yet)"))
        return None

    def take_maintenance(self, v, now):
        entries, why = maintenance_entries(v)
        for e in entries:
            self.add_maintenance(e, now)
        return why

    def start_test(self, rid, req, now):
        channels = self.notify.get("channels") or []
        if req.get("all"):
            chosen = [c for c in channels if c.get("enabled", True)]
        else:
            chosen = [c for c in channels if c.get("id") == req.get("channel")]
        result_path = state_path(os.path.join("requests", "%s.result.json" % rid))
        if not chosen:
            text = "The agent on %s has no channel \"%s\". Push the settings again." % (self.cfg.name, req.get("channel")) \
                if not req.get("all") else "The agent on %s has no channels. Push the settings again." % self.cfg.name
            write_atomic(result_path, json.dumps({"node": self.cfg.node, "ok": False, "message": text, "results": []}))
            return
        test = {"path": result_path, "all": bool(req.get("all")), "waiting": set(), "results": []}
        msg = {"title": "SparkBox test from the agent on %s" % self.cfg.name,
               "body": "This is how alerts from %s reach you while SparkBox is closed." % self.cfg.name,
               "severity": "warn", "state": "test", "node": self.cfg.node, "rule": None, "alertId": None, "tsMs": now,
               "via": self.cfg.name, "messageId": "t-%s" % rid}
        with self.lock:
            self.tests[rid] = test
        for ch in chosen:
            cid = ch.get("id")
            if now - self.last_test.get(cid, -TEST_EVERY_MS) < TEST_EVERY_MS:
                test["results"].append({"channel": cid, "name": ch.get("name"), "kind": ch.get("kind"), "ok": False,
                                        "error": "A test went through this channel seconds ago; wait a moment", "ms": None})
                continue
            self.last_test[cid] = now
            if ch.get("kind") == "heartbeat":
                threading.Thread(target=self.test_heartbeat, args=(rid, ch), daemon=True).start()
                test["waiting"].add(cid)
                continue
            if self.outbox.enqueue(msg, [cid], test=rid):
                test["waiting"].add(cid)
            else:
                test["results"].append({"channel": cid, "name": ch.get("name"), "kind": ch.get("kind"), "ok": False,
                                        "error": "This agent can't send through %s channels" % ch.get("kind"), "ms": None})
        self.finish_test(rid)

    def test_heartbeat(self, rid, ch):
        started = now_ms()
        try:
            ping_heartbeat(ch)
            ok, error = True, None
        except Failure as f:
            ok, error = False, f.message
        self.test_done(rid, {"channel": ch.get("id"), "channelName": ch.get("name"), "kind": "heartbeat", "ok": ok, "error": error,
                             "ms": now_ms() - started})

    def test_done(self, rid, d):
        """A test delivery finished (from a worker thread)."""
        if not rid:
            return
        with self.lock:
            test = self.tests.get(rid)
            if test is None:
                return
            test["waiting"].discard(d["channel"])
            test["results"].append({"channel": d["channel"], "name": d.get("channelName"), "kind": d.get("kind"), "ok": d["ok"],
                                    "error": d.get("error"), "ms": d.get("ms"), "delivery": d if d.get("id") else None})
        self.finish_test(rid)

    def finish_test(self, rid):
        with self.lock:
            test = self.tests.get(rid)
            if test is None or test["waiting"]:
                return
            del self.tests[rid]
        results = test["results"]
        ok = bool(results) and all(r["ok"] for r in results)
        if test["all"]:
            message = "Sent from the agent on %s through %d channel%s." % (self.cfg.name, len(results), "" if len(results) == 1 else "s") if ok \
                else "From %s: %s" % (self.cfg.name, "; ".join("%s failed: %s" % (r["name"] or r["channel"], r["error"]) for r in results if not r["ok"]))
        else:
            r = results[0]
            message = "Sent from the agent on %s." % self.cfg.name if r["ok"] else "Failed: %s" % r["error"]
        out = {"node": self.cfg.node, "ok": ok, "message": message, "results": results}
        if not test["all"]:
            out["delivery"] = results[0].get("delivery")
        write_atomic(test["path"], json.dumps(out))

    # -- the loop

    def watch_peers(self):
        while True:
            started = time.monotonic()
            try:
                self.watch.poll()
                sw = self.cfg.switch
                if sw and self.leading and peers.gateway_answers(sw["host"], ping=ping_once):
                    self.switch_ok_mono = time.monotonic()
                elif not (sw and self.leading):
                    self.switch_ok_mono = time.monotonic()
            except Exception as e:  # noqa: BLE001 - the watch must keep going
                log("peer watch: %s: %s" % (type(e).__name__, e))
            time.sleep(max(peers.EVERY - (time.monotonic() - started), 1.0))

    def tidy(self, now=None):
        """Hourly: retention, the size cap, a little space back to the disk, the logs trimmed, and
        answers nobody collected removed."""
        now = now or now_ms()
        self.history.prune()
        self.history.enforce_cap(beat=lambda: sd_notify("WATCHDOG=1"))
        self.history.vacuum()
        for name in ("alerts.jsonl", "deliveries.jsonl"):
            trim_log(state_path(name))
        folder = state_path("requests")
        try:
            for name in os.listdir(folder):
                path = os.path.join(folder, name)
                if name.endswith(".result.json") and now - os.lstat(path).st_mtime * 1000 > RESULT_KEEP_MS:
                    os.remove(path)
        except OSError:
            pass

    def install_signals(self):
        """SIGHUP (systemctl reload): re-read agent.json and rules.json at the next cycle."""
        signal.signal(signal.SIGHUP, lambda *_: setattr(self, "reload_wanted", True))
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))

    def run(self):
        # The state holds no secrets (those stay in the credential): the login user reads it
        # for `status`, `export` and `alerts` (StateDirectoryMode=0755).
        os.umask(0o022)
        self.install_signals()
        self.sampler = collector.Collector(gpu_every=GPU_EVERY, docker_every=DOCKER_EVERY, slow_every=SLOW_EVERY)
        self.outbox.start()
        self.heartbeat.start()
        self.endpoints.start()
        threading.Thread(target=self.watch_peers, daemon=True, name="peers").start()
        while self.cycle() is None:
            time.sleep(CORE_EVERY)
        self.listeners.sync(self.frame)
        sd_notify("READY=1")
        log("watching %s (%s) · listening on %s" % (self.cfg.name, "leader" if self.leading else "not the leader",
                                                     ", ".join("%s:%d" % a for a in sorted(self.listeners.servers)) or "nothing yet"))
        last_bind, last_tidy, last_status = time.monotonic(), time.monotonic(), 0.0
        try:
            while True:
                time.sleep(CORE_EVERY)
                mono = time.monotonic()
                if self.reload_wanted:
                    self.reload()
                try:
                    self.cycle(mono)
                except Exception as e:  # noqa: BLE001 - one bad cycle must not stop the watch
                    log("cycle failed: %s: %s" % (type(e).__name__, e))
                self.handle_requests()
                if mono - last_bind >= REBIND_EVERY:
                    last_bind = mono
                    if self.listeners.sync(self.frame):
                        log("listening on %s" % ", ".join("%s:%d" % a for a in sorted(self.listeners.servers)))
                if mono - last_status >= 10:
                    last_status = mono
                    write_atomic(state_path("status.json"), json.dumps(self.status()))
                    sd_notify("STATUS=%s" % self.summary())
                if mono - last_tidy >= 3600:
                    last_tidy = mono
                    self.tidy()
        finally:
            self.history.flush()

    def close(self):
        """Stops the workers and listeners and closes the files (tests; `run` ends with the process)."""
        self.outbox.close()
        self.listeners.close()
        try:
            self.history.flush()
        except sqlite3.Error:
            pass
        with self.history.lock:
            self.history.db.close()

    def summary(self):
        firing = self.engine.firing()
        parts = ["%d firing" % len(firing), "leader" if self.leading else "watching"]
        if self.isolated:
            parts.append("isolated: %s" % self.isolated)
        if self.outbox.pending():
            parts.append("%d to send" % self.outbox.pending())
        return " · ".join(parts)


def ping_once(addr):
    """One ping, a second at most (bounded: never waits on a stuck child)."""
    return collector.run(["ping", "-c", "1", "-W", "1", addr], timeout=3) is not None


# ---- Command line (run over SSH as the login user, who reads the state but not the channels)

def local_healthz(port):
    try:
        with urllib.request.urlopen("http://127.0.0.1:%d/healthz" % port, timeout=3) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read())
        except ValueError:
            return None
    except (OSError, ValueError):
        return None


def cmd_status():
    """What `systemctl is-active` can't say: the running agent's status.json (written every
    10 s), its heartbeat, and when the files can't be read, its /healthz on loopback."""
    status = load_json(state_path("status.json"), None)
    beat = load_json(state_path("beat.json"), {})
    if status is None:
        status = local_healthz(Config.load().port) or {}
    status.update({"lastBeatMs": beat.get("lastBeatMs") or status.get("lastBeatMs"), "version": status.get("version") or beat.get("version") or VERSION,
                   "historyFromMs": status.get("historyFromMs") or beat.get("historyFromMs"),
                   "configHash": status.get("configHash") or beat.get("configHash")})
    print(json.dumps(status))
    return 0


def cmd_export(args):
    since, until, entity = int(arg(args, "--since", "0")), arg(args, "--until"), arg(args, "--entity")
    path = state_path("history.db")
    if not os.path.exists(path):
        return 0
    rows = None
    for mode in (True, "immutable"):
        h = None
        try:
            h = History(path, readonly=mode)
            rows = list(h.export(since, int(until) if until else None, entity))
            break
        except sqlite3.OperationalError:
            # The agent isn't running, so the WAL's -shm file is gone and this user can't
            # make one: read the file as it stands.
            continue
        finally:
            if h is not None:
                h.db.close()
    for row in rows or []:
        sys.stdout.write(json.dumps(row, separators=(",", ":")) + "\n")
    return 0


def cmd_since(name, args):
    """`alerts` and `deliveries`: the lines of alerts.jsonl or deliveries.jsonl since a time."""
    since = int(arg(args, "--since", "0"))
    try:
        with open(state_path(name)) as f:
            for line in f:
                try:
                    if json.loads(line).get("tsMs", 0) >= since:
                        sys.stdout.write(line if line.endswith("\n") else line + "\n")
                except ValueError:
                    continue
    except OSError:
        pass
    return 0


def cmd_test_notify(request, wait=60.0):
    """Asks the running agent (which holds the channels) for a test, and waits for it:
    {"channel": id} for one channel, {"all": true} for every enabled one."""
    folder = state_path("requests")
    rid = uuid.uuid4().hex[:12]
    try:
        os.makedirs(folder, exist_ok=True)
        write_atomic(os.path.join(folder, "%s.json" % rid), json.dumps(request))
    except OSError as e:
        print(json.dumps({"ok": False, "message": "Couldn't ask the agent: %s" % e.strerror, "results": []}))
        return 1
    result_path = os.path.join(folder, "%s.result.json" % rid)
    deadline = time.monotonic() + wait
    while time.monotonic() < deadline:
        result = load_json(result_path, None)
        if result is not None:
            try:
                os.remove(result_path)
            except OSError:
                pass
            print(json.dumps(result))
            return 0 if result.get("ok") else 1
        time.sleep(0.25)
    try:
        os.remove(os.path.join(folder, "%s.json" % rid))
    except OSError:
        pass
    print(json.dumps({"ok": False, "message": "The agent didn't answer in %d s. Is sparkbox-agent running?" % wait, "results": []}))
    return 1


def arg(args, name, default=None):
    if name in args:
        i = args.index(name)
        if i + 1 < len(args):
            return args[i + 1]
    return default


def main(argv):
    if not argv:
        print(__doc__.strip().splitlines()[-4], file=sys.stderr)
        return 2
    cmd, rest = argv[0], argv[1:]
    if cmd == "run":
        keep_notify_socket()
        Agent().run()
        return 0
    if cmd == "once":
        a = Agent()
        a.sampler = collector.Collector(gpu_every=GPU_EVERY, docker_every=DOCKER_EVERY, slow_every=SLOW_EVERY, stream_gpu=False, kernel=False)
        a.cycle()
        time.sleep(CORE_EVERY)
        a.cycle()
        a.history.flush()
        print(json.dumps(a.status()))
        return 0
    if cmd == "export":
        return cmd_export(rest)
    if cmd == "alerts":
        return cmd_since("alerts.jsonl", rest)
    if cmd == "deliveries":
        return cmd_since("deliveries.jsonl", rest)
    if cmd == "test-notify" and "--all" in rest:
        return cmd_test_notify({"all": True})
    if cmd == "test-notify" and rest:
        return cmd_test_notify({"channel": rest[0]})
    if cmd == "status":
        return cmd_status()
    if cmd == "stream":
        # The collector's own frames: the same sampler code the agent uses, one JSON line per second.
        collector.main()
        return 0
    print("unknown command: %s" % " ".join(argv), file=sys.stderr)
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except KeyboardInterrupt:
        pass
