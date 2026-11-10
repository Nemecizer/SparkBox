"""Tests for roce.py against fixture sysfs trees and real mlx5 `ethtool -S` / `mlnx_qos` output.

Run from this folder: python3 -I -m unittest roce_test
"""

import os
import shutil
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import roce  # noqa: E402

FIXTURES = os.path.join(HERE, "..", "fabric", "fixtures")


def fixture(name):
    with open(os.path.join(FIXTURES, name)) as f:
        return f.read()


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def make_device(root, dev, iface, hw=None, port=None, gids=None):
    """A /sys/class/infiniband/<dev> tree with a netdev and the given counter files."""
    base = os.path.join(root, "class", "infiniband", dev)
    os.makedirs(os.path.join(base, "device", "net", iface), exist_ok=True)
    for name, v in (hw or {}).items():
        write(os.path.join(base, "ports", "1", "hw_counters", name), f"{v}\n")
    for name, v in (port or {}).items():
        write(os.path.join(base, "ports", "1", "counters", name), f"{v}\n")
    for i, t in enumerate(gids or []):
        write(os.path.join(base, "ports", "1", "gid_attrs", "types", str(i)), t)


class SysfsTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="roce-sys-")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_reads_the_counters_the_driver_has_and_leaves_the_rest_out(self):
        make_device(
            self.root, "rocep1s0f1", "enp1s0f1np1",
            hw={"np_ecn_marked_roce_packets": 41200, "np_cnp_sent": 39870, "rp_cnp_handled": 38115, "out_of_sequence": 0,
                "local_ack_timeout_err": 2, "rx_icrc_encapsulated": 0, "req_cqe_error": 1},
            port={"port_rcv_errors": 0, "symbol_error": 0, "link_downed": 1},
            gids=["IB/RoCE v1\n", "RoCE v2\n"],
        )
        got = roce.sysfs_counters("rocep1s0f1", sys_root=self.root)
        self.assertEqual(got["ecnMarked"], 41200)
        self.assertEqual(got["cnpHandled"], 38115)
        self.assertEqual(got["localAckTimeoutErr"], 2)
        self.assertEqual(got["rxIcrcErr"], 0)
        self.assertEqual(got["reqCqeErr"], 1)
        self.assertEqual(got["linkDowned"], 1)
        # Files that aren't there are absent, never 0.
        self.assertNotIn("packetSeqErr", got)
        self.assertNotIn("portXmitDiscards", got)
        self.assertTrue(roce.roce_v2("rocep1s0f1", sys_root=self.root))

    def test_finds_rdma_devices_by_netdev(self):
        make_device(self.root, "rocep1s0f1", "enp1s0f1np1")
        make_device(self.root, "roceP2p1s0f1", "enP2p1s0f1np1")
        self.assertEqual(roce.rdma_devices(self.root), {"enp1s0f1np1": "rocep1s0f1", "enP2p1s0f1np1": "roceP2p1s0f1"})
        self.assertEqual(roce.rdma_devices(os.path.join(self.root, "nothing")), {})

    def test_unreadable_gid_slots_and_only_v1(self):
        make_device(self.root, "mlx5_0", "eth0", gids=["", "IB/RoCE v1\n"])
        self.assertFalse(roce.roce_v2("mlx5_0", sys_root=self.root))
        make_device(self.root, "mlx5_1", "eth1")
        self.assertIsNone(roce.roce_v2("mlx5_1", sys_root=self.root))

    def test_a_garbled_counter_file_is_absent(self):
        make_device(self.root, "rocep1s0f1", "enp1s0f1np1", hw={"out_of_sequence": "n/a"})
        self.assertNotIn("outOfSequence", roce.sysfs_counters("rocep1s0f1", sys_root=self.root))


