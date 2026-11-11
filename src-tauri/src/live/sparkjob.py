#!/usr/bin/env python3
"""SparkBox job runner.

SparkBox copies this file to ~/.sparkbox/sparkjob.py on each node. Long jobs
(model downloads, copies between nodes) run detached, so they keep going when
SparkBox disconnects; each job's progress lives in ~/.sparkbox/jobs/<id>.json.

Commands (each prints one JSON value):
  start <kind> <id>        read the job spec (JSON) on stdin, start it detached; a download fetches
                           only spec files[] and include[] patterns, serve and pull skip exclude[]
  list [--look]            every job on this node; a dead job's status and secret files are
                           settled here, the one thing a read changes; --look only reports. A job
                           that's over and left its child running says "orphan": true, and its
                           message says so
  cancel <id>              stop a job, its download's container, and remove its secret files; the
                           job's message says so if that container may still be running
  scan <root>...           model folders under the given roots, finished or partway
  manifest <path>          files and sizes of one model folder
  check <root> <image> <ray image> [quick] [--instance <i>]   readiness facts; the vLLM version
                           and whether a container sees the GPU are read in test containers
                           (sparkbox-checkimage-*, labelled sparkbox.test=1 and
                           sparkbox.instance=<i>), which quick skips
  deploy-files <id> <role> a load's launch files, from JSON on stdin: argv.json and env.json in
                           ~/.sparkbox/deploy/<id>, the API key in ~/.sparkbox/keys/<id>, and the
                           cache folders; only `vllm serve` passes
  delete <root> <path>     remove a model folder (or one left partway) that lives inside root,
                           unless a job, or the child a dead job left, is still writing to it
  ls <path>                a folder's entries, owners and free space (Files window)
  read <path>              a text file up to 2 MB with its SHA-256, mode and owner; head and
                           tail of a bigger one (Files window editor)
  size <path>...           total bytes under the paths
  du <path>...             bytes under each path (Calculate size)
  df <path>                free and total bytes of the filesystem holding path
  commit <path> --sha256 <new> [--expect <old>] [--from <file>] [--backups <dir>] [--owner uid:gid] [--nonce n]
                           atomic save (text on stdin unless --from), keeping the old version
  backups <dir> <path>     saved previous versions of path, newest first
  trash <path>...          move to the freedesktop Trash (from ~/sparkbox-test, to its own
                           .sparkbox-trash); untrash <trashed>... puts them back
  rm <path>...             delete for good (system folders and the home folder refused)
  summary <dir> <name>...  bytes, entries and tar size of items in dir, plus free space
  place <from> <to>        put staged items in place by policy (JSON spec on stdin)
  cp <dir> <to> <id> <name>...   copy within this node into <to>'s staging folder, with progress
  cleanup <to> <id>        remove a transfer's staging folder (says if anything is left)
  tree <dir> <name>...     every entry under the named items: kind and size (to verify a copy)
  sweep <dir> [<id>...]    remove staging folders no running transfer owns: untouched for 10
                           minutes (or 24 hours, whatever their id); the ids are kept
  lease claim <instance> <tag> <mac> <ttl_s>   take or renew the controller lease, unless the
                           other SparkBox's heartbeat is under ttl_s old (node clock)
  lease release <instance> give the lease up, if it's this instance's

Job kinds: download (Hugging Face -> this node), serve (temporary read-only
rsync server for one model folder), pull (copy a model folder from a serve job).
Secrets (Hugging Face token, rsync password) arrive on stdin and stay in memory, with two
exceptions that last only as long as their job. rsync needs its password in a 0600 file under
~/.sparkbox/rsyncd/<job>. The Hugging Face token goes in a 0400 file under a 0700 folder,
~/.sparkbox/job-secrets/<job>, mounted read-only and named by HF_TOKEN_PATH: passing it with -e or
--env-file would put it in the container's config, where docker inspect shows it. Both folders go
when the job ends, before it reads as over (its finally backs that up), when it's cancelled, and at
the next `list` after a SIGKILL or a reboot.

A download's container is named sparkbox-dl-<job> and labelled sparkbox.deployment=<job> and
sparkbox.instance=<instance> (the spec's). hf runs as the container's PID 1, which ignores SIGTERM,
and the container outlives its docker CLI, so ending the CLI doesn't end the download: Cancel, a job
told to stop, and a new job on the same folder stop the container first, with docker stop, and only
one docker lists with both labels. A retry has a new job id, so it finds the old one in the folder's
marker. A docker that can't say what's running counts as one that might be: a new job on the folder
ends there, and Cancel says its container may still be running. `list` stays a read: it stops
nothing, and a dead download's message says a container may still be running.

A download or pull writes <dest>/.sparkbox.partial (job, kind, time) first and removes it only once
the files are checked and <dest>/.sparkbox.json is written, so a folder left partway reads as
incomplete and can't be loaded. Its record says what was compared: checked "sha256" for a download
(Hugging Face's checksums), "sizes" for a copy over the fabric; a record with no checked is an
older download's.

A SIGKILL ends the job and nothing else, so its rsync or its docker CLI runs on, and that child is
what writes the folder. The job records the child's pid and its start time from /proc, and a new job
on the folder stops it first, after the containers: TERM to what the dead job left, up to 5 s, then
KILL. It stops only the process it recorded: same pid, same start time, still doing that job's work.
A pid used again is left alone. Whatever else is still writing the folder ends the new job instead,
with "An earlier copy may still be writing to <dest>.", because there's never a second writer: a job
still running on it, an rsync the record doesn't vouch for, another user's, the other SparkBox's, or
one that won't die. Delete refuses with the same words while such an rsync, or the CLI of the job on
the folder's marker, is there. It doesn't ask docker: a container whose CLI has gone isn't looked for.

The job records its own start time beside its pid too, so a job that's gone (a reboot, a power cut)
isn't taken for whatever has its pid now: that process holds no folder and Cancel doesn't signal it.
A job an older sparkjob.py started has no start time on record, and only its pid to go on.
"""
import fcntl
import fnmatch
import hashlib
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor

HOME = os.path.expanduser("~")
JOBS = os.path.join(HOME, ".sparkbox", "jobs")
SECRETS = os.path.join(HOME, ".sparkbox", "job-secrets")
RSYNCD = os.path.join(HOME, ".sparkbox", "rsyncd")
DEPLOY = os.path.join(HOME, ".sparkbox", "deploy")
KEYS = os.path.join(HOME, ".sparkbox", "keys")
PROC = "/proc"
SYS = "/sys"
MANIFEST = ".sparkbox.json"
PARTIAL = ".sparkbox.partial"
TOKEN_MOUNT = "/run/sparkbox/hf_token"
WEIGHT_EXT = (".safetensors", ".bin", ".gguf", ".pt", ".pth")


# ---- small helpers

def now():
    return int(time.time())


