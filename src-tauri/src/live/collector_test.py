"""Tests for collector.py's parsers, using text shaped like DGX OS output.
Run: python3 -m unittest src/live/collector_test.py (from src-tauri/)."""
import importlib
import json
import os
import shutil
import sys
import tempfile
import threading
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(__file__))
import collector as c  # noqa: E402

STAT_A = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 1\n"
STAT_B = "cpu  200 0 100 850 50 0 0 0 0 0\ncpu0 150 0 75 400 25 0 0 0 0 0\ncpu1 50 0 25 450 25 0 0 0 0 0\nintr 2\n"

MEMINFO = """MemTotal:       128528384 kB
MemFree:         2000000 kB
MemAvailable:   20000000 kB
Buffers:          100000 kB
Cached:          9000000 kB
SwapCached:            0 kB
SReclaimable:     900000 kB
SwapTotal:       4194304 kB
SwapFree:        4194304 kB
HugePages_Total:       0
"""

NET_DEV = """Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0
enp1s0f1np1: 5000000 100 3 7 0 0 0 0 7000000 200 2 9 0 0 0 0
"""

DISKSTATS = " 259       0 nvme0n1 100 0 2048 10 50 0 4096 20 0 30 30 0 0 0 0\n 259       2 nvme0n1p2 90 0 1024 9 40 0 2048 10 0 20 20 0 0 0 0\n"

MOUNTS = """/dev/nvme0n1p2 / ext4 rw,relatime 0 0
proc /proc proc rw 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
/dev/nvme0n1p2 /var/lib/docker ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
"""

FAILED_JSON = json.dumps([
    {"unit": "nvidia-dgx-telemetry.service", "load": "loaded", "active": "failed", "sub": "failed",
     "description": "NVIDIA DGX telemetry"},
    {"unit": "fwupd-refresh.service", "load": "loaded", "active": "failed", "sub": "failed", "description": "Refresh fwupd"},
])

FAILED_PLAIN = """nvidia-dgx-telemetry.service loaded failed failed NVIDIA DGX telemetry
● fwupd-refresh.service      loaded failed failed Refresh fwupd metadata
"""

CHRONY = """Reference ID    : C0A82D01 (192.168.50.1)
Stratum         : 3
System time     : 0.000183421 seconds slow of NTP time
Last offset     : -0.000021014 seconds
"""


#: Threads already running when the current test began.
before = set()


def threads_left(wait=10):
    """Waits for the threads the current test started to end, and names those that didn't."""
    until = c.time.monotonic() + wait
    started = [t for t in threading.enumerate() if t not in before and t is not threading.current_thread()]
    for t in started:
        t.join(max(until - c.time.monotonic(), 0))
    return sorted(t.name for t in started if t.is_alive())


class Case(unittest.TestCase):
    """Every test here ends with its threads ended. The collector's threads run forever and call
    nvidia-smi and journalctl from PATH, so one left behind starts a later test's fake too."""

    def run(self, result=None):
        before.clear()
        before.update(threading.enumerate())
        # agent_test.py's subclasses run the agent, whose threads aren't this file's.
        if type(self).__module__ == __name__:
            self.addCleanup(lambda: self.assertEqual(threads_left(), []))
        return super().run(result)


def reloaded():
    """collector.py loaded again with its thread loops cut short, so a Collector can be built
    freely: the sampler takes one reading and stops, the stream and the kernel log don't start."""
    mod = importlib.reload(c)

    class Sampler(mod.GpuSampler):
        def run(self):
            try:
                self.read_once()
            finally:
                self.ready.set()

    class Stream(mod.GpuStream):
        def run(self):
            self.alive = False

    class Watcher(mod.KernelWatcher):
        def run(self):
            pass

    mod.GpuSampler, mod.GpuStream, mod.KernelWatcher = Sampler, Stream, Watcher
    return mod


