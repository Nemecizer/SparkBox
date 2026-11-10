#!/usr/bin/env python3
"""RoCE and PHY counters for every fabric interface with an RDMA device (stdlib only).

The collector imports this module and puts `counters(iface, dev)` for each fabric
interface under that interface's "roce" key (`collect()` reads them all). Every value is cumulative (since the driver
loaded); SparkBox turns them into rates. A counter this driver doesn't expose is
left out (None), never reported as 0: absent is not the same as clean.

Sources, per interface:
  /sys/class/infiniband/<dev>/ports/1/hw_counters/   ECN, CNP, retransmit and CQE errors
  /sys/class/infiniband/<dev>/ports/1/counters/      port receive errors, discards, symbol errors, link downs
  ethtool -S <iface>                                 PFC pause per priority, PHY discards, CRC and symbol errors
  mlnx_qos -i <iface>                                trust mode and the priorities with PFC on (when installed)
  /sys/class/infiniband/<dev>/ports/1/gid_attrs/types/*   whether a RoCE v2 GID exists

ethtool and mlnx_qos are run at most every 10 s per interface; sysfs is read every call.

judge() is the node agent's roce-iface rule: one alert per fabric interface, its severity
the worst counter's and its message what the rising counters mean (the same table and the
same cases as the app's health/roce_rules.rs).
"""

import math
import os
import re
import signal
import subprocess
import threading
import time

# hw_counters file -> RoceCounters field (serde camelCase).
HW_COUNTERS = {
    "np_ecn_marked_roce_packets": "ecnMarked",
    "np_cnp_sent": "cnpSent",
    "rp_cnp_handled": "cnpHandled",
    "rp_cnp_ignored": "cnpIgnored",
    "out_of_sequence": "outOfSequence",
    "packet_seq_err": "packetSeqErr",
    "local_ack_timeout_err": "localAckTimeoutErr",
    "implied_nak_seq_err": "impliedNakSeqErr",
    "rnr_nak_retry_err": "rnrNakRetryErr",
    "duplicate_request": "duplicateRequest",
    "rx_icrc_encapsulated": "rxIcrcErr",
    "req_cqe_error": "reqCqeErr",
    "resp_cqe_error": "respCqeErr",
}

PORT_COUNTERS = {
    "port_rcv_errors": "portRcvErrors",
    "port_xmit_discards": "portXmitDiscards",
    "symbol_error": "symbolError",
    "link_downed": "linkDowned",
}

ETHTOOL_COUNTERS = {
    "rx_pause_ctrl_phy": "rxPauseCtrl",
    "tx_pause_ctrl_phy": "txPauseCtrl",
    "rx_discards_phy": "rxDiscardsPhy",
    "rx_crc_errors_phy": "rxCrcErrorsPhy",
    "rx_symbol_err_phy": "rxSymbolErrPhy",
    "rx_out_of_buffer": "rxOutOfBuffer",
}

PRIO_PAUSE = re.compile(r"^(rx|tx)_prio([0-7])_pause$")
SLOW_EVERY_S = 10
#: ethtool and mlnx_qos taking longer than this are left alone for SLOW_BACKOFF_S.
SLOW_RUN_S, SLOW_BACKOFF_S = 2.0, 300


def read_int(path):
    """A sysfs counter, or None when the file isn't there or can't be read."""
    try:
        with open(path) as f:
            return int(f.read().strip())
    except (OSError, ValueError):
        return None


def rdma_devices(sys_root="/sys"):
    """{netdev: rdma device} for every RDMA device with a network interface (RoCE)."""
    out = {}
    base = os.path.join(sys_root, "class", "infiniband")
    try:
        devs = sorted(os.listdir(base))
    except OSError:
        return out
    for dev in devs:
        net = os.path.join(base, dev, "device", "net")
        try:
            for iface in sorted(os.listdir(net)):
                out.setdefault(iface, dev)
        except OSError:
            continue
    return out


