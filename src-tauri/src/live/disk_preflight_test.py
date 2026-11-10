"""Offline destination admission: no network, installation or real model transfer."""
import errno
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import sparkjob_test as fixture

sj = fixture.sj
GIB = 1024**3


def capacity(available, total=1000 * GIB, **extra):
    return SimpleNamespace(f_bavail=available, f_bfree=total, f_blocks=total, f_frsize=1, f_flag=0, **extra)


class DestinationDisk(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=fixture.TMP_HOME)
        self.root = Path(self.temp.name).resolve()
        self.dest = str(self.root / "new" / "model")

    def tearDown(self):
        self.temp.cleanup()

    def test_exact_boundary_rounds_up_and_uses_user_available_blocks(self):
        need = 20 * GIB + 2  # a one-byte transfer, rounded upward after the 5% margin
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(need)):
            self.assertTrue(sj.destination_disk(self.dest, 1)["ok"])
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(need - 1)):
            check = sj.destination_disk(self.dest, 1, "cell2")
            self.assertFalse(check["ok"])
            self.assertEqual(check["requiredBytes"], need)
            with self.assertRaisesRegex(RuntimeError, "cell2.*available.*required"):
                sj.require_destination_disk(self.dest, 1, "cell2")

    def test_missing_destination_uses_its_real_ancestor_without_creating_it(self):
        folder = self.root / "mounted"; folder.mkdir()
        link = self.root / "models"; link.symlink_to(folder, target_is_directory=True)
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(500 * GIB)) as read:
            check = sj.destination_disk(str(link / "org" / "model"), 100)
        read.assert_called_once_with(str(folder))
        self.assertEqual(check["probe"], str(folder))
        self.assertFalse((folder / "org").exists())

    def test_broken_links_files_and_unreadable_filesystems_refuse(self):
        broken = self.root / "broken"; broken.symlink_to(self.root / "missing")
        file = self.root / "file"; file.write_text("kept")
        for dest in (str(broken / "model"), str(file), str(file / "model")):
            with self.subTest(dest=dest), self.assertRaisesRegex(RuntimeError, "Cannot check destination disk"):
                sj.destination_disk(dest, 1)
        with mock.patch.object(sj.os, "statvfs", side_effect=PermissionError("not readable")):
            with self.assertRaisesRegex(RuntimeError, "cell4.*not readable"):
                sj.destination_disk(self.dest, 1, "cell4")

    def test_parent_after_a_symlink_uses_the_filesystem_parent_not_a_lexical_path(self):
        mounted = self.root / "mounted"; child = mounted / "nested"; child.mkdir(parents=True)
        link = self.root / "models"; link.symlink_to(child, target_is_directory=True)
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(500 * GIB)) as read:
            check = sj.destination_disk(str(link / ".." / "org" / "model"), 1)
        read.assert_called_once_with(str(mounted))
        self.assertEqual(check["probe"], str(mounted))
        self.assertFalse((mounted / "org").exists())

    def test_unknown_invalid_and_read_only_readings_refuse(self):
        for st in (capacity(-1), capacity(1, 0), capacity(1001, 1000), capacity(0, 0)):
            with self.subTest(st=st), mock.patch.object(sj.os, "statvfs", return_value=st), self.assertRaises(RuntimeError):
                sj.destination_disk(self.dest, 1)
        st = capacity(500 * GIB); st.f_flag = getattr(os, "ST_RDONLY", 1)
        with mock.patch.object(sj.os, "statvfs", return_value=st), self.assertRaisesRegex(RuntimeError, "read-only"):
            sj.destination_disk(self.dest, 1)

    def test_known_zero_files_count_but_missing_sizes_never_become_zero(self):
        self.assertEqual(sj.transfer_bytes([{"size":0}, {"size":1}]), 1)
        for files in ([], None, [{}], [{"size":None}], [{"size":True}], [{"size":-1}], [{"size":2**53}]):
            with self.subTest(files=files), self.assertRaises(RuntimeError): sj.transfer_bytes(files)

    def test_warning_threshold_and_projected_usage_are_independent_of_admission(self):
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(150 * GIB)):
            self.assertIsNone(sj.destination_disk(self.dest, 0)["warning"])
            check = sj.destination_disk(self.dest, 10 * GIB)
            self.assertTrue(check["ok"])
            self.assertIn("86.0%", check["warning"])
        with mock.patch.object(sj.os, "statvfs", return_value=capacity(0)):
            self.assertFalse(sj.destination_disk(self.dest, 0)["ok"])

    def test_low_space_refuses_before_job_folder_process_tool_or_token(self):
        spec = {"dest":self.dest, "node":"cell2", "files":[{"path":"model.safetensors","size":1}], "instance":"dev"}
        with mock.patch.object(sj, "check_download_tool"), mock.patch.object(sj, "JOBS", str(self.root / "jobs")), \
             mock.patch.object(sj.os, "statvfs", return_value=capacity(1)), mock.patch.object(sj.os, "fork") as fork, \
             mock.patch.object(sj, "prepare_download_tool") as install, mock.patch.object(sj, "write_token") as token:
            for kind in ("download", "pull"):
                with self.subTest(kind=kind), self.assertRaisesRegex(RuntimeError, "Not enough disk space on cell2"):
                    sj.start(kind, "refused", spec)
            fork.assert_not_called(); install.assert_not_called(); token.assert_not_called()
        self.assertFalse((self.root / "jobs").exists()); self.assertFalse(Path(self.dest).exists())

    def test_space_lost_under_folder_lock_leaves_old_writer_and_marker_alone(self):
        spec = {"dest":self.dest, "node":"cell2", "files":[{"path":"model.safetensors","size":1}], "instance":"dev"}
        jobs = self.root / "jobs"; jobs.mkdir()
        with mock.patch.object(sj, "JOBS", str(jobs)), mock.patch.object(sj, "check_download_tool"), \
             mock.patch.object(sj.os, "statvfs", side_effect=[capacity(500 * GIB), capacity(1)]), \
             mock.patch.object(sj, "stop_leftover") as stop, mock.patch.object(sj, "mark_partial") as mark, \
             mock.patch.object(sj, "prepare_download_tool") as install:
            with self.assertRaisesRegex(RuntimeError, "Not enough disk space"):
                sj.job_download("race", spec)
            stop.assert_not_called(); mark.assert_not_called(); install.assert_not_called()

    def test_resume_disk_refusal_preserves_original_plan_record_and_marker(self):
        dest = self.root / "partial"; dest.mkdir()
        jobs = self.root / "jobs"; jobs.mkdir()
        spec = {"repo":"org/model", "revision":"a"*40, "image":"vllm:test", "dest":str(dest), "node":"cell2",
                "files":[{"path":"model.safetensors","size":10}], "instance":"dev"}
        with mock.patch.object(sj,"JOBS",str(jobs)), mock.patch.object(sj,"check_download_tool"):
            plan = sj.download_plan(spec)
            sj.write_status("original", kind="download", state="cancelled", dest=str(dest), instance="dev", downloadPlan=plan, planHash=sj.plan_hash(plan))
            sj.mark_partial(str(dest), "original", "download")
            before = {str(p):p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
            with mock.patch.object(sj.os,"statvfs",return_value=capacity(1)), self.assertRaisesRegex(RuntimeError,"Not enough disk"):
                sj.start("download","new",dict(spec,resumeOf="original",resumeHash=sj.plan_hash(plan)))
            after = {str(p):p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
            self.assertEqual(before, after)

    def test_enospc_names_node_destination_and_current_free_space(self):
        with mock.patch.object(sj.os,"statvfs",return_value=capacity(1024)):
            for error in (OSError(errno.ENOSPC,"No space left on device"), RuntimeError("rsync: No space left on device (28)")):
                message = sj.disk_failure({"node":"cell3","dest":self.dest},error)
                self.assertIn("cell3",message); self.assertIn(self.dest,message); self.assertIn("available now",message)
                self.assertIn("Partial files are retained",message)
        self.assertEqual(sj.disk_failure({},RuntimeError("checksum mismatch")),"checksum mismatch")
        # Image/helper jobs without a model destination keep their own failure context.
        other = OSError(errno.ENOSPC, "No space left on device")
        self.assertEqual(sj.disk_failure({"node":"cell3"}, other), str(other))


if __name__ == "__main__": unittest.main()