class Parsers(Case):
    def test_cpu(self):
        total, cores = c.cpu_percent(c.parse_stat(STAT_A), c.parse_stat(STAT_B))
        self.assertEqual(total, 75.0)  # busy +150 of +200 jiffies
        self.assertEqual(cores, [100.0, 0.0])

    def test_memory_uses_available_not_free(self):
        m = c.memory(c.parse_meminfo(MEMINFO))
        self.assertEqual(m["total"], 128528384 * 1024)
        self.assertEqual(m["used"], (128528384 - 20000000) * 1024)
        self.assertEqual(m["cached"], (9000000 + 100000 + 900000) * 1024)
        self.assertEqual(m["swapUsed"], 0)

    def test_swap_in_is_a_rate_not_occupancy_and_unknown_is_not_zero(self):
        self.assertEqual(c.swap_pages_in("nr_free_pages 22\npswpin 100\npswpout 200\n"), 100)
        for text in ("", "pswpin bad", "pswpin -1", "pswpout 100"):
            self.assertIsNone(c.swap_pages_in(text))
        self.assertEqual(c.swap_in_rate(100, 612, 2, 4096), 1048576)
        self.assertEqual(c.swap_in_rate(100, 100, 2, 65536), 0)
        for prev, cur, dt in ((None, 100, 2), (100, None, 2), (100, 2, 2), (100, 200, 0), (100, 200, 16)):
            self.assertIsNone(c.swap_in_rate(prev, cur, dt, 4096))

    def test_net_and_disk(self):
        n = c.parse_net_dev(NET_DEV)["enp1s0f1np1"]
        self.assertEqual((n["rx"], n["tx"]), (5000000, 7000000))
        self.assertEqual(c.parse_diskstats(DISKSTATS)["nvme0n1p2"], (1024 * 512, 2048 * 512))

    def test_netdev_errors_and_drops(self):
        n = c.parse_net_dev(NET_DEV)["enp1s0f1np1"]
        self.assertEqual((n["rxErrors"], n["rxDrops"], n["txErrors"], n["txDrops"]), (3, 7, 2, 9))

    def test_counter_deltas_unwrap_32_bit_and_ignore_resets(self):
        self.assertEqual(c.counter_delta(100, 150), 50)
        self.assertEqual(c.counter_delta(2 ** 32 - 10, 5), 15)
        self.assertEqual(c.counter_delta(2 ** 40, 7), 0)  # a 64-bit counter went back: reset, not traffic
        self.assertEqual(c.counter_delta(None, 7), 0)

    def test_disk_space_is_what_df_reports(self):
        """The four root disks of the Live tour: statvfs -> total, used and avail exactly as
        `df -B1 /` printed them (doctor.txt), and df's Use% from used / (used + avail)."""
        class St:
            def __init__(self, **kw):
                self.__dict__.update(kw)
        for d in json.loads(load(HERE, "fixtures", "disks-doctor.json"))["disks"]:
            got = c.disk_space(St(**d["statvfs"]))
            df = d["df"]
            self.assertEqual(got, {"total": df["total"], "used": df["used"], "avail": df["avail"]}, d["node"])
            self.assertEqual(-(-got["used"] * 100 // (got["used"] + got["avail"])), df["usePct"], d["node"])
        gx10 = next(d for d in json.loads(load(HERE, "fixtures", "disks-doctor.json"))["disks"] if d["node"] == "gx10-0000")
        free = c.disk_space(St(**gx10["statvfs"]))["avail"]
        self.assertEqual(round(free / 2 ** 30, 1), 520.5, "not the 567 GiB that total minus used gives")
        self.assertEqual(round((gx10["df"]["total"] - gx10["df"]["used"]) / 2 ** 30, 1), 567.1)

    def test_mounts_keep_real_filesystems_once(self):
        self.assertEqual(c.parse_mounts(MOUNTS), [("/dev/nvme0n1p2", "/", "ext4"), ("/dev/nvme0n1p1", "/boot/efi", "vfat")])

    def test_nvidia_smi_with_unsupported_fields(self):
        g = c.parse_gpu("96, 71, [N/A], 2418, 580.95.05\n")
        self.assertEqual((g["utilPct"], g["tempC"], g["powerW"], g["smClockMhz"], g["driver"]), (96.0, 71.0, 0.0, 2418, "580.95.05"))
        self.assertEqual(c.parse_gpu("")["utilPct"], 0.0)

    def test_gpu_extras_tolerate_na_per_field(self):
        g = c.parse_gpu_extra("3003, [N/A], P0, 0x0000000000000044\n")
        self.assertEqual(g, {"clockMaxMhz": 3003, "powerLimitW": None, "pstate": "P0",
                             "throttle": ["SW power cap", "HW thermal slowdown"]})
        g = c.parse_gpu_extra("[N/A], 140.00, [N/A], [N/A]")
        self.assertEqual((g["clockMaxMhz"], g["powerLimitW"], g["pstate"], g["throttle"]), (None, 140.0, None, []))
        self.assertEqual(c.parse_gpu_extra("")["throttle"], [])

    def test_throttle_reasons_decode(self):
        self.assertEqual(c.decode_throttle("0x0000000000000001"), ["GPU idle"])
        self.assertEqual(c.decode_throttle("0x0000000000000000"), [])
        self.assertEqual(c.decode_throttle("0x00000000000000A8"), ["HW slowdown", "SW thermal slowdown", "HW power brake slowdown"])

    def test_gpu_processes(self):
        apps = c.parse_apps("4242, /usr/bin/python3, 88123\n17, ollama, [N/A]\n")
        self.assertEqual(apps[0], {"pid": 4242, "name": "python3", "mem": 88123 * 1048576})
        self.assertEqual(apps[1]["mem"], 0)

    def test_failed_units(self):
        self.assertEqual(c.parse_failed_units(FAILED_JSON), ["fwupd-refresh.service", "nvidia-dgx-telemetry.service"])
        self.assertEqual(c.parse_failed_units(FAILED_PLAIN), ["fwupd-refresh.service", "nvidia-dgx-telemetry.service"])
        self.assertEqual(c.parse_failed_units("[]"), [])
        self.assertEqual(c.parse_failed_units(None), [])

    def test_container_from_cgroup(self):
        cid = "a" * 64
        self.assertEqual(c.container_of("0::/system.slice/docker-%s.scope\n" % cid), cid)
        self.assertEqual(c.container_of("0::/docker/%s\n" % cid), cid)
        self.assertIsNone(c.container_of("0::/user.slice/user-1000.slice/session-3.scope\n"))

    def test_docker_details(self):
        self.assertEqual(c.docker_time("1970-01-02T00:00:00.123456789Z"), 86400)
        self.assertEqual(c.docker_time("0001-01-01T00:00:00Z"), 0)
        self.assertEqual(
            c.ports([{"IP": "0.0.0.0", "PrivatePort": 8000, "PublicPort": 8000, "Type": "tcp"},
                     {"IP": "::", "PrivatePort": 8000, "PublicPort": 8000, "Type": "tcp"},
                     {"PrivatePort": 6379, "Type": "tcp"}]),
            ["0.0.0.0:8000->8000/tcp", "6379/tcp"])

    def test_inspect_meta(self):
        info = {"RestartCount": 4, "HostConfig": {"RestartPolicy": {"Name": "unless-stopped"}},
                "State": {"StartedAt": "1970-01-02T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z", "ExitCode": 137,
                          "OOMKilled": True, "Health": {"Status": "unhealthy"}}}
        self.assertEqual(c.inspect_meta(info), {"imageId": None, "envFlags": {}, "startedAt": 86400, "finishedAt": 0, "exitCode": 137, "health": "unhealthy",
                                                "restartCount": 4, "oomKilled": True, "restartPolicy": "unless-stopped"})
        self.assertEqual(c.inspect_meta({})["restartPolicy"], None)

    def test_env_flags_keep_only_the_nccl_settings(self):
        env = ["PATH=/usr/bin", "HF_TOKEN=hf_abc", "NCCL_SOCKET_IFNAME=enp1s0f1np1", "NCCL_IB_HCA=rocep1s0f1,roceP2p1s0f1",
               "VLLM_HOST_IP=192.168.100.11", "NCCL_IB_DISABLE=0", "NCCL_DEBUG_TOKEN=abc", "BROKEN"]
        self.assertEqual(c.inspect_meta({"Config": {"Env": env}})["envFlags"], {
            "NCCL_SOCKET_IFNAME": "enp1s0f1np1", "NCCL_IB_HCA": "rocep1s0f1,roceP2p1s0f1",
            "VLLM_HOST_IP": "192.168.100.11", "NCCL_IB_DISABLE": "0", "NCCL_DEBUG_TOKEN": "(hidden)"})

    def test_systemd_unit_from_cgroup(self):
        self.assertEqual(c.unit_of("0::/system.slice/ollama.service\n"), "ollama.service")
        self.assertIsNone(c.unit_of("0::/user.slice/user-1000.slice/session-4.scope\n"))
        self.assertIsNone(c.unit_of("0::/system.slice/docker-%s.scope\n" % ("a" * 64)))

    def test_pid_stat_with_spaces_in_the_name(self):
        line = "4242 (python3 (vllm)) S 1 4242 4242 0 -1 4194560 100 0 0 0 750 250 0 0 20 5 40 0 12345 9000000 2048 18446744073709551615"
        self.assertEqual(c.parse_pid_stat(line), {"name": "python3 (vllm)", "state": "S", "ppid": 1, "ticks": 1000, "nice": 5,
                                                  "threads": 40, "start": 12345, "rss": 2048})
        self.assertIsNone(c.parse_pid_stat("garbage"))

    def test_listening_sockets(self):
        tcp = ("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
               "   0: 00000000:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 55501 1\n"
               "   1: 0100007F:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1201 1\n"
               "   2: 0100007F:9C40 0200A8C0:0016 01 00000000:00000000 00:00000000 00000000  1000        0 7777 1\n")
        self.assertEqual(c.parse_listeners(tcp), {"55501": ("0.0.0.0", 8000), "1201": ("127.0.0.1", 22)})
        tcp6 = ("header\n   0: 00000000000000000000000000000000:2CAA 00000000000000000000000000000000:0000 0A "
                "00000000:00000000 00:00000000 00000000  1000 0 900 1\n")
        self.assertEqual(c.parse_listeners(tcp6), {"900": ("::", 11434)})

    def test_cuda_version(self):
        self.assertEqual(c.cuda_version("| NVIDIA-SMI 580.95.05   Driver Version: 580.95.05   CUDA Version: 13.0 |"), "13.0")

    def test_clock_offset(self):
        self.assertEqual(c.parse_clock_offset(CHRONY), -0.183)
        self.assertIsNone(c.parse_clock_offset(""))


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


class SysfsTree(Case):
    """A fake /sys with two ConnectX-7 halves (one with 64-bit ext counters), an unused half
    without an RDMA device, and the LAN port."""

    def setUp(self):
        self.root = tempfile.mkdtemp()
        os.environ["SPARKBOX_SYSFS_ROOT"] = self.root
        self.c = reloaded()
        sysd = lambda p: os.path.join(self.root, p)
        for iface, dev in (("enp1s0f1np1", "rocep1s0f1"), ("enP2p1s0f1np1", "roceP2p1s0f1"), ("enp1s0f0np0", None), ("enP7s7", None)):
            base = sysd("devices/%s" % iface)
            os.makedirs(base)
            os.makedirs(sysd("class/net/%s" % iface))
            os.symlink(base, sysd("class/net/%s/device" % iface))
            driver = "mlx5_core" if iface != "enP7s7" else "r8169"
            os.symlink(sysd("drivers/%s" % driver), os.path.join(base, "driver"))
            write(sysd("class/net/%s/operstate" % iface), "up\n" if iface != "enp1s0f0np0" else "down\n")
            write(sysd("class/net/%s/speed" % iface), "200000\n" if driver == "mlx5_core" else "10000\n")
            write(sysd("class/net/%s/mtu" % iface), "9000\n" if driver == "mlx5_core" else "1500\n")
            write(sysd("class/net/%s/address" % iface), "4c:bb:47:00:00:01\n")
            write(sysd("class/net/%s/carrier_changes" % iface), "4\n")
            if dev:
                os.makedirs(os.path.join(base, "infiniband", dev))
        os.makedirs(sysd("drivers/mlx5_core"), exist_ok=True)
        os.makedirs(sysd("drivers/r8169"), exist_ok=True)
        self.counters("rocep1s0f1", rcv=1000, xmit=2000)
        write(sysd("class/infiniband/roceP2p1s0f1/ports/1/counters_ext/port_rcv_data_64"), "500\n")
        write(sysd("class/infiniband/roceP2p1s0f1/ports/1/counters_ext/port_xmit_data_64"), "600\n")
        write(sysd("class/infiniband/roceP2p1s0f1/ports/1/counters/port_rcv_data"), "1\n")
        write(sysd("class/infiniband/rocep1s0f1/fw_ver"), "28.43.1014\n")

    def tearDown(self):
        del os.environ["SPARKBOX_SYSFS_ROOT"]
        importlib.reload(c)
        shutil.rmtree(self.root)

    def counters(self, dev, rcv, xmit):
        base = os.path.join(self.root, "class/infiniband/%s/ports/1/counters" % dev)
        write(base + "/port_rcv_data", "%d\n" % rcv)
        write(base + "/port_xmit_data", "%d\n" % xmit)

    def test_interfaces_map_to_rdma_devices(self):
        found = {i["name"]: i for i in self.c.interfaces()}
        self.assertEqual(found["enp1s0f1np1"]["rdma"], "rocep1s0f1")
        self.assertEqual(found["enP2p1s0f1np1"]["rdma"], "roceP2p1s0f1")
        self.assertIsNone(found["enp1s0f0np0"]["rdma"], "an uncabled half without an RDMA device")
        self.assertEqual(found["enp1s0f1np1"]["role"], "fabric")
        self.assertEqual(found["enP7s7"]["role"], "lan")
        self.assertEqual((found["enp1s0f1np1"]["mtu"], found["enp1s0f1np1"]["carrier"]), (9000, 4))
        self.assertEqual(found["enp1s0f0np0"]["speed"], 0, "a down link has no speed")

    def test_rdma_counters_are_4_byte_words_and_prefer_ext(self):
        self.assertEqual(self.c.rdma_bytes("rocep1s0f1"), (4000, 8000))
        self.assertEqual(self.c.rdma_bytes("roceP2p1s0f1"), (2000, 2400), "the 64-bit ext counters win")
        self.assertEqual(self.c.rdma_bytes("mlx5_missing"), (None, None))

    def test_rdma_rate_in_bits_and_through_a_wrap(self):
        before = self.c.rdma_bytes("rocep1s0f1")
        self.counters("rocep1s0f1", rcv=1000 + 25_000_000, xmit=2000)
        rx, tx = self.c.rdma_rate(before, self.c.rdma_bytes("rocep1s0f1"), 1.0)
        self.assertEqual(rx, 25_000_000 * 4 * 8)  # 800 Mb/s
        self.assertEqual(tx, 0)
        self.counters("rocep1s0f1", rcv=2 ** 32 - 100, xmit=0)
        a = self.c.rdma_bytes("rocep1s0f1")
        self.counters("rocep1s0f1", rcv=150, xmit=0)
        rx, _ = self.c.rdma_rate(a, self.c.rdma_bytes("rocep1s0f1"), 2.0)
        self.assertEqual(rx, 250 * 4 * 8 / 2.0)
        self.assertEqual(self.c.rdma_rate((None, None), (None, None), 1.0), (None, None))

    def test_firmware_comes_from_the_rdma_device(self):
        self.assertEqual(self.c.system_extras()["cx7Fw"], "28.43.1014")


class WholeFrame(SysfsTree):
    """One full frame from fixture /proc and /sys trees: every field the Rust side reads is there."""

    def setUp(self):
        super().setUp()
        self.proc = tempfile.mkdtemp()
        os.environ["SPARKBOX_PROC_ROOT"] = self.proc
        self.c = reloaded()
        p = lambda rel, text: write(os.path.join(self.proc, rel), text)
        p("stat", STAT_A)
        p("meminfo", MEMINFO)
        p("net/dev", NET_DEV.replace("enp1s0f1np1", "enp1s0f1np1") + "enP7s7: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n")
        p("diskstats", DISKSTATS)
        p("mounts", "")
        p("loadavg", "0.50 0.40 0.30 1/200 999\n")
        p("uptime", "93784.12 100.0\n")
        p("net/tcp", "header\n")
        p("net/tcp6", "header\n")

    def tearDown(self):
        del os.environ["SPARKBOX_PROC_ROOT"]
        shutil.rmtree(self.proc)
        super().tearDown()

    def test_recorded_qwen_container_cpu_and_gpu_memory_remain_distinct(self):
        fixture = json.loads(load(HERE, "..", "models", "fixtures", "qwen3-06b-cell1-2026-11-07", "node-samples.json"))
        cid = "c" * 64
        cg = os.path.join(self.root, "fs/cgroup/system.slice/docker-%s.scope" % cid)
        col = self.c.Collector(stream_gpu=False, kernel=False)
        col.poll_gpu = col.poll_docker = lambda now: None
        col.last_proc = col.last_units = col.last_slow = float("inf")
        col.listing = [{"Id": cid, "Names": ["/sparkbox-fixture"], "State": "running", "Image": "fixture:tested"}]
        frames = []
        for sample in fixture["samples"]:
            write(os.path.join(self.proc, "meminfo"), sample["meminfo"])
            write(os.path.join(cg, "cpu.stat"), sample["cpuStat"])
            write(os.path.join(cg, "memory.current"), str(sample["memoryCurrent"]))
            col.apps = [dict(a, cid=cid) for a in self.c.parse_apps(sample["appsCsv"])]
            gpu = sample["gpuCsv"].split(",")
            col.gpu = self.c.parse_gpu(",".join(gpu[:5]))
            col.gpu_extra = self.c.parse_gpu_extra(",".join(gpu[5:]))
            with patch.object(self.c.time, "monotonic", return_value=100 + sample["elapsedMs"] / 1000):
                frames.append(col.frame())
        idle, busy, after = frames
        self.assertEqual([f["containers"][0]["mem"] for f in frames], [4951273472, 4951007232, 4951539712])
        self.assertEqual([f["containers"][0]["cpuPct"] for f in frames], [0, 52.4, 27.3])
        for frame in frames:
            self.assertEqual(frame["mem"]["gpuAlloc"], 24869076992)  # 23,717 MiB, not bytes or GiB
            self.assertEqual(frame["containers"][0]["gpuMem"], 24869076992)
            self.assertLess(frame["containers"][0]["mem"], frame["containers"][0]["gpuMem"])
            self.assertEqual(frame["mem"]["used"] + frame["mem"]["available"], frame["mem"]["total"])
            self.assertEqual(frame["mem"]["swapUsed"], 0)
            self.assertIsNone(frame["gpu"]["powerLimitW"], "GB10 did not report a watt limit")
            self.assertEqual(frame["gpu"]["throttle"], [])
        self.assertEqual((busy["gpu"]["utilPct"], busy["gpu"]["smClockMhz"]), (96.0, 2444))
        self.assertEqual(busy["mem"]["available"], 95465033728)
        self.assertEqual(idle["mem"]["gpuAlloc"], after["mem"]["gpuAlloc"], "idle requests do not release the reserved pool")

    def test_a_frame_has_the_whole_schema(self):
        col = self.c.Collector()
        self.c.time.sleep(0.01)
        self.counters("rocep1s0f1", rcv=1000 + 250_000, xmit=2000 + 125_000)
        frame = json.loads(json.dumps(col.frame()))
        self.assertEqual(set(frame) >= {"cpu", "mem", "gpu", "disks", "nets", "system", "containers", "processes",
                                        "listeners", "failedUnits", "errors"}, True)
        nets = {n["iface"]: n for n in frame["nets"]}
        f = nets["enp1s0f1np1"]
        for key in ("mtu", "mac", "rxErrors", "txErrors", "rxDrops", "txDrops", "carrierChanges", "rdmaDev", "rdmaRxBps", "rdmaTxBps"):
            self.assertIn(key, f)
        self.assertEqual((f["rxErrors"], f["txDrops"], f["mtu"], f["rdmaDev"]), (3, 9, 9000, "rocep1s0f1"))
        self.assertGreater(f["rdmaRxBps"], 0)
        self.assertGreater(f["rdmaRxBps"], f["rdmaTxBps"])
        self.assertNotIn("rdmaDev", nets["enP7s7"], "the LAN port has no RDMA counters")
        for key in ("clockMaxMhz", "powerLimitW", "pstate", "throttle"):
            self.assertIn(key, frame["gpu"])
        self.assertEqual(frame["system"]["collectorVersion"], self.c.COLLECTOR_BUILD)
        self.assertIn("containerToolkit", frame["system"])
        self.assertIn("imageStore", frame["system"])
        self.assertEqual(frame["system"]["uptimeS"], 93784)
        self.assertEqual(frame["system"]["cx7Fw"], "28.43.1014")
        self.assertIsInstance(frame["failedUnits"], list)


class ProcTree(Case):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        os.environ["SPARKBOX_PROC_ROOT"] = self.root
        self.c = importlib.reload(c)
        write(os.path.join(self.root, "net/dev"), NET_DEV)
        write(os.path.join(self.root, "4242/stat"),
              "4242 (vllm serve) R 1 4242 4242 0 -1 4194560 100 0 0 0 750 250 0 0 20 0 12 0 98765 9000000 2048 0")
        write(os.path.join(self.root, "4242/cgroup"), "0::/system.slice/docker-%s.scope\n" % ("b" * 64))
        write(os.path.join(self.root, "4242/cmdline"), "vllm\0serve\0/models/llama\0")
        os.makedirs(os.path.join(self.root, "4242/fd"))
        write(os.path.join(self.root, "net/tcp"), "header\n")

    def tearDown(self):
        del os.environ["SPARKBOX_PROC_ROOT"]
        importlib.reload(c)
        shutil.rmtree(self.root)

    def test_processes_read_from_the_proc_root(self):
        procs, listeners, ticks, total = self.c.processes({}, 1.0, set())
        self.assertEqual(total, 1)
        self.assertEqual(len(procs), 1)
        p = procs[0]
        self.assertEqual((p["pid"], p["name"], p["state"], p["ppid"], p["threads"], p["startTime"]), (4242, "vllm serve", "R", 1, 12, 98765))
        self.assertEqual(p["cpuTimeS"], round(1000 / self.c.CLK_TCK, 2))
        self.assertEqual(p["cmd"], "vllm serve /models/llama")
        self.assertEqual(ticks, {4242: 1000})
        self.assertEqual(listeners, [])


HERE = os.path.dirname(os.path.abspath(__file__))
JOURNAL = os.path.join(HERE, "..", "agent", "fixtures", "journal-kernel.jsonl")


class KernelLog(Case):
    """Kernel events from fixture `journalctl -k -o json` lines shaped like DGX OS's."""

    def events(self, containers=None):
        out = []
        with open(JOURNAL) as f:
            for line in f:
                parsed = c.parse_journal_line(line)
                event = parsed and c.classify_kernel(parsed[1], parsed[0], containers)
                if event:
                    out.append(event)
        return out

    def test_journal_lines_parse_including_byte_array_messages(self):
        with open(JOURNAL) as f:
            lines = f.read().splitlines()
        ts, msg = c.parse_journal_line(lines[0])
        self.assertEqual(ts, 1761660000000)
        self.assertTrue(msg.startswith("NVRM: Xid (PCI:000f:01:00): 79"))
        ts, msg = c.parse_journal_line(lines[-1])
        self.assertIn("Xid (PCI:000f:01:00): 31", msg, "MESSAGE sent as an array of bytes")
        self.assertIsNone(c.parse_journal_line("not json"))

    def test_xid_codes_and_severity(self):
        xids = [e for e in self.events() if e["kind"] == "xid"]
        self.assertEqual([(e["code"], e["severity"]) for e in xids], [(79, "crit"), (13, "warn"), (31, "warn")])

    def test_mlx5_errors_but_not_routine_syndromes(self):
        mlx = [e for e in self.events() if e["kind"] == "mlx5"]
        self.assertEqual(len(mlx), 2, [e["text"] for e in mlx])
        self.assertIn("Link down", mlx[0]["text"])
        self.assertIn("No done completion", mlx[1]["text"])

    def test_oom_kill_in_a_vllm_container_is_critical_and_counted_once(self):
        cid = "4f2a9c1b7e3d5a6f8b0c2d4e6f8a0b1c3d5e7f9a1b3c5d7e9f0a2b4c6d8e0f1a"
        store = c.KernelEvents()
        for e in self.events({cid: ("vllm-head", "vllm/vllm-openai:v0.11.0")}):
            store.add(e)
        ooms = [e for e in store.list() if e["kind"] == "oom"]
        self.assertEqual(len(ooms), 2, "the oom-kill: line and its 'Killed process' line are one event")
        self.assertEqual((ooms[0]["process"], ooms[0]["severity"]), ("python3 (vllm-head)", "crit"))
        self.assertEqual((ooms[1]["process"], ooms[1]["severity"]), ("ollama", "crit"))
        self.assertNotIn("pid", ooms[0], "the pid stays inside the collector")
        plain = c.classify_kernel("Out of memory: Killed process 77 (chrome) total-vm:1kB", 5)
        self.assertEqual(plain["severity"], "warn")

    def test_oom_kill_in_a_load_started_by_its_image_id_is_critical(self):
        """C-L2 names a container sparkbox-<id> and runs its image by id, so only the
        sparkbox.image label says what's in it."""
        cid = "a" * 64
        listing = [{"Id": cid, "Names": ["/sparkbox-m2tp2-r0"], "Image": "sha256:" + "a" * 64,
                    "Labels": {"sparkbox.image": "vllm/vllm-openai:glm53-flash-arm64-cu130"}},
                   {"Id": "b" * 64, "Names": ["/web"], "Image": "nginx:1", "Labels": None}]
        index = c.Collector.container_index(types.SimpleNamespace(listing=listing))
        line = ("oom-kill:constraint=CONSTRAINT_NONE,nodemask=(null),cpuset=docker-%s.scope,mems_allowed=0,"
                "global_oom,task_memcg=/system.slice/docker-%s.scope,task=%s,pid=4242,uid=0")
        for task in ("python3", "pt_main_thread", "vllm"):
            self.assertEqual(c.classify_kernel(line % (cid, cid, task), 1000, index)["severity"], "crit", task)
        self.assertEqual(c.classify_kernel(line % ("b" * 64, "b" * 64, "nginx"), 1000, index)["severity"], "warn")

    def test_hung_task(self):
        hung = [e for e in self.events() if e["kind"] == "hung"]
        self.assertEqual((hung[0]["process"], hung[0]["severity"]), ("jbd2/nvme0n1p2-8", "warn"))

    def test_other_kernel_lines_are_ignored(self):
        self.assertIsNone(c.classify_kernel("e1000e: enP7s7 NIC Link is Up 10 Gbps Full Duplex", 1))

    def test_dmesg_lines(self):
        ts, msg = c.parse_dmesg_line("2026-10-28T14:10:00,123456+01:00 NVRM: Xid (PCI:000f:01:00): 48, DBE", 0)
        self.assertEqual(ts, (1793196600 - 3600) * 1000 + 123)
        self.assertEqual(c.classify_kernel(msg, ts)["code"], 48)
        self.assertEqual(c.parse_dmesg_line("[93784.120] INFO: task kworker/3:1:88 blocked for more than 120 seconds.", 7),
                         (7, "INFO: task kworker/3:1:88 blocked for more than 120 seconds."))

    def test_the_keep_limit(self):
        store = c.KernelEvents(keep=3)
        for i in range(5):
            store.add({"tsMs": i, "kind": "hung", "process": "p%d" % i, "severity": "warn"})
        self.assertEqual([e["process"] for e in store.list()], ["p2", "p3", "p4"])

    def test_the_xid_table_matches_the_rust_one(self):
        with open(os.path.join(HERE, "..", "health", "xid.json")) as f:
            table = json.load(f)
        self.assertEqual({x["code"]: (x["severity"], x["name"]) for x in table}, c.XID)
        self.assertEqual({x["code"] for x in table if x.get("latch")}, set(c.FATAL_XID), "the Xids that hold until a restart")

    def test_a_fatal_xid_stays_however_much_the_log_says_after_it(self):
        store = c.KernelEvents(keep=3)
        store.add({"tsMs": 1, "kind": "xid", "code": 79, "text": "NVRM: Xid (PCI:000f:01:00): 79", "severity": "crit", "process": None})
        for i in range(5):
            store.add({"tsMs": 10 + i, "kind": "hung", "process": "p%d" % i, "severity": "warn"})
        self.assertEqual([(e["kind"], e.get("code") or e.get("process")) for e in store.list()],
                         [("xid", 79), ("hung", "p2"), ("hung", "p3"), ("hung", "p4")])
        store.add({"tsMs": 99, "kind": "xid", "code": 79, "text": "again", "severity": "crit", "process": None})
        self.assertEqual([e["tsMs"] for e in store.list() if e["kind"] == "xid"], [1, 99], "the first one this boot, and the latest")

    def test_the_whole_boot_is_read_and_a_restarted_follower_adds_nothing_twice(self):
        bindir = tempfile.mkdtemp()
        log = os.path.join(bindir, "calls")
        with open(os.path.join(bindir, "journalctl"), "w") as f:
            # Every run prints the whole fixture (as `-b` from the start would) and ends soon.
            f.write("#!/bin/sh\necho \"$*\" >> %s\ncat %s\ncase \"$*\" in *-f*) sleep 0.3 ;; esac\n" % (log, JOURNAL))
        os.chmod(os.path.join(bindir, "journalctl"), 0o755)
        old, wait = os.environ["PATH"], c.KernelWatcher.RESTART_WAIT
        os.environ["PATH"] = bindir + os.pathsep + old
        c.KernelWatcher.RESTART_WAIT = 0.05
        try:
            store = c.KernelEvents()
            w = c.KernelWatcher(store)
            for _ in range(200):
                if os.path.exists(log):
                    with open(log) as f:
                        if sum(1 for l in f if " -f " in l) >= 2:
                            break
                c.time.sleep(0.02)
            c.time.sleep(0.3)
            with open(log) as f:
                follows = [l.split() for l in f if " -f " in l]
            self.assertIn("-b", follows[0])
            self.assertIn("--no-tail", follows[0], "from the start of the boot, not the last 10 lines")
            self.assertIn("--after-cursor=s=1;i=100b", follows[1], "a restart picks up after the last entry read")
            xids = [e for e in store.list() if e["kind"] == "xid"]
            self.assertEqual([(e["code"], e["tsMs"]) for e in xids], [(79, 1761660000000), (13, 1761660001000), (31, 1761660011000)],
                             "each once, with its own time")
            self.assertEqual(w.source, "journal")
        finally:
            threads_left()  # the fake ends, so its watcher gives up after three quick tries
            os.environ["PATH"] = old
            c.KernelWatcher.RESTART_WAIT = wait
            shutil.rmtree(bindir)

    def test_the_watcher_reads_a_followed_journal(self):
        bindir = tempfile.mkdtemp()
        script = os.path.join(bindir, "journalctl")
        with open(script, "w") as f:
            # journalctl -f keeps running; the fixture prints its lines and waits a moment the same way.
            f.write("#!/bin/sh\ncat %s\ncase \"$*\" in *-f*) sleep 0.3 ;; esac\n" % JOURNAL)
        os.chmod(script, 0o755)
        old, wait = os.environ["PATH"], c.KernelWatcher.RESTART_WAIT
        os.environ["PATH"] = bindir + os.pathsep + old
        c.KernelWatcher.RESTART_WAIT = 0.05
        try:
            store = c.KernelEvents()
            w = c.KernelWatcher(store)
            for _ in range(100):
                if len(store.list()) >= 8:
                    break
                c.time.sleep(0.02)
            kinds = [e["kind"] for e in store.list()]
            self.assertEqual(kinds.count("xid"), 3)
            self.assertEqual(w.source, "journal")
        finally:
            threads_left()  # the fake ends, so its watcher gives up after three quick tries
            os.environ["PATH"] = old
            c.KernelWatcher.RESTART_WAIT = wait
            shutil.rmtree(bindir)

    def test_an_unreadable_journal_falls_back_to_dmesg(self):
        bindir = tempfile.mkdtemp()
        # journalctl as a user outside systemd-journal: no entries, keeps following.
        with open(os.path.join(bindir, "journalctl"), "w") as f:
            f.write("#!/bin/sh\necho 'Hint: You are currently not seeing messages from other users and the system.' >&2\n"
                    "case \"$*\" in *-f*) sleep 0.3 ;; esac\n")
        with open(os.path.join(bindir, "dmesg"), "w") as f:
            f.write("#!/bin/sh\necho '2026-10-28T14:10:00,123456+00:00 NVRM: Xid (PCI:000f:01:00): 79, GPU has fallen off the bus.'\nsleep 0.3\n")
        for name in ("journalctl", "dmesg"):
            os.chmod(os.path.join(bindir, name), 0o755)
        old, wait = os.environ["PATH"], c.KernelWatcher.RESTART_WAIT
        os.environ["PATH"] = bindir + os.pathsep + old
        c.KernelWatcher.RESTART_WAIT = 0.05
        try:
            store = c.KernelEvents()
            w = c.KernelWatcher(store)
            for _ in range(150):
                if store.list():
                    break
                c.time.sleep(0.02)
            self.assertEqual([(e["kind"], e["code"]) for e in store.list()], [("xid", 79)])
            self.assertEqual(w.source, "dmesg")
        finally:
            threads_left()  # the fake ends, so its watcher gives up after three quick tries
            os.environ["PATH"] = old
            c.KernelWatcher.RESTART_WAIT = wait
            shutil.rmtree(bindir)


class GpuReadings(Case):
    def test_one_streamed_line_holds_every_field(self):
        gpu, extra = c.parse_gpu_line("96, 71, 63.20, 2418, 580.95.05, 3003, [N/A], P0, 0x0000000000000004")
        self.assertEqual((gpu["utilPct"], gpu["powerW"], gpu["driver"]), (96.0, 63.2, "580.95.05"))
        self.assertEqual(extra, {"clockMaxMhz": 3003, "powerLimitW": None, "pstate": "P0", "throttle": ["SW power cap"]})

    def test_the_stream_reader_keeps_the_latest_line(self):
        bindir = tempfile.mkdtemp()
        script = os.path.join(bindir, "nvidia-smi")
        with open(script, "w") as f:
            f.write('#!/bin/sh\necho "$@" > %s/args\necho "10, 40, 20.0, 600, 580.95.05, 3003, [N/A], P8, 0x1"\n'
                    'echo "97, 72, 88.5, 2400, 580.95.05, 3003, [N/A], P0, 0x0"\n' % bindir)
        os.chmod(script, 0o755)
        old = os.environ["PATH"]
        os.environ["PATH"] = bindir + os.pathsep + old
        try:
            s = c.GpuStream(every_ms=2000)
            for _ in range(100):
                if not s.alive:
                    break
                c.time.sleep(0.02)
            with open(os.path.join(bindir, "args")) as f:
                args = f.read()
            self.assertIn("-lms 2000", args)
            self.assertIn("--format=csv,noheader,nounits", args)
            self.assertFalse(s.alive, "the fixture exits, so the collector falls back to polling")
        finally:
            os.environ["PATH"] = old
            shutil.rmtree(bindir)

    def test_a_live_stream_feeds_the_sampler(self):
        sampler = c.GpuSampler.__new__(c.GpuSampler)
        sampler.every, sampler.poll_timeout, sampler.nvml, sampler.error, sampler.latest = 2.0, 8, None, None, None
        sampler.backoff, sampler.not_before = c.NVIDIA_SMI_BACKOFF, 0.0
        sampler.ready = c.threading.Event()
        stream = type("S", (), {})()
        stream.latest = (c.time.monotonic(),) + c.parse_gpu_line("97, 72, 88.5, 2400, 580.95.05, 3003, [N/A], P0, 0x0")
        sampler.stream = stream
        calls = []
        old = c.run_ex
        c.run_ex = lambda cmd, timeout=8: calls.append(cmd) or ("ok", "", "")
        try:
            sampler.read_once()
        finally:
            c.run_ex = old
        latest, why = sampler.reading(c.time.monotonic())
        self.assertEqual((latest[1]["utilPct"], why), (97.0, None))
        self.assertEqual([x[1].split("=")[0] for x in calls], ["--query-compute-apps"], "only the per-process query still runs")

    def test_pynvml_when_it_imports(self):
        class Util:
            gpu = 55

        class FakeNvml:
            NVML_CLOCK_SM = 1
            NVML_TEMPERATURE_GPU = 0

            def nvmlInit(self):
                pass

            def nvmlDeviceGetHandleByIndex(self, i):
                return "h"

            def nvmlSystemGetDriverVersion(self):
                return b"580.95.05"

            def nvmlDeviceGetUtilizationRates(self, h):
                return Util()

            def nvmlDeviceGetPowerUsage(self, h):
                return 61200

            def nvmlDeviceGetEnforcedPowerLimit(self, h):
                raise RuntimeError("Not Supported")

            def nvmlDeviceGetClockInfo(self, h, kind):
                return 2400

            def nvmlDeviceGetMaxClockInfo(self, h, kind):
                return 3003

            def nvmlDeviceGetPerformanceState(self, h):
                return 0

            def nvmlDeviceGetCurrentClocksEventReasons(self, h):
                return 0x40

            def nvmlDeviceGetTemperature(self, h, kind):
                return 68

        gpu, extra = c.NvmlGpu(FakeNvml()).read()
        self.assertEqual((gpu["utilPct"], gpu["tempC"], gpu["powerW"], gpu["smClockMhz"], gpu["driver"]), (55.0, 68.0, 61.2, 2400, "580.95.05"))
        self.assertEqual(extra, {"clockMaxMhz": 3003, "powerLimitW": None, "pstate": "P0", "throttle": ["HW thermal slowdown"]})


class Extras(Case):
    def test_wake_on_lan_from_ethtool(self):
        text = "Settings for enP7s7:\n\tSupports Wake-on: pumbg\n\tWake-on: g\n\tLink detected: yes\n"
        self.assertEqual(c.parse_wol(text), "g")
        self.assertIsNone(c.parse_wol("Settings for x:\n\tLink detected: yes\n"))
        self.assertIsNone(c.parse_wol(None))

    def test_roce_counters_come_from_roce_py_when_present(self):
        old = c.roce
        try:
            c.roce = None
            self.assertIsNone(c.roce_counters("enp1s0f1np1", "rocep1s0f1"))
            fake = type("R", (), {})()
            fake.counters = lambda iface, dev: {"ecnMarked": 12, "cnpSent": None}
            c.roce = fake
            self.assertEqual(c.roce_counters("enp1s0f1np1", "rocep1s0f1"), {"ecnMarked": 12, "cnpSent": None})
            self.assertIsNone(c.roce_counters("enP7s7", None), "only interfaces with an RDMA device")
        finally:
            c.roce = old


class Bounded(Case):
    """Commands never hold the caller: past the deadline the process group is killed, and a
    child that still won't let go (here a grandchild in a session of its own keeps the pipe,
    as a process stuck in the kernel would) is left to the reaper."""

    def stubborn(self, bindir, name):
        pidfile = os.path.join(bindir, "%s.pid" % name)
        script = os.path.join(bindir, name)
        with open(script, "w") as f:
            f.write("#!%s\nimport os, signal, sys, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                    "if os.fork() == 0:\n    os.setsid()\n    open(%r, 'a').write('%%d\\n' %% os.getpid())\n    time.sleep(60)\n"
                    "time.sleep(60)\n" % (sys.executable, pidfile))
        os.chmod(script, 0o755)

        def kill():
            for pid in load(pidfile).split() if os.path.exists(pidfile) else []:
                try:
                    os.kill(int(pid), 9)
                except OSError:
                    pass
        self.addCleanup(kill)
        return script

    def sampler(self, **kwargs):
        """A real GpuSampler. Its loop has no way out, so the test owns one: past stop(), the next
        turn raises what the loop doesn't catch, and its thread ends."""
        class Sampler(c.GpuSampler):
            stopped = False

            def read_once(self):
                if self.stopped:
                    raise SystemExit
                super().read_once()

        sampler = Sampler(**kwargs)

        def stop():
            sampler.stopped = True
            threads_left()
        self.addCleanup(stop)
        return sampler

    def test_a_child_that_ignores_sigterm_and_holds_the_pipe_returns_within_its_deadline(self):
        bindir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, bindir)
        script = self.stubborn(bindir, "hang")
        started = c.time.monotonic()
        self.assertEqual(c.run_ex([script], timeout=0.5)[0], "timeout")
        self.assertLess(c.time.monotonic() - started, 0.5 + c.KILL_WAIT + 1)
        self.assertIsNone(c.run([script], timeout=0.5))
        self.assertEqual(c.run_ex(["/nonexistent/tool"])[0], "missing")
        self.assertEqual(c.run_ex(["sh", "-c", "echo no >&2; exit 3"])[:1], ("failed",))

    def test_a_hung_nvidia_smi_never_holds_a_frame_and_its_numbers_are_null(self):
        bindir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, bindir)
        self.stubborn(bindir, "nvidia-smi")
        old, deadline = os.environ["PATH"], c.GPU_DEADLINE
        os.environ["PATH"] = bindir + os.pathsep + old
        c.GPU_DEADLINE = 1.0
        try:
            sampler = self.sampler(every=0.2, stream=False, poll_timeout=0.5)
            started = c.time.monotonic()
            self.assertEqual(sampler.reading(started)[0], None)
            sampler_returned = None
            for _ in range(100):
                if sampler.error:
                    sampler_returned = c.time.monotonic() - started
                    break
                c.time.sleep(0.05)
            self.assertIsNotNone(sampler_returned)
            self.assertLess(sampler_returned, 0.5 + c.KILL_WAIT + 1, "the sampler thread gave up within its deadline")
            latest, why = sampler.reading(c.time.monotonic())
            self.assertEqual((latest, why), (None, "nvidia-smi didn't answer in 1 s"))
        finally:
            os.environ["PATH"] = old
            c.GPU_DEADLINE = deadline

    def test_a_failing_nvidia_smi_says_why(self):
        bindir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, bindir)
        with open(os.path.join(bindir, "nvidia-smi"), "w") as f:
            f.write("#!/bin/sh\necho 'Unable to determine the device handle for GPU0000:0F:01.0: Unknown Error'\nexit 255\n")
        os.chmod(os.path.join(bindir, "nvidia-smi"), 0o755)
        old = os.environ["PATH"]
        os.environ["PATH"] = bindir + os.pathsep + old
        try:
            # This checks a real process's failure text, not how fast macOS starts its shell.
            # The hung-process tests above separately enforce the short kill/deadline bounds.
            sampler = self.sampler(every=0.2, stream=False, poll_timeout=30)
            self.assertTrue(sampler.ready.wait(35), "the failing GPU command never completed")
            self.assertEqual(sampler.reading(c.time.monotonic()),
                             (None, "nvidia-smi failed: Unable to determine the device handle for GPU0000:0F:01.0: Unknown Error"))
        finally:
            os.environ["PATH"] = old


