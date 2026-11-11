"""Tests for sparkjob.py. Run: python3 -m unittest src/live/sparkjob_test.py (from src-tauri/).
They use a temporary HOME, so nothing touches the real ~/.sparkbox, and that HOME and whatever is
still running from it go when the run ends."""
import atexit
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

TMP_HOME = tempfile.mkdtemp(prefix="sparkjob-home-")
os.environ["HOME"] = TMP_HOME
sys.path.insert(0, os.path.dirname(__file__))
import sparkjob as sj  # noqa: E402

LINUX_IDS, LINUX_FACTS = sj.process_ids, sj.process_facts


def marked(mark):
    """The processes with mark in their command line, each as its pid and then its words."""
    listed = subprocess.run(["ps", "axww", "-o", "pid=", "-o", "command="], capture_output=True, text=True).stdout
    return [l.split() for l in listed.splitlines() if mark in l]


def end_stand_ins(mark):
    """Ends every process with mark in its command line and waits until they've gone. It kills no
    group: a stand-in a test runs directly is in this process's, and a stand-in's processes all have
    that command line anyway."""
    for words in marked(mark):
        try:
            os.kill(int(words[0]), signal.SIGKILL)
        except OSError:
            pass
    wait_for("the stand-ins to go", lambda: not marked(mark))


def clean_up():
    """The end of a run, whether its tests passed or not: what's left of the jobs it started, which
    would write their status back into a home that's gone, its stand-ins and that home. A forked job
    never gets here."""
    if os.getpid() == RUNNER:
        listed = subprocess.run(["ps", "axww", "-o", "pid=", "-o", "ppid="], capture_output=True, text=True).stdout
        for pid, parent in map(str.split, listed.splitlines()):
            if int(parent) == RUNNER:
                end_job(int(pid))
        try:
            end_stand_ins(TMP_HOME)
        finally:
            shutil.rmtree(TMP_HOME, ignore_errors=True)


RUNNER = os.getpid()
atexit.register(clean_up)
signal.signal(signal.SIGTERM, signal.default_int_handler)  # `timeout` ends a run the way Ctrl-C does


def ps_ids():
    """The processes writers() could care about, this run's alone: asking `ps` about each of a Mac's
    takes seconds, and what another run left is none of this one's."""
    return [int(w[0]) for w in marked(TMP_HOME) if os.path.basename(w[1]) in ("rsync", "docker")]


def ps_facts(pid):
    """sj.process_facts where there's no /proc. `ps` splits argv on spaces and gives the start to the
    second, which the stand-ins here tolerate; on Linux (check.sh, Python 3.12) nothing replaces it."""
    words = subprocess.run(["ps", "-ww", "-o", "uid=", "-o", "state=", "-o", "lstart=", "-o", "command=", "-p", str(pid)],
                           capture_output=True, text=True).stdout.split()
    if len(words) < 8 or words[1].startswith("Z"):
        return None
    return {"start": " ".join(words[2:7]), "uid": int(words[0]), "argv": words[7:]}


if not os.path.isdir("/proc/self"):
    sj.process_ids, sj.process_facts = ps_ids, ps_facts


def make_model(path, files):
    os.makedirs(path, exist_ok=True)
    for name, data in files.items():
        full = os.path.join(path, name)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as f:
            f.write(data)


def wait_done(job_id, timeout=20):
    for _ in range(int(timeout * 10)):
        st = sj.read_status(job_id)
        if st.get("state") in ("done", "failed", "cancelled"):
            return st
        time.sleep(0.1)
    raise AssertionError("job never finished: %r" % sj.read_status(job_id))


def image_tool():
    return {"kind": "image", "path": "sha256:" + "b" * 64, "fingerprint": "sha256:" + "b" * 64, "label": "vllm:test"}


