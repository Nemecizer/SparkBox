"""Version evidence stays read-only, bounded, allowlisted, and honest about missing facts."""
import hashlib
import json
import subprocess
import unittest
from unittest import mock
import collector
import sparkjob

IMAGE_ID = "sha256:" + "a" * 64

class VersionFacts(unittest.TestCase):
    def facts(self, env=None, image_id=IMAGE_ID, package="install ok installed|1.17.8-1", dgx='DGX_OTA_VERSION="7.5"\n DGX_OTA_VERSION = "7.6"\n'):
        def read(argv):
            if argv == ["nvidia-smi"]: return "Driver Version: 580.1    CUDA Version: 13.0"
            if argv[0] == "dpkg-query": return package
            self.assertEqual(argv, ["docker", "image", "inspect", "selected:image"])
            return json.dumps([{"Id": image_id, "Config": {"Env": env or []}}])
        with mock.patch.object(sparkjob, "version_read", side_effect=read), mock.patch.object(sparkjob, "read_sys", return_value=dgx), mock.patch("builtins.open", mock.mock_open(read_data=b"collector-source")):
            return sparkjob.version_facts("selected:image")

    def test_only_version_declarations_leave_the_selected_image(self):
        result = self.facts(["NCCL_VERSION=2.27.3-1", "HF_TOKEN=secret", "NCCL_SOCKET_IFNAME=private-interface"])
        self.assertEqual(result["imageId"], IMAGE_ID)
        self.assertEqual(result["nccl"], "2.27.3-1")
        self.assertEqual((result["driver"], result["cuda"], result["dgxOta"], result["toolkit"]), ("580.1", "13.0", "7.6", "1.17.8-1"))
        self.assertEqual(result["collector"], hashlib.sha256(b"collector-source").hexdigest()[:12])
        self.assertEqual(set(result), {"driver", "cuda", "dgxOta", "toolkit", "collector", "kernel", "imageId", "nccl"})
        self.assertNotIn("secret", json.dumps(result)); self.assertNotIn("private-interface", json.dumps(result))

    def test_absent_conflicting_and_invalid_declarations_are_unknown(self):
        for env in [[], ["NCCL_VERSION=2.27", "NCCL_VERSION=2.28"], ["NCCL_VERSION=(hidden)"], ["NCCL_VERSION=token=secret"], ["NCCL_VERSION=2.1\nsecret"]]:
            self.assertIsNone(self.facts(env)["nccl"])
        self.assertIsNone(self.facts(image_id="vllm:tag")["imageId"])
        self.assertIsNone(self.facts(package="deinstall ok config-files|1.17.8")["toolkit"])
        self.assertIsNone(self.facts(dgx='DGX_OTA_VERSION="7.6"\nDGX_OTA_VERSION=""\n')["dgxOta"])

    def test_failed_or_malformed_reads_leave_unknowns(self):
        with mock.patch.object(sparkjob, "version_read", return_value="not-json"), mock.patch.object(sparkjob, "read_sys", return_value=None), mock.patch("builtins.open", side_effect=OSError):
            result = sparkjob.version_facts("selected:image")
        for key in ("driver", "cuda", "imageId", "nccl", "toolkit", "collector", "dgxOta"): self.assertIsNone(result[key])

    def test_each_new_command_is_a_bounded_read(self):
        for error in (OSError(), subprocess.TimeoutExpired("read", 5)):
            with mock.patch.object(sparkjob.subprocess, "run", side_effect=error) as run:
                self.assertEqual(sparkjob.version_read(["nvidia-smi"]), "")
                self.assertEqual(run.call_args.kwargs["timeout"], 5)
        with mock.patch.object(sparkjob.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "bad", "secret")):
            self.assertEqual(sparkjob.version_read(["nvidia-smi"]), "")

    def test_collector_uses_an_immutable_image_id_and_rejects_conflicting_nccl(self):
        self.assertEqual(collector.inspect_meta({"Image": IMAGE_ID})["imageId"], IMAGE_ID)
        for bad in (None, "vllm:latest", "sha256:short"): self.assertIsNone(collector.inspect_meta({"Image": bad})["imageId"])
        self.assertNotIn("NCCL_VERSION", collector.env_flags(["NCCL_VERSION=2.27", "NCCL_VERSION=2.28"]))
        self.assertEqual(collector.env_flags(["NCCL_VERSION=2.27", "NCCL_VERSION=2.27"])["NCCL_VERSION"], "2.27")
        self.assertRegex(collector.COLLECTOR_BUILD, r"^[0-9a-f]{12}$")

    def test_collector_store_and_toolkit_match_preflight_meanings(self):
        for info, expected in [({"Driver": "overlay2"}, "overlay2"), ({"Driver": "overlayfs", "DriverStatus": [["driver-type", "io.containerd.snapshotter.v1"]]}, "containerd"), ({}, None)]:
            with mock.patch.object(collector, "docker", return_value=info), mock.patch.object(collector, "read", return_value=""), mock.patch.object(collector.glob, "glob", return_value=[]), mock.patch.object(collector, "run", side_effect=lambda argv, **kw: "install ok installed|1.17.8-1" if argv[0] == "dpkg-query" else ""):
                result = collector.system_extras()
            self.assertEqual(result["imageStore"], expected)
            self.assertEqual(result["containerToolkit"], "1.17.8-1")
        with mock.patch.object(collector, "docker", side_effect=OSError), mock.patch.object(collector, "read", return_value=""), mock.patch.object(collector.glob, "glob", return_value=[]), mock.patch.object(collector, "run", return_value=""):
            result = collector.system_extras()
        self.assertIsNone(result["imageStore"]); self.assertIsNone(result["containerToolkit"])