class UnreadableGpu(WholeFrame):
    def test_the_frame_has_null_gpu_numbers_the_reason_and_the_boot_id(self):
        write(os.path.join(self.proc, "sys/kernel/random/boot_id"), "8c0f1e3a-4b5c-6d7e-8f9a-0b1c2d3e4f50\n")
        self.c = reloaded()
        col = self.c.Collector(stream_gpu=False, kernel=False)
        col.gpu_sampler.latest, col.gpu_sampler.error = None, "nvidia-smi didn't answer in 10 s"
        started = self.c.time.monotonic()
        frame = json.loads(json.dumps(self.c.sample(col)))
        self.assertLess(self.c.time.monotonic() - started, 1.0)
        self.assertEqual(frame["bootId"], "8c0f1e3a-4b5c-6d7e-8f9a-0b1c2d3e4f50")
        g = frame["gpu"]
        self.assertEqual((g["utilPct"], g["tempC"], g["powerW"], g["smClockMhz"]), (None, None, None, None), "unknown, never 0")
        self.assertEqual(g["error"], "nvidia-smi didn't answer in 10 s")
        self.assertIn("nvidia-smi didn't answer in 10 s", frame["errors"])
        col.gpu_sampler.latest = (self.c.time.monotonic(),) + self.c.parse_gpu_line("97, 72, 88.5, 2400, 580.95.05, 3003, [N/A], P0, 0x0") + ([],)
        col.gpu_sampler.error = None
        frame = self.c.sample(col)
        self.assertEqual(frame["gpu"]["utilPct"], 97.0)
        self.assertNotIn("error", frame["gpu"])