def sysfs_counters(dev, port="1", sys_root="/sys"):
    """The hw_counters and counters files this driver has, as RoceCounters fields."""
    out = {}
    root = os.path.join(sys_root, "class", "infiniband", dev, "ports", port)
    for name, key in HW_COUNTERS.items():
        v = read_int(os.path.join(root, "hw_counters", name))
        if v is not None:
            out[key] = v
    for name, key in PORT_COUNTERS.items():
        v = read_int(os.path.join(root, "counters", name))
        if v is not None:
            out[key] = v
    return out


def roce_v2(dev, port="1", sys_root="/sys"):
    """True when a RoCE v2 GID is configured, False when only v1 is, None when unknown."""
    types = os.path.join(sys_root, "class", "infiniband", dev, "ports", port, "gid_attrs", "types")
    seen = []
    try:
        names = os.listdir(types)
    except OSError:
        return None
    for n in names:
        try:
            with open(os.path.join(types, n)) as f:
                t = f.read().strip()
        except OSError:
            # Unused GID slots can't be read (EINVAL).
            continue
        if t:
            seen.append(t)
    if not seen:
        return None
    return any("v2" in t.lower() for t in seen)


def parse_ethtool(text):
    """`ethtool -S <iface>`: the pause, PHY discard, CRC and symbol counters it has.
    Per-priority pause is an 8-slot list, None where a priority isn't reported."""
    stats = {}
    for line in text.splitlines():
        name, sep, value = line.strip().partition(":")
        if not sep:
            continue
        try:
            stats[name.strip()] = int(value.strip())
        except ValueError:
            continue
    out = {}
    rx, tx = [None] * 8, [None] * 8
    traffic = [None] * 8
    any_prio = False
    for name, v in stats.items():
        byte_priority = re.fullmatch(r"rx_prio([0-7])_bytes", name)
        if byte_priority:
            traffic[int(byte_priority.group(1))] = v if v >= 0 else None
        m = PRIO_PAUSE.match(name)
        if m:
            (rx if m.group(1) == "rx" else tx)[int(m.group(2))] = v
            any_prio = True
        elif name in ETHTOOL_COUNTERS:
            out[ETHTOOL_COUNTERS[name]] = v
    if any(v is not None for v in traffic):
        out["rxPrioBytes"] = traffic
    if any_prio:
        out["rxPause"] = rx
        out["txPause"] = tx
    return out


def parse_mlnx_qos(text):
    """`mlnx_qos -i <iface>`: trust mode ("pcp"/"dscp") and the priorities with PFC enabled."""
    out = {}
    m = re.search(r"Priority trust state:\s*(\w+)", text)
    if m:
        out["trust"] = m.group(1).lower()
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if line.strip().lower().startswith("pfc configuration"):
            prios, enabled = None, None
            for nxt in lines[i + 1:i + 4]:
                words = nxt.split()
                # "priority 0 1 … 7" and "enabled 0 0 0 1 …"; the later "priority:  3" lines belong to the TCs.
                if words and words[0] == "priority" and prios is None:
                    prios = [int(x) for x in words[1:] if x.isdigit()]
                elif words and words[0] == "enabled" and enabled is None:
                    enabled = [int(x) for x in words[1:] if x.isdigit()]
            if prios and enabled and len(prios) == len(enabled):
                out["pfcPrios"] = [p for p, e in zip(prios, enabled) if e]
            break
    return out


def run(argv, timeout=3):
    """stdout of a command, or None when it isn't installed, fails or hangs. One that hangs
    (ethtool on a wedged mlx5 driver) is killed with its process group and, if it still
    won't exit, left to a thread that waits for it: the caller never does."""
    try:
        p = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             env=dict(os.environ, LC_ALL="C"), start_new_session=True)
    except OSError:
        return None
    try:
        out, _ = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            p.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            threading.Thread(target=p.wait, daemon=True, name="reaper").start()
        return None
    if p.returncode != 0:
        return None
    return out.decode("utf-8", "replace")