class EthtoolTest(unittest.TestCase):
    def test_parses_a_real_mlx5_dump(self):
        got = roce.parse_ethtool(fixture("mlx5_ethtool_S.txt"))
        self.assertEqual(got["rxPause"], [0, 0, 0, 3127, 0, 0, 0, 0])
        self.assertEqual(got["txPause"], [0, 0, 0, 2204, 0, 0, 0, 0])
        self.assertEqual(got["rxPauseCtrl"], 3127)
        self.assertEqual(got["txPauseCtrl"], 2204)
        self.assertEqual(got["rxCrcErrorsPhy"], 3)
        self.assertEqual(got["rxSymbolErrPhy"], 0)
        self.assertEqual(got["rxDiscardsPhy"], 0)
        self.assertEqual(got["rxOutOfBuffer"], 0)
        # Look-alikes (pause_duration, pause_transition, rx_prio3_discards) aren't confused for these.
        self.assertEqual(set(got), {"rxPause", "txPause", "rxPrioBytes", "rxPauseCtrl", "txPauseCtrl", "rxCrcErrorsPhy", "rxSymbolErrPhy", "rxDiscardsPhy", "rxOutOfBuffer"})

    def test_priority_traffic_is_distinct_from_pause_and_missing_is_unknown(self):
        got = roce.parse_ethtool("rx_prio0_bytes: 52\nrx_prio3_bytes: 400\nrx_prio3_pause: 2\nrx_prio8_bytes: 99\nrx_prio2_bytes: -1\n")
        self.assertEqual(got["rxPrioBytes"], [52, None, None, 400, None, None, None, None])
        self.assertEqual(got["rxPause"][3], 2)

    def test_missing_priorities_are_none_and_missing_counters_absent(self):
        got = roce.parse_ethtool("NIC statistics:\n     rx_prio3_pause: 12\n     tx_prio3_pause: 7\n     rx_packets: 5\n")
        self.assertEqual(got["rxPause"], [None, None, None, 12, None, None, None, None])
        self.assertEqual(len(got["txPause"]), 8)
        self.assertNotIn("rxCrcErrorsPhy", got)
        self.assertEqual(roce.parse_ethtool("no stats available\n"), {})

    def test_mlnx_qos(self):
        self.assertEqual(roce.parse_mlnx_qos(fixture("mlnx_qos.txt")), {"trust": "dscp", "pfcPrios": [3]})
        off = fixture("mlnx_qos.txt").replace("enabled     0   0   0   1", "enabled     0   0   0   0")
        self.assertEqual(roce.parse_mlnx_qos(off)["pfcPrios"], [])
        self.assertEqual(roce.parse_mlnx_qos(""), {})


class ReaderTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="roce-reader-")
        make_device(self.root, "rocep1s0f1", "enp1s0f1np1", hw={"out_of_sequence": 4}, port={"link_downed": 1}, gids=["RoCE v2\n"])
        make_device(self.root, "rocep1s0f0", "enp1s0f0np0", hw={"out_of_sequence": 0})
        self.calls = []
        self.now = 1000.0

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def runner(self, argv, timeout=3):
        self.calls.append(argv)
        if argv[0] == "ethtool":
            return fixture("mlx5_ethtool_S.txt")
        if argv[0] == "mlnx_qos":
            return None  # not installed
        return None

    def test_collects_per_interface_and_runs_ethtool_every_10_s(self):
        r = roce.Reader(sys_root=self.root, runner=self.runner, clock=lambda: self.now)
        got = r.collect(["enp1s0f1np1"])
        self.assertEqual(list(got), ["enp1s0f1np1"])
        c = got["enp1s0f1np1"]
        self.assertEqual(c["outOfSequence"], 4)
        self.assertEqual(c["txPause"][3], 2204)
        self.assertTrue(c["roceV2"])
        self.assertNotIn("trust", c, "no mlnx_qos: no trust or PFC claims")
        self.assertEqual(self.calls, [["ethtool", "-S", "enp1s0f1np1"], ["mlnx_qos", "-i", "enp1s0f1np1"]])
        self.now += 5
        r.collect(["enp1s0f1np1"])
        self.assertEqual(len(self.calls), 2, "cached for 10 s")
        self.now += 6
        r.collect(["enp1s0f1np1"])
        self.assertEqual(len(self.calls), 4)

    def test_a_slow_ethtool_is_left_alone_for_5_minutes(self):
        def slow(argv, timeout=3):
            self.calls.append(argv[0])
            self.now += 3  # it took 3 s (a wedged driver, killed at its deadline)
            return None
        r = roce.Reader(sys_root=self.root, runner=slow, clock=lambda: self.now)
        r.collect(["enp1s0f1np1"])
        self.assertEqual(len(self.calls), 2)
        self.now += 60
        r.collect(["enp1s0f1np1"])
        self.assertEqual(len(self.calls), 2, "not again a minute later")
        self.now += 300
        r.collect(["enp1s0f1np1"])
        self.assertEqual(len(self.calls), 4, "tried again after 5 minutes")

    def test_counters_are_cumulative_reads(self):
        r = roce.Reader(sys_root=self.root, runner=lambda argv, timeout=3: None, clock=lambda: self.now)
        self.assertEqual(r.collect()["enp1s0f1np1"]["outOfSequence"], 4)
        write(os.path.join(self.root, "class", "infiniband", "rocep1s0f1", "ports", "1", "hw_counters", "out_of_sequence"), "9\n")
        self.assertEqual(r.collect()["enp1s0f1np1"]["outOfSequence"], 9)
        self.assertNotIn("rxPause", r.collect()["enp1s0f1np1"], "no ethtool: no pause counters at all")

    def test_the_collectors_entry_point_reads_one_interface(self):
        r = roce.Reader(sys_root=self.root, runner=self.runner, clock=lambda: self.now)
        c = r.counters("enp1s0f1np1", "rocep1s0f1")
        self.assertEqual((c["outOfSequence"], c["linkDowned"], len(c["rxPause"])), (4, 1, 8))
        roce._reader = r
        try:
            self.assertEqual(roce.counters("enp1s0f1np1", "rocep1s0f1"), c)
        finally:
            roce._reader = None

    def test_run_handles_a_missing_tool(self):
        self.assertIsNone(roce.run(["sparkbox-no-such-tool-xyz"]))

    def test_run_never_waits_on_a_hung_tool(self):
        # ethtool on a wedged mlx5 driver: it ignores SIGTERM and a grandchild in its own
        # session holds the pipe, as a process stuck in the kernel would.
        pidfile = os.path.join(self.root, "child.pid")
        script = os.path.join(self.root, "ethtool")
        write(script, "#!%s\nimport os, signal, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                      "if os.fork() == 0:\n    os.setsid()\n    open(%r, 'w').write(str(os.getpid()))\n    time.sleep(60)\n"
                      "time.sleep(60)\n" % (sys.executable, pidfile))
        os.chmod(script, 0o755)
        started = time.monotonic()
        try:
            self.assertIsNone(roce.run([script, "-S", "enp1s0f1np1"], timeout=0.5))
            self.assertLess(time.monotonic() - started, 0.5 + 2 + 1)
        finally:
            try:
                with open(pidfile) as f:
                    os.kill(int(f.read()), 9)
            except (OSError, ValueError):
                pass


