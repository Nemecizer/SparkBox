#!/usr/bin/env python3
"""SparkBox's launcher: starts vLLM from files, with no shell in the path.

Mounted read-only at /run/sparkbox/launch.py beside the load's deploy/argv.json and
deploy/env.json, and with --key its key file. It checks them, puts them in the
child environment and supervises vLLM without a shell. PID 1 must outlive vLLM's
resource tracker: Linux kills a PID namespace's remaining processes when PID 1
exits, which can strand the tracker's host-IPC semaphores. Forward stop signals,
then reap the children while their own cleanup finishes. Never unlink host files.
Standard library only: it runs on the image's Python.
"""

import json
import os
import signal
import subprocess
import sys
import time

# plan::KEY_REFUSED. A load that can't start this way exits with it, and nothing was started.
REFUSED = 78
NOT_FOUND = 127
KEY_LINE = "the API key file is missing or empty, so vLLM was not started."
KEY_ENV = "VLLM_API_KEY"
# These decide what `vllm` means, so the load's own env.json can't set them.
RUNS_SOMETHING_ELSE = ("PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "PYTHONPATH", "PYTHONHOME")
CHILD_DRAIN_SECONDS = 20
# Leave a margin inside SparkBox's unchanged `docker stop -t 30` limit.
STOP_DRAIN_SECONDS = 25


def refuse(line, code=REFUSED):
    print("sparkbox: " + line, file=sys.stderr, flush=True)
    sys.exit(code)


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def plain(text):
    """Text the kernel can carry in an argument or a variable."""
    try:
        text.encode("utf-8")
    except UnicodeError:
        return False
    return "\0" not in text


def read_argv(root):
    try:
        argv = read_json(os.path.join(root, "deploy", "argv.json"))
    except (OSError, ValueError):
        refuse("deploy/argv.json can't be read, so vLLM was not started.")
    if not isinstance(argv, list) or not all(isinstance(a, str) and plain(a) for a in argv):
        refuse("deploy/argv.json isn't a list of plain strings, so vLLM was not started.")
    if argv[:2] != ["vllm", "serve"]:
        refuse("this launcher only starts vllm serve, so nothing was started.")
    return argv


def read_env(root):
    try:
        env = read_json(os.path.join(root, "deploy", "env.json"))
    except (OSError, ValueError):
        refuse("deploy/env.json can't be read, so vLLM was not started.")
    if not isinstance(env, dict) or not all(isinstance(v, str) and plain(v) for v in env.values()):
        refuse("deploy/env.json isn't an object of plain strings, so vLLM was not started.")
    if not all(k and "=" not in k and plain(k) for k in env):
        refuse("deploy/env.json has a name that can't be a variable, so vLLM was not started.")
    if KEY_ENV in env:
        refuse(KEY_ENV + " can't come from deploy/env.json, so vLLM was not started.")
    if any(k in env for k in RUNS_SOMETHING_ELSE):
        refuse("deploy/env.json may not change what vllm means, so vLLM was not started.")
    return env


def read_key(root):
    """The key, stripped. Never printed: anything wrong with it reads as the one line."""
    try:
        with open(os.path.join(root, "key"), "rb") as f:
            key = f.read().decode("utf-8").strip()
    except (OSError, ValueError):
        refuse(KEY_LINE)
    if not key or "\0" in key:
        refuse(KEY_LINE)
    return key


def child_reaper():
    """PID 1 adopts orphans already; subreaping also covers an init wrapper on Linux.

    Non-Linux runs are local launcher fixtures, not the shipped container path.
    No PID/name scan, host signal or shared-memory removal is involved.
    """
    if sys.platform.startswith("linux"):
        import ctypes
        libc = ctypes.CDLL(None, use_errno=True)
        if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
            refuse("child cleanup supervision is unavailable, so vLLM was not started.")


def supervise(argv, env):
    child_reaper()
    pending = []
    stop_at = None

    def stopping(sig, _frame):
        nonlocal stop_at
        if stop_at is None:
            stop_at = time.monotonic()
        pending.append(sig)

    # Installed before spawn so a stop during startup is forwarded once the child exists.
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, stopping)
    try:
        child = subprocess.Popen(argv, env=env, start_new_session=True)
    except OSError as e:
        refuse("vllm couldn't be started: " + (e.strerror or "exec failed") + ".", NOT_FOUND)

    drain_at = None
    while True:
        # Reap only our children, including adopted descendants. Preserve vLLM's own status.
        no_children = False
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                no_children = True
                break
            if not pid:
                break
            if pid == child.pid:
                child.returncode = os.waitstatus_to_exitcode(status)
        if child.returncode is not None and drain_at is None:
            drain_at = time.monotonic()
            # The engine has finished its own shutdown. End any remaining workers in its
            # private group; Python's resource tracker ignores TERM and cleans up at EOF.
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        while pending:
            sig = pending.pop(0)
            if child.returncode is None:
                try:
                    os.kill(child.pid, sig)
                except ProcessLookupError:
                    pass
        if no_children:
            code = child.returncode
            if code is None:
                refuse("vLLM's exit status could not be read.", 125)
            return code if code >= 0 else 128 - code
        if drain_at is not None:
            deadline = drain_at + CHILD_DRAIN_SECONDS
            if stop_at is not None:
                deadline = min(deadline, stop_at + STOP_DRAIN_SECONDS)
            if time.monotonic() >= deadline:
                # Docker still owns the container and its final stop limit. Keep uncertainty
                # visible; never guess which /dev/shm entries belong to this load.
                print("sparkbox: child cleanup did not finish before the shutdown limit; "
                      "check the unload report for leftovers.", file=sys.stderr, flush=True)
                return 124
        time.sleep(0.05)


def main():
    root = os.path.dirname(os.path.abspath(__file__))
    options = sys.argv[1:]
    if options not in ([], ["--key"]):
        refuse("the launcher takes --key and nothing else, so vLLM was not started.")
    argv = read_argv(root)
    env = read_env(root)
    os.environ.pop(KEY_ENV, None)
    if options:
        env[KEY_ENV] = read_key(root)
    child_env = dict(os.environ)
    child_env.update(env)
    sys.stdout.flush()
    sys.stderr.flush()
    sys.exit(supervise(argv, child_env))


if __name__ == "__main__":
    main()