class Reader:
    """Keeps the slow sources (ethtool, mlnx_qos) for 10 s between runs, and for 5 minutes
    after a run that took over 2 s (a wedged driver mustn't stall every tenth frame)."""

    def __init__(self, sys_root="/sys", runner=run, clock=time.monotonic):
        self.sys_root = sys_root
        self.runner = runner
        self.clock = clock
        self.slow = {}  # iface -> (until when it's kept, dict)

    def slow_counters(self, iface):
        now = self.clock()
        cached = self.slow.get(iface)
        if cached and now < cached[0]:
            return cached[1]
        out = {}
        text = self.runner(["ethtool", "-S", iface])
        if text:
            out.update(parse_ethtool(text))
        qos = self.runner(["mlnx_qos", "-i", iface])
        if qos:
            out.update(parse_mlnx_qos(qos))
        slow = self.clock() - now > SLOW_RUN_S
        self.slow[iface] = (now + (SLOW_BACKOFF_S if slow else SLOW_EVERY_S), out)
        return out

    def counters(self, iface, dev):
        """One interface's counters, given its RDMA device (the collector already knows it)."""
        c = sysfs_counters(dev, sys_root=self.sys_root)
        c.update(self.slow_counters(iface))
        v2 = roce_v2(dev, sys_root=self.sys_root)
        if v2 is not None:
            c["roceV2"] = v2
        return c

    def collect(self, ifaces=None):
        """{iface: counters} for each interface with an RDMA device (only `ifaces`, if given)."""
        out = {}
        for iface, dev in rdma_devices(self.sys_root).items():
            if ifaces is not None and iface not in ifaces:
                continue
            out[iface] = self.counters(iface, dev)
        return out


# ---- One RoCE alert per interface (the node agent's roce-iface rule)

#: health/fixtures/roce-iface.json without its comment and cases (roce_test.py checks they
#: agree): the counters roce-iface watches, their default lines, and what each group means.
IFACE_TABLE = {
    "rule": "roce-iface", "rateWindowS": 10, "sustainWindowS": 60, "pauseStormS": 30,
    "counters": [
        {"key": "outOfSequence", "label": "Out of sequence", "group": "loss", "sustained": True, "warn": 1, "crit": 100},
        {"key": "packetSeqErr", "label": "Sequence errors", "group": "loss", "sustained": True, "warn": 1, "crit": 100},
        {"key": "localAckTimeoutErr", "label": "ACK timeouts", "group": "loss", "sustained": True, "warn": 1, "crit": 20},
        {"key": "impliedNakSeqErr", "label": "Implied NAKs", "group": "loss", "sustained": True, "warn": 1, "crit": 100},
        {"key": "rxCrcErrorsPhy", "label": "CRC errors", "group": "phy", "sustained": False, "warn": 0, "crit": 10},
        {"key": "rxSymbolErrPhy", "label": "Symbol errors (PHY)", "group": "phy", "sustained": False, "warn": 0, "crit": 10},
        {"key": "symbolError", "label": "Symbol errors (port)", "group": "phy", "sustained": False, "warn": 0, "crit": 10},
        {"key": "portRcvErrors", "label": "Receive errors", "group": "phy", "sustained": False, "warn": 0, "crit": 10},
        {"key": "rxDiscardsPhy", "label": "Discards (PHY)", "group": "drop", "sustained": True, "warn": 1, "crit": None},
        {"key": "rxOutOfBuffer", "label": "Out of buffer", "group": "drop", "sustained": True, "warn": 1, "crit": None},
        {"key": "portXmitDiscards", "label": "Transmit discards", "group": "drop", "sustained": True, "warn": 1, "crit": None},
        {"key": "txPause", "label": "Pause sent", "group": "pause", "sustained": True, "warn": 10000, "crit": None},
        {"key": "rxPause", "label": "Pause received", "group": "pause", "sustained": True, "warn": 10000, "crit": None},
    ],
    "groups": {
        "loss": {"headline": "RoCE retransmitting", "cause": "packets are being dropped: congestion or PFC off"},
        "phy": {"headline": "PHY errors rising", "cause": "check the optic or cable"},
        "drop": {"headline": "Dropping packets", "cause": "packets are being dropped: congestion or PFC off"},
        "pause": {"headline": "PFC pause storm", "cause": "find the node that sends the pause frames"},
    },
    "causeOrder": ["phy", "pause", "loss", "drop"],
    "info": ["ecnMarked", "cnpSent", "cnpHandled"],
}
#: Pause frames past this many a second (sent plus received) are a storm (fabric::PAUSE_STORM_PER_S).
PAUSE_STORM_PER_S = 10_000.0
SLACK_MS, MIN_SPAN_MS = 500, 2_000


