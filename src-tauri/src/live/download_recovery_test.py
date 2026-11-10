"""Offline recovery fixtures; Docker is a local stand-in, HOME is temporary."""
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock
import sparkjob_test as fixture

sj = fixture.sj


class DownloadRecovery(unittest.TestCase):
    def setUp(self):
        self.dest = tempfile.mkdtemp(dir=fixture.TMP_HOME)
        self.old = "resume-" + os.path.basename(self.dest).replace("_", "-")
        self.spec = {"repo": "org/model", "revision": "a" * 40, "dest": self.dest, "image": "vllm:test", "tool": fixture.image_tool(), "include": [],
                     "files": [{"path": "model.safetensors", "size": 10, "sha256": "b" * 64}], "instance": "dev"}
        self.plan = sj.download_plan(self.spec)
        self.status = sj.write_status(self.old, kind="download", state="cancelled", dest=self.dest, instance="dev",
                                      downloadPlan=self.plan, planHash=sj.plan_hash(self.plan))
        sj.mark_partial(self.dest, self.old, "download")
        self.resume = dict(self.spec, resumeOf=self.old, resumeHash=sj.plan_hash(self.plan))

    def tearDown(self):
        for name in (self.old, self.old + "-new"):
            Path(sj.status_path(name)).unlink(missing_ok=True)
        shutil.rmtree(self.dest, ignore_errors=True)

    def test_saved_plan_is_pinned_without_credentials_or_extra_fields(self):
        spec = dict(self.spec, token="fake-secret", password="fake-password", arbitrary="fake-extra")
        spec["files"] = [dict(self.spec["files"][0], token="fake-nested")]
        self.assertEqual(sj.download_plan(spec), self.plan)
        with mock.patch.object(os, "fork", return_value=999999), mock.patch.object(sj, "boot_id", return_value="boot-a"), mock.patch.object(sj, "download_tools", return_value={"options":[fixture.image_tool()]}):
            st = sj.start("download", self.old + "-new", spec)
        saved = sj.read_status(st["id"])
        self.assertEqual(saved["downloadPlan"], self.plan)
        self.assertEqual(saved["bootId"], "boot-a")
        self.assertNotIn("fake-", json.dumps(saved))
        with self.assertRaisesRegex(RuntimeError, "already exists"), mock.patch.object(sj, "download_tools", return_value={"options":[fixture.image_tool()]}):
            sj.start("download", self.old + "-new", spec)
        self.assertEqual(sj.read_status(st["id"]), saved)

    def test_resume_rechecks_plan_owner_state_and_marker_before_cleanup(self):
        sj.check_download_resume(self.resume)
        for key, value in [("revision", "b" * 40), ("dest", self.dest + "/other"), ("instance", "other"), ("resumeHash", "0" * 64)]:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                sj.check_download_resume(dict(self.resume, **{key: value}))
        sj.write_status(self.old, state="done")
        with self.assertRaisesRegex(RuntimeError, "stopped"):
            sj.check_download_resume(self.resume)
        sj.write_status(self.old, state="running", pid=os.getpid(), pidStart=sj.start_of(os.getpid()))
        with self.assertRaisesRegex(RuntimeError, "stopped"):
            sj.check_download_resume(self.resume)
        sj.write_status(self.old, state="cancelled")
        sj.mark_partial(self.dest, "newer-job", "download")
        with mock.patch.object(sj, "stop_leftover") as stop:
            with self.assertRaisesRegex(RuntimeError, "no longer belongs"):
                sj.claim_folder(self.dest, "new-attempt", "download", "dev", self.resume)
            stop.assert_not_called()

    def test_two_resumes_cannot_claim_the_folder_and_old_record_survives(self):
        original = Path(sj.status_path(self.old)).read_bytes()
        with mock.patch.object(sj, "stop_leftover") as stop:
            sj.claim_folder(self.dest, "new-attempt", "download", "dev", self.resume)
            with self.assertRaisesRegex(RuntimeError, "no longer belongs"):
                sj.claim_folder(self.dest, "other-attempt", "download", "dev", self.resume)
            self.assertEqual(stop.call_count, 1)
        self.assertEqual(Path(sj.status_path(self.old)).read_bytes(), original)

    def test_legacy_unpinned_or_unsafe_file_lists_have_no_recovery_plan(self):
        for change in ({"revision": "main"}, {"files": []}, {"include": ["*"]}, {"dest": "/"},
                       {"files": [{"path": "../secret"}]}, {"files": [{"path": "weights*"}]},
                       {"files": [{"path": "weights", "sha256": "bad"}]}):
            with self.subTest(change=change):
                self.assertIsNone(sj.download_plan(dict(self.spec, **change)))
        sj.write_status(self.old, downloadPlan=None)
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            sj.check_download_resume(self.resume)

    def test_reboot_with_identical_pid_and_start_ticks_is_not_alive_or_killed(self):
        st = dict(self.status, state="running", pid=4242, pidStart=100, childPid=4243, childStart=100,
                  bootId="old-boot", childBootId="old-boot")
        with mock.patch.object(sj, "boot_id", return_value="new-boot"), mock.patch.object(sj, "alive", return_value=True), \
             mock.patch.object(sj, "process_facts", return_value={"start": 100, "uid": os.getuid(), "argv": ["rsync", "-a", self.dest]}):
            self.assertFalse(sj.job_running(st))
            self.assertIsNone(sj.orphan(st))
            with mock.patch.object(sj, "read_status", return_value=st), mock.patch.object(sj, "stop_download"), \
                 mock.patch.object(sj, "labelled_containers", return_value=[]), mock.patch.object(os, "kill") as kill, \
                 mock.patch.object(os, "killpg") as killpg:
                sj.cancel(self.old)
                kill.assert_not_called()
                killpg.assert_not_called()

    def test_detached_resume_verifies_pinned_files_and_preserves_old_attempt(self):
        docker = fixture.Docker()
        # This fixture also models --rm after success; the older stand-in only models docker stop.
        fixture.stand_in(docker.bin, "docker", (fixture.Docker.SCRIPT + '\nopen(stopped, "w").close()\n') % docker.bin)
        first = dict(docker.spec(self.dest), revision="a" * 40)
        job = self.old + "-download"
        fixture.sparkjob("start", "download", job, stdin=first, path=docker.bin)
        run = docker.run()
        docker.kill(job, run)
        fixture.sparkjob("list", path=docker.bin)
        old = sj.read_status(job)
        original = Path(sj.status_path(job)).read_bytes()
        retry = dict(old["downloadPlan"], tool=fixture.image_tool(), instance="dev", token="fake-fresh-token", resumeOf=job, resumeHash=old["planHash"])
        resumed = job + "-retry"
        fixture.sparkjob("start", "download", resumed, stdin=retry, path=docker.bin)
        fixture.wait_for("the resumed container", lambda: docker.run()["id"] != run["id"])
        argv = docker.run()["argv"]
        self.assertIn("--rm", argv, "the fixture's automatic removal models the actual command")
        self.assertEqual(argv[argv.index("--revision") + 1], "a" * 40)
        self.assertEqual([argv[i + 1] for i, arg in enumerate(argv) if arg == "--include"], ["config.json", "model.safetensors"])
        self.assertFalse(docker.second_writer())
        self.assertIn(["stop", "-t", "5", run["id"]], docker.calls("stop"))
        docker.finish()
        result = fixture.wait_done(resumed)
        self.assertEqual(result["state"], "done", result.get("message"))
        self.assertEqual(result["resumeOf"], job)
        self.assertEqual(sj.read_manifest(self.dest)["revision"], "a" * 40)
        self.assertEqual(sj.read_manifest(self.dest)["checked"], "sha256")
        self.assertEqual(sj.read_marker(self.dest), {})
        self.assertEqual(Path(sj.status_path(job)).read_bytes(), original)
        self.assertNotIn("fake-fresh-token", json.dumps(result))
        self.assertFalse(os.path.exists(os.path.join(sj.SECRETS, resumed)))
        self.assertEqual(docker.running(), [])

    def test_reboot_is_reported_read_only_then_settled_by_normal_listing(self):
        sj.write_status(self.old, state="running", pid=4242, pidStart=100, bootId="old-boot")
        before = Path(sj.status_path(self.old)).read_bytes()
        with mock.patch.object(sj, "boot_id", return_value="new-boot"), mock.patch.object(sj, "alive", return_value=True), \
             mock.patch.object(sj, "start_of", return_value=100):
            view = next(j for j in sj.list_jobs(look=True) if j["id"] == self.old)
            self.assertEqual(view["state"], "failed")
            self.assertEqual(Path(sj.status_path(self.old)).read_bytes(), before)
            sj.list_jobs()
            self.assertEqual(sj.read_status(self.old)["state"], "failed")
            self.assertEqual(sj.read_status(self.old)["downloadPlan"], self.plan)

    def test_unreadable_boot_identity_does_not_authorize_signaling_old_processes(self):
        st = dict(self.status, bootId="saved", childPid=4243, childStart=100)
        with mock.patch.object(sj, "boot_id", return_value=None):
            self.assertTrue(sj.child_replaced(st))
            self.assertTrue(sj.unverified_boot(st))
            self.assertIsNone(sj.orphan(st))


if __name__ == "__main__":
    unittest.main()
