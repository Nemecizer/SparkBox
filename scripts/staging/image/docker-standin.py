#!/usr/bin/python3
# The staging node's `docker` (/usr/local/bin/docker): a stand-in for the Docker CLI, since the
# staging Spark has no Docker inside. It answers the verbs a load of C-L2, its unload (C-U2), its
# pre-flight (C-P2) and a download send (run, ps, stop, kill, rm, inspect, image inspect, and `info`
# with the pre-flight's format) the way Docker 29's CLI does, with its words and exit codes (those of
# a name clash, an unknown image or container, `rm` of a running container, `rm -f` of a missing
# name, `ps --filter name=` as a regular expression, a bare NAME in --env-file, `inspect`, what a
# signal does to `docker run`, and the pre-flight's formats, `{{.Status}}`, a quoted `{{.Command}}`
# and `docker info`'s store line, checked against Docker Desktop 29.9.2 on 2026-10-30, 2026-10-31
# and 2026-11-01), and keeps its "containers" and "images" in /var/lib/sparkbox-staging-docker. Every
# call is recorded in calls.jsonl there: its argv exactly as the shell passed it, its environment,
# and every process's command line and (where the caller may read it) environment at that moment,
# so a test can show what reached docker and that a secret reached none of them. Any other verb
# answers as a CLI with no daemon behind it (exit 1).
#
# What a container runs: nothing, except `hf`. launch.py and vLLM never run here, whatever a load's
# `run` says: this records how a load was started and doesn't pretend it worked. `run -d` starts no
# process. A foreground `run` with
# --entrypoint hf runs `hf download` against a folder standing in for the Hub, and the other
# foreground commands end at once. Its process is the daemon's, not the CLI's, so what real Docker
# does holds here too:
#   - it has a session of its own, so kill -9 of the CLI or its process group leaves it running;
#   - the CLI hands SIGTERM and SIGINT on, and ends with the container's exit code;
#   - `hf` is the container's PID 1 and has no SIGTERM handler, so the kernel drops SIGTERM: the
#     CLI and the container both go on, until something sends SIGKILL. With --init it ends, 143.
#     (Real hf on Docker Desktop: scripts/staging/token-proof.sh step 5.)
#
#   images.json      {"<ref>": "sha256:<64 hex>"}: what `image inspect` and `run` find (a test writes it)
#   containers.json  what `run` made: id, name, labels, image, running, and what `inspect` shows
#   calls.jsonl      one line per call
#   hub/<path>       the Hub's files, for `hf download` (a test writes them)
#   hf-hold          a file's path (a test writes it): hf stops halfway through that file until
#                    this is removed
#   hf.jsonl         one line per `hf download`: where its token came from and the token's SHA-256
import fcntl
import fnmatch
import hashlib
import json
import os
import re
import secrets
import signal
import sys
import time

STATE = "/var/lib/sparkbox-staging-docker"
NO_DAEMON = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?"
# The image's own environment, which `inspect` lists after the container's.
IMAGE_ENV = ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"]
# The pre-flight's `docker info -f '{{.Driver}} {{json .DriverStatus}}'`, as Docker Desktop 29.9.2
# answers. The Sparks' store is overlay2 (a look-only read, 2026-11-01), so this isn't their line.
STORE_LINE = 'overlayfs [["driver-type","io.containerd.snapshotter.v1"]]'
STORE_FORMAT = "{{.Driver}} {{json .DriverStatus}}"

# `docker run` options that take a value (the rest are flags).
RUN_VALUES = {
    "--name", "--label", "-l", "--network", "--net", "--ipc", "--gpus", "--ulimit", "--restart", "-e", "--env",
    "-v", "--volume", "--mount", "--entrypoint", "--shm-size", "-p", "--publish", "-w", "--workdir", "-u", "--user",
    "--pid", "--runtime", "--security-opt", "--cap-add", "--cap-drop", "--device", "--memory", "-m", "--cpus",
    "--platform", "--hostname", "-h", "--add-host", "--env-file", "--log-driver", "--log-opt", "--tmpfs",
}