class Inspection(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(dir=TMP_HOME)

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_scan_finds_plain_and_hf_cache_models_once(self):
        make_model(os.path.join(self.root, "Qwen", "Qwen3.8-27B-FP8"),
                   {"config.json": b"{}", "model-00001.safetensors": b"x" * 10})
        snap = os.path.join(self.root, "hub", "models--nvidia--GLM-5.3-Flash-NVFP4", "snapshots", "abc123")
        blobs = os.path.join(self.root, "hub", "models--nvidia--GLM-5.3-Flash-NVFP4", "blobs")
        make_model(blobs, {"b1": b"{}", "b2": b"y" * 20})
        os.makedirs(snap)
        os.symlink(os.path.join(blobs, "b1"), os.path.join(snap, "config.json"))
        os.symlink(os.path.join(blobs, "b2"), os.path.join(snap, "model.safetensors"))
        make_model(os.path.join(self.root, "notes"), {"readme.txt": b"hi"})

        found = {m["repo"]: m for m in sj.scan([self.root, self.root])}
        self.assertEqual(set(found), {"Qwen/Qwen3.8-27B-FP8", "nvidia/GLM-5.3-Flash-NVFP4"})
        self.assertEqual(found["nvidia/GLM-5.3-Flash-NVFP4"]["sizeBytes"], 22, "symlinked blobs are counted")
        self.assertEqual(found["Qwen/Qwen3.8-27B-FP8"]["format"], "safetensors")

    def test_verify_reports_missing_wrong_size_and_bad_checksum(self):
        make_model(self.root, {"a.safetensors": b"aaaa", "b.safetensors": b"bbbb", "config.json": b"{}"})
        good = hashlib.sha256(b"aaaa").hexdigest()
        self.assertEqual(sj.verify(self.root, [{"path": "a.safetensors", "size": 4, "sha256": good}]), [])
        problems = sj.verify(self.root, [
            {"path": "b.safetensors", "size": 4, "sha256": "0" * 64},
            {"path": "config.json", "size": 99},
            {"path": "gone.safetensors", "size": 1},
        ])
        self.assertEqual(len(problems), 3)
        self.assertTrue(any("wrong checksum" in p for p in problems), problems)

    def test_delete_only_removes_model_folders_inside_root(self):
        model = os.path.join(self.root, "org", "model")
        make_model(model, {"config.json": b"{}", "w.safetensors": b"1"})
        with self.assertRaises(SystemExit):
            sj.delete(self.root, os.path.join(self.root, "org"))  # no config.json: not a model
        with self.assertRaises(SystemExit):
            sj.delete(os.path.join(self.root, "org", "model"), model)  # the root itself
        with self.assertRaises(SystemExit):
            sj.delete(self.root, "/tmp")
        sj.delete(self.root, model)
        self.assertFalse(os.path.exists(model))

    def test_manifest_round_trip(self):
        make_model(self.root, {"config.json": b"{}", "w.safetensors": b"12345"})
        sj.write_manifest(self.root, "org/model", "rev1", [{"path": "w.safetensors", "size": 5, "sha256": "abc"}])
        self.assertEqual(sj.read_manifest(self.root)["revision"], "rev1")
        self.assertEqual([f["path"] for f in sj.listing(self.root)], [".sparkbox.json", "config.json", "w.safetensors"])


class LibraryReliability(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(dir=TMP_HOME)
        self.model = os.path.join(self.root, "org", "tiny")
        make_model(self.model, {"config.json": b"{}", "w.safetensors": b"12345"})

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_relative_roots_are_absolute_in_results_and_empty_roots_do_not_scan_home(self):
        with mock.patch.object(sj.os, "getcwd", return_value=os.path.dirname(self.root)):
            row = sj.scan([os.path.basename(self.root)])[0]
        self.assertEqual(row["path"], self.model)
        self.assertEqual(row["root"], self.root)
        with mock.patch.object(sj.os, "walk") as walk, self.assertRaises(ValueError):
            sj.scan([""])
        walk.assert_not_called()

    def test_missing_root_is_empty_but_nonfolder_or_unreadable_root_fails(self):
        self.assertEqual(sj.scan([self.root + "-missing"]), [])
        broken = os.path.join(self.root, "broken-root")
        os.symlink(self.root + "-missing", broken)
        with self.assertRaises(FileNotFoundError):
            sj.scan([broken])
        with self.assertRaises(NotADirectoryError):
            sj.scan([os.path.join(self.model, "config.json")])
        stat = os.stat
        def denied(path, *args, **kwargs):
            if path == self.root:
                raise PermissionError("fixture root denied")
            return stat(path, *args, **kwargs)
        with mock.patch.object(sj.os, "stat", side_effect=denied), self.assertRaises(PermissionError):
            sj.scan([self.root])

    def test_unreadable_subtree_does_not_return_a_partly_successful_scan(self):
        walk = os.walk
        def denied(path, **kwargs):
            yield from walk(path, **kwargs)
            if path == self.root:
                kwargs["onerror"](PermissionError("fixture subtree denied"))
        with mock.patch.object(sj.os, "walk", side_effect=denied), self.assertRaises(PermissionError):
            sj.scan([self.root])

    def test_unreadable_weight_is_an_error_instead_of_a_smaller_model(self):
        os.unlink(os.path.join(self.model, "w.safetensors"))
        os.symlink(os.path.join(self.root, "missing"), os.path.join(self.model, "w.safetensors"))
        with self.assertRaises(FileNotFoundError):
            sj.scan([self.root])

    def cache_snapshot(self):
        snapshot = os.path.join(self.root, "hub", "models--example--tiny", "snapshots", "a" * 40)
        blobs = os.path.join(os.path.dirname(os.path.dirname(snapshot)), "blobs")
        make_model(blobs, {"config": b"{}", "weight": b"12345"})
        os.makedirs(snapshot)
        for name, blob in [("config.json", "config"), ("w.safetensors", "weight"), ("missing.safetensors", "absent")]:
            os.symlink(os.path.relpath(os.path.join(blobs, blob), snapshot), os.path.join(snapshot, name))
        return snapshot

    def test_missing_cache_blobs_are_incomplete_without_hiding_healthy_models(self):
        snapshot = self.cache_snapshot()
        sj.write_manifest(snapshot, "example/tiny", "a" * 40, [], checked="sha256")
        manifest = os.path.join(snapshot, sj.MANIFEST)
        with open(manifest, "rb") as f:
            before = f.read()
        rows = {row["path"]: row for row in sj.scan([self.root])}
        self.assertEqual(set(rows), {self.model, snapshot})
        self.assertIsNone(rows[self.model]["partial"])
        broken = rows[snapshot]
        self.assertEqual(broken["partial"], "stopped")
        self.assertEqual(broken["missingCacheFiles"], 1)
        self.assertFalse(broken["verified"], "an old manifest cannot certify a missing blob")
        self.assertEqual(broken["checked"], "")
        self.assertIsNone(broken["progress"])
        self.assertEqual(broken["sizeBytes"], 7 + len(before))
        with open(manifest, "rb") as f:
            self.assertEqual(f.read(), before)
        self.assertTrue(os.path.islink(os.path.join(snapshot, "missing.safetensors")))

    def test_cache_permissions_and_disappearing_regular_files_still_fail(self):
        snapshot = self.cache_snapshot()
        stat = os.stat
        for error in (PermissionError("denied"), FileNotFoundError("disappeared")):
            make_model(snapshot, {"readme.txt": b"keep"})
            def fail(path, *args, **kwargs):
                if path == os.path.join(snapshot, "readme.txt"):
                    raise error
                return stat(path, *args, **kwargs)
            with mock.patch.object(sj.os, "stat", side_effect=fail), self.assertRaises(type(error)):
                sj.scan([self.root])

    def test_a_missing_config_link_is_incomplete_and_a_restored_blob_rescans(self):
        snapshot = self.cache_snapshot()
        config = os.path.realpath(os.path.join(snapshot, "config.json"))
        os.unlink(config)
        row = next(row for row in sj.scan([self.root]) if row["path"] == snapshot)
        self.assertEqual(row["missingCacheFiles"], 2)
        self.assertEqual(row["partial"], "stopped")
        make_model(os.path.dirname(config), {"config": b"{}", "absent": b"restored"})
        row = next(row for row in sj.scan([self.root]) if row["path"] == snapshot)
        self.assertEqual(row["missingCacheFiles"], 0)
        self.assertIsNone(row["partial"])
        self.assertFalse(row["verified"], "restoring a blob is not publisher verification")

    def test_missing_cache_files_keep_active_writer_protection(self):
        snapshot = self.cache_snapshot()
        sj.mark_partial(snapshot, "cache-writer", "pull")
        with mock.patch.object(sj, "read_status", return_value={"state": "running", "step": "copying", "startedAt": sj.now()}):
            row = next(row for row in sj.scan([self.root]) if row["path"] == snapshot)
        self.assertEqual(row["missingCacheFiles"], 1)
        self.assertEqual(row["partial"], "copying")
        self.assertFalse(row["verified"])

    def test_progress_is_live_marker_bound_and_never_marks_verification_finished(self):
        sj.mark_partial(self.model, "library-progress", "download")
        status = {"state": "running", "step": "verifying", "bytesDone": 100, "bytesTotal": 100, "startedAt": sj.now()}
        with mock.patch.object(sj, "read_status", return_value=status):
            row = sj.scan([self.root])[0]
            self.assertEqual(row["progress"], {"step": "verifying", "bytesDone": 100, "bytesTotal": 100})
            self.assertFalse(row["verified"])
            self.assertEqual(row["partial"], "downloading")
            status["step"] = "downloading"; status["bytesDone"] = 37
            self.assertEqual(sj.scan([self.root])[0]["progress"]["bytesDone"], 37)
            status["bytesDone"] = "unknown"
            self.assertIsNone(sj.scan([self.root])[0]["progress"])
            status["state"] = "failed"
            stopped = sj.scan([self.root])[0]
            self.assertEqual(stopped["partial"], "stopped")
            self.assertIsNone(stopped["progress"])


class DetachedJobs(unittest.TestCase):
    def test_a_pull_job_runs_detached_reports_progress_and_verifies(self):
        if not shutil.which("rsync"):
            self.skipTest("rsync not installed")
        src = tempfile.mkdtemp(dir=TMP_HOME)
        dest = os.path.join(TMP_HOME, "copy-of-model")
        payload = os.urandom(300_000)
        make_model(src, {"config.json": b"{}", "model.safetensors": payload})
        files = [{"path": "config.json", "size": 2},
                 {"path": "model.safetensors", "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}]

        st = sj.start("pull", "job-pull-1", {"src": src, "dest": dest, "password": "pw", "files": files, "checksums": True})
        self.assertEqual(st["state"], "running")
        self.assertTrue(st["pid"] > 0)
        st = wait_done("job-pull-1")
        self.assertEqual(st["state"], "done", st.get("message"))
        with open(os.path.join(dest, "model.safetensors"), "rb") as f:
            self.assertEqual(f.read(), payload)
        self.assertIn("job-pull-1", [j["id"] for j in sj.list_jobs()])

    def test_a_failing_job_explains_itself(self):
        st = sj.start("pull", "job-pull-bad", {"src": "/nonexistent/source", "dest": os.path.join(TMP_HOME, "x"),
                                               "password": "pw", "files": [{"path":"config.json", "size":2}]})
        st = wait_done("job-pull-bad")
        self.assertEqual(st["state"], "failed")
        self.assertIn("copy failed", st["message"])

    def test_bad_kinds_and_ids_are_refused(self):
        with self.assertRaises(SystemExit):
            sj.start("explode", "job-1", {})
        with self.assertRaises(SystemExit):
            sj.start("pull", "../../etc", {})

    def test_a_dead_job_is_marked_failed(self):
        sj.write_status("job-ghost", kind="pull", state="running", pid=999999)
        ghost = [j for j in sj.list_jobs() if j["id"] == "job-ghost"][0]
        self.assertEqual(ghost["state"], "failed")


    def test_a_download_asks_hf_for_the_served_files_only_and_verifies_them(self):
        bin_dir = tempfile.mkdtemp(dir=TMP_HOME)
        repo = tempfile.mkdtemp(dir=TMP_HOME)
        log = os.path.join(bin_dir, "argv.json")
        make_model(repo, {"config.json": b"{}", "tokenizer.json": b"[]", "model.safetensors": b"w" * 1000,
                          "original/consolidated.pth": b"p" * 5000})
        # A stand-in docker: "image inspect" succeeds; "run ... hf download" copies the repo
        # files that match an --include into the /dest mount, the way hf does.
        with open(os.path.join(bin_dir, "docker"), "w") as f:
            f.write("""#!/usr/bin/env python3
import fnmatch, json, os, shutil, sys
a = sys.argv[1:]
if a[:2] == ["image", "ls"]:
    print(json.dumps({"ID":"sha256:" + "b" * 64, "Repository":"vllm", "Tag":"test"}))
    sys.exit(0)
if a[-1:] == ["version"]:
    print("hf 2.4.0")
    sys.exit(0)
if a[0] != "run":
    sys.exit(0)
json.dump(a, open(%r, "w"))
dest = a[a.index("-v") + 1].split(":")[0]
pats = [a[i + 1] for i, x in enumerate(a) if x == "--include"]
for root, _, names in os.walk(%r):
    for n in names:
        rel = os.path.relpath(os.path.join(root, n), %r)
        if not pats or any(fnmatch.fnmatch(rel, p) for p in pats):
            os.makedirs(os.path.dirname(os.path.join(dest, rel)), exist_ok=True)
            shutil.copy(os.path.join(root, n), os.path.join(dest, rel))
""" % (log, repo, repo))
        os.chmod(os.path.join(bin_dir, "docker"), 0o755)
        old_path = os.environ["PATH"]
        os.environ["PATH"] = bin_dir + os.pathsep + old_path
        try:
            dest = os.path.join(TMP_HOME, "dl-model")
            files = [{"name": "config.json", "size": 2},
                     {"path": "model.safetensors", "size": 1000, "sha256": hashlib.sha256(b"w" * 1000).hexdigest()}]
            sj.start("download", "job-dl-1", {"repo": "org/m", "revision": "abc", "dest": dest, "image": "vllm:x", "tool": image_tool(), "instance": "dev",
                                               "files": files, "include": ["*.json"]})
            st = wait_done("job-dl-1")
        finally:
            os.environ["PATH"] = old_path
        self.assertEqual(st["state"], "done", st.get("message"))
        argv = json.load(open(log))
        includes = [argv[i + 1] for i, x in enumerate(argv) if x == "--include"]
        self.assertEqual(includes, ["config.json", "model.safetensors", "*.json"])
        self.assertFalse(os.path.exists(os.path.join(dest, "original")), "unlisted weights stay on the Hub")
        self.assertTrue(os.path.exists(os.path.join(dest, "tokenizer.json")), "include patterns are honoured")
        manifest = sj.read_manifest(dest)
        self.assertEqual([f["path"] for f in manifest["files"]], ["config.json", "model.safetensors"])

    def fake_docker(self, with_hf=True):
        bin_dir = tempfile.mkdtemp(dir=TMP_HOME)
        calls = os.path.join(bin_dir, "calls.log")
        stand_in(bin_dir, "docker", """#!/usr/bin/env python3
import json, sys
a = sys.argv[1:]
with open(%r, "a") as f: f.write(" ".join(a) + chr(10))
if a[:2] == ["image", "ls"]:
    print(json.dumps({"ID":"sha256:" + "b" * 64, "Repository":"vllm", "Tag":"test"}))
elif a[0] == "pull":
    raise SystemExit(99)
elif a[0] == "run" and a[-1:] == ["version"]:
    print("hf 2.4.0")
    raise SystemExit(0 if %r else 1)
""" % (calls, with_hf))
        return bin_dir, calls

    def download_with(self, bin_dir, job_id, tool=True, files=None):
        with mock.patch.dict(os.environ, {"PATH": bin_dir + os.pathsep + os.environ["PATH"]}):
            spec = {"repo":"org/m", "revision":"abc", "dest":os.path.join(TMP_HOME, job_id),
                    "image":"unused:configured", "files":files if files is not None else known_empty_file(os.path.join(TMP_HOME, job_id)), "instance":"dev"}
            if tool: spec["tool"] = image_tool()
            sj.start("download", job_id, spec)
            return wait_done(job_id)

    def test_only_the_selected_installed_image_runs_and_nothing_is_pulled(self):
        bin_dir, calls = self.fake_docker()
        st = self.download_with(bin_dir, "job-dl-local")
        self.assertEqual(st["state"], "done", st.get("message"))
        with open(calls) as f: log = f.read()
        self.assertNotIn("\npull ", log)
        self.assertIn("--pull never", log)
        self.assertIn("--entrypoint hf " + image_tool()["path"] + " download org/m", log)
        self.assertEqual(st["toolVersion"], "hf 2.4.0")

    def test_no_reviewed_tool_refuses_before_creating_a_job_or_pulling(self):
        bin_dir, calls = self.fake_docker()
        with self.assertRaisesRegex(RuntimeError, "Review the tool choice"):
            self.download_with(bin_dir, "job-dl-nochoice", tool=False)
        self.assertFalse(os.path.exists(sj.status_path("job-dl-nochoice")))
        self.assertFalse(os.path.exists(calls), "no tool is run when no choice was supplied")

    def test_an_image_without_hf_fails_without_trying_another_tool(self):
        bin_dir, calls = self.fake_docker(with_hf=False)
        st = self.download_with(bin_dir, "job-dl-nohf")
        self.assertEqual(st["state"], "failed")
        self.assertIn("could not run hf version", st["message"])
        with open(calls) as f: log = f.read()
        self.assertNotIn(" download ", log)
        self.assertNotIn("\npull ", log)

    def test_a_download_with_a_missing_served_file_fails(self):
        bin_dir, _ = self.fake_docker()
        st = self.download_with(bin_dir, "job-dl-missing", files=[{"path":"model.safetensors", "size":5}])
        self.assertEqual(st["state"], "failed")
        self.assertIn("missing model.safetensors", st["message"])

    def test_a_pull_leaves_excluded_files_behind(self):
        if not shutil.which("rsync"):
            self.skipTest("rsync not installed")
        src = tempfile.mkdtemp(dir=TMP_HOME)
        dest = os.path.join(TMP_HOME, "pulled-model")
        make_model(src, {"config.json": b"{}", "model.safetensors": b"m" * 100,
                         "original/consolidated.pth": b"p" * 400, "model.gguf": b"g" * 50})
        files = [{"path": "config.json", "size": 2}, {"path": "model.safetensors", "size": 100},
                 {"path": "original/consolidated.pth", "size": 400}]
        spec = {"src": src, "dest": dest, "password": "pw", "files": files, "exclude": ["original/", "*.gguf"]}
        sj.start("pull", "job-pull-ex", spec)
        st = wait_done("job-pull-ex")
        self.assertEqual(st["state"], "done", st.get("message"))
        self.assertEqual(sorted(os.listdir(dest)), [".sparkbox.json", "config.json", "model.safetensors"])
        self.assertEqual(sum(f["size"] for f in sj.served_files(spec)), 102, "excluded files aren't expected")

    def test_a_serve_job_config_excludes_patterns(self):
        base = tempfile.mkdtemp(dir=TMP_HOME)
        conf = sj.rsyncd_conf(base, "/models/m", base + "/secrets", "192.168.100.0/24", ["original/", "*.pth"])
        self.assertIn("exclude from = %s/exclude" % base, conf)
        with open(os.path.join(base, "exclude")) as f:
            self.assertEqual(f.read(), "original/\n*.pth\n")
        self.assertNotIn("exclude", sj.rsyncd_conf(base, "/m", "s", "a", []))

    def test_rsync_exclude_patterns(self):
        ex = sj.rsync_excluded
        self.assertTrue(ex("original/consolidated.pth", ["original/"]))
        self.assertFalse(ex("original", ["original/"]), "a dir/ pattern skips files of that name")
        self.assertTrue(ex("a/b/model.gguf", ["*.gguf"]))
        self.assertTrue(ex("metal/model.bin", ["/metal"]))
        self.assertFalse(ex("x/metal/model.bin", ["/metal"]), "an anchored pattern only matches at the root")
        self.assertTrue(ex("x/onnx/model.onnx", ["onnx/*.onnx"]))
        self.assertFalse(ex("model.safetensors", ["*.pth", "original/"]))


class FilesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def test_ls_reports_kinds_sizes_modes_and_links(self):
        os.mkdir(os.path.join(self.tmp, "dir"))
        with open(os.path.join(self.tmp, "a.txt"), "w") as f:
            f.write("hello")
        os.symlink("dir", os.path.join(self.tmp, "link"))
        listing = sj.ls(self.tmp)
        by = {e["name"]: e for e in listing["entries"]}
        self.assertEqual(listing["path"], os.path.realpath(self.tmp))
        self.assertEqual(by["a.txt"]["kind"], "file")
        self.assertEqual(by["a.txt"]["size"], 5)
        self.assertTrue(by["a.txt"]["mode"].startswith("-rw"))
        self.assertEqual(by["dir"]["kind"], "dir")
        self.assertEqual(by["link"]["kind"], "link")
        self.assertEqual(by["link"]["target"], "dir")
        self.assertTrue(by["link"]["linkDir"])
        with self.assertRaises(SystemExit):
            sj.ls(os.path.join(self.tmp, "missing"))

    def test_read_returns_checksum_mode_and_owner_and_peeks_at_large_files(self):
        text = os.path.join(self.tmp, "t.conf")
        with open(text, "w") as f:
            f.write("key = välue\n")
        os.chmod(text, 0o640)
        got = sj.read_text(text)
        self.assertEqual(got["content"], "key = välue\n")
        self.assertEqual(got["sha256"], hashlib.sha256("key = välue\n".encode()).hexdigest())
        self.assertEqual(got["mode"], "-rw-r-----")
        self.assertIn(":", got["owner"])
        self.assertFalse(got["large"])
        binary = os.path.join(self.tmp, "b.bin")
        with open(binary, "wb") as f:
            f.write(b"\x00\x01")
        with self.assertRaises(SystemExit):
            sj.read_text(binary)
        big = os.path.join(self.tmp, "big.log")
        with open(big, "w") as f:
            f.write("first line\n" + "x" * sj.TEXT_LIMIT + "\nlast line\n")
        peek = sj.read_text(big)
        self.assertTrue(peek["large"])
        self.assertEqual(peek["content"], "")
        self.assertTrue(peek["head"].startswith("first line"))
        self.assertTrue(peek["tail"].endswith("last line\n"))

    @unittest.skipIf(os.geteuid() == 0, "root reads everything")
    def test_a_file_only_its_owner_can_read_comes_back_as_a_permission_reply(self):
        # Like /etc/netplan/40-cx7.yaml on the real Sparks: mode 600, not ours to read.
        secret = os.path.join(self.tmp, "40-cx7.yaml")
        with open(secret, "w") as f:
            f.write("network: {}\n")
        os.chmod(secret, 0o000)
        try:
            got = sj.read_text(secret)
        finally:
            os.chmod(secret, 0o600)
        self.assertEqual(got["kind"], "permission")
        self.assertEqual(got["path"], os.path.realpath(secret))
        self.assertEqual(got["mode"], "0")
        self.assertEqual(got["owner"], sj.owner_names(os.stat(secret))[0])
        self.assertNotIn("content", got, "nothing of the file comes back")

    def test_size_adds_files_and_folders(self):
        os.mkdir(os.path.join(self.tmp, "d"))
        for name, n in (("d/one", 10), ("two", 5)):
            with open(os.path.join(self.tmp, name), "w") as f:
                f.write("x" * n)
        self.assertEqual(sj.total_size([os.path.join(self.tmp, "d"), os.path.join(self.tmp, "two")]), 15)


class SafetyTest(unittest.TestCase):
    """Atomic saves, backups, the Trash and the protected-path check."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(dir=TMP_HOME)
        self.backups = os.path.join(TMP_HOME, ".sparkbox", "backups")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write(self, name, text, mode=0o644):
        p = os.path.join(self.tmp, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
        os.chmod(p, mode)
        return p

    def commit(self, path, text, expect, **kw):
        src = os.path.join(self.tmp, ".staged")
        with open(src, "wb") as f:
            f.write(text.encode())
        return sj.commit(path, hashlib.sha256(text.encode()).hexdigest(), expect, src=src, **kw)

    def test_protected_paths_are_normalised_first(self):
        for p in ["/", "//etc", "/etc/.", "/usr/../etc", "/usr/bin/python3", "/var/lib", "/opt/nvidia",
                  TMP_HOME, TMP_HOME + "/", os.path.dirname(TMP_HOME)]:
            self.assertTrue(sj.protected(sj.resolve_item(p)), p)
        for p in [os.path.join(TMP_HOME, "notes.txt"), "/usr/local/bin/tool", "/etc/netplan/50-old.yaml"]:
            self.assertFalse(sj.protected(sj.resolve_item(p)), p)
        with self.assertRaises(SystemExit):
            sj.removable("relative/path")

    def test_a_symlink_is_trashed_not_its_target(self):
        target = self.write("real.txt", "keep me")
        link = os.path.join(self.tmp, "link")
        os.symlink(target, link)
        self.assertEqual(sj.resolve_item(link), os.path.join(os.path.realpath(self.tmp), "link"))
        sj.trash_one(link)
        self.assertTrue(os.path.exists(target))
        self.assertFalse(os.path.lexists(link))

    def test_trash_and_undo_round_trip_with_trashinfo(self):
        a = self.write("a b.txt", "one")
        b = self.write("folder/inner.txt", "two")
        first = sj.trash_one(a)
        self.assertFalse(os.path.exists(a))
        self.assertTrue(first["trashed"].startswith(os.path.join(TMP_HOME, ".local", "share", "Trash", "files")))
        info = os.path.join(TMP_HOME, ".local/share/Trash/info", os.path.basename(first["trashed"]) + ".trashinfo")
        with open(info) as f:
            body = f.read()
        self.assertIn("[Trash Info]", body)
        self.assertIn("Path=" + os.path.realpath(self.tmp).replace(" ", "%20") + "/a%20b.txt", body)
        self.assertIn("DeletionDate=", body)
        # A second item with the same name gets its own slot.
        again = self.write("a b.txt", "newer")
        second = sj.trash_one(again)
        self.assertNotEqual(first["trashed"], second["trashed"])
        sj.untrash_one(first["trashed"])
        with open(a) as f:
            self.assertEqual(f.read(), "one")
        self.assertFalse(os.path.exists(info))
        # Undo refuses to overwrite something that came back meanwhile.
        with self.assertRaises(SystemExit):
            sj.untrash_one(second["trashed"])
        folder = sj.trash_one(os.path.dirname(b))
        sj.untrash_one(folder["trashed"])
        self.assertTrue(os.path.exists(b))
        with self.assertRaises(SystemExit):
            sj.untrash_one(a)  # not in a Trash folder

    def test_commit_replaces_atomically_keeps_mode_and_backups(self):
        p = self.write("app.conf", "v1\n", 0o640)
        old_sha = hashlib.sha256(b"v1\n").hexdigest()
        res = self.commit(p, "v2\n", old_sha, backups=self.backups, nonce="n1")
        self.assertFalse(res["conflict"])
        with open(p) as f:
            self.assertEqual(f.read(), "v2\n")
        self.assertEqual(stat_mode(p), 0o640)
        self.assertTrue(res["backup"])
        with open(res["backup"]) as f:
            self.assertEqual(f.read(), "v1\n")
        for i in range(7):
            with open(p, "rb") as f:
                cur = hashlib.sha256(f.read()).hexdigest()
            self.commit(p, "v%d\n" % (i + 3), cur, backups=self.backups)
            time.sleep(0.002)
        kept = sj.list_backups(self.backups, p)
        self.assertEqual(len(kept), sj.BACKUPS_KEPT)
        with open(kept[0]["path"]) as f:
            self.assertEqual(f.read(), "v8\n", "newest first")
        self.assertEqual([n for n in os.listdir(self.tmp) if ".sparkbox-" in n], [], "no temporary files left")

    def test_commit_reports_a_conflict_and_changes_nothing(self):
        p = self.write("hosts", "original\n")
        res = self.commit(p, "mine\n", hashlib.sha256(b"stale").hexdigest())
        self.assertTrue(res["conflict"])
        self.assertEqual(res["sha256"], hashlib.sha256(b"original\n").hexdigest())
        with open(p) as f:
            self.assertEqual(f.read(), "original\n")
        os.unlink(p)
        res = self.commit(p, "mine\n", hashlib.sha256(b"original\n").hexdigest())
        self.assertTrue(res["conflict"])
        self.assertIsNone(res["sha256"], "deleted on disk")

    def test_commit_refuses_truncated_text(self):
        p = self.write("big.yaml", "a: 1\n")
        src = os.path.join(self.tmp, ".staged")
        with open(src, "w") as f:
            f.write("a: 2\nb:")  # cut short
        with self.assertRaises(SystemExit):
            sj.commit(p, hashlib.sha256(b"a: 2\nb: 3\n").hexdigest(), None, src=src)
        with open(p) as f:
            self.assertEqual(f.read(), "a: 1\n")

    def test_commit_follows_symlinks_to_the_real_file(self):
        real = self.write("dotfiles/zshrc", "old")
        link = os.path.join(self.tmp, ".zshrc")
        os.symlink(real, link)
        self.commit(link, "new", hashlib.sha256(b"old").hexdigest())
        self.assertTrue(os.path.islink(link), "the link stays a link")
        with open(real) as f:
            self.assertEqual(f.read(), "new")

    def test_sudoers_files_must_pass_visudo(self):
        self.assertTrue(sj.needs_visudo("/etc/sudoers"))
        self.assertTrue(sj.needs_visudo("/etc/sudoers.d/90-sparkbox"))
        self.assertFalse(sj.needs_visudo("/etc/hosts"))

    def test_summary_counts_bytes_entries_and_tar_size(self):
        self.write("m/config.json", "{}")
        self.write("m/sub/w.bin", "x" * 1000)
        s = sj.summary(self.tmp, ["m", "missing"])
        by = {i["name"]: i for i in s["items"]}
        self.assertEqual(by["m"]["bytes"], 1002)
        self.assertEqual(by["m"]["entries"], 4)  # m, config.json, sub, w.bin
        self.assertEqual(by["m"]["tarBytes"], 4 * 512 + 512 + 1024)
        self.assertFalse(by["missing"]["exists"])
        self.assertGreater(s["free"], 0)


class PlaceTest(unittest.TestCase):
    """Each conflict policy, on a staged copy and on a move within one node."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(dir=TMP_HOME)
        self.dest = os.path.join(self.tmp, "dest")
        self.stage = sj.stage_dir(self.dest, "t1")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def put(self, root, rel, text, age=0):
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
        if age:
            t = time.time() - age
            os.utime(p, (t, t))
        return p

    def read(self, rel):
        with open(os.path.join(self.dest, rel)) as f:
            return f.read()

    def test_policies(self):
        self.put(self.dest, "cfg/a.txt", "old a")
        self.put(self.dest, "cfg/only-old.txt", "old only")
        self.put(self.dest, "merge/a.txt", "old a")
        self.put(self.dest, "merge/keep.txt", "keep")
        self.put(self.dest, "notes.txt", "old notes")
        self.put(self.dest, "skip.txt", "untouched")
        self.put(self.dest, "newer/stale.txt", "old", age=3600)
        self.put(self.dest, "newer/fresh.txt", "dest is newer")
        for rel, text in [("cfg/a.txt", "new a"), ("merge/a.txt", "new a"), ("merge/b.txt", "new b"),
                          ("notes.txt", "new notes"), ("skip.txt", "no"), ("fresh.txt", "brand new")]:
            self.put(self.stage, rel, text)
        self.put(self.stage, "newer/stale.txt", "newer copy")
        self.put(self.stage, "newer/fresh.txt", "older copy", age=7200)
        results = sj.place(self.stage, self.dest, [
            {"name": "cfg", "policy": "replace"},
            {"name": "merge", "policy": "merge"},
            {"name": "notes.txt", "policy": "keepBoth"},
            {"name": "skip.txt", "policy": "skip"},
            {"name": "newer", "policy": "replaceIfNewer"},
            {"name": "fresh.txt"},
        ], "t1")
        by = {r["name"]: r for r in results}
        # Replace swaps the whole folder: the old-only file is gone.
        self.assertEqual(self.read("cfg/a.txt"), "new a")
        self.assertFalse(os.path.exists(os.path.join(self.dest, "cfg/only-old.txt")))
        # Merge keeps what's only at the destination.
        self.assertEqual(self.read("merge/a.txt"), "new a")
        self.assertEqual(self.read("merge/b.txt"), "new b")
        self.assertEqual(self.read("merge/keep.txt"), "keep")
        # Keep both: "notes 2.txt".
        self.assertEqual(by["notes.txt"]["as"], "notes 2.txt")
        self.assertEqual(self.read("notes.txt"), "old notes")
        self.assertEqual(self.read("notes 2.txt"), "new notes")
        self.assertEqual(self.read("skip.txt"), "untouched")
        self.assertEqual(self.read("newer/stale.txt"), "newer copy")
        self.assertEqual(self.read("newer/fresh.txt"), "dest is newer")
        self.assertEqual(self.read("fresh.txt"), "brand new")
        self.assertEqual([n for n in os.listdir(self.dest) if "sparkbox-old" in n], [])
        sj.cleanup(self.dest, "t1")
        self.assertFalse(os.path.exists(self.stage))

    def test_a_name_taken_after_the_check_is_kept_as_both(self):
        self.put(self.dest, "late.txt", "arrived meanwhile")
        self.put(self.stage, "late.txt", "ours")
        res = sj.place(self.stage, self.dest, [{"name": "late.txt"}], "t1")
        self.assertEqual(res[0]["action"], "keptBoth")
        self.assertEqual(self.read("late.txt"), "arrived meanwhile")

    def test_move_within_a_node(self):
        src = os.path.join(self.tmp, "src")
        self.put(src, "folder/x.txt", "x")
        self.put(src, "file.txt", "f")
        self.put(self.dest, "file.txt", "old")
        sj.place(src, self.dest, [{"name": "folder"}, {"name": "file.txt", "policy": "replace"}], "t2", move=True)
        self.assertEqual(self.read("folder/x.txt"), "x")
        self.assertEqual(self.read("file.txt"), "f")
        self.assertEqual(os.listdir(src), [])

    def test_local_copy_reports_progress_and_copies_links_and_modes(self):
        src = os.path.join(self.tmp, "src")
        p = self.put(src, "run/run.sh", "#!/bin/sh\n")
        os.chmod(p, 0o755)
        os.symlink("run.sh", os.path.join(src, "run", "latest"))
        res = sj.local_copy(src, self.stage, ["run"])
        self.assertEqual(res["files"], 3)
        self.assertEqual(stat_mode(os.path.join(self.stage, "run/run.sh")), 0o755)
        self.assertEqual(os.readlink(os.path.join(self.stage, "run/latest")), "run.sh")


def stat_mode(p):
    return os.stat(p).st_mode & 0o7777



class StagingTest(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.realpath(tempfile.mkdtemp(dir=TMP_HOME))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_tree_lists_every_entry_with_kind_and_size(self):
        make_model(os.path.join(self.tmp, "proj"), {"a.txt": b"abc", "sub/b.bin": b"x" * 10})
        os.symlink("a.txt", os.path.join(self.tmp, "proj", "link"))
        t = sj.tree(self.tmp, ["proj", "gone"])
        self.assertEqual(t["proj/a.txt"], ["f", 3])
        self.assertEqual(t["proj/sub/b.bin"], ["f", 10])
        self.assertEqual(t["proj/sub"][0], "d")
        self.assertEqual(t["proj/link"], ["l", 0])
        self.assertNotIn("gone", t)

    def test_sweep_removes_stale_and_orphaned_staging_only(self):
        def stage(nonce, age):
            p = os.path.join(self.tmp, ".sparkbox-incoming-" + nonce)
            make_model(p, {"f": b"x" * 100})
            t = time.time() - age
            for x in (os.path.join(p, "f"), p):
                os.utime(x, (t, t))
            return p
        old = stage("1-aaaa", 2 * 86400)
        orphan = stage("2-bbbb", 3600)
        mine = stage("3-cccc", 3600)
        busy = stage("4-dddd", 5)
        make_model(os.path.join(self.tmp, "keep-me"), {"g": b"y"})
        removed = sj.sweep(self.tmp, {"3-cccc"})
        self.assertEqual(sorted(r["path"] for r in removed), sorted([old, orphan]))
        self.assertTrue(all(r["removed"] and r["bytes"] == 100 for r in removed))
        self.assertTrue(os.path.isdir(mine) and os.path.isdir(busy), "a running copy and a fresh one stay")
        self.assertTrue(os.path.isdir(os.path.join(self.tmp, "keep-me")))

    def test_cleanup_says_when_something_is_left(self):
        p = sj.stage_dir(self.tmp, "5-eeee")
        make_model(p, {"f": b"z"})
        self.assertEqual(sj.cleanup(self.tmp, "5-eeee"), {})
        self.assertFalse(os.path.exists(p))


class TestFolderTest(unittest.TestCase):
    """A path written under ~/sparkbox-test (what the fence's files-test class admits) never
    reaches past it through a link."""

    def setUp(self):
        self.test = os.path.join(TMP_HOME, "sparkbox-test")
        self.models = os.path.join(TMP_HOME, "models")
        shutil.rmtree(self.test, ignore_errors=True)
        shutil.rmtree(self.models, ignore_errors=True)
        make_model(os.path.join(self.models, "glm53"), {"config.json": b"{}", "model.safetensors": b"w" * 64})
        os.makedirs(self.test)
        os.symlink(TMP_HOME, os.path.join(self.test, "link"))
        self.glm = os.path.join(self.test, "link", "models", "glm53")

    def tearDown(self):
        if os.path.islink(self.test):
            os.unlink(self.test)
        shutil.rmtree(self.test, ignore_errors=True)
        shutil.rmtree(self.models, ignore_errors=True)

    def refused(self, fn, *args):
        with self.assertRaises(SystemExit) as e:
            fn(*args)
        self.assertIn("sparkbox-test", str(e.exception))
        self.assertTrue(os.path.isfile(os.path.join(self.models, "glm53", "config.json")), "the model is still there")
        return str(e.exception)

    def test_every_write_verb_refuses_a_link_out_of_the_test_folder(self):
        why = self.refused(sj.remove, self.glm)
        self.assertIn("leads outside ~/sparkbox-test", why)
        self.refused(sj.remove, "~/sparkbox-test/link/models/glm53")
        self.refused(sj.trash_one, self.glm)
        self.refused(sj.delete, os.path.join(self.test, "link", "models"), self.glm)
        self.refused(sj.commit, os.path.join(self.glm, "config.json"), hashlib.sha256(b"x").hexdigest(), "-")
        self.refused(sj.place, os.path.join(self.test, "staged"), os.path.join(self.test, "link", "models"), [{"name": "glm53", "policy": "replace"}], "t1")
        self.refused(sj.stage_dir, os.path.join(self.test, "link", "models"), "t1")
        self.refused(sj.cleanup, os.path.join(self.test, "link", "models"), "t1")
        self.refused(sj.sweep, os.path.join(self.test, "link", "models"), set())
        self.refused(sj.local_copy, os.path.join(self.test, "link", "models"), os.path.join(self.test, "copy"), ["glm53"])
        # Backups for a save in the test folder stay in it too.
        a = os.path.join(self.test, "a.txt")
        with open(a, "w") as f:
            f.write("one")
        self.refused(sj.commit, a, hashlib.sha256(b"x").hexdigest(), "-", None, os.path.join(self.test, "link", "models"))

    def test_the_test_folder_itself_a_link_is_refused(self):
        shutil.rmtree(self.test)
        os.symlink(self.models, self.test)
        why = self.refused(sj.remove, os.path.join(self.test, "glm53"))
        self.assertIn("~/sparkbox-test is a link", why)

    def test_inside_the_folder_everything_still_works(self):
        # The link itself goes, not what it points at; plain files and folders as before.
        sj.remove(os.path.join(self.test, "link"))
        self.assertFalse(os.path.lexists(os.path.join(self.test, "link")))
        self.assertTrue(os.path.isdir(TMP_HOME))
        os.makedirs(os.path.join(self.test, "old"))
        self.assertEqual(sj.remove("~/sparkbox-test/old")["path"], os.path.join(os.path.realpath(TMP_HOME), "sparkbox-test", "old"))
        # A link inside that stays inside is fine.
        os.makedirs(os.path.join(self.test, "real"))
        os.symlink(os.path.join(self.test, "real"), os.path.join(self.test, "alias"))
        with open(os.path.join(self.test, "real", "x.txt"), "w") as f:
            f.write("x")
        sj.remove(os.path.join(self.test, "alias", "x.txt"))
        self.assertFalse(os.path.exists(os.path.join(self.test, "real", "x.txt")))
        # Paths outside the test folder are the fence's call, as before.
        sj.remove(os.path.join(self.models, "glm53", "model.safetensors"))

    def test_place_takes_only_plain_item_names(self):
        stage = os.path.join(self.test, "staged")
        os.makedirs(os.path.join(stage, "ok"))
        for bad in ["../models", "..", ".", "", "a/b", "../../x"]:
            with self.assertRaises(SystemExit, msg=bad):
                sj.place(stage, os.path.join(self.test, "dest"), [{"name": bad, "policy": "replace"}], "t1")
        with self.assertRaises(SystemExit):
            sj.place(stage, os.path.join(self.test, "dest"), [{"name": "ok"}], "../x")
        self.assertTrue(os.path.isdir(os.path.join(self.models, "glm53")))
        self.assertEqual(sj.place(stage, os.path.join(self.test, "dest"), [{"name": "ok"}], "t1")[0]["action"], "copied")

    def test_the_command_line_refuses_it_too(self):
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        r = subprocess.run([sys.executable, "-I", here, "rm", self.glm], env=dict(os.environ, HOME=TMP_HOME), capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("leads outside ~/sparkbox-test", r.stderr)
        self.assertTrue(os.path.isdir(os.path.join(self.models, "glm53")))


class UntrashTest(unittest.TestCase):
    """Trash and Undo under ~/sparkbox-test (what the fence's files-test class admits) stay
    inside it: the item goes to the folder's own .sparkbox-trash, and comes back only inside the
    folder, whatever its trashinfo says."""

    def setUp(self):
        self.test = os.path.join(TMP_HOME, "sparkbox-test")
        self.models = os.path.join(TMP_HOME, "models")
        for d in (self.test, self.models, os.path.join(TMP_HOME, ".local")):
            shutil.rmtree(d, ignore_errors=True)
        os.makedirs(self.test)
        os.makedirs(self.models)
        self.trash = os.path.join(os.path.realpath(TMP_HOME), "sparkbox-test", ".sparkbox-trash")

    def tearDown(self):
        for d in (self.test, self.models, os.path.join(TMP_HOME, ".local")):
            shutil.rmtree(d, ignore_errors=True)

    def write(self, path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)

    def planted(self, name, original):
        """An item in the folder's Trash whose trashinfo says it came from `original`."""
        self.write(os.path.join(self.trash, "files", name), "planted")
        self.write(os.path.join(self.trash, "info", name + ".trashinfo"), "[Trash Info]\nPath=%s\nDeletionDate=2026-10-30T10:00:00\n" % original)
        return os.path.join(self.test, ".sparkbox-trash", "files", name)

    def test_trash_and_undo_in_the_test_folder_stay_in_it(self):
        a = os.path.join(self.test, "sub", "a b.txt")
        self.write(a, "one")
        got = sj.trash_one(a)
        self.assertEqual(got["trashed"], os.path.join(self.trash, "files", "a b.txt"))
        self.assertFalse(os.path.exists(os.path.join(TMP_HOME, ".local", "share", "Trash")), "nothing lands outside the folder")
        sj.untrash_one(got["trashed"])
        with open(a) as f:
            self.assertEqual(f.read(), "one")
        # Written as ~/…, the way the exit sees it, too.
        got = sj.trash_one("~/sparkbox-test/sub")
        sj.untrash_one("~/sparkbox-test/.sparkbox-trash/files/sub")
        self.assertTrue(os.path.isfile(a))

    def test_the_folder_itself_and_its_trash_stay_out_of_the_home_trash(self):
        # ~/sparkbox-test has no Trash of its own to go into, nor does what's already in it: both
        # are refused, so nothing from the folder ever reaches ~/.local/share/Trash.
        a = os.path.join(self.test, "a.txt")
        self.write(a, "one")
        for written in (self.test, self.test + "/", "~/sparkbox-test", os.path.join(os.path.realpath(TMP_HOME), "sparkbox-test")):
            with self.assertRaises(SystemExit, msg=written) as e:
                sj.trash_one(written)
            self.assertEqual(str(e.exception), "SparkBox won't put ~/sparkbox-test itself in the Trash; delete what's in it instead.")
        trashed = sj.trash_one(a)["trashed"]
        for written in (trashed, "~/sparkbox-test/.sparkbox-trash/files/a.txt", os.path.join(self.test, ".sparkbox-trash"), os.path.join(self.test, ".sparkbox-trash", "info")):
            with self.assertRaises(SystemExit, msg=written) as e:
                sj.trash_one(written)
            self.assertTrue(str(e.exception).endswith(" is already in the Trash; Delete removes it for good."), str(e.exception))
        self.assertEqual(str(e.exception), "%s is already in the Trash; Delete removes it for good." % os.path.join(self.trash, "info"))
        self.assertTrue(os.path.isdir(self.test) and os.path.isfile(trashed), "nothing moved")
        self.assertFalse(os.path.exists(os.path.join(TMP_HOME, ".local", "share", "Trash")), "nothing lands in the home Trash")
        # The command line says the same.
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        r = subprocess.run([sys.executable, "-I", here, "trash", "~/sparkbox-test"], env=dict(os.environ, HOME=TMP_HOME), capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("won't put ~/sparkbox-test itself in the Trash", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(TMP_HOME, ".local", "share", "Trash")))

    def test_a_trashed_symlink_in_the_folder_comes_back_as_the_link(self):
        # A symlink inside ~/sparkbox-test is trashed and undone as the link itself; its target
        # (here ~/models/glm, outside the folder) is never followed or touched.
        target = os.path.join(self.models, "glm")
        os.makedirs(target)
        with open(os.path.join(target, "config.json"), "w") as f:
            f.write("{}")
        link = os.path.join(self.test, "link")
        os.symlink(target, link)
        got = sj.trash_one(link)
        self.assertFalse(os.path.lexists(link))
        self.assertEqual(got["trashed"], os.path.join(self.trash, "files", "link"))
        self.assertTrue(os.path.islink(got["trashed"]), "the link moved, not its target")
        sj.untrash_one(got["trashed"])
        self.assertTrue(os.path.islink(link), "the link itself is back")
        self.assertEqual(os.readlink(link), target)
        self.assertTrue(os.path.isfile(os.path.join(target, "config.json")), "the target is untouched")

    def test_an_item_whose_trashinfo_points_outside_the_folder_stays_in_the_trash(self):
        for name, original in [("x", os.path.join(self.models, "x")), ("y", "~/models/y"), ("z", os.path.join(self.test, "..", "models", "z")),
                               ("w", "relative/w"), ("v", os.path.join(self.test, "link", "v"))]:
            if name == "v":
                os.symlink(self.models, os.path.join(self.test, "link"))
            trashed = self.planted(name, original)
            with self.assertRaises(SystemExit, msg=name) as e:
                sj.untrash_one(trashed)
            self.assertIn("sparkbox-test", str(e.exception))
            self.assertTrue(os.path.exists(os.path.join(self.trash, "files", name)), name)
        self.assertEqual(os.listdir(self.models), [], "nothing was put in ~/models")
        # Back into the folder is fine.
        trashed = self.planted("ok", os.path.join(self.test, "back", "ok"))
        self.assertEqual(sj.untrash_one(trashed)["path"], os.path.join(os.path.realpath(self.test), "back", "ok"))
        with open(os.path.join(self.test, "back", "ok")) as f:
            self.assertEqual(f.read(), "planted")

    def test_a_test_folder_trash_that_leads_elsewhere_is_refused(self):
        # The folder's Trash a link to the home Trash, whose items go back anywhere.
        home_trash = os.path.join(TMP_HOME, ".local", "share", "Trash")
        self.write(os.path.join(home_trash, "files", "m"), "model")
        self.write(os.path.join(home_trash, "info", "m.trashinfo"), "[Trash Info]\nPath=%s\n" % os.path.join(self.models, "m"))
        os.symlink(home_trash, os.path.join(self.test, ".sparkbox-trash"))
        with self.assertRaises(SystemExit) as e:
            sj.untrash_one(os.path.join(self.test, ".sparkbox-trash", "files", "m"))
        self.assertIn("leads outside ~/sparkbox-test", str(e.exception))
        self.assertFalse(os.path.exists(os.path.join(self.models, "m")))

    def test_the_home_trash_still_puts_things_back_where_they_were(self):
        # Outside the folder, Undo is the fence's call (the exit calls it `other`), as before.
        m = os.path.join(self.models, "notes.txt")
        self.write(m, "keep")
        got = sj.trash_one(m)
        self.assertTrue(got["trashed"].startswith(os.path.join(TMP_HOME, ".local", "share", "Trash", "files")))
        sj.untrash_one(got["trashed"])
        self.assertTrue(os.path.isfile(m))

    def test_the_command_line_refuses_it_too(self):
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        trashed = self.planted("cli", os.path.join(self.models, "cli"))
        r = subprocess.run([sys.executable, "-I", here, "untrash", trashed], env=dict(os.environ, HOME=TMP_HOME), capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("outside ~/sparkbox-test", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.models, "cli")))


class NewFileTest(unittest.TestCase):
    """`commit <path> --sha256 <of nothing> --expect absent` (Files › New file, C8): a new empty
    file where nothing is; anything already there, a link too, is a conflict and stays as it was."""

    EMPTY = hashlib.sha256(b"").hexdigest()

    def setUp(self):
        self.test = os.path.join(TMP_HOME, "sparkbox-test")
        shutil.rmtree(self.test, ignore_errors=True)
        os.makedirs(self.test)

    def tearDown(self):
        shutil.rmtree(self.test, ignore_errors=True)

    def run_cli(self, path):
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        return subprocess.run([sys.executable, "-I", here, "commit", path, "--sha256", self.EMPTY, "--expect", "absent"],
                              env=dict(os.environ, HOME=TMP_HOME), input=b"", capture_output=True)

    def cli(self, path):
        r = self.run_cli(path)
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads(r.stdout.decode().strip().splitlines()[-1])

    def test_a_missing_folder_is_a_plain_error_and_makes_nothing(self):
        # The folder as it was given, ~ and all; a non-zero exit, one JSON line, no traceback.
        for path, folder in (("~/sparkbox-test/x/notes.txt", "~/sparkbox-test/x"),
                             (os.path.join(self.test, "x", "y", "notes.txt"), os.path.join(self.test, "x", "y"))):
            r = self.run_cli(path)
            self.assertNotEqual(r.returncode, 0)
            self.assertEqual(json.loads(r.stdout.decode()), {"error": "The folder %s doesn't exist." % folder})
            self.assertEqual(r.stderr, b"", "no traceback")
        self.assertEqual(os.listdir(self.test), [], "nothing made")
        # A file where the folder should be is no folder either.
        with open(os.path.join(self.test, "x"), "w") as f:
            f.write("a file")
        r = self.run_cli("~/sparkbox-test/x/notes.txt")
        self.assertEqual((r.returncode, json.loads(r.stdout.decode())), (1, {"error": "The folder ~/sparkbox-test/x doesn't exist."}))
        with self.assertRaises(sj.NoFolder):
            sj.commit(os.path.join(self.test, "nope", "a.txt"), self.EMPTY, "absent")

    def test_it_makes_the_file_once_and_conflicts_the_second_time(self):
        self.assertEqual(self.EMPTY, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        made = self.cli("~/sparkbox-test/notes.txt")
        path = os.path.join(os.path.realpath(TMP_HOME), "sparkbox-test", "notes.txt")
        self.assertEqual((made["conflict"], made["path"], made["size"]), (False, path, 0))
        mask = os.umask(0)
        os.umask(mask)
        self.assertEqual(stat_mode(path), 0o666 & ~mask)
        self.assertEqual([f for f in os.listdir(self.test)], ["notes.txt"], "no temporary file left")
        again = self.cli("~/sparkbox-test/notes.txt")
        self.assertEqual((again["conflict"], again["path"]), (True, path))

    def test_a_file_or_a_link_already_there_is_a_conflict_and_unchanged(self):
        f = os.path.join(self.test, "kept.txt")
        with open(f, "w") as out:
            out.write("keep")
        got = sj.commit(f, self.EMPTY, "absent")
        self.assertTrue(got["conflict"])
        with open(f) as kept:
            self.assertEqual(kept.read(), "keep")
        # A link to a file, and a link that leads nowhere (inside the folder): neither is followed.
        os.symlink(f, os.path.join(self.test, "to-kept"))
        os.symlink(os.path.join(self.test, "missing"), os.path.join(self.test, "dangling"))
        for name in ("to-kept", "dangling"):
            link = os.path.join(self.test, name)
            self.assertTrue(self.cli(link)["conflict"], name)
            self.assertTrue(os.path.islink(link), name)
        self.assertFalse(os.path.lexists(os.path.join(self.test, "missing")), "the dangling link's target wasn't made")
        with open(f) as kept:
            self.assertEqual(kept.read(), "keep")
        self.assertEqual(sorted(os.listdir(self.test)), ["dangling", "kept.txt", "to-kept"], "no temporary file left")


class CheckImageTest(unittest.TestCase):
    """The full readiness check reads vLLM's version in a test container of SparkBox's: named
    sparkbox-checkimage-*, labelled sparkbox.test=1 and sparkbox.instance=<i>, removed after."""

    def setUp(self):
        self.bin = tempfile.mkdtemp(dir=TMP_HOME)
        self.log = os.path.join(self.bin, "docker.log")
        self.mode = os.path.join(self.bin, "mode")
        with open(os.path.join(self.bin, "docker"), "w") as f:
            # A GB10's docker as sparkjob.py calls it: notes each call, has every image, and
            # prints the version from `run`, or nvidia-smi's line from a run with the GPU (or hangs,
            # when told to: `run` alone, or everything; or has no image, or sees no GPU).
            f.write('#!/bin/sh\necho "$*" >> LOG\nmode=$(cat MODE 2>/dev/null)\n[ "$mode" = wedged ] && exec sleep 30\n'
                    'case "$1" in\n'
                    '  image) [ "$mode" = noimage ] && exit 1; echo \'[{"Id":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","Config":{"Env":[]}}]\' ;;\n'
                    '  run) [ "$mode" = hang ] && exec sleep 30\n'
                    '    case "$*" in\n'
                    '      *--gpus*) [ "$mode" = error ] && { echo "could not select device driver" >&2; exit 125; }\n'
                    '        [ "$mode" = nogpu ] && { echo "No devices were found"; exit 0; }\n'
                    '        [ "$mode" = empty ] && exit 0\n'
                    '        echo "GPU 0: NVIDIA GB10 (UUID: GPU-5e1f)"; echo "MIG 1g: none" ;;\n'
                    '      *) echo 0.11.0 ;;\n'
                    '    esac ;;\n'
                    'esac\nexit 0\n'
                    .replace("LOG", self.log).replace("MODE", self.mode))
        os.chmod(os.path.join(self.bin, "docker"), 0o755)
        self.path = os.environ.get("PATH", "")
        os.environ["PATH"] = self.bin + os.pathsep + self.path
        self.root = tempfile.mkdtemp(dir=TMP_HOME)

    def tearDown(self):
        os.environ["PATH"] = self.path
        shutil.rmtree(self.bin, ignore_errors=True)
        shutil.rmtree(self.root, ignore_errors=True)

    def calls(self):
        with open(self.log) as f:
            return [l.rstrip("\n") for l in f]

    def test_the_version_probe_runs_in_a_labelled_named_test_container(self):
        got = sj.check(self.root, "vllm:img", "vllm:img-ray", instance="dev")
        self.assertEqual(got["vllmVersion"], "0.11.0")
        run = [c for c in self.calls() if c.startswith("run ")]
        self.assertEqual(len(run), 3)
        words = run[0].split(" ")
        self.assertEqual(words[:3], ["run", "--rm", "--name"])
        self.assertRegex(words[3], r"^sparkbox-checkimage-[0-9a-f]{8}$")
        self.assertEqual(" ".join(words[4:]), "--label sparkbox.test=1 --label sparkbox.instance=dev --entrypoint python3 sha256:" + "a" * 64 + " -c import vllm; print(vllm.__version__)")
        self.assertFalse(any("vllm:img-ray" in c for c in self.calls()))
        # Quick: no container at all.
        before = len(self.calls())
        quick = sj.check(self.root, "vllm:img", "vllm:img-ray", quick=True, instance="dev")
        self.assertEqual((quick["vllmVersion"], quick["gpuInContainer"]), ("", None))
        self.assertFalse([c for c in self.calls()[before:] if c.startswith("run ")])

    def test_the_full_check_asks_a_test_container_for_the_gpu(self):
        got = sj.check(self.root, "vllm:img", "vllm:img-ray", instance="dev")
        self.assertEqual(got["gpuInContainer"], "GPU 0: NVIDIA GB10 (UUID: GPU-5e1f)")
        run = [c for c in self.calls() if c.startswith("run ")][1]
        words = run.split(" ")
        self.assertEqual(words[:6], ["run", "--rm", "--gpus", "all", "--name", words[5]])
        self.assertRegex(words[5], r"^sparkbox-checkimage-[0-9a-f]{8}$")
        self.assertEqual(" ".join(words[6:]), "--label sparkbox.test=1 --label sparkbox.instance=dev --entrypoint nvidia-smi sha256:" + "a" * 64 + " -L")

    def test_no_image_means_no_gpu_answer_and_no_container(self):
        with open(self.mode, "w") as f:
            f.write("noimage")
        got = sj.check(self.root, "vllm:img", "vllm:img-ray", instance="dev")
        self.assertIsNone(got["gpuInContainer"])
        self.assertFalse([c for c in self.calls() if c.startswith("run ")])

    def test_a_container_that_cant_see_the_gpu_says_so(self):
        with open(self.mode, "w") as f:
            f.write("nogpu")
        self.assertEqual(sj.check(self.root, "vllm:img", "vllm:img-ray", instance="dev")["gpuInContainer"], "")

    def test_a_gpu_probe_past_its_limit_is_removed_by_name(self):
        with open(self.mode, "w") as f:
            f.write("hang")
        limit, sj.GPU_CHECK_S = sj.GPU_CHECK_S, 1
        try:
            self.assertIsNone(sj.gpu_in_container("vllm:img", "dev"), "not checked, which isn't no GPU")
        finally:
            sj.GPU_CHECK_S = limit
        name = next(w for w in self.calls()[0].split(" ") if w.startswith("sparkbox-checkimage-"))
        self.assertIn("rm -f " + name, self.calls())

    def test_a_failed_gpu_probe_does_not_claim_no_gpu(self):
        with open(self.mode, "w") as f:
            f.write("error")
        self.assertIsNone(sj.check(self.root, "vllm:img", "vllm:img-ray", instance="dev")["gpuInContainer"])

    def test_a_successful_empty_gpu_list_means_no_gpu(self):
        with open(self.mode, "w") as f:
            f.write("empty")
        self.assertEqual(sj.gpu_in_container("vllm:img", "dev"), "")

    def test_with_no_docker_the_gpu_wasnt_checked(self):
        os.environ["PATH"] = self.root
        self.assertIsNone(sj.gpu_in_container("vllm:img", "dev"))

    def test_a_probe_past_its_time_limit_is_removed_by_name(self):
        with open(self.mode, "w") as f:
            f.write("hang")
        limit, sj.CHECK_TIMEOUT_S = sj.CHECK_TIMEOUT_S, 1
        try:
            got = sj.check(self.root, "vllm:img", "vllm:img-ray", instance="release")
        finally:
            sj.CHECK_TIMEOUT_S = limit
        self.assertEqual(got["vllmVersion"], "")
        run = [c for c in self.calls() if c.startswith("run ")][0]
        name = run.split(" ")[3]
        self.assertIn("rm -f " + name, self.calls())

    def test_looking_for_hf_runs_in_a_labelled_named_test_container(self):
        self.assertTrue(sj.has_hf("vllm:img", "dev"))
        run = [c for c in self.calls() if c.startswith("run ")]
        self.assertEqual(len(run), 1)
        words = run[0].split(" ")
        self.assertEqual(words[:3], ["run", "--rm", "--name"])
        self.assertRegex(words[3], r"^sparkbox-checkimage-[0-9a-f]{8}$")
        self.assertEqual(" ".join(words[4:]), "--label sparkbox.test=1 --label sparkbox.instance=dev --entrypoint hf vllm:img --help")

    def test_looking_for_hf_past_its_time_limit_is_removed_by_name(self):
        with open(self.mode, "w") as f:
            f.write("hang")
        limit, sj.CHECK_TIMEOUT_S = sj.CHECK_TIMEOUT_S, 1
        try:
            self.assertFalse(sj.has_hf("vllm:img", "release"))
        finally:
            sj.CHECK_TIMEOUT_S = limit
        run = [c for c in self.calls() if c.startswith("run ")][0]
        self.assertIn("rm -f " + run.split(" ")[3], self.calls())

    def test_a_docker_that_never_answers_ends_the_search_for_an_image(self):
        # The daemon hangs on every verb, removing the container too: the job fails and says why.
        with open(self.mode, "w") as f:
            f.write("wedged")
        limits = sj.CHECK_TIMEOUT_S, sj.DOCKER_STOP_S
        sj.CHECK_TIMEOUT_S, sj.DOCKER_STOP_S = 0.3, 0.3
        started = time.time()
        try:
            self.assertFalse(sj.has_hf("vllm:img", "dev"))
            self.assertTrue([c for c in self.calls() if c.startswith("rm -f ")])
            tools = sj.download_tools()
            self.assertFalse(any(t["kind"] == "image" for t in tools["options"]))
            self.assertTrue(any("No image will be pulled" in n for n in tools["notes"]))
        finally:
            sj.CHECK_TIMEOUT_S, sj.DOCKER_STOP_S = limits
        self.assertLess(time.time() - started, 15, "the fake docker sleeps 30 s on every verb")

    def test_the_arguments(self):
        self.assertEqual(sj.check_args(["r", "i"]), ("r", "i", "", False, ""))
        self.assertEqual(sj.check_args(["r", "i", "quick", "--instance", "dev"]), ("r", "i", "", True, "dev"))
        self.assertEqual(sj.check_args(["r", "i", "ri"]), ("r", "i", "ri", False, ""))
        self.assertEqual(sj.check_args(["r", "i", "ri", "quick"]), ("r", "i", "ri", True, ""))
        self.assertEqual(sj.check_args(["r", "i", "ri", "--instance", "dev"]), ("r", "i", "ri", False, "dev"))
        self.assertEqual(sj.check_args(["r", "i", "ri", "quick", "--instance", "release"]), ("r", "i", "ri", True, "release"))
        for bad in [["r"], ["r", "i", "ri", "--instance"], ["r", "i", "ri", "--instance", "DEV"], ["r", "i", "ri", "--instance", "dev;x"],
                    ["r", "i", "ri", "--instance", "--rm"], ["r", "i", "ri", "--instance", "dev", "extra"], ["r", "i", "ri", "extra"],
                    ["r", "i", "ri", "--instance", "dev", "quick"]]:
            with self.assertRaises(SystemExit, msg=bad):
                sj.check_args(bad)

    def test_the_command_line(self):
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        env = dict(os.environ, HOME=TMP_HOME)
        r = subprocess.run([sys.executable, "-I", here, "check", self.root, "vllm:img", "vllm:img-ray", "--instance", "dev"], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout)["vllmVersion"], "0.11.0")
        self.assertTrue(any("--label sparkbox.instance=dev" in c for c in self.calls()))
        r = subprocess.run([sys.executable, "-I", here, "check", self.root, "vllm:img", "vllm:img-ray", "--instance", "x y"], env=env, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)


class LeaseTest(unittest.TestCase):
    """The controller lease: one SparkBox (release or dev) changes a node at a time."""

    def setUp(self):
        for p in (sj.LEASE, sj.LEASE_LOCK):
            if os.path.exists(p):
                os.remove(p)

    def held(self):
        with open(sj.LEASE) as f:
            return json.load(f)

    def test_claim_renew_and_the_other_instance_is_refused(self):
        self.assertEqual(sj.lease_claim("release", "p1-r1", "Studio", 45, clock=lambda: 1000.0), {"held": True})
        self.assertEqual(self.held(), {"instance": "release", "tag": "p1-r1", "mac": "Studio", "heartbeat": 1000.0})
        # Renewing your own lease moves the heartbeat.
        self.assertEqual(sj.lease_claim("release", "p1-r1", "Studio", 45, clock=lambda: 1010.0), {"held": True})
        self.assertEqual(self.held()["heartbeat"], 1010.0)
        # The other instance, 20 s later: refused, told who holds it and since when.
        got = sj.lease_claim("dev", "g0-4-gabc", "Studio", 45, clock=lambda: 1030.0)
        self.assertEqual(got, {"held": False, "by": {"instance": "release", "tag": "p1-r1", "mac": "Studio"}, "ageS": 20})
        self.assertEqual(self.held()["instance"], "release", "nothing was written")

    def test_a_stale_heartbeat_is_taken_over(self):
        sj.lease_claim("release", "p1-r1", "Studio", 45, clock=lambda: 1000.0)
        self.assertEqual(sj.lease_claim("dev", "t", "Studio", 45, clock=lambda: 1045.0), {"held": True}, "45 s old is stale")
        self.assertEqual(self.held(), {"instance": "dev", "tag": "t", "mac": "Studio", "heartbeat": 1045.0})
        # A heartbeat from the future (the clock moved back) doesn't hold the node forever.
        sj.lease_claim("dev", "t", "Studio", 45, clock=lambda: 5000.0)
        self.assertEqual(sj.lease_claim("release", "r", "Studio", 45, clock=lambda: 4000.0), {"held": True})

    def test_release_removes_only_your_own_lease(self):
        sj.lease_claim("release", "r", "Studio", 45, clock=lambda: 1000.0)
        self.assertEqual(sj.lease_release("dev"), {"released": False})
        self.assertTrue(os.path.exists(sj.LEASE), "the other instance's lease stays")
        self.assertEqual(sj.lease_release("release"), {"released": True})
        self.assertFalse(os.path.exists(sj.LEASE))
        self.assertEqual(sj.lease_release("release"), {"released": False}, "nothing to release")
        self.assertEqual(sj.lease_claim("dev", "t", "Studio", 45, clock=lambda: 1001.0), {"held": True}, "free again at once")

    def test_an_unreadable_lease_file_is_replaced_atomically(self):
        os.makedirs(os.path.dirname(sj.LEASE), exist_ok=True)
        with open(sj.LEASE, "w") as f:
            f.write("{half")
        self.assertEqual(sj.lease_claim("dev", "t", "Studio", 45, clock=lambda: 1.0), {"held": True})
        self.assertEqual(self.held()["instance"], "dev")
        leftovers = [n for n in os.listdir(os.path.dirname(sj.LEASE)) if n.startswith("controller.json.")]
        self.assertEqual(leftovers, [], "no temporary file is left")

    def test_claims_wait_for_the_lock(self):
        # Another claimant holds the lock: this claim waits for it, then decides on what it wrote.
        import threading
        os.makedirs(os.path.dirname(sj.LEASE_LOCK), exist_ok=True)
        lock = open(sj.LEASE_LOCK, "a")
        fcntl = __import__("fcntl")
        fcntl.flock(lock, fcntl.LOCK_EX)
        result = {}
        t = threading.Thread(target=lambda: result.update(sj.lease_claim("dev", "t", "Studio", 45, clock=lambda: 1001.0)))
        t.start()
        time.sleep(0.3)
        self.assertFalse(result, "the claim is waiting")
        with open(sj.LEASE, "w") as f:
            json.dump({"instance": "release", "tag": "r", "mac": "Studio", "heartbeat": 1000.0}, f)
        fcntl.flock(lock, fcntl.LOCK_UN)
        lock.close()
        t.join(5)
        self.assertEqual(result, {"held": False, "by": {"instance": "release", "tag": "r", "mac": "Studio"}, "ageS": 1})

    def test_the_command_line(self):
        import subprocess
        here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
        env = dict(os.environ, HOME=TMP_HOME)
        run = lambda *a: json.loads(subprocess.run([sys.executable, "-I", here, *a], env=env, capture_output=True, text=True, check=True).stdout)
        self.assertEqual(run("lease", "claim", "dev", "g0", "Joe's Mac", "45"), {"held": True})
        self.assertEqual(self.held()["mac"], "Joe's Mac")
        self.assertFalse(run("lease", "claim", "release", "r", "Studio", "45")["held"])
        self.assertEqual(run("lease", "release", "dev"), {"released": True})


HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sparkjob.py")
TOKEN = "hf_test_0123456789abcdefTOKEN"


def sparkjob(*args, stdin=None, path=None, env=None):
    """The real command line: `start` returns and its job runs on detached, as on a node."""
    extra = dict(env or {})
    if path:
        extra["PATH"] = path + os.pathsep + os.environ["PATH"]
    r = subprocess.run([sys.executable, "-I", HERE, *args], env=dict(os.environ, HOME=TMP_HOME, **extra),
                       input=None if stdin is None else json.dumps(stdin), capture_output=True, text=True)
    if r.returncode != 0:
        raise AssertionError(r.stderr)
    return json.loads(r.stdout)


def wait_for(what, cond, timeout=10):
    for _ in range(int(timeout * 20)):
        if cond():
            return
        time.sleep(0.05)
    raise AssertionError("never happened: " + what)


def pid_gone(pid):
    try:
        os.kill(pid, 0)
        return False
    except OSError:
        return True


def end_job(pid):
    """A job a test started in this process and didn't see out, and the group it made. Only while
    it's this process's child and not waited for, so its pid can't be anyone else's."""
    try:
        if os.waitpid(pid, os.WNOHANG)[0] == 0:
            os.killpg(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
    except OSError:
        pass


def stand_in(bin_dir, name, body):
    path = os.path.join(bin_dir, name)
    with open(path, "w") as f:
        f.write(body)
    os.chmod(path, 0o755)


class Docker:
    """A stand-in docker that behaves as Docker does towards a signal. `run` records its argv, its
    environment and what Docker would keep of the container (-e values, -v mounts, labels), then
    stays running until told to finish. The CLI ignores SIGTERM, as hf does as a container's PID 1
    (scripts/staging/token-proof.sh step 5), and the container is the daemon's: it outlives a kill
    of the CLI, and only `docker stop` ends it. `ps` answers from the record, by label; `stop` is
    logged and ends it; `inspect` answers from the record, as the real one would."""

    SCRIPT = """#!/usr/bin/env python3
import json, os, shutil, signal, sys, time
state = %r
a = sys.argv[1:]
record = os.path.join(state, "run.json")
stopped = os.path.join(state, "stopped")
with open(os.path.join(state, "calls"), "a") as f:
    f.write(json.dumps(a) + "\\n")
if os.path.exists(os.path.join(state, "hang-" + a[0])):
    time.sleep(30)
if os.path.exists(os.path.join(state, "fail-" + a[0])):
    print("Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?", file=sys.stderr)
    sys.exit(1)
def container():
    return json.load(open(record)) if os.path.exists(record) else None
def labels(run):
    return [run["argv"][i + 1] for i, x in enumerate(run["argv"]) if x == "--label"]
if a[:2] == ["image", "inspect"]:
    print("sha256:" + "b" * 64)
    sys.exit(0)
if a[:2] == ["image", "ls"]:
    print(json.dumps({"ID":"sha256:" + "b" * 64, "Repository":"vllm", "Tag":"test"}))
    sys.exit(0)
if a[:1] == ["run"] and a[-1:] == ["version"]:
    print("hf 2.4.0")
    sys.exit(0)
if a[0] == "inspect":
    run = json.load(open(record))
    print(json.dumps([{"Config": {"Env": run["env"], "Cmd": run["argv"]}, "HostConfig": {"Binds": run["binds"]}}]))
    sys.exit(0)
if a[0] == "ps":
    run = container()
    wanted = [x[len("label="):] for i, x in enumerate(a) if a[i - 1] == "--filter"]
    if run and not os.path.exists(stopped) and all(w in labels(run) for w in wanted):
        print(run["id"])
    sys.exit(0)
if a[0] == "stop":
    run = container()
    if run and run["id"] in a:
        open(stopped, "w").close()
        print(run["id"])
        sys.exit(0)
    print("Error response from daemon: No such container: " + a[-1], file=sys.stderr)
    sys.exit(1)
if a[0] != "run":
    sys.exit(0)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
flags = [a[i + 1] for i, x in enumerate(a) if x == "-e"]
env = [f if "=" in f else "%%s=%%s" %% (f, os.environ.get(f, "")) for f in flags]
binds = [a[i + 1] for i, x in enumerate(a) if x == "-v"]
tmp = record + ".tmp"
if os.path.exists(record) and not os.path.exists(stopped):
    open(os.path.join(state, "second-writer"), "w").close()
if os.path.exists(stopped):
    os.unlink(stopped)
json.dump({"id": "c0ffee%%06d" %% os.getpid(), "argv": a, "env": env, "binds": binds, "environ": dict(os.environ), "pid": os.getpid()}, open(tmp, "w"))
os.replace(tmp, record)
deadline = time.time() + 60
while not os.path.exists(os.path.join(state, "finish")):
    if os.path.exists(stopped):
        sys.exit(137)
    if time.time() > deadline:
        sys.exit(1)
    time.sleep(0.05)
dest = [b.split(":")[0] for b in binds if b.split(":")[1] == "/dest"][0]
for src in os.listdir(os.path.join(state, "repo")):
    shutil.copy(os.path.join(state, "repo", src), dest)
"""

    def __init__(self):
        self.bin = tempfile.mkdtemp(dir=TMP_HOME)
        make_model(os.path.join(self.bin, "repo"), {"config.json": b"{}", "model.safetensors": b"w" * 100})
        stand_in(self.bin, "docker", self.SCRIPT % self.bin)
        self.record = os.path.join(self.bin, "run.json")

    def run(self):
        wait_for("docker run", lambda: os.path.exists(self.record))
        with open(self.record) as f:
            return json.load(f)

    def finish(self):
        open(os.path.join(self.bin, "finish"), "w").close()

    def spec(self, dest, token=TOKEN, instance="dev"):
        files = [{"path": "config.json", "size": 2}, {"path": "model.safetensors", "size": 100, "sha256": hashlib.sha256(b"w" * 100).hexdigest()}]
        return {"repo": "org/m", "revision": "abc", "dest": dest, "image": "vllm:x", "tool": image_tool(), "files": files, "token": token, "label": "Download org/m", "instance": instance}

    def start(self, job_id, token=TOKEN, env=None, instance="dev", dest=None):
        dest = dest or os.path.join(TMP_HOME, "models", job_id)
        sparkjob("start", "download", job_id, stdin=self.spec(dest, token, instance), path=self.bin, env=env)
        return dest

    def hang(self, verb):
        """`docker <verb>` stops answering, as a wedged daemon does."""
        open(os.path.join(self.bin, "hang-" + verb), "w").close()

    def refuse(self, verb):
        """`docker <verb>` fails, as it does with the daemon down."""
        open(os.path.join(self.bin, "fail-" + verb), "w").close()

    def heal(self):
        for name in os.listdir(self.bin):
            if name.startswith(("hang-", "fail-")):
                os.unlink(os.path.join(self.bin, name))

    def end(self):
        """The daemon ends the container, whatever SparkBox did."""
        open(os.path.join(self.bin, "stopped"), "w").close()

    def calls(self, verb):
        if not os.path.exists(os.path.join(self.bin, "calls")):
            return []
        with open(os.path.join(self.bin, "calls")) as f:
            return [c for c in map(json.loads, f) if c[:1] == [verb]]

    def second_writer(self):
        return os.path.exists(os.path.join(self.bin, "second-writer"))

    def kill(self, job_id, run):
        """SIGKILL to the job and its docker CLI: the container is the daemon's and runs on."""
        pid = sj.read_status(job_id)["pid"]
        os.kill(pid, signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(pid))
        os.kill(run["pid"], signal.SIGKILL)
        wait_for("the CLI to die", lambda: pid_gone(run["pid"]))
        assert self.running() == [run["id"]], "the container outlives its CLI"

    def running(self):
        """The container is there (it isn't only its CLI): docker ps, as the node would answer."""
        return subprocess.run([os.path.join(self.bin, "docker"), "ps", "-q"], capture_output=True, text=True).stdout.split()


def docker_hang(docker):
    docker.hang("ps")


def docker_refuse(docker):
    docker.refuse("ps")


class Secrets(unittest.TestCase):
    """C-T and C-S: a job's secrets are nowhere Docker or the job's records keep them, and they end
    with the job, however it ends."""

    def everything_recorded(self, job_id, docker):
        with open(docker.record) as f:
            recorded = f.read()
        inspect = subprocess.run([os.path.join(docker.bin, "docker"), "inspect", job_id], capture_output=True, text=True).stdout
        with open(os.path.join(sj.JOBS, job_id + ".json")) as f:
            status = f.read()
        with open(os.path.join(sj.JOBS, job_id + ".log"), "rb") as f:
            log = f.read().decode("utf-8", "replace")
        return {"run": recorded, "inspect": inspect, "status": status, "log": log}

    def test_a_token_creation_race_cannot_follow_a_symlink(self):
        victim = os.path.join(TMP_HOME, "token-race-victim")
        with open(victim, "w") as f:
            f.write("keep")
        real_open = os.open

        def raced(path, flags, mode=0o777):
            if path.endswith("/hf_token"):
                os.symlink(victim, path)
            return real_open(path, flags, mode)

        with mock.patch.object(os, "open", side_effect=raced):
            with self.assertRaises(FileExistsError):
                sj.write_token("creation-race", "test-secret")
        with open(victim) as f:
            self.assertEqual(f.read(), "keep")

    def test_the_token_is_in_a_private_file_and_nowhere_docker_keeps_it(self):
        docker = Docker()
        dest = docker.start("job-tok-1", env={"HF_TOKEN": "hf_from_the_shell"})
        run = docker.run()
        for name, text in self.everything_recorded("job-tok-1", docker).items():
            self.assertNotIn(TOKEN, text, name)
            self.assertNotIn("hf_from_the_shell", text, name)
        self.assertNotIn("HF_TOKEN", run["environ"], "the job's environment has none")
        self.assertIn("HF_TOKEN_PATH=/run/sparkbox/hf_token", run["env"])
        self.assertEqual([e for e in run["env"] if e.startswith("HF_TOKEN=")], [])
        folder = os.path.join(sj.SECRETS, "job-tok-1")
        file = os.path.join(folder, "hf_token")
        self.assertIn(file + ":/run/sparkbox/hf_token:ro", run["binds"])
        self.assertEqual(stat_mode(sj.SECRETS), 0o700)
        self.assertEqual(stat_mode(folder), 0o700)
        self.assertEqual(stat_mode(file), 0o400)
        with open(file) as f:
            self.assertEqual(f.read(), TOKEN)
        docker.finish()
        st = wait_done("job-tok-1")
        self.assertEqual(st["state"], "done", st.get("message"))
        self.assertFalse(os.path.exists(folder), "gone after a normal end")
        self.assertEqual(sj.read_manifest(dest)["checked"], "sha256")

    def test_cancel_takes_the_token_with_it(self):
        docker = Docker()
        docker.start("job-tok-2")
        run = docker.run()
        folder = os.path.join(sj.SECRETS, "job-tok-2")
        self.assertTrue(os.path.isdir(folder))
        started = time.time()
        st = sparkjob("cancel", "job-tok-2", path=docker.bin)
        self.assertEqual(st["state"], "cancelled")
        self.assertLess(time.time() - started, 3.5)
        self.assertFalse(os.path.exists(folder), "gone after a cancel")
        wait_for("the docker client to stop", lambda: pid_gone(run["pid"]))

    def test_cancel_stops_the_container_by_both_labels_not_only_its_cli(self):
        docker = Docker()
        docker.start("job-stop-1")
        run = docker.run()
        self.assertEqual(sj.read_status("job-stop-1")["instance"], "dev")
        st = sparkjob("cancel", "job-stop-1", path=docker.bin)
        self.assertEqual(st["state"], "cancelled")
        self.assertEqual(docker.running(), [], "hf ignores SIGTERM: only docker stop ends it")
        self.assertEqual(docker.calls("stop"), [["stop", "-t", "5", run["id"]]])
        wanted = ["ps", "-q", "--filter", "label=sparkbox.deployment=job-stop-1", "--filter", "label=sparkbox.instance=dev"]
        self.assertIn(wanted, docker.calls("ps"))
        wait_for("the CLI to end", lambda: pid_gone(run["pid"]))

    def test_a_container_without_both_labels_is_never_touched(self):
        docker = Docker()
        docker.start("job-stop-2", instance="release")
        run = docker.run()
        old_path = os.environ["PATH"]
        os.environ["PATH"] = docker.bin + os.pathsep + old_path
        try:
            sj.stop_download("job-stop-2", "dev")  # the other SparkBox's
            sj.stop_download("job-stop-9", "release")  # another job's
            sj.stop_download("job-stop-2", "")
            sj.stop_download("../job-stop-2", "release")
            self.assertEqual(docker.calls("stop"), [])
            self.assertEqual(docker.running(), [run["id"]])
            sj.stop_download("job-stop-2", "release")
            self.assertEqual(docker.calls("stop"), [["stop", "-t", "5", run["id"]]])
        finally:
            os.environ["PATH"] = old_path

    def test_no_docker_and_nothing_found_are_both_fine(self):
        old_path = os.environ["PATH"]
        os.environ["PATH"] = tempfile.mkdtemp(dir=TMP_HOME)
        try:
            self.assertEqual(sj.labelled_containers("sparkbox.deployment=x"), [])
            sj.stop_download("x", "dev")
        finally:
            os.environ["PATH"] = old_path

    def test_a_retry_stops_a_killed_jobs_container_first(self):
        # No list in between: the old job's id comes from the folder's marker.
        docker = Docker()
        dest = docker.start("job-re-1")
        first = docker.run()
        docker.kill("job-re-1", first)
        docker.start("job-re-2", dest=dest)
        wait_for("the new job's container", lambda: docker.run()["id"] != first["id"])
        self.assertFalse(docker.second_writer(), "the old container was still writing")
        self.assertEqual(docker.calls("stop"), [["stop", "-t", "5", first["id"]]])
        self.assertEqual(sj.read_marker(dest)["job"], "job-re-2")
        docker.finish()
        self.assertEqual(wait_done("job-re-2")["state"], "done")

    def test_a_retry_refuses_a_folder_the_other_sparkbox_is_writing(self):
        docker = Docker()
        dest = docker.start("job-re-3", instance="release")
        first = docker.run()
        docker.kill("job-re-3", first)
        docker.start("job-re-4", dest=dest, instance="dev")
        st = wait_done("job-re-4")
        self.assertEqual(st["state"], "failed")
        self.assertEqual(st["message"], "An earlier download is still writing to this folder: container sparkbox-dl-job-re-3 is running.")
        self.assertEqual(docker.calls("stop"), [], "it isn't this SparkBox's to stop")
        self.assertEqual(docker.running(), [first["id"]])
        self.assertFalse(docker.second_writer())
        self.assertEqual(sj.read_marker(dest)["job"], "job-re-3")

    def test_a_copy_onto_a_killed_download_stops_its_container_first(self):
        docker = Docker()
        dest = docker.start("job-re-5")
        first = docker.run()
        docker.kill("job-re-5", first)
        stand_in(docker.bin, "rsync", "#!/bin/sh\nexit 0\n")
        sparkjob("start", "pull", "job-re-6", path=docker.bin, stdin={"src": "rsync://sparkbox@127.0.0.1:1/model", "dest": dest, "password": "p", "files": known_empty_file(dest), "instance": "dev"})
        self.assertEqual(wait_done("job-re-6")["state"], "done")
        self.assertEqual(docker.calls("stop"), [["stop", "-t", "5", first["id"]]])

    def test_list_stops_nothing_and_says_a_container_may_be_running(self):
        docker = Docker()
        docker.start("job-re-7")
        first = docker.run()
        docker.kill("job-re-7", first)
        calls = docker.calls("ps")
        by = {j["id"]: j for j in sparkjob("list", path=docker.bin)}
        self.assertEqual(by["job-re-7"]["state"], "failed")
        self.assertEqual(by["job-re-7"]["message"], "The job stopped unexpectedly (node restarted?). Its container may still be running: Resume download stops it first.")
        self.assertEqual(docker.calls("stop"), [])
        self.assertEqual(docker.calls("ps"), calls, "list doesn't even ask docker")
        self.assertEqual(docker.running(), [first["id"]])

    def test_a_download_needs_an_instance_it_can_label_with(self):
        docker = Docker()
        for job_id, instance in (("job-in-1", ""), ("job-in-2", "--privileged")):
            docker.start(job_id, instance=instance)
            st = wait_done(job_id)
            self.assertEqual((st["state"], st["message"]), ("failed", "A download needs its SparkBox's instance name."))
        self.assertEqual(docker.calls("run"), [])

    def test_a_sigkilled_job_loses_its_token_at_the_next_list(self):
        docker = Docker()
        docker.start("job-tok-3")
        run = docker.run()
        pid = sj.read_status("job-tok-3")["pid"]
        folder = os.path.join(sj.SECRETS, "job-tok-3")
        os.kill(pid, signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(pid))
        os.kill(run["pid"], signal.SIGKILL)
        self.assertTrue(os.path.isdir(folder), "SIGKILL runs no finally")
        by = {j["id"]: j for j in sparkjob("list")}
        self.assertFalse(os.path.exists(folder), "gone at the next list")
        self.assertEqual(by["job-tok-3"]["state"], "failed")

    def test_cancel_covers_a_job_that_died_with_its_secrets_still_there(self):
        docker = Docker()
        docker.start("job-tok-5")
        run = docker.run()
        pid = sj.read_status("job-tok-5")["pid"]
        os.kill(pid, signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(pid))
        os.kill(run["pid"], signal.SIGKILL)
        folder = os.path.join(sj.SECRETS, "job-tok-5")
        self.assertTrue(os.path.isdir(folder))
        sparkjob("cancel", "job-tok-5", path=docker.bin)
        self.assertFalse(os.path.exists(folder))
        self.assertEqual(docker.running(), [], "and its container stops")

    def test_a_download_told_to_stop_stops_docker_too(self):
        # SIGTERM to the job alone, as a node shutting down might.
        docker = Docker()
        docker.start("job-tok-6")
        run = docker.run()
        pid = sj.read_status("job-tok-6")["pid"]
        os.kill(pid, signal.SIGTERM)
        wait_for("the job to end", lambda: pid_gone(pid))
        wait_for("docker to stop", lambda: pid_gone(run["pid"]))
        self.assertEqual(docker.running(), [])
        st = sj.read_status("job-tok-6")
        self.assertEqual((st["state"], st["message"]), ("failed", "The job was stopped."))
        self.assertFalse(os.path.exists(os.path.join(sj.SECRETS, "job-tok-6")))

    def test_no_token_means_no_path_and_no_mount(self):
        docker = Docker()
        docker.start("job-tok-4", token="")
        run = docker.run()
        self.assertEqual([e for e in run["env"] if e.startswith("HF_TOKEN")], [])
        self.assertEqual(run["binds"], [os.path.join(TMP_HOME, "models", "job-tok-4") + ":/dest"])
        self.assertFalse(os.path.exists(os.path.join(sj.SECRETS, "job-tok-4")))
        docker.finish()
        self.assertEqual(wait_done("job-tok-4")["state"], "done")

    def test_the_download_command_with_and_without_a_token(self):
        spec = {"repo": "org/m", "revision": "abc", "image": "vllm:x", "tool": image_tool(), "instance": "dev", "files": [{"path": "a.safetensors"}]}
        uid = "%d:%d" % (os.getuid(), os.getgid())
        head = ["docker", "run", "--rm", "--name", "sparkbox-dl-dl-1", "--label", "sparkbox.instance=dev", "--label", "sparkbox.deployment=dl-1", "--user", uid]
        tail = ["-e", "HF_HOME=/tmp/hf", "-e", "HF_HUB_DISABLE_PROGRESS_BARS=1", "-e", "HF_HUB_DISABLE_UPDATE_CHECK=1", "-e", "HF_HUB_DISABLE_TELEMETRY=1"]
        rest = ["--pull", "never", "-v", "/m/x:/dest", "--entrypoint", "hf", "vllm:x", "download", "org/m", "--revision", "abc", "--local-dir", "/dest", "--include", "a.safetensors"]
        self.assertEqual(sj.download_cmd("dl-1", spec, "/m/x"), head + tail + rest)
        token = "/h/.sparkbox/job-secrets/j/hf_token"
        self.assertEqual(sj.download_cmd("dl-1", spec, "/m/x", token),
                         head + ["-e", "HF_TOKEN_PATH=/run/sparkbox/hf_token"] + tail + ["-v", token + ":/run/sparkbox/hf_token:ro"] + rest)

    def serve(self, job_id, rsync="#!/bin/sh\nexec sleep 60\n"):
        bin_dir = tempfile.mkdtemp(dir=TMP_HOME)
        stand_in(bin_dir, "rsync", rsync)
        path = tempfile.mkdtemp(dir=TMP_HOME)
        sparkjob("start", "serve", job_id, stdin={"path": path, "address": "127.0.0.1", "port": 8999, "password": "rsync-pw"}, path=bin_dir)
        secrets = os.path.join(sj.RSYNCD, job_id, "secrets")
        wait_for("the rsync server", lambda: os.path.exists(secrets) and sj.read_status(job_id).get("childPid"))
        return secrets, sj.read_status(job_id)

    def test_cancelling_a_copys_file_server_removes_its_password_file(self):
        secrets, st = self.serve("job-serve-1")
        self.assertEqual(stat_mode(secrets), 0o600)
        started = time.time()
        got = sparkjob("cancel", "job-serve-1")
        self.assertEqual(got["state"], "cancelled")
        self.assertLess(time.time() - started, 3.5)
        self.assertFalse(os.path.exists(os.path.dirname(secrets)), "the password file outlived the cancel")
        wait_for("rsync to stop", lambda: pid_gone(st["childPid"]))
        wait_for("the job to end", lambda: pid_gone(st["pid"]))
        self.assertEqual(sj.read_status("job-serve-1")["state"], "cancelled", "it doesn't turn into done")

    def test_a_canceled_job_that_ends_by_itself_stays_canceled(self):
        # cancel() writes the state before it signals: a server that goes down at that moment
        # ends its job normally, and that job doesn't turn into done.
        flag = os.path.join(TMP_HOME, "rsync-may-end")
        secrets, st = self.serve("job-serve-5", rsync='#!/bin/sh\ni=0; while [ ! -e "%s" ] && [ $i -lt 6000 ]; do i=$((i + 1)); sleep 0.05; done\n' % flag)
        sj.write_status("job-serve-5", state="cancelled", message="Canceled")
        open(flag, "w").close()
        wait_for("the job to end", lambda: pid_gone(st["pid"]))
        self.assertEqual(sj.read_status("job-serve-5")["state"], "cancelled")

    def test_a_file_server_stops_its_rsync_when_it_is_told_to_stop(self):
        # SIGTERM to the job alone, as a node shutting down might: the child isn't left behind.
        secrets, st = self.serve("job-serve-2")
        os.kill(st["pid"], signal.SIGTERM)
        wait_for("the job to end", lambda: pid_gone(st["pid"]))
        wait_for("rsync to stop", lambda: pid_gone(st["childPid"]))
        self.assertFalse(os.path.exists(secrets))
        self.assertEqual(sj.read_status("job-serve-2")["state"], "failed")

    def test_a_sigkilled_servers_password_file_goes_at_the_next_list(self):
        secrets, st = self.serve("job-serve-3")
        os.kill(st["pid"], signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(st["pid"]))
        os.kill(st["childPid"], signal.SIGKILL)
        self.assertTrue(os.path.exists(secrets), "SIGKILL runs no finally")
        sparkjob("list")
        self.assertFalse(os.path.exists(os.path.dirname(secrets)))

    def test_list_look_reports_a_dead_job_and_changes_nothing(self):
        secrets, st = self.serve("job-serve-6")
        path = sj.status_path("job-serve-6")
        os.kill(st["pid"], signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(st["pid"]))
        os.kill(st["childPid"], signal.SIGKILL)
        with open(path, "rb") as f:
            before = f.read()
        listed = {j["id"]: j for j in sparkjob("list", "--look")}
        self.assertEqual(listed["job-serve-6"]["state"], "failed", "it still reads as dead")
        self.assertIn("stopped unexpectedly", listed["job-serve-6"]["message"])
        with open(path, "rb") as f:
            self.assertEqual(f.read(), before, "look-only rewrote the status")
        self.assertTrue(os.path.exists(secrets), "look-only removed a file")
        sparkjob("list")
        self.assertNotEqual(sj.read_status("job-serve-6")["state"], "running")
        self.assertFalse(os.path.exists(os.path.dirname(secrets)))

    def test_list_leaves_a_running_jobs_secrets_alone(self):
        secrets, st = self.serve("job-serve-4")
        sparkjob("list")
        self.assertTrue(os.path.exists(secrets), "this job is alive")
        sparkjob("cancel", "job-serve-4")

    def test_a_job_that_starts_after_list_read_the_jobs_folder_keeps_its_secrets(self):
        # list reads every status, then sweeps: a job can start, and write its secrets, in between.
        real = sj.sweep_secrets

        def a_job_starts_first(*args):
            sj.write_status("job-late-1", kind="download", state="running", pid=os.getpid())
            sj.write_token("job-late-1", TOKEN)
            os.makedirs(os.path.join(sj.RSYNCD, "job-late-1"))
            real(*args)
        sj.sweep_secrets = a_job_starts_first
        try:
            sj.list_jobs()
        finally:
            sj.sweep_secrets = real
        self.assertTrue(os.path.exists(os.path.join(sj.SECRETS, "job-late-1", "hf_token")), "a live job's token")
        self.assertTrue(os.path.isdir(os.path.join(sj.RSYNCD, "job-late-1")), "a live job's rsync password")
        sj.write_status("job-late-1", state="failed")
        sj.list_jobs()
        self.assertFalse(os.path.exists(os.path.join(sj.SECRETS, "job-late-1")), "once it's over, they go")
        self.assertFalse(os.path.exists(os.path.join(sj.RSYNCD, "job-late-1")))

    def test_a_job_reads_as_over_only_once_its_token_has_gone(self):
        # sj.start forks this process, so the job writes its statuses through this stand-in.
        docker = Docker()
        saw = os.path.join(docker.bin, "secrets-at-the-last-status")
        real = sj.write_status

        def noting(job_id, **fields):
            if fields.get("state") == "done":
                with open(saw, "w") as f:
                    f.write(str(os.path.exists(os.path.join(sj.SECRETS, job_id))))
            return real(job_id, **fields)
        path = os.environ["PATH"]
        sj.write_status, os.environ["PATH"] = noting, docker.bin + os.pathsep + path
        try:
            pid = sj.start("download", "job-tok-7", docker.spec(os.path.join(TMP_HOME, "models", "job-tok-7")))["pid"]
        finally:
            sj.write_status, os.environ["PATH"] = real, path
        docker.run()
        docker.finish()
        os.waitpid(pid, 0)
        self.assertEqual(sj.read_status("job-tok-7")["state"], "done")
        with open(saw) as f:
            self.assertEqual(f.read(), "False", "the job read as done while its token was on disk")

    def test_docker_that_cant_say_is_not_none_running(self):
        docker = Docker()
        path, limit = os.environ["PATH"], sj.DOCKER_PS_S
        os.environ["PATH"], sj.DOCKER_PS_S = docker.bin + os.pathsep + path, 0.3
        try:
            docker.refuse("ps")
            self.assertIsNone(sj.labelled_containers("sparkbox.deployment=x"))
            docker.heal()
            docker.hang("ps")
            self.assertIsNone(sj.labelled_containers("sparkbox.deployment=x"))
            sj.stop_download("x", "dev")
            self.assertEqual(docker.calls("stop"), [], "and nothing listed, nothing stopped")
        finally:
            os.environ["PATH"], sj.DOCKER_PS_S = path, limit

    def test_a_retry_stops_when_docker_cant_say_whether_the_old_container_runs(self):
        # The daemon is down, and the first job's container may be writing still.
        docker = Docker()
        dest = docker.start("job-re-8")
        first = docker.run()
        docker.kill("job-re-8", first)
        docker.refuse("ps")
        docker.start("job-re-9", dest=dest)
        st = wait_done("job-re-9")
        docker.heal()
        self.assertEqual(st["state"], "failed")
        self.assertEqual(st["message"], "Docker didn't answer, so SparkBox can't tell whether an earlier download is still writing to this folder.")
        self.assertEqual(len(docker.calls("run")), 2, "only the first version probe and download; no retry container")
        self.assertFalse(docker.second_writer())
        self.assertEqual(sj.read_marker(dest)["job"], "job-re-8")
        docker.end()

    def test_a_docker_ps_past_its_limit_stops_a_retry_too(self):
        docker = Docker()
        dest = docker.start("job-re-10", token="")
        docker.kill("job-re-10", docker.run())
        docker.hang("ps")
        path, limit = os.environ["PATH"], sj.DOCKER_PS_S
        os.environ["PATH"], sj.DOCKER_PS_S = docker.bin + os.pathsep + path, 0.3
        try:
            with self.assertRaisesRegex(RuntimeError, "Docker didn't answer"):
                sj.stop_leftover(dest, "dev")
        finally:
            os.environ["PATH"], sj.DOCKER_PS_S = path, limit
        docker.heal()
        docker.end()

    def test_a_copys_marker_asks_nothing_of_docker(self):
        # A copy has no container, and rsync needs no daemon: it can go on with docker down.
        docker = Docker()
        docker.refuse("ps")
        dest = tempfile.mkdtemp(dir=TMP_HOME)
        sj.mark_partial(dest, "share-1-cell2", "pull")
        path = os.environ["PATH"]
        os.environ["PATH"] = docker.bin + os.pathsep + path
        try:
            sj.stop_leftover(dest, "dev")
        finally:
            os.environ["PATH"] = path
        self.assertEqual(docker.calls("ps"), [])

    def cancel_with(self, docker, job_id):
        """sj.cancel, with docker on the PATH and its limits short."""
        path = os.environ["PATH"]
        limits = sj.DOCKER_PS_S, sj.DOCKER_STOP_S, sj.CANCEL_WAIT_S
        os.environ["PATH"] = docker.bin + os.pathsep + path
        sj.DOCKER_PS_S, sj.DOCKER_STOP_S, sj.CANCEL_WAIT_S = 0.3, 0.3, 0.1
        try:
            return sj.cancel(job_id)
        finally:
            os.environ["PATH"] = path
            sj.DOCKER_PS_S, sj.DOCKER_STOP_S, sj.CANCEL_WAIT_S = limits

    MAY_BE_RUNNING = "Canceled, but its container may still be running: Resume download stops it first."

    def test_cancel_says_so_when_the_container_didnt_stop(self):
        docker = Docker()
        docker.start("job-can-1", token="")
        run = docker.run()
        docker.hang("stop")
        st = self.cancel_with(docker, "job-can-1")
        self.assertEqual((st["state"], st["message"]), ("cancelled", self.MAY_BE_RUNNING))
        self.assertEqual(sj.read_status("job-can-1")["message"], self.MAY_BE_RUNNING)
        self.assertEqual(docker.running(), [run["id"]], "the message is true")
        docker.heal()
        docker.end()

    def test_cancel_says_so_when_docker_cant_say(self):
        for job_id, verb in (("job-can-2", docker_hang), ("job-can-3", docker_refuse)):
            docker = Docker()
            docker.start(job_id, token="")
            docker.run()
            verb(docker)
            st = self.cancel_with(docker, job_id)
            self.assertEqual((st["state"], st["message"]), ("cancelled", self.MAY_BE_RUNNING), job_id)
            docker.heal()
            docker.end()

    def test_a_container_that_stopped_reads_as_plainly_canceled(self):
        docker = Docker()
        docker.start("job-can-4", token="")
        docker.run()
        st = sparkjob("cancel", "job-can-4", path=docker.bin)
        self.assertEqual((st["state"], st["message"]), ("cancelled", "Canceled"))
        self.assertEqual(docker.running(), [])

    def test_only_a_download_has_a_container_to_ask_about(self):
        secrets, _ = self.serve("job-can-5")
        docker = Docker()
        docker.refuse("ps")
        st = self.cancel_with(docker, "job-can-5")
        self.assertEqual((st["state"], st["message"]), ("cancelled", "Canceled"))
        self.assertEqual(docker.calls("ps"), [])

    def test_cancels_last_look_fits_in_what_sparkbox_waits_for(self):
        # stop (5 s of ps, 10 s of stop) and the 3 s wait can use most of the 20 s Rust gives it.
        self.assertLess(sj.CANCEL_LIMIT_S + 1, 20)
        docker = Docker()
        docker.start("job-can-6", token="")
        docker.run()
        real, limit, asked = sj.labelled_containers, sj.CANCEL_LIMIT_S, []

        def noting(*labels, timeout=None):
            asked.append(timeout)
            return real(*labels, timeout=timeout)
        sj.labelled_containers, sj.CANCEL_LIMIT_S = noting, 0
        try:
            self.cancel_with(docker, "job-can-6")
        finally:
            sj.labelled_containers, sj.CANCEL_LIMIT_S = real, limit
        self.assertEqual(asked, [None, 0.1], "the last look gets what's left, at least a moment")

    def test_a_job_with_no_pid_yet_is_starting_not_dead(self):
        # start() writes the status, then forks: `list` may read it in between.
        sj.write_status("job-just-started", kind="serve", state="running")
        folder = os.path.join(sj.RSYNCD, "job-just-started")
        os.makedirs(folder)
        by = {j["id"]: j for j in sj.list_jobs()}
        self.assertEqual(by["job-just-started"]["state"], "running")
        self.assertTrue(os.path.isdir(folder))
        sj.write_status("job-just-started", state="cancelled")
        sj.list_jobs()
        self.assertFalse(os.path.exists(folder))

    def test_secrets_of_a_folder_with_no_job_go_and_nothing_else(self):
        os.makedirs(sj.SECRETS, exist_ok=True)
        os.makedirs(os.path.join(sj.SECRETS, "no-such-job"))
        with open(os.path.join(sj.SECRETS, "..keep"), "w") as f:
            f.write("not a job's")
        try:
            sj.list_jobs()
            self.assertEqual(os.listdir(sj.SECRETS).count("no-such-job"), 0)
            self.assertTrue(os.path.exists(os.path.join(sj.SECRETS, "..keep")), "only a job id's folder is touched")
        finally:
            os.unlink(os.path.join(sj.SECRETS, "..keep"))

    def test_drop_secrets_takes_only_a_job_id(self):
        keep = os.path.join(TMP_HOME, ".sparkbox", "keep-me")
        os.makedirs(keep, exist_ok=True)
        for folder in (sj.SECRETS, sj.RSYNCD):
            os.makedirs(folder, exist_ok=True)
        for bad in ("../keep-me", "..", ".", "", "a/b"):
            sj.drop_secrets(bad)
        self.assertTrue(os.path.isdir(keep))
        self.assertTrue(os.path.isdir(sj.SECRETS) and os.path.isdir(sj.RSYNCD))

    def test_a_cancel_during_the_checksums_stops_the_workers(self):
        # A job's SIGTERM handler would make a forked hashing worker pass Stopped back and hash
        # the next file. Each FIFO blocks its worker in open(), so without SIGTERM's default action
        # the job never ends.
        import multiprocessing
        folder = tempfile.mkdtemp(dir=TMP_HOME)
        names = ["w%d.safetensors" % i for i in range(16)]
        for n in names:
            os.mkfifo(os.path.join(folder, n))
        spec = {"src": "/nonexistent", "dest": folder, "password": "pw", "checksums": True,
                "files": [{"path": n, "size": 0, "sha256": "0" * 64} for n in names]}
        bin_dir = tempfile.mkdtemp(dir=TMP_HOME)
        stand_in(bin_dir, "rsync", "#!/bin/sh\nexit 0\n")
        method, path = multiprocessing.get_start_method(), os.environ["PATH"]
        multiprocessing.set_start_method("fork", force=True)  # as on a node's Python 3.12
        os.environ["PATH"] = bin_dir + os.pathsep + path
        try:
            pid = sj.start("pull", "job-hash-1", spec)["pid"]
            # Its workers never leave those FIFOs: a test that stops here mustn't leave them.
            self.addCleanup(end_job, pid)
            wait_for("the checksums to start", lambda: sj.read_status("job-hash-1").get("step") == "verifying")
            time.sleep(0.5)
            sj.cancel("job-hash-1")
            for _ in range(120):
                if os.waitpid(pid, os.WNOHANG)[0]:
                    break
                time.sleep(0.05)
            else:
                os.killpg(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
                self.fail("the job kept hashing after the cancel")
        finally:
            multiprocessing.set_start_method(method, force=True)
            os.environ["PATH"] = path
        self.assertEqual(sj.read_status("job-hash-1")["state"], "cancelled")


class PartialFolders(unittest.TestCase):
    """C-P: a folder written partway says so, whatever stopped it."""

    def setUp(self):
        self.root = tempfile.mkdtemp(dir=TMP_HOME)
        self.src = os.path.join(self.root, "src")
        self.dest = os.path.join(self.root, "org", "m")
        make_model(self.src, {"config.json": b"{}", "model.safetensors": b"m" * 1000, "original/consolidated.pth": b"p" * 50, ".sparkbox.json": b'{"verifiedAt": 1}'})
        self.files = [{"path": "config.json", "size": 2}, {"path": "model.safetensors", "size": 1000}]

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def item(self, path=None):
        found = {m["path"]: m for m in sj.scan([self.root])}
        return found.get(path or self.dest)

    def spec(self, label="Copy org/m to cell2"):
        return {"src": self.src, "dest": self.dest, "password": "pw", "files": self.files, "exclude": ["original/", "metal/", "*.pth"],
                "checksums": False, "repo": "org/m", "revision": "rev1", "label": label}

    def rsync(self, after):
        """A stand-in rsync that records its argv, runs the real one (or copies the files), then
        runs `after`: the job is alive past the transfer for as long as a test needs."""
        bin_dir = tempfile.mkdtemp(dir=TMP_HOME)
        argv = os.path.join(bin_dir, "argv.json")
        real = shutil.which("rsync")
        body = "#!/bin/sh\n"
        if real:
            body += 'if [ "$1" = --server ]; then exec %s "$@"; fi\n' % real  # a local copy's other end
        body += "python3 -c 'import json,sys; json.dump(sys.argv[2:], open(sys.argv[1], \"w\"))' %s \"$@\"\n" % argv
        if real:
            body += '%s "$@"\n' % real
        else:
            body += 'cp "%s/config.json" "%s/model.safetensors" "%s/"\n' % (self.src, self.src, self.dest)
        stand_in(bin_dir, "rsync", body + after + "\n")
        return bin_dir, argv

    def test_a_killed_pull_is_stopped_unverified_and_its_stale_record_is_gone(self):
        make_model(self.dest, {"config.json": b"{}", "model.safetensors": b"o" * 3, "old.safetensors": b"o",
                               ".sparkbox.json": json.dumps({"repo": "org/m", "verifiedAt": 5}).encode()})
        moved = os.path.join(self.root, "moved")
        bin_dir, _ = self.rsync('touch "%s"; exec sleep 60' % moved)
        sparkjob("start", "pull", "job-part-1", stdin=self.spec(), path=bin_dir)
        wait_for("the marker", lambda: os.path.exists(os.path.join(self.dest, sj.PARTIAL)))
        wait_for("the transfer", lambda: os.path.exists(moved))
        self.assertEqual(self.item()["partial"], "copying")
        self.assertFalse(self.item()["verified"])
        self.assertFalse(os.path.exists(os.path.join(self.dest, sj.MANIFEST)), "the record of what was there before is gone")
        st = sj.read_status("job-part-1")
        os.killpg(st["pid"], signal.SIGKILL)
        wait_for("the job to die", lambda: pid_gone(st["pid"]))
        got = self.item()
        self.assertEqual((got["partial"], got["verified"], got["checked"]), ("stopped", False, ""))
        with open(os.path.join(self.dest, sj.PARTIAL)) as f:
            self.assertEqual(json.load(f)["kind"], "pull", "the marker survived the transfer")
        self.assertEqual(sorted(os.listdir(os.path.join(self.dest))), [sj.PARTIAL, "config.json", "model.safetensors"], "what wasn't on the source is gone")

    def test_a_finished_pull_has_its_own_record_and_no_marker(self):
        make_model(self.dest, {".sparkbox.json": b'{"repo": "stale", "verifiedAt": 5}'})
        self.files.append({"path": ".sparkbox.json", "size": 17})  # the source's: it isn't copied, or checked for
        bin_dir, argv = self.rsync("exit 0")
        sparkjob("start", "pull", "job-part-2", stdin=self.spec(), path=bin_dir)
        st = wait_done("job-part-2")
        self.assertEqual(st["state"], "done", st.get("message"))
        self.assertEqual(st["dest"], self.dest, "the job says where it writes")
        self.assertFalse(os.path.exists(os.path.join(self.dest, sj.PARTIAL)))
        record = sj.read_manifest(self.dest)
        self.assertEqual((record["checked"], record["repo"], record["revision"]), ("sizes", "org/m", "rev1"))
        self.assertEqual([f["path"] for f in record["files"]], ["config.json", "model.safetensors"])
        got = self.item()
        self.assertEqual((got["partial"], got["verified"], got["checked"]), (None, True, "sizes"))
        with open(argv) as f:
            self.assertEqual(json.load(f), ["-aL", "--partial", "--delete", "--exclude=original/", "--exclude=metal/", "--exclude=*.pth", "--exclude=/.sparkbox.json",
                                            "--exclude=/.sparkbox.partial", "--exclude=/.cache/", self.src + "/", self.dest + "/"])

    def test_the_marker_outlives_rsyncs_delete_until_the_file_check_passes(self):
        # The source says 1000 bytes; a folder that came up short fails the check and stays marked.
        self.files[1]["size"] = 1001
        bin_dir, _ = self.rsync("exit 0")
        sparkjob("start", "pull", "job-part-3", stdin=self.spec(), path=bin_dir)
        st = wait_done("job-part-3")
        self.assertEqual(st["state"], "failed")
        self.assertIn("Copy incomplete", st["message"])
        got = self.item()
        self.assertEqual((got["partial"], got["verified"]), ("stopped", False))
        self.assertFalse(os.path.exists(os.path.join(self.dest, sj.MANIFEST)))

    def test_a_download_that_fails_its_check_stays_incomplete(self):
        docker = Docker()
        with open(os.path.join(docker.bin, "repo", "config.json"), "w") as f:
            f.write("not the promised two bytes")
        dest = docker.start("job-part-4")
        docker.run()
        self.assertEqual(self.scan_one(dest)["partial"], "downloading")
        docker.finish()
        self.assertEqual(wait_done("job-part-4")["state"], "failed")
        got = self.scan_one(dest)
        self.assertEqual((got["partial"], got["verified"], got["checked"]), ("stopped", False, ""))
        self.assertIsNone(sj.read_manifest(dest))

    def test_a_finished_download_is_verified_by_sha256_and_loses_its_marker(self):
        docker = Docker()
        dest = docker.start("job-part-5", token="")
        wait_for("the marker", lambda: os.path.exists(os.path.join(dest, sj.PARTIAL)))
        with open(os.path.join(dest, sj.PARTIAL)) as f:
            marker = json.load(f)
        self.assertEqual((marker["job"], marker["kind"]), ("job-part-5", "download"))
        self.assertLessEqual(abs(marker["at"] - time.time()), 30)
        docker.finish()
        self.assertEqual(wait_done("job-part-5")["state"], "done")
        got = self.scan_one(dest)
        self.assertEqual((got["partial"], got["verified"], got["checked"]), (None, True, "sha256"))

    def scan_one(self, dest):
        return {m["path"]: m for m in sj.scan([os.path.dirname(dest)])}[dest]

    def test_scan_reads_each_kind_of_folder(self):
        def folder(name, files):
            p = os.path.join(self.root, "org", name)
            make_model(p, files)
            return p
        weights = {"config.json": b"{}", "w.safetensors": b"1"}
        # A download from before records said what they checked.
        older = folder("older", dict(weights, **{sj.MANIFEST: b'{"repo": "org/older", "verifiedAt": 5}'}))
        self.assertEqual((self.item(older)["partial"], self.item(older)["verified"], self.item(older)["checked"]), (None, True, "sha256"))
        # Found by hand: nothing says it's unfinished.
        hand = folder("hand", weights)
        self.assertEqual((self.item(hand)["partial"], self.item(hand)["verified"], self.item(hand)["checked"]), (None, False, ""))
        # An older download, cancelled: no marker, no record, hf's unfinished file.
        cut = folder("cut", dict(weights, **{".cache/huggingface/download/w2.safetensors.abc.incomplete": b"x"}))
        self.assertEqual(self.item(cut)["partial"], "stopped")
        # A finished one keeps hf's leftovers: its record says it's whole.
        whole = folder("whole", dict(weights, **{sj.MANIFEST: b'{"verifiedAt": 5, "checked": "sizes"}', ".cache/huggingface/download/w.safetensors.metadata": b"x",
                                                 ".cache/huggingface/download/w2.safetensors.abc.incomplete": b"x"}))
        self.assertEqual((self.item(whole)["partial"], self.item(whole)["verified"], self.item(whole)["checked"]), (None, True, "sizes"))
        # A marker with no such job, and one that can't be read, are stopped; a record doesn't outvote them.
        gone = folder("gone", dict(weights, **{sj.PARTIAL: b'{"job": "no-such-job", "kind": "pull", "at": 1}', sj.MANIFEST: b'{"verifiedAt": 5}'}))
        self.assertEqual((self.item(gone)["partial"], self.item(gone)["verified"]), ("stopped", False))
        torn = folder("torn", dict(weights, **{sj.PARTIAL: b'{"job": '}))
        self.assertEqual(self.item(torn)["partial"], "stopped")
        planted = folder("planted", dict(weights, **{sj.PARTIAL: b'{"job": "../../x"}'}))
        self.assertEqual(self.item(planted)["partial"], "stopped")
        # Before its first weights finish a folder is only a marker, or hf's file: still listed.
        early = folder("early", {sj.PARTIAL: b'{"job": "no-such-job"}'})
        self.assertEqual(self.item(early)["partial"], "stopped")
        early2 = folder("early2", {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x"})
        self.assertEqual(self.item(early2)["partial"], "stopped")
        # hf's own folder isn't a model below it.
        early3 = folder("early3", {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x", ".cache/huggingface/old/config.json": b"{}"})
        self.assertEqual(self.item(early3)["partial"], "stopped")

    def test_a_stray_hf_file_above_finished_models_hides_none_of_them(self):
        # `hf download --local-dir ~/models/acme`, by hand, cut short, above two finished models.
        acme = os.path.join(self.root, "acme")
        good = [os.path.join(acme, n) for n in ("good-a", "good-b")]
        for path in good:
            make_model(path, {"config.json": b"{}", "w.safetensors": b"1", sj.MANIFEST: b'{"repo": "acme/x", "verifiedAt": 5}'})
        make_model(acme, {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x"})
        found = {m["path"]: m for m in sj.scan([self.root])}
        self.assertNotIn(acme, found)
        self.assertEqual([(found[p]["verified"], found[p]["partial"]) for p in good], [(True, None), (True, None)])
        with self.assertRaises(SystemExit):
            sj.delete(self.root, acme)
        self.assertTrue(all(os.path.exists(os.path.join(p, "config.json")) for p in good), "Delete left them alone")

    def test_a_stray_hf_file_at_the_root_hides_none_of_the_models(self):
        make_model(self.root, {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x"})
        found = {m["path"]: m for m in sj.scan([self.root])}
        self.assertEqual(list(found), [self.src], "the model, not one row for the whole folder")

    def test_a_stray_hf_file_above_a_folder_left_partway_hides_it_too(self):
        org = os.path.join(self.root, "org2")
        partway = os.path.join(org, "partway")
        make_model(partway, {sj.PARTIAL: b'{"job": "no-such-job"}'})
        make_model(org, {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x"})
        self.assertEqual(self.item(partway)["partial"], "stopped")
        self.assertIsNone(self.item(org))
        with self.assertRaises(SystemExit):
            sj.delete(self.root, org)

    def test_delete_takes_a_folder_that_stopped_before_it_had_a_config(self):
        early = os.path.join(self.root, "org", "early")
        make_model(early, {sj.PARTIAL: b'{"job": "no-such-job"}'})
        sj.delete(self.root, early)
        self.assertFalse(os.path.exists(early))
        junk = os.path.join(self.root, "org", "junk")
        make_model(junk, {"notes.txt": b"x"})
        with self.assertRaises(SystemExit):
            sj.delete(self.root, junk)
        cut = os.path.join(self.root, "org", "cut")
        make_model(cut, {".cache/huggingface/download/w.safetensors.abc.incomplete": b"x"})
        sj.delete(self.root, cut)
        self.assertFalse(os.path.exists(cut), "hf's unfinished files alone still make it a folder Delete takes")

    def test_delete_refuses_a_folder_a_job_is_still_writing(self):
        for job_id, kind in (("job-del-1", "pull"), ("job-del-2", "download")):
            folder = os.path.join(self.root, "org", job_id)
            make_model(folder, {"config.json": b"{}"})
            sj.write_status(job_id, kind=kind, state="running", pid=os.getpid())
            sj.mark_partial(folder, job_id, kind)
            with self.assertRaises(SystemExit) as refused:
                sj.delete(self.root, folder)
            self.assertEqual(str(refused.exception), "refusing to delete %s: a download or copy is still writing to it. Follow it in Jobs." % folder)
            self.assertTrue(os.path.exists(os.path.join(folder, "config.json")))
            # Once it has stopped the folder is the owner's to delete.
            sj.write_status(job_id, state="failed")
            sj.delete(self.root, folder)
            self.assertFalse(os.path.exists(folder))


class FabricFacts(unittest.TestCase):
    """fabric() reads a tree laid out as a node's sysfs is, and `ip -j -4 addr`'s real JSON."""

    def setUp(self):
        self.sys = tempfile.mkdtemp(dir=TMP_HOME)
        self.bin = tempfile.mkdtemp(dir=TMP_HOME)
        self.addrs = []
        self.real, sj.SYS = sj.SYS, self.sys
        self.path = os.environ["PATH"]
        os.environ["PATH"] = self.bin + os.pathsep + self.path

    def tearDown(self):
        sj.SYS = self.real
        os.environ["PATH"] = self.path
        shutil.rmtree(self.sys, ignore_errors=True)
        shutil.rmtree(self.bin, ignore_errors=True)

    def nic(self, name, ip=None, driver="mlx5_core", rdma=None, speed="200000", mtu="9000", operstate="up", state="4: ACTIVE", phys="5: LinkUp"):
        base = os.path.join(self.sys, "class", "net", name)
        os.makedirs(os.path.join(base, "device"))
        target = os.path.join(self.sys, "bus", "drivers", driver)
        os.makedirs(target, exist_ok=True)
        os.symlink(target, os.path.join(base, "device", "driver"))
        for file, text in (("speed", speed), ("mtu", mtu), ("operstate", operstate)):
            if text is not None:
                with open(os.path.join(base, file), "w") as f:
                    f.write(text + "\n")
        if rdma:
            os.makedirs(os.path.join(base, "device", "infiniband", rdma))
            port = os.path.join(self.sys, "class", "infiniband", rdma, "ports", "1")
            os.makedirs(port)
            for file, text in (("state", state), ("phys_state", phys)):
                with open(os.path.join(port, file), "w") as f:
                    f.write(text + "\n")
        self.addrs.append({"ifname": name, "addr_info": [{"family": "inet", "local": ip, "prefixlen": 24}] if ip else []})
        stand_in(self.bin, "ip", "#!/bin/sh\ncat <<'EOF'\n%s\nEOF\n" % json.dumps(self.addrs))

    def test_each_interface_says_its_mtu_state_and_rdma_port(self):
        self.nic("enp1s0f1np1", "192.168.100.1", rdma="rocep1s0f1")
        self.nic("enP2p1s0f1np1", "192.168.101.1", rdma="roceP2p1s0f1", mtu="1500", state="1: DOWN", phys="3: Disabled", operstate="down")
        got = sj.fabric()
        self.assertEqual(got, [
            {"iface": "enp1s0f1np1", "ip": "192.168.100.1", "prefix": 24, "speedMbps": 200000, "rdma": "rocep1s0f1",
             "mtu": 9000, "operstate": "up", "rdmaState": "4: ACTIVE", "rdmaPhys": "5: LinkUp"},
            {"iface": "enP2p1s0f1np1", "ip": "192.168.101.1", "prefix": 24, "speedMbps": 200000, "rdma": "roceP2p1s0f1",
             "mtu": 1500, "operstate": "down", "rdmaState": "1: DOWN", "rdmaPhys": "3: Disabled"},
        ])

    def test_the_lan_and_a_card_with_no_address_stay_out(self):
        self.nic("enP7s7", "192.168.50.51", driver="r8169", rdma=None, speed="1000", mtu="1500")
        self.nic("enp1s0f0np0", None, rdma="rocep1s0f0", speed="-1", mtu="1500", operstate="down")
        self.assertEqual(sj.fabric(), [])

    def test_what_a_down_link_cant_say_reads_as_nothing(self):
        # sysfs refuses to read a down link's speed; a port with no RDMA device has no state.
        self.nic("enp1s0f1np1", "192.168.100.1", rdma=None, speed=None)
        self.nic("enP2p1s0f1np1", "192.168.101.1", rdma="roceP2p1s0f1", speed="-1")
        got = sj.fabric()
        self.assertEqual((got[0]["speedMbps"], got[0]["rdma"], got[0]["rdmaState"], got[0]["rdmaPhys"]), (0, "", None, None))
        self.assertEqual((got[1]["speedMbps"], got[1]["rdmaState"]), (-1, "4: ACTIVE"))

    def test_an_mtu_or_state_that_cant_be_read_is_none_not_a_guess(self):
        self.nic("enp1s0f1np1", "192.168.100.1", rdma="rocep1s0f1", mtu=None, operstate=None)
        got = sj.fabric()[0]
        self.assertEqual((got["mtu"], got["operstate"], got["rdmaState"]), (None, None, "4: ACTIVE"))

    def test_the_quick_check_carries_them(self):
        self.nic("enp1s0f1np1", "192.168.100.1", rdma="rocep1s0f1")
        got = sj.check(self.sys, "vllm:img", "vllm:img-ray", quick=True)
        self.assertEqual(got["fabric"][0]["rdmaState"], "4: ACTIVE")
        self.assertEqual(got["fabric"][0]["mtu"], 9000)


class ProcessFacts(unittest.TestCase):
    """stat and cmdline as Linux writes them. check.sh runs the real-/proc test under Linux."""

    def setUp(self):
        self.proc = tempfile.mkdtemp(dir=TMP_HOME)
        self.real, sj.PROC = sj.PROC, self.proc

    def tearDown(self):
        sj.PROC = self.real
        shutil.rmtree(self.proc, ignore_errors=True)

    def process(self, pid, comm, state, start, argv):
        base = os.path.join(self.proc, str(pid))
        os.makedirs(base, exist_ok=True)
        tail = [state, "1", "2", "3"] + ["0"] * 15 + [str(start)] + ["0"] * 30
        with open(os.path.join(base, "stat"), "wb") as f:
            f.write(("%d (%s) %s\n" % (pid, comm, " ".join(tail))).encode())
        with open(os.path.join(base, "cmdline"), "wb") as f:
            f.write(b"".join(w.encode() + b"\0" for w in argv))

    def test_the_start_time_is_field_22_counted_past_the_name(self):
        self.process(4242, "rsync", "S", 88123, ["rsync", "-aL", "/a/", "/b/"])
        self.process(4243, "odd) name (x", "R", 91, ["python3", "two words"])
        got = LINUX_FACTS(4242)
        self.assertEqual((got["start"], got["argv"]), (88123, ["rsync", "-aL", "/a/", "/b/"]))
        self.assertEqual(got["uid"], os.getuid())
        got = LINUX_FACTS(4243)
        self.assertEqual((got["start"], got["argv"]), (91, ["python3", "two words"]))

    def test_a_process_thats_gone_or_a_zombie_is_none(self):
        self.process(4244, "rsync", "Z", 5, [])
        self.process(4245, "rsync", "X", 5, [])
        self.assertIsNone(LINUX_FACTS(4244))
        self.assertIsNone(LINUX_FACTS(4245))
        self.assertIsNone(LINUX_FACTS(9999))

    def test_every_process_is_listed_and_only_processes(self):
        self.process(4246, "rsync", "S", 5, ["rsync"])
        os.mkdir(os.path.join(self.proc, "sys"))
        with open(os.path.join(self.proc, "meminfo"), "w") as f:
            f.write("x")
        self.assertEqual(LINUX_IDS(), [4246])
        sj.PROC = os.path.join(self.proc, "no-such-folder")
        self.assertEqual(LINUX_IDS(), [])

    @unittest.skipUnless(os.path.isdir("/proc/self"), "no /proc here")
    def test_the_real_proc(self):
        sj.PROC = self.real
        sleeper = subprocess.Popen(["sleep", "30"])
        try:
            with open("/proc/%d/stat" % sleeper.pid) as f:
                field22 = int(f.read().split()[21])
            got = LINUX_FACTS(sleeper.pid)
            self.assertEqual((got["start"], got["argv"], got["uid"]), (field22, ["sleep", "30"], os.getuid()))
            self.assertIn(sleeper.pid, LINUX_IDS())
        finally:
            sleeper.kill()
            sleeper.wait()
        self.assertIsNone(LINUX_FACTS(sleeper.pid))


LIFE_S = 300


class Orphans(unittest.TestCase):
    """What a SIGKILLed job leaves. The stand-in rsync is two processes with one command line, as a
    real one is (generator and receiver), and has `rsync` as its argv[0]; each writes to the folder
    until it's stopped, or LIFE_S is up: a run that's killed outright has no tearDown to stop it.
    Once self.quick() is on, the next rsync says who else is running, and ends."""

    REFUSED = "An earlier copy may still be writing to %s."

    def setUp(self):
        self.root = tempfile.mkdtemp(dir=TMP_HOME)
        self.dest = os.path.join(self.root, "org", "m")
        os.makedirs(self.dest)
        self.bin = tempfile.mkdtemp(dir=TMP_HOME)
        self.rsync(LIFE_S)
        self.sleepers = []

    def rsync(self, life):
        # macOS's bash 3.2 has no $BASHPID, so the parent says who the second process is.
        code = """
[ -e STATE/stubborn ] && trap "" TERM
echo $$ >> STATE/pids
( while [ $SECONDS -lt LIFE ]; do : >> "${@: -1}b"; sleep 0.05; done ) &
echo $! >> STATE/pids
while [ $SECONDS -lt LIFE ]; do : >> "${@: -1}a"; sleep 0.05; done
""".replace("LIFE", str(life))
        stand_in(self.bin, "rsync", """#!/bin/bash
if [ -e STATE/quick ]; then
  echo ran >> STATE/quick-ran
  for p in $(cat STATE/pids 2>/dev/null); do kill -0 $p 2>/dev/null && echo $p >> STATE/beside; done
  exit 0
fi
exec -a rsync /bin/bash -c '%s' stand-in "$@"
""".replace("STATE", self.bin) % code.replace("STATE", self.bin))

    def tearDown(self):
        end_stand_ins(self.bin)
        for proc in self.sleepers:
            proc.kill()
            proc.wait()
        shutil.rmtree(self.root, ignore_errors=True)
        shutil.rmtree(self.bin, ignore_errors=True)

    def file(self, name):
        return os.path.join(self.bin, name)

    def pids(self):
        try:
            with open(self.file("pids")) as f:
                return [int(n) for n in f.read().split()]
        except OSError:
            return []

    def pull(self, job_id, instance="dev"):
        """A pull as `start` runs it. It's a child of this process, and of this process's facts."""
        path = os.environ["PATH"]
        os.environ["PATH"] = self.bin + os.pathsep + path
        try:
            sj.start("pull", job_id, {"src": "rsync://sparkbox@127.0.0.1:1/model", "dest": self.dest, "password": "p", "files": known_empty_file(self.dest), "instance": instance})
        finally:
            os.environ["PATH"] = path

    def writing(self, job_id):
        """Starts a pull and waits until both processes of its rsync write to the folder."""
        self.pull(job_id)
        for name in ("a", "b"):
            wait_for("the copy writing " + name, lambda: os.path.exists(os.path.join(self.dest, name)))
        self.assertEqual(len(self.pids()), 2, "both processes are known")
        wait_for("its record", lambda: sj.read_status(job_id).get("childPid"))
        return sj.read_status(job_id)

    def killed(self, job_id):
        """SIGKILL to the job alone, as the OOM killer sends it: its rsync runs on."""
        st = self.writing(job_id)
        os.kill(st["pid"], signal.SIGKILL)
        os.waitpid(st["pid"], 0)
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()), "the job's rsync outlives it")
        return st

    def quick(self):
        open(self.file("quick"), "w").close()

    def still_grows(self):
        path = os.path.join(self.dest, "a")
        os.unlink(path)
        wait_for("the old copy to go on", lambda: os.path.exists(path))

    def sleeper(self):
        proc = subprocess.Popen(["sleep", "60"])
        self.sleepers.append(proc)
        return proc

    def test_an_orphan_in_another_process_group_is_never_signaled(self):
        st = {"pid": 100, "childPid": 101, "instance": "dev"}
        with mock.patch.object(sj, "orphan", return_value={"uid": os.getuid()}), \
                mock.patch.object(os, "getpgid", return_value=200), \
                mock.patch.object(sj, "group_gone", return_value=True), \
                mock.patch.object(os, "killpg") as kill:
            with self.assertRaisesRegex(RuntimeError, "earlier copy may still be writing"):
                sj.stop_orphan(st, self.dest, "dev")
            kill.assert_not_called()

    def test_a_new_pull_stops_what_a_killed_pull_left_and_only_then_starts(self):
        st = self.killed("orphan-1")
        self.assertIsNotNone(st["childStart"])
        self.assertEqual(st["childStart"], sj.process_facts(st["childPid"])["start"])
        self.quick()
        self.pull("orphan-2")
        got = wait_done("orphan-2")
        self.assertEqual(got["state"], "done", got.get("message"))
        self.assertTrue(os.path.exists(self.file("quick-ran")), "the new copy ran")
        self.assertFalse(os.path.exists(self.file("beside")), "the old copy was gone before the new one started")
        wait_for("both old processes to go", lambda: not any(sj.process_facts(p) for p in self.pids()))
        self.assertEqual(sj.read_manifest(self.dest)["files"], [{"path":"disk-fixture","size":0}])

    def test_a_copy_that_ignores_term_goes_at_kill_after_five_seconds(self):
        self.assertEqual((sj.ORPHAN_TERM_S, sj.ORPHAN_KILL_S), (5, 2))
        open(self.file("stubborn"), "w").close()
        self.killed("orphan-3")
        self.quick()
        began = time.time()
        self.pull("orphan-4")
        got = wait_done("orphan-4")
        self.assertEqual(got["state"], "done", got.get("message"))
        self.assertGreaterEqual(time.time() - began, 5, "it was given its five seconds")
        self.assertFalse(os.path.exists(self.file("beside")))
        self.assertTrue(os.path.exists(self.file("quick-ran")))

    def test_a_kill_that_cant_be_confirmed_refuses(self):
        self.killed("orphan-5")
        self.quick()
        real = sj.group_gone, os.killpg, sj.ORPHAN_TERM_S, sj.ORPHAN_KILL_S
        sj.group_gone, os.killpg, sj.ORPHAN_TERM_S, sj.ORPHAN_KILL_S = (lambda _: False), (lambda *_: None), 0.1, 0.1
        try:
            self.pull("orphan-6")
            got = wait_done("orphan-6")
        finally:
            sj.group_gone, os.killpg, sj.ORPHAN_TERM_S, sj.ORPHAN_KILL_S = real
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")), "never a second writer")
        self.assertEqual(sj.read_marker(self.dest)["job"], "orphan-5", "the folder still names the job that left it")
        self.still_grows()

    def test_a_recycled_pid_that_isnt_rsync_is_left_alone(self):
        for job_id, started in (("orphan-7", 1), ("orphan-7b", None)):
            other = self.sleeper()
            start = sj.process_facts(other.pid)["start"] if started is None else started
            sj.write_status(job_id, kind="pull", state="running", pid=999999, childPid=other.pid, childStart=start, dest=self.dest, instance="dev")
            sj.mark_partial(self.dest, job_id, "pull")
            self.quick()
            next_job = job_id + "-next"
            self.pull(next_job)
            got = wait_done(next_job)
            self.assertEqual(got["state"], "done", got.get("message"))
            self.assertIsNone(other.poll(), "whatever has that pid now is none of this job's business")
            other.kill()

    def test_a_recycled_pid_thats_an_rsync_on_this_folder_is_not_signalled_and_refuses(self):
        self.killed("orphan-8")
        sj.write_status("orphan-8", childStart=sj.read_status("orphan-8")["childStart"] + 1 if isinstance(sj.read_status("orphan-8")["childStart"], int) else "Sat Jan  1 00:00:00 2000")
        self.quick()
        self.pull("orphan-9")
        got = wait_done("orphan-9")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()), "nothing the record can't vouch for is signalled")

    def test_another_users_copy_is_not_signalled_and_refuses(self):
        st = self.killed("orphan-10")
        real = sj.process_facts
        sj.process_facts = lambda pid: dict(real(pid), uid=os.getuid() + 1) if pid == st["childPid"] and real(pid) else real(pid)
        self.quick()
        try:
            self.pull("orphan-11")
            got = wait_done("orphan-11")
        finally:
            sj.process_facts = real
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()))

    def test_the_other_sparkboxs_copy_is_not_signalled_and_refuses(self):
        self.killed("orphan-12")
        self.quick()
        self.pull("orphan-13", instance="release")
        got = wait_done("orphan-13")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()))

    def test_with_the_marker_gone_a_writer_still_refuses_and_stays(self):
        self.killed("orphan-14")
        os.unlink(os.path.join(self.dest, sj.PARTIAL))
        self.quick()
        self.pull("orphan-15")
        got = wait_done("orphan-15")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()))

    def test_a_writer_with_no_child_on_record_refuses(self):
        # The job died between starting rsync and recording it.
        self.killed("orphan-16")
        with open(sj.status_path("orphan-16")) as f:
            st = json.load(f)
        del st["childPid"], st["childStart"]
        with open(sj.status_path("orphan-16"), "w") as f:
            json.dump(st, f)
        self.quick()
        self.pull("orphan-17")
        got = wait_done("orphan-17")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))

    def test_a_copy_with_no_start_time_on_record_is_not_signalled_and_refuses(self):
        # A job an older sparkjob.py started: nothing tells its child from a later process.
        self.killed("orphan-18")
        sj.write_status("orphan-18", childStart=None)
        self.quick()
        self.pull("orphan-19")
        got = wait_done("orphan-19")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()))

    def test_a_live_copy_refuses_a_second_one(self):
        st = self.writing("orphan-20")
        self.assertEqual(st["pidStart"], sj.process_facts(st["pid"])["start"])
        self.quick()
        self.pull("orphan-21")
        got = wait_done("orphan-21")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertTrue(sj.alive(st["pid"]) and sj.job_running(sj.read_status("orphan-20")))
        self.assertFalse(os.path.exists(self.file("quick-ran")))
        sj.cancel("orphan-20")
        os.waitpid(st["pid"], 0)

    def test_a_live_copy_between_its_steps_refuses_a_second_one(self):
        # Checking its files: rsync has ended, and its record is yet to be written.
        sj.write_status("orphan-22", kind="pull", state="running", pid=os.getpid(), dest=self.dest)
        sj.mark_partial(self.dest, "orphan-22", "pull")
        self.quick()
        self.pull("orphan-23")
        got = wait_done("orphan-23")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))
        self.assertFalse(os.path.exists(self.file("quick-ran")))

    def test_a_live_download_refuses_a_second_writer_too(self):
        sj.write_status("orphan-29", kind="download", state="running", pid=os.getpid(), dest=self.dest, instance="dev")
        sj.mark_partial(self.dest, "orphan-29", "download")
        self.quick()
        self.pull("orphan-30")
        got = wait_done("orphan-30")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))

    def test_of_two_copies_started_together_one_refuses(self):
        # Each looks at the folder, finds it free and takes a moment to mark it.
        self.quick()
        slow = sj.mark_partial

        def marking(*args):
            time.sleep(0.4)
            slow(*args)

        sj.mark_partial = marking
        try:
            self.pull("orphan-31")
            self.pull("orphan-32")
        finally:
            sj.mark_partial = slow
        states = sorted((wait_done("orphan-31")["state"], wait_done("orphan-32")["state"]))
        self.assertEqual(states, ["done", "failed"], "one writer")
        with open(self.file("quick-ran")) as f:
            self.assertEqual(f.read().split(), ["ran"], "one rsync")

    def test_list_reports_an_orphan_and_stops_nothing(self):
        st = self.killed("orphan-24")
        stopped = "The job stopped unexpectedly (node restarted?)."
        says = stopped + " Its copy is still running: a new copy stops it first."
        by = {j["id"]: j for j in sj.list_jobs(look=True)}
        self.assertEqual((by["orphan-24"]["state"], by["orphan-24"]["orphan"], by["orphan-24"]["message"]), ("failed", True, says))
        self.assertEqual(sj.read_status("orphan-24")["state"], "running", "--look changes nothing")
        by = {j["id"]: j for j in sj.list_jobs()}
        self.assertEqual((by["orphan-24"]["state"], by["orphan-24"]["orphan"], by["orphan-24"]["message"]), ("failed", True, says))
        by = {j["id"]: j for j in sj.list_jobs()}
        self.assertEqual((by["orphan-24"]["orphan"], by["orphan-24"]["message"]), (True, says), "a job already marked failed still says so")
        self.assertNotIn("orphan", sj.read_status("orphan-24"), "a fact of the moment isn't kept")
        self.assertEqual(sj.read_status("orphan-24")["message"], stopped, "a fact of the moment isn't kept")
        self.assertTrue(all(sj.process_facts(p) for p in self.pids()), "list stops nothing")
        self.still_grows()
        os.killpg(st["pid"], signal.SIGKILL)
        wait_for("the orphan to go", lambda: not any(sj.process_facts(p) for p in self.pids()))
        by = {j["id"]: j for j in sj.list_jobs()}
        self.assertNotIn("orphan", by["orphan-24"])
        self.assertEqual(by["orphan-24"]["message"], stopped)

    def test_a_job_that_ended_another_way_says_so_too(self):
        self.assertEqual(sj.with_orphan({"kind": "pull", "message": "Canceled"})["message"], "Canceled. Its copy is still running: a new copy stops it first.")
        self.assertEqual(sj.with_orphan({"kind": "pull", "message": ""})["message"], "Its copy is still running: a new copy stops it first.")
        self.assertEqual(sj.with_orphan({"kind": "download", "message": "Canceled"})["message"], "Canceled. Its container may still be running: Resume download stops it first.")
        said = "Canceled, but its container may still be running: Resume download stops it first."
        self.assertEqual(sj.with_orphan({"kind": "download", "message": said})["message"], said, "once is enough")

    def test_list_doesnt_call_a_used_again_pid_an_orphan(self):
        other = self.sleeper()
        sj.write_status("orphan-25", kind="pull", state="failed", pid=999999, childPid=other.pid, childStart=1, dest=self.dest)
        self.assertNotIn("orphan", {j["id"]: j for j in sj.list_jobs(look=True)}["orphan-25"])
        real = sj.process_facts(other.pid)["start"]
        sj.write_status("orphan-25", childStart=real)
        self.assertNotIn("orphan", {j["id"]: j for j in sj.list_jobs(look=True)}["orphan-25"], "the same start, but no rsync on this folder")

    def test_cancel_leaves_a_used_again_pid_alone(self):
        other = self.sleeper()
        sj.write_status("orphan-26", kind="pull", state="running", pid=999999, childPid=other.pid, childStart=1, dest=self.dest)
        sj.cancel("orphan-26")
        time.sleep(0.2)
        self.assertIsNone(other.poll())

    def test_cancel_leaves_a_process_using_the_jobs_own_pid_again_alone(self):
        # The job died with its status at running (a reboot), and its pid is a group leader's now.
        other = subprocess.Popen(["sleep", "60"], start_new_session=True)
        self.sleepers.append(other)
        sj.write_status("orphan-34", kind="pull", state="running", pid=other.pid, pidStart=1, dest=self.dest)
        wait, sj.CANCEL_WAIT_S = sj.CANCEL_WAIT_S, 30
        began = time.time()
        try:
            sj.cancel("orphan-34")
        finally:
            sj.CANCEL_WAIT_S = wait
        self.assertLess(time.time() - began, 20, "it doesn't wait for a group that isn't the job's")
        time.sleep(0.2)
        self.assertIsNone(other.poll())

    def test_cancel_still_reaches_a_job_with_no_start_time_on_record(self):
        # A job an older sparkjob.py started, which only has its pid.
        other = self.sleeper()
        sj.write_status("orphan-35", kind="pull", state="running", pid=other.pid, dest=self.dest)
        sj.cancel("orphan-35")
        wait_for("the job to stop", lambda: other.poll() is not None)

    def test_a_process_using_a_jobs_pid_again_holds_no_folder(self):
        other = self.sleeper()
        sj.write_status("orphan-36", kind="pull", state="running", pid=other.pid, pidStart=1, dest=self.dest, instance="dev")
        sj.mark_partial(self.dest, "orphan-36", "pull")
        self.assertFalse(sj.job_running(sj.read_status("orphan-36")))
        self.assertEqual(sj.scan([self.root])[0]["partial"], "stopped")
        self.quick()
        self.pull("orphan-37")
        got = wait_done("orphan-37")
        self.assertEqual(got["state"], "done", got.get("message"))
        self.assertIsNone(other.poll(), "whatever has that pid now is none of this job's business")

    def test_a_job_whose_pid_is_its_own_still_holds_the_folder(self):
        other = self.sleeper()
        start = sj.process_facts(other.pid)["start"]
        sj.write_status("orphan-38", kind="pull", state="running", pid=other.pid, pidStart=start, dest=self.dest, instance="dev")
        sj.mark_partial(self.dest, "orphan-38", "pull")
        self.assertTrue(sj.job_running(sj.read_status("orphan-38")))
        self.quick()
        self.pull("orphan-39")
        got = wait_done("orphan-39")
        self.assertEqual((got["state"], got["message"]), ("failed", self.REFUSED % self.dest))

    def test_with_no_start_time_on_record_a_pid_in_use_still_holds_the_folder(self):
        other = self.sleeper()
        sj.write_status("orphan-40", kind="pull", state="running", pid=other.pid, dest=self.dest, instance="dev")
        self.assertTrue(sj.job_running(sj.read_status("orphan-40")))

    def test_delete_refuses_while_a_dead_jobs_copy_writes_the_folder(self):
        st = self.killed("orphan-41")
        self.assertEqual(sj.scan([self.root])[0]["partial"], "stopped", "the job is over")
        with self.assertRaises(SystemExit) as refused:
            sj.delete(self.root, self.dest)
        self.assertEqual(str(refused.exception), self.REFUSED % self.dest)
        self.still_grows()
        os.killpg(st["pid"], signal.SIGKILL)
        wait_for("the orphan to go", lambda: not any(sj.process_facts(p) for p in self.pids()))
        self.assertEqual(sj.delete(self.root, self.dest), {"deleted": os.path.realpath(self.dest)})
        self.assertFalse(os.path.exists(self.dest))

    def test_delete_refuses_a_writer_with_the_marker_gone(self):
        self.killed("orphan-42")
        os.unlink(os.path.join(self.dest, sj.PARTIAL))
        make_model(self.dest, {"config.json": b"{}"})
        with self.assertRaises(SystemExit) as refused:
            sj.delete(self.root, self.dest)
        self.assertEqual(str(refused.exception), self.REFUSED % self.dest)
        self.assertTrue(os.path.isdir(self.dest))

    def test_a_stand_in_ends_by_itself(self):
        # What a run killed outright leaves: nothing else would ever end it.
        self.rsync(1)
        self.pull("orphan-43")
        wait_for("both processes", lambda: len(self.pids()) == 2)
        wait_for("the stand-in to end", lambda: not marked(self.bin), timeout=30)
        self.assertEqual(wait_done("orphan-43")["state"], "done")

    def test_cancel_still_reaches_a_child_with_no_start_time_on_record(self):
        # A job an older sparkjob.py started, which only has its pid.
        other = self.sleeper()
        sj.write_status("orphan-27", kind="pull", state="running", pid=999999, childPid=other.pid, dest=self.dest)
        sj.cancel("orphan-27")
        wait_for("the child to stop", lambda: other.poll() is not None)

    def test_cancel_reaches_an_orphan_through_its_group(self):
        st = self.killed("orphan-28")
        sj.cancel("orphan-28")
        wait_for("the orphan to go", lambda: not any(sj.process_facts(p) for p in self.pids()))
        self.assertEqual(sj.read_status("orphan-28")["state"], "cancelled")
        self.assertTrue(st["pid"])

    def test_a_serve_jobs_rsync_is_no_writer(self):
        self.assertFalse(sj.rsync_to(["rsync", "--daemon", "--no-detach", "--config=/x", self.dest + "/"], os.path.realpath(self.dest)))
        self.assertFalse(sj.rsync_to(["rsync", "--server", "--sender", "-vlogDtpr", ".", self.dest + "/"], os.path.realpath(self.dest)))
        self.assertFalse(sj.rsync_to(["rsync", "-a", self.dest + "/", "/elsewhere/"], os.path.realpath(self.dest)))
        self.assertFalse(sj.rsync_to(["cp", "-a", "/x", self.dest + "/"], os.path.realpath(self.dest)))
        self.assertTrue(sj.rsync_to(["/usr/bin/rsync", "-aL", "/x/", self.dest + "//"], os.path.realpath(self.dest)))