def out(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()


def status_path(job_id):
    return os.path.join(JOBS, job_id + ".json")


def write_status(job_id, **fields):
    os.makedirs(JOBS, exist_ok=True)
    path = status_path(job_id)
    # start() and the job it forked both write the status: one read-modify-write at a time
    # (flock), each through its own temporary file.
    with open(os.path.join(JOBS, ".status.lock"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            try:
                with open(path) as f:
                    st = json.load(f)
            except (OSError, ValueError):
                st = {"id": job_id, "startedAt": now()}
            st.update(fields)
            st["updatedAt"] = now()
            tmp = "%s.%d.tmp" % (path, os.getpid())
            with open(tmp, "w") as f:
                json.dump(st, f)
            os.replace(tmp, path)
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)
    return st


def valid_id(job_id):
    return 0 < len(job_id) <= 64 and all(c.isalnum() or c == "-" for c in job_id)


def scan_error(error):
    """An unreadable subtree is not an empty Library."""
    raise error


def folder_size(path, strict=False):
    total = 0
    for dirpath, _, names in os.walk(path, onerror=scan_error if strict else None):
        for n in names:
            try:
                total += os.stat(os.path.join(dirpath, n)).st_size
            except OSError:
                if strict:
                    raise
    return total


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def verify(root, files, job_id=None):
    """Checks sizes, and SHA-256 where known, of files relative to root (in parallel).
    Returns a list of problems (empty when everything matches)."""
    problems, to_hash = [], []
    for f in files:
        p = os.path.join(root, f["path"])
        try:
            size = os.stat(p).st_size
        except OSError:
            problems.append("missing " + f["path"])
            continue
        if f.get("size") is not None and size != f["size"]:
            problems.append("%s is %d bytes, expected %d" % (f["path"], size, f["size"]))
        elif f.get("sha256"):
            to_hash.append(f)
    if job_id:
        write_status(job_id, step="verifying", bytesDone=0, bytesTotal=sum(f.get("size") or 0 for f in to_hash))
    if to_hash:
        # The workers die with the job: a job's SIGTERM raises Stopped, which a worker would only
        # hand back before hashing its next file.
        with ProcessPoolExecutor(max_workers=min(8, os.cpu_count() or 2), initializer=signal.signal,
                                 initargs=(signal.SIGTERM, signal.SIG_DFL)) as pool:
            digests = pool.map(sha256_file, [os.path.join(root, f["path"]) for f in to_hash])
            done = 0
            for f, digest in zip(to_hash, digests):
                done += f.get("size") or 0
                if job_id:
                    write_status(job_id, bytesDone=done)
                if digest != f["sha256"]:
                    problems.append("%s has the wrong checksum" % f["path"])
    return problems


def listing(path):
    """Files under a model folder (symlinks followed), relative paths and sizes."""
    files = []
    for dirpath, _, names in os.walk(path):
        for n in names:
            full = os.path.join(dirpath, n)
            rel = os.path.relpath(full, path)
            if rel.startswith(".") and rel != MANIFEST:
                continue
            try:
                files.append({"path": rel, "size": os.stat(full).st_size})
            except OSError:
                pass
    return sorted(files, key=lambda f: f["path"])


def read_manifest(path):
    try:
        with open(os.path.join(path, MANIFEST)) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def write_manifest(path, repo, revision, files, checked="sha256"):
    with open(os.path.join(path, MANIFEST), "w") as f:
        json.dump({"repo": repo, "revision": revision, "files": files, "checked": checked, "verifiedAt": now()}, f)


def mark_partial(path, job_id, kind):
    """The folder is being written by this job, until its files are checked. Whole or not there:
    a marker that can't be read says the job has gone."""
    marker = os.path.join(path, PARTIAL)
    tmp = "%s.%d" % (marker, os.getpid())
    with open(tmp, "w") as f:
        json.dump({"job": job_id, "kind": kind, "at": now()}, f)
    os.replace(tmp, marker)


def read_marker(path):
    """What a folder's marker says, or {}: one that can't be read says the job has gone."""
    try:
        with open(os.path.join(path, PARTIAL)) as f:
            marker = json.load(f)
    except (OSError, ValueError):
        return {}
    return marker if isinstance(marker, dict) else {}


def marker_job(marker):
    """The job a folder's marker names, or None."""
    job = marker.get("job")
    return job if isinstance(job, str) and valid_id(job) else None


def finish_partial(path):
    try:
        os.unlink(os.path.join(path, PARTIAL))
    except FileNotFoundError:
        pass


def process_ids():
    """The pid of every process, or none where there's no /proc: only Linux has one."""
    try:
        return [int(n) for n in os.listdir(PROC) if n.isdigit()]
    except OSError:
        return []


def process_facts(pid):
    """What /proc says of a running process: its start time (stat's field 22, clock ticks since
    boot), its owner and its argv. None once it's gone or a zombie. Field 2, the command's name, can
    hold spaces and parentheses, so the fields are counted from the last ")"."""
    base = os.path.join(PROC, str(pid))
    try:
        with open(os.path.join(base, "stat"), "rb") as f:
            fields = f.read().rpartition(b")")[2].split()
        with open(os.path.join(base, "cmdline"), "rb") as f:
            argv = f.read().decode("utf-8", "surrogateescape").split("\0")[:-1]
        uid = os.stat(base).st_uid
    except OSError:
        return None
    if len(fields) < 20 or fields[0] in (b"Z", b"X"):
        return None
    return {"start": int(fields[19]), "uid": uid, "argv": argv}


def start_of(pid):
    """A running process's start time, what tells it from a later process with its pid."""
    facts = process_facts(pid)
    return facts["start"] if facts else None


def boot_id():
    try:
        with open(os.path.join(PROC, "sys/kernel/random/boot_id")) as f:
            return f.read().strip() or None
    except OSError:
        return None


def different_boot(st, child=False):
    saved = (st.get("childBootId") or st.get("bootId")) if child else st.get("bootId")
    current = boot_id()
    return bool(saved and current and saved != current)


def unverified_boot(st, child=False):
    """Never signal a recorded process if its saved boot cannot be matched now."""
    saved = (st.get("childBootId") or st.get("bootId")) if child else st.get("bootId")
    return bool(saved and saved != boot_id())


def note_child(job_id, proc):
    """The child on the job's record, with its start time."""
    write_status(job_id, childPid=proc.pid, childStart=start_of(proc.pid), childBootId=boot_id())


def stop_process(proc, stop_first=None):
    """Ends a job's child, if it's still running; stop_first runs before it does."""
    if proc.poll() is None:
        if stop_first:
            stop_first()
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


def run_watched(job_id, cmd, env, dest, total, stop_first=None, pass_fds=(), limit=None):
    """Runs cmd, reporting the size of dest as progress every 2 s. Returns its exit code and output tail."""
    log = open(os.path.join(JOBS, job_id + ".log"), "ab")
    proc = subprocess.Popen(cmd, env=env, stdout=log, stderr=subprocess.STDOUT, pass_fds=pass_fds)
    try:
        note_child(job_id, proc)
        last_bytes, last_t = folder_size(dest), time.time()
        deadline = time.monotonic() + limit if limit else None
        while proc.poll() is None:
            if deadline and time.monotonic() > deadline:
                raise RuntimeError("The selected tool exceeded its time limit.")
            time.sleep(2)
            size, t = folder_size(dest), time.time()
            rate = max(size - last_bytes, 0) / max(t - last_t, 0.001)
            last_bytes, last_t = size, t
            write_status(job_id, bytesDone=size, bytesTotal=total, rateBps=rate)
    finally:
        stop_process(proc, stop_first)
        log.close()
    return proc.returncode, tail(os.path.join(JOBS, job_id + ".log"))


def tail(path, n=6):
    try:
        with open(path, "rb") as f:
            lines = f.read()[-4000:].decode("utf-8", "replace").strip().splitlines()
        return " | ".join(l.strip() for l in lines[-n:])
    except OSError:
        return ""


# ---- jobs (run in the detached child)

def file_rel(f):
    """A spec file's path relative to the model folder (Hugging Face calls it the name)."""
    return f.get("name") or f.get("path")


def served_files(spec):
    """The spec's files as {path, size, sha256}, minus any the spec excludes."""
    patterns = spec.get("exclude") or []
    files = [dict(f, path=file_rel(f)) for f in spec.get("files") or [] if file_rel(f)]
    return [f for f in files if not rsync_excluded(f["path"], patterns)]


def rsync_excluded(rel, patterns):
    """Whether rsync's --exclude patterns drop rel (a path relative to the transfer root).
    Covers the forms SparkBox sends: a name or glob, a dir/ pattern, an anchored /path and a**/b."""
    parts = rel.split("/")
    for pat in patterns:
        dir_only = pat.endswith("/")
        pat = pat.rstrip("/")
        if not pat:
            continue
        if pat.startswith("/"):
            # Anchored: the whole path, or (for a folder) a leading part of it.
            for i in range(len(parts), 0, -1):
                if (i < len(parts) or not dir_only) and fnmatch.fnmatchcase("/".join(parts[:i]), pat[1:]):
                    return True
            continue
        if "/" in pat:
            # Unanchored with a slash: matches the end of the path (any tail of it).
            for i in range(len(parts)):
                for j in range(i + 1, len(parts) + 1):
                    if (j < len(parts) or not dir_only) and fnmatch.fnmatchcase("/".join(parts[i:j]), pat):
                        return True
            continue
        names = parts[:-1] if dir_only else parts
        if any(fnmatch.fnmatchcase(n, pat) for n in names):
            return True
    return False


# ---- explicit download tools (no discovery step executes a tool or starts a container)
UV_VERSION = "0.13.7"
UV_URL = "https://github.com/astral-sh/uv/releases/download/0.13.7/uv-aarch64-unknown-linux-gnu.tar.gz"
UV_SHA256 = "71279f8d3c67241e807b9d8db872d1bf541579fb9f08b7d1eb1682196d1e57dc"
HF_VERSION = "2.4.0"
XET_VERSION = "1.8.0"
TOOL_BUNDLE = "uv-%s-hf-%s-xet-%s" % (UV_VERSION, HF_VERSION, XET_VERSION)


def tool_base():
    return os.path.join(os.path.realpath(HOME), ".sparkbox")


def executable_tool(kind, path, label):
    path = os.path.realpath(path)
    try:
        if not os.path.isfile(path) or not os.access(path, os.X_OK) or os.path.getsize(path) > 100 * 1024 * 1024:
            return None
        with open(path, "rb") as f:
            digest = hashlib.sha256()
            for chunk in iter(lambda: f.read(1024 * 1024), b""):
                digest.update(chunk)
            digest = digest.hexdigest()
        return {"kind": kind, "path": path, "fingerprint": digest, "label": label}
    except OSError:
        return None


def managed_tool():
    base = tool_base()
    try:
        with open(os.path.join(base, "venv", ".sparkbox-tool.json")) as f:
            receipt = json.load(f)
        if receipt.get("bundle") != TOOL_BUNDLE or os.path.islink(base) or os.path.islink(os.path.join(base, "venv")):
            return None
        tool = executable_tool("managed", os.path.join(base, "venv", "bin", "hf"), "SparkBox hf " + HF_VERSION)
        expected = os.path.join(base, "venv", "bin", "hf")
        return tool if tool and tool["path"] == expected and tool["fingerprint"] == receipt.get("hfSha256") else None
    except (OSError, ValueError, AttributeError):
        return None


def install_tool_choice():
    return {"kind": "install", "path": os.path.join(tool_base(), "venv"), "fingerprint": TOOL_BUNDLE,
            "label": "Install SparkBox hf %s + hf-xet %s" % (HF_VERSION, XET_VERSION)}


def download_tools():
    import platform
    options, notes = [], []
    managed = managed_tool()
    host = executable_tool("host", shutil.which("hf") or "", "Existing hf on this node") if shutil.which("hf") else None
    if host and (not managed or host["path"] != managed["path"]):
        options.append(host)
    if managed:
        options.append(managed)
    elif os.path.lexists(os.path.join(tool_base(), "venv")) or os.path.islink(tool_base()):
        notes.append("The existing SparkBox tool folder is not a verified installation; it will not be replaced.")
    elif sys.platform.startswith("linux") and platform.machine() in ("aarch64", "arm64") and sys.version_info >= (3, 10):
        options.append(install_tool_choice())
    else:
        notes.append("Managed installation needs ARM64 Linux with Python 3.10 or newer already installed.")
    try:
        result = subprocess.run(["docker", "image", "ls", "--no-trunc", "--format", "{{json .}}"], capture_output=True, text=True, timeout=8)
        if result.returncode != 0:
            raise RuntimeError()
        seen = set()
        for line in result.stdout.splitlines():
            row = json.loads(line)
            ident = row.get("ID", "")
            if not re.fullmatch(r"sha256:[a-f0-9]{64}", ident) or ident in seen:
                continue
            seen.add(ident)
            label = ":".join([row.get("Repository", ""), row.get("Tag", "")])
            options.append({"kind": "image", "path": ident, "fingerprint": ident, "label": label})
    except (OSError, ValueError, AttributeError, RuntimeError, subprocess.TimeoutExpired):
        notes.append("Installed images could not be read. No image will be pulled.")
    return {"options": options, "notes": notes}


def check_download_tool(spec):
    selected = spec.get("tool")
    if not isinstance(selected, dict) or selected not in download_tools()["options"]:
        raise RuntimeError("The download tool is missing or changed. Review the tool choice again; nothing is installed or pulled automatically.")
    if selected["kind"] == "install" and spec.get("installConsent") is not True:
        raise RuntimeError("Installing the download tool needs explicit consent for this attempt.")
    return selected


def tool_environment():
    # The tool gets no inherited token, pip/uv index or Python injection setting.
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HF_", "HUGGING_FACE_", "UV_", "PIP_", "PYTHON"))}
    env.update(HF_HUB_DISABLE_TELEMETRY="1", HF_HUB_DISABLE_PROGRESS_BARS="1",
               HF_HUB_DISABLE_UPDATE_CHECK="1", UV_NO_CONFIG="1", UV_PYTHON_DOWNLOADS="never")
    return env


def install_download_tool(job_id, instance):
    """Build in a private staging folder and publish only a verified, previously absent venv.
    The installer lock is inherited by uv: killing the parent cannot admit a competing installer.
    No existing venv or uv binary is overwritten, and no system package/Python is installed."""
    import tarfile
    import tempfile
    import urllib.request
    base = tool_base()
    if os.path.islink(base):
        raise RuntimeError("SparkBox's tool folder is a symlink; installation refused.")
    os.makedirs(base, mode=0o700, exist_ok=True)
    with open(os.path.join(base, ".download-tool.lock"), "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("Another download-tool installation is still running. Wait, then review the tool choice again.")
        target, bin_dir = os.path.join(base, "venv"), os.path.join(base, "bin")
        if os.path.lexists(target) or os.path.islink(bin_dir):
            raise RuntimeError("An existing tool folder will not be replaced. Review the tool choice again.")
        stage = tempfile.mkdtemp(prefix="tool-" + job_id + "-", dir=base)
        write_status(job_id, toolStage=stage, step="installing the approved download tool")
        env = tool_environment()
        env.update(UV_CACHE_DIR=os.path.join(stage, "cache"), HF_HOME=os.path.join(stage, "hf-home"))
        try:
            archive = os.path.join(stage, "uv.tar.gz")
            with urllib.request.urlopen(UV_URL, timeout=30) as response, open(archive, "xb") as output:
                digest, size = hashlib.sha256(), 0
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    size += len(chunk)
                    if size > 32 * 1024 * 1024:
                        raise RuntimeError("The uv archive exceeded its size limit.")
                    digest.update(chunk)
                    output.write(chunk)
            if digest.hexdigest() != UV_SHA256:
                raise RuntimeError("The uv checksum did not match. Nothing was installed.")
            uv = os.path.join(stage, "uv")
            with tarfile.open(archive, "r:gz") as tar:
                member = tar.getmember("uv-aarch64-unknown-linux-gnu/uv")
                if not member.isfile() or member.size > 100 * 1024 * 1024:
                    raise RuntimeError("The verified uv archive has an unexpected binary.")
                with tar.extractfile(member) as source, open(uv, "xb") as output:
                    shutil.copyfileobj(source, output)
            os.chmod(uv, 0o700)
            venv = os.path.join(stage, "venv")
            commands = [
                [uv, "venv", "--no-config", "--no-python-downloads", "--relocatable", "--python", sys.executable, venv],
                [uv, "pip", "install", "--no-config", "--no-python-downloads", "--python", os.path.join(venv, "bin", "python"),
                 "--index-url", "https://pypi.org/simple", "--only-binary", ":all:",
                 "huggingface_hub[hf_xet]==" + HF_VERSION, "hf-xet==" + XET_VERSION],
            ]
            for cmd in commands:
                code, _ = run_watched(job_id, cmd, env, stage, 0, pass_fds=(lock.fileno(),), limit=600)
                if code:
                    raise RuntimeError("Download-tool installation failed. Read the job log; the model download did not start.")
            hf = os.path.join(venv, "bin", "hf")
            version = subprocess.run([hf, "version"], env=dict(env, HF_HUB_OFFLINE="1"), capture_output=True, text=True, timeout=15)
            if version.returncode or not re.search(r"(?<![\d.])" + re.escape(HF_VERSION) + r"(?![\d.])", version.stdout):
                raise RuntimeError("The installed hf version did not match the approved version.")
            tool = executable_tool("managed", hf, "SparkBox hf " + HF_VERSION)
            if not tool:
                raise RuntimeError("The installed hf executable could not be verified.")
            with open(os.path.join(venv, ".sparkbox-tool.json"), "x") as f:
                json.dump({"bundle": TOOL_BUNDLE, "instance": instance, "job": job_id, "hfSha256": tool["fingerprint"]}, f)
            os.makedirs(bin_dir, mode=0o700, exist_ok=True)
            saved_uv = os.path.join(bin_dir, "uv-" + UV_VERSION)
            # link, never replace: an unrelated binary with this name refuses installation.
            if os.path.lexists(saved_uv):
                if os.path.islink(saved_uv) or executable_tool("uv", saved_uv, "") != dict(executable_tool("uv", uv, ""), path=saved_uv):
                    raise RuntimeError("An existing uv binary differs; it will not be replaced.")
            else:
                os.link(uv, saved_uv)
            os.mkdir(target, 0o700)  # exclusive: never replace even an empty existing folder
            try:
                names = os.listdir(venv)
                for name in sorted(names, key=lambda n: n == ".sparkbox-tool.json"):
                    os.rename(os.path.join(venv, name), os.path.join(target, name))
            except BaseException:
                shutil.rmtree(target)  # only the directory created by this attempt
                raise
            return managed_tool()
        finally:
            shutil.rmtree(stage, ignore_errors=True)


def prepare_download_tool(job_id, spec):
    tool = check_download_tool(spec)
    write_status(job_id, toolUsed=tool["label"], toolIdentity=tool)
    if tool["kind"] == "install":
        tool = install_download_tool(job_id, spec["instance"])
    if not tool:
        raise RuntimeError("The selected download tool could not be verified.")
    write_status(job_id, toolUsed=tool["label"], toolIdentity=tool, step="checking the selected hf tool")
    env = dict(tool_environment(), HF_HUB_OFFLINE="1")
    if tool["kind"] == "image":
        command = download_cmd(job_id, dict(spec, image=tool["path"]), os.path.expanduser(spec["dest"]))
        command = command[:command.index(tool["path"]) + 1] + ["version"]
        at = command.index("--entrypoint")
        command[at:at] = ["--network", "none"]
    else:
        command = [tool["path"], "version"]
    import tempfile
    log_path = os.path.join(JOBS, job_id + ".log")
    probe_offset = os.path.getsize(log_path) if os.path.exists(log_path) else 0
    with tempfile.TemporaryDirectory(prefix="hf-probe-", dir=tool_base()) as probe:
        env["HF_HOME"] = probe
        stop_container = (lambda: stop_download(job_id, spec["instance"])) if tool["kind"] == "image" else None
        code, _ = run_watched(job_id, command, env, os.path.expanduser(spec["dest"]), 0,
                              stop_container, limit=30)
    if code:
        raise RuntimeError("The selected tool could not run hf version. Choose another tool; no fallback was started.")
    # Only this probe's output counts; earlier installer output is not an hf version.
    with open(log_path, "rb") as log:
        log.seek(max(probe_offset, os.path.getsize(log_path) - 4000))
        lines = log.read(4000).decode("utf-8", "replace").strip().splitlines()
    version = lines[-1].strip()[:160] if lines else ""
    if not version:
        raise RuntimeError("The selected tool did not report an hf version. No download was started.")
    write_status(job_id, toolVersion=version)
    return tool


def hf_to(argv, dest):
    """A host hf download writing this exact destination, including its Python shebang form."""
    if not argv or "download" not in argv or "--local-dir" not in argv:
        return False
    tool = any(os.path.basename(a) == "hf" or a == "huggingface_hub.cli.hf" for a in argv[:3])
    at = argv.index("--local-dir") + 1
    return tool and at < len(argv) and os.path.realpath(argv[at]) == dest


def tool_install_child(argv, st):
    stage = st.get("toolStage", "")
    return bool(stage and argv and os.path.join(stage, "uv") in argv[:2])


def download_cmd(job_id, spec, dest, token_file=None):
    """The hf download command, run inside the explicitly selected local image so files
    stay owned by this user. Each wanted file and each include pattern is an --include. A token
    is a read-only mounted file that HF_TOKEN_PATH names: with -e or --env-file it would be in the
    container's config. The name and labels are how stop_download finds it."""
    cmd = ["docker", "run", "--rm", "--name", "sparkbox-dl-" + job_id, "--label", "sparkbox.instance=" + spec["instance"],
           "--label", "sparkbox.deployment=" + job_id, "--user", "%d:%d" % (os.getuid(), os.getgid())]
    if token_file:
        cmd += ["-e", "HF_TOKEN_PATH=" + TOKEN_MOUNT]
    cmd += ["-e", "HF_HOME=/tmp/hf", "-e", "HF_HUB_DISABLE_PROGRESS_BARS=1", "-e", "HF_HUB_DISABLE_UPDATE_CHECK=1", "-e", "HF_HUB_DISABLE_TELEMETRY=1"]
    if token_file:
        cmd += ["-v", "%s:%s:ro" % (token_file, TOKEN_MOUNT)]
    cmd += ["--pull", "never", "-v", "%s:/dest" % dest, "--entrypoint", "hf", spec["image"],
            "download", spec["repo"], "--revision", spec["revision"], "--local-dir", "/dest"]
    wanted = [file_rel(f) for f in spec.get("files") or [] if file_rel(f)] + list(spec.get("include") or [])
    for pattern in dict.fromkeys(wanted):
        cmd += ["--include", pattern]
    return cmd


def has_hf(image, instance):
    """An explicitly requested test-container check; not used for download selection."""
    ran = in_test_container(image, instance, "hf", "--help")
    return ran is not None and ran.returncode == 0


def write_token(job_id, token):
    """The token in a 0400 file under a 0700 folder, for this job alone (drop_secrets removes it)."""
    os.makedirs(SECRETS, mode=0o700, exist_ok=True)
    base = os.path.join(SECRETS, job_id)
    os.mkdir(base, 0o700)
    path = os.path.join(base, "hf_token")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
    with os.fdopen(fd, "w") as f:
        f.write(token)
    return path


DOCKER_PS_S = 5
DOCKER_STOP_S = 10


def labelled_containers(*labels, timeout=None):
    """Ids of the running containers that have every label, or None when docker can't say: "can't
    say" is never "none". No docker at all has no containers."""
    filters = [w for label in labels for w in ("--filter", "label=" + label)]
    try:
        listed = subprocess.run(["docker", "ps", "-q"] + filters, capture_output=True, text=True, timeout=timeout or DOCKER_PS_S)
    except OSError:
        return []
    except subprocess.TimeoutExpired:
        return None
    return listed.stdout.split() if listed.returncode == 0 else None


def stop_download(job_id, instance):
    """Stops this SparkBox's container of a download job, if it has one running: exactly what docker
    lists with both labels, never a name alone. Ending the CLI wouldn't end it (see the top of this
    file). Nothing listed is fine; stop_leftover and cancel look again afterwards."""
    if not instance or not valid_id(job_id):
        return
    ids = labelled_containers("sparkbox.deployment=" + job_id, "sparkbox.instance=" + instance)
    if ids:
        try:
            subprocess.run(["docker", "stop", "-t", "5"] + ids, capture_output=True, timeout=DOCKER_STOP_S)
        except (OSError, subprocess.TimeoutExpired):
            pass


ORPHAN_TERM_S = 5
ORPHAN_KILL_S = 2


def rsync_to(argv, real):
    """argv is an rsync writing to the folder `real`: its last word, and neither a server nor the
    sending end."""
    return (len(argv) > 2 and os.path.basename(argv[0]) == "rsync" and "--daemon" not in argv and "--sender" not in argv
            and os.path.realpath(argv[-1]) == real)


def docker_run_of(argv, job):
    """argv is the docker CLI of a download job's container: `docker run --name sparkbox-dl-<job>`."""
    return bool(argv) and os.path.basename(argv[0]) == "docker" and "sparkbox-dl-" + job in argv


def writers(dest, job):
    """Whatever is writing to dest, or about to, whoever started it and whether or not a record
    says so: an rsync with dest as its destination, and the docker CLI of the job on the folder's
    marker. That CLI can't be asked for its container: it may not have one yet."""
    real = os.path.realpath(dest)
    found = []
    for pid in process_ids():
        facts = process_facts(pid)
        if facts and (rsync_to(facts["argv"], real) or hf_to(facts["argv"], real) or (job and docker_run_of(facts["argv"], job))):
            found.append(pid)
    return found


def orphan(st):
    """What a job that's over left running, if anything: the child on its record, while that's
    still the same process (the start time) doing the job's work. Its pid used again is someone
    else's; a child with no start time on record can't be told from them, and comes to nothing here."""
    pid, started, dest = st.get("childPid"), st.get("childStart"), st.get("dest")
    if started is None or not dest or job_running(st) or unverified_boot(st, child=True):
        return None
    facts = process_facts(pid)
    if not facts or facts["start"] != started:
        return None
    work = rsync_to(facts["argv"], os.path.realpath(dest)) or docker_run_of(facts["argv"], st.get("id", "")) or hf_to(facts["argv"], os.path.realpath(dest)) or tool_install_child(facts["argv"], st)
    return facts if work else None


def copy_refused(dest):
    return RuntimeError("An earlier copy may still be writing to %s." % dest)


def stop_orphan(st, dest, instance):
    """Ends what a killed job left: TERM to the group the job made for itself (a real rsync is two
    processes), up to 5 s, then KILL. That group can't be someone else's while the child it's
    confirmed for is in it. One this SparkBox may not stop ends the new job; stop_leftover's last look
    finds one that won't stop."""
    child = orphan(st)
    if child is None:
        return
    group = st.get("pid")
    try:
        ours = st.get("instance", "") == instance and child["uid"] == os.getuid() and os.getpgid(st["childPid"]) == group
    except OSError:
        return
    if not ours:
        raise copy_refused(dest)
    for sig, wait in ((signal.SIGTERM, ORPHAN_TERM_S), (signal.SIGKILL, ORPHAN_KILL_S)):
        if group_gone(group):
            return
        try:
            os.killpg(group, sig)
        except OSError:
            return
        deadline = time.time() + wait
        while not group_gone(group) and time.time() < deadline:
            time.sleep(0.05)


def uses_download_container(st):
    # Unknown and legacy records stay conservative. Only an explicit host route rules out Docker.
    return st.get("toolIdentity", {}).get("kind") not in ("host", "managed", "install")


def stop_leftover(dest, instance):
    """The job that left this folder partway may have left its container or its child writing to it
    (a SIGKILL runs no finally). They stop before a new job touches the folder, so there's never a
    second writer. A job still running on it, one this SparkBox may not stop, or one that won't stop
    ends the new job instead, and so does a docker that can't say and any writer no record vouches
    for. A copy has no container, so its marker asks nothing of docker."""
    marker = read_marker(dest)
    job = marker_job(marker)
    st = read_status(job) if job else {}
    if job_running(st):
        raise copy_refused(dest)
    download = job and marker.get("kind") != "pull" and uses_download_container(st)
    if download:
        stop_download(job, instance)
    if job:
        stop_orphan(st, dest, instance)
    if download:
        left = labelled_containers("sparkbox.deployment=" + job)
        if left is None:
            raise RuntimeError("Docker didn't answer, so SparkBox can't tell whether an earlier download is still writing to this folder.")
        if left:
            raise RuntimeError("An earlier download is still writing to this folder: container sparkbox-dl-%s is running." % job)
    if writers(dest, job):
        raise copy_refused(dest)


def claim_folder(dest, job_id, kind, instance, resume=None, disk_check=None):
    """This job becomes the one writer of dest, or raises. One job at a time looks and marks: two
    started together would both find the folder free."""
    os.makedirs(dest, exist_ok=True)
    with open(os.path.join(JOBS, ".folders.lock"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if resume is not None:
            check_download_resume(resume)
        if disk_check is not None:
            disk_check()
        stop_leftover(dest, instance)
        mark_partial(dest, job_id, kind)


def chain_empty(dest):
    """Read-only fresh-folder admission, repeated while holding the writer lock."""
    dest = os.path.expanduser(dest)
    if (not os.path.isabs(dest) or os.path.normpath(dest) != dest or dest in ("/", HOME)
            or os.path.realpath(dest) != dest):
        raise RuntimeError("A chain needs an absolute destination without symlinks or parent traversal.")
    if os.path.lexists(dest) and (not os.path.isdir(dest) or os.listdir(dest)):
        raise RuntimeError("A chain needs an empty destination: " + dest + ". Keep existing files; use the Library or a separately reviewed download to recover.")
    return dest


def chain_preflight(spec):
    dest = chain_empty(spec["dest"])
    check = destination_disk(dest, spec["bytes"], spec.get("node", "this node"))
    return {"dest": dest, "disk": check, "rsync": shutil.which("rsync") is not None, "fabric": fabric()}


def checked_chain(job_id, spec, kind):
    """Persist only this non-secret exact participant list; never replay it on startup."""
    chain = spec.get("chain")
    if not isinstance(chain, dict) or set(chain) != {"id", "source", "repo", "revision", "expiresAt", "members"}:
        raise RuntimeError("Invalid chain plan.")
    members = chain["members"]
    if (not isinstance(chain["id"], str) or not re.fullmatch(r"dc-[A-Za-z0-9-]{1,37}", chain["id"])
            or type(chain["expiresAt"]) is not int or not now() < chain["expiresAt"] <= now() + 86400
            or not isinstance(members, list) or not 2 <= len(members) <= 17 or not instance_ok(spec.get("instance", ""))):
        raise RuntimeError("The chain plan is invalid or expired. Review a new attempt.")
    nodes = set()
    for index, member in enumerate(members):
        if (not isinstance(member, dict) or set(member) != {"node", "id", "dest"}
                or not isinstance(member["node"], str) or not member["node"] or member["node"] in nodes
                or member["id"] != "%s-%d" % (chain["id"], index)
                or not isinstance(member["dest"], str) or not member["dest"].startswith("/")
                or any(ord(c) < 32 for c in member["dest"])):
            raise RuntimeError("Invalid or repeated chain participant.")
        nodes.add(member["node"])
    own = next((m for m in members if m["id"] == job_id and m["node"] == spec.get("node")), None)
    if (not own or own["dest"] != os.path.expanduser(spec.get("dest", "")) or chain["source"] != members[0]["node"]
            or (kind == "download") != (own == members[0]) or chain["repo"] != spec.get("repo") or chain["revision"] != spec.get("revision")
            or not download_plan(spec) or spec.get("resumeOf") or not shutil.which("rsync")
            or not re.fullmatch(r"[A-Za-z0-9]{32,128}", str(spec.get("password", "")))):
        raise RuntimeError("The chain participant, files, tool prerequisites or secret are invalid.")
    transfer_bytes(spec["files"])
    if any(part.startswith(".") for f in spec["files"] for part in f["path"].split("/")):
        raise RuntimeError("A chain cannot copy hidden paths.")
    if kind == "download":
        import ipaddress
        try:
            source = ipaddress.IPv4Address(spec["address"])
            allowed = [ipaddress.IPv4Address(ip) for ip in spec["allow"].split(" ")]
            if source.is_loopback or source.is_unspecified or len(allowed) != len(members)-1 or len(set(allowed)) != len(allowed) or source in allowed:
                raise ValueError()
            if type(spec["port"]) is not int or not 8730 <= spec["port"] < 8930:
                raise ValueError()
        except (KeyError, ValueError):
            raise RuntimeError("Invalid chain fabric listener or target allow list.") from None
    return chain


def chain_manifest(spec, manifest):
    """Fresh source hashes must still name exactly the inspected publisher files."""
    chain = spec["chain"]
    files = validate_copy_manifest(manifest)
    if (manifest.get("chainId") != chain["id"] or manifest.get("repo") != chain["repo"] or manifest.get("revision") != chain["revision"]):
        raise RuntimeError("The copy source belongs to another chain or revision.")
    expected = {f["path"]: f for f in spec["files"]}
    if {f["path"] for f in files} != set(expected) or any(f["size"] != expected[f["path"]]["size"]
            or (expected[f["path"]].get("sha256") and f["sha256"] != expected[f["path"]]["sha256"]) for f in files):
        raise RuntimeError("The source file list or publisher checksum changed. Nothing was copied.")
    return files


def job_download(job_id, spec):
    dest = os.path.expanduser(spec["dest"])
    if spec.get("chain"):
        checked_chain(job_id, spec, "download")
    instance = spec.get("instance", "")
    if not instance_ok(instance):
        raise RuntimeError("A download needs its SparkBox's instance name.")
    check_download_tool(spec)
    files = served_files(dict(spec, exclude=[]))
    total = transfer_bytes(files)
    def disk_check():
        if spec.get("chain"): chain_empty(dest)
        return require_destination_disk(dest, total, spec.get("node", "this node"))
    disk_check()
    claim_folder(dest, job_id, "download", instance, spec if spec.get("resumeOf") else None, disk_check)
    tool = prepare_download_tool(job_id, spec)
    env = tool_environment()
    token_file = write_token(job_id, spec["token"]) if spec.get("token") else None
    if tool["kind"] == "image":
        cmd = download_cmd(job_id, dict(spec, image=tool["path"]), dest, token_file)
    else:
        env.update(HF_HOME=os.path.join(tool_base(), "hf-home"),
                   HF_TOKEN_PATH=token_file or os.path.join(SECRETS, job_id, "no-token"))
        cmd = [tool["path"], "download", spec["repo"], "--revision", spec["revision"], "--local-dir", dest]
        for pattern in dict.fromkeys([file_rel(f) for f in spec.get("files") or [] if file_rel(f)] + list(spec.get("include") or [])):
            cmd += ["--include", pattern]
    write_status(job_id, step="downloading", bytesDone=folder_size(dest), bytesTotal=total)
    stop_container = (lambda: stop_download(job_id, instance)) if tool["kind"] == "image" else None
    code, last = run_watched(job_id, cmd, env, dest, total, stop_container)
    if code != 0:
        raise RuntimeError("hf download failed (exit %d): %s" % (code, last))
    problems = verify(dest, files, job_id)
    if problems:
        raise RuntimeError("Download incomplete: " + "; ".join(problems[:5]))
    write_manifest(dest, spec["repo"], spec["revision"], files)
    finish_partial(dest)
    if spec.get("chain"):
        # The Hub token is no longer needed while copies run. Never persist it in the plan.
        shutil.rmtree(os.path.join(SECRETS, job_id), ignore_errors=True)
        spec.pop("token", None)
        if now() >= spec["chain"]["expiresAt"]:
            raise RuntimeError("Download verified, but the chain expired before sharing. Use the Library to copy it after review.")
        return job_serve_v2(job_id, dict(spec, path=dest, protocol=2, expected=[m["id"] for m in spec["chain"]["members"][1:]]))
    return {"path": dest}


def rsyncd_conf(base, path, secrets, allow, exclude):
    """rsyncd.conf for a serve job; excluded patterns go in a file next to it."""
    conf = ("use chroot = no\npid file = %s/rsyncd.pid\n[model]\n  path = %s\n  read only = yes\n"
            "  auth users = sparkbox\n  secrets file = %s\n  hosts allow = %s\n" % (base, path, secrets, allow))
    if exclude:
        excludes = os.path.join(base, "exclude")
        with open(excludes, "w") as f:
            f.write("".join(p + "\n" for p in exclude))
        conf += "  exclude from = %s\n" % excludes
    return conf


def job_serve(job_id, spec):
    if spec.get("protocol") == 2:
        return job_serve_v2(job_id, spec)
    path = os.path.expanduser(spec["path"])
    if not os.path.isdir(path):
        raise RuntimeError("no model folder at " + path)
    base = os.path.join(RSYNCD, job_id)
    os.makedirs(base, exist_ok=True)
    secrets = os.path.join(base, "secrets")
    fd = os.open(secrets, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write("sparkbox:%s\n" % spec["password"])
    conf = os.path.join(base, "rsyncd.conf")
    with open(conf, "w") as f:
        f.write(rsyncd_conf(base, path, secrets, spec.get("allow", "192.168.0.0/16"), spec.get("exclude") or []))
    write_status(job_id, step="serving", message="Sharing %s on %s:%d" % (path, spec["address"], spec["port"]))
    proc = subprocess.Popen(["rsync", "--daemon", "--no-detach", "--config=" + conf,
                             "--address=" + spec["address"], "--port=%d" % spec["port"]],
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    note_child(job_id, proc)
    try:
        proc.wait(timeout=spec.get("ttl", 12 * 3600))
        err = proc.stderr.read().decode("utf-8", "replace").strip()
        if proc.returncode not in (0, -15):
            raise RuntimeError("rsync server stopped: " + err[-300:])
    except subprocess.TimeoutExpired:
        pass
    finally:
        stop_process(proc)
        shutil.rmtree(base, ignore_errors=True)
    return {}


# What a pull neither copies nor deletes: the record, the marker and hf's unfinished files.
PULL_EXCLUDE = ["/" + MANIFEST, "/" + PARTIAL, "/.cache/"]


def job_pull(job_id, spec):
    if spec.get("protocol") == 2:
        return job_pull_v2(job_id, spec)
    dest = os.path.expanduser(spec["dest"])
    exclude = list(spec.get("exclude") or []) + PULL_EXCLUDE
    files = served_files(dict(spec, exclude=exclude))
    total = transfer_bytes(files)
    disk_check = lambda: require_destination_disk(dest, total, spec.get("node", "this node"))
    disk_check()
    claim_folder(dest, job_id, "pull", spec.get("instance", ""), disk_check=disk_check)
    # rsync's --delete skips excluded files, so an older record would outlive this copy.
    try:
        os.unlink(os.path.join(dest, MANIFEST))
    except FileNotFoundError:
        pass
    env = dict(os.environ, RSYNC_PASSWORD=spec["password"])
    cmd = ["rsync", "-aL", "--partial", "--delete"] + ["--exclude=" + p for p in exclude]
    cmd += [spec["src"].rstrip("/") + "/", dest + "/"]
    write_status(job_id, step="copying", bytesDone=folder_size(dest), bytesTotal=total)
    code, last = run_watched(job_id, cmd, env, dest, total)
    if code != 0:
        raise RuntimeError("copy failed (rsync exit %d): %s" % (code, last))
    problems = verify(dest, [{"path": f["path"], "size": f.get("size"), "sha256": f.get("sha256") if spec.get("checksums") else None}
                             for f in files], job_id)
    if problems:
        raise RuntimeError("Copy incomplete: " + "; ".join(problems[:5]))
    write_manifest(dest, spec.get("repo", ""), spec.get("revision", ""), files, checked="sizes")
    finish_partial(dest)
    return {"path": dest}


# Protocol 2: the source owns its lifetime and the target verifies a fresh manifest.
COPY_EXCLUDE_V2 = PULL_EXCLUDE + ["/.*"]


def copy_manifest(path, exclude, job_id):
    from concurrent.futures import ThreadPoolExecutor
    files = served_files({"files": listing(path), "exclude": list(exclude) + COPY_EXCLUDE_V2})
    root = os.path.realpath(path)
    # Hugging Face snapshots link to their own repository's content-addressed blobs.
    # Permit exactly those files, never arbitrary links elsewhere on the node.
    snapshots = os.path.dirname(root)
    repository = os.path.dirname(snapshots)
    blobs = os.path.join(repository, "blobs")
    hf = os.path.basename(snapshots) == "snapshots" and os.path.basename(repository).startswith("models--") and os.path.realpath(blobs) == blobs
    for directory, dirs, _ in os.walk(path):
        if any(os.path.islink(os.path.join(directory, d)) for d in dirs):
            raise RuntimeError("Directory symlinks in a copy source need manual review.")
    for f in files:
        real = os.path.realpath(os.path.join(path, f["path"]))
        own_blob = hf and os.path.dirname(real) == blobs and re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", os.path.basename(real))
        if (os.path.commonpath([root, real]) != root and not own_blob) or not os.path.isfile(real):
            raise RuntimeError("A source file escapes the shared folder: " + f["path"])
    write_status(job_id, step="hashing", bytesDone=0, bytesTotal=sum(f["size"] for f in files))
    with ThreadPoolExecutor(max_workers=8) as pool:
        hashes = pool.map(sha256_file, [os.path.join(path, f["path"]) for f in files])
        done = 0
        for f, digest in zip(files, hashes):
            f["sha256"] = digest
            done += f["size"]
            write_status(job_id, bytesDone=done)
    return files


def validate_copy_manifest(value):
    files = value.get("files") if isinstance(value, dict) else None
    if not isinstance(files, list) or not files or len(files) > 100000:
        raise RuntimeError("The source manifest is empty or invalid.")
    seen = set()
    for f in files:
        path = f.get("path") if isinstance(f, dict) else None
        if (not isinstance(path, str) or not path or path.startswith("/") or "\\" in path
                or any(p in ("", ".", "..") for p in path.split("/")) or any(ord(c) < 32 for c in path)
                or path in seen or path in (MANIFEST, PARTIAL)):
            raise RuntimeError("Unsafe or repeated path in the source manifest.")
        if (not isinstance(f.get("size"), int) or isinstance(f["size"], bool) or f["size"] < 0
                or not re.fullmatch(r"[0-9a-f]{64}", str(f.get("sha256", "")))):
            raise RuntimeError("The source manifest has no valid size and SHA-256 for " + path)
        seen.add(path)
    return files


def transfer_bytes(files):
    """Unknown sizes are not zero; partial files do not earn unverified space credit."""
    if not isinstance(files, list) or not files:
        raise RuntimeError("The transfer has no known file sizes. Refresh its details before starting.")
    sizes = [f.get("size") if isinstance(f, dict) else None for f in files]
    if any(type(n) is not int or n < 0 for n in sizes) or sum(sizes) > 2**53 - 1:
        raise RuntimeError("The transfer has unknown or invalid file sizes. Refresh its details before starting.")
    return sum(sizes)


def destination_disk(dest, size, node="this node", reserve=20 * 1024**3):
    """Read the actual destination filesystem, including a missing folder's existing ancestor.
    Never create a folder, follow a broken link as a fallback, or count root-reserved blocks."""
    if (not isinstance(dest, str) or not dest or any(ord(c) < 32 for c in dest)
            or type(size) is not int or not 0 <= size <= 2**53 - 1):
        raise RuntimeError("Choose a destination folder and a known transfer size.")
    dest = os.path.expanduser(dest)
    if not os.path.isabs(dest):
        raise RuntimeError("The destination must be an absolute path or start with ~/.")
    # Keep '..' until the filesystem resolves it: collapsing it before a symlink
    # could inspect the wrong mount. Strip only trailing separators for ascent.
    probe = dest.rstrip("/") or "/"
    try:
        while True:
            try:
                os.lstat(probe)
                break
            except FileNotFoundError:
                parent = os.path.dirname(probe)
                if parent == probe:
                    raise RuntimeError("Cannot find the destination filesystem.")
                probe = parent
        if not os.path.isdir(probe):
            raise RuntimeError("The destination ancestor is not an accessible folder (it may be a broken link).")
        probe = os.path.realpath(probe, strict=True)
        st = os.statvfs(probe)
        if st.f_flag & getattr(os, "ST_RDONLY", 1):
            raise RuntimeError("The destination filesystem is read-only.")
        available, capacity = st.f_bavail * st.f_frsize, st.f_blocks * st.f_frsize
        if st.f_frsize <= 0 or capacity <= 0 or not 0 <= available <= capacity:
            raise RuntimeError("The destination returned invalid capacity or available-space readings.")
    except (OSError, RuntimeError) as e:
        raise RuntimeError("Cannot check destination disk on %s at %s: %s" % (node, dest, e)) from e
    need = (size * 105 + 99) // 100 + reserve
    used = 100 * (capacity - available) / capacity
    projected = min(100, 100 * (capacity - available + size) / capacity)
    warning = ("This disk is %.1f%% full and could be %.1f%% full after the transfer." % (used, projected)) if projected > 85 else None
    return {"node": node, "dest": dest, "probe": probe, "transferBytes": size, "availableBytes": available,
            "capacityBytes": capacity, "requiredBytes": need, "usedPercent": used, "projectedUsedPercent": projected,
            "ok": available >= need, "warning": warning}


def require_destination_disk(dest, size, node="this node", reserve=20 * 1024**3):
    check = destination_disk(dest, size, node, reserve)
    if not check["ok"]:
        raise RuntimeError("Not enough disk space on %s at %s: %s available; %s required (full transfer plus 5%% and 20 GiB)." %
                           (node, check["dest"], human(check["availableBytes"]), human(check["requiredBytes"])))
    return check


def copy_disk_check(dest, size, reserve=20 * 1024**3):
    return require_destination_disk(dest, size, reserve=reserve)


def disk_failure(spec, error):
    """A filesystem can fill after admission. Keep the failed job and name its destination."""
    message = str(error)
    if not spec.get("dest") or (getattr(error, "errno", None) != 28 and "no space left on device" not in message.lower()):
        return message
    node, dest = spec.get("node", "this node"), spec.get("dest", "")
    try:
        free = human(destination_disk(dest, 0, node)["availableBytes"])
    except (OSError, RuntimeError):
        free = "unknown space"
    return "No space left on device on %s. Destination %s reports %s available now. Check the destination and tool/cache disks, then review a new attempt. Partial files are retained." % (node, dest, free)


def parent_death_guard(parent):
    """Linux owns the listener even if its detached job is killed without cleanup."""
    import ctypes
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGTERM, 0, 0, 0) != 0:  # PR_SET_PDEATHSIG
        os._exit(125)
    if os.getppid() != parent:  # Parent died between fork and prctl.
        os._exit(125)


def job_serve_v2(job_id, spec):
    try:
        return _job_serve_v2(job_id, spec)
    finally:
        shutil.rmtree(os.path.join(RSYNCD, job_id), ignore_errors=True)


def firewall_hint(port):
    """Read only; never open a port or ask for sudo from a detached copy job."""
    if not shutil.which("ufw"):
        return "Firewall rules were not available to this job."
    try:
        result = subprocess.run(["ufw", "status"], capture_output=True, text=True, timeout=4)
        if result.returncode == 0 and "Status: inactive" in result.stdout:
            return "ufw reports inactive; other network filters may still apply."
        if result.returncode == 0 and "Status: active" in result.stdout:
            return "ufw is active: verify TCP %d is allowed from the copy targets. No rules were changed." % port
    except (OSError, subprocess.TimeoutExpired):
        pass
    return "Firewall rules could not be read without privileges; review them if the targets cannot connect."


def rsync_group_config():
    # An explicit gid makes Linux rsync call setgroups even for our current gid.
    # Ordinary-user jobs must retain their existing groups; no sudo is needed.
    return "gid = %d\n" % os.getgid() if os.geteuid() == 0 else ""


def _job_serve_v2(job_id, spec):
    import socket
    path = os.path.realpath(os.path.expanduser(spec["path"]))
    expected = spec.get("expected", [])
    if (not os.path.isdir(path) or not expected or any(not valid_id(n) for n in expected)
            or len(set(expected)) != len(expected)):
        raise RuntimeError("A share needs a source folder and distinct expected copy jobs.")
    if any(not isinstance(p, str) or any(c.isspace() or c == "\0" for c in p) for p in spec.get("exclude", [])):
        raise RuntimeError("Invalid sharing exclusions.")
    base = os.path.join(RSYNCD, job_id)
    for value in (base, path, spec["address"], spec.get("allow", ""), spec["password"]):
        if any(c in value for c in "\r\n\0"):
            raise RuntimeError("Invalid sharing configuration.")
    os.makedirs(RSYNCD, mode=0o700, exist_ok=True)
    os.chmod(RSYNCD, 0o700)
    os.makedirs(base, mode=0o700, exist_ok=True)
    meta, done_dir = os.path.join(base, "meta"), os.path.join(base, "done")
    os.makedirs(meta, mode=0o700, exist_ok=True)
    os.makedirs(done_dir, mode=0o700, exist_ok=True)
    files = copy_manifest(path, spec.get("exclude") or [], job_id)
    manifest = {"files": files, "repo": spec.get("repo", ""), "revision": spec.get("revision", ""), "image": spec.get("imageMetadata")}
    if spec.get("chain"):
        manifest["chainId"] = spec["chain"]["id"]
        chain_manifest(spec, manifest)
    with open(os.path.join(meta, "manifest.json"), "w") as f:
        json.dump(manifest, f)
    secret = os.path.join(base, "secrets")
    fd = os.open(secret, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write("sparkbox:" + spec["password"] + "\n")
    conf = os.path.join(base, "rsyncd.conf")
    log = os.path.join(base, "activity.log")
    common = "  auth users = sparkbox\n  secrets file = %s\n  hosts allow = %s\n" % (secret, spec.get("allow", spec["address"]))
    with open(conf, "w") as f:
        f.write("use chroot = no\nuid = %d\n%slog file = %s\ntransfer logging = yes\ntimeout = 120\n" % (os.getuid(), rsync_group_config(), log))
        f.write("[model]\n  path = %s\n  read only = yes\n%s" % (path, common))
        f.write("  exclude = %s\n" % " ".join(list(spec.get("exclude") or []) + COPY_EXCLUDE_V2))
        f.write("[meta]\n  path = %s\n  read only = yes\n%s" % (meta, common))
        f.write("[done]\n  path = %s\n  read only = no\n  list = no\n%s" % (done_dir, common))
    parent = os.getpid()
    proc = subprocess.Popen(["rsync", "--daemon", "--no-detach", "--config=" + conf,
                             "--address=" + spec["address"], "--port=%d" % spec["port"]],
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                            preexec_fn=(lambda: parent_death_guard(parent)) if sys.platform == "linux" else None)
    note_child(job_id, proc)
    began, last_activity = time.monotonic(), time.monotonic()
    try:
        # Publish serving only after the actual listener accepts a connection.
        while True:
            if proc.poll() is not None:
                raise RuntimeError("The copy server could not start: " + proc.stderr.read().decode(errors="replace")[-300:])
            try:
                with socket.create_connection((spec["address"], spec["port"]), timeout=1):
                    break
            except OSError:
                if time.monotonic() - began > 10:
                    raise RuntimeError("The copy server did not become ready.")
                time.sleep(.1)
        write_status(job_id, step="serving", rateBps=0, message="Sharing verified files; waiting for %d copies. %s" % (len(expected), firewall_hint(spec["port"])))
        last_stamp = None
        completed = set()
        failed = set()
        while proc.poll() is None:
            if spec.get("chain") and now() >= spec["chain"]["expiresAt"]:
                raise RuntimeError("The chain expired before all copies acknowledged verification.")
            for name in expected:
                marker = os.path.join(done_dir, name)
                try:
                    fd = os.open(marker, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(fd) as f:
                        st = os.fstat(f.fileno())
                        content = f.read(17)
                        if stat.S_ISREG(st.st_mode) and st.st_size <= 16 and content in ("done\n", "failed\n"):
                            completed.add(name)
                            if content == "failed\n": failed.add(name)
                except OSError:
                    pass
            if len(completed) == len(expected):
                if spec.get("chain") and failed:
                    raise RuntimeError("Some chain copies failed. Keep verified copies; review Jobs before recovery.")
                return {"copiesFinished": len(completed)}
            try:
                stamp = os.stat(log).st_mtime_ns
                if stamp != last_stamp:
                    last_stamp, last_activity = stamp, time.monotonic()
            except OSError:
                pass
            if time.monotonic() - last_activity >= min(max(spec.get("idleSeconds", 1800), 1), 1800):
                if spec.get("chain"):
                    raise RuntimeError("The chain file server expired while waiting for copy acknowledgments. Review each target in Jobs.")
                return {"idleExpired": True, "copiesFinished": len(completed)}
            time.sleep(.5)
        raise RuntimeError("The copy server stopped before its copies finished.")
    finally:
        stop_process(proc)
        shutil.rmtree(base, ignore_errors=True)


def job_pull_v2(job_id, spec):
    try:
        return _job_pull_v2(job_id, spec)
    except BaseException:
        # Admission/manifest failures happen before the ordinary copy's acknowledgment block.
        # Best effort only; an unreachable source still has its idle/deadline cleanup.
        if spec.get("chain"):
            try:
                from urllib.parse import urlsplit
                src = urlsplit(spec["src"])
                if src.scheme == "rsync" and src.username == "sparkbox" and src.hostname and src.path == "/model":
                    base = os.path.join(SECRETS, job_id)
                    os.makedirs(base, mode=0o700, exist_ok=True)
                    ack = os.path.join(base, "failed")
                    with open(ack, "w") as f: f.write("failed\n")
                    subprocess.run(["rsync", "--timeout=2", ack, "rsync://sparkbox@%s:%d/done/%s" % (src.hostname, src.port or 873, job_id)],
                                   env=dict(os.environ, RSYNC_PASSWORD=spec["password"]), capture_output=True, timeout=3)
            except Exception:
                pass
        raise
    finally:
        shutil.rmtree(os.path.join(SECRETS, job_id), ignore_errors=True)


def _job_pull_v2(job_id, spec):
    from urllib.parse import urlsplit
    import socket
    src = urlsplit(spec["src"])
    if src.scheme != "rsync" or src.username != "sparkbox" or not src.hostname or src.path != "/model" or src.query or src.fragment:
        raise RuntimeError("Invalid copy source.")
    if not shutil.which("rsync"):
        raise RuntimeError("The target needs rsync before it can copy this model.")
    base = os.path.join(SECRETS, job_id)
    os.makedirs(SECRETS, mode=0o700, exist_ok=True)
    os.chmod(SECRETS, 0o700)
    os.makedirs(base, mode=0o700, exist_ok=True)
    manifest_path = os.path.join(base, "manifest.json")
    env = dict(os.environ, RSYNC_PASSWORD=spec["password"])
    server = "rsync://sparkbox@%s:%d" % (src.hostname, src.port or 873)
    write_status(job_id, step="waiting", message="Waiting for the source's verified file list")
    # Hashing is node-side and can outlast an SSH session. Wait up to 30 minutes for readiness.
    deadline = time.monotonic() + 1800
    if spec.get("chain"):
        checked_chain(job_id, spec, "pull")
        deadline = time.monotonic() + max(0, spec["chain"]["expiresAt"] - now())
    while True:
        try:
            with socket.create_connection((src.hostname, src.port or 873), timeout=3):
                pass
            result = subprocess.run(["rsync", "--timeout=30", server + "/meta/manifest.json", manifest_path], env=env, capture_output=True, timeout=40)
            if result.returncode == 0:
                break
        except (OSError, subprocess.TimeoutExpired):
            pass
        if time.monotonic() >= deadline:
            if spec.get("chain"):
                raise RuntimeError("The chain expired while waiting for the verified source. Reconnect and review Jobs; no replacement was started.")
            raise RuntimeError("The source did not become ready in 30 minutes. Check its job, fabric address and firewall, then retry.")
        time.sleep(2)
    if os.path.getsize(manifest_path) > 16 * 1024**2:
        raise RuntimeError("The source manifest is too large.")
    with open(manifest_path) as f:
        manifest = json.load(f)
    files = chain_manifest(spec, manifest) if spec.get("chain") else validate_copy_manifest(manifest)
    dest = os.path.expanduser(spec["dest"])
    total = sum(f["size"] for f in files)
    if spec.get("imageTransfer"):
        info = manifest.get("image") or {}
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", info.get("id", "")) or info.get("id") != spec.get("imageId"):
            raise RuntimeError("The image manifest does not match the selected source image.")
        target_info = docker_info()
        if image_store(target_info) != info.get("store"):
            raise RuntimeError("Different Docker image stores; pull the same tag on each node instead.")
        if [f["path"] for f in files] != ["image.tar"]:
            raise RuntimeError("An image transfer must contain only image.tar.")
        size = info.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            raise RuntimeError("The image manifest has no valid image size.")
        copy_disk_check(dest, 2 * max(size, total))
        copy_disk_check(target_info["DockerRootDir"], 2 * max(size, total))
    def disk_check():
        if spec.get("chain"):
            if now() >= spec["chain"]["expiresAt"]: raise RuntimeError("The chain expired before copying.")
            chain_empty(dest)
        return require_destination_disk(dest, total, spec.get("node", "this node"))
    disk_check()
    claim_folder(dest, job_id, "pull", spec.get("instance", ""), disk_check=disk_check)
    try:
        os.unlink(os.path.join(dest, MANIFEST))
    except FileNotFoundError:
        pass
    ok = False
    try:
        exclude = list(spec.get("exclude") or []) + COPY_EXCLUDE_V2
        cmd = ["rsync", "-aL", "--partial", "--delete", "--timeout=120"] + ["--exclude=" + p for p in exclude] + [spec["src"].rstrip("/") + "/", dest + "/"]
        for attempt in range(4):
            write_status(job_id, step="copying", message="Copy attempt %d of 4" % (attempt + 1), bytesDone=folder_size(dest), bytesTotal=total)
            code, last = run_watched(job_id, cmd, env, dest, total)
            if code == 0:
                break
            if attempt == 3:
                raise RuntimeError("Copy failed after three retries (rsync exit %d): %s" % (code, last))
            time.sleep((1, 3, 10)[attempt])
        for f in files:
            if os.path.commonpath([os.path.realpath(dest), os.path.realpath(os.path.join(dest, f["path"]))]) != os.path.realpath(dest):
                raise RuntimeError("A copied file escapes the destination.")
        copied = served_files({"files": listing(dest), "exclude": exclude})
        if {f["path"] for f in copied} != {f["path"] for f in files}:
            raise RuntimeError("The copied file set changed after the source manifest was made; no verified record was written.")
        problems = verify(dest, files, job_id)
        if problems:
            raise RuntimeError("Copy checksum verification failed: " + "; ".join(problems[:5]))
        write_manifest(dest, manifest.get("repo", ""), manifest.get("revision", ""), files, checked="source-sha256")
        finish_partial(dest)
        ok = True
        return {"path": dest, "verified": "source-sha256"}
    finally:
        ack = os.path.join(base, "completion")
        with open(ack, "w") as f:
            f.write("done\n" if ok else "failed\n")
        try:
            subprocess.run(["rsync", "--timeout=10", ack, server + "/done/" + job_id], env=env, capture_output=True, timeout=15)
        except (OSError, subprocess.TimeoutExpired):
            pass  # The source's idle timeout remains the fallback.


def docker_info():
    result = subprocess.run(["docker", "info", "--format", "{{json .}}"], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError("Docker is unavailable; no image job started.")
    value = json.loads(result.stdout)
    if not value.get("DockerRootDir") or not value.get("Driver"):
        raise RuntimeError("Docker did not report its image store and disk location.")
    return value


def image_store(info):
    pairs = info.get("DriverStatus") or []
    return "containerd" if any(len(p) == 2 and p[0] == "driver-type" and str(p[1]).startswith("io.containerd.snapshotter") for p in pairs) else info.get("Driver")


def image_metadata(ref):
    if not isinstance(ref, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:/@+-]{0,254}", ref):
        raise RuntimeError("Invalid image reference.")
    result = subprocess.run(["docker", "image", "inspect", ref], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError("The selected image is not available on this node.")
    entry = json.loads(result.stdout)[0]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", entry.get("Id", "")) or entry.get("Architecture") != "arm64" or entry.get("Os") != "linux":
        raise RuntimeError("The selected image must be a local linux/arm64 image with a known identity.")
    return {"id": entry["Id"], "size": entry["Size"], "store": image_store(docker_info())}


def image_cache(job_id):
    if not valid_id(job_id):
        raise RuntimeError("Invalid image job id.")
    base = os.path.join(os.path.expanduser("~/.sparkbox/cache/images"), job_id)
    # Never reuse an earlier export or follow a cache symlink into another folder.
    if os.path.realpath(base) != os.path.abspath(base):
        raise RuntimeError("An image cache path is a symbolic link.")
    os.makedirs(base, mode=0o700, exist_ok=False)
    return base


def job_pull_image(job_id, spec):
    """Docker's local Engine API streams progress; the detached job owns its socket."""
    import http.client
    import socket
    from urllib.parse import urlencode
    ref = spec.get("image", "")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:/@+-]{0,254}", ref):
        raise RuntimeError("Invalid image reference.")
    size = spec.get("estimatedBytes", 0)
    if not isinstance(size, int) or isinstance(size, bool) or not 0 < size <= 1024**4:
        raise RuntimeError("Provide an estimated unpacked image size before pulling.")
    info = docker_info()
    copy_disk_check(info["DockerRootDir"], 2 * size)
    class LocalDocker(http.client.HTTPConnection):
        def connect(self):
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.settimeout(120)
            self.sock.connect("/var/run/docker.sock")
    conn = LocalDocker("localhost", timeout=120)
    try:
        conn.request("POST", "/images/create?" + urlencode({"fromImage": ref, "platform": "linux/arm64"}))
        response = conn.getresponse()
        if response.status != 200:
            raise RuntimeError("Docker refused the image pull (HTTP %d). Check registry access on the node." % response.status)
        layers = {}
        while True:
            line = response.readline(1024 * 1024 + 1)
            if not line:
                break
            if len(line) > 1024 * 1024:
                raise RuntimeError("Docker sent an oversized progress record.")
            item = json.loads(line)
            if item.get("error") or item.get("errorDetail"):
                raise RuntimeError("Docker could not pull the image; check the tag and registry access on the node.")
            p = item.get("progressDetail") or {}
            if item.get("id") and p.get("total"):
                layers[str(item["id"])[:80]] = (max(0, int(p.get("current", 0))), max(0, int(p["total"])))
            write_status(job_id, step="pulling image", bytesDone=sum(p[0] for p in layers.values()), bytesTotal=sum(p[1] for p in layers.values()))
        return image_metadata(ref)
    finally:
        conn.close()


def job_image_save(job_id, spec):
    meta = image_metadata(spec["image"])
    if meta["id"] != spec.get("imageId"):
        raise RuntimeError("The source image changed; check it again before distributing.")
    base = image_cache(job_id)
    try:
        copy_disk_check(base, 2 * meta["size"])
        archive = os.path.join(base, "image.tar")
        write_status(job_id, step="saving image")
        code, _ = run_watched(job_id, ["docker", "save", "--platform", "linux/arm64", "-o", archive, meta["id"]], os.environ.copy(), base, meta["size"])
        if code:
            raise RuntimeError("Docker could not export linux/arm64; no image was distributed.")
        return job_serve_v2(job_id, dict(spec, path=base, imageMetadata=meta, exclude=[]))
    finally:
        shutil.rmtree(base, ignore_errors=True)


def job_image_load(job_id, spec):
    ref = spec.get("image", "")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:/+-]{0,254}", ref) or ref.startswith("sha256:"):
        raise RuntimeError("Invalid target image tag.")
    base = image_cache(job_id)
    try:
        job_pull_v2(job_id, dict(spec, dest=base, imageTransfer=True, exclude=[]))
        archive = os.path.join(base, "image.tar")
        write_status(job_id, step="loading verified image")
        code, _ = run_watched(job_id, ["docker", "load", "-i", archive], os.environ.copy(), base, os.path.getsize(archive))
        if code:
            raise RuntimeError("Docker could not load the verified archive. It may have left unused layers.")
        meta = image_metadata(spec["imageId"])
        if meta["id"] != spec["imageId"]:
            raise RuntimeError("The loaded image's identity differs from the source.")
        tagged = subprocess.run(["docker", "tag", meta["id"], ref], capture_output=True, timeout=30)
        if tagged.returncode:
            raise RuntimeError("The image loaded by ID, but Docker could not set its requested tag.")
        return meta
    finally:
        shutil.rmtree(base, ignore_errors=True)


JOB_KINDS = {"download": job_download, "serve": job_serve, "pull": job_pull,
             "pull-image": job_pull_image, "image-save": job_image_save, "image-load": job_image_load}


class Stopped(BaseException):
    """SIGTERM reached the job: its finally blocks run (they must not be caught as a failure)."""


def on_sigterm(signum, frame):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)  # one Stopped, whoever else signals: cleanup runs
    raise Stopped()


def drop_secrets(job_id):
    """Removes a job's secret files, if any: the token's folder and the rsync server's."""
    if valid_id(job_id):
        for folder in (SECRETS, RSYNCD):
            shutil.rmtree(os.path.join(folder, job_id), ignore_errors=True)


def finish_job(job_id, **fields):
    """A job's last status, its secrets gone first: a job that reads as over has none left.
    cancel() writes its state before it signals, so a job it stopped keeps that state."""
    drop_secrets(job_id)
    if read_status(job_id).get("state") != "cancelled":
        write_status(job_id, **fields)


def download_plan(spec):
    """Only the reproducible, non-secret fields. Old/unpinned plans cannot be resumed."""
    repo, rev, image = (spec.get(k) for k in ("repo", "revision", "image"))
    dest = spec.get("dest")
    if not (isinstance(repo, str) and re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*/[A-Za-z0-9_-][A-Za-z0-9_.-]*", repo)
            and isinstance(rev, str) and re.fullmatch(r"[a-f0-9]{40}", rev)
            and isinstance(image, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/:@-]{0,255}", image)
            and isinstance(dest, str) and dest and "\0" not in dest):
        return None
    dest = os.path.expanduser(dest)
    if not os.path.isabs(dest) or os.path.normpath(dest) != dest or dest in ("/", HOME):
        return None
    # Current SparkBox downloads use an exact file list, with no extra patterns.
    if spec.get("include") or spec.get("exclude"):
        return None
    source = spec.get("files")
    if not isinstance(source, list) or not source or len(source) > 100000:
        return None
    files, names = [], set()
    for f in source:
        if not isinstance(f, dict):
            return None
        name, size, sha = file_rel(f), f.get("size"), f.get("sha256")
        if (not isinstance(name, str) or not name or name.startswith("/") or any(p in ("", ".", "..") for p in name.split("/"))
                or any(c in name for c in "\0\\*?[]") or name in names
                or (size is not None and (type(size) is not int or size < 0))
                or (sha is not None and (not isinstance(sha, str) or not re.fullmatch(r"[a-f0-9]{64}", sha)))):
            return None
        names.add(name)
        files.append({"path": name, "size": size, "sha256": sha})
    return {"repo": repo, "revision": rev, "dest": dest, "image": image, "files": files, "include": []}


def plan_hash(plan):
    return hashlib.sha256(json.dumps(plan, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def check_download_resume(spec):
    """Recheck under the folder lock, before stopping anything or claiming the folder."""
    previous = spec.get("resumeOf")
    if not isinstance(previous, str) or not valid_id(previous):
        raise RuntimeError("The saved download ID is invalid. Refresh Jobs.")
    old = read_status(previous)
    plan = download_plan(spec)
    saved = old.get("downloadPlan")
    if (not plan or plan != saved or download_plan(saved) != saved
            or old.get("planHash") != plan_hash(plan) or spec.get("resumeHash") != plan_hash(plan)):
        raise RuntimeError("The saved download plan changed or is incomplete. Refresh Jobs and review a new download.")
    if old.get("kind") != "download" or old.get("state") not in ("failed", "cancelled", "running") or job_running(old):
        raise RuntimeError("Only a stopped download can be resumed. Refresh Jobs.")
    if not instance_ok(spec.get("instance", "")) or old.get("instance") != spec["instance"]:
        raise RuntimeError("This download belongs to another SparkBox instance.")
    if old.get("dest") != plan["dest"] or marker_job(read_marker(plan["dest"])) != previous:
        raise RuntimeError("This folder no longer belongs to the stopped attempt. Refresh Jobs and the Library.")


def on_chain_expiry(_signum, _frame):
    raise RuntimeError("The chain reached its 24-hour deadline. Keep verified copies and review Jobs before recovery.")


def start(kind, job_id, spec):
    if kind not in JOB_KINDS or not valid_id(job_id):
        raise SystemExit("bad job")
    dest = {"dest": os.path.expanduser(spec["dest"])} if spec.get("dest") else {}
    recovery = {}
    if spec.get("chain"):
        recovery["chain"] = checked_chain(job_id, spec, kind)
        chain_empty(spec["dest"])
    if kind == "download":
        check_download_tool(spec)
        plan = download_plan(spec)
        if plan:
            recovery = {"downloadPlan": plan, "planHash": plan_hash(plan)}
        if spec.get("resumeOf"):
            check_download_resume(spec)
            recovery["resumeOf"] = spec["resumeOf"]
        recovery["diskCheck"] = require_destination_disk(spec["dest"], transfer_bytes(served_files(dict(spec, exclude=[]))), spec.get("node", "this node"))
    elif kind == "pull":
        recovery["diskCheck"] = require_destination_disk(spec["dest"], transfer_bytes(served_files(spec)), spec.get("node", "this node"))
    os.makedirs(JOBS, exist_ok=True)
    # New attempts never overwrite a retained record, including two simultaneous starts.
    with open(os.path.join(JOBS, ".start.lock"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if os.path.lexists(status_path(job_id)):
            raise RuntimeError("This job ID already exists. Start a new attempt.")
        st = write_status(job_id, kind=kind, state="running", step="starting", message="", bytesDone=0, bytesTotal=0,
                          rateBps=0, label=spec.get("label", ""), instance=spec.get("instance", ""), bootId=boot_id(), **dest, **recovery)
    pid = os.fork()
    if pid:
        write_status(job_id, pid=pid)
        st["pid"] = pid
        return st
    # Child: detach from SparkBox's SSH session, then run the job. Its pid is on record before it
    # makes any secret file, so `list` can't take one for a dead job's.
    signal.signal(signal.SIGTERM, on_sigterm)
    os.setsid()
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(devnull, fd)
    try:
        write_status(job_id, pid=os.getpid(), pidStart=start_of(os.getpid()))
        if spec.get("chain"):
            signal.signal(signal.SIGALRM, on_chain_expiry)
            signal.alarm(max(1, spec["chain"]["expiresAt"] - now()))
        result = JOB_KINDS[kind](job_id, spec)
        finish_job(job_id, state="done", step="done", message="", result=result)
    except Stopped:
        finish_job(job_id, state="failed", message="The job was stopped.")
    except Exception as e:  # report every failure in the status file
        finish_job(job_id, state="failed", message=disk_failure(spec, e)[:600])
    finally:
        if spec.get("chain"): signal.alarm(0)
        drop_secrets(job_id)  # a SIGTERM may have landed in finish_job
    os._exit(0)


def read_status(job_id):
    try:
        with open(status_path(job_id)) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (OSError, TypeError):
        return False


def pid_reused(pid, started):
    """Another process has this pid now: there is one, and its start time isn't the one on record.
    With no start time on record (a job an older sparkjob.py started) nothing can tell, so it isn't."""
    if started is None:
        return False
    now_there = start_of(pid)
    return now_there is not None and now_there != started


def job_running(st):
    """The status says running and its process is there, not a later one with its pid. start() writes
    the status a moment before the job has a pid, so a job that has none yet still counts for a few
    seconds."""
    if st.get("state") != "running" or different_boot(st):
        return False
    if st.get("pid") is None:
        return now() - st.get("startedAt", 0) < 30
    return alive(st.get("pid")) and not pid_reused(st.get("pid"), st.get("pidStart"))


def sweep_secrets():
    """Drops the secret files of every job that's over: SIGKILL and a reboot leave them behind,
    since no finally ran. `list` is a read, and this is the one thing a read removes, beside
    rewriting a dead job's status: only SparkBox's own files under ~/.sparkbox/job-secrets and
    ~/.sparkbox/rsyncd, only for jobs that are over. Each status is read here, after its folder
    was seen: a job writes its status before any secret, so a job that started after `list` read
    the jobs folder still counts as running."""
    for folder in (SECRETS, RSYNCD):
        try:
            names = os.listdir(folder)
        except OSError:
            continue
        for name in names:
            if not job_running(read_status(name)):
                drop_secrets(name)


ORPHAN_WORDS = {"pull": "Its copy is still running: a new copy stops it first.",
                "download": "Its container may still be running: Resume download stops it first."}


def with_orphan(st):
    """What `list` reports of a job that's over and left its child running: it says so, on top of
    how it ended. That's a fact of the moment, so it's never written to the status."""
    message = st.get("message", "")
    words = ORPHAN_WORDS.get(st.get("kind"), "")
    if st.get("kind") == "download" and not uses_download_container(st):
        words = "Its download tool is still running: Resume download stops it first."
    if "stops it first" not in message:
        message = message.rstrip(".") + ". " + words if message else words
    return dict(st, orphan=True, message=message)


def list_jobs(look=False):
    """Every job on this node. Otherwise it also writes a dead job's status and sweeps; `look`
    (`list --look`, what look-only sends) reports the same list and changes nothing. Neither stops
    an orphan: it only says so, and a new job on the folder is what stops it."""
    jobs = []
    if os.path.isdir(JOBS):
        for name in sorted(os.listdir(JOBS)):
            if name.endswith(".json"):
                st = read_status(name[:-5])
                if st.get("state") == "running" and not job_running(st):
                    why = "The job stopped unexpectedly (node restarted?)."
                    if st.get("kind") == "download" and uses_download_container(st):
                        why += " Its container may still be running: Resume download stops it first."
                    if look:
                        st = dict(st, state="failed", message=why)
                    else:
                        st = write_status(st["id"], state="failed", message=why)
                jobs.append(with_orphan(st) if orphan(st) else st)
    if not look:
        sweep_secrets()
    return jobs


CANCEL_WAIT_S = 3
CANCEL_LIMIT_S = 18  # SparkBox waits 20 s for cancel's answer


def group_gone(pid):
    try:
        os.killpg(pid, 0)
        return False
    except (OSError, TypeError):
        return True


def child_replaced(st):
    """The child on a job's record has gone, or its pid is another process's now."""
    if unverified_boot(st, child=True):
        return True
    if st.get("childStart") is None:
        return False
    facts = process_facts(st.get("childPid"))
    return not facts or facts["start"] != st["childStart"]


def cancel(job_id):
    began = time.time()
    st = read_status(job_id)
    write_status(job_id, state="cancelled", message="Canceled")
    if uses_download_container(st):
        stop_download(job_id, st.get("instance"))
    # A dead job isn't a replaced one: its group can outlive it (its orphan), and while it does that
    # pid can't be used again.
    replaced = {"childPid": child_replaced(st), "pid": unverified_boot(st) or pid_reused(st.get("pid"), st.get("pidStart"))}
    for key in ("childPid", "pid"):
        if replaced[key]:
            continue
        try:
            os.killpg(st.get(key), signal.SIGTERM)
        except (OSError, TypeError):
            try:
                os.kill(st.get(key), signal.SIGTERM)
            except (OSError, TypeError):
                pass
    # The job's own finally removes its secrets; this covers a job that had no time to, or had died.
    deadline = time.time() + CANCEL_WAIT_S
    while not replaced["pid"] and not group_gone(st.get("pid")) and time.time() < deadline:
        time.sleep(0.1)
    drop_secrets(job_id)
    if st.get("kind") == "download" and uses_download_container(st):
        # Only an empty list says it stopped, and the answer can't wait past SparkBox's limit.
        left = max(CANCEL_LIMIT_S - (time.time() - began), 0.1)
        if labelled_containers("sparkbox.deployment=" + job_id, timeout=min(left, DOCKER_PS_S)) != []:
            write_status(job_id, message="Canceled, but its container may still be running: Resume download stops it first.")
    return read_status(job_id)


# ---- inspection

def is_model_dir(path, names):
    return "config.json" in names and any(n.endswith(WEIGHT_EXT) for n in names)


def has_incomplete(path, strict=False):
    """hf's unfinished files are in the folder: <dest>/.cache/huggingface/download/*.incomplete."""
    try:
        return any(n.endswith(".incomplete") for n in os.listdir(os.path.join(path, ".cache", "huggingface", "download")))
    except FileNotFoundError:
        return False
    except OSError:
        if strict:
            raise
        return False


def holds_models(path):
    """A folder below path is a model or one left partway, so hf's unfinished files in path (a
    hand-run `hf download --local-dir` cut short) don't make path a folder of its own."""
    for dirpath, dirnames, names in os.walk(path, onerror=scan_error):
        dirnames[:] = [d for d in dirnames if d != ".cache"]
        if dirpath != path and ("config.json" in names or PARTIAL in names):
            return True
    return False


def partial_state(path, names, manifest):
    """Why a folder isn't finished: "downloading" or "copying" while the job in its marker runs,
    "stopped" when that job is over (or an older download left hf's *.incomplete files and no
    record), or None."""
    if PARTIAL in names:
        marker = read_marker(path)
        job = marker_job(marker)
        if job and job_running(read_status(job)):
            return "copying" if marker.get("kind") == "pull" else "downloading"
        return "stopped"
    return "stopped" if not manifest and has_incomplete(path) else None


def scan_progress(path, partial):
    """Only a live marker-bound job supplies progress. Never infer completion from bytes."""
    if partial not in ("downloading", "copying"):
        return None
    job = marker_job(read_marker(path))
    status = read_status(job) if job else {}
    if not job_running(status):
        return None
    step = status.get("step", "")
    phase = "verifying" if "verif" in step else "waiting" if "wait" in step else partial
    done, total = status.get("bytesDone", 0), status.get("bytesTotal", 0)
    if type(done) is not int or type(total) is not int or min(done, total) < 0:
        return None
    return {"step": phase, "bytesDone": done, "bytesTotal": total}


def repo_from_path(path, root):
    """Best guess at the Hugging Face repo for a folder: HF cache layout or <root>/<org>/<name>."""
    parts = path.split(os.sep)
    for p in parts:
        if p.startswith("models--"):
            return p[len("models--"):].replace("--", "/", 1)
    rel = os.path.relpath(path, root)
    return rel if rel.count("/") == 1 else os.path.basename(path)


def scan_model_size(path):
    """A dangling link in an HF snapshot is known incomplete data, not a failed node scan.
    Count present bytes only; never hide permission errors or vanished regular files.
    Ordinary folders and the search roots keep their strict failure behavior."""
    parent = os.path.dirname(path)
    cache_snapshot = (os.path.basename(parent) == "snapshots"
                      and os.path.basename(os.path.dirname(parent)).startswith("models--"))
    total, missing = 0, 0
    for dirpath, _, names in os.walk(path, onerror=scan_error):
        for name in names:
            full = os.path.join(dirpath, name)
            try:
                total += os.stat(full).st_size
            except FileNotFoundError:
                # lstat must still see a symlink; a concurrent removal isn't evidence
                # of a missing cache blob. Other errors deliberately propagate.
                if not cache_snapshot or not stat.S_ISLNK(os.lstat(full).st_mode):
                    raise
                missing += 1
    return total, missing


def scan(roots, max_depth=5):
    found, seen = [], set()
    for root in roots:
        if not root:
            raise ValueError("Model search root is empty. Set the model folder in Settings.")
        root = os.path.abspath(os.path.expanduser(root))
        try:
            mode = os.stat(root).st_mode
        except FileNotFoundError:
            if os.path.lexists(root):
                raise  # a broken root link is not a successfully scanned empty folder
            continue
        if not stat.S_ISDIR(mode):
            raise NotADirectoryError("Model search root is not a folder: " + root)
        base_depth = root.rstrip("/").count("/")
        for dirpath, dirnames, names in os.walk(root, followlinks=False, onerror=scan_error):
            if dirpath.count("/") - base_depth >= max_depth:
                dirnames[:] = []
            if os.path.basename(dirpath) in ("blobs", ".git", "__pycache__"):
                dirnames[:] = []
                continue
            # A folder partway has a marker, or only hf's unfinished files, before it has weights.
            if is_model_dir(dirpath, names) or PARTIAL in names or (has_incomplete(dirpath, strict=True) and not holds_models(dirpath)):
                real = os.path.realpath(dirpath)
                if real in seen:
                    continue
                seen.add(real)
                manifest = read_manifest(dirpath) or {}
                partial = partial_state(dirpath, names, manifest)
                size, missing = scan_model_size(dirpath)
                if missing:
                    # Keep an active writer's state so Delete/Copy cannot treat its
                    # incomplete snapshot as an idle target. Never promote its manifest.
                    partial = partial or "stopped"
                verified = bool(manifest.get("verifiedAt")) and partial is None
                found.append({
                    "path": dirpath,
                    "root": root,
                    "repo": manifest.get("repo") or repo_from_path(dirpath, root),
                    "revision": manifest.get("revision", ""),
                    "sizeBytes": size,
                    "missingCacheFiles": missing,
                    "verified": verified,
                    "partial": partial,
                    "progress": scan_progress(dirpath, partial),
                    "checked": (manifest.get("checked") or "sha256") if verified else "",
                    "format": "gguf" if any(n.endswith(".gguf") for n in names) else "safetensors",
                })
                dirnames[:] = []
    return found


def succeeds(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, timeout=CHECK_TIMEOUT_S).returncode == 0
    except (OSError, subprocess.TimeoutExpired):  # not installed, or a daemon that doesn't answer
        return False


def docker_ok():
    return succeeds(["docker", "info"])


def image_present(image):
    return succeeds(["docker", "image", "inspect", image])


def read_sys(path):
    """A sysfs file's text, or None when it can't be read (a link that's down has no speed)."""
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return None


def fabric():
    """ConnectX-7 interfaces with an IPv4 address: speed, MTU and state, and their RDMA device's port
    state as sysfs words it ("4: ACTIVE", "5: LinkUp"). What can't be read is null, never a guess."""
    try:
        addrs = json.loads(subprocess.run(["ip", "-j", "-4", "addr"], capture_output=True, text=True).stdout or "[]")
    except (OSError, ValueError):
        addrs = []
    out_ = []
    for a in addrs:
        name = a.get("ifname", "")
        base = os.path.join(SYS, "class", "net", name)
        driver = os.path.basename(os.path.realpath(base + "/device/driver"))
        if not driver.startswith("mlx5") or not a.get("addr_info"):
            continue
        rdma = sorted(os.listdir(base + "/device/infiniband")) if os.path.isdir(base + "/device/infiniband") else []
        port = os.path.join(SYS, "class", "infiniband", rdma[0], "ports", "1") if rdma else None
        try:
            speed = int(read_sys(base + "/speed"))
        except (TypeError, ValueError):
            speed = 0
        mtu = read_sys(base + "/mtu")
        out_.append({"iface": name, "ip": a["addr_info"][0]["local"], "prefix": a["addr_info"][0].get("prefixlen", 24),
                     "speedMbps": speed, "rdma": rdma[0] if rdma else "", "mtu": int(mtu) if mtu and mtu.isdigit() else None,
                     "operstate": read_sys(base + "/operstate"),
                     "rdmaState": read_sys(port + "/state") if port else None, "rdmaPhys": read_sys(port + "/phys_state") if port else None})
    return out_


CHECK_TIMEOUT_S = 120
GPU_CHECK_S = 60


def instance_ok(instance):
    """A --instance value: release, dev (a short lowercase name, never an option or a path)."""
    return (0 < len(instance) <= 32 and instance.isascii() and instance[0].islower()
            and all(c.islower() or c.isdigit() or c == "-" for c in instance))


def in_test_container(img, instance, entrypoint, *args, gpus=False, limit=None):
    """Runs a short command in `img`, in a test container of SparkBox's: named
    sparkbox-checkimage-<rand>, labelled sparkbox.test=1 and sparkbox.instance=<instance> (an
    older SparkBox names no instance), removed when it's done, also when it runs past the time
    limit. What it did, or None past the limit or with no docker."""
    name = "sparkbox-checkimage-" + os.urandom(4).hex()
    labels = ["--label", "sparkbox.test=1"] + (["--label", "sparkbox.instance=" + instance] if instance else [])
    cmd = ["docker", "run", "--rm"] + (["--gpus", "all"] if gpus else []) + ["--name", name] + labels + ["--entrypoint", entrypoint, img] + list(args)
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=limit or CHECK_TIMEOUT_S)
    except OSError:
        return None
    except subprocess.TimeoutExpired:
        try:
            subprocess.run(["docker", "rm", "-f", name], capture_output=True, timeout=DOCKER_STOP_S)
        except (OSError, subprocess.TimeoutExpired):
            pass
        return None


def vllm_version_in(img, instance):
    """vLLM's version inside `img`, read in a test container. "" when it can't be read."""
    r = in_test_container(img, instance, "python3", "-c", "import vllm; print(vllm.__version__)")
    return r.stdout.strip().splitlines()[-1] if r and r.returncode == 0 and r.stdout.strip() else ""


def gpu_in_container(img, instance):
    """The GPU as a container sees it: nvidia-smi's first line, "" when it doesn't see one, or None
    when the probe failed or ran past its limit: the GPU's presence wasn't established."""
    r = in_test_container(img, instance, "nvidia-smi", "-L", gpus=True, limit=GPU_CHECK_S)
    if r is None or r.returncode != 0:
        return None
    lines = r.stdout.strip().splitlines()
    if not lines or lines[0].strip().lower() == "no devices were found":
        return ""
    return lines[0]


def version_read(argv):
    """A bounded read: absent tools, failures and timeouts are unknown."""
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=5)
        return r.stdout.strip() if r.returncode == 0 else ""
    except (OSError, subprocess.TimeoutExpired):
        return ""


def version_facts(image):
    """Fresh host facts and selected-image declarations; never starts a container.
    Only whitelisted fields leave this function, never an image's full environment."""
    dgx = read_sys("/etc/dgx-release") or ""
    ota = None
    for line in dgx.splitlines():
        key, _, value = line.partition("=")
        if key.strip() == "DGX_OTA_VERSION":
            ota = value.strip().strip('"') or None
    smi = version_read(["nvidia-smi"])
    driver = re.search(r"Driver Version:\s*([\d.]+)", smi)
    cuda = re.search(r"CUDA Version:\s*([\d.]+)", smi)
    toolkit = version_read(["dpkg-query", "-W", "-f=${Status}|${Version}", "nvidia-container-toolkit"])
    toolkit = toolkit.removeprefix("install ok installed|") if toolkit.startswith("install ok installed|") else None
    try:
        with open(os.path.join(os.path.dirname(__file__), "collector.py"), "rb") as source:
            collector = hashlib.sha256(source.read()).hexdigest()[:12]
    except OSError:
        collector = None
    image_id, nccl = None, None
    try:
        data = json.loads(version_read(["docker", "image", "inspect", image]))
        entry = data[0]
        if re.fullmatch(r"sha256:[0-9a-f]{64}", entry.get("Id") or ""):
            image_id = entry["Id"]
        declared = [v.split("=", 1)[1] for v in (entry.get("Config") or {}).get("Env") or [] if isinstance(v, str) and v.startswith("NCCL_VERSION=")]
        if len(set(declared)) == 1 and re.fullmatch(r"[0-9][A-Za-z0-9.+_-]{0,63}", declared[0]):
            nccl = declared[0]
    except (ValueError, KeyError, IndexError, TypeError, AttributeError):
        pass
    return {"dgxOta": ota, "kernel": os.uname().release,
            "driver": driver.group(1) if driver else None, "cuda": cuda.group(1) if cuda else None,
            "toolkit": toolkit, "collector": collector, "imageId": image_id, "nccl": nccl}


def parser_choices(help_text, flag):
    """Read only an argparse choice list for one flag; never return arbitrary help text."""
    match = re.search(r"(?m)^\s*" + re.escape(flag) + r"(?:[ =]|\n\s*)+\{([^}]{1,16000})\}", help_text)
    if not match:
        return None
    names = [v.strip() for v in match.group(1).replace("\n", " ").split(",")]
    if not names or len(names) > 256 or any(not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,79}", n) for n in names):
        return None
    return sorted(set(names))


def image_parsers(image_id, instance):
    result = in_test_container(image_id, instance, "vllm", "serve", "--help")
    if result is None or result.returncode:
        return None
    return {"imageId": image_id, "tools": parser_choices(result.stdout, "--tool-call-parser"),
            "reasoning": parser_choices(result.stdout, "--reasoning-parser")}


def check(root, image, ray_image="", quick=False, instance=""):
    root = os.path.expanduser(root)
    probe = root if os.path.isdir(root) else os.path.dirname(root.rstrip("/")) or HOME
    st = os.statvfs(probe)
    vllm_version, gpu, parsers = "", None, None
    versions = version_facts(image)
    tested = versions.get("imageId")
    # The legacy ray_image operand is accepted for old clients, never executed.
    if not quick and tested:
        vllm_version, gpu = vllm_version_in(tested, instance), gpu_in_container(tested, instance)
        parsers = image_parsers(tested, instance)
    return {
        "home": HOME,
        "rootPath": root,
        "rootExists": os.path.isdir(root),
        "freeBytes": st.f_bavail * st.f_frsize,
        "docker": docker_ok(),
        "rsync": shutil.which("rsync") is not None,
        "python": sys.version.split()[0],
        "imagePresent": image_present(image),
        "imageRef": image,
        "parsers": parsers,
        "vllmVersion": vllm_version,
        "gpuInContainer": gpu,
        "fabric": fabric(),
        "versions": versions,
    }


def check_args(args):
    """`<root> <image> <ray image> [quick] [--instance <i>]` as check's arguments."""
    # Accept the retired third operand from older clients, but never inspect or execute it.
    count = 3 if len(args) > 2 and args[2] not in ("quick", "--instance") else 2
    head, rest = args[:count], args[count:]
    quick = rest[:1] == ["quick"]
    rest = rest[1:] if quick else rest
    instance = ""
    if rest[:1] == ["--instance"] and len(rest) == 2 and instance_ok(rest[1]):
        instance, rest = rest[1], []
    if len(head) < 2 or rest:
        raise SystemExit("usage: check <root> <image> <ray image> [quick] [--instance <release|dev>]")
    return head[0], head[1], head[2] if len(head) == 3 else "", quick, instance


def delete(root, path):
    root = confined(root, os.path.realpath(os.path.expanduser(root)))
    real = confined(path, os.path.realpath(os.path.expanduser(path)))
    folder = (os.path.isfile(os.path.join(real, "config.json")) or os.path.isfile(os.path.join(real, PARTIAL))
              or (has_incomplete(real) and not holds_models(real)))
    if not real.startswith(root + os.sep) or not folder:
        raise SystemExit("refusing to delete %s: not a model folder inside %s" % (path, root))
    if partial_state(real, os.listdir(real), read_manifest(real)) in ("downloading", "copying"):
        raise SystemExit("refusing to delete %s: a download or copy is still writing to it. Follow it in Jobs." % path)
    if writers(real, marker_job(read_marker(real))):
        raise SystemExit(str(copy_refused(path)))
    shutil.rmtree(real)
    return {"deleted": real}


# ---- Files window

TEXT_LIMIT = 2 * 1024 * 1024
PEEK = 64 * 1024
BACKUPS_KEPT = 5

# Never removed or trashed: the system's own folders, and anything inside the system trees
# (except /usr/local). The home folder and its ancestors are refused separately.
PROTECTED = {"/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root", "/run",
             "/sbin", "/snap", "/srv", "/sys", "/tmp", "/usr", "/var", "/mnt", "/media", "/lost+found",
             "/private/etc", "/private/var", "/private/tmp"}
SYSTEM_TREES = ("/bin", "/boot", "/dev", "/lib", "/lib64", "/proc", "/sbin", "/sys", "/usr", "/snap")
SYSTEM_PARENTS = ("/usr", "/var", "/opt", "/private", "/private/var")


# ~/sparkbox-test is the folder the write fence's files-test class covers. The fence decides on
# the path as SparkBox writes it, so every write verb here holds a path written under it to the
# folder's real path once links are followed: a link inside pointing at ~/models, or the folder
# itself a link, is refused instead of followed.
TEST_DIR = "sparkbox-test"


def written_in_test_dir(path):
    """The path as written is ~/sparkbox-test or under it (as ~/…, $HOME/… or the home's real path)."""
    p = os.path.normpath(os.path.expanduser(path))
    for home in {HOME.rstrip("/") or "/", os.path.realpath(HOME)}:
        root = os.path.join(home, TEST_DIR)
        if p == root or p.startswith(root + os.sep):
            return True
    return False


def confined(path, real):
    """`real` (what `path` resolves to), once checked that a path written under ~/sparkbox-test
    stays inside it; any other path is returned as it is."""
    if not written_in_test_dir(path):
        return real
    root = os.path.join(os.path.realpath(HOME), TEST_DIR)
    if os.path.islink(root):
        raise SystemExit("SparkBox won't change %s: ~/%s is a link." % (path, TEST_DIR))
    if real != root and not real.startswith(root + os.sep):
        raise SystemExit("SparkBox won't change %s: it leads outside ~/%s, to %s." % (path, TEST_DIR, real))
    return real


def plain_name(name):
    """An item name from a spec on stdin: one path component, never a way out of its folder."""
    if not isinstance(name, str) or name in ("", ".", "..") or "/" in name or "\0" in name:
        raise SystemExit("bad item name %r" % (name,))
    return name


def resolve_item(path):
    """The real path of the item itself, like `realpath -m` on its folder: //etc, /etc/. and
    /usr/../etc all become /etc, but a symlink at the end stays the link (we act on the link)."""
    p = os.path.expanduser(path)
    if not p.startswith("/"):
        raise SystemExit("%s isn't an absolute path" % path)
    p = p.rstrip("/") or "/"
    head, tail = os.path.split(p)
    if tail in ("", ".", ".."):
        return os.path.realpath(p)
    return os.path.join(os.path.realpath(head or "/"), tail)


def protected(real):
    home = os.path.realpath(HOME)
    if real in PROTECTED or os.path.dirname(real) == "/":
        return True
    if real == home or home.startswith(real.rstrip("/") + "/"):
        return True
    if os.path.dirname(real) in SYSTEM_PARENTS:
        return True
    if real.startswith("/usr/local/"):
        return False
    return any(real.startswith(t + "/") for t in SYSTEM_TREES)


def removable(path):
    real = confined(path, resolve_item(path))
    if protected(real):
        raise SystemExit("SparkBox won't delete %s." % real)
    if not os.path.lexists(real):
        raise SystemExit("%s doesn't exist." % real)
    return real


def owner_names(st):
    try:
        import pwd
        user = pwd.getpwuid(st.st_uid).pw_name
    except (ImportError, KeyError):
        user = str(st.st_uid)
    try:
        import grp
        group = grp.getgrgid(st.st_gid).gr_name
    except (ImportError, KeyError):
        group = str(st.st_gid)
    return user, group


def disk_space(path):
    while path and not os.path.isdir(path):
        path = os.path.dirname(path)
    try:
        st = os.statvfs(path or "/")
        return st.f_bavail * st.f_frsize, st.f_blocks * st.f_frsize
    except OSError:
        return None, None


def ls(path):
    path = os.path.realpath(os.path.expanduser(path or "~"))
    entries = []
    try:
        it = os.scandir(path)
    except OSError as e:
        raise SystemExit("can't open %s: %s" % (path, e.strerror))
    names = {}
    with it:
        for e in it:
            try:
                st = e.stat(follow_symlinks=False)
            except OSError:
                continue
            kind = "link" if stat.S_ISLNK(st.st_mode) else "dir" if stat.S_ISDIR(st.st_mode) else "file"
            if st.st_uid not in names:
                names[st.st_uid] = owner_names(st)[0]
            entry = {"name": e.name, "kind": kind, "size": st.st_size, "mtime": int(st.st_mtime),
                     "mode": stat.filemode(st.st_mode), "target": None, "linkDir": False, "owner": names[st.st_uid]}
            if kind == "link":
                try:
                    entry["target"] = os.readlink(e.path)
                    entry["linkDir"] = os.path.isdir(e.path)
                except OSError:
                    pass
            entries.append(entry)
    free, total = disk_space(path)
    return {"path": path, "entries": entries, "free": free, "capacity": total}


def facts(real, st):
    user, group = owner_names(st)
    return {"path": real, "size": st.st_size, "mtime": int(st.st_mtime), "mode": stat.filemode(st.st_mode),
            "owner": "%s:%s" % (user, group), "uid": st.st_uid, "gid": st.st_gid,
            "writable": os.access(real, os.W_OK) and os.access(os.path.dirname(real), os.W_OK)}


def read_text(path):
    """A text file for the editor, with what a save needs to detect changes made meanwhile.
    Files over 2 MB come back as their first and last 64 KB, for a read-only viewer."""
    real = os.path.realpath(os.path.expanduser(path))
    try:
        st = os.stat(real)
    except OSError as e:
        raise SystemExit("can't read %s: %s" % (real, e.strerror))
    if stat.S_ISDIR(st.st_mode):
        raise SystemExit("%s is a folder" % real)
    info = facts(real, st)
    try:
        f = open(real, "rb")
    except PermissionError:
        # Only root (or the owner) can read it: say so with the facts, so the editor can explain
        # it and offer to read it with sudo instead of showing an error.
        return {"kind": "permission", "path": real, "mode": "%o" % stat.S_IMODE(st.st_mode), "owner": owner_names(st)[0]}
    with f:
        if st.st_size > TEXT_LIMIT:
            head = f.read(PEEK)
            f.seek(max(st.st_size - PEEK, PEEK))
            tail = f.read(PEEK)
            if b"\0" in head:
                raise SystemExit("%s looks like a binary file" % real)
            return dict(info, content="", sha256="", large=True,
                        head=head.decode("utf-8", "replace"), tail=tail.decode("utf-8", "replace"))
        data = f.read()
    if b"\0" in data:
        raise SystemExit("%s looks like a binary file" % real)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        raise SystemExit("%s isn't UTF-8 text" % real)
    return dict(info, content=text, sha256=hashlib.sha256(data).hexdigest(), large=False)


def needs_visudo(real):
    return real == "/etc/sudoers" or real.startswith("/etc/sudoers.d/")


def backup_dir(backups, real):
    return os.path.join(backups, hashlib.sha1(real.encode()).hexdigest()[:16])


def list_backups(backups, path):
    real = os.path.realpath(os.path.expanduser(path))
    d = backup_dir(os.path.expanduser(backups), real)
    name = os.path.basename(real)
    found = []
    if os.path.isdir(d):
        for f in sorted(os.listdir(d), reverse=True):
            if f.startswith(name + ".") and f != "path":
                st = os.stat(os.path.join(d, f))
                found.append({"path": os.path.join(d, f), "mtime": int(st.st_mtime), "size": st.st_size})
    return found


def keep_backup(backups, real, owner):
    """Copies the current file into backups/<path hash>/<name>.<time>, keeping the newest five."""
    d = backup_dir(os.path.expanduser(backups), real)
    os.makedirs(d, mode=0o700, exist_ok=True)
    with open(os.path.join(d, "path"), "w") as f:
        f.write(real + "\n")
    name = os.path.basename(real)
    dest = os.path.join(d, "%s.%s" % (name, time.strftime("%Y%m%d-%H%M%S") + "-%03d" % (int(time.time() * 1000) % 1000)))
    shutil.copyfile(real, dest)
    os.chmod(dest, 0o600)
    if owner:
        uid, gid = (int(x) for x in owner.split(":"))
        for p in (d, dest, os.path.join(d, "path")):
            os.chown(p, uid, gid)
    old = sorted(f for f in os.listdir(d) if f.startswith(name + ".") and f != "path")
    for f in old[:-BACKUPS_KEPT]:
        os.unlink(os.path.join(d, f))
    return dest


class NoFolder(Exception):
    """A new file's folder isn't there (its path as given): `main` reports it as {"error": …}."""


def commit(target, new_sha, expect, src=None, backups=None, owner=None, nonce=None):
    """Saves a file without ever leaving it half-written. The new text is checked against its
    SHA-256 (a dropped connection can't save a truncated file), written to .<name>.sparkbox-<nonce>
    next to the target, given the target's mode and owner (chmod/chown --reference), checked with
    visudo for sudoers files, and renamed over the target. A target changed since it was read
    (expect differs) is reported as a conflict and left alone. expect "absent" makes a new file:
    a conflict if anything is at the path already (a link too, even one that leads nowhere), and
    NoFolder when the folder it would go in doesn't exist."""
    real = confined(target, os.path.realpath(os.path.expanduser(target)))
    if backups:
        confined(backups, os.path.realpath(os.path.expanduser(backups)))
    folder, name = os.path.split(real)
    exists = os.path.lexists(real)
    absent = expect == "absent"
    if absent and not os.path.isdir(folder):
        raise NoFolder(os.path.dirname(target.rstrip("/")) or folder)
    if absent:
        here = os.path.expanduser(target)
        found = here if os.path.lexists(here) else real if exists else None
        if found:
            st = os.lstat(found)
            return {"conflict": True, "path": real, "sha256": None, "mtime": int(st.st_mtime), "size": st.st_size}
    elif expect not in (None, "", "-"):
        current = sha256_file(real) if exists else None
        if current != expect:
            st = os.stat(real) if exists else None
            return {"conflict": True, "path": real, "sha256": current,
                    "mtime": int(st.st_mtime) if st else None, "size": st.st_size if st else None}
    if src:
        with open(os.path.expanduser(src), "rb") as f:
            data = f.read()
    else:
        data = sys.stdin.buffer.read()
    if hashlib.sha256(data).hexdigest() != new_sha:
        raise SystemExit("The new text arrived incomplete (%d bytes), so %s wasn't changed." % (len(data), real))
    # Leftovers from a save that was killed half-way.
    for f in os.listdir(folder):
        if f.startswith("." + name + ".sparkbox-"):
            try:
                if time.time() - os.lstat(os.path.join(folder, f)).st_mtime > 600:
                    os.unlink(os.path.join(folder, f))
            except OSError:
                pass
    tmp = os.path.join(folder, ".%s.sparkbox-%s" % (name, nonce or os.urandom(4).hex()))
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        if exists:
            st = os.stat(real)
            os.chmod(tmp, stat.S_IMODE(st.st_mode))
            if (st.st_uid, st.st_gid) != (os.getuid(), os.getgid()):
                try:
                    os.chown(tmp, st.st_uid, st.st_gid)
                except PermissionError:
                    raise SystemExit("%s belongs to someone else; save it with sudo to keep its owner." % real)
        else:
            mask = os.umask(0)
            os.umask(mask)
            os.chmod(tmp, 0o666 & ~mask)
        if needs_visudo(real):
            check = subprocess.run(["visudo", "-cf", tmp], capture_output=True, text=True)
            if check.returncode != 0:
                raise SystemExit("visudo rejected the new %s, so it wasn't saved: %s"
                                 % (real, (check.stderr or check.stdout).strip()[-400:]))
        if os.environ.get("SPARKJOB_FAULT") == "before-replace":
            os._exit(137)  # tests: the process dies here, as if the connection dropped
        backup = keep_backup(backups, real, owner) if exists and backups else None
        if absent:
            # Linked into place, never over something that appeared meanwhile.
            try:
                os.link(tmp, real)
            except FileExistsError:
                os.unlink(tmp)
                st = os.lstat(real)
                return {"conflict": True, "path": real, "sha256": None, "mtime": int(st.st_mtime), "size": st.st_size}
            os.unlink(tmp)
        else:
            os.replace(tmp, real)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    try:
        dfd = os.open(folder, os.O_RDONLY)
        os.fsync(dfd)
        os.close(dfd)
    except OSError:
        pass
    st = os.stat(real)
    return {"conflict": False, "path": real, "sha256": new_sha, "mtime": int(st.st_mtime), "size": st.st_size,
            "backup": backup}


# Trash: the freedesktop.org layout (~/.local/share/Trash/files + info/<name>.trashinfo), so
# desktop file managers on the node see the same Trash. An item on another filesystem goes into
# .sparkbox-trash next to it instead, because moving it home would mean copying it.

# An item from ~/sparkbox-test goes to ~/sparkbox-test/.sparkbox-trash, so a files-test fence's
# Trash and Undo stay inside the folder (the home Trash would put things back anywhere).

def test_trash(real):
    """~/sparkbox-test/.sparkbox-trash for an item inside the folder (not the folder itself, nor
    what's already in that Trash), else None."""
    root = os.path.join(os.path.realpath(HOME), TEST_DIR)
    trash = os.path.join(root, ".sparkbox-trash")
    inside = real.startswith(root + os.sep) and real != trash and not real.startswith(trash + os.sep)
    return trash if inside and not os.path.islink(root) else None


def trash_base(real):
    in_test = test_trash(real)
    if in_test:
        return in_test
    home_trash = os.path.join(HOME, ".local", "share", "Trash")
    try:
        same = os.lstat(os.path.dirname(real)).st_dev == os.stat(HOME).st_dev
    except OSError:
        same = False
    return home_trash if same else os.path.join(os.path.dirname(real), ".sparkbox-trash")


def trash_one(path):
    from urllib.parse import quote
    real = removable(path)
    if written_in_test_dir(path) and not test_trash(real):
        # ~/sparkbox-test's own Trash is where its items go: the folder itself, and what's already
        # in that Trash, have nowhere inside the folder to go (the home Trash would put them back
        # anywhere), so they aren't moved.
        root = os.path.join(os.path.realpath(HOME), TEST_DIR)
        if real == root:
            raise SystemExit("SparkBox won't put ~/%s itself in the Trash; delete what's in it instead." % TEST_DIR)
        raise SystemExit("%s is already in the Trash; Delete removes it for good." % real)
    base = trash_base(real)
    files, info = os.path.join(base, "files"), os.path.join(base, "info")
    for d in (files, info):
        os.makedirs(d, mode=0o700, exist_ok=True)
    name = os.path.basename(real)
    stem, ext = os.path.splitext(name) if not os.path.isdir(real) else (name, "")
    n = 1
    while True:
        candidate = name if n == 1 else "%s %d%s" % (stem, n, ext)
        info_path = os.path.join(info, candidate + ".trashinfo")
        try:
            fd = os.open(info_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            n += 1
            continue
        if os.path.lexists(os.path.join(files, candidate)):
            os.close(fd)
            os.unlink(info_path)
            n += 1
            continue
        break
    with os.fdopen(fd, "w") as f:
        f.write("[Trash Info]\nPath=%s\nDeletionDate=%s\n" % (quote(real), time.strftime("%Y-%m-%dT%H:%M:%S")))
    dest = os.path.join(files, candidate)
    try:
        os.rename(real, dest)
    except OSError as e:
        os.unlink(info_path)
        raise SystemExit("can't move %s to the Trash: %s" % (real, e.strerror))
    return {"path": real, "trashed": dest}


def untrash_one(trashed):
    """Puts a trashed item back where its trashinfo says it came from. One written under
    ~/sparkbox-test (its .sparkbox-trash) goes back only inside the folder: what files-test
    admits stays there, whatever the trashinfo was made to say."""
    from urllib.parse import unquote
    written = trashed
    # Resolve the item itself, not what it points at: a trashed symlink is put back as the link
    # (realpath would follow it out of the folder and confine would then refuse its own Trash).
    trashed = confined(written, resolve_item(written))
    files = os.path.dirname(trashed)
    base = os.path.dirname(files)
    if os.path.basename(files) != "files" or os.path.basename(base) not in ("Trash", ".sparkbox-trash"):
        raise SystemExit("%s isn't in a Trash folder." % trashed)
    info_path = os.path.join(base, "info", os.path.basename(trashed) + ".trashinfo")
    original = None
    try:
        with open(info_path) as f:
            for line in f:
                if line.startswith("Path="):
                    original = unquote(line[5:].strip())
    except OSError:
        pass
    if not original:
        raise SystemExit("SparkBox can't tell where %s came from." % trashed)
    if written_in_test_dir(written):
        if not written_in_test_dir(original):
            raise SystemExit("SparkBox won't put %s back at %s: it's outside ~/%s." % (trashed, original, TEST_DIR))
        original = confined(original, resolve_item(original))
    if os.path.lexists(original):
        raise SystemExit("%s already exists, so the trashed copy was left in the Trash." % original)
    os.makedirs(os.path.dirname(original), exist_ok=True)
    os.rename(trashed, original)
    os.unlink(info_path)
    return {"path": original, "trashed": trashed}


def total_size(paths):
    total = 0
    for p in paths:
        p = os.path.expanduser(p)
        if os.path.isdir(p) and not os.path.islink(p):
            total += folder_size(p)
        else:
            try:
                total += os.lstat(p).st_size
            except OSError:
                pass
    return total


def remove(path):
    real = removable(path)
    if os.path.isdir(real) and not os.path.islink(real):
        shutil.rmtree(real)
    else:
        os.unlink(real)
    return {"path": real}


def summary(folder, names):
    """What copying `names` out of (or into) `folder` involves: per item its kind, bytes, entries,
    newest mtime and the size of its tar stream; plus the free space there."""
    folder = os.path.realpath(os.path.expanduser(folder))
    items = []
    for name in names:
        p = os.path.join(folder, name)
        if not os.path.lexists(p):
            items.append({"name": name, "exists": False})
            continue
        st = os.lstat(p)
        kind = "link" if stat.S_ISLNK(st.st_mode) else "dir" if stat.S_ISDIR(st.st_mode) else "file"
        size, entries, tar = 0, 0, 0

        def add(rel, s):
            nonlocal size, entries, tar
            entries += 1
            tar += 512 + (1024 if len(rel) > 99 else 0)
            if stat.S_ISREG(s.st_mode):
                size += s.st_size
                tar += -(-s.st_size // 512) * 512

        add(name, st)
        if kind == "dir":
            for dirpath, dirnames, filenames in os.walk(p):
                for n in dirnames + filenames:
                    full = os.path.join(dirpath, n)
                    try:
                        add(os.path.relpath(full, folder), os.lstat(full))
                    except OSError:
                        pass
        items.append({"name": name, "exists": True, "kind": kind, "bytes": size, "entries": entries,
                      "mtime": int(st.st_mtime), "tarBytes": tar})
    free, total = disk_space(folder)
    return {"path": folder, "items": items, "free": free, "capacity": total}


def free_name(folder, name, is_dir):
    stem, ext = (name, "") if is_dir else os.path.splitext(name)
    n = 2
    while os.path.lexists(os.path.join(folder, "%s %d%s" % (stem, n, ext))):
        n += 1
    return "%s %d%s" % (stem, n, ext)


def _move(src, dst):
    try:
        os.rename(src, dst)
    except OSError:
        shutil.move(src, dst)  # another filesystem: copy, then remove


def _merge(src, dst, newer_only):
    """Moves src's contents into dst. Files with the same name are replaced (or, with newer_only,
    only when src's copy is newer); folders merge."""
    src_dir = os.path.isdir(src) and not os.path.islink(src)
    dst_dir = os.path.isdir(dst) and not os.path.islink(dst)
    if src_dir and dst_dir:
        for n in os.listdir(src):
            _merge(os.path.join(src, n), os.path.join(dst, n), newer_only)
        return
    if os.path.lexists(dst):
        if newer_only and os.lstat(src).st_mtime <= os.lstat(dst).st_mtime:
            return
        if dst_dir:
            shutil.rmtree(dst)
        elif src_dir:
            os.unlink(dst)
    if src_dir or not os.path.lexists(dst):
        _move(src, dst)
    else:
        try:
            os.replace(src, dst)
        except OSError:
            shutil.copy2(src, dst, follow_symlinks=False)


def place(source, dest, items, nonce, move=False):
    """Puts copied (or, with move, original) items from `source` into `dest`, each by its policy:
    replace (the old item is swapped out whole), merge, skip, keepBoth ("name 2") or
    replaceIfNewer. An item with no conflict when it was checked but taken meanwhile is kept
    as both, never overwritten. Unused staged leftovers go when the staging folder is removed."""
    source = confined(source, os.path.realpath(os.path.expanduser(source)))
    dest = confined(dest, os.path.realpath(os.path.expanduser(dest)))
    if not valid_id(nonce):
        raise SystemExit("bad staging id")
    names = [plain_name(item["name"]) for item in items]
    os.makedirs(dest, exist_ok=True)
    results = []
    for item, name in zip(items, names):
        policy = item.get("policy") or "new"
        src, dst = os.path.join(source, name), os.path.join(dest, name)
        if not os.path.lexists(src) or policy == "skip":
            results.append({"name": name, "action": "skipped", "as": name})
            continue
        if src == dst:
            results.append({"name": name, "action": "skipped", "as": name})
            continue
        exists = os.path.lexists(dst)
        if policy == "new" and exists:
            policy = "keepBoth"
        if not exists:
            _move(src, dst)
            results.append({"name": name, "action": "copied", "as": name})
        elif policy == "keepBoth":
            other = free_name(dest, name, os.path.isdir(src))
            _move(src, os.path.join(dest, other))
            results.append({"name": name, "action": "keptBoth", "as": other})
        elif policy == "replace":
            old = os.path.join(dest, ".%s.sparkbox-old-%s" % (name, nonce))
            os.rename(dst, old)
            try:
                _move(src, dst)
            except BaseException:
                os.rename(old, dst)
                raise
            if os.path.isdir(old) and not os.path.islink(old):
                shutil.rmtree(old, ignore_errors=True)
            else:
                os.unlink(old)
            results.append({"name": name, "action": "replaced", "as": name})
        elif policy in ("merge", "replaceIfNewer"):
            _merge(src, dst, policy == "replaceIfNewer")
            if move and os.path.lexists(src):
                if os.path.isdir(src) and not os.path.islink(src):
                    shutil.rmtree(src, ignore_errors=True)
                else:
                    os.unlink(src)
            results.append({"name": name, "action": "merged", "as": name})
        else:
            raise SystemExit("unknown policy %s" % policy)
    return results


def stage_dir(dest, nonce):
    if not valid_id(nonce):
        raise SystemExit("bad staging id")
    return os.path.join(confined(dest, os.path.realpath(os.path.expanduser(dest))), ".sparkbox-incoming-" + nonce)


def cleanup(dest, nonce):
    stage = stage_dir(dest, nonce)
    shutil.rmtree(stage, ignore_errors=True)
    if os.path.lexists(stage):
        raise SystemExit("couldn't remove %s (%s left)" % (stage, human(total_size([stage]))))
    return {}


def human(n):
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1000 or unit == "TB":
            return ("%d %s" if unit == "B" else "%.1f %s") % (n, unit)
        n /= 1000.0


def tree(folder, names):
    """{relative path: [kind, size]} for the named items and everything under them: what a
    copy's receiving side must hold before it's put in place."""
    folder = os.path.realpath(os.path.expanduser(folder))
    found = {}

    def add(rel, st):
        kind = "l" if stat.S_ISLNK(st.st_mode) else "d" if stat.S_ISDIR(st.st_mode) else "f"
        found[rel] = [kind, st.st_size if kind == "f" else 0]

    for name in names:
        p = os.path.join(folder, name)
        try:
            add(name, os.lstat(p))
        except OSError:
            continue
        if os.path.isdir(p) and not os.path.islink(p):
            for dirpath, dirnames, filenames in os.walk(p):
                for n in dirnames + filenames:
                    full = os.path.join(dirpath, n)
                    try:
                        add(os.path.relpath(full, folder), os.lstat(full))
                    except OSError:
                        pass
    return found


QUIET_S = 10 * 60
STALE_S = 24 * 3600


def newest_mtime(path):
    newest = os.lstat(path).st_mtime
    for dirpath, dirnames, filenames in os.walk(path):
        for n in dirnames + filenames:
            try:
                newest = max(newest, os.lstat(os.path.join(dirpath, n)).st_mtime)
            except OSError:
                pass
    return newest


def sweep(folder, keep, quiet_s=QUIET_S, stale_s=STALE_S):
    """Removes leftover staging folders in folder: any untouched for stale_s, and those not in
    keep (this SparkBox's running transfers) untouched for quiet_s."""
    folder = confined(folder, os.path.realpath(os.path.expanduser(folder)))
    removed = []
    try:
        names = os.listdir(folder)
    except OSError:
        return removed
    for n in names:
        if not n.startswith(".sparkbox-incoming-"):
            continue
        p = os.path.join(folder, n)
        try:
            idle = time.time() - newest_mtime(p)
        except OSError:
            continue
        mine = n[len(".sparkbox-incoming-"):] in keep
        if idle >= stale_s or (not mine and idle >= quiet_s):
            size = total_size([p])
            shutil.rmtree(p, ignore_errors=True)
            removed.append({"path": p, "bytes": size, "removed": not os.path.lexists(p)})
    return removed


def local_copy(folder, stage, names):
    """Copies items within this node into the staging folder, printing a progress line per file."""
    folder = confined(folder, os.path.realpath(os.path.expanduser(folder)))
    names = [plain_name(n) for n in names]
    os.makedirs(stage, exist_ok=True)
    done, files = 0, 0
    last = 0.0

    def report(rel, force=False):
        nonlocal last
        if force or time.time() - last > 0.25:
            last = time.time()
            out({"done": done, "files": files, "current": rel})

    def copy(src, dst, rel):
        nonlocal done, files
        st = os.lstat(src)
        files += 1
        if stat.S_ISLNK(st.st_mode):
            os.symlink(os.readlink(src), dst)
        elif stat.S_ISDIR(st.st_mode):
            os.mkdir(dst)
            for n in sorted(os.listdir(src)):
                copy(os.path.join(src, n), os.path.join(dst, n), os.path.join(rel, n))
            shutil.copystat(src, dst)
        else:
            with open(src, "rb") as fi, open(dst, "wb") as fo:
                for chunk in iter(lambda: fi.read(8 << 20), b""):
                    fo.write(chunk)
                    done += len(chunk)
                    report(rel)
            shutil.copystat(src, dst)
        report(rel)

    for name in names:
        copy(os.path.join(folder, name), os.path.join(stage, name), name)
    report("", force=True)
    return {"done": done, "files": files, "ok": True}


# ---- the controller lease: which SparkBox (release or dev) changes this node right now

LEASE = os.path.join(HOME, ".sparkbox", "controller.json")
LEASE_LOCK = os.path.join(HOME, ".sparkbox", "controller.lock")


def _locked(fn):
    """Runs fn() holding an exclusive flock on controller.lock, so two claims never interleave."""
    os.makedirs(os.path.dirname(LEASE_LOCK), exist_ok=True)
    with open(LEASE_LOCK, "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            return fn()
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def _read_lease():
    try:
        with open(LEASE) as f:
            held = json.load(f)
        return held if isinstance(held, dict) else None
    except (OSError, ValueError):
        return None


def lease_claim(instance, tag, mac, ttl_s, clock=time.time):
    """{held: true} once this instance holds the lease (its heartbeat written now), or
    {held: false, by, ageS} while the other instance's heartbeat is under ttl_s old."""
    def claim():
        now = clock()
        held = _read_lease()
        if held and held.get("instance") != instance:
            age = now - float(held.get("heartbeat") or 0)
            if 0 <= age < ttl_s:
                return {"held": False, "by": {k: held.get(k, "") for k in ("instance", "tag", "mac")}, "ageS": int(age)}
        tmp = "%s.%d.tmp" % (LEASE, os.getpid())
        with open(tmp, "w") as f:
            json.dump({"instance": instance, "tag": tag, "mac": mac, "heartbeat": now}, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, LEASE)
        return {"held": True}
    return _locked(claim)


def lease_release(instance):
    """Removes the lease when it's this instance's; anyone else's stays."""
    def release():
        held = _read_lease()
        if held and held.get("instance") == instance:
            os.remove(LEASE)
            return {"released": True}
        return {"released": False}
    return _locked(release)


# ---- a load's launch files (contract C-F), which launch.py reads inside the container

ROLES = ("single", "head", "worker")
CACHE_DIRS = (".cache/vllm", ".cache/flashinfer", ".triton")
# The names a load's environment may hold: what a recipe may set (recipe.rs ENV_NAMES) and what
# SparkBox sets for each rank (plan.rs RANK_ENV). The fence can't see stdin, so this holds whatever
# Rust built: anything else, PATH and LD_PRELOAD above all, could run something else.
ENV_NAMES = frozenset((
    "CUTE_DSL_ARCH", "B12X_POLICY_MODE", "B12X_W4A16_A4_PREFILL", "OMP_NUM_THREADS", "PYTORCH_CUDA_ALLOC_CONF",
    "TOKENIZERS_PARALLELISM", "TRANSFORMERS_VERBOSITY",
    "HF_HUB_OFFLINE", "GLOO_SOCKET_IFNAME", "TP_SOCKET_IFNAME", "UCX_NET_DEVICES", "OMPI_MCA_btl_tcp_if_include",
))
ENV_FAMILIES = ("NCCL_", "VLLM_")
ENV_SECRET = ("_KEY", "_TOKEN", "_SECRET", "_PASSWORD")
# What the families mustn't set, as recipe.rs ENV_CODE: any name with PLUGIN in it loads a library or
# a plugin, the logging config can name Python callables, and the debug file is written where it says.
ENV_CODE = frozenset(("VLLM_LOGGING_CONFIG_PATH", "NCCL_DEBUG_FILE"))
# What no load's argv may hold: plan.rs refuses these from Expert, and the fence can't see argv.
KEY_FLAGS = ("--api-key", "--hf-token")
CODE_FLAGS = ("--middleware", "--tool-parser-plugin", "--reasoning-parser-plugin", "--allowed-local-media-path",
              "--logits-processors", "--worker-cls", "--worker-extension-cls", "--scheduler-cls", "--config")
# Whole options that begin one of those: a shortened word is read as the longer one, these aren't.
PLAIN_FLAGS = ("--reasoning-parser",)


def env_allowed(name):
    if name.endswith(ENV_SECRET) or "PLUGIN" in name or name in ENV_CODE:
        return False
    return name in ENV_NAMES or (name.startswith(ENV_FAMILIES) and re.fullmatch("[A-Z0-9_]+", name) is not None)


def refused_flag(word):
    """The option an argv word mustn't be, or None. A key belongs in its file, remote code is the
    checkbox's plain --trust-remote-code, and the rest load code or open files. It reads a word the
    way plan.rs's Expert rule does, as vLLM's parser would: _ for -, one dash for two, --flag.key for
    --flag, and a long option cut short."""
    if not word.startswith("-"):
        return None
    flag, equals, _ = word.partition("=")
    flag = "--" + flag.lstrip("-").replace("_", "-").partition(".")[0]
    for name in KEY_FLAGS:
        if name.startswith(flag):
            return name
    if flag == "--trust-remote-code" and not equals and "." not in word:
        return None
    if "--trust-remote-code".startswith(flag):
        return "--trust-remote-code=..."
    if flag in PLAIN_FLAGS:
        return None
    for name in CODE_FLAGS:
        if name.startswith(flag):
            return name
    return "--ssl-..." if flag.startswith("--ssl") else "a plugin" if "plugin" in flag else None


def write_private(path, value):
    """JSON in a 0600 file, whole or not at all: a temporary file beside it, then a rename."""
    tmp = "%s.%s.tmp" % (path, os.urandom(4).hex())
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(value, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def drop_key(path):
    """The key file gone, or the empty folder Docker makes at a bind mount's missing source."""
    try:
        if os.path.isdir(path) and not os.path.islink(path):
            os.rmdir(path)
        else:
            os.unlink(path)
    except FileNotFoundError:
        pass


def deploy_files(load, role, spec):
    """Writes what launch.py reads, as the cluster user. Everything is checked before anything is
    written, and no refusal repeats what it was given: a word of argv could be a key."""
    if not (load.isascii() and valid_id(load)):
        raise SystemExit("bad load id")
    if role not in ROLES:
        raise SystemExit("bad role")
    if not isinstance(spec, dict) or set(spec) - {"argv", "env", "key"}:
        raise SystemExit("deploy-files takes argv, env and key.")
    argv, env, key = spec.get("argv"), spec.get("env"), spec.get("key")
    if not (isinstance(argv, list) and all(isinstance(w, str) and "\0" not in w for w in argv) and argv[:2] == ["vllm", "serve"]):
        raise SystemExit("A load runs vllm serve and nothing else, so no files were written.")
    for word in argv:
        flag = refused_flag(word)
        if flag:
            raise SystemExit("A load can't take %s, so no files were written." % flag)
    if not (isinstance(env, dict) and all(isinstance(v, str) and "\0" not in v for v in env.values())):
        raise SystemExit("A load's environment is names and text, so no files were written.")
    for name in env:
        if not env_allowed(name):
            raise SystemExit("A load can't set %s, so no files were written." % (name if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) else "that name"))
    if key is not None and role == "worker":
        raise SystemExit("A worker takes no API key, so no files were written.")
    if key is not None and not (isinstance(key, str) and key and "\0" not in key):
        raise SystemExit("The API key is empty or isn't text, so no files were written.")
    base = os.path.join(DEPLOY, load)
    for folder in (DEPLOY, base):
        os.makedirs(folder, mode=0o700, exist_ok=True)
        os.chmod(folder, 0o700)
    write_private(os.path.join(base, "argv.json"), argv)
    write_private(os.path.join(base, "env.json"), env)
    os.makedirs(KEYS, mode=0o700, exist_ok=True)
    path = os.path.join(KEYS, load)
    drop_key(path)
    if key is not None:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(fd, "w") as f:
            f.write(key)
    for folder in CACHE_DIRS:
        os.makedirs(os.path.join(HOME, folder), mode=0o700, exist_ok=True)
    return {"dir": base}


def deploy_spec():
    """deploy-files' JSON from stdin. A parser's error can quote what it read, so it's dropped."""
    try:
        return json.load(sys.stdin)
    except ValueError:
        raise SystemExit("deploy-files needs JSON on stdin, so no files were written.") from None


def recipe_import_text(config, script):
    """Parse a deliberately small shell subset. Never source, expand commands, or return secrets."""
    import shlex
    values, warnings = {}, []
    fields = {"MODEL_PATH", "MODEL", "MODEL_DIR", "MODEL_LOCAL_NAME", "SERVED_NAME", "IMAGE", "VLLM_IMAGE", "DOCKER_IMAGE",
              "NNODES", "MAX_MODEL_LEN", "MAX_NUM_SEQS", "GPU_MEM_UTIL", "MTP_TOKENS", "MOE_BACKEND", "PORT", "HEAD_IP", "MASTER_PORT"}
    recipe_env = {"CUTE_DSL_ARCH", "B12X_POLICY_MODE", "B12X_W4A16_A4_PREFILL", "OMP_NUM_THREADS",
                  "PYTORCH_CUDA_ALLOC_CONF", "TOKENIZERS_PARALLELISM", "TRANSFORMERS_VERBOSITY"}
    logical = script.replace("\\\n", " ")
    for line in (config + "\n" + logical).splitlines():
        match = re.fullmatch(r"\s*(?:export\s+)?([A-Z_][A-Z0-9_]*)=(.*)", line)
        if not match:
            continue
        name, value = match.groups()
        if name not in fields and not (name in recipe_env or (name.startswith(ENV_FAMILIES) and env_allowed(name))):
            continue
        try:
            tokens = shlex.split(value, comments=True)
        except ValueError:
            warnings.append("An assignment with unsupported quoting was skipped."); continue
        if len(tokens) != 1 or any(c in tokens[0] for c in "`$\r\n\0"):
            warnings.append("An assignment needing shell expansion was skipped."); continue
        values[name] = tokens[0]
    commands = [line.strip() for line in logical.splitlines() if re.match(r"\s*(?:exec\s+)?vllm\s+serve(?:\s|$)", line)]
    if len(commands) != 1:
        raise RuntimeError("Import expects one literal 'vllm serve' command. Complex wrappers must be translated manually; no script was executed.")
    words = shlex.split(commands[0], comments=True)
    if words[0] == "exec":
        words.pop(0)
    words = words[2:]
    def expand(word):
        word = re.sub(r"\$\{([A-Z_][A-Z0-9_]*)\}|\$([A-Z_][A-Z0-9_]*)", lambda m: values.get(m.group(1) or m.group(2), "$UNKNOWN"), word)
        if any(c in word for c in "`$\r\n\0") or word in (";", "|", "||", "&&", ">", "2>&1"):
            raise RuntimeError("The launch uses unresolved shell expressions; translate those options manually. Nothing was executed.")
        return word
    # Secrets are removed before any variable expansion or result serialization.
    clean = []
    i = 0
    while i < len(words):
        flag = words[i].split("=", 1)[0].replace("_", "-")
        if words[i] in ("${NODE_ARGS[@]}", "${EXTRA[@]}"):
            warnings.append("Shell array " + words[i].split("[")[0][2:] + " was omitted; review rank and optional backend settings in SparkBox.")
            i += 1
        elif flag in ("--api-key", "--hf-token"):
            i += 1 if "=" in words[i] else 2
            warnings.append("A secret option was omitted; SparkBox creates its own API key.")
        else:
            clean.append(expand(words[i])); i += 1
    if not clean or not clean[0].startswith("/"):
        raise RuntimeError("Import needs a literal absolute model path (or a literal MODEL_PATH assignment).")
    model = clean.pop(0)
    options, args = {}, []
    managed = {"--served-model-name", "--tensor-parallel-size", "--pipeline-parallel-size", "--gpu-memory-utilization", "--max-model-len", "--max-num-seqs", "--tool-call-parser", "--reasoning-parser", "--kv-cache-dtype", "--enable-prefix-caching", "--no-enable-prefix-caching", "--enable-expert-parallel"}
    ignored = {"--host", "--port", "--nnodes", "--node-rank", "--master-addr", "--master-port", "--distributed-executor-backend", "--headless", "--trust-remote-code", "--enable-auto-tool-choice"}
    i = 0
    while i < len(clean):
        flag, eq, value = clean[i].partition("=")
        flag = flag.replace("_", "-")
        if not flag.startswith("--"):
            raise RuntimeError("The launch has an unsupported positional argument.")
        if not eq and i + 1 < len(clean) and not clean[i + 1].startswith("--"):
            i += 1; value = clean[i]; eq = "="
        if flag in managed:
            options[flag] = value
        elif flag in ignored:
            warnings.append(flag + " is controlled by SparkBox and was not imported.")
        else:
            item = {"flag": flag, "value": value if eq else None, "json": None}
            if flag == "--speculative-config" and eq:
                item["json"], item["value"] = json.loads(value), None
            args.append(item)
        i += 1
    tp, pp = int(options.get("--tensor-parallel-size", "1")), int(options.get("--pipeline-parallel-size", "1"))
    topology = "single" if tp == pp == 1 else "tp%dpp%d" % (tp, pp) if tp > 1 and pp > 1 else "tp%d" % tp if tp > 1 else "pp%d" % pp
    image = next((values[n] for n in ("VLLM_IMAGE", "DOCKER_IMAGE", "IMAGE") if values.get(n)), "")
    if not image:
        raise RuntimeError("No literal IMAGE, VLLM_IMAGE or DOCKER_IMAGE was found; choose an image explicitly before importing.")
    if model.startswith("/models/"):
        warnings.append("The script's model path is inside its container. Choose the corresponding host folder from the Library before loading.")
    if values.get("MOE_BACKEND"):
        warnings.append("MOE_BACKEND was not translated from the optional shell array; review it manually.")
    kv = options.get("--kv-cache-dtype", "auto")
    if kv == "bf16":
        kv = "bfloat16"
    if kv not in ("auto", "bfloat16", "fp8", "fp8_e4m3", "fp8_e5m2"):
        raise RuntimeError("The script uses a KV cache dtype this importer does not recognize.")
    recipe = {"id": "imported", "title": "Imported launch (unverified)", "match": ["imported"], "image": image, "backend": "native",
              "topologies": [topology], "env": {n: v for n, v in values.items() if n not in fields}, "args": args,
              "mounts": ["vllm", "flashinfer", "triton"], "multiNodeDocker": "privileged",
              "defaults": {"share": float(options["--gpu-memory-utilization"]) if "--gpu-memory-utilization" in options else None,
                           "context": int(options["--max-model-len"]) if "--max-model-len" in options else None,
                           "seqs": int(options["--max-num-seqs"]) if "--max-num-seqs" in options else None},
              "toolParser": options.get("--tool-call-parser"), "reasoningParser": options.get("--reasoning-parser"),
              "note": "Imported without executing the scripts. Compatibility has not been verified."}
    return {"recipe": recipe, "modelPath": model, "servedName": options.get("--served-model-name", values.get("SERVED_NAME", "")),
            "tensorParallel": tp, "pipelineParallel": pp, "kvCacheDtype": kv, "prefixCaching": "--no-enable-prefix-caching" not in options,
            "expertParallel": "--enable-expert-parallel" in options, "warnings": sorted(set(warnings))}


def recipe_import(folder):
    folder = os.path.realpath(os.path.expanduser(folder))
    contents = []
    for name in ("config.env", "inner.sh"):
        with open(os.path.join(folder, name)) as f:
            text = f.read(65537)
        if len(text) > 65536:
            raise RuntimeError("An import file exceeds 64 KiB.")
        contents.append(text)
    return recipe_import_text(*contents)


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    cmd = argv[1]
    if cmd == "start":
        out(start(argv[2], argv[3], json.load(sys.stdin)))
    elif cmd == "download-tools" and len(argv) == 2:
        out(download_tools())
    elif cmd == "chain-preflight" and len(argv) == 2:
        out(chain_preflight(json.load(sys.stdin)))
    elif cmd == "disk-preflight" and len(argv) == 2:
        spec = json.load(sys.stdin)
        out(destination_disk(spec["dest"], spec["bytes"], spec.get("node", "this node")))
    elif cmd == "list":
        out(list_jobs(look="--look" in argv[2:]))
    elif cmd == "cancel":
        out(cancel(argv[2]))
    elif cmd == "scan":
        out(scan(argv[2:]))
    elif cmd == "manifest":
        path = os.path.expanduser(argv[2])
        manifest = read_manifest(path) or {}
        known = {f["path"]: f for f in manifest.get("files", [])}
        files = [dict(f, sha256=known.get(f["path"], {}).get("sha256")) for f in listing(path) if f["path"] != MANIFEST]
        out({"path": path, "repo": manifest.get("repo", ""), "revision": manifest.get("revision", ""), "files": files})
    elif cmd == "check":
        out(check(*check_args(argv[2:])))
    elif cmd == "recipe-import" and len(argv) == 3:
        out(recipe_import(argv[2]))
    elif cmd == "deploy-files" and len(argv) == 4:
        out(deploy_files(argv[2], argv[3], deploy_spec()))
    elif cmd == "delete":
        out(delete(argv[2], argv[3]))
    elif cmd == "ls":
        out(ls(argv[2] if len(argv) > 2 else "~"))
    elif cmd == "read":
        out(read_text(argv[2]))
    elif cmd == "size":
        out(total_size(argv[2:]))
    elif cmd == "du":
        out([{"path": p, "bytes": total_size([p])} for p in argv[2:]])
    elif cmd == "df":
        free, total = disk_space(os.path.realpath(os.path.expanduser(argv[2])))
        out({"free": free, "capacity": total})
    elif cmd == "commit":
        opts = dict(zip(argv[3::2], argv[4::2]))
        try:
            out(commit(argv[2], opts.get("--sha256"), opts.get("--expect"), src=opts.get("--from"),
                       backups=opts.get("--backups"), owner=opts.get("--owner"), nonce=opts.get("--nonce")))
        except NoFolder as e:
            out({"error": "The folder %s doesn't exist." % e.args[0]})
            sys.exit(1)
    elif cmd == "backups":
        out(list_backups(argv[2], argv[3]))
    elif cmd == "trash":
        out([trash_one(p) for p in argv[2:]])
    elif cmd == "untrash":
        out([untrash_one(p) for p in argv[2:]])
    elif cmd == "rm":
        out([remove(p) for p in argv[2:]])
    elif cmd == "summary":
        out(summary(argv[2], argv[3:]))
    elif cmd == "place":
        spec = json.load(sys.stdin)
        out(place(argv[2], argv[3], spec["items"], spec["nonce"], move=bool(spec.get("move"))))
    elif cmd == "cp":
        out(local_copy(argv[2], stage_dir(argv[3], argv[4]), argv[5:]))
    elif cmd == "cleanup":
        out(cleanup(argv[2], argv[3]))
    elif cmd == "tree":
        out(tree(argv[2], argv[3:]))
    elif cmd == "sweep":
        out(sweep(argv[2], set(argv[3:])))
    elif cmd == "lease" and len(argv) == 7 and argv[2] == "claim":
        out(lease_claim(argv[3], argv[4], argv[5], float(argv[6])))
    elif cmd == "lease" and len(argv) == 4 and argv[2] == "release":
        out(lease_release(argv[3]))
    else:
        raise SystemExit("unknown command " + cmd)


if __name__ == "__main__":
    main(sys.argv)