def load(name, default):
    try:
        with open(os.path.join(STATE, name)) as f:
            return json.load(f)
    except FileNotFoundError:
        return default


def save(name, value):
    path = os.path.join(STATE, name)
    tmp = "%s.%d.tmp" % (path, os.getpid())
    with open(tmp, "w") as f:
        json.dump(value, f, indent=1)
    os.replace(tmp, path)


def locked():
    """One change to the list of containers at a time (a running container takes itself off it)."""
    f = open(os.path.join(STATE, ".lock"), "a")
    fcntl.flock(f, fcntl.LOCK_EX)
    return f


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read().replace(b"\0", b" ").decode("utf-8", "replace").strip()
    except OSError:
        return None


def record():
    procs, environs = [], []
    for pid in sorted((p for p in os.listdir("/proc") if p.isdigit()), key=int):
        cmdline = read("/proc/%s/cmdline" % pid)
        if cmdline is not None:
            procs.append("%s %s" % (pid, cmdline))
        env = read("/proc/%s/environ" % pid)
        if env is not None:
            environs.append("%s %s" % (pid, env))
    line = {"argv": sys.argv[1:], "uid": os.getuid(), "env": dict(os.environ), "procs": procs, "environs": environs}
    with open(os.path.join(STATE, "calls.jsonl"), "a") as f:
        f.write(json.dumps(line) + "\n")


def fail(msg, code=1):
    print(msg, file=sys.stderr)
    sys.exit(code)


def run_fail(msg):
    """`docker run`'s refusal: the daemon's words, then the CLI's hint, exit 125."""
    print(msg + "\n\nRun 'docker run --help' for more information", file=sys.stderr)
    sys.exit(125)


def options(args, values):
    """Splits `args` into (options as (name, value) pairs, operands), docker's way: `--opt=v`,
    `--opt v`, `-abc` clusters of flags, and everything from the first operand on is operands."""
    opts, i = [], 0
    while i < len(args) and args[i].startswith("-") and args[i] != "-":
        a = args[i]
        if a == "--":
            i += 1
            break
        if a.startswith("--") and "=" in a:
            opts.append(tuple(a.split("=", 1)))
        elif a in values:
            if i + 1 >= len(args):
                fail('flag needs an argument: %s' % a, 125)
            opts.append((a, args[i + 1]))
            i += 1
        elif not a.startswith("--") and len(a) > 2:
            opts.extend(("-" + c, None) for c in a[1:])
        else:
            opts.append((a, None))
        i += 1
    return opts, args[i:]


def value(opts, *names):
    return next((v for k, v in opts if k in names), None)


def full_ref(ref):
    last = ref.rsplit("/", 1)[-1]
    return ref if ":" in last or "@" in ref else ref + ":latest"


def find_image(images, ref):
    if ref in images:
        return images[ref]
    if full_ref(ref) in images:
        return images[full_ref(ref)]
    for image_id in images.values():
        if ref.startswith("sha256:") and image_id.startswith(ref) and len(ref) > 7:
            return image_id
    return None


def find(containers, ref):
    for c in containers:
        if ref == c["name"] or (len(ref) >= 1 and c["id"].startswith(ref)):
            return c
    return None


def config_env(opts):
    """Config.Env as Docker's CLI builds it: an --env-file's lines first, then each -e. K=V stays
    as it is; a bare K takes the CLI's own K, and with no such K in the CLI's environment an
    --env-file's line is dropped, where a -e one stays bare."""
    lines = []
    for path in (v for k, v in opts if k == "--env-file"):
        with open(path) as f:
            lines += [l.strip() for l in f if l.strip() and not l.startswith("#")]
    lines = [l for l in lines if "=" in l or l in os.environ]
    lines += [v for k, v in opts if k in ("-e", "--env")]
    return [l if "=" in l or l not in os.environ else "%s=%s" % (l, os.environ[l]) for l in lines] + IMAGE_ENV