def known_empty_file(dest):
    # Lifecycle fixtures now supply known sizes; their process/ownership assertions are unchanged.
    os.makedirs(dest, exist_ok=True)
    with open(os.path.join(dest, "disk-fixture"), "wb") as f:
        f.write(b"")
    return [{"path":"disk-fixture", "size":0}]


class DownloadOrphans(unittest.TestCase):
    """The same hole on the download path. A SIGKILLed job leaves `docker run`, and a CLI that hasn't
    got its container yet is invisible to `docker ps`: stop_leftover looks, finds nothing and goes
    ahead, and then the old CLI starts a second writer. This stand-in's first `run` waits for a file
    before its container exists, as a CLI does between its start and the daemon's answer, for
    LIFE_S at most."""

    def setUp(self):
        self.bin = tempfile.mkdtemp(dir=TMP_HOME)
        self.dest = os.path.join(TMP_HOME, "dl-orphan-" + os.urandom(3).hex())
        code = """
echo $$ > STATE/cli
while [ ! -e STATE/go ] && [ $SECONDS -lt LIFE ]; do sleep 0.05; done
echo old >> STATE/started
""".replace("LIFE", str(LIFE_S))
        stand_in(self.bin, "docker", """#!/bin/bash
case "$1" in
  image) echo '{"ID":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","Repository":"vllm","Tag":"test"}' ;;
  run)
    if [ "${!#}" = version ]; then echo "hf 2.4.0"; exit 0; fi
    if [ ! -e STATE/first ]; then
      touch STATE/first
      exec -a docker /bin/bash -c '%s' stand-in "$@"
    fi
    echo new >> STATE/started ;;
esac
exit 0
""".replace("STATE", self.bin) % code.replace("STATE", self.bin))

    def tearDown(self):
        end_stand_ins(self.bin)
        shutil.rmtree(self.bin, ignore_errors=True)
        shutil.rmtree(self.dest, ignore_errors=True)

    def file(self, name):
        return os.path.join(self.bin, name)

    def download(self, job_id, instance="dev"):
        path = os.environ["PATH"]
        os.environ["PATH"] = self.bin + os.pathsep + path
        try:
            sj.start("download", job_id, {"repo": "org/m", "revision": "abc", "dest": self.dest, "image": "vllm:x", "tool": image_tool(), "instance": instance, "files": known_empty_file(self.dest)})
        finally:
            os.environ["PATH"] = path

    def killed(self, job_id):
        self.download(job_id)
        wait_for("the CLI's pid", lambda: os.path.exists(self.file("cli")) and os.path.getsize(self.file("cli")) > 0)
        wait_for("its record", lambda: sj.read_status(job_id).get("childPid"))
        st = sj.read_status(job_id)
        os.kill(st["pid"], signal.SIGKILL)
        os.waitpid(st["pid"], 0)
        with open(self.file("cli")) as f:
            self.cli = int(f.read())
        self.assertTrue(sj.process_facts(self.cli), "the CLI outlives the job")
        return st

    def test_a_retry_stops_a_killed_jobs_cli_before_its_container_exists(self):
        self.killed("dlorphan-1")
        self.download("dlorphan-2")
        got = wait_done("dlorphan-2")
        self.assertEqual(got["state"], "done", got.get("message"))
        self.assertIsNone(sj.process_facts(self.cli), "the old CLI went first")
        open(self.file("go"), "w").close()
        time.sleep(0.3)
        with open(self.file("started")) as f:
            self.assertEqual(f.read().split(), ["new"], "the old CLI never started a second writer")

    def test_list_says_so_and_stops_nothing(self):
        self.killed("dlorphan-3")
        by = {j["id"]: j for j in sj.list_jobs()}
        self.assertEqual((by["dlorphan-3"]["state"], by["dlorphan-3"]["orphan"]), ("failed", True))
        self.assertEqual(by["dlorphan-3"]["message"].count("Resume download stops it first."), 1)
        self.assertTrue(sj.process_facts(self.cli))

    def test_delete_refuses_while_a_killed_jobs_cli_is_there(self):
        self.killed("dlorphan-8")
        with self.assertRaises(SystemExit) as refused:
            sj.delete(os.path.dirname(self.dest), self.dest)
        self.assertEqual(str(refused.exception), "An earlier copy may still be writing to %s." % self.dest)
        self.assertTrue(os.path.isdir(self.dest))
        self.assertTrue(sj.process_facts(self.cli), "delete stops nothing")

    def test_the_other_sparkboxs_cli_refuses_and_stays(self):
        self.killed("dlorphan-4")
        self.download("dlorphan-5", instance="release")
        got = wait_done("dlorphan-5")
        self.assertEqual((got["state"], got["message"]), ("failed", "An earlier copy may still be writing to %s." % self.dest))
        self.assertTrue(sj.process_facts(self.cli), "not this SparkBox's to stop")
        self.assertFalse(os.path.exists(self.file("started")))

    def test_a_cli_with_no_child_on_record_refuses(self):
        self.killed("dlorphan-6")
        with open(sj.status_path("dlorphan-6")) as f:
            st = json.load(f)
        del st["childPid"], st["childStart"]
        with open(sj.status_path("dlorphan-6"), "w") as f:
            json.dump(st, f)
        self.download("dlorphan-7")
        got = wait_done("dlorphan-7")
        self.assertEqual((got["state"], got["message"]), ("failed", "An earlier copy may still be writing to %s." % self.dest))
        self.assertTrue(sj.process_facts(self.cli))


