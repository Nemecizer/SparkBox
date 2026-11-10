"""Tests for launch.py, run for real: python3 -m unittest discover -s src-tauri/src/live -p 'launch_test.py'

Each test lays out what a container sees (launch.py, deploy/, key) in a temp folder and puts a stand-in
`vllm` on PATH. The stand-in records its argv, environment and PID, so a pass means it really started
with what the files said. Lifecycle tests exercise real children and Linux orphan reaping.
"""

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import signal
import importlib.util
from unittest.mock import patch, Mock

LAUNCH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "launch.py")
KEY = "sbk-3f2a9c7e1d"
KEY_LINE = "sparkbox: the API key file is missing or empty, so vLLM was not started.\n"
STAND_IN = """#!/bin/sh
exec "{python}" - "$@" <<'EOF'
import json, os, sys
json.dump({{"argv": ["vllm"] + sys.argv[1:], "env": dict(os.environ), "pid": os.getpid(), "ppid": os.getppid()}},
          open(os.environ["RECORD"], "w"))
print("the stand-in ran")
EOF
"""


class Launch(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="sparkbox-launch-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.root = os.path.join(self.tmp, "run", "sparkbox")
        os.makedirs(os.path.join(self.root, "deploy"))
        shutil.copy(LAUNCH, self.root)
        self.bin = os.path.join(self.tmp, "bin")
        os.mkdir(self.bin)
        self.record = os.path.join(self.tmp, "record.json")
        self.write_vllm(STAND_IN.format(python=sys.executable))
        self.argv(["vllm", "serve", "/models/m", "--port", "8000"])
        self.env({"HF_HUB_OFFLINE": "1"})

    def write_vllm(self, text):
        path = os.path.join(self.bin, "vllm")
        with open(path, "w") as f:
            f.write(text)
        os.chmod(path, 0o755)

    def argv(self, value):
        self.write_file("deploy/argv.json", json.dumps(value))

    def env(self, value):
        self.write_file("deploy/env.json", json.dumps(value))

    def write_file(self, name, text):
        with open(os.path.join(self.root, name), "w") as f:
            f.write(text)

    def write_key(self, data):
        with open(os.path.join(self.root, "key"), "wb") as f:
            f.write(data)

    def run_launch(self, *options, environ=None):
        env = {"PATH": self.bin + os.pathsep + "/usr/bin:/bin", "RECORD": self.record}
        env.update(environ or {})
        return subprocess.run(
            [sys.executable, os.path.join(self.root, "launch.py"), *options],
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def recorded(self):
        with open(self.record) as f:
            return json.load(f)

    def assert_refused(self, r, line=None, code=78):
        self.assertEqual(r.returncode, code, r.stderr)
        self.assertFalse(os.path.exists(self.record), "vLLM was not started")
        self.assertEqual(r.stderr.count("\n"), 1, "one line: " + r.stderr)
        self.assertTrue(r.stderr.startswith("sparkbox: "), r.stderr)
        self.assertEqual(r.stdout, "")
        if line is not None:
            self.assertEqual(r.stderr, line)

    def assert_no_key(self, r):
        self.assertNotIn(KEY, r.stdout + r.stderr)


class Starts(Launch):
    def test_execs_vllm_with_the_files_argv_and_env(self):
        self.env({"HF_HUB_OFFLINE": "1", "VLLM_HOST_IP": "192.168.100.1"})
        r = self.run_launch()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout, "the stand-in ran\n")
        got = self.recorded()
        self.assertEqual(got["argv"], ["vllm", "serve", "/models/m", "--port", "8000"])
        self.assertEqual(got["env"]["HF_HUB_OFFLINE"], "1")
        self.assertEqual(got["env"]["VLLM_HOST_IP"], "192.168.100.1")

    def test_launcher_forwards_term_to_its_private_vllm_child(self):
        """The supervisor stays alive and forwards Docker's stop signal to vLLM."""
        self.write_vllm(
            '#!/bin/sh\necho "$$" > "$RECORD.pid"\ntrap \'echo term > "$RECORD.term"; exit 0\' TERM\n'
            'echo ready\ni=0; while [ "$i" -lt 400 ]; do sleep 0.05; i=$((i + 1)); done\n'
        )
        env = {"PATH": self.bin + os.pathsep + "/usr/bin:/bin", "RECORD": self.record}
        p = subprocess.Popen(
            [sys.executable, os.path.join(self.root, "launch.py")], env=env, stdout=subprocess.PIPE, text=True
        )
        self.addCleanup(p.kill)
        self.addCleanup(p.stdout.close)
        self.assertEqual(p.stdout.readline(), "ready\n")
        with open(self.record + ".pid") as f:
            self.assertNotEqual(int(f.read()), p.pid, "the launcher stays to reap vLLM's children")
        p.terminate()
        self.assertEqual(p.wait(timeout=10), 0)
        self.assertTrue(os.path.exists(self.record + ".term"), "TERM arrived")

    def test_a_quoted_json_flag_arrives_as_one_element(self):
        spec = '{"method": "mtp", "note": "it\'s $(id) `x` \\"y\\"", "num_speculative_tokens": 5}'
        self.argv(["vllm", "serve", "/m", "--speculative-config", spec, "--chat-template", "a b  c"])
        r = self.run_launch()
        self.assertEqual(r.returncode, 0, r.stderr)
        got = self.recorded()["argv"]
        self.assertEqual(got, ["vllm", "serve", "/m", "--speculative-config", spec, "--chat-template", "a b  c"])

    def test_a_newline_in_an_argument_stays_in_it(self):
        self.argv(["vllm", "serve", "/m", "--x", "a\nb"])
        self.assertEqual(self.run_launch().returncode, 0)
        self.assertEqual(self.recorded()["argv"][-1], "a\nb")


class Lifecycle(Launch):
    def test_exit_status_and_signal_status_are_preserved(self):
        for statement, code in [('sys.exit(7)', 7), ('os.kill(os.getpid(), signal.SIGTERM)', 143)]:
            self.write_vllm('#!' + sys.executable + '\nimport os,signal,sys\n' + statement + '\n')
            self.assertEqual(self.run_launch().returncode, code)

    def test_interrupt_is_forwarded_without_restarting(self):
        self.write_vllm('#!' + sys.executable + '\nimport signal,time\n'
                        'signal.signal(signal.SIGINT, lambda *_: exit(23))\n'
                        'print("ready", flush=True)\nwhile True: time.sleep(.05)\n')
        with subprocess.Popen([sys.executable, os.path.join(self.root, 'launch.py')],
                env={'PATH': self.bin + os.pathsep + '/usr/bin:/bin'},
                stdout=subprocess.PIPE, text=True) as proc:
            try:
                self.assertEqual(proc.stdout.readline(), 'ready\n')
                proc.send_signal(signal.SIGINT)
                self.assertEqual(proc.wait(timeout=5), 23)
            finally:
                if proc.poll() is None:
                    proc.kill()

    @unittest.skipUnless(sys.platform.startswith('linux'), 'Linux orphan adoption; also run in local Ubuntu')
    def test_waits_for_orphan_tracker_and_preserves_unrelated_semaphore(self):
        import multiprocessing
        from pathlib import Path
        unrelated = multiprocessing.get_context('spawn').Semaphore(1)
        other = Path('/dev/shm/sem.' + unrelated._semlock.name.lstrip('/'))
        self.assertTrue(other.exists())
        self.write_vllm('#!' + sys.executable + r'''
import json,multiprocessing,os,time
sem = multiprocessing.get_context('spawn').Semaphore(1)
if os.fork() == 0:
    os.setsid()
    time.sleep(.4)
    open(os.environ['RECORD'] + '.child-finished', 'w').close()
    os._exit(0)
json.dump({'name': sem._semlock.name}, open(os.environ['RECORD'], 'w'))
os._exit(7)
''')
        result = self.run_launch()
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertTrue(os.path.exists(self.record + '.child-finished'))
        self.assertFalse(Path('/dev/shm/sem.' + self.recorded()['name'].lstrip('/')).exists())
        self.assertTrue(other.exists(), 'unrelated registered resources remain untouched')

    def module(self):
        spec = importlib.util.spec_from_file_location('sparkbox_launcher_test', LAUNCH)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_cleanup_timeout_is_bounded_and_visible(self):
        module = self.module()
        # Natural exit gets 20 seconds; an earlier Docker stop caps drain at 25 seconds
        # from the signal, even when the engine spent 10 seconds shutting down itself.
        for stopping, times in [(False, [0, 21]), (True, [0, 10, 26])]:
            with self.subTest(stopping=stopping):
                child = Mock(pid=12345, returncode=None)
                clock = iter(times)
                handlers = {}

                def start(*args, **kwargs):
                    if stopping:
                        handlers[signal.SIGTERM](signal.SIGTERM, None)
                    return child

                # The engine exited, but an adopted child remains. Keep uncertainty visible.
                with patch.object(module, 'child_reaper'), \
                     patch.object(module.signal, 'signal', side_effect=lambda sig, handler: handlers.update({sig: handler})), \
                     patch.object(module.subprocess, 'Popen', side_effect=start), \
                     patch.object(module.os, 'waitpid', side_effect=[(12345, 0), (0, 0)]), \
                     patch.object(module.os, 'killpg') as stop, \
                     patch.object(module.time, 'monotonic', side_effect=lambda: next(clock)), \
                     patch('sys.stderr') as stderr:
                    self.assertEqual(module.supervise(['vllm', 'serve'], {}), 124)
                    stop.assert_called_once_with(12345, signal.SIGTERM)
                    self.assertIn('check the unload report for leftovers', ''.join(str(c) for c in stderr.write.call_args_list))

    def test_unavailable_linux_reaper_refuses_before_start(self):
        module = self.module()
        with patch.object(module.sys, 'platform', 'linux'), patch('ctypes.CDLL') as libc, \
             patch.object(module.subprocess, 'Popen') as start, patch('sys.stderr'):
            libc.return_value.prctl.return_value = -1
            with self.assertRaises(SystemExit) as stopped:
                module.supervise(['vllm', 'serve'], {})
            self.assertEqual(stopped.exception.code, 78)
            start.assert_not_called()


class Key(Launch):
    def test_the_key_reaches_vllms_environment_and_no_output(self):
        self.write_key((KEY + "\n").encode())
        r = self.run_launch("--key")
        self.assertEqual(r.returncode, 0, r.stderr)
        got = self.recorded()
        self.assertEqual(got["env"]["VLLM_API_KEY"], KEY, "stripped")
        self.assertNotIn(KEY, " ".join(got["argv"]))
        self.assert_no_key(r)

    def test_no_key_option_removes_an_inherited_key(self):
        r = self.run_launch(environ={"VLLM_API_KEY": KEY})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("VLLM_API_KEY", self.recorded()["env"])
        self.assert_no_key(r)

    def test_a_key_file_is_ignored_without_the_option(self):
        self.write_key(KEY.encode())
        self.assertEqual(self.run_launch().returncode, 0)
        self.assertNotIn("VLLM_API_KEY", self.recorded()["env"])

    def test_the_key_option_replaces_an_inherited_key(self):
        self.write_key(KEY.encode())
        self.assertEqual(self.run_launch("--key", environ={"VLLM_API_KEY": "other"}).returncode, 0)
        self.assertEqual(self.recorded()["env"]["VLLM_API_KEY"], KEY)

    def test_a_missing_key_file_refuses(self):
        r = self.run_launch("--key", environ={"VLLM_API_KEY": KEY})
        self.assert_refused(r, KEY_LINE)
        self.assert_no_key(r)

    def test_an_empty_key_file_refuses(self):
        for data in (b"", b"\n", b"   \n\t\r\n"):
            self.write_key(data)
            self.assert_refused(self.run_launch("--key"), KEY_LINE)

    def test_a_folder_in_place_of_the_key_refuses(self):
        os.mkdir(os.path.join(self.root, "key"))
        self.assert_refused(self.run_launch("--key"), KEY_LINE)

    @unittest.skipIf(os.geteuid() == 0, "root reads a 0000 file")
    def test_an_unreadable_key_file_refuses(self):
        self.write_key(KEY.encode())
        os.chmod(os.path.join(self.root, "key"), 0)
        r = self.run_launch("--key")
        os.chmod(os.path.join(self.root, "key"), stat.S_IRUSR)
        self.assert_refused(r, KEY_LINE)
        self.assert_no_key(r)

    def test_a_key_that_cant_be_text_refuses_without_printing_it(self):
        for data in (b"\xff\xfe" + KEY.encode(), KEY.encode() + b"\0tail"):
            self.write_key(data)
            r = self.run_launch("--key")
            self.assert_refused(r, KEY_LINE)
            self.assert_no_key(r)

    def test_no_failure_prints_the_key(self):
        self.write_key(KEY.encode())
        os.remove(os.path.join(self.bin, "vllm"))
        r = self.run_launch("--key")
        self.assertEqual(r.returncode, 127)
        self.assert_no_key(r)
        self.write_vllm("#!/bin/sh\n")
        self.env({"VLLM_API_KEY": KEY})
        r = self.run_launch("--key")
        self.assert_refused(r)
        self.assert_no_key(r)


class Refuses(Launch):
    def test_anything_but_vllm_serve(self):
        for argv in (
            ["sh", "-c", "id"],
            ["/bin/sh", "serve"],
            ["bash"],
            ["python3", "-c", "print(1)"],
            ["vllm"],
            ["vllm", "bench"],
            ["vllm", "--help"],
            ["./vllm", "serve"],
            ["/usr/bin/vllm", "serve"],
            ["VLLM", "serve"],
            [],
        ):
            self.argv(argv)
            self.assert_refused(self.run_launch(), "sparkbox: this launcher only starts vllm serve, so nothing was started.\n")

    def test_argv_of_a_wrong_type(self):
        for value in ({"vllm": "serve"}, "vllm serve", None, 3, ["vllm", "serve", 1], ["vllm", "serve", None], [["vllm"], "serve"]):
            self.argv(value)
            self.assert_refused(self.run_launch())

    def test_env_of_a_wrong_type(self):
        for value in (["A=1"], "A=1", None, 3, {"A": 1}, {"A": None}, {"A": ["x"]}):
            self.env(value)
            self.assert_refused(self.run_launch())

    def test_bad_json(self):
        for name in ("deploy/argv.json", "deploy/env.json"):
            self.setUp()
            self.write_file(name, "{not json")
            self.assert_refused(self.run_launch())

    def test_files_that_arent_there(self):
        for name in ("deploy/argv.json", "deploy/env.json"):
            self.setUp()
            os.remove(os.path.join(self.root, name))
            self.assert_refused(self.run_launch())

    def test_text_the_kernel_cant_carry(self):
        self.argv(["vllm", "serve", "a\0b"])
        self.assert_refused(self.run_launch())
        self.argv(["vllm", "serve", "\ud800"])
        self.assert_refused(self.run_launch())
        self.argv(["vllm", "serve"])
        for env in ({"A": "a\0b"}, {"A\0B": "1"}, {"A=B": "1"}, {"": "1"}, {"A": "\ud800"}):
            self.env(env)
            self.assert_refused(self.run_launch())

    def test_the_key_in_env_json(self):
        self.env({"VLLM_API_KEY": KEY})
        r = self.run_launch()
        self.assert_refused(r, "sparkbox: VLLM_API_KEY can't come from deploy/env.json, so vLLM was not started.\n")
        self.assert_no_key(r)
        self.write_key(KEY.encode())
        r = self.run_launch("--key")
        self.assert_refused(r)
        self.assert_no_key(r)

    def test_a_name_that_changes_what_vllm_means(self):
        for name in ("PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "PYTHONPATH", "PYTHONHOME"):
            self.env({name: "/tmp/evil"})
            self.assert_refused(self.run_launch())

    def test_any_other_option(self):
        for options in (["--keys"], ["--key", "extra"], ["--key=x"], ["-k"], ["serve"]):
            self.assert_refused(self.run_launch(*options), "sparkbox: the launcher takes --key and nothing else, so vLLM was not started.\n")

    def test_a_vllm_that_cant_run_exits_127_with_one_line(self):
        os.remove(os.path.join(self.bin, "vllm"))
        r = self.run_launch()
        self.assert_refused(r, code=127)
        self.write_vllm("#!/bin/sh\n")
        os.chmod(os.path.join(self.bin, "vllm"), 0o644)
        self.assert_refused(self.run_launch(), code=127)


class Root(Launch):
    def test_the_root_is_where_the_file_is_whatever_the_working_directory(self):
        env = {"PATH": self.bin + os.pathsep + "/usr/bin:/bin", "RECORD": self.record}
        r = subprocess.run(
            [sys.executable, os.path.relpath(os.path.join(self.root, "launch.py"), self.tmp)],
            env=env,
            cwd=self.tmp,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.recorded()["argv"][1], "serve")

    def test_only_the_standard_library(self):
        """Run with no site-packages, as a bare image's Python would."""
        env = {"PATH": self.bin + os.pathsep + "/usr/bin:/bin", "RECORD": self.record}
        r = subprocess.run(
            [sys.executable, "-I", "-S", os.path.join(self.root, "launch.py")], env=env, capture_output=True, text=True, timeout=30
        )
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