class SampleFrame(WholeFrame):
    """sample() is what the agent imports: the same frame the live stream sends."""

    def test_sample_returns_a_frame_with_kernel_events_and_wake_on_lan(self):
        col = self.c.Collector(stream_gpu=False, kernel=False)
        col.kernel.add({"tsMs": 5, "kind": "xid", "code": 79, "text": "NVRM: Xid (PCI:000f:01:00): 79", "severity": "crit", "process": None})
        col.wol = {"enP7s7": "g"}
        col.last_wol = self.c.time.monotonic() + 1000
        frame = self.c.sample(col)
        self.assertEqual(frame["kernelEvents"][0]["code"], 79)
        nets = {n["iface"]: n for n in frame["nets"]}
        self.assertEqual(nets["enP7s7"]["lanWol"], "g")
        self.assertNotIn("lanWol", nets["enp1s0f1np1"])
        self.assertEqual(frame["v"], 6)

    def test_a_bad_reading_is_skipped_not_fatal(self):
        col = self.c.Collector(stream_gpu=False, kernel=False)
        col.frame = lambda: 1 / 0
        self.assertIsNone(self.c.sample(col))
        self.assertIn("ZeroDivisionError", col.errors["loop"])


DOCTOR = os.path.join(HERE, "fixtures", "doctor")
sys.path.insert(0, DOCTOR)
import spark_tree  # noqa: E402


