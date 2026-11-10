#!/usr/bin/env python3
"""SparkBox collector.

SparkBox copies this file to ~/.sparkbox/collector.py on each Spark and runs it
over SSH. It prints one JSON line per second describing the node and exits as
soon as SparkBox disconnects. It only reads /proc, /sys, nvidia-smi, ethtool,
the kernel log, systemctl and the Docker socket; it never changes anything.

`sample()` takes one reading; the always-on agent (sparkbox_agent.py, next to
this file in /opt/sparkbox-agent) imports it, so both see the node the same way.

Nothing here can hang the caller. Every command runs with a deadline, its process group
is killed when it passes, and a child that still won't exit (stuck in the kernel, D
state) is left to a reaper thread. The GPU is read in a thread of its own: when
nvidia-smi or NVML stops answering (an Xid 79, a wedged driver), the frame's GPU numbers
are null, never 0, and gpu.error says why ("nvidia-smi didn't answer in 10 s").

Frames carry bootId (/proc/sys/kernel/random/boot_id). The kernel log is read from the
start of the current boot and then followed, so a fault from before SparkBox connected is
reported once, with its own time.

Processes: every userland process, cut to the 200 busiest, always keeping GPU users,
listeners, model servers and the system daemons an operator looks for (`DAEMONS`), plus the 10
busiest kernel threads (marked kernel) and SparkBox's own (marked sparkbox). A process's
full name comes from argv[0] where the kernel's 15-character comm cuts it.

nvidia-smi runs one at a time: while one SparkBox started is stuck in the kernel no other is
started (the GPU reads "nvidia-smi is stuck in the kernel"), and a query that timed out is
tried again after 10 s, then 20, 40 … up to 5 minutes.

Wake-on-LAN, Wi-Fi and LLDP are read without root: ethtool can't show Wake-on to a normal
user, so NetworkManager's setting is read instead; `iw dev <if> link` and lldpcli answer
anyone (lldpcli only when lldpd lets the user in; otherwise there's no LLDP). A neighbour that
is this node itself (each ConnectX-7 half hears its sibling through the switch) is dropped.

SPARKBOX_PROC_ROOT, SPARKBOX_SYSFS_ROOT and SPARKBOX_ROOT (/etc, /var/run) point it at copies
of those trees, and SPARKBOX_DOCKER_SOCK at another Docker socket (the tests and the fake
Spark use fixture trees).
"""
import calendar
import collections
import glob
import hashlib
import http.client
import json
import os
import pwd
import re
import signal
import socket
import subprocess
import sys
import threading
import time

# roce.py (RoCE and PHY counters) ships next to this file when it's there; run with
# python3 -I the script's own folder isn't on the path, so put it there.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import roce  # noqa: E402
except ImportError:
    roce = None

#: Frame schema: 4 adds bootId and null GPU numbers (with gpu.error) when the GPU can't be read;
#: 5 adds procTotal, the comm/kernel/sparkbox process flags, listener scope and family, Wi-Fi,
#: LLDP, Wake-on-LAN as an object and memory/IO pressure; 6 adds each disk's avail (df's
#: Available) and every LLDP neighbour that isn't this node (lldpPeers, lldpSelfOnly).
VERSION = 6
# Identifies this loaded collector's source, independently of the frame schema.
try:
    with open(__file__, "rb") as _source:
        COLLECTOR_BUILD = hashlib.sha256(_source.read()).hexdigest()[:12]
except OSError:
    COLLECTOR_BUILD = None
GPU_EVERY = 2.0
DOCKER_EVERY = 3.0
INSPECT_EVERY = 10.0
UNITS_EVERY = 10.0
SLOW_EVERY = 30.0
PROC_EVERY = 5.0
WOL_EVERY = 60.0
#: Userland processes sent at most (the busiest, after the ones always kept).
MAX_PROCESSES = 200
#: Kernel threads sent (the busiest), for "Show kernel threads".
KERNEL_THREADS = 10
#: Characters of a command line sent.
CMD_CHARS = 240
#: Processes always sent: model servers (by command line) and the daemons an operator looks for.
SERVER_PATTERNS = ("vllm serve", "python -m vllm", "python3 -m vllm", "sglang", "ollama serve", "llama-server", "trtllm-serve",
                   "tritonserver", "ray::", "ray/", "raylet", "gcs_server")
DAEMONS = frozenset(("dockerd", "containerd", "sshd", "nginx", "ollama", "dashboard-service", "dashboard-admin", "systemd-journald",
                     "NetworkManager", "lldpd", "earlyoom", "xrdp", "xrdp-sesman", "cupsd", "master", "avahi-daemon"))
#: nvidia-smi after a timeout: tried again this long after, doubling up to NVIDIA_SMI_BACKOFF_MAX.
NVIDIA_SMI_BACKOFF = 10.0
NVIDIA_SMI_BACKOFF_MAX = 300.0
#: Kernel events kept (and sent with each frame), besides the fatal Xids of this boot.
KERNEL_KEEP = 50
#: A GPU reading older than this means the GPU (nvidia-smi or NVML) stopped answering.
GPU_DEADLINE = 10.0
#: How long the first frame waits for the first GPU reading.
GPU_FIRST_WAIT = 3.0
#: After killing a command that ran out of time, how long to wait before leaving it to the reaper.
KILL_WAIT = 2.0
GPU_FIELDS = "utilization.gpu,temperature.gpu,power.draw,clocks.sm,driver_version,clocks.max.sm,power.limit,pstate"
#: Give up when a write to SparkBox has been stuck this long (the SSH channel is gone).
WRITE_STALL = 10.0
CLK_TCK = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
UNIT = re.compile(r"/([^/]+\.(?:service|scope))$", re.M)
REAL_FS = {"ext4", "xfs", "btrfs", "vfat", "f2fs", "zfs"}
CGROUP_ID = re.compile(r"docker[-/]([0-9a-f]{64})")
PROC = os.environ.get("SPARKBOX_PROC_ROOT", "/proc").rstrip("/") or "/proc"
SYS = os.environ.get("SPARKBOX_SYSFS_ROOT", "/sys").rstrip("/") or "/sys"
ROOT = os.environ.get("SPARKBOX_ROOT", "").rstrip("/")
DOCKER_SOCK = os.environ.get("SPARKBOX_DOCKER_SOCK") or ROOT + "/var/run/docker.sock"

#: nvidia-smi clocks_event_reasons bits, in the words nvidia-smi -q uses.
#: GPU Xid errors: severity, name, meaning, next step. A copy of
#: src-tauri/src/health/xid.json (collector_test.py checks they agree).
XID = {
    13: ("warn", "Graphics engine exception"), 31: ("warn", "GPU memory page fault"), 43: ("warn", "GPU stopped processing"),
    45: ("warn", "Preemptive cleanup"), 48: ("crit", "Double-bit ECC error"), 61: ("warn", "Micro-controller breakpoint"),
    62: ("crit", "Micro-controller halt"), 63: ("warn", "Memory page retired"), 64: ("crit", "Memory retirement failed"),
    68: ("warn", "Video decoder exception"), 69: ("warn", "Graphics engine class error"), 74: ("crit", "NVLink error"),
    79: ("crit", "GPU fallen off the bus"), 92: ("warn", "High single-bit ECC rate"), 94: ("crit", "Contained memory error"),
    95: ("crit", "Uncontained memory error"), 109: ("crit", "Context switch timeout"), 119: ("crit", "GSP timeout"),
    120: ("crit", "GSP error"), 154: ("crit", "GPU recovery required"),
}
#: Xids after which the GPU needs a reset ("latch" in xid.json): reported until the node restarts.
FATAL_XID = frozenset((48, 79, 94, 95, 119, 120, 154))
#: Model servers whose OOM kill is critical.
MODEL_SERVERS = ("vllm", "ray", "ollama", "sglang", "trtllm", "tritonserver")

XID_RE = re.compile(r"NVRM: Xid \(PCI:([^)]+)\): (\d+),\s*(.*)")
MLX5_RE = re.compile(r"mlx5_core\b.*?(module (?:is )?(?:absent|unplugged)|unplugged|(?:cmd|wait_func)\S*.*?(?:timeout|no done completion)|"
                     r"command timeout|health compromised|link down|fw fatal)", re.I)
OOM_RE = re.compile(r"Out of memory: Killed process (\d+) \((.+?)\)")
OOM_KILL_RE = re.compile(r"oom-kill:.*?task_memcg=([^,]*),task=([^,]+),pid=(\d+)")
HUNG_RE = re.compile(r"INFO: task (.+?):(\d+) blocked for more than (\d+) seconds")

THROTTLE_BITS = [
    (0x1, "GPU idle"),
    (0x2, "Applications clocks setting"),
    (0x4, "SW power cap"),
    (0x8, "HW slowdown"),
    (0x10, "Sync boost"),
    (0x20, "SW thermal slowdown"),
    (0x40, "HW thermal slowdown"),
    (0x80, "HW power brake slowdown"),
    (0x100, "Display clock setting"),
]


def proc(path):
    """A path under /proc (or SPARKBOX_PROC_ROOT): proc("net/dev")."""
    return PROC + "/" + path


def sysfs(path):
    """A path under /sys (or SPARKBOX_SYSFS_ROOT): sysfs("class/net")."""
    return SYS + "/" + path


def read(path, default=""):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return default


def read_int(path):
    try:
        return int(read(path).strip())
    except ValueError:
        return None


def run_ex(cmd, timeout=8):
    """(status, stdout, stderr) of a command; status is ok, failed (non-zero exit), missing
    (not installed) or timeout. A command past its deadline has its process group killed;
    one that still won't exit within 2 s (stuck in the kernel) goes to the reaper, so the
    caller never waits on it."""
    try:
        p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             errors="replace", start_new_session=True)
    except OSError:
        return "missing", None, None
    try:
        out, err = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        kill_group(p)
        try:
            p.communicate(timeout=KILL_WAIT)
        except subprocess.TimeoutExpired:
            reap(p)
        return "timeout", None, None
    return ("ok" if p.returncode == 0 else "failed"), out, err