def pause_total(c, direction):
    """Pause frames sent ("tx") or received ("rx"): the per-priority sum, else the PHY total."""
    prios = c.get(direction + "Pause")
    prios = [v for v in prios if isinstance(v, (int, float))] if isinstance(prios, list) else []
    if prios:
        return sum(prios)
    v = c.get(direction + "PauseCtrl")
    return v if isinstance(v, (int, float)) else None


def totals(c):
    """{counter key: cumulative value} for the counters roce-iface reads; absent ones left out."""
    out = {}
    for k in [d["key"] for d in IFACE_TABLE["counters"]] + IFACE_TABLE["info"]:
        v = pause_total(c, k[:2]) if k in ("txPause", "rxPause") else c.get(k)
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            out[k] = v
    return out


class IfaceRates:
    """One interface's counter samples, as fabric::rates keeps them: per-second rates over
    the last 10 s and the last full minute (None, never 0, without enough history or the
    counter), and how long pause frames have stormed. A counter that went back (a driver
    reload) starts the history again."""

    def __init__(self):
        self.samples = []  # [(ms, totals)]
        self.pause_since = None

    def add(self, ts, counters):
        cur = totals(counters or {})
        if self.samples and any(k in self.samples[-1][1] and cur[k] < self.samples[-1][1][k] for k in cur):
            self.samples, self.pause_since = [], None
        if self.samples:
            last_ts, last = self.samples[-1]
            dt = (ts - last_ts) / 1000.0
            if dt > 0:
                pause = lambda t: (t.get("txPause") or 0) + (t.get("rxPause") or 0)
                if max(pause(cur) - pause(last), 0) / dt > PAUSE_STORM_PER_S:
                    if self.pause_since is None:
                        self.pause_since = last_ts
                else:
                    self.pause_since = None
        self.samples.append((ts, cur))
        keep = IFACE_TABLE["sustainWindowS"] * 1000 + 15_000
        while len(self.samples) > 2 and ts - self.samples[0][0] > keep:
            self.samples.pop(0)

    def base(self, now, window, min_span):
        """The first sample within `window` of now, if the span to the latest is at least `min_span`."""
        if not self.samples:
            return None
        last_ts = self.samples[-1][0]
        for s in self.samples:
            if now - s[0] <= window + SLACK_MS:
                return s if last_ts - s[0] >= min_span else None
        return None

    def rates(self, now):
        """{key: (per second over 10 s, per second over the last full minute)}."""
        if not self.samples:
            return {}
        last_ts, last = self.samples[-1]
        short = self.base(now, IFACE_TABLE["rateWindowS"] * 1000, MIN_SPAN_MS)
        full = self.base(now, IFACE_TABLE["sustainWindowS"] * 1000, IFACE_TABLE["sustainWindowS"] * 1000 - 5_000)

        def per_s(b, k):
            if b is None or k not in b[1]:
                return None
            dt = (last_ts - b[0]) / 1000.0
            return max(last[k] - b[1][k], 0) / dt if dt > 0 else None
        return {k: (per_s(short, k), per_s(full, k)) for k in last}

    def storm_s(self, now):
        return None if self.pause_since is None else (now - self.pause_since) // 1000


def fmt_per_s(v):
    """"162/s", "4.1/s", "0.004/s", "0/s": real precision (fabric::rates::fmt_per_s)."""
    if v <= 0:
        return "0/s"
    if v >= 10_000:
        return "%.0fk/s" % (v / 1000)
    if v >= 1_000:
        return "%.1fk/s" % (v / 1000)
    if v >= 10:
        return "%.0f/s" % v
    if v >= 1:
        return "%.1f/s" % v
    digits = int(-math.floor(math.log10(v))) + 1
    return ("%.*f" % (digits, v)).rstrip("0").rstrip(".") + "/s"