def load(*parts):
    with open(os.path.join(*parts)) as f:
        return f.read()


class SparkTree(Case):
    """A node rebuilt from live/fixtures/doctor (the Live tour's captures): its /proc, /sys and
    /etc, and its own commands first on PATH, answering as they did for the cluster user."""
    NODE = "cell1"

    def setUp(self):
        self.dest = tempfile.mkdtemp()
        self.env = spark_tree.build(self.NODE, self.dest)
        self.old = dict(os.environ)
        os.environ.update(self.env)
        os.environ["PATH"] = os.path.join(self.dest, "bin") + os.pathsep + self.old["PATH"]
        os.environ["SB_FAKE_STATE"] = os.path.join(self.dest, "state")
        os.makedirs(os.environ["SB_FAKE_STATE"])
        self.c = reloaded()

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.old)
        importlib.reload(c)
        spark_tree.unlock(self.dest)
        shutil.rmtree(self.dest)

    def frame(self):
        col = self.c.Collector(stream_gpu=False, kernel=False)
        return col, self.c.sample(col)


class SwapSamples(SparkTree):
    def test_frames_publish_rates_and_relearn_after_a_missing_counter(self):
        path = self.c.proc("vmstat")
        with open(path, "w") as f:
            f.write("pswpin 100\n")
        col, first = self.frame()
        self.assertIsNone(first["mem"]["swapInBps"])
        col.page_size = 4096
        col.prev_swap_at = start = self.c.time.monotonic() - 2
        with open(path, "w") as f:
            f.write("pswpin 612\n")
        second = self.c.sample(col)
        self.assertAlmostEqual(second["mem"]["swapInBps"], 512 * 4096 / (col.prev_swap_at - start))
        os.remove(path)
        self.assertIsNone(self.c.sample(col)["mem"]["swapInBps"])
        with open(path, "w") as f:
            f.write("pswpin 700\n")
        self.assertIsNone(self.c.sample(col)["mem"]["swapInBps"], "the missing interval is not a rate")
        self.assertEqual(self.c.sample(col)["mem"]["swapInBps"], 0)