def run(cmd, timeout=8):
    """stdout of a command, or None when it fails, isn't installed or hangs (run_ex)."""
    status, out, _ = run_ex(cmd, timeout)
    return out if status == "ok" else None


def kill_group(p):
    try:
        os.killpg(p.pid, signal.SIGKILL)
    except OSError:
        try:
            p.kill()
        except OSError:
            pass


#: Children handed to the reaper that haven't exited yet, by program name.
_stuck = collections.Counter()
_stuck_lock = threading.Lock()


def stuck(name):
    """How many children called `name` are still stuck after their deadline."""
    with _stuck_lock:
        return _stuck[name]


def reap(p):
    """Waits for a killed child in a thread of its own: one stuck in the kernel exits only
    when its call returns, and the caller mustn't wait for that. The parent's ends of its
    pipes are closed first, so nothing is held open while it's stuck; `stuck()` counts it
    until it exits."""
    name = os.path.basename(str(p.args[0] if isinstance(p.args, (list, tuple)) else p.args))
    for f in (p.stdin, p.stdout, p.stderr):
        try:
            if f:
                f.close()
        except OSError:
            pass
    with _stuck_lock:
        _stuck[name] += 1

    def wait():
        try:
            p.wait()
        finally:
            with _stuck_lock:
                _stuck[name] -= 1
    threading.Thread(target=wait, daemon=True, name="reaper").start()


# ---- Parsers: pure functions over file/command text (see collector_test.py).

def parse_stat(text):
    """CPU lines of /proc/stat -> {"cpu": (busy, total), "cpu0": ...} in jiffies."""
    out = {}
    for line in text.splitlines():
        if not line.startswith("cpu"):
            continue
        parts = line.split()
        vals = [int(v) for v in parts[1:9]]  # user nice system idle iowait irq softirq steal
        idle = vals[3] + vals[4]
        total = sum(vals)
        out[parts[0]] = (total - idle, total)
    return out


def cpu_percent(prev, cur):
    """(total %, [core 0 %, core 1 %, ...]) between two parse_stat results."""
    def pct(name):
        if name not in prev or name not in cur:
            return 0.0
        busy = cur[name][0] - prev[name][0]
        total = cur[name][1] - prev[name][1]
        return round(100.0 * busy / total, 1) if total > 0 else 0.0
    cores = sorted((n for n in cur if n != "cpu"), key=lambda n: int(n[3:]))
    return pct("cpu"), [pct(n) for n in cores]


def parse_meminfo(text):
    out = {}
    for line in text.splitlines():
        key, _, rest = line.partition(":")
        fields = rest.split()
        if fields:
            out[key.strip()] = int(fields[0]) * (1024 if len(fields) > 1 else 1)
    return out


def memory(info):
    total = info.get("MemTotal", 0)
    available = info.get("MemAvailable", 0)
    swap_total = info.get("SwapTotal", 0)
    return {
        "total": total,
        "used": max(total - available, 0),
        "available": available,
        "cached": info.get("Cached", 0) + info.get("Buffers", 0) + info.get("SReclaimable", 0),
        "gpuAlloc": 0,
        "swapTotal": swap_total,
        "swapUsed": max(swap_total - info.get("SwapFree", 0), 0),
    }


def swap_pages_in(text):
    """/proc/vmstat pswpin, or unknown; a missing counter is never a zero reading."""
    for line in text.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[0] == "pswpin":
            try:
                value = int(fields[1])
                return value if value >= 0 else None
            except ValueError:
                return None
    return None


def swap_in_rate(previous, current, dt, page_size):
    if previous is None or current is None or current < previous or dt <= 0 or dt > 15:
        return None
    return (current - previous) * page_size / dt


def parse_net_dev(text):
    """/proc/net/dev -> {iface: {rx, tx, rxErrors, rxDrops, txErrors, txDrops}} (bytes and packet counts)."""
    out = {}
    for line in text.splitlines()[2:]:
        name, _, rest = line.partition(":")
        f = rest.split()
        if len(f) >= 12:
            out[name.strip()] = {"rx": int(f[0]), "rxErrors": int(f[2]), "rxDrops": int(f[3]),
                                 "tx": int(f[8]), "txErrors": int(f[10]), "txDrops": int(f[11])}
    return out


def counter_delta(prev, cur):
    """Growth of a hardware counter between two reads. A 32-bit counter that wrapped is
    unwrapped; anything else that went backwards was reset (driver reload), so it's 0."""
    if prev is None or cur is None:
        return 0
    if cur >= prev:
        return cur - prev
    if prev < 2 ** 32:
        return cur + 2 ** 32 - prev
    return 0


def rdma_device(iface):
    """The RDMA device behind a netdev (e.g. enp1s0f1np1 -> rocep1s0f1), or None."""
    found = sorted(glob.glob(sysfs("class/net/%s/device/infiniband/*" % iface)))
    return os.path.basename(found[0]) if found else None


def rdma_bytes(dev, port=1):
    """(received, sent) RDMA bytes on one port. The InfiniBand data counters count 4-byte
    words; the 64-bit extended counters are used when the kernel has them."""
    base = sysfs("class/infiniband/%s/ports/%d" % (dev, port))
    def one(name):
        for path in ("%s/counters_ext/%s_64" % (base, name), "%s/counters/%s" % (base, name)):
            v = read_int(path)
            if v is not None:
                return v * 4
        return None
    return one("port_rcv_data"), one("port_xmit_data")


