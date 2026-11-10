#!/usr/bin/env python3
"""Builds a DGX Spark as the Live tour found it, from the fixtures next to this file, for
collector_test.py and fake_node's Spark profile (`Profile::spark`):

    python3 -I spark_tree.py <node> <dest>

<dest>/proc, <dest>/sys and <dest>/root (/etc) are what collector.py reads through
SPARKBOX_PROC_ROOT, SPARKBOX_SYSFS_ROOT and SPARKBOX_ROOT; <dest>/bin holds the node's own
commands as they answer the cluster user (no root): nvidia-smi, ip, ethtool (refuses Wake-on
without root), nmcli, iw, timedatectl, lldpcli (refused), docker, systemctl, journalctl, apt,
ss, hostname and python3 --version, and stand-ins for fwupdmgr, snap and ollama. <dest>/env
lists the variables to run them with.

What's captured and what's built is in README.md here.
"""
import json
import os
import re
import stat
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
NODES = ("cell1", "cell2", "cell3", "gx10-0000")
#: The cluster's users (uids as on the Sparks where the tour showed them).
USERS = {"root": 0, "daemon": 1, "lp": 7, "www-data": 33, "messagebus": 102, "syslog": 104, "avahi": 108, "kernoops": 110,
         "postfix": 112, "rtkit": 113, "xrdp": 115, "_lldpd": 117, "colord": 119, "gdm": 120, "dgx-dashboard-service-user": 985,
         "ollama": 983, "gnome-remote-desktop": 989, "polkitd": 990, "systemd-resolve": 991, "systemd-oom": 994,
         "systemd-timesync": 996, "sparkuser": 1000}
#: RDMA devices behind the ConnectX-7 netdevs.
RDMA = {"enp1s0f0np0": "rocep1s0f0", "enp1s0f1np1": "rocep1s0f1", "enP2p1s0f0np0": "roceP2p1s0f0", "enP2p1s0f1np1": "roceP2p1s0f1"}


def fixture(node, name, default=""):
    try:
        with open(os.path.join(HERE, node, name)) as f:
            return f.read()
    except OSError:
        return default