class ProcessesFromARealSpark(SparkTree):
    def test_removed_container_keeps_cached_process_out_of_host_discovery(self):
        cid = "d2436db60e84" + "a" * 52
        process = {"pid": 249, "cid": cid, "name": "python3", "cmd": "python3 -m vllm.entrypoints.openai.api_server --port 8000"}
        named = c.Collector.process_out(process, {cid: "sparkbox-test"}, [])
        self.assertEqual(named["container"], "sparkbox-test")
        # The Docker refresh sees removal before the next /proc refresh.
        removed = c.Collector.process_out(process, {}, [])
        self.assertEqual(removed["container"], cid[:12])
        self.assertNotIn("cid", removed)
        self.assertEqual(process["cid"], cid, "publishing must not mutate the cached process")
        host = c.Collector.process_out({**process, "cid": None}, {}, [])
        self.assertNotIn("container", host, "real host processes remain discoverable")

    def test_the_list_has_full_names_no_kernel_threads_unmarked_and_sparkboxs_own_marked(self):
        procs, listeners, _, total = self.c.processes({}, 1.0, set())
        fixture = json.loads(load(DOCTOR, "cell1", "processes.json"))
        self.assertEqual(total, len(fixture), "procTotal counts every process, kernel threads too")
        by = {}
        for p in procs:
            by.setdefault(p["name"], []).append(p)
        for name in ("ollama", "nginx", "dockerd", "containerd", "dashboard-service", "dashboard-admin", "systemd-journald", "sshd",
                     "master", "cupsd", "lldpd", "xrdp", "xrdp-sesman", "avahi-daemon", "NetworkManager", "earlyoom", "networkd-dispatcher"):
            self.assertIn(name, by, name)
        ds = by["dashboard-service"][0]
        self.assertEqual((ds["comm"], ds["user"], ds["unit"]), ("dashboard-servi", "dgx-dashboard-service-user", "dgx-dashboard.service"))
        self.assertNotIn("comm", by["dockerd"][0], "comm only when it differs from the name")
        # Ollama runs as ollama, and its fds aren't readable: it's listed, its port isn't claimed.
        ol = by["ollama"][0]
        self.assertEqual((ol["user"], ol["cmd"], ol.get("ports")), ("ollama", "/usr/local/bin/ollama serve", None))
        self.assertEqual(len(by["nginx"]), 21, "nginx's master and its 20 workers")
        kernel = [p for p in procs if p["cmd"].startswith("[") or p["ppid"] == 2 or p["pid"] == 2]
        self.assertTrue(kernel and all(p.get("kernel") for p in kernel), "every kernel thread is marked")
        self.assertLessEqual(len(kernel), self.c.KERNEL_THREADS)
        ours = [p for p in procs if p.get("sparkbox")]
        self.assertEqual(sorted(p["name"] for p in ours), ["journalctl", "nvidia-smi", "python3"], "the collector and what it started")
        self.assertTrue(all(".sparkbox/collector.py" in p["cmd"] or p["ppid"] == by["python3"][0]["pid"] for p in ours))
        self.assertEqual(listeners, [l for l in listeners if l["pid"] is None], "root's sockets have no visible owner")

    def test_the_frame_stays_under_64_kb_at_the_cap(self):
        # 400 more userland processes, each with a long command line: the list is cut at 200.
        proc = self.env["SPARKBOX_PROC_ROOT"]
        for i in range(400):
            pid = 900000 + i
            write(os.path.join(proc, "%d/stat" % pid), "%d (worker) S 1 %d %d 0 -1 4194560 1 0 0 0 %d 0 0 0 20 0 1 0 5000 1000 %d 0" % (pid, pid, pid, i, 100 + i))
            write(os.path.join(proc, "%d/cmdline" % pid), "/usr/bin/python3\0-m\0worker\0--queue\0%s\0" % ("x" * 300))
            write(os.path.join(proc, "%d/status" % pid), "Name:\tworker\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t%d kB\n" % (1000 + i))
        col, frame = self.frame()
        procs = frame["processes"]
        user = [p for p in procs if not p.get("kernel")]
        self.assertEqual(len(user), self.c.MAX_PROCESSES)
        self.assertIn("ollama", [p["name"] for p in user], "the daemons are kept however busy the rest is")
        self.assertIn("dashboard-service", [p["name"] for p in user])
        self.assertEqual(frame["procTotal"], 400 + len(json.loads(load(DOCTOR, "cell1", "processes.json"))))
        self.assertLess(len(json.dumps(frame, separators=(",", ":"))), 64 * 1024)
        self.assertTrue(all(len(p["cmd"]) <= self.c.CMD_CHARS for p in procs))

    def test_the_rest_of_the_frame_reads_as_cell1(self):
        _, f = self.frame()
        s = f["system"]
        self.assertEqual((s["hostname"], s["model"], s["kernel"], s["bios"], s["cx7Fw"], s["cuda"]),
                         ("cell1-example", "MS-C931", "7.0.0-1019-nvidia", "5.36_1.8.0", "28.50.1002", "13.0"))
        self.assertEqual(s["clockOffsetMs"], -0.312, "timedatectl timesync-status, no root needed")
        self.assertEqual(f["mem"]["total"], 130661117952)
        self.assertEqual((f["mem"]["psiSome10"], f["mem"]["psiIoFull10"]), (0.0, 0.0))
        nets = {n["iface"]: n for n in f["nets"]}
        self.assertEqual(sorted(nets), ["enP2p1s0f0np0", "enP2p1s0f1np1", "enP7s7", "enp1s0f0np0", "enp1s0f1np1", "wlP9s9"])
        self.assertEqual((nets["enp1s0f1np1"]["ip"], nets["enp1s0f1np1"]["mtu"], nets["enp1s0f1np1"]["speedMbps"]), ("192.168.100.1", 9000, 200000))
        self.assertEqual((nets["enP2p1s0f1np1"]["ip"], nets["enp1s0f0np0"]["up"]), ("192.168.101.1", False))
        self.assertEqual(nets["enP7s7"]["lanWol"]["state"], "unknown")
        self.assertNotIn("wifi", nets["wlP9s9"], "down Wi-Fi isn't asked about")
        self.assertNotIn("lldp", nets["enp1s0f1np1"], "lldpd doesn't let the cluster user in")
        self.assertEqual([(l["port"], l["scope"], l["family"]) for l in f["listeners"]],
                         [(22, "all", "both"), (25, "all", "both"), (53, "local", "v4"), (631, "local", "both"), (3350, "local", "v6"),
                          (3389, "all", "v6"), (8080, "all", "v4"), (11000, "local", "v4"), (11434, "local", "v4")])