def rdma_rate(prev, cur, dt):
    """(rx bits/s, tx bits/s) between two rdma_bytes reads, or (None, None) without counters."""
    if cur[0] is None and cur[1] is None:
        return None, None
    def rate(a, b):
        if b is None:
            return None
        if a is None:
            return 0.0
        # Counters count 4-byte words; a wrap is at 2^32 words.
        return counter_delta(a // 4, b // 4) * 4 * 8 / dt
    return rate(prev[0], cur[0]), rate(prev[1], cur[1])


def parse_diskstats(text):
    """/proc/diskstats -> {device: (bytes read, bytes written)}."""
    out = {}
    for line in text.splitlines():
        f = line.split()
        if len(f) >= 10:
            out[f[2]] = (int(f[5]) * 512, int(f[9]) * 512)
    return out


def disk_space(st):
    """A statvfs result as df -B1 reports it: total, used (blocks not free) and avail, the space
    a normal user can still write (f_bavail; ext4 keeps 5% back for root, so free is not
    total minus used). df's Use% is used / (used + avail)."""
    return {"total": st.f_blocks * st.f_frsize, "used": (st.f_blocks - st.f_bfree) * st.f_frsize, "avail": st.f_bavail * st.f_frsize}


def parse_mounts(text):
    """Real filesystems from /proc/mounts as (device, mount point, type), first mount per device."""
    seen, out = set(), []
    for line in text.splitlines():
        f = line.split()
        if len(f) >= 3 and f[2] in REAL_FS and f[0].startswith("/dev/") and f[0] not in seen:
            seen.add(f[0])
            out.append((f[0], f[1].replace("\\040", " "), f[2]))
    return out


def number(field):
    """nvidia-smi fields such as '73', '45.10', '[N/A]', '[Not Supported]' -> float or None."""
    try:
        return float(field.strip())
    except ValueError:
        return None


#: The GPU numbers when the GPU can't be read: unknown, never 0.
NO_GPU = {"utilPct": None, "tempC": None, "powerW": None, "smClockMhz": None, "driver": ""}
NO_GPU_EXTRA = {"clockMaxMhz": None, "powerLimitW": None, "pstate": None, "throttle": []}


def parse_gpu(csv):
    """utilization.gpu, temperature.gpu, power.draw, clocks.sm, driver_version."""
    line = next((l for l in csv.splitlines() if l.strip()), "")
    f = [x.strip() for x in line.split(",")]
    f += [""] * (5 - len(f))
    return {
        "utilPct": number(f[0]) or 0.0,
        "tempC": number(f[1]) or 0.0,
        "powerW": number(f[2]) or 0.0,
        "smClockMhz": int(number(f[3]) or 0),
        "driver": f[4],
    }


def decode_throttle(field):
    """'0x0000000000000044' -> ['SW power cap', 'HW thermal slowdown']; [N/A] -> []."""
    try:
        bits = int(field.strip(), 16)
    except ValueError:
        return []
    return [name for bit, name in THROTTLE_BITS if bits & bit]


def parse_gpu_extra(csv):
    """clocks.max.sm, power.limit, pstate, clocks_event_reasons.active; [N/A] fields become None."""
    line = next((l for l in csv.splitlines() if l.strip()), "")
    f = [x.strip() for x in line.split(",")]
    f += [""] * (4 - len(f))
    clock, limit = number(f[0]), number(f[1])
    return {
        "clockMaxMhz": int(clock) if clock else None,
        "powerLimitW": limit if limit else None,
        "pstate": f[2] if f[2].startswith("P") else None,
        "throttle": decode_throttle(f[3]),
    }


def parse_apps(csv):
    """pid, process_name, used_memory (MiB) -> [{pid, name, mem bytes}]."""
    out = []
    for line in csv.splitlines():
        f = [x.strip() for x in line.split(",")]
        if len(f) >= 3 and f[0].isdigit():
            mib = number(f[2])
            out.append({"pid": int(f[0]), "name": os.path.basename(f[1]), "mem": int(mib * 1048576) if mib else 0})
    return out


def parse_failed_units(text):
    """`systemctl --failed --output=json` (or its plain text) -> ['x.service', ...]."""
    text = (text or "").strip()
    if not text:
        return []
    try:
        return sorted(u["unit"] for u in json.loads(text) if u.get("unit"))
    except (ValueError, TypeError, KeyError):
        pass
    out = []
    for line in text.splitlines():
        f = line.replace("●", " ").split()
        if f and "." in f[0] and "failed" in f:
            out.append(f[0])
    return sorted(out)


def container_of(cgroup_text):
    """Docker container id from a /proc/<pid>/cgroup file, or None."""
    m = CGROUP_ID.search(cgroup_text)
    return m.group(1) if m else None


def unit_of(cgroup_text):
    """The systemd unit a process runs in (e.g. ollama.service), or None for sessions."""
    for line in cgroup_text.splitlines():
        m = UNIT.search(line.strip())
        if m and not m.group(1).startswith(("session-", "docker-", "user@")):
            return m.group(1)
    return None


def parse_pid_stat(text):
    """/proc/<pid>/stat -> {name, state, ppid, ticks, nice, threads, start, rss} or None.
    The name may contain spaces and parens; start is field 22 (clock ticks after boot)."""
    left, right = text.find("("), text.rfind(")")
    if left < 0 or right < 0:
        return None
    rest = text[right + 2:].split()
    try:
        return {"name": text[left + 1:right], "state": rest[0], "ppid": int(rest[1]),
                "ticks": int(rest[11]) + int(rest[12]), "nice": int(rest[16]), "threads": int(rest[17]),
                "start": int(rest[19]), "rss": int(rest[21])}
    except (IndexError, ValueError):
        return None


def ipv6_text(addr_hex):
    """/proc/net/tcp6's address (four little-endian 32-bit words) -> its usual text form."""
    try:
        raw = b"".join(bytes.fromhex(addr_hex[i:i + 8])[::-1] for i in range(0, 32, 8))
        if raw[:12] == b"\0" * 10 + b"\xff\xff":
            return ".".join(str(b) for b in raw[12:])  # an IPv4-mapped address
        return socket.inet_ntop(socket.AF_INET6, raw)
    except (ValueError, OSError):
        return "ipv6"


def parse_listeners(text):
    """{socket inode: (address, port)} for listening TCP sockets in /proc/net/tcp or tcp6."""
    out = {}
    for line in text.splitlines()[1:]:
        f = line.split()
        if len(f) < 10 or f[3] != "0A":
            continue
        addr_hex, port_hex = f[1].rsplit(":", 1)
        if len(addr_hex) == 8:
            addr = ".".join(str(int(addr_hex[i:i + 2], 16)) for i in (6, 4, 2, 0))
        else:
            addr = ipv6_text(addr_hex)
        out[f[9]] = (addr, int(port_hex, 16))
    return out


def listen_scope(addr):
    """Who can reach a listening address: local (loopback), all (every interface), fabric
    (192.168.100-101, the ConnectX-7 links) or lan (any other address of the node)."""
    if addr in ("0.0.0.0", "::", "*"):
        return "all"
    if addr.startswith("127.") or addr == "::1":
        return "local"
    if addr.startswith(("192.168.100.", "192.168.101.")):
        return "fabric"
    return "lan"


SCOPE_ORDER = {"local": 0, "fabric": 1, "lan": 2, "all": 3}


def fold_listeners(sockets, owner):
    """One listener per port: the same port on IPv4 and IPv6 (0.0.0.0 and ::, or 127.0.0.1
    and ::1) is one row with family "both", and the widest scope wins."""
    by_port = {}
    for inode, (addr, port) in sockets.items():
        by_port.setdefault(port, []).append((inode, addr))
    out = []
    for port in sorted(by_port):
        socks = by_port[port]
        v4 = [a for _, a in socks if ":" not in a]
        v6 = [a for _, a in socks if ":" in a]
        scope = max((listen_scope(a) for _, a in socks), key=SCOPE_ORDER.get)
        # The address shown: the widest one, IPv4 first.
        addr = sorted((a for _, a in socks), key=lambda a: (-SCOPE_ORDER[listen_scope(a)], ":" in a, a))[0]
        pid, name = next((owner[i] for i, _ in socks if i in owner), (None, None))
        out.append({"port": port, "addr": addr, "pid": pid, "process": name, "scope": scope,
                    "family": "both" if v4 and v6 else "v6" if v6 else "v4"})
    return out


def parse_clock_offset(chrony):
    """`chronyc tracking` -> offset from NTP time in ms (signed), or None."""
    m = re.search(r"System time\s*:\s*([\d.]+) seconds (fast|slow)", chrony or "")
    if not m:
        return None
    ms = float(m.group(1)) * 1000
    return round(ms if m.group(2) == "fast" else -ms, 3)


def parse_timesync_offset(text):
    """`timedatectl timesync-status` -> the offset from the time server in ms, or None.
    Offset is printed like "+1.234ms", "-345us" or "+2.1s"."""
    m = re.search(r"^\s*Offset:\s*([+-]?[\d.]+)\s*(us|µs|ms|s)\b", text or "", re.M)
    if not m:
        return None
    return round(float(m.group(1)) * {"us": 0.001, "µs": 0.001, "ms": 1.0, "s": 1000.0}[m.group(2)], 3)


def parse_wol(text):
    """`ethtool <iface>` -> the Wake-on setting ("g" magic packet, "d" off), or None (a
    normal user gets "Cannot get wake-on-lan settings: Operation not permitted")."""
    for line in (text or "").splitlines():
        line = line.strip()
        if line.startswith("Wake-on:"):
            return line.split(":", 1)[1].strip() or None
    return None


#: Wake-on-LAN that can't be read: a normal user can't run ethtool's query, and NetworkManager
#: either isn't asked or leaves the setting to the driver.
WOL_NEEDS_ROOT = "Not readable without sudo (ethtool needs root)"
WOL_DRIVER_DEFAULT = "Not readable without sudo. NetworkManager leaves it to the driver default."


def wol_info(ethtool_letters, nm_value):
    """Wake-on-LAN as an object: ethtool's letters when it could read them, else NetworkManager's
    802-3-ethernet.wake-on-lan (None when nmcli couldn't say)."""
    if ethtool_letters:
        on = "g" in ethtool_letters
        state = "on" if on else "off" if ethtool_letters == "d" else "unknown"
        detail = "On (magic packet)" if on else "Off" if state == "off" else "Wake-on: %s" % ethtool_letters
        return {"state": state, "source": "ethtool", "detail": detail, "raw": ethtool_letters}
    if nm_value is None:
        return {"state": "unknown", "source": "none", "detail": WOL_NEEDS_ROOT}
    flags = {f.strip().lower() for f in nm_value.replace("|", ",").split(",") if f.strip()}
    if "magic" in flags:
        return {"state": "on", "source": "networkmanager", "detail": "On (NetworkManager: magic packet)"}
    if flags & {"none", "disable", "disabled", "off"}:
        return {"state": "off", "source": "networkmanager", "detail": "Off (NetworkManager)"}
    return {"state": "unknown", "source": "networkmanager", "detail": WOL_DRIVER_DEFAULT}


def parse_iw_link(text):
    """`iw dev <if> link` -> {ssid, signalDbm, bitrateMbps} (None where iw didn't say), or None
    when it isn't associated."""
    if not text or text.strip().startswith("Not connected"):
        return None
    ssid = re.search(r"^\s*SSID:\s*(.*)$", text, re.M)
    signal = re.search(r"^\s*signal:\s*(-?\d+)\s*dBm", text, re.M)
    rate = re.search(r"^\s*tx bitrate:\s*([\d.]+)\s*MBit/s", text, re.M)
    if not (ssid or signal or rate):
        return None
    return {"ssid": ssid.group(1).strip() if ssid else None, "signalDbm": int(signal.group(1)) if signal else None,
            "bitrateMbps": float(rate.group(1)) if rate else None}


def parse_lldp(text):
    """`lldpcli -f json show neighbors` -> {interface: [{system, port, chassisId, portId}]}, every
    neighbour heard on each interface. lldpcli prints one neighbour as an object and several as a
    list, at each level. `port` is what to show (the port's name: its description when its id is
    a MAC, as lldpd's own are); `chassisId` and `portId` are the ids as sent."""
    try:
        data = json.loads(text or "")
    except ValueError:
        return {}
    ifaces = (data.get("lldp") or {}).get("interface") if isinstance(data, dict) else None
    items = []
    if isinstance(ifaces, dict):
        items = list(ifaces.items())
    elif isinstance(ifaces, list):
        for entry in ifaces:
            if isinstance(entry, dict):
                items += list(entry.items())
    out = {}
    for name, info in items:
        if not isinstance(info, dict):
            continue
        chassis = info.get("chassis") or {}
        if not isinstance(chassis, dict):
            chassis = {}
        if "id" in chassis:  # no system name: the chassis object itself
            system, body = (chassis.get("name") or {}).get("value") if isinstance(chassis.get("name"), dict) else None, chassis
        else:
            system = next(iter(chassis), None)
            body = chassis.get(system) if isinstance(chassis.get(system), dict) else {}
        chassis_id = lldp_id(body.get("id"))
        port = info.get("port") or {}
        port_id = lldp_id(port.get("id"))
        is_mac = isinstance(port.get("id"), dict) and port["id"].get("type") == "mac"
        shown = port.get("descr") if port.get("descr") and (is_mac or not port_id) else port_id
        out.setdefault(name, []).append({"system": system, "port": shown, "chassisId": chassis_id, "portId": port_id})
    return out


def lldp_id(field):
    """An lldpcli id ({type, value}, or the bare value), lower case when it's a MAC."""
    value = field.get("value") if isinstance(field, dict) else field
    if not isinstance(value, str):
        return None
    return value.lower() if re.match(r"^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$", value) else value


def own_neighbors(neighbors, host, macs):
    """(neighbours from elsewhere, whether this node heard only itself). Each ConnectX-7 half
    hears its sibling half through the switch, and lldpd names it after this node: a neighbour
    whose system name is this node's hostname, or whose port or chassis id is one of this
    node's MACs, is this node, never the far end of the cable."""
    me = (host or "").split(".")[0].lower()
    mine = {m.lower() for m in macs if m}
    def is_self(n):
        return bool((n.get("system") or "").split(".")[0].lower() == me and me) or n.get("portId") in mine or n.get("chassisId") in mine
    kept = [n for n in neighbors if not is_self(n)]
    return kept, bool(neighbors) and not kept


def parse_psi(text):
    """/proc/pressure/<resource> -> (some avg10, full avg10) in percent; None where absent."""
    some = full = None
    for line in (text or "").splitlines():
        m = re.match(r"(some|full)\s+avg10=([\d.]+)", line)
        if m:
            if m.group(1) == "some":
                some = float(m.group(2))
            else:
                full = float(m.group(2))
    return some, full


def parse_gpu_line(line):
    """One line of the streaming query (GPU_FIELDS plus the throttle field) -> (gpu, extras)."""
    f = [x.strip() for x in line.split(",")]
    f += [""] * (9 - len(f))
    return parse_gpu(", ".join(f[:5])), parse_gpu_extra(", ".join(f[5:9]))


# ---- Kernel log

def parse_journal_entry(line):
    """`journalctl -k -o json` line -> (unix ms, message, cursor) or None. MESSAGE can be
    an array of bytes when it isn't valid UTF-8."""
    try:
        entry = json.loads(line)
    except ValueError:
        return None
    if not isinstance(entry, dict):
        return None
    msg = entry.get("MESSAGE")
    if isinstance(msg, list):
        msg = bytes(b for b in msg if isinstance(b, int) and 0 <= b < 256).decode("utf-8", "replace")
    if not isinstance(msg, str):
        return None
    try:
        ts = int(entry.get("__REALTIME_TIMESTAMP", "0")) // 1000
    except (TypeError, ValueError):
        ts = 0
    cursor = entry.get("__CURSOR")
    return ts or int(time.time() * 1000), msg, cursor if isinstance(cursor, str) else None


def parse_journal_line(line):
    """`journalctl -k -o json` line -> (unix ms, message) or None."""
    e = parse_journal_entry(line)
    return e[:2] if e else None


def parse_dmesg_line(line, now_ms):
    """A `dmesg --time-format iso` line, or a plain "[  123.456] text" one -> (unix ms, message)."""
    m = re.match(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)[,.](\d+)([+-]\d\d:?\d\d)\s+(.*)", line)
    if m:
        try:
            base = calendar.timegm(time.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S"))
            off = m.group(3).replace(":", "")
            sign = -1 if off[0] == "+" else 1
            base += sign * (int(off[1:3]) * 3600 + int(off[3:5]) * 60)
            return base * 1000 + int(m.group(2)[:3].ljust(3, "0")), m.group(4)
        except ValueError:
            pass
    m = re.match(r"\[\s*[\d.]+\]\s*(.*)", line)
    return now_ms, (m.group(1) if m else line).rstrip("\n")


def is_model_server(name, image=""):
    text = ("%s %s" % (name or "", image or "")).lower()
    return any(k in text for k in MODEL_SERVERS)


def classify_kernel(msg, ts_ms, containers=None):
    """A kernel log message -> a kernel event dict, or None for anything else.

    containers maps a Docker container id to (name, image), so a "python3" killed for
    memory inside a vLLM container counts as a model server."""
    containers = containers or {}
    m = XID_RE.search(msg)
    if m:
        code = int(m.group(2))
        severity = XID.get(code, ("warn", ""))[0]
        return {"tsMs": ts_ms, "kind": "xid", "code": code, "text": msg.strip()[:400], "severity": severity, "process": None}
    m = OOM_KILL_RE.search(msg)
    if m:
        memcg, task, pid = m.group(1), m.group(2), int(m.group(3))
        cid = container_of(memcg)
        name, image = containers.get(cid, (None, "")) if cid else (None, "")
        crit = is_model_server(task) or (name is not None and is_model_server(name, image))
        process = "%s (%s)" % (task, name) if name else task
        return {"tsMs": ts_ms, "kind": "oom", "code": None, "text": msg.strip()[:400], "severity": "crit" if crit else "warn",
                "process": process, "pid": pid}
    m = OOM_RE.search(msg)
    if m:
        name = m.group(2)
        return {"tsMs": ts_ms, "kind": "oom", "code": None, "text": msg.strip()[:400],
                "severity": "crit" if is_model_server(name) else "warn", "process": name, "pid": int(m.group(1))}
    m = HUNG_RE.search(msg)
    if m:
        return {"tsMs": ts_ms, "kind": "hung", "code": None, "text": msg.strip()[:400], "severity": "warn", "process": m.group(1)}
    if MLX5_RE.search(msg):
        return {"tsMs": ts_ms, "kind": "mlx5", "code": None, "text": msg.strip()[:400], "severity": "warn", "process": None}
    return None


class KernelEvents:
    """The recent kernel events, plus the first of each fatal Xid this boot (kept however
    much else the log says since, so a GPU that needs a reset stays reported). One OOM
    kill logs two lines (oom-kill: and "Out of memory: Killed process"); they become one
    event, keeping the container's name. A journal entry seen before (a restarted
    follower reads the boot again) is skipped by its cursor."""

    def __init__(self, keep=KERNEL_KEEP):
        self.events = collections.deque(maxlen=keep)
        self.fatal = {}  # Xid code -> its first event this boot
        self.seen = set()
        self.order = collections.deque()
        self.lock = threading.Lock()

    def once(self, key):
        """False when this entry (a journal cursor, or a dmesg line's time and text) was added before."""
        with self.lock:
            if key in self.seen:
                return False
            self.seen.add(key)
            self.order.append(key)
            if len(self.order) > 20_000:
                self.seen.discard(self.order.popleft())
            return True

    def add(self, event):
        with self.lock:
            if event["kind"] == "xid" and event.get("code") in FATAL_XID and event["code"] not in self.fatal:
                self.fatal[event["code"]] = event
            if event["kind"] == "oom":
                for e in self.events:
                    if e["kind"] == "oom" and e.get("pid") == event.get("pid") and abs(e["tsMs"] - event["tsMs"]) < 10_000:
                        if event["severity"] == "crit":
                            e["severity"] = "crit"
                        if "(" in (event.get("process") or "") and "(" not in (e.get("process") or ""):
                            e["process"] = event["process"]
                        return
            self.events.append(event)

    def list(self):
        with self.lock:
            pinned = [e for e in self.fatal.values() if not any(x is e for x in self.events)]
            return [{k: v for k, v in e.items() if k != "pid"} for e in sorted(pinned, key=lambda e: e["tsMs"]) + list(self.events)]


class KernelWatcher:
    """Reads the kernel log in a thread, from the start of this boot, then follows it:
    `journalctl -k -b -f --no-tail -o json`, or `dmesg -w` (the ring buffer, then new
    lines) where the journal can't be read. A follower that ends is started again after
    the last cursor it read."""

    JOURNAL = ["journalctl", "-k", "-b", "-f", "--no-tail", "-o", "json", "--no-pager"]
    DMESG = ["dmesg", "-w", "--time-format", "iso"]
    RESTART_WAIT = 5.0

    def __init__(self, events, containers=lambda: {}):
        self.events = events
        self.containers = containers
        self.source = None
        self.error = None
        self.cursor = None
        self.read_any = False
        threading.Thread(target=self.run, daemon=True, name="kernel-log").start()

    def readable(self):
        # A user outside systemd-journal (or adm) sees no kernel entries at all, yet
        # `journalctl -f` keeps running: check there's something to read first.
        return bool((run(["journalctl", "-k", "-b", "-n", "1", "-o", "json", "--no-pager"], timeout=5) or "").strip())

    def follow(self, cmd, kind):
        """Reads one follower until it ends; returns how long it ran."""
        try:
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace")
        except OSError:
            return None
        started = time.monotonic()
        self.source = kind
        with p.stdout:
            for line in p.stdout:
                self.read_any = True
                if kind == "journal":
                    entry = parse_journal_entry(line)
                    if not entry:
                        continue
                    ts, msg, cursor = entry
                    if cursor:
                        self.cursor = cursor
                        if not self.events.once(cursor):
                            continue
                else:
                    ts, msg = parse_dmesg_line(line, int(time.time() * 1000))
                    if not self.events.once((ts, msg)):
                        continue
                event = classify_kernel(msg, ts, self.containers())
                if event:
                    self.events.add(event)
        p.wait()
        return time.monotonic() - started

    def run(self):
        quick = 0
        while quick < 3:
            if self.readable():
                after = ["--after-cursor=%s" % self.cursor] if self.cursor else []
                ran = self.follow(self.JOURNAL + after, "journal")
            else:
                ran = self.follow(self.DMESG, "dmesg")
            # One that ran a while and ended (journald restarted) resumes after its cursor;
            # one that keeps ending at once is given up on after three tries.
            quick = 0 if ran is not None and ran > 30 else quick + 1
            time.sleep(1 if quick == 0 else self.RESTART_WAIT)
        if not self.read_any:
            self.source = None
            self.error = "Kernel log: neither journalctl -k nor dmesg could be read (add the user to systemd-journal or adm)"


# ---- GPU

class GpuStream:
    """One long-running `nvidia-smi --query-gpu=… -lms 2000` instead of a process every
    poll. `latest` is the newest (gpu, extras); None until the first line, or when the
    stream died (the caller then polls the old way)."""

    def __init__(self, every_ms=2000):
        self.latest = None
        self.alive = True
        self.every_ms = every_ms
        self.started = time.monotonic()
        threading.Thread(target=self.run, daemon=True, name="nvidia-smi").start()

    def silent(self):
        """Running, yet nothing for GPU_DEADLINE: its nvidia-smi is stuck (in the kernel, with the GPU)."""
        last = self.latest[0] if self.latest else self.started
        return self.alive and time.monotonic() - last > GPU_DEADLINE

    def run(self):
        for throttle in ("clocks_event_reasons.active", "clocks_throttle_reasons.active"):
            cmd = ["nvidia-smi", "--query-gpu=%s,%s" % (GPU_FIELDS, throttle), "--format=csv,noheader,nounits", "-lms", str(self.every_ms)]
            self.started = time.monotonic()
            try:
                p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
            except OSError:
                break
            lines = 0
            with p.stdout:
                for line in p.stdout:
                    if line.strip():
                        lines += 1
                        self.latest = (time.monotonic(),) + parse_gpu_line(line)
            p.wait()
            if lines:
                break  # it worked, then stopped: don't retry with the older field name
        self.alive = False
        self.latest = None


class NvmlGpu:
    """The same readings through pynvml (nvidia-ml-py), when it imports: no processes at all."""

    def __init__(self, nvml):
        self.nvml = nvml
        nvml.nvmlInit()
        self.handle = nvml.nvmlDeviceGetHandleByIndex(0)
        try:
            driver = nvml.nvmlSystemGetDriverVersion()
            self.driver = driver.decode() if isinstance(driver, bytes) else str(driver)
        except Exception:  # noqa: BLE001 - NVML raises its own error types
            self.driver = ""

    def call(self, name, *args):
        try:
            return getattr(self.nvml, name)(self.handle, *args)
        except Exception:  # noqa: BLE001 - "Not Supported" on GB10 for some fields
            return None

    def read(self):
        n = self.nvml
        util = self.call("nvmlDeviceGetUtilizationRates")
        power = self.call("nvmlDeviceGetPowerUsage")
        limit = self.call("nvmlDeviceGetEnforcedPowerLimit")
        clock = self.call("nvmlDeviceGetClockInfo", getattr(n, "NVML_CLOCK_SM", 1))
        clock_max = self.call("nvmlDeviceGetMaxClockInfo", getattr(n, "NVML_CLOCK_SM", 1))
        pstate = self.call("nvmlDeviceGetPerformanceState")
        reasons = self.call("nvmlDeviceGetCurrentClocksEventReasons") or self.call("nvmlDeviceGetCurrentClocksThrottleReasons")
        temp = self.call("nvmlDeviceGetTemperature", getattr(n, "NVML_TEMPERATURE_GPU", 0))
        gpu = {"utilPct": float(getattr(util, "gpu", 0) or 0), "tempC": float(temp or 0), "powerW": (power or 0) / 1000.0,
               "smClockMhz": int(clock or 0), "driver": self.driver}
        extra = {"clockMaxMhz": int(clock_max) if clock_max else None, "powerLimitW": limit / 1000.0 if limit else None,
                 "pstate": "P%d" % pstate if isinstance(pstate, int) else None,
                 "throttle": [name for bit, name in THROTTLE_BITS if (reasons or 0) & bit]}
        return gpu, extra


def nvml_gpu():
    """An NvmlGpu when pynvml imports and finds the GPU, else None."""
    try:
        import pynvml  # noqa: PLC0415 - optional
        return NvmlGpu(pynvml)
    except Exception:  # noqa: BLE001 - not installed, no driver, no GPU
        return None


def first_line(text):
    return next((l.strip() for l in (text or "").splitlines() if l.strip()), "")


class GpuSampler:
    """Reads the GPU in a thread of its own: NVML, else the newest line of nvidia-smi's
    stream, else one nvidia-smi query per poll (with a deadline). A GPU that stops
    answering (an Xid 79, a driver stuck in the kernel) can hang that thread, never a
    frame: `reading(now)` is (latest, None) while the newest reading is under 10 s old,
    else (None, why)."""

    def __init__(self, every=GPU_EVERY, stream=True, nvml=None, poll_timeout=8):
        self.every, self.poll_timeout = every, poll_timeout
        self.nvml = nvml
        self.stream = GpuStream(int(every * 1000)) if stream and not nvml else None
        self.source = "NVML" if nvml else "nvidia-smi"
        self.extra_query = "clocks_event_reasons.active"
        self.latest = None  # (monotonic, gpu, extras, apps)
        self.error = None   # why the last read failed, when it said
        self.started = time.monotonic()
        self.backoff = NVIDIA_SMI_BACKOFF
        self.not_before = 0.0  # after a timeout, no nvidia-smi before this (monotonic)
        self.ready = threading.Event()
        threading.Thread(target=self.run, daemon=True, name="gpu").start()

    def smi(self, args):
        """One nvidia-smi run: run_ex's (status, stdout, stderr), or ("stuck", None, None) without
        starting one while an earlier one is stuck in the kernel or a timeout's back-off runs."""
        if stuck("nvidia-smi") or time.monotonic() < self.not_before:
            return "stuck", None, None
        status, out, err = run_ex(["nvidia-smi"] + args, self.poll_timeout)
        if status == "timeout":
            self.not_before = time.monotonic() + self.backoff
            self.backoff = min(self.backoff * 2, NVIDIA_SMI_BACKOFF_MAX)
        elif status == "ok":
            self.backoff = NVIDIA_SMI_BACKOFF
        return status, out, err

    def stuck_message(self):
        n = stuck("nvidia-smi") + (1 if self.stream and self.stream.silent() else 0)
        if n:
            return "nvidia-smi is stuck in the kernel (%d process%s)" % (n, "" if n == 1 else "es")
        return "nvidia-smi didn't answer in %d s; trying again in %d s" % (GPU_DEADLINE, max(self.not_before - time.monotonic(), 0) + 0.5)

    def poll(self):
        """One nvidia-smi query: (gpu, extras), or None with self.error set."""
        status, out, err = self.smi(["--query-gpu=utilization.gpu,temperature.gpu,power.draw,clocks.sm,driver_version",
                                     "--format=csv,noheader,nounits"])
        if status != "ok":
            self.error = {"missing": "nvidia-smi isn't installed",
                          "timeout": "nvidia-smi didn't answer in %d s" % GPU_DEADLINE}.get(status) or \
                (self.stuck_message() if status == "stuck" else None) or \
                "nvidia-smi failed: %s" % (first_line(err) or first_line(out) or "no output")[:160]
            return None
        gpu = parse_gpu(out)
        # Older drivers call the throttle field clocks_throttle_reasons; a failed query only loses the extras.
        status, extra, _ = self.smi(["--query-gpu=clocks.max.sm,power.limit,pstate,%s" % self.extra_query, "--format=csv,noheader,nounits"])
        if status == "failed" and self.extra_query == "clocks_event_reasons.active":
            self.extra_query = "clocks_throttle_reasons.active"
        return gpu, parse_gpu_extra(extra if status == "ok" else "")

    def read_once(self):
        streamed = self.stream.latest if self.stream else None
        if self.nvml:
            got = self.nvml.read()
        elif streamed and time.monotonic() - streamed[0] < 3 * self.every:
            got = streamed[1:]
        elif self.stream and self.stream.silent():
            # The stream's nvidia-smi is alive but says nothing: stuck. Another would be too.
            self.error = self.stuck_message()
            return
        else:
            got = self.poll()  # no stream (or it died): one query per poll
        if got is None:
            return
        status, out, _ = self.smi(["--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader,nounits"])
        apps = parse_apps(out if status == "ok" else "")
        for a in apps:
            a["cid"] = container_of(read(proc("%d/cgroup" % a["pid"])))
        self.latest = (time.monotonic(),) + tuple(got) + (apps,)
        self.error = None
        self.ready.set()

    def run(self):
        while True:
            started = time.monotonic()
            try:
                self.read_once()
            except Exception as e:  # noqa: BLE001 - NVML raises its own types; keep reading
                self.error = "%s: %s" % (self.source, e)
            self.ready.set()
            time.sleep(max(self.every - (time.monotonic() - started), 0.1))

    def reading(self, now):
        latest = self.latest
        if latest and now - latest[0] < GPU_DEADLINE:
            return latest, None
        if stuck("nvidia-smi"):
            return None, self.stuck_message()
        if self.error:
            return None, self.error
        if latest is None and now - self.started < GPU_DEADLINE:
            return None, None  # still starting
        return None, "%s didn't answer in %d s" % (self.source, GPU_DEADLINE)


def roce_counters(iface, dev):
    """RoCE and PHY counters for a fabric interface from roce.py, or None without it."""
    if roce is None or not dev:
        return None
    for name in ("counters", "read_counters", "sample"):
        fn = getattr(roce, name, None)
        if fn:
            try:
                return fn(iface, dev)
            except Exception:  # noqa: BLE001 - one unreadable counter must not end the frame
                return None
    return None


def user_name(uid, cache={}):
    if uid not in cache:
        name = None
        if ROOT:
            # A fixture tree has its own /etc/passwd.
            for line in read(ROOT + "/etc/passwd").splitlines():
                f = line.split(":")
                if len(f) > 2 and f[2] == str(uid):
                    name = f[0]
                    break
        if name is None:
            try:
                name = pwd.getpwuid(uid).pw_name
            except KeyError:
                name = str(uid)
        cache[uid] = name
    return cache[uid]


def proc_status(pid):
    """(real uid, resident bytes or None) from /proc/<pid>/status, which anyone can read."""
    uid = rss = None
    for line in read(proc("%d/status" % pid)).splitlines():
        try:
            if line.startswith("Uid:"):
                uid = int(line.split()[1])
            elif line.startswith("VmRSS:"):
                rss = int(line.split()[1]) * 1024
        except (IndexError, ValueError):
            pass
    if uid is None:
        try:
            uid = os.stat(proc("%d" % pid)).st_uid
        except OSError:
            pass
    return uid, rss


def full_name(comm, argv):
    """The process name: the kernel cuts comm to 15 characters ("dashboard-servi"), so the file
    name of argv[0] (or of the script an interpreter runs, argv[1]) is used when comm is the
    start of it ("dashboard-service", "networkd-dispatcher")."""
    if len(comm) == 15:
        for a in argv[:2]:
            base = os.path.basename(a.split(" ")[0]) if a else ""
            if len(base) > 15 and base.startswith(comm):
                return base
    return comm


def is_server(cmd):
    return any(p in cmd for p in SERVER_PATTERNS)


def processes(prev, dt, gpu_pids, me=None):
    """Every process since the last call, cut to what's sent, and the listening ports.

    Returns (process list, listener list, ticks by pid for next time, process count). The
    list is every userland process up to MAX_PROCESSES, the busiest (CPU, then memory)
    after the ones always kept: GPU users, processes with a listening socket SparkBox can
    see, model servers and DAEMONS. Then the KERNEL_THREADS busiest kernel threads (pid 2,
    its children, or no command line), marked kernel. The collector itself and anything
    it started, or anything run from ~/.sparkbox, is marked sparkbox."""
    me = os.getpid() if me is None else me
    sockets = {}
    for path in ("net/tcp", "net/tcp6"):
        sockets.update(parse_listeners(read(proc(path))))
    ticks, rows, owner = {}, [], {}
    page = os.sysconf("SC_PAGE_SIZE") if hasattr(os, "sysconf") else 4096
    for entry in os.listdir(PROC):
        if not entry.isdigit():
            continue
        pid = int(entry)
        st = parse_pid_stat(read(proc("%d/stat" % pid)))
        if not st:
            continue
        t = st["ticks"]
        ticks[pid] = t
        cpu = max(t - prev.get(pid, t), 0) / CLK_TCK / max(dt, 0.001) * 100
        argv = read(proc("%d/cmdline" % pid)).split("\0")
        cmd = " ".join(a for a in argv if a).strip()
        kernel = pid == 2 or st["ppid"] == 2 or not cmd
        listening = []
        if not kernel:
            try:
                for fd in os.listdir(proc("%d/fd" % pid)):
                    link = os.readlink(proc("%d/fd/%s" % (pid, fd)))
                    if link.startswith("socket:[") and link[8:-1] in sockets:
                        listening.append(sockets[link[8:-1]][1])
                        owner[link[8:-1]] = (pid, full_name(st["name"], argv))
            except OSError:
                pass  # someone else's process: its sockets aren't visible without root
        rows.append((pid, st, cpu, sorted(set(listening)), cmd, argv, kernel))
    busiest = lambda r: (-r[2], -r[1]["rss"], r[0])
    user = sorted((r for r in rows if not r[6]), key=busiest)
    keep = [r for r in user if r[0] in gpu_pids or r[3] or is_server(r[4]) or full_name(r[1]["name"], r[5]) in DAEMONS
            or r[1]["name"] in DAEMONS]
    kept = {r[0] for r in keep}
    keep += [r for r in user if r[0] not in kept][:max(MAX_PROCESSES - len(keep), 0)]
    keep = sorted(keep, key=busiest)[:MAX_PROCESSES]
    keep += sorted((r for r in rows if r[6]), key=lambda r: (-r[2], -r[1]["ticks"], r[0]))[:KERNEL_THREADS]
    # SparkBox's own: the collector, anything run from ~/.sparkbox, and what they started.
    ours = {r[0] for r in rows if r[0] == me or "/.sparkbox/" in r[4] or r[4].startswith(".sparkbox/")}
    for _ in range(3):
        ours |= {r[0] for r in rows if r[1]["ppid"] in ours and not r[6]}
    out = []
    for pid, st, cpu, listening, cmd, argv, kernel in keep:
        uid, rss = proc_status(pid)
        if uid is None:
            continue
        cgroup = read(proc("%d/cgroup" % pid)) if not kernel else ""
        name = full_name(st["name"], argv)
        p = {"pid": pid, "name": name, "user": user_name(uid), "cmd": (cmd or "[%s]" % st["name"])[:CMD_CHARS],
             "cpuPct": round(cpu, 1), "rss": rss if rss is not None else st["rss"] * page, "nice": st["nice"], "cid": container_of(cgroup),
             "unit": unit_of(cgroup), "state": st["state"], "ppid": st["ppid"],
             "threads": st["threads"], "startTime": st["start"], "cpuTimeS": round(st["ticks"] / CLK_TCK, 2)}
        # Fields at their default are left out: 200 processes must fit a frame.
        if listening:
            p["ports"] = listening
        if name != st["name"]:
            p["comm"] = st["name"]
        if kernel:
            p["kernel"] = True
        elif pid in ours:
            p["sparkbox"] = True
        out.append(p)
    return out, fold_listeners(sockets, owner), ticks, len(rows)


def cuda_version(smi_text):
    m = re.search(r"CUDA Version:\s*([\d.]+)", smi_text or "")
    return m.group(1) if m else ""


def docker_time(stamp):
    """'2026-10-25T08:01:02.123456789Z' -> unix seconds; Docker's zero time -> 0."""
    if not stamp or stamp.startswith("0001-"):
        return 0
    try:
        return calendar.timegm(time.strptime(stamp[:19], "%Y-%m-%dT%H:%M:%S"))
    except ValueError:
        return 0


def ports(entries):
    """Docker's port list -> ['0.0.0.0:8000->8000/tcp', '6379/tcp'] without IPv6 duplicates."""
    out = []
    for p in entries or []:
        if p.get("IP") == "::":
            continue
        if p.get("PublicPort"):
            s = "%s:%s->%s/%s" % (p.get("IP", "0.0.0.0"), p["PublicPort"], p.get("PrivatePort"), p.get("Type", "tcp"))
        else:
            s = "%s/%s" % (p.get("PrivatePort"), p.get("Type", "tcp"))
        if s not in out:
            out.append(s)
    return out


SECRET_NAME = re.compile(r"KEY|TOKEN|SECRET|PASSWORD|PASS|CREDENTIAL")


def env_flags(env):
    """NCCL_* and VLLM_HOST_IP from a container's environment, for the NCCL transport
    check; a secret-looking name keeps only that it's set."""
    out = {}
    for kv in env or []:
        k, sep, v = kv.partition("=")
        if sep and (k.startswith("NCCL_") or k == "VLLM_HOST_IP"):
            out[k] = "(hidden)" if SECRET_NAME.search(k) else v
    # Conflicting declarations cannot establish a version. Other transport flags keep
    # their existing display behavior.
    declared = {v.split("=", 1)[1] for v in env or [] if v.startswith("NCCL_VERSION=")}
    if len(declared) > 1:
        out.pop("NCCL_VERSION", None)
    return out


def inspect_meta(info):
    """The parts of `docker inspect` the snapshot needs."""
    state = info.get("State") or {}
    return {
        "imageId": info.get("Image") if re.fullmatch(r"sha256:[0-9a-f]{64}", info.get("Image") or "") else None,
        "envFlags": env_flags((info.get("Config") or {}).get("Env")),
        "startedAt": docker_time(state.get("StartedAt")),
        "finishedAt": docker_time(state.get("FinishedAt")),
        "exitCode": state.get("ExitCode", 0),
        "health": (state.get("Health") or {}).get("Status"),
        "restartCount": info.get("RestartCount", 0),
        "oomKilled": bool(state.get("OOMKilled")),
        "restartPolicy": ((info.get("HostConfig") or {}).get("RestartPolicy") or {}).get("Name") or None,
    }


# ---- System access

class UnixHTTP(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=3)
        self.unix_path = path

    def connect(self):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(3)
        try:
            s.connect(self.unix_path)
        except OSError:
            s.close()
            raise
        self.sock = s


def docker(path):
    conn = UnixHTTP(DOCKER_SOCK)
    try:
        conn.request("GET", path)
        r = conn.getresponse()
        body = r.read()
        if r.status != 200:
            raise RuntimeError("Docker API returned %d" % r.status)
        return json.loads(body)
    finally:
        conn.close()


def cgroup_usage(cid):
    """(cpu µs, memory bytes) for a container, from cgroup v2 (systemd or cgroupfs layout)."""
    for d in (sysfs("fs/cgroup/system.slice/docker-%s.scope" % cid), sysfs("fs/cgroup/docker/%s" % cid)):
        if os.path.isdir(d):
            usec = 0
            for line in read(d + "/cpu.stat").splitlines():
                if line.startswith("usage_usec"):
                    usec = int(line.split()[1])
            mem = int(read(d + "/memory.current", "0").strip() or 0)
            return usec, mem
    return None


def interfaces():
    """Physical interfaces as dicts: name, role, up, speed Mb/s, mtu, mac, carrier changes, RDMA device.
    ConnectX-7 (mlx5) ports are the fabric."""
    out = []
    for name in sorted(os.listdir(sysfs("class/net"))):
        base = sysfs("class/net/" + name)
        if not os.path.exists(base + "/device"):
            continue  # lo, docker0, veth*, bridges
        driver = os.path.basename(os.path.realpath(base + "/device/driver"))
        role = "wifi" if os.path.exists(base + "/wireless") else "fabric" if driver.startswith("mlx5") else "lan"
        up = read(base + "/operstate").strip() == "up"
        speed = read_int(base + "/speed") or 0
        out.append({"name": name, "role": role, "up": up, "speed": max(speed, 0) if up else 0,
                    "mtu": read_int(base + "/mtu"), "mac": read(base + "/address").strip() or None,
                    "carrier": read_int(base + "/carrier_changes"),
                    "rdma": rdma_device(name) if role == "fabric" else None})
    return out


def addresses():
    try:
        data = json.loads(run(["ip", "-j", "-4", "addr"]) or "[]")
    except ValueError:
        return {}
    return {i["ifname"]: i["addr_info"][0]["local"] for i in data if i.get("addr_info")}


def cpu_temp():
    temps = []
    for path in glob.glob(sysfs("class/thermal/thermal_zone*/temp")):
        try:
            temps.append(int(read(path).strip()) / 1000.0)
        except ValueError:
            pass
    return max(temps) if temps else 0.0


def os_name():
    fields = {}
    for path in (ROOT + "/etc/os-release", ROOT + "/etc/dgx-release"):
        for line in read(path).splitlines():
            k, _, v = line.partition("=")
            fields[k.strip()] = v.strip().strip('"')
    dgx = fields.get("DGX_PRETTY_NAME") or fields.get("DGX_OTA_VERSION")
    base = fields.get("PRETTY_NAME", "Linux")
    return "%s (%s)" % (dgx, base) if dgx else base


def dgx_release():
    """/etc/dgx-release: the DGX OS build the node was imaged with and the last OTA applied, with
    its date as the file gives it. Each OTA adds a block (7.5.0, then 7.6.0): the last one counts."""
    build = ota = date = None
    for line in read(ROOT + "/etc/dgx-release").splitlines():
        k, _, v = line.partition("=")
        k, v = k.strip(), v.strip().strip('"') or None
        if k == "DGX_SWBUILD_VERSION":
            build = v
        elif k == "DGX_OTA_VERSION":
            ota, date = v, None
        elif k == "DGX_OTA_DATE":
            date = v
    return {"dgxBuild": build, "dgxOta": ota, "dgxOtaDate": date}


def model_name():
    name = read(sysfs("class/dmi/id/product_name")).strip()
    return name or read(proc("device-tree/model")).strip("\x00\n ")


def system_extras():
    """BIOS, ConnectX-7 firmware, Docker, NTP, reboot-required and DGX OS: slow-changing facts."""
    cx7 = None
    for dev in sorted(glob.glob(sysfs("class/infiniband/*"))):
        if os.path.basename(dev).startswith(("roce", "mlx5")):
            cx7 = read(dev + "/fw_ver").strip() or None
            if cx7:
                break
    ntp = (run(["timedatectl", "show", "-p", "NTPSynchronized", "--value"], timeout=4) or "").strip()
    # systemd-timesyncd (DGX OS) tells anyone its offset; chrony, where it runs instead, too.
    offset = parse_timesync_offset(run(["timedatectl", "timesync-status"], timeout=4))
    if offset is None:
        offset = parse_clock_offset(run(["chronyc", "-n", "tracking"], timeout=4))
    try:
        engine = docker("/version").get("Version")
    except (OSError, RuntimeError, ValueError):
        engine = None
    try:
        info = docker("/info")
        driver = info.get("Driver")
        store = "containerd" if any(len(p) == 2 and p[0] == "driver-type" and str(p[1]).startswith("io.containerd.snapshotter") for p in info.get("DriverStatus") or []) else driver
    except (OSError, RuntimeError, ValueError, TypeError):
        store = None
    toolkit = (run(["dpkg-query", "-W", "-f=${Status}|${Version}", "nvidia-container-toolkit"], timeout=4) or "").strip()
    toolkit = toolkit.removeprefix("install ok installed|") if toolkit.startswith("install ok installed|") else None
    return dict({
        "bios": read(sysfs("class/dmi/id/bios_version")).strip() or None,
        "cx7Fw": cx7,
        "docker": engine,
        "imageStore": store,
        "containerToolkit": toolkit,
        "ntpSynced": {"yes": True, "no": False}.get(ntp),
        "clockOffsetMs": offset,
        "rebootRequired": os.path.exists(ROOT + "/var/run/reboot-required"),
    }, **dgx_release())


def hostname():
    return read(proc("sys/kernel/hostname")).strip() or socket.gethostname()


def kernel_release():
    return read(proc("sys/kernel/osrelease")).strip() or os.uname().release


def nm_wake_on_lan(iface):
    """NetworkManager's 802-3-ethernet.wake-on-lan for the connection on `iface`, readable
    without root; None when nmcli isn't there or the device has no connection."""
    conn = first_line(run(["nmcli", "-g", "GENERAL.CONNECTION", "device", "show", iface], timeout=4))
    if not conn:
        return None
    out = run(["nmcli", "-g", "802-3-ethernet.wake-on-lan", "connection", "show", conn], timeout=4)
    return None if out is None else out.strip()


def lldp_neighbors():
    """LLDP neighbours by interface, when lldpd lets this user ask; {} otherwise."""
    return parse_lldp(run(["lldpcli", "-f", "json", "show", "neighbors"], timeout=4))


def failed_units():
    out = run(["systemctl", "--failed", "--output=json", "--no-pager"], timeout=5)
    if out is None:
        out = run(["systemctl", "--failed", "--no-legend", "--plain", "--no-pager"], timeout=5)
    return parse_failed_units(out)


class Writer:
    """Writes frames to SparkBox. A watchdog ends the collector when one write has been
    stuck for WRITE_STALL seconds: the SSH channel is gone but nobody closed the pipe."""

    def __init__(self, out=sys.stdout):
        self.out = out
        self.started = None
        threading.Thread(target=self.watch, daemon=True).start()

    def watch(self):
        while True:
            time.sleep(1.0)
            if self.started is not None and time.monotonic() - self.started > WRITE_STALL:
                os._exit(3)

    def write(self, line):
        self.started = time.monotonic()
        try:
            self.out.write(line + "\n")
            self.out.flush()
        finally:
            self.started = None


# ---- Main loop

class Collector:
    """Takes readings. The cadences are what the agent and the live stream share: core
    numbers every call, the GPU every 2 s, Docker every few seconds, slow facts every 30 s."""

    def __init__(self, gpu_every=GPU_EVERY, docker_every=DOCKER_EVERY, slow_every=SLOW_EVERY, stream_gpu=True, kernel=True):
        self.gpu_every, self.docker_every, self.slow_every = gpu_every, docker_every, slow_every
        self.errors = {}
        self.prev_stat = parse_stat(read(proc("stat")))
        self.prev_net = parse_net_dev(read(proc("net/dev")))
        try:
            self.prev_rdma = {i["rdma"]: rdma_bytes(i["rdma"]) for i in interfaces() if i["rdma"]}
        except OSError:
            self.prev_rdma = {}
        self.prev_disk = parse_diskstats(read(proc("diskstats")))
        self.prev_t = time.monotonic()
        self.prev_swap_in = None
        self.prev_swap_at = self.prev_t
        self.page_size = os.sysconf("SC_PAGE_SIZE")
        self.prev_cg = {}
        self.gpu, self.gpu_extra, self.apps = dict(NO_GPU), parse_gpu_extra(""), []
        self.listing, self.meta, self.last_docker = [], {}, -docker_every
        self.units, self.last_units = [], -UNITS_EVERY
        self.addrs, self.model, self.osn, self.extras, self.last_slow = {}, "", "", {}, -slow_every
        self.procs, self.listeners, self.ticks, self.last_proc, self.proc_total = [], [], {}, None, 0
        self.wol, self.last_wol = {}, -WOL_EVERY
        self.wifi, self.last_wifi, self.lldp = {}, -UNITS_EVERY, {}
        self.cuda = cuda_version(run(["nvidia-smi"], timeout=5))
        self.boot_id = read(proc("sys/kernel/random/boot_id")).strip() or None
        self.gpu_sampler = GpuSampler(gpu_every, stream=stream_gpu, nvml=nvml_gpu() if stream_gpu else None)
        self.gpu_sampler.ready.wait(GPU_FIRST_WAIT)
        self.kernel = KernelEvents()
        self.kernel_watch = KernelWatcher(self.kernel, self.container_index) if kernel else None

    def container_index(self):
        """Container id -> (name, image), for naming what an OOM kill hit. A SparkBox load runs its
        image by id, so its image's name comes from its label."""
        return {c["Id"]: ((c.get("Names") or ["/"])[0].lstrip("/"), (c.get("Labels") or {}).get("sparkbox.image") or c.get("Image", "")) for c in self.listing}

    def poll_gpu(self, now):
        """The sampler's newest reading; when there's none for 10 s the GPU numbers are null
        (unknown, not idle) and errors.gpu says why."""
        latest, why = self.gpu_sampler.reading(now)
        if latest:
            _, gpu, self.gpu_extra, self.apps = latest
            self.gpu = dict(gpu, driver=gpu.get("driver") or self.gpu.get("driver", ""))
            self.errors.pop("gpu", None)
            return
        driver = self.gpu.get("driver", "")
        self.gpu, self.gpu_extra, self.apps = dict(NO_GPU, driver=driver), dict(NO_GPU_EXTRA), []
        if why:
            self.errors["gpu"] = why
        else:
            self.errors.pop("gpu", None)

    def poll_wol(self, now, lan):
        """Wake-on-LAN of the LAN ports: ethtool (root only on DGX OS), else NetworkManager."""
        if now - self.last_wol < WOL_EVERY:
            return
        self.last_wol = now
        self.wol = {}
        for name in lan:
            letters = parse_wol(run(["ethtool", name], timeout=4))
            self.wol[name] = wol_info(letters, None if letters else nm_wake_on_lan(name))

    def poll_wifi(self, now, wifi):
        """The association of each Wi-Fi interface that's up, from `iw dev <if> link` (no root)."""
        if now - self.last_wifi < UNITS_EVERY:
            return
        self.last_wifi = now
        self.wifi = {name: parse_iw_link(run(["iw", "dev", name, "link"], timeout=4)) for name in wifi}

    def poll_docker(self, now):
        if now - self.last_docker < self.docker_every:
            return
        self.last_docker = now
        try:
            self.listing = docker("/containers/json?all=1")
            self.errors.pop("docker", None)
            for c in self.listing:
                key = c.get("State")
                cached = self.meta.get(c["Id"])
                # Re-inspect on a state change, and every 10 s for health and restart counts.
                if not cached or cached[0] != key or now - cached[1] >= INSPECT_EVERY:
                    self.meta[c["Id"]] = (key, now, inspect_meta(docker("/containers/%s/json" % c["Id"])))
            ids = {c["Id"] for c in self.listing}
            for gone in [cid for cid in self.meta if cid not in ids]:
                self.meta.pop(gone, None)
            for gone in [cid for cid in self.prev_cg if cid not in ids]:
                self.prev_cg.pop(gone, None)
        except PermissionError:
            self.errors["docker"] = "Docker: permission denied (add this user to the docker group)"
            self.listing = []
        except (OSError, RuntimeError, ValueError) as e:
            self.errors["docker"] = "Docker: %s" % e
            self.listing = []

    def frame(self):
        now = time.monotonic()
        dt = max(now - self.prev_t, 0.001)
        self.prev_t = now

        stat = parse_stat(read(proc("stat")))
        total_pct, cores = cpu_percent(self.prev_stat, stat)
        self.prev_stat = stat
        load = [float(x) for x in read(proc("loadavg"), "0 0 0").split()[:3]]
        mem = memory(parse_meminfo(read(proc("meminfo"))))
        swap_in = swap_pages_in(read(proc("vmstat")))
        mem["swapInBps"] = swap_in_rate(self.prev_swap_in, swap_in, now - self.prev_swap_at, self.page_size)
        self.prev_swap_in, self.prev_swap_at = swap_in, now

        self.poll_gpu(now)
        self.poll_docker(now)

        if self.last_proc is None or now - self.last_proc >= PROC_EVERY:
            try:
                self.procs, self.listeners, self.ticks, self.proc_total = processes(
                    self.ticks, now - self.last_proc if self.last_proc else 1.0, {a["pid"] for a in self.apps})
                self.errors.pop("procs", None)
            except OSError as e:
                self.errors["procs"] = "Processes: %s" % e
            self.last_proc = now

        if now - self.last_units >= UNITS_EVERY:
            self.last_units = now
            self.units = failed_units()

        if now - self.last_slow >= self.slow_every:
            self.last_slow = now
            self.addrs, self.model, self.osn, self.extras = addresses(), model_name(), os_name(), system_extras()
            self.lldp = lldp_neighbors()

        net = parse_net_dev(read(proc("net/dev")))
        nets = []
        ifaces = interfaces()
        host = hostname()
        macs = [i["mac"] for i in ifaces]
        self.poll_wol(now, [i["name"] for i in ifaces if i["role"] == "lan"])
        self.poll_wifi(now, [i["name"] for i in ifaces if i["role"] == "wifi" and i["up"]])
        for i in ifaces:
            name = i["name"]
            cur = net.get(name)
            last = self.prev_net.get(name, cur)
            rx = counter_delta(last["rx"], cur["rx"]) if cur and last else 0
            tx = counter_delta(last["tx"], cur["tx"]) if cur and last else 0
            entry = {"iface": name, "role": i["role"], "up": i["up"], "speedMbps": i["speed"],
                     "rxBps": rx * 8 / dt, "txBps": tx * 8 / dt,
                     # The address stays known while the link is down, so "down with an IP" can be seen.
                     "ip": self.addrs.get(name), "mtu": i["mtu"], "mac": i["mac"], "carrierChanges": i["carrier"],
                     "rxErrors": cur["rxErrors"] if cur else 0, "txErrors": cur["txErrors"] if cur else 0,
                     "rxDrops": cur["rxDrops"] if cur else 0, "txDrops": cur["txDrops"] if cur else 0}
            if i["rdma"]:
                counters = rdma_bytes(i["rdma"])
                rrx, rtx = rdma_rate(self.prev_rdma.get(i["rdma"], counters), counters, dt)
                self.prev_rdma[i["rdma"]] = counters
                entry.update({"rdmaDev": i["rdma"], "rdmaRxBps": rrx, "rdmaTxBps": rtx})
                counters = roce_counters(name, i["rdma"])
                if counters is not None:
                    entry["roce"] = counters
            if i["role"] == "lan" and self.wol.get(name):
                entry["lanWol"] = self.wol[name]
            if i["role"] == "wifi" and i["up"]:
                # Wi-Fi has no sysfs speed: it's the tx bitrate, or unknown (null, never 0).
                link = self.wifi.get(name)
                entry["wifi"] = link
                entry["speedMbps"] = int(round(link["bitrateMbps"])) if link and link.get("bitrateMbps") else None
            if self.lldp.get(name):
                # Every neighbour that isn't this node; `lldp` stays the first, for older SparkBoxes.
                kept, self_only = own_neighbors(self.lldp[name], host, macs)
                entry["lldpPeers"] = kept
                if kept:
                    entry["lldp"] = {"system": kept[0]["system"], "port": kept[0]["port"]}
                if self_only:
                    entry["lldpSelfOnly"] = True
            nets.append(entry)
        self.prev_net = net

        disk = parse_diskstats(read(proc("diskstats")))
        disks = []
        for dev, mount, fstype in parse_mounts(read(proc("mounts"))):
            try:
                st = os.statvfs(mount)
            except OSError:
                continue
            name = os.path.basename(dev)
            r0, w0 = self.prev_disk.get(name, disk.get(name, (0, 0)))
            r1, w1 = disk.get(name, (r0, w0))
            disks.append(dict({"mount": mount, "device": dev, "fsType": fstype}, **disk_space(st),
                              readBps=max(r1 - r0, 0) / dt, writeBps=max(w1 - w0, 0) / dt))
        self.prev_disk = disk

        names = {c["Id"]: c["Names"][0].lstrip("/") if c.get("Names") else c["Id"][:12] for c in self.listing}
        gpu_by_cid = {}
        for a in self.apps:
            if a.get("cid"):
                gpu_by_cid[a["cid"]] = gpu_by_cid.get(a["cid"], 0) + a["mem"]
        containers = []
        for c in self.listing:
            cid = c["Id"]
            info = self.meta.get(cid, (None, 0, inspect_meta({})))[2]
            cpu = mem_used = 0
            usage = cgroup_usage(cid) if c.get("State") == "running" else None
            if usage:
                usec, mem_used = usage
                last = self.prev_cg.get(cid)
                if last:
                    cpu = max(usec - last[0], 0) / ((now - last[1]) * 1e6) * 100
                self.prev_cg[cid] = (usec, now)
            labels = c.get("Labels") or {}
            containers.append(dict(info, **{
                "id": cid[:12], "name": names[cid], "image": c.get("Image", ""), "state": c.get("State", ""),
                "cpuPct": round(cpu, 1), "mem": mem_used, "gpuMem": gpu_by_cid.get(cid, 0),
                "ports": ports(c.get("Ports")), "project": labels.get("com.docker.compose.project"),
                "command": c.get("Command", ""), "labels": labels,
            }))

        apps = self.apps
        mem["gpuAlloc"] = sum(a["mem"] for a in apps)
        for res, keys in (("memory", ("psiSome10", "psiFull10")), ("io", ("psiIoSome10", "psiIoFull10"))):
            some, full = parse_psi(read(proc("pressure/" + res)))
            mem[keys[0]], mem[keys[1]] = some, full
        system = {"hostname": hostname(), "model": self.model, "os": self.osn, "kernel": kernel_release(),
                  "driver": self.gpu["driver"], "cuda": self.cuda,
                  "uptimeS": int(float(read(proc("uptime"), "0").split()[0]))}
        system.update(self.extras)
        system["collectorVersion"] = COLLECTOR_BUILD
        return {
            "v": VERSION,
            "bootId": self.boot_id,
            "cpu": {"totalPct": total_pct, "perCore": cores, "load": load, "tempC": cpu_temp()},
            "mem": mem,
            "gpu": dict({"utilPct": self.gpu["utilPct"], "tempC": self.gpu["tempC"], "powerW": self.gpu["powerW"],
                         "smClockMhz": self.gpu["smClockMhz"],
                         "procs": [{"pid": a["pid"], "name": a["name"], "mem": a["mem"],
                                    "container": names.get(a.get("cid"))} for a in apps]}, **self.gpu_extra,
                        **({"error": self.errors["gpu"]} if "gpu" in self.errors else {})),
            "disks": disks,
            "nets": nets,
            "system": system,
            "containers": containers,
            "processes": [self.process_out(p, names, apps) for p in self.procs],
            "procTotal": self.proc_total,
            "listeners": self.listeners,
            "failedUnits": self.units,
            "kernelEvents": self.kernel.list(),
            "errors": sorted(list(self.errors.values()) + ([self.kernel_watch.error] if self.kernel_watch and self.kernel_watch.error else [])),
        }


    @staticmethod
    def process_out(p, names, apps):
        out = {k: v for k, v in p.items() if k != "cid"}
        if p["cid"]:
            # Docker and /proc refresh on different clocks. After docker rm, the
            # cached process can outlive the container's name for one sample.
            # Keep its known container identity so discovery cannot mistake it
            # for a new model server running directly on the host.
            out["container"] = names.get(p["cid"], p["cid"][:12])
        gpu = sum(a["mem"] for a in apps if a["pid"] == p["pid"])
        if gpu:
            out["gpuMem"] = gpu
        return out


_default = None


def sample(collector=None):
    """One reading of the node as a frame dict (the schema SparkBox reads). Without a
    collector, a module-wide one is made on first use and kept, so rates have a "before"."""
    global _default
    if collector is None:
        if _default is None:
            _default = Collector()
        collector = _default
    try:
        frame = collector.frame()
    except Exception as e:
        # One bad read must not end the stream. Skip this reading (SparkBox marks the node
        # stale rather than showing old numbers as new) and report it with the next one.
        collector.errors["loop"] = "Collector: %s: %s" % (type(e).__name__, e)
        return None
    collector.errors.pop("loop", None)
    return frame


def main():
    out = Writer()
    collector = Collector()
    while True:
        time.sleep(1.0)
        frame = sample(collector)
        if frame is None:
            continue
        try:
            out.write(json.dumps(frame, separators=(",", ":")))
        except BrokenPipeError:
            return  # SparkBox went away


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