def run(args):
    opts, rest = options(args, RUN_VALUES)
    if not rest:
        fail('docker: \'docker run\' requires at least 1 argument', 125)
    image = rest[0]
    name = value(opts, "--name") or "sparkbox_standin_%s" % secrets.token_hex(3)
    with locked():
        images, containers = load("images.json", {}), load("containers.json", [])
        image_id = find_image(images, image)
        if image_id is None and image.startswith("sha256:"):
            run_fail("docker: Error response from daemon: No such image: %s" % image)
        if image_id is None:
            # Docker would pull it here; the stand-in has no registry, and says so.
            print("Unable to find image '%s' locally" % full_ref(image), file=sys.stderr)
            fail("docker (SparkBox staging stand-in): no registry to pull %s from" % full_ref(image), 125)
        clash = next((c for c in containers if c["name"] == name), None)
        if clash:
            run_fail('docker: Error response from daemon: Conflict. The container name "/%s" is already in use by container "%s". You have to remove (or rename) that container to be able to reuse that name.' % (name, clash["id"]))
        c = {
            "id": secrets.token_hex(32), "name": name, "image": image_id, "running": True, "pid": None,
            "labels": dict(v.split("=", 1) if "=" in v else (v, "") for k, v in opts if k in ("--label", "-l")),
            "ref": image, "user": value(opts, "-u", "--user") or "", "env": config_env(opts), "at": time.time(),
            "entrypoint": value(opts, "--entrypoint"), "cmd": rest[1:],
            "binds": [v for k, v in opts if k in ("-v", "--volume")],
            "rm": ("--rm", None) in opts, "init": ("--init", None) in opts,
        }
        containers.append(c)
        save("containers.json", containers)
    if ("-d", None) in opts or ("--detach", None) in opts:
        print(c["id"])
    else:
        foreground(c)


def foreground(c):
    """`docker run` without -d: the CLI, while the container's process runs in its own session."""
    sys.stdout.flush()
    pid = os.fork()
    if pid == 0:
        os.setsid()
        code = 1
        try:
            code = process(c)
        except SystemExit as e:
            code = e.code
        finally:
            sys.stdout.flush()
            ended(c["id"])
            os._exit(code)
    with locked():
        containers = load("containers.json", [])
        here = find(containers, c["id"])
        if here:
            here["pid"] = pid
            save("containers.json", containers)
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, lambda s, _: os.kill(pid, s))
    _, status = os.waitpid(pid, 0)
    sys.exit(128 + os.WTERMSIG(status) if os.WIFSIGNALED(status) else os.WEXITSTATUS(status))


def ended(cid):
    """A container's process is over: --rm takes it off the list, or it stays, exited."""
    with locked():
        containers = load("containers.json", [])
        c = find(containers, cid)
        if c and c["rm"]:
            containers.remove(c)
        elif c:
            c["running"] = False
        save("containers.json", containers)


def process(c):
    """What a foreground container runs (see the top of this file for its signals)."""
    if c["entrypoint"] != "hf":
        return 0
    signal.signal(signal.SIGINT, lambda *_: sys.exit(130))
    if c["init"]:
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    else:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    return hf(c)


def host_path(c, path):
    """The path on the node behind a path in the container: a -v of it or of a folder above it."""
    for bind in c["binds"]:
        src, dst = bind.split(":")[:2]
        dst = dst.rstrip("/")
        if path == dst or path.startswith(dst + "/"):
            return src + path[len(dst):]
    return None


def note_token(c):
    """huggingface_hub's get_token(): HF_TOKEN in the container's environment, else the file
    HF_TOKEN_PATH names. No such file: no token. A file it may not read: hf dies, as the real one
    does. Only where it came from and a hash go on record."""
    env = dict(e.split("=", 1) for e in c["env"] if "=" in e)
    token, source = env.get("HF_TOKEN"), "env"
    if not token and env.get("HF_TOKEN_PATH"):
        path = host_path(c, env["HF_TOKEN_PATH"])
        try:
            with open(path or "") as f:
                token, source = f.read().strip(), "file"
        except FileNotFoundError:
            token = None
    line = {"source": source if token else "none", "sha256": hashlib.sha256(token.encode()).hexdigest() if token else ""}
    with open(os.path.join(STATE, "hf.jsonl"), "a") as f:
        f.write(json.dumps(line) + "\n")