class DgxOsOnCell1(SparkTree):
    """/etc/dgx-release as the Live tour read it (doctor.txt: the 7.2.3 image, then OTA 7.5.0 and 7.6.0)."""

    def test_the_frame_has_the_build_and_the_last_ota_with_its_date(self):
        _, f = self.frame()
        s = f["system"]
        self.assertEqual((s["dgxBuild"], s["dgxOta"], s["dgxOtaDate"]), ("7.2.3", "7.6.0", "Sun Oct  4 11:41:51 EDT 2026"))
        self.assertEqual(s["os"], "NVIDIA DGX Spark (Ubuntu 24.04.5 LTS)")

    def test_no_file_and_no_ota_yet(self):
        path = os.path.join(self.env["SPARKBOX_ROOT"], "etc", "dgx-release")
        with open(path, "w") as f:
            f.write('DGX_NAME="DGX Spark"\nDGX_SWBUILD_VERSION="7.2.3"\n')
        self.assertEqual(self.c.dgx_release(), {"dgxBuild": "7.2.3", "dgxOta": None, "dgxOtaDate": None})
        os.remove(path)
        self.assertEqual(self.c.dgx_release(), {"dgxBuild": None, "dgxOta": None, "dgxOtaDate": None})


class DgxOsOnGx10(SparkTree):
    NODE = "gx10-0000"

    def test_its_ota_came_on_october_15(self):
        self.assertEqual(self.c.dgx_release(), {"dgxBuild": "7.2.3", "dgxOta": "7.6.0", "dgxOtaDate": "Thu Oct 15 10:29:19 EDT 2026"})


class LldpOnCell1(SparkTree):
    def test_the_fabric_halves_hear_only_cell1_and_the_lan_hears_cell3(self):
        with open(os.path.join(self.dest, "bin", "lldpcli"), "w") as f:
            f.write("#!/bin/sh\ncat '%s'\n" % os.path.join(HERE, "fixtures", "lldp", "cell1-neighbors.json"))
        _, f = self.frame()
        nets = {n["iface"]: n for n in f["nets"]}
        for half in ("enp1s0f1np1", "enP2p1s0f1np1"):
            n = nets[half]
            self.assertEqual((n["lldpPeers"], n.get("lldpSelfOnly"), n.get("lldp")), ([], True, None), half)
        lan = nets["enP7s7"]
        self.assertEqual([p["system"] for p in lan["lldpPeers"]], ["cell3-example"])
        self.assertEqual(lan["lldp"], {"system": "cell3-example", "port": "enP7s7"})
        self.assertNotIn("lldpSelfOnly", lan)
        self.assertNotIn("lldpPeers", nets["enp1s0f0np0"], "nothing heard there")
        self.assertNotIn("fc:9d:05:c3:bb:46", json.dumps(nets["enP2p1s0f1np1"]), "cell1's own NIC is never its far end")


class WifiOnCell3(SparkTree):
    NODE = "cell3"

    def test_wifi_from_iw_and_unknown_without_it(self):
        _, f = self.frame()
        w = {n["iface"]: n for n in f["nets"]}["wlP9s9"]
        self.assertEqual((w["up"], w["ip"], w["carrierChanges"], w["speedMbps"]), (True, "192.168.50.229", 154, 867))
        self.assertEqual(w["wifi"], {"ssid": "ExampleLAN", "signalDbm": -52, "bitrateMbps": 866.7})
        os.remove(os.path.join(self.dest, "bin", "iw"))
        _, f = self.frame()
        w = {n["iface"]: n for n in f["nets"]}["wlP9s9"]
        self.assertEqual((w["speedMbps"], w["wifi"]), (None, None), "unknown, never 0")


class WakeOnLanWithoutRoot(SparkTree):
    def wol(self):
        col = self.c.Collector(stream_gpu=False, kernel=False)
        col.poll_wol(self.c.time.monotonic(), ["enP7s7"])
        return col.wol["enP7s7"]

    def test_the_four_answers(self):
        state = os.environ["SB_FAKE_STATE"]
        # ethtool refuses a normal user; NetworkManager's own setting is read instead.
        with open(os.path.join(state, "nm-wol"), "w") as f:
            f.write("magic")
        self.assertEqual(self.wol(), {"state": "on", "source": "networkmanager", "detail": "On (NetworkManager: magic packet)"})
        for value in ("default", "ignore", ""):
            with open(os.path.join(state, "nm-wol"), "w") as f:
                f.write(value)
            self.assertEqual(self.wol(), {"state": "unknown", "source": "networkmanager",
                                          "detail": "Not readable without sudo. NetworkManager leaves it to the driver default."}, value)
        with open(os.path.join(state, "nmcli"), "w") as f:
            f.write("missing")
        self.assertEqual(self.wol(), {"state": "unknown", "source": "none", "detail": "Not readable without sudo (ethtool needs root)"})
        # Under sudo (the fake's root), ethtool shows it.
        os.environ["SB_FAKE_ROOT"] = "1"
        self.assertEqual(self.wol(), {"state": "on", "source": "ethtool", "detail": "On (magic packet)", "raw": "g"})
        self.assertEqual(c.wol_info(None, "none")["state"], "off")