def deploy(load, role, payload, umask=None):
    """The real command line, its JSON on stdin."""
    text = payload if isinstance(payload, str) else json.dumps(payload)
    return subprocess.run([sys.executable, "-I", HERE, "deploy-files", load, role], env=dict(os.environ, HOME=TMP_HOME), input=text, capture_output=True, text=True,
                          preexec_fn=(lambda: os.umask(umask)) if umask is not None else None)


class DeployFiles(unittest.TestCase):
    """Contract C-F, on real files in a temporary home."""

    ARGV = ["vllm", "serve", "/m", "--served-model-name", "m", "--port", "8000", "--speculative-config", '{"a": "b c"}', "--trust-remote-code"]
    ENV = {"HF_HUB_OFFLINE": "1", "VLLM_ENGINE_READY_TIMEOUT_S": "3600", "NCCL_SOCKET_IFNAME": "enp1s0f1np1", "VLLM_HOST_IP": "192.168.100.1",
           "GLOO_SOCKET_IFNAME": "enp1s0f1np1", "TP_SOCKET_IFNAME": "enp1s0f1np1", "UCX_NET_DEVICES": "enp1s0f1np1",
           "OMPI_MCA_btl_tcp_if_include": "enp1s0f1np1", "NCCL_IB_HCA": "rocep1s0f1,roceP2p1s0f1", "NCCL_IB_DISABLE": "0",
           "NCCL_IGNORE_CPU_AFFINITY": "1", "NCCL_DEBUG": "INFO", "CUTE_DSL_ARCH": "sm_121a"}
    KEY = "sbk-" + "ab12" * 16

    def setUp(self):
        self.load = "d" + os.urandom(4).hex()
        self.base = os.path.join(sj.DEPLOY, self.load)
        self.key_file = os.path.join(sj.KEYS, self.load)

    def payload(self, **over):
        return dict({"argv": self.ARGV, "env": self.ENV, "key": None}, **over)

    def mode(self, path):
        return stat_mode(path)

    def refused(self, r, words):
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertIn(words, r.stderr)
        self.assertEqual(r.stdout, "")
        self.assertNotIn("Traceback", r.stderr)
        self.assertFalse(os.path.exists(self.base), "nothing is written")
        self.assertFalse(os.path.exists(self.key_file), "nothing is written")

    def test_the_files_their_modes_and_what_it_prints(self):
        r = deploy(self.load, "head", self.payload(key=self.KEY))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout), {"dir": self.base})
        self.assertEqual((self.mode(self.base), self.mode(sj.DEPLOY)), (0o700, 0o700))
        for name, want in (("argv.json", self.ARGV), ("env.json", self.ENV)):
            path = os.path.join(self.base, name)
            self.assertEqual(self.mode(path), 0o600, name)
            with open(path) as f:
                self.assertEqual(json.load(f), want)
        self.assertEqual((self.mode(self.key_file), self.mode(sj.KEYS)), (0o400, 0o700))
        with open(self.key_file) as f:
            self.assertEqual(f.read(), self.KEY, "exactly the key")
        self.assertEqual(sorted(os.listdir(self.base)), ["argv.json", "env.json"], "no temporary file is left")

    def test_the_modes_hold_whatever_the_umask(self):
        r = deploy(self.load, "single", self.payload(key=self.KEY), umask=0)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual((self.mode(self.base), self.mode(os.path.join(self.base, "env.json")), self.mode(self.key_file)), (0o700, 0o600, 0o400))

    def test_an_older_folder_is_made_private(self):
        os.makedirs(self.base, mode=0o755)
        os.chmod(self.base, 0o755)
        self.assertEqual(deploy(self.load, "single", self.payload()).returncode, 0)
        self.assertEqual(self.mode(self.base), 0o700)

    def test_the_cache_folders_are_the_cluster_users(self):
        for folder in (".cache/vllm", ".cache/flashinfer", ".triton"):
            shutil.rmtree(os.path.join(TMP_HOME, folder), ignore_errors=True)
        self.assertEqual(deploy(self.load, "worker", self.payload()).returncode, 0)
        for folder in (".cache/vllm", ".cache/flashinfer", ".triton"):
            path = os.path.join(TMP_HOME, folder)
            self.assertEqual((os.stat(path).st_uid, stat_mode(path)), (os.getuid(), 0o700), folder)

    def test_a_folder_already_there_keeps_what_it_holds(self):
        cache = os.path.join(TMP_HOME, ".triton")
        os.makedirs(cache, exist_ok=True)
        keep = os.path.join(cache, "kernel.o")
        open(keep, "w").close()
        self.assertEqual(deploy(self.load, "worker", self.payload()).returncode, 0)
        self.assertTrue(os.path.exists(keep))

    def test_a_second_run_replaces_the_files_and_the_key(self):
        self.assertEqual(deploy(self.load, "head", self.payload(key=self.KEY)).returncode, 0)
        argv = self.ARGV[:3]
        r = deploy(self.load, "head", self.payload(argv=argv, key="sbk-second"))
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.key_file) as f:
            self.assertEqual(f.read(), "sbk-second")
        self.assertEqual(self.mode(self.key_file), 0o400, "a new file, not the old one opened again")
        with open(os.path.join(self.base, "argv.json")) as f:
            self.assertEqual(json.load(f), argv)

    def test_no_key_leaves_no_old_key(self):
        self.assertEqual(deploy(self.load, "single", self.payload(key=self.KEY)).returncode, 0)
        self.assertEqual(deploy(self.load, "single", self.payload()).returncode, 0)
        self.assertFalse(os.path.exists(self.key_file))

    def test_the_folder_docker_makes_at_a_missing_mount_is_replaced(self):
        os.makedirs(self.key_file)
        self.assertEqual(deploy(self.load, "single", self.payload(key=self.KEY)).returncode, 0)
        self.assertEqual(self.mode(self.key_file), 0o400)

    def test_the_key_is_in_no_output_status_or_launch_file(self):
        r = deploy(self.load, "head", self.payload(key=self.KEY))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn(self.KEY, r.stdout + r.stderr)
        for folder in (self.base, sj.JOBS):
            for name in (os.listdir(folder) if os.path.isdir(folder) else []):
                path = os.path.join(folder, name)
                if os.path.isfile(path):
                    with open(path, "rb") as f:
                        self.assertNotIn(self.KEY.encode(), f.read(), path)

    def test_a_refusal_repeats_nothing_it_was_given(self):
        secret = "sbk-should-not-show"
        for payload in (self.payload(argv=["vllm", "serve", "--api-key=" + secret]), self.payload(argv=["sh", "-c", secret]),
                        self.payload(env={"VLLM_API_KEY": secret}), self.payload(env={"A=" + secret: "x"}), self.payload(env={"X": 1, "k": secret}),
                        self.payload(argv=["vllm", "serve", 1, secret]), self.payload(key=secret, extra=secret)):
            r = deploy(self.load, "single", payload)
            self.assertNotEqual(r.returncode, 0)
            self.assertNotIn(secret, r.stdout + r.stderr)
        r = deploy(self.load, "single", '{"argv": ["vllm", "serve"], "key": "%s"' % secret)
        self.refused(r, "needs JSON on stdin")
        self.assertNotIn(secret, r.stderr)

    def test_a_bad_id_or_role_is_refused(self):
        for bad in ("../x", "a/b", "", "a b", "é", ".", "x" * 65):
            r = deploy(bad, "single", self.payload())
            self.assertNotEqual(r.returncode, 0, bad)
            self.assertIn("bad load id", r.stderr)
        for bad in ("", "Head", "rank", "worker ", "../worker"):
            self.refused(deploy(self.load, bad, self.payload()), "bad role")

    def test_only_vllm_serve_runs(self):
        for argv in ([], ["vllm"], ["vllm", "bench"], ["sh", "-c", "id"], ["python3", "serve"], ["/usr/bin/vllm", "serve"], ["vllm", "--serve"],
                     "vllm serve", ["vllm", "serve", 1], ["vllm", "serve", "a\0b"], None):
            self.refused(deploy(self.load, "single", self.payload(argv=argv)), "vllm serve and nothing else")

    def test_a_key_on_a_worker_is_refused(self):
        self.refused(deploy(self.load, "worker", self.payload(key=self.KEY)), "A worker takes no API key")
        self.refused(deploy(self.load, "worker", self.payload(key="")), "A worker takes no API key")

    def test_a_key_thats_not_text_or_empty_is_refused(self):
        for key in ("", 7, ["k"], True, "a\0b"):
            self.refused(deploy(self.load, "head", self.payload(key=key)), "empty or isn't text")

    def test_the_keys_flag_is_refused_in_every_form(self):
        for word in ("--api-key", "--api-key=sbk-x", "--api_key", "--api_key=x", "--api-ke", "--api-k=x", "--api", "-api-key", "--api-key.x=1"):
            self.refused(deploy(self.load, "single", self.payload(argv=["vllm", "serve", "/m", word])), "can't take --api-key")
        for word in ("--hf-token", "--hf_token=x", "--hf-t"):
            self.refused(deploy(self.load, "single", self.payload(argv=["vllm", "serve", "/m", word])), "can't take --hf-token")

    def test_remote_code_is_the_plain_flag_only(self):
        for word in ("--trust-remote-code=True", "--trust-remote-code=", "--trust_remote_code=1", "--trust-remote=x", "--trust-r=x", "--trust-remote", "--trust-remote-code.x=1"):
            self.refused(deploy(self.load, "single", self.payload(argv=["vllm", "serve", "/m", word])), "can't take --trust-remote-code=...")
        self.assertEqual(deploy(self.load, "single", self.payload(argv=["vllm", "serve", "/m", "--trust-remote-code"])).returncode, 0)

    def test_what_loads_code_or_opens_files_is_refused(self):
        for word in ("--middleware", "--middleware=m.py", "--tool-parser-plugin", "--tool_parser_plugin=p.py", "--reasoning-parser-plugin=p.py", "--reasoning-parser-p",
                     "--allowed-local-media-path=/", "--logits-processors=x", "--worker-cls=x", "--worker-extension-cls=x", "--scheduler-cls=x", "--config", "--config=/tmp/x.yaml",
                     "--conf=/tmp/x.yaml", "--ssl-keyfile=/k", "--ssl_certfile=/c", "--some-new-plugin=x"):
            r = deploy(self.load, "single", self.payload(argv=["vllm", "serve", "/m", word]))
            self.refused(r, "can't take")

    def test_every_option_a_load_sends_passes(self):
        argv = ["vllm", "serve", "/m", "--served-model-name", "m", "--gpu-memory-utilization", "0.75", "--enable-expert-parallel", "--max-model-len", "32768", "--max-num-seqs", "4",
                "--kv-cache-dtype", "fp8", "--enable-prefix-caching", "--no-enable-prefix-caching", "--tool-call-parser", "hermes", "--enable-auto-tool-choice", "--reasoning-parser", "qwen3",
                "--tensor-parallel-size", "2", "--pipeline-parallel-size", "2", "--nnodes", "2", "--node-rank", "1", "--master-addr", "192.168.100.1", "--master-port", "29501",
                "--host", "0.0.0.0", "--port", "8000", "--headless", "--block-size", "256", "--enforce-eager", "--speculative-config", '{"method": "mtp"}', "--temperature", "-1",
                "--api-server-count", "2", "--trust-request-chat-template", "--config-format", "hf", "--logits-processor-pattern", "x"]
        r = deploy(self.load, "single", self.payload(argv=argv))
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_env_names_outside_the_contract_are_refused(self):
        self.refused(deploy(self.load, "single", self.payload(env={"VLLM_A=B": "x"})), "A load can't set that name")
        for name in ("VLLM_API_KEY", "HF_TOKEN", "NCCL_KEY", "VLLM_lower", "VLLM_TOKEN", "VLLM_DB_SECRET", "NCCL_PASSWORD", "HF_HUB_TOKEN", "OPENAI_API_KEY",
                     "PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "PYTHONPATH", "PYTHONSTARTUP", "HOME", "hf_token", "vllm_x", "BASH_ENV", "ENV", "MASTER_ADDR", "RAY_ADDRESS"):
            self.refused(deploy(self.load, "single", self.payload(env={name: "x"})), "A load can't set " + name)

    def test_env_names_that_load_code_or_write_files_are_refused(self):
        for name in ("NCCL_NET_PLUGIN", "NCCL_TUNER_PLUGIN", "NCCL_PROFILER_PLUGIN", "VLLM_PLUGINS", "VLLM_LOGGING_CONFIG_PATH", "NCCL_DEBUG_FILE"):
            self.assertFalse(sj.env_allowed(name), name)
            self.refused(deploy(self.load, "single", self.payload(env={name: "x"})), "A load can't set " + name)

    def test_env_values_must_be_text(self):
        for value in (1, None, ["x"], "a\0b"):
            self.refused(deploy(self.load, "single", self.payload(env={"NCCL_DEBUG": value})), "names and text")

    def test_the_lists_agree_with_rusts(self):
        # recipe.rs and plan.rs build the environment, and this is what they're checked against.
        here = os.path.dirname(os.path.abspath(__file__))
        with open(os.path.join(here, "..", "models", "recipe.rs")) as f:
            recipe = re.search(r"const ENV_NAMES: &\[&str\] = &\[(.*?)\];", f.read(), re.S).group(1)
        with open(os.path.join(here, "..", "models", "plan.rs")) as f:
            rank = re.search(r"pub const RANK_ENV: &\[&str\] = &\[(.*?)\];", f.read(), re.S).group(1)
        names = set(re.findall(r'"([^"]+)"', recipe + rank))
        self.assertGreater(len(names), 10)
        self.assertEqual({n for n in names if not sj.env_allowed(n)}, set(), "Rust sets a name deploy-files refuses")
        self.assertEqual(set(sj.ENV_NAMES) - names, set(), "deploy-files lets through a name Rust doesn't list")
        with open(os.path.join(here, "..", "models", "plan.rs")) as f:
            code = re.search(r"const CODE_FLAGS: &\[&str\] = &\[(.*?)\];", f.read(), re.S).group(1)
        self.assertEqual(set(re.findall(r'"([^"]+)"', code)) - {"--ssl-"}, set(sj.CODE_FLAGS), "the flags that load code or open files")

    def test_recipes_refuse_the_same_names(self):
        here = os.path.dirname(os.path.abspath(__file__))
        with open(os.path.join(here, "..", "models", "recipe.rs")) as f:
            found = re.search(r"const ENV_CODE: &\[&str\] = &\[(.*?)\];", f.read(), re.S)
        if not found:
            self.skipTest("recipe.rs has no ENV_CODE yet, so only deploy-files refuses these names")
        self.assertEqual(set(re.findall(r'"([^"]+)"', found.group(1))), set(sj.ENV_CODE))

    def test_unknown_fields_and_other_shapes_are_refused(self):
        self.refused(deploy(self.load, "single", self.payload(extra=1)), "takes argv, env and key")
        self.refused(deploy(self.load, "single", ["vllm", "serve"]), "takes argv, env and key")

    def test_the_write_is_temporary_file_then_rename(self):
        path = os.path.join(TMP_HOME, "render-" + os.urandom(3).hex() + ".json")
        sj.write_private(path, ["old"])
        seen = {}
        real = os.replace

        def spy(src, dst):
            seen.update(src=src, mode=stat_mode(src), dst=dst)
            with open(src) as f:
                seen["whole"] = json.load(f)
            real(src, dst)

        os.replace = spy
        try:
            sj.write_private(path, ["new"])
        finally:
            os.replace = real
        self.assertEqual((seen["dst"], seen["mode"], seen["whole"]), (path, 0o600, ["new"]))
        self.assertNotEqual(seen["src"], path)
        self.assertFalse(os.path.exists(seen["src"]))

    def test_a_rename_that_fails_leaves_the_old_file_and_no_temporary_one(self):
        folder = tempfile.mkdtemp(dir=TMP_HOME)
        path = os.path.join(folder, "env.json")
        sj.write_private(path, {"old": "1"})
        real = os.replace

        def fail(src, dst):
            raise OSError("no")

        os.replace = fail
        try:
            with self.assertRaises(OSError):
                sj.write_private(path, {"new": "2"})
        finally:
            os.replace = real
        self.assertEqual(os.listdir(folder), ["env.json"])
        with open(path) as f:
            self.assertEqual(json.load(f), {"old": "1"})


class P2Offline(unittest.TestCase):
    def test_import_is_literal_only_redacts_keys_and_keeps_json(self):
        config = 'IMAGE="vllm/vllm-openai:test"\nMODEL_PATH="/home/test/models/qwen"\nHF_TOKEN="do-not-return"\nOMP_NUM_THREADS=4\n'
        script = 'exec vllm serve "$MODEL_PATH" --served-model-name qwen --tensor-parallel-size 2 --max-model-len 32768 --tool-call-parser hermes --api-key "$HF_TOKEN" --speculative-config \'{"method":"mtp","num_speculative_tokens":2}\' --trust-remote-code'
        with mock.patch.object(sj.subprocess, "run", side_effect=AssertionError("import must never execute")):
            draft = sj.recipe_import_text(config, script)
        self.assertEqual(draft["tensorParallel"], 2)
        self.assertEqual(draft["modelPath"], "/home/test/models/qwen")
        self.assertEqual(draft["recipe"]["topologies"], ["tp2"])
        self.assertEqual(draft["recipe"]["args"][0]["json"]["method"], "mtp")
        self.assertNotIn("do-not-return", json.dumps(draft))
        self.assertNotIn("HF_TOKEN", json.dumps(draft))
        self.assertEqual(draft["recipe"]["env"], {"OMP_NUM_THREADS": "4"})
        self.assertTrue(any("trust-remote-code" in w for w in draft["warnings"]))

    def test_import_recorded_glm_wrapper_keeps_literal_model_settings_without_executing_arrays(self):
        with open(os.path.join(os.path.dirname(__file__), "../models/fixtures/logs/glm53-inner.sh")) as f:
            script = f.read()
        config = 'IMAGE=vllm/test:1\nMODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4\nSERVED_NAME=glm\nPORT=8000\nNNODES=4\nMAX_MODEL_LEN=32768\nMAX_NUM_SEQS=4\nGPU_MEM_UTIL=0.75\nMTP_TOKENS=2\nHEAD_IP=192.168.100.1\nMASTER_PORT=29500\n'
        with mock.patch.object(sj.subprocess, "run", side_effect=AssertionError("must not execute")):
            draft = sj.recipe_import_text(config, script)
        self.assertEqual((draft["modelPath"], draft["tensorParallel"], draft["kvCacheDtype"]), ("/models/GLM-5.3-Flash-NVFP4", 4, "bfloat16"))
        self.assertEqual(draft["recipe"]["args"][0]["json"], {"method": "mtp", "num_speculative_tokens": 2})
        self.assertTrue(any("NODE_ARGS" in w for w in draft["warnings"]))
        self.assertTrue(any("Library" in w for w in draft["warnings"]))

    def test_import_refuses_dynamic_shell_and_multiple_launches(self):
        config = 'IMAGE=vllm:test\nMODEL_PATH=/models/test'
        for script in ['vllm serve $(touch /tmp/never-execute)', 'vllm serve "$MISSING"', 'vllm serve /models/a ; reboot', 'vllm serve /models/a\nvllm serve /models/b', 'python3 inner.py']:
            with self.assertRaises((RuntimeError, ValueError)):
                sj.recipe_import_text(config, script)

    def test_image_metadata_requires_arm64_and_immutable_identity(self):
        image = {"Id": "sha256:" + "a"*64, "Architecture": "arm64", "Os": "linux", "Size": 123}
        with mock.patch.object(sj, "docker_info", return_value={"DockerRootDir": TMP_HOME, "Driver": "overlay2"}), mock.patch.object(sj.subprocess, "run") as run:
            run.return_value = type("R", (), {"returncode": 0, "stdout": json.dumps([image])})()
            self.assertEqual(sj.image_metadata("vllm:test"), {"id": image["Id"], "size": 123, "store": "overlay2"})
            self.assertEqual(run.call_args.args[0], ["docker", "image", "inspect", "vllm:test"])
            for field, value in [("Architecture", "amd64"), ("Id", "vllm:tag"), ("Os", "windows")]:
                run.return_value.stdout = json.dumps([dict(image, **{field: value})])
                with self.assertRaisesRegex(RuntimeError, "linux/arm64"):
                    sj.image_metadata("vllm:test")

    def test_image_load_verifies_identity_before_tagging_and_removes_archive(self):
        image_id = "sha256:" + "a"*64
        for loaded in [image_id, "sha256:" + "b"*64]:
            base = tempfile.mkdtemp(dir=TMP_HOME)
            with open(os.path.join(base, "image.tar"), "wb") as f: f.write(b"archive")
            with mock.patch.object(sj, "image_cache", return_value=base), mock.patch.object(sj, "job_pull_v2") as copy, mock.patch.object(sj, "run_watched", return_value=(0, "")) as run, mock.patch.object(sj, "image_metadata", return_value={"id": loaded, "size":7,"store":"overlay2"}), mock.patch.object(sj.subprocess, "run", return_value=type("R", (), {"returncode":0})()) as tag:
                spec = {"image":"vllm:test", "imageId":image_id}
                if loaded == image_id:
                    self.assertEqual(sj.job_image_load("image-test", spec)["id"], image_id)
                    self.assertEqual(tag.call_args.args[0], ["docker", "tag", image_id, "vllm:test"])
                else:
                    with self.assertRaisesRegex(RuntimeError, "identity"):
                        sj.job_image_load("image-test", spec)
                    tag.assert_not_called()
                self.assertTrue(copy.call_args.args[1]["imageTransfer"])
                self.assertEqual(run.call_args.args[1][:3], ["docker", "load", "-i"])
            self.assertFalse(os.path.exists(base))

    def test_invalid_image_destination_is_rejected_before_docker_load(self):
        with mock.patch.object(sj, "run_watched", side_effect=AssertionError("must not load")), mock.patch.object(sj, "image_cache", side_effect=AssertionError("must not create cache")):
            for image in ["x;touch /tmp/x", "sha256:" + "a"*64, "-option"]:
                with self.assertRaisesRegex(RuntimeError, "tag"):
                    sj.job_image_load("invalid-image", {"image": image})

    def test_firewall_diagnostics_are_read_only_and_bounded(self):
        with mock.patch.object(sj.shutil, "which", return_value="/usr/sbin/ufw"), mock.patch.object(sj.subprocess, "run", return_value=type("R", (), {"returncode":0,"stdout":"Status: active\n"})()) as run:
            self.assertIn("TCP 8765", sj.firewall_hint(8765))
            self.assertEqual(run.call_args.args[0], ["ufw", "status"])
            self.assertEqual(run.call_args.kwargs["timeout"], 4)

    def test_ordinary_copy_server_keeps_groups_without_privileged_setgroups(self):
        with mock.patch.object(sj.os, "geteuid", return_value=1000), mock.patch.object(sj.os, "getgid", return_value=1000):
            self.assertEqual(sj.rsync_group_config(), "")
        # Preserve the existing explicit group limit if launched by root.
        with mock.patch.object(sj.os, "geteuid", return_value=0), mock.patch.object(sj.os, "getgid", return_value=123):
            self.assertEqual(sj.rsync_group_config(), "gid = 123\n")

    def test_image_parser_choices_are_bounded_and_do_not_return_help_text(self):
        text = "  --tool-call-parser {hermes,glm47,\n    qwen3_xml,openai}\n  --reasoning-parser {glm45,qwen3}"
        self.assertEqual(sj.parser_choices(text, "--tool-call-parser"), ["glm47", "hermes", "openai", "qwen3_xml"])
        self.assertEqual(sj.parser_choices(text, "--reasoning-parser"), ["glm45", "qwen3"])
        self.assertIsNone(sj.parser_choices("--tool-call-parser {good,../../plugin}", "--tool-call-parser"))
        self.assertIsNone(sj.parser_choices("unavailable", "--reasoning-parser"))

    def test_manifest_refuses_traversal_missing_hashes_and_repeated_files(self):
        good = {"path": "weights/file.bin", "size": 4, "sha256": "0" * 64}
        self.assertEqual(sj.validate_copy_manifest({"files": [good]}), [good])
        for path in ("../outside", "/outside", "a/../../outside", "a//b", sj.MANIFEST):
            with self.assertRaises(RuntimeError):
                sj.validate_copy_manifest({"files": [dict(good, path=path)]})
        with self.assertRaises(RuntimeError):
            sj.validate_copy_manifest({"files": [dict(good, sha256=None)]})
        with self.assertRaises(RuntimeError):
            sj.validate_copy_manifest({"files": [good, good]})

    def test_copy_checks_disk_before_claiming_target(self):
        with mock.patch.object(sj.os, "statvfs", return_value=type("Disk", (), {"f_bavail": 1, "f_blocks": 1024**4, "f_frsize": 1, "f_flag": 0})()):
            with self.assertRaisesRegex(RuntimeError, "Not enough disk space"):
                sj.copy_disk_check(TMP_HOME, 1024)

    def test_manifest_refuses_external_symlinks_and_directory_symlinks(self):
        source, outside = tempfile.mkdtemp(dir=TMP_HOME), tempfile.mkdtemp(dir=TMP_HOME)
        make_model(outside, {"private": b"not part of this model"})
        os.symlink(os.path.join(outside, "private"), os.path.join(source, "weights.bin"))
        with self.assertRaisesRegex(RuntimeError, "escapes"):
            sj.copy_manifest(source, [], "manifest-links")
        os.unlink(os.path.join(source, "weights.bin"))
        os.symlink(outside, os.path.join(source, "nested"))
        with self.assertRaisesRegex(RuntimeError, "Directory symlinks"):
            sj.copy_manifest(source, [], "manifest-links")

    def run_verified_copy(self, corrupt=False, hf=False, extra=False):
        import socket
        if not shutil.which("rsync"):
            self.skipTest("rsync not installed")
        source, target = tempfile.mkdtemp(dir=TMP_HOME), tempfile.mkdtemp(dir=TMP_HOME)
        if hf:
            source = os.path.join(source, "models--fixture--tiny", "snapshots", "revision")
            os.makedirs(source)
        make_model(source, {"config.json": b"{}", "weights.bin": b"good"})
        if hf:
            blob = os.path.join(os.path.dirname(os.path.dirname(source)), "blobs", "a" * 64)
            os.makedirs(os.path.dirname(blob))
            os.rename(os.path.join(source, "weights.bin"), blob)
            os.symlink(os.path.relpath(blob, source), os.path.join(source, "weights.bin"))
        suffix = os.urandom(4).hex()
        serve, pull = "v2-serve-" + suffix, "v2-pull-" + suffix
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        sj.start("serve", serve, {"protocol": 2, "path": source, "address": "127.0.0.1", "port": port,
            "password": "local-test", "allow": "127.0.0.1", "expected": [pull], "idleSeconds": 20, "instance": "dev"})
        try:
            until = time.monotonic() + 10
            while sj.read_status(serve).get("step") != "serving":
                self.assertNotEqual(sj.read_status(serve).get("state"), "failed", sj.read_status(serve))
                self.assertLess(time.monotonic(), until)
                time.sleep(.05)
            if corrupt:
                with open(os.path.join(source, "weights.bin"), "wb") as f:
                    f.write(b"evil")
            if extra:
                with open(os.path.join(source, "added.bin"), "wb") as f:
                    f.write(b"not in the manifest")
            sj.start("pull", pull, {"protocol": 2, "src": "rsync://sparkbox@127.0.0.1:%d/model" % port,
                "dest": target, "password": "local-test", "instance": "dev", "files": [{"path":"config.json","size":2},{"path":"weights.bin","size":4}]})
            result = wait_done(pull, timeout=30)
            if corrupt or extra:
                self.assertEqual(result["state"], "failed", result)
                self.assertIn("file set changed" if extra else "checksum", result["message"])
                self.assertFalse(os.path.exists(os.path.join(target, sj.MANIFEST)))
            else:
                self.assertEqual(result["state"], "done", result)
                manifest = sj.read_manifest(target)
                self.assertEqual(manifest["checked"], "source-sha256")
                self.assertTrue(all(re.fullmatch("[0-9a-f]{64}", f["sha256"]) for f in manifest["files"]))
                self.assertFalse(os.path.exists(os.path.join(target, sj.PARTIAL)))
            self.assertEqual(wait_done(serve)["state"], "done")
            self.assertFalse(os.path.exists(os.path.join(sj.RSYNCD, serve)))
            self.assertFalse(os.path.exists(os.path.join(sj.SECRETS, pull)))
            self.assertEqual(os.stat(sj.SECRETS).st_mode & 0o777, 0o700)
            self.assertEqual(os.stat(sj.RSYNCD).st_mode & 0o777, 0o700)
        finally:
            for job in (pull, serve):
                if sj.read_status(job).get("state") == "running":
                    sj.cancel(job)

    def test_verified_copy_finishes_without_mac_polling_or_cancel(self):
        self.run_verified_copy()

    def test_source_changed_after_hashing_fails_without_a_verified_manifest(self):
        self.run_verified_copy(corrupt=True)

    def test_hf_snapshot_links_copy_only_its_own_blobs_and_finish_verified(self):
        self.run_verified_copy(hf=True)

    def test_new_source_files_after_hashing_never_get_a_verified_manifest(self):
        self.run_verified_copy(extra=True)


if __name__ == "__main__":
    unittest.main()