class IfaceRatesTest(unittest.TestCase):
    """The rate windows roce-iface judges by (the app's fabric::rates)."""

    def test_ten_second_and_full_minute_rates_and_none_before_there_is_history(self):
        r = roce.IfaceRates()
        t0 = 1_800_000_000_000
        r.add(t0, {"outOfSequence": 0})
        self.assertEqual(r.rates(t0)["outOfSequence"], (None, None), "one sample: no rate yet, not 0")
        for i in range(1, 31):  # a minute of 2 s samples at 5/s
            r.add(t0 + i * 2000, {"outOfSequence": i * 10})
        r10, r60 = r.rates(t0 + 60_000)["outOfSequence"]
        self.assertAlmostEqual(r10, 5.0)
        self.assertAlmostEqual(r60, 5.0)
        r.add(t0 + 62_000, {"outOfSequence": 290})  # reset (a driver reload): history starts again
        self.assertEqual(r.rates(t0 + 62_000)["outOfSequence"], (None, None))
        self.assertNotIn("rxCrcErrorsPhy", r.rates(t0 + 62_000), "a counter the driver doesn't report is absent")

    def test_a_pause_storm_is_timed(self):
        r = roce.IfaceRates()
        t0 = 1_800_000_000_000
        for i in range(20):
            r.add(t0 + i * 2000, {"txPause": [0, 0, 0, i * 50_000, 0, 0, 0, 0]})  # 25k/s on priority 3
        self.assertEqual(r.storm_s(t0 + 38_000), 38)
        r.add(t0 + 40_000, {"txPause": [0, 0, 0, 19 * 50_000 + 10, 0, 0, 0, 0]})
        self.assertIsNone(r.storm_s(t0 + 40_000), "the storm ended")

    def test_rates_print_with_real_precision(self):
        self.assertEqual([roce.fmt_per_s(v) for v in (0, 0.004, 0.0412, 4.12, 162, 1500, 50200)],
                         ["0/s", "0.004/s", "0.041/s", "4.1/s", "162/s", "1.5k/s", "50k/s"])

    def test_halves(self):
        self.assertEqual(roce.half("enP2p1s0f1np1", "192.168.101.12", []), 2)
        self.assertEqual(roce.half("enp1s0f1np1", "192.168.100.12", []), 1)
        self.assertEqual(roce.half("enP2p1s0f1np1", None, ["enp1s0f1np1", "enP2p1s0f1np1"]), 2)


if __name__ == "__main__":
    unittest.main()