def write(path, text, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if mode is not None:
        os.chmod(path, mode)


def network(node):
    """(links: [(iface, state, mac)], addrs: {iface: cidr}, sysfs: {iface: (state, speed, driver, wireless)})."""
    links, addrs, sysfs = [], {}, {}
    for line in fixture(node, "network.txt").splitlines():
        f = line.split()
        m = re.match(r"(\S+) state=(\S*) speed=(\S*) driver=(\S*) wireless=(\S+)", line)
        if m:
            sysfs[m.group(1)] = (m.group(2), m.group(3), m.group(4), m.group(5) == "yes")
        elif len(f) >= 3 and re.match(r"([0-9a-f]{2}:){5}[0-9a-f]{2}$", f[2]):
            links.append((f[0], f[1], f[2]))
        elif len(f) >= 3 and "/" in f[2]:
            addrs[f[0]] = f[2]
    return links, addrs, sysfs


def listening(node):
    """[(address, port)] from `ss -tlnp` as the cluster user ran it."""
    out = []
    for line in fixture(node, "listening.txt").splitlines()[1:]:
        f = line.split()
        if len(f) < 4:
            continue
        addr, _, port = f[3].rpartition(":")
        addr = addr.split("%")[0].strip("[]")
        out.append(("::" if addr == "*" else addr, int(port)))
    return out


def tcp_hex(addr, port, v6):
    if not v6:
        return "%s:%04X" % ("".join("%02X" % int(o) for o in reversed(addr.split("."))), port)
    import socket
    raw = socket.inet_pton(socket.AF_INET6, addr)
    return "%s:%04X" % ("".join(raw[i:i + 4][::-1].hex().upper() for i in range(0, 16, 4)), port)


def build_proc(node, root, facts, procs):
    p = lambda rel, text, mode=None: write(os.path.join(root, rel), text, mode)
    cpu = fixture(node, "cpu.txt").splitlines()
    stat_lines = [l for l in cpu if l.startswith("cpu")]
    base = stat_lines[-1].split()
    cores = [l for l in stat_lines if re.match(r"cpu\d", l)]
    for i in range(len(cores), 20):
        cores.append("cpu%d %s" % (i, " ".join(str(int(v) + i * 7) for v in base[1:])))
    p("stat", "\n".join([stat_lines[0]] + cores) + "\nintr 1\nctxt 1\nbtime 1792960000\n")
    mem = fixture(node, "memory.txt").split("               total")[0]
    p("meminfo", mem + "SReclaimable:     140000 kB\n")
    p("loadavg", next((l for l in cpu if re.match(r"[\d.]+ [\d.]+ [\d.]+ \d+/\d+", l)), "0.00 0.00 0.00 1/600 1") + "\n")
    p("uptime", "%d.42 %d.17\n" % (facts["uptimeS"], facts["uptimeS"] * 19))
    p("sys/kernel/random/boot_id", facts["bootId"] + "\n")
    p("sys/kernel/hostname", facts["hostname"] + "\n")
    p("sys/kernel/osrelease", facts["osrelease"] + "\n")
    p("pressure/memory", "some avg10=0.00 avg60=0.00 avg300=0.00 total=1530\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1204\n")
    p("pressure/io", "some avg10=0.00 avg60=0.02 avg300=0.01 total=8812044\nfull avg10=0.00 avg60=0.02 avg300=0.01 total=8410533\n")
    links, _, _ = network(node)
    dev = ["Inter-|   Receive                                                |  Transmit",
           " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed"]
    for i, (iface, _, _) in enumerate(links):
        n = facts["nets"].get(iface, {})
        dev.append("%s: %d %d %d %d 0 0 0 0 %d %d %d %d 0 0 0 0" % (iface, 9_000_000_000 + i * 77, 8_100_000 + i, n.get("rxErrors", 0), n.get("rxDrops", 0),
                                                                      2_400_000_000 + i * 31, 3_900_000 + i, n.get("txErrors", 0), n.get("txDrops", 0)))
    p("net/dev", "\n".join(dev) + "\n")
    tcp = ["  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode"]
    tcp6 = ["  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode"]
    for i, (addr, port) in enumerate(listening(node)):
        v6 = ":" in addr
        line = "%4d: %s %s 0A 00000000:00000000 00:00000000 00000000     0        0 %d 1 0000000000000000 100 0 0 10 0" % (
            i, tcp_hex(addr, port, v6), ("0" * 32 + ":0000") if v6 else "00000000:0000", 40000 + i)
        (tcp6 if v6 else tcp).append(line)
    p("net/tcp", "\n".join(tcp) + "\n")
    p("net/tcp6", "\n".join(tcp6) + "\n")
    p("diskstats", " 259       0 nvme0n1 812345 1200 98123456 222000 1312345 9800 312345678 921000 0 1022000 1143000 0 0 0 0\n"
                   " 259       1 nvme0n1p1 412 0 22345 120 2 0 8 0 0 140 120 0 0 0 0\n"
                   " 259       2 nvme0n1p2 811800 1200 98000123 221800 1312343 9800 312345670 921000 0 1021800 1142800 0 0 0 0\n")
    p("mounts", "".join(l + "\n" for l in fixture(node, "disks.txt").splitlines() if l.startswith("/dev/")))
    for x in procs:
        pid = x["pid"]
        utime, stime = x["ticks"] * 3 // 4, x["ticks"] - x["ticks"] * 3 // 4
        p("%d/stat" % pid, "%d (%s) %s %d %d %d 0 -1 4194560 100 0 0 0 %d %d 0 0 20 %d %d 0 %d %d %d 18446744073709551615\n" % (
            pid, x["comm"], x["state"], x["ppid"], pid, pid, utime, stime, x["nice"], x["threads"], x["start"], x["rss"] * 3, x["rss"] // 4096))
        p("%d/cmdline" % pid, "".join(a + "\0" for a in x["argv"]))
        uid = USERS.get(x["user"], 0)
        p("%d/status" % pid, "Name:\t%s\nState:\t%s\nPid:\t%d\nPPid:\t%d\nUid:\t%d\t%d\t%d\t%d\nGid:\t%d\t%d\t%d\t%d\n%s" % (
            x["comm"], x["state"], pid, x["ppid"], uid, uid, uid, uid, uid, uid, uid, uid,
            "VmRSS:\t%8d kB\nThreads:\t%d\n" % (x["rss"] // 1024, x["threads"]) if x["argv"] else "Threads:\t1\n"))
        unit = x.get("unit")
        if unit and unit.startswith("session-"):
            cg = "0::/user.slice/user-1000.slice/%s\n" % unit
        elif unit and unit.startswith("user@"):
            cg = "0::/user.slice/user-%s.slice/%s/app.slice/x.service\n" % (unit[5:].split(".")[0], unit)
        elif unit:
            cg = "0::/system.slice/%s\n" % unit
        else:
            cg = "0::/\n"
        p("%d/cgroup" % pid, cg)
        fd = os.path.join(root, "%d/fd" % pid)
        os.makedirs(fd, exist_ok=True)
        if x["user"] != "sparkuser":
            os.chmod(fd, 0)  # someone else's: its fds aren't readable without root


def build_sys(node, root, facts):
    s = lambda rel, text: write(os.path.join(root, rel), text)
    links, _, sysfs = network(node)
    mac = {i: m for i, _, m in links}
    for iface, (state, speed, driver, wireless) in sysfs.items():
        if not driver:
            continue  # lo, docker0, bridges: no device
        n = facts["nets"].get(iface, {})
        base = "class/net/%s" % iface
        s(base + "/operstate", state + "\n")
        s(base + "/speed", (speed or "-1") + "\n")
        s(base + "/mtu", "%d\n" % (n.get("mtu") or 1500))
        s(base + "/address", mac.get(iface, "00:00:00:00:00:00") + "\n")
        s(base + "/carrier_changes", "%d\n" % (n.get("carrier") or 0))
        devdir = os.path.join(root, "devices", iface)
        os.makedirs(devdir, exist_ok=True)
        os.makedirs(os.path.join(root, "drivers", driver), exist_ok=True)
        os.symlink(os.path.join(root, "drivers", driver), os.path.join(devdir, "driver"))
        os.symlink(devdir, os.path.join(root, base, "device"))
        if wireless:
            os.makedirs(os.path.join(root, base, "wireless"), exist_ok=True)
        if iface in RDMA:
            dev = RDMA[iface]
            os.makedirs(os.path.join(devdir, "infiniband", dev), exist_ok=True)
            s("class/infiniband/%s/fw_ver" % dev, facts["cx7Fw"] + "\n")
            for c, v in (("port_rcv_data", 1_234_567), ("port_xmit_data", 2_345_678), ("port_rcv_errors", 0), ("symbol_error", 0),
                         ("link_downed", 0), ("port_xmit_discards", 0)):
                s("class/infiniband/%s/ports/1/counters/%s" % (dev, c), "%d\n" % v)
    s("class/dmi/id/product_name", fixture(node, "model.txt").splitlines()[0] + "\n")
    s("class/dmi/id/bios_version", facts["bios"] + "\n")
    for i, line in enumerate(fixture(node, "thermal.txt").splitlines()):
        kind, _, temp = line.partition(" ")
        s("class/thermal/thermal_zone%d/type" % i, kind + "\n")
        s("class/thermal/thermal_zone%d/temp" % i, temp + "\n")


def build_root(node, root):
    host = fixture(node, "host.txt").splitlines()
    os_lines = [l for l in host if re.match(r"[A-Z_]+=", l) and not l.startswith("DGX_")]
    dgx = [l for l in host if l.startswith("DGX_")]
    write(os.path.join(root, "etc/os-release"), "\n".join(os_lines) + "\n")
    write(os.path.join(root, "etc/dgx-release"), "\n".join(dgx) + "\n")
    write(os.path.join(root, "etc/passwd"), "".join("%s:x:%d:%d::/home/%s:/bin/sh\n" % (u, i, i, u) for u, i in USERS.items()))
    # The fabric's netplan file is root's and 600: the cluster user can't read it.
    write(os.path.join(root, "etc/netplan/40-cx7.yaml"), "network:\n  version: 2\n  ethernets: {}\n", 0)
    os.makedirs(os.path.join(root, "var/run"), exist_ok=True)
    build_standin_etc(node, root)


#: STAND-INS, not captured: the doctor reads these from P1.13 on, and the Live tour never did.
#: Until a real doctor report replaces them, the fake Spark answers with Ubuntu 24.04's stock
#: files; the machine-id is made up (one per node). Nothing here is a fact about the cluster.
STANDIN_SSHD_CONFIG = """# This is the sshd server system-wide configuration file.  See
# sshd_config(5) for more information.
Include /etc/ssh/sshd_config.d/*.conf
#PermitRootLogin prohibit-password
#MaxAuthTries 6
#MaxSessions 10
KbdInteractiveAuthentication no
UsePAM yes
X11Forwarding yes
PrintMotd no
AcceptEnv LANG LC_*
Subsystem\tsftp\t/usr/lib/openssh/sftp-server
"""
STANDIN_GRUB = """GRUB_DEFAULT=0
GRUB_TIMEOUT_STYLE=hidden
GRUB_TIMEOUT=0
GRUB_DISTRIBUTOR=`( . /etc/os-release; echo ${NAME:-Ubuntu} ) 2>/dev/null || echo Ubuntu`
GRUB_CMDLINE_LINUX_DEFAULT="quiet splash"
GRUB_CMDLINE_LINUX=""
"""
STANDIN_UFW = """# /etc/ufw/ufw.conf
#
# Set to yes to start on boot. If setting this remotely, be sure to add a rule
# to allow your remote connection before starting ufw. Eg: 'ufw allow 22/tcp'
ENABLED=no
LOGLEVEL=low
"""


def build_standin_etc(node, root):
    """The stand-in /etc files (STANDIN_*), world-readable as on Ubuntu."""
    import hashlib
    write(os.path.join(root, "etc/ssh/sshd_config"), STANDIN_SSHD_CONFIG)
    os.makedirs(os.path.join(root, "etc/ssh/sshd_config.d"), exist_ok=True)
    write(os.path.join(root, "etc/default/grub"), STANDIN_GRUB)
    write(os.path.join(root, "etc/ufw/ufw.conf"), STANDIN_UFW)
    write(os.path.join(root, "etc/machine-id"), hashlib.md5(("stand-in " + node).encode()).hexdigest() + "\n")


SHIM = r'''#!%(python)s -I
# %(name)s as %(node)s answers the cluster user (spark_tree.py).
import json, os, sys, time
NODE, FIX, PYTHON = %(node)r, %(fix)r, %(python)r
args = sys.argv[1:]
root = os.environ.get("SB_FAKE_ROOT") == "1"
state = os.environ.get("SB_FAKE_STATE", "/nonexistent")
def fixture(name, default=""):
    try:
        with open(os.path.join(FIX, NODE, name)) as f:
            return f.read()
    except OSError:
        return default
def state_file(name, default=None):
    try:
        with open(os.path.join(state, name)) as f:
            return f.read().strip()
    except OSError:
        return default
facts = json.loads(fixture("facts.json", "{}"))
'''

SHIMS = {
    "nvidia-smi": r'''
values = {"utilization.gpu": "0", "temperature.gpu": str(facts["gpu"]["tempC"]), "power.draw": "%.2f" % facts["gpu"]["powerW"],
          "clocks.sm": str(facts["gpu"]["smClockMhz"]), "driver_version": "580.178.04", "clocks.max.sm": str(facts["gpu"]["clockMaxMhz"]),
          "power.limit": "[N/A]", "pstate": facts["gpu"]["pstate"], "name": "NVIDIA GB10",
          "clocks_event_reasons.active": "0x0000000000000001", "clocks_throttle_reasons.active": "0x0000000000000001"}
query = next((a.split("=", 1)[1] for a in args if a.startswith("--query-gpu=")), None)
if any(a.startswith("--query-compute-apps") for a in args):
    sys.exit(0)  # no GPU processes
if query is None:
    sys.stdout.write(fixture("nvidia-smi.txt"))
    sys.exit(0)
line = lambda: ", ".join(values.get(f, "[N/A]") for f in query.split(","))
if "-lms" in args:
    every = int(args[args.index("-lms") + 1]) / 1000.0
    while True:
        print(line(), flush=True)
        time.sleep(every)
print(line())
''',
    "ip": r'''
links, addrs = [], {}
for l in fixture("network.txt").splitlines():
    f = l.split()
    if len(f) >= 3 and f[2].count(":") == 5:
        links.append(f)
    elif len(f) >= 3 and "/" in f[2]:
        addrs[f[0]] = f
words = [a for a in args if not a.startswith("-")]
if "-j" in args and words[:1] in (["addr"], ["a"]):
    print(json.dumps([{"ifname": i, "operstate": f[1], "addr_info": [{"family": "inet", "local": f[2].split("/")[0], "prefixlen": int(f[2].split("/")[1])}]}
                      for i, f in addrs.items()]))
elif "-br" in args and words[:1] == ["link"]:
    print("\n".join("%-16s %-14s %s %s" % tuple(f[:4]) for f in links))
elif "-br" in args:
    print("\n".join("%-16s %-14s %s" % tuple(f[:3]) for f in addrs.values()))
elif words[:2] == ["route", "show"] or words[:1] == ["route"]:
    print("default via 192.168.50.1 dev enP7s7 proto dhcp src %s metric 100" % addrs["enP7s7"][2].split("/")[0])
    if "wlP9s9" in addrs:
        print("default via 192.168.50.1 dev wlP9s9 proto dhcp src %s metric 600" % addrs["wlP9s9"][2].split("/")[0])
elif "-o" in args and words[:1] == ["link"]:
    for n, f in enumerate(links, 1):
        print("%d: %s: %s mtu %d state %s\\    link/ether %s" % (n, f[0], f[3], facts["nets"].get(f[0], {}).get("mtu") or 1500, f[1], f[2]))
elif "-o" in args:
    for n, (i, f) in enumerate(addrs.items(), 1):
        print("%d: %s    inet %s scope global %s\\       valid_lft forever preferred_lft forever" % (n, i, f[2], i))
else:
    sys.exit("ip: the fake Spark answers -j addr, -br, -o link/addr and route show")
''',
    "ethtool": r'''
iface = args[-1] if args else ""
if args[:1] == ["-s"]:
    if not root:
        sys.exit("Cannot get current wake-on-lan settings: Operation not permitted")
    open(os.path.join(state, "wol"), "w").write(args[args.index("wol") + 1] if "wol" in args else "d")
    sys.exit(0)
if args[:1] in (["-S"], ["-i"], ["-m"]):
    sys.exit(0)
print("Settings for %s:" % iface)
print("\tSupported ports: [ TP MII ]\n\tSupported link modes:   10baseT/Half 10baseT/Full\n\t                        100baseT/Half 100baseT/Full\n\t                        1000baseT/Full")
print("\tSpeed: 1000Mb/s\n\tDuplex: Full\n\tAuto-negotiation: on\n\tPort: Twisted Pair")
if root:
    print("\tSupports Wake-on: pumbg\n\tWake-on: %s" % (state_file("wol") or "g"))
else:
    print("Cannot get wake-on-lan settings: Operation not permitted", file=sys.stderr)
print("\tLink detected: yes")
''',
    "nmcli": r'''
if state_file("nmcli") == "missing":
    sys.exit("Error: NetworkManager is not running.")
if args[:3] == ["-g", "GENERAL.CONNECTION", "device"]:
    print("netplan-enP7s7" if args[-1] == "enP7s7" else "netplan-wlP9s9" if args[-1] == "wlP9s9" and NODE == "cell3" else "")
elif args[:2] == ["-g", "802-3-ethernet.wake-on-lan"]:
    if args[-1] != "netplan-enP7s7":
        sys.exit("Error: %s - no such connection profile." % args[-1])
    print(state_file("nm-wol", "default"))
elif args[:2] == ["radio", "wifi"] and len(args) == 2:
    print("enabled")
else:
    print("")
''',
    "iw": r'''
if args[:1] == ["dev"] and args[2:3] == ["link"]:
    text = fixture("iw-link.txt") if args[1] == "wlP9s9" else ""
    sys.stdout.write(text or "Not connected.\n")
else:
    sys.exit("iw: the fake Spark answers dev <if> link")
''',
    "timedatectl": r'''
if args[:1] == ["show"]:
    print("yes" if "--value" in args else "NTPSynchronized=yes")
elif args[:1] == ["timesync-status"]:
    sys.stdout.write(fixture("timesync.txt"))
else:
    print("               Local time: Thu 2026-10-29 07:15:30 EDT\nSystem clock synchronized: yes\n              NTP service: active")
''',
    "lldpcli": r'''
# lldpd's socket is root:_lldpd 0660: the cluster user isn't let in.
sys.exit("lldpctl: unable to connect to socket /run/lldpd.socket: Permission denied")
''',
    "docker": r'''
cs = facts.get("containers", [])
def find(ref):
    return next((c for c in cs if c["name"] == ref or c["id"].startswith(ref) or ref.startswith(c["id"])), None)
words = [a for a in args if not a.startswith("-")]
if args[:1] == ["version"]:
    print(facts["docker"])
elif words[:1] == ["ps"]:
    for c in cs:
        print("%s %s %s %s Exited (%d) 2 days ago" % (c["id"], c["name"], c["image"], c["state"], c["exitCode"]))
elif words[:1] == ["inspect"]:
    found = [find(w) for w in words[1:] if w != "container"]
    if not all(found):
        sys.exit("Error: No such object: %s" % words[-1])
    raw = fixture("inspect-%s.json" % found[0]["name"])
    print(raw if raw else json.dumps([{"Id": c["id"], "Name": "/" + c["name"], "Config": {"Image": c["image"], "Env": []},
                                       "State": {"Status": c["state"], "ExitCode": c["exitCode"]}} for c in found]))
elif words[:1] == ["logs"]:
    c = find(words[-1])
    if not c:
        sys.exit("Error: No such container: %s" % words[-1])
    log = ""
    if c["name"] == "glm53" and NODE == "cell1":
        try:
            log = open(os.path.join(FIX, "..", "..", "..", "models", "fixtures", "logs", "glm53-rank0.log")).read()
        except OSError:
            pass
    lines = log.splitlines(True)
    if "--tail" in args:
        lines = lines[-int(args[args.index("--tail") + 1]):]
    sys.stderr.write("".join(lines))  # vLLM logs to stderr
elif words[:1] == ["info"]:
    # STAND-IN (spark_tree.py's STANDIN_* note): the image store wasn't captured. overlay2 is
    # what an install upgraded to Docker 29 keeps; a fresh one would show the containerd store.
    driver, status = "overlay2", [["Backing Filesystem", "extfs"], ["Supports d_type", "true"], ["Using metacopy", "false"],
                                  ["Native Overlay Diff", "true"], ["userxattr", "false"]]
    fmt = next((args[i + 1] for i, a in enumerate(args) if a in ("-f", "--format") and i + 1 < len(args)), None)
    if fmt is None:
        print("Server:\n Server Version: %s\n Storage Driver: %s" % (facts["docker"], driver))
    elif fmt == "{{.Driver}} {{json .DriverStatus}}":
        print("%s %s" % (driver, json.dumps(status, separators=(",", ":"))))
    else:
        sys.exit("docker: the fake Spark answers info -f '{{.Driver}} {{json .DriverStatus}}' only")
else:
    sys.exit("docker: the fake Spark answers version, ps, inspect, logs and info")
''',
    "systemctl": r'''
units = json.loads(fixture("services.json", "[]"))
by = {u["unit"]: u for u in units}
words = [a for a in args if not a.startswith("-")]
TIMERS = {"anacron.service": "anacron.timer", "apt-daily.service": "apt-daily.timer", "apt-daily-upgrade.service": "apt-daily-upgrade.timer",
          "fstrim.service": "fstrim.timer", "logrotate.service": "logrotate.timer", "man-db.service": "man-db.timer",
          "motd-news.service": "motd-news.timer", "e2scrub_all.service": "e2scrub_all.timer", "fwupd-refresh.service": "fwupd-refresh.timer",
          "sysstat-collect.service": "sysstat-collect.timer", "update-notifier-download.service": "update-notifier-download.timer"}
def kind(u):
    return "oneshot" if u["sub"] == "exited" or u["unit"] in TIMERS else "dbus" if u["unit"] in ("dbus.service", "polkit.service") else "simple"
if "--failed" in args:
    print("[]" if "--output=json" in args else "")
elif words[:1] == ["list-units"]:
    print(json.dumps([{"unit": u["unit"], "load": "loaded", "active": u["active"], "sub": u["sub"], "description": u["description"]} for u in units]))
elif words[:1] == ["list-unit-files"]:
    print(json.dumps([{"unit_file": u["unit"], "state": u["enabled"] if u["enabled"] != "?" else "indirect", "preset": "enabled"} for u in units
                      if "@" not in u["unit"] or u["unit"].endswith("@.service")]))
elif words[:1] == ["show"] and any(w in by or "*" in w for w in words[1:]):
    props = []
    for i, a in enumerate(args):
        if a in ("-p", "--property"):
            props += args[i + 1].split(",")
        elif a.startswith("--property="):
            props += a[11:].split(",")
    blocks = []
    names = [u["unit"] for u in units] if any("*" in w for w in words[1:]) else words[1:]
    for w in names:
        u = by.get(w)
        if not u:
            continue
        v = {"Id": w, "Type": kind(u), "TriggeredBy": TIMERS.get(w, ""), "ActiveState": u["active"], "SubState": u["sub"], "Result": "success",
             "UnitFileState": u["enabled"] if u["enabled"] != "?" else "", "LoadState": "loaded", "Description": u["description"],
             # gx10-0000 has no systemd-networkd to dispatch for: its start condition isn't met.
             "ConditionResult": "no" if w == "networkd-dispatcher.service" and u["active"] == "inactive" else "yes",
             "MainPID": "2175" if w == "ollama.service" else "0"}
        blocks.append("\n".join("%s=%s" % (p, v.get(p, "")) for p in (props or sorted(v))))
    print("\n\n".join(blocks))
elif words[:1] == ["status"] and words[-1] in by:
    u = by[words[-1]]
    print("● %s - %s\n     Loaded: loaded (/usr/lib/systemd/system/%s; %s; preset: enabled)\n     Active: %s (%s) since Sun 2026-10-25 17:08:10 EDT; 3 days ago" % (
        u["unit"], u["description"], u["unit"], u["enabled"], u["active"], u["sub"]))
    if u["unit"] == "ollama.service":
        print("   Main PID: 2175 (ollama)\n      Tasks: 12 (limit: 154069)\n     Memory: 20.1M (peak: 554.8M swap: 2.8M swap peak: 2.8M)\n        CPU: 7.259s")
    sys.exit(0 if u["active"] == "active" else 3)
else:
    os.execv(os.path.join(os.environ["SB_FAKE_BASE_BIN"], "systemctl"), ["systemctl"] + args)
''',
    "journalctl": r'''
unit = args[args.index("-u") + 1] if "-u" in args else None
n = int(args[args.index("-n") + 1]) if "-n" in args else None
if "-k" in args or "--dmesg" in args:
    # The kernel log is readable (the cluster user is in adm); nothing new arrives here.
    print(json.dumps({"__REALTIME_TIMESTAMP": "1792962489357000", "__CURSOR": "s=1;i=1", "MESSAGE": "Booting Linux on physical CPU 0x0000000000 [0x410fd871]"}))
    sys.stdout.flush()
    if "-f" in args:
        # A real follower dies of SIGPIPE at its next line once its reader is gone; this one
        # never writes again, so it ends when its reader (the collector) is gone instead.
        parent = os.getppid()
        while os.getppid() == parent:
            time.sleep(1)
    sys.exit(0)
lines = fixture("journal-%s.txt" % (unit or "").replace(".service", "")).splitlines()
if n is not None:
    lines = lines[-n:]
if "json" in args:
    import calendar
    for l in lines:
        mon, day, hms, host, rest = l.split(" ", 4)
        ident, _, msg = rest.partition(": ")
        name, _, pid = ident.rstrip("]").partition("[")
        t = calendar.timegm(time.strptime("2026 %s %s %s" % (mon, day, hms), "%Y %b %d %H:%M:%S")) + 4 * 3600
        print(json.dumps({"__REALTIME_TIMESTAMP": str(t * 1000000), "PRIORITY": "6", "SYSLOG_IDENTIFIER": name, "_PID": pid, "MESSAGE": msg}))
else:
    print("\n".join(lines))
''',
    "apt": r'''
if args[:2] == ["list", "--upgradable"]:
    sys.stdout.write(fixture("apt-upgradable.txt"))
else:
    sys.exit("E: the fake Spark answers apt list --upgradable")
''',
    "apt-mark": r'''
sys.exit(0)  # nothing held
''',
    "apt-config": r'''
# `apt-config dump [prefix]`. Unattended-Upgrade "0" is as the Updates window read it; the rest
# of APT::Periodic is a STAND-IN (Ubuntu 24.04's defaults: spark_tree.py's STANDIN_* note).
conf = ['APT::Periodic "";', 'APT::Periodic::Update-Package-Lists "1";', 'APT::Periodic::Download-Upgradeable-Packages "0";',
        'APT::Periodic::AutocleanInterval "0";', 'APT::Periodic::Unattended-Upgrade "0";']
prefix = next((a for a in args[1:] if not a.startswith("-")), "") if args[:1] == ["dump"] else "APT::Periodic::Unattended-Upgrade"
for line in conf:
    if line.split(" ")[0] == prefix or line.startswith(prefix + "::") or not prefix:
        print(line)
''',
    "grep": r'''
# /etc is the node's (SPARKBOX_ROOT/etc), as for cat: sshd_config, default/grub, ufw/ufw.conf.
# Anything else is this Mac's own grep on its own files.
import subprocess
top = os.environ.get("SPARKBOX_ROOT", "")
mapped = [top + a if top and a.startswith("/etc/") else a for a in args]
p = subprocess.run(["/usr/bin/grep"] + mapped, capture_output=True)
for stream, data in ((sys.stdout, p.stdout), (sys.stderr, p.stderr)):
    stream.buffer.write(data.replace((top + "/etc/").encode(), b"/etc/") if top else data)
sys.exit(p.returncode)
''',
    "ss": r'''
text = fixture("listening.txt")
if "-H" in "".join(a for a in args if a.startswith("-") and not a.startswith("--")):
    text = "\n".join(text.splitlines()[1:]) + "\n"
sys.stdout.write(text)  # no process column: that needs root
''',
    "cat": r'''
# /etc is the node's (SPARKBOX_ROOT/etc): /etc/netplan/40-cx7.yaml is root's and 600.
code = 0
for a in args:
    if not a.startswith("/etc/"):
        continue
    try:
        with open(os.environ.get("SPARKBOX_ROOT", "") + a) as f:
            sys.stdout.write(f.read())
    except PermissionError:
        print("cat: %s: Permission denied" % a, file=sys.stderr)
        code = 1
    except OSError:
        print("cat: %s: No such file or directory" % a, file=sys.stderr)
        code = 1
if not any(a.startswith("/etc/") for a in args):
    os.execv("/bin/cat", ["cat"] + args)
sys.exit(code)
''',
    "hostname": r'''
print(facts["hostname"])
''',
    # STAND-INS (spark_tree.py's STANDIN_* note) for the update check's reads (ops/update_check.rs
    # CHECK_SCRIPT), which the Live tour didn't capture: fwupd has nothing to do (exit 2), every snap
    # is current, and `ollama --version` gives the version cell1's journal logged
    # (rounds/r4 text-service-ollama-cell1.txt: "version 0.21.0"), on cell3 as well. cell2 and
    # gx10-0000 run no Ollama: the shell can't find it.
    "fwupdmgr": r'''
print("No updatable devices")
sys.exit(2)
''',
    "snap": r'''
if args[:2] != ["refresh", "--list"]:
    sys.exit("error: the fake Spark answers snap refresh --list only")
print("All snaps up to date.")
''',
    "ollama": r'''
if NODE not in ("cell1", "cell3"):
    sys.stderr.write("sh: 1: ollama: not found\n")
    sys.exit(127)
if args not in (["--version"], ["-v"]):
    sys.exit("ollama: the fake Spark answers --version only")
print("ollama version is 0.21.0")
''',
    "python3": r'''
if args == ["--version"]:
    print("Python 3.12.3")
    sys.exit(0)
os.execv(PYTHON, [PYTHON] + args)
''',
}


def build_bin(node, dest):
    fix = HERE
    for name, body in SHIMS.items():
        path = os.path.join(dest, name)
        write(path, SHIM % {"python": sys.executable, "name": name, "node": node, "fix": fix} + body)
        os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def build(node, dest):
    """Builds the trees for `node` under `dest`; returns the variables collector.py runs with."""
    facts = json.loads(fixture(node, "facts.json"))
    procs = json.loads(fixture(node, "processes.json"))
    build_proc(node, os.path.join(dest, "proc"), facts, procs)
    build_sys(node, os.path.join(dest, "sys"), facts)
    build_root(node, os.path.join(dest, "root"))
    build_bin(node, os.path.join(dest, "bin"))
    env = {"SPARKBOX_PROC_ROOT": os.path.join(dest, "proc"), "SPARKBOX_SYSFS_ROOT": os.path.join(dest, "sys"),
           "SPARKBOX_ROOT": os.path.join(dest, "root")}
    write(os.path.join(dest, "env"), "".join("%s=%s\n" % kv for kv in env.items()))
    return env


def unlock(dest):
    """Gives back the folders made unreadable (someone else's fds), so `dest` can be removed."""
    for d, dirs, _ in os.walk(dest):
        for x in dirs:
            p = os.path.join(d, x)
            if not os.path.islink(p):
                os.chmod(p, 0o755)
    netplan = os.path.join(dest, "root/etc/netplan/40-cx7.yaml")
    if os.path.exists(netplan):
        os.chmod(netplan, 0o600)


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in NODES:
        sys.exit("usage: spark_tree.py {%s} <dest>" % ",".join(NODES))
    build(sys.argv[1], sys.argv[2])