def default_lines():
    return [{"key": c["key"], "label": c["label"], "warn": c["warn"], "crit": c["crit"], "enabled": True} for c in IFACE_TABLE["counters"]]


def phy_words(keys):
    crc = "rxCrcErrorsPhy" in keys
    sym = any("symbol" in k.lower() for k in keys)
    return {(True, True): "CRC and symbol errors rising", (True, False): "CRC errors rising",
            (False, True): "Symbol errors rising"}.get((crc, sym), "PHY errors rising")


def judge(lines, rates, storm_s, active):
    """One interface against the rule's counter lines (the app's health/roce_rules.rs
    judge): None, or (severity, message, cause, detail, worst rate). To raise, a counter
    passes its line, a sustained one averaging past it for a minute too and pause frames
    storming for 30 s. Once raised it holds while any loss, PHY or drop counter still grows
    (pause frames while past their line)."""
    t = IFACE_TABLE
    defs = {c["key"]: c for c in t["counters"]}
    hits = []
    for line in lines or default_lines():
        d = defs.get(line.get("key"))
        if d is None or line.get("enabled") is False:
            continue
        r10, r60 = rates.get(d["key"], (None, None))
        if r10 is None:
            continue
        warn = line["warn"] if line.get("warn") is not None else math.inf
        if d["group"] == "pause":
            lasted = storm_s is not None and storm_s >= t["pauseStormS"]
        else:
            lasted = not d["sustained"] or (r60 is not None and r60 > warn)
        raised = r10 > warn and lasted
        held = active and (r10 > warn if d["group"] == "pause" else r10 > 0)
        if not (raised or held):
            continue
        hits.append((d, line, r10, "crit" if line.get("crit") is not None and r10 > line["crit"] else "warn"))
    if not hits:
        return None
    severity = "crit" if any(h[3] == "crit" for h in hits) else "warn"
    rising = lambda g: any(h[0]["group"] == g for h in hits)
    cause = t["groups"].get(next((g for g in t["causeOrder"] if rising(g)), hits[0][0]["group"]), {}).get("cause", "")
    # The headline is what hurts the traffic most; PHY errors, when they rise, say why.
    lead = next(g for g in ("loss", "pause", "drop", "phy") if rising(g))
    phy = [h[0]["key"] for h in hits if h[0]["group"] == "phy"]
    parts = [phy_words(phy)] if lead == "phy" else [t["groups"][lead]["headline"]] + ([phy_words(phy)] if phy else [])
    if not any(p.lower() == cause.lower() for p in parts):
        parts.append(cause)
    ordered = sorted(hits, key=lambda h: -h[2])
    detail = " · ".join("%s %s" % (h[1].get("label") or h[0]["label"], fmt_per_s(h[2])) for h in ordered)
    return severity, " · ".join(parts), cause, detail, ordered[0][2]


def half(iface, ip, fabric):
    """1 or 2: the ConnectX half by its subnet (192.168.100.x, .101.x), else by order (fabric::half_of)."""
    if ip and ip.startswith("192.168.101."):
        return 2
    if ip and ip.startswith("192.168.100."):
        return 1
    return (fabric.index(iface) % 2) + 1 if iface in fabric else 1


_reader = None


def _shared():
    global _reader
    if _reader is None:
        _reader = Reader()
    return _reader


def collect(ifaces=None):
    """{iface: counters} for every RDMA-backed interface, from the shared reader."""
    return _shared().collect(ifaces)


def counters(iface, dev):
    """The collector's entry point: `roce.counters("enp1s0f1np1", "rocep1s0f1")`, every frame."""
    return _shared().counters(iface, dev)


if __name__ == "__main__":
    import json
    print(json.dumps(collect(), indent=2, sort_keys=True))