def fetch(hub, dest, name):
    """One file as `hf download --local-dir` writes it: it grows as <hash>.<etag>.incomplete under
    .cache/huggingface/download, moves into place when it's whole, and leaves its .lock and
    .metadata there. A file already in place at its size is left alone."""
    final = os.path.join(dest, name)
    with open(os.path.join(hub, name), "rb") as f:
        data = f.read()
    if os.path.exists(final) and os.path.getsize(final) == len(data):
        return
    cache = os.path.join(dest, ".cache", "huggingface", "download", os.path.dirname(name))
    os.makedirs(cache, exist_ok=True)
    os.makedirs(os.path.dirname(final), exist_ok=True)
    base = os.path.basename(name)
    etag = hashlib.sha1(data).hexdigest()
    incomplete = os.path.join(cache, "%s.%s.incomplete" % (hashlib.sha1(base.encode()).hexdigest()[:8], etag))
    open(os.path.join(cache, base + ".lock"), "w").close()
    with open(incomplete, "wb") as f:
        f.write(data[:len(data) // 2])
        f.flush()
        while read(os.path.join(STATE, "hf-hold")) == name:
            time.sleep(0.05)
        f.write(data[len(data) // 2:])
    os.replace(incomplete, final)
    with open(os.path.join(cache, base + ".metadata"), "w") as f:
        f.write("%s\n%s\n%f\n" % ("0" * 40, etag, time.time()))


def hf(c):
    """`hf download <repo> --local-dir <dir> --include ...` from the stand-in's hub folder. Any
    other hf command ends at once (has_hf only asks whether `hf --help` runs)."""
    args = c["cmd"]
    if args[:1] != ["download"]:
        return 0
    opts, _ = options(args[2:], {"--revision", "--local-dir", "--include", "--exclude"})
    note_token(c)
    dest = host_path(c, value(opts, "--local-dir"))
    if dest is None:
        fail("hf (SparkBox staging stand-in): --local-dir isn't a mounted folder: it would be lost with the container")
    hub = os.path.join(STATE, "hub")
    names = sorted(os.path.relpath(os.path.join(d, f), hub) for d, _, files in os.walk(hub) for f in files)
    patterns = [v for k, v in opts if k == "--include"]
    for name in names:
        if not patterns or any(fnmatch.fnmatchcase(name, p) for p in patterns):
            fetch(hub, dest, name)
    print(value(opts, "--local-dir"))
    return 0


def matches(c, filters):
    for f in filters:
        key, _, value_ = f.partition("=")
        if key == "label":
            k, eq, v = value_.partition("=")
            if k not in c["labels"] or (eq and c["labels"][k] != v):
                return False
        elif key == "name":
            # A regular expression, searched in the name and in /name.
            if not re.search(value_, c["name"]) and not re.search(value_, "/" + c["name"]):
                return False
        elif key == "id":
            if not c["id"].startswith(value_):
                return False
        else:
            fail('Error response from daemon: invalid filter \'%s\'' % key)
    return True


def status(c):
    """What `ps` says of a container, as if it had started: the stand-in doesn't run it."""
    took = int(time.time() - c["at"])
    if not c["running"]:
        return "Exited (0) %d seconds ago" % took
    return "Up %d seconds" % took if took != 1 else "Up 1 second"


def command(c, whole):
    """{{.Command}}: its words joined and quoted, cut to 20 characters without --no-trunc."""
    text = " ".join(([c["entrypoint"]] if c["entrypoint"] else []) + c["cmd"])
    return '"%s"' % (text if whole or len(text) <= 20 else text[:19] + "…")


def template(fmt, c, whole=False):
    out, i = "", 0
    while i < len(fmt):
        j = fmt.find("{{", i)
        if j < 0:
            out += fmt[i:]
            break
        out += fmt[i:j]
        k = fmt.find("}}", j)
        field = fmt[j + 2:k].strip()
        if field == ".ID":
            out += c["id"] if whole else c["id"][:12]
        elif field == ".Names":
            out += c["name"]
        elif field == ".Image":
            out += c["image"]
        elif field == ".State":
            out += "running" if c["running"] else "exited"
        elif field == ".Status":
            out += status(c)
        elif field == ".Command":
            out += command(c, whole)
        elif field.startswith(".Label "):
            out += c["labels"].get(json.loads(field[len(".Label "):]), "")
        else:
            fail("template: the docker stand-in doesn't model %s" % field, 1)
        i = k + 2
    return out


def ps(args):
    opts, rest = options(args, {"--filter", "-f", "--format", "-n", "--last"})
    if rest:
        fail('"docker ps" accepts no arguments.', 1)
    every = any(k in ("-a", "--all") for k, _ in opts)
    quiet = any(k in ("-q", "--quiet") for k, _ in opts)
    filters = [v for k, v in opts if k in ("--filter", "-f")]
    fmt = next((v for k, v in opts if k == "--format"), None)
    whole = ("--no-trunc", None) in opts
    for c in load("containers.json", []):
        if (every or c["running"]) and matches(c, filters):
            print(c["id"][:12] if quiet else template(fmt, c, whole) if fmt else "%s   %s   %s" % (c["id"][:12], c["image"], c["name"]))


def alive(pid):
    state = read("/proc/%d/stat" % pid)
    return state is not None and state.rsplit(")", 1)[-1].split()[0] != "Z"


def halt(cid, grace):
    """Ends a container's process, if it has one: SIGTERM, and SIGKILL after `grace` seconds (none:
    SIGKILL at once). Then the container is exited, or gone with --rm. False: no such container."""
    with locked():
        c = find(load("containers.json", []), cid)
    if c is None:
        return False
    pid = c["pid"]
    if pid and alive(pid):
        if grace:
            os.kill(pid, signal.SIGTERM)
        deadline = time.time() + grace
        while alive(pid) and time.time() < deadline:
            time.sleep(0.05)
        if alive(pid):
            os.kill(pid, signal.SIGKILL)
        while alive(pid):
            time.sleep(0.01)
    ended(c["id"])
    return True


def stop(args):
    opts, rest = options(args, {"-t", "--timeout", "--time", "-s", "--signal"})
    if not rest:
        fail('docker: \'docker stop\' requires at least 1 argument', 1)
    grace, code = int(value(opts, "-t", "--timeout", "--time") or 10), 0
    for ref in rest:
        if not halt(ref, grace):
            print("Error response from daemon: No such container: %s" % ref, file=sys.stderr)
            code = 1
            continue
        print(ref)
    sys.exit(code)


def kill(args):
    opts, rest = options(args, {"-s", "--signal"})
    if not rest:
        fail('docker: \'docker kill\' requires at least 1 argument', 1)
    code = 0
    for ref in rest:
        if not halt(ref, 0):
            print("Error response from daemon: cannot kill container: %s: No such container: %s" % (ref, ref), file=sys.stderr)
            code = 1
            continue
        print(ref)
    sys.exit(code)


def rm(args):
    opts, rest = options(args, set())
    if not rest:
        fail('docker: \'docker rm\' requires at least 1 argument', 1)
    force = any(k in ("-f", "--force") for k, _ in opts)
    code = 0
    for ref in rest:
        with locked():
            c = find(load("containers.json", []), ref)
        if c is None:
            if not force:  # with -f a missing name is silent, exit 0
                print("Error response from daemon: No such container: %s" % ref, file=sys.stderr)
                code = 1
            continue
        if c["running"] and not force:
            print('Error response from daemon: cannot remove container "%s": container is running: stop the container before removing or force remove' % c["name"], file=sys.stderr)
            code = 1
            continue
        halt(c["id"], 0)
        with locked():
            containers = load("containers.json", [])
            left = find(containers, c["id"])
            if left:
                containers.remove(left)
                save("containers.json", containers)
        print(ref)
    sys.exit(code)


def described(c):
    """A container as `docker inspect` shows it: what Docker keeps of how it was started."""
    command = ([c["entrypoint"]] if c["entrypoint"] else []) + c["cmd"]
    mounts = []
    for bind in c["binds"]:
        parts = bind.split(":")
        mode = parts[2] if len(parts) > 2 else ""
        mounts.append({"Type": "bind", "Source": parts[0], "Destination": parts[1], "Mode": mode, "RW": mode != "ro", "Propagation": "rprivate"})
    return {
        "Id": c["id"], "Path": command[0] if command else "", "Args": command[1:],
        "State": {"Status": "running" if c["running"] else "exited", "Running": c["running"], "Pid": c["pid"] or 0, "ExitCode": 0},
        "Image": c["image"], "Name": "/" + c["name"],
        "HostConfig": {"Binds": c["binds"], "AutoRemove": c["rm"], "Init": c["init"]},
        "Mounts": mounts,
        "Config": {"User": c["user"], "Env": c["env"], "Cmd": c["cmd"], "Image": c["ref"],
                   "Entrypoint": [c["entrypoint"]] if c["entrypoint"] else None, "Labels": c["labels"]},
    }


def inspect(args, no_such):
    opts, rest = options(args, {"-f", "--format", "--type"})
    if not rest:
        fail('docker: \'docker inspect\' requires at least 1 argument', 1)
    if value(opts, "-f", "--format"):
        fail("template: the docker stand-in doesn't model %s" % value(opts, "-f", "--format"), 1)
    containers, found, code = load("containers.json", []), [], 0
    for ref in rest:
        c = find(containers, ref)
        if c is None:
            print(no_such % ref, file=sys.stderr)
            code = 1
            continue
        found.append(described(c))
    print(json.dumps(found, indent=4))
    sys.exit(code)


def image_inspect(args):
    opts, rest = options(args, {"-f", "--format"})
    if not rest:
        fail('docker: \'docker image inspect\' requires at least 1 argument', 1)
    fmt = next((v for k, v in opts if k in ("-f", "--format")), None)
    images, code, found = load("images.json", {}), 0, []
    for ref in rest:
        image_id = find_image(images, ref)
        if image_id is None:
            print("Error response from daemon: No such image: %s" % (ref if ref.startswith("sha256:") else full_ref(ref)), file=sys.stderr)
            code = 1
            continue
        tags = [r for r, i in images.items() if i == image_id]
        if fmt is None:
            found.append({"Id": image_id, "RepoTags": tags})
        elif fmt.strip() == "{{.Id}}":
            print(image_id)
        else:
            fail("template: the docker stand-in doesn't model %s" % fmt, 1)
    if fmt is None:
        print(json.dumps(found, indent=4))
    sys.exit(code)


def info(args):
    """Whether a daemon answers (sparkjob's `docker` fact, before a load) and the pre-flight's store
    line. Any other format finds no daemon, which is how the update check has always read it here."""
    opts, _ = options(args, {"-f", "--format"})
    fmt = value(opts, "-f", "--format")
    if fmt is None:
        print("Server:\n Server Version: 29.9.2 (SparkBox staging stand-in)")
    elif fmt == STORE_FORMAT:
        print(STORE_LINE)
    else:
        fail(NO_DAEMON)
    sys.exit(0)


def main():
    # Its own files stay readable whatever umask the caller set.
    os.umask(0o022)
    record()
    args = sys.argv[1:]
    verb, rest = (args[0], args[1:]) if args else ("", [])
    if verb == "container" and rest[:1] == ["inspect"]:
        inspect(rest[1:], "Error response from daemon: No such container: %s")
    if verb == "container" and rest and rest[0] in ("run", "ls", "stop", "kill", "rm"):
        verb, rest = ("ps" if rest[0] == "ls" else rest[0]), rest[1:]
    if verb == "image" and rest[:1] == ["inspect"]:
        image_inspect(rest[1:])
    elif verb == "info":
        info(rest)
    elif verb == "inspect":
        inspect(rest, "error: no such object: %s")
    elif verb in ("run", "ps", "stop", "kill", "rm"):
        {"run": run, "ps": ps, "stop": stop, "kill": kill, "rm": rm}[verb](rest)
    else:
        fail(NO_DAEMON)


main()