class NodeParsers(Case):
    def test_iw_link(self):
        text = load(DOCTOR, "cell3", "iw-link.txt")
        self.assertEqual(c.parse_iw_link(text), {"ssid": "ExampleLAN", "signalDbm": -52, "bitrateMbps": 866.7})
        self.assertIsNone(c.parse_iw_link("Not connected.\n"))
        self.assertIsNone(c.parse_iw_link(None))

    def test_timesync_offset(self):
        self.assertEqual(c.parse_timesync_offset("       Offset: -312us\n"), -0.312)
        self.assertEqual(c.parse_timesync_offset("       Offset: +1.234ms\n"), 1.234)
        self.assertEqual(c.parse_timesync_offset("       Offset: +2.5s\n"), 2500.0)
        self.assertIsNone(c.parse_timesync_offset("Failed to query server: Unit dbus-org.freedesktop.timesync1.service not found."))

    def test_lldp_in_both_shapes(self):
        one = {"lldp": {"interface": {"enp1s0f1np1": {"chassis": {"crs804": {"id": {"type": "mac", "value": "48:A9:8A:00:00:01"}}},
                                                     "port": {"id": {"type": "ifname", "value": "qsfp56-dd-1-2"}, "descr": "qsfp56-dd-1-2"}}}}}
        many = {"lldp": {"interface": [{"enp1s0f1np1": {"chassis": {"crs804": {}}, "port": {"id": {"value": "qsfp56-dd-1-2"}}}},
                                       {"enP2p1s0f1np1": {"chassis": {"crs804": {}}, "port": {"id": {"value": "qsfp56-dd-1-1"}}}}]}}
        self.assertEqual(c.parse_lldp(json.dumps(one)),
                         {"enp1s0f1np1": [{"system": "crs804", "port": "qsfp56-dd-1-2", "chassisId": "48:a9:8a:00:00:01", "portId": "qsfp56-dd-1-2"}]})
        self.assertEqual(c.parse_lldp(json.dumps(many))["enP2p1s0f1np1"], [{"system": "crs804", "port": "qsfp56-dd-1-1", "chassisId": None, "portId": "qsfp56-dd-1-1"}])
        self.assertEqual(c.parse_lldp("lldpctl: unable to connect"), {})
        # A chassis without a system name is the chassis object itself.
        bare = {"lldp": {"interface": {"enP7s7": {"chassis": {"id": {"type": "mac", "value": "aa:bb:cc:dd:ee:ff"}}, "port": {"id": {"type": "mac", "value": "aa:bb:cc:dd:ee:f0"}}}}}}
        self.assertEqual(c.parse_lldp(json.dumps(bare)), {"enP7s7": [{"system": None, "port": "aa:bb:cc:dd:ee:f0", "chassisId": "aa:bb:cc:dd:ee:ff", "portId": "aa:bb:cc:dd:ee:f0"}]})

    def test_lldp_keeps_every_neighbour_and_drops_this_node_itself(self):
        """cell1 as lldpd shows it (the real shape behind "Switch port fc:9d:05:c3:bb:46"): each
        ConnectX-7 half hears the other half of the same node through the switch, named
        cell1-example with the sibling's MAC as its port id, and the LAN port hears cell3."""
        heard = c.parse_lldp(load(HERE, "fixtures", "lldp", "cell1-neighbors.json"))
        self.assertEqual(sorted(heard), ["enP2p1s0f1np1", "enP7s7", "enp1s0f1np1"])
        self.assertEqual(heard["enp1s0f1np1"], [{"system": "cell1-example", "port": "enP2p1s0f1np1", "chassisId": "fc:9d:05:d5:54:96", "portId": "fc:9d:05:ed:d4:6c"}])
        self.assertEqual(heard["enP2p1s0f1np1"][0]["portId"], "fc:9d:05:c3:bb:46")
        macs = ["fc:9d:05:d5:54:96", "fc:9d:05:71:99:97", "fc:9d:05:c3:bb:46", "fc:9d:05:ea:6a:92", "fc:9d:05:ed:d4:6c", "9c:c7:d3:b6:d1:8d"]
        for half in ("enp1s0f1np1", "enP2p1s0f1np1"):
            self.assertEqual(c.own_neighbors(heard[half], "cell1-example", macs), ([], True), half)
        kept, self_only = c.own_neighbors(heard["enP7s7"], "cell1-example", macs)
        self.assertEqual(([n["system"] for n in kept], self_only), (["cell3-example"], False))
        # Either clue is enough: the name (lldpd may send the FQDN) or one of this node's MACs.
        named = [{"system": "cell1-example.local", "port": "x", "chassisId": "00:00:00:00:00:01", "portId": "00:00:00:00:00:02"}]
        self.assertEqual(c.own_neighbors(named, "cell1-example", macs), ([], True))
        unnamed = [{"system": None, "port": "enp1s0f1np1", "chassisId": None, "portId": "fc:9d:05:c3:bb:46"}]
        self.assertEqual(c.own_neighbors(unnamed, "cell1-example", macs), ([], True))
        # A switch that speaks LLDP and another Spark through it are both kept, in order.
        crs = {"system": "crs804", "port": "qsfp56-dd-1-1", "chassisId": "48:a9:8a:00:00:01", "portId": "qsfp56-dd-1-1"}
        cell2 = {"system": "cell2-example", "port": "enp1s0f1np1", "chassisId": "fc:9d:05:9b:e9:14", "portId": "fc:9d:05:3d:3e:ae"}
        self.assertEqual(c.own_neighbors(heard["enp1s0f1np1"] + [crs, cell2], "cell1-example", macs), ([crs, cell2], False))
        self.assertEqual(c.own_neighbors([], "cell1-example", macs), ([], False), "nothing heard isn't self-only")

    def test_psi(self):
        text = "some avg10=1.25 avg60=0.50 avg300=0.10 total=123\nfull avg10=0.40 avg60=0.10 avg300=0.00 total=45\n"
        self.assertEqual(c.parse_psi(text), (1.25, 0.4))
        self.assertEqual(c.parse_psi(""), (None, None))

    def test_full_names(self):
        self.assertEqual(c.full_name("dashboard-servi", ["/opt/nvidia/dgx-dashboard-service/dashboard-service", "-port", "11000"]), "dashboard-service")
        self.assertEqual(c.full_name("networkd-dispat", ["/usr/bin/python3", "/usr/bin/networkd-dispatcher"]), "networkd-dispatcher")
        self.assertEqual(c.full_name("avahi-daemon", ["avahi-daemon: running [cell1-example.local]"]), "avahi-daemon")
        self.assertEqual(c.full_name("python3", ["python3", "-u", "/home/sparkuser/.sparkbox/collector.py"]), "python3")

    def test_ipv6_addresses(self):
        self.assertEqual(c.ipv6_text("00000000000000000000000001000000"), "::1")
        self.assertEqual(c.ipv6_text("0000000000000000FFFF00000100007F"), "127.0.0.1")
        self.assertEqual(c.ipv6_text("00000000000000000000000000000000"), "::")


class StuckNvidiaSmiDoesNotLeak(Case):
    """An nvidia-smi that won't die (stuck in the kernel with the GPU): KILL is a no-op here, as
    it is for a process in D state. Over 60 polls SparkBox starts it once, says so, and its
    threads and file descriptors stay level."""

    def test_sixty_polls_start_one_and_hold_nothing_open(self):
        bindir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, bindir)
        started = os.path.join(bindir, "started")
        with open(os.path.join(bindir, "nvidia-smi"), "w") as f:
            f.write("#!/bin/sh\ntrap '' TERM HUP INT\necho $$ >> %s\nexec sleep 600\n" % started)
        os.chmod(os.path.join(bindir, "nvidia-smi"), 0o755)
        old_path, old_kill, old_wait = os.environ["PATH"], c.kill_group, c.KILL_WAIT
        os.environ["PATH"] = bindir + os.pathsep + old_path
        c.kill_group = lambda p: None
        c.KILL_WAIT = 0.1
        fds = lambda: len(os.listdir("/dev/fd"))
        try:
            sampler = c.GpuSampler.__new__(c.GpuSampler)
            sampler.every, sampler.poll_timeout, sampler.nvml, sampler.stream = 1.0, 0.2, None, None
            sampler.error, sampler.latest, sampler.source, sampler.extra_query = None, None, "nvidia-smi", "clocks_event_reasons.active"
            sampler.backoff, sampler.not_before, sampler.started, sampler.ready = c.NVIDIA_SMI_BACKOFF, 0.0, c.time.monotonic(), c.threading.Event()
            sampler.read_once()
            self.assertEqual(c.stuck("nvidia-smi"), 1)
            threads, files = c.threading.active_count(), fds()
            for _ in range(60):
                sampler.read_once()
                sampler.not_before = 0.0  # even with the back-off over, nothing starts while one is stuck
            self.assertEqual((c.threading.active_count(), fds()), (threads, files))
            self.assertEqual(len(load(started).split()), 1, "started once")
            self.assertEqual(sampler.error, "nvidia-smi is stuck in the kernel (1 process)")
            self.assertEqual(sampler.reading(c.time.monotonic()), (None, "nvidia-smi is stuck in the kernel (1 process)"))
            self.assertEqual(sampler.backoff, 2 * c.NVIDIA_SMI_BACKOFF, "the next try waits 20 s")
        finally:
            for pid in load(started).split() if os.path.exists(started) else []:
                try:
                    os.kill(int(pid), 9)
                except OSError:
                    pass
            os.environ["PATH"], c.kill_group, c.KILL_WAIT = old_path, old_kill, old_wait
        for _ in range(100):
            if not c.stuck("nvidia-smi"):
                break
            c.time.sleep(0.05)
        self.assertEqual(c.stuck("nvidia-smi"), 0, "the reaper lets go once it exits")

    def test_back_off_doubles_to_five_minutes(self):
        sampler = c.GpuSampler.__new__(c.GpuSampler)
        sampler.poll_timeout, sampler.backoff, sampler.not_before = 0.1, c.NVIDIA_SMI_BACKOFF, 0.0
        old = c.run_ex
        c.run_ex = lambda cmd, timeout=8: ("timeout", None, None)
        try:
            waits = []
            for _ in range(8):
                sampler.not_before = 0.0
                sampler.smi(["--query-gpu=utilization.gpu"])
                waits.append(round(sampler.not_before - c.time.monotonic()))
            self.assertEqual(waits, [10, 20, 40, 80, 160, 300, 300, 300])
            c.run_ex = lambda cmd, timeout=8: ("ok", "0\n", "")
            sampler.not_before = 0.0
            sampler.smi(["--query-gpu=utilization.gpu"])
            self.assertEqual(sampler.backoff, c.NVIDIA_SMI_BACKOFF, "an answer resets it")
        finally:
            c.run_ex = old


if __name__ == "__main__":
    unittest.main()
