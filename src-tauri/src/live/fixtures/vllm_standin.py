#!/usr/bin/env python3
"""A vLLM OpenAI server stand-in for fake nodes. models/fake_vllm.rs's docker shim starts it the
way a real container starts vLLM: `--contain` plays dockerd for one container, which runs the real
launch.py from the real mounts, and launch.py supervises the `vllm` on the container's PATH, which execs
this file. Standard library only, Python 3.12 or newer.

    vllm_standin.py --contain DIR NAME POLICY PARENT -- COMMAND...
    vllm_standin.py [--sb-... knobs] serve MODEL [real vLLM flags]

--contain is the daemon's part: a session of its own, the container's environment and no other (no
variable of the node's gets in), its output in DIR/NAME.log, NAME.pid and NAME.started once it
runs, and its real exit status in NAME.exit: 78 from launch.py, 1 from a rank that lost another,
137 from a KILL. POLICY is `--restart`'s: with unless-stopped it's started again whatever it exited
with, as dockerd does, after 100 ms and doubling, NAME.restarts counting, until `docker stop`
(NAME.stopping) or `docker rm`, or the test (PARENT) has gone.

`serve` reads what a real `vllm serve` is given (parse_known_args, so a flag the stand-in doesn't
know is passed over and a bad one isn't invented): MODEL, --served-model-name, --host, --port,
--max-model-len, --nnodes, --node-rank, --master-addr, --master-port, --headless,
--tensor-parallel-size and --pipeline-parallel-size, with VLLM_API_KEY from its environment, which
only launch.py can have put there. With --sb-mounts, MODEL has to be inside a real -v mount and
hold a config.json.

Several nodes really rendezvous, through --sb-rendezvous, a folder the test shares, keyed by the
master address and port: each rank writes rank-<n> with its pid. A headless rank waits for rank 0,
and rank 0 waits for all --nnodes before /health can turn 200, as vLLM's engine does. Once a rank
has seen another, that one going away makes it log an error and exit 1. No --sb-rendezvous with
--nnodes above 1 is an error, never a load that pretends. A rank's log uses real lines from the
glm53 capture (multiproc_executor.py's mq_connect_ip=, world_size=, nccl==2.30.7, "Loading model
from scratch..."), plus NCCL's NET/IB or NET/Socket line per its environment; those two are from
the plan, not a capture.

Rank 0 answers what SparkBox asks a real vLLM, in vLLM's own shapes:
- GET /health: 503 while it warms up, then 200 with an empty body.
- GET /v1/models: NAME with its max_model_len. With a key, every /v1 route wants
  `Authorization: Bearer <key>` and answers 401 {"error": "Unauthorized"} without it.
- POST /v1/completions: one token, with usage.
- POST /v1/chat/completions: a reply, or SSE chunks (a last usage chunk when
  stream_options.include_usage). A prompt with "17+25" is answered "42", or with --sb-garbage
  "4 2 2 2…" until max_tokens runs out (the GB10 garbage-output bug). A stream sends max_tokens
  tokens (64 when not given), one every --sb-stream-ms, so a client can stop it mid-reply. With
  --sb-thinks, a reply that isn't streamed thinks for --sb-think-tokens tokens first: inline in the
  content between <think> tags (Qwen3 without a reasoning parser; "inline" skips it when the
  request's chat_template_kwargs turn enable_thinking off, "inline-always" doesn't), "parsed" into
  reasoning_content with content null until the answer (gpt-oss, or a reasoning parser), or
  "untagged" in the content with no opening tag, as the template wrote it (DeepSeek-R1 without a
  reasoning parser). A reply cut off by max_tokens while thinking ends with finish_reason
  "length" and no answer.
- GET /openapi.json: `paths` from --sb-openapi, comma-separated (vLLM's chat, completions, models,
  messages, count_tokens and responses when the flag is left out; --sb-openapi none serves no
  document, a 404).
- GET /metrics: a few vLLM gauges and counters.

The log goes to stdout (the container's log): weight loading while it warms up, then
"Application startup complete." once /health is 200. Knobs: --sb-warmup, --sb-hang (it never gets
past starting), --sb-hold-join F (it doesn't join while F exists), --sb-die-after S (a rank that
fails partway, exit 1), --sb-mq-lan IP (the message queue uses this LAN address whatever
VLLM_HOST_IP says), --sb-print-key (it prints its key in two of the shapes a log could). At start it writes DIR/shm/vllm-<pid> (its
/dev/shm segment) and DIR/pid. SIGTERM, as `docker stop` sends it, removes the segment and exits 0;
SIGKILL leaves the segment behind, as it does for a real vLLM. It exits by itself once the test
process (--sb-parent-pid) is gone.
"""

import argparse
import http.server
import json
import os
import signal
import subprocess
import sys
import threading
import time

DEFAULT_PATHS = [
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/models",
    "/v1/messages",
    "/v1/messages/count_tokens",
    "/v1/responses",
]

# What a normal prompt gets back, a token at a time.
WORDS = ["Hello", " from", " the", " vLLM", " stand", "-in", "."]
# What a thinking model thinks first, a token at a time.
THOUGHT = ["Okay", ",", " so", " I", " need", " to", " add", " 17", " and", " 25", "."]


def log(line):
    print(line, flush=True)


def stamp():
    return time.strftime("%m-%d %H:%M:%S")


class Engine:
    def __init__(self, args):
        self.args = args
        # None until every rank has joined: only then do the weights load.
        self.started = None
        self.key = os.environ.get("VLLM_API_KEY") or None
        self.lock = threading.Lock()
        self.running = 0
        self.generated = 0
        self.prompted = 0
        self.served = 0
        self.warm_logged = False

    def join(self):
        self.started = time.monotonic()

    def warm(self):
        return self.started is not None and not self.args.hang and time.monotonic() - self.started >= self.args.warmup

    def count(self, prompt, out, done=False):
        with self.lock:
            self.prompted += prompt
            self.generated += out
            self.served += 1 if done else 0

    def metrics(self):
        name = self.args.served
        with self.lock:
            rows = [
                ("vllm:num_requests_running", "gauge", self.running),
                ("vllm:num_requests_waiting", "gauge", 0),
                ("vllm:kv_cache_usage_perc", "gauge", 0.01 * self.running),
                ("vllm:prompt_tokens_total", "counter", self.prompted),
                ("vllm:generation_tokens_total", "counter", self.generated),
                ("vllm:request_success_total", "counter", self.served),
            ]
        out = []
        for metric, kind, value in rows:
            out.append(f"# TYPE {metric} {kind}")
            out.append(f'{metric}{{engine="0",model_name="{name}"}} {float(value)}')
        return "\n".join(out) + "\n"


def answer_tokens(engine, prompt, max_tokens):
    """The reply's tokens and why it ended: (tokens, finish_reason). The sum is answered and
    stops; garbage, and anything else, runs on until max_tokens."""
    if "17+25" in prompt and not engine.args.garbage:
        return ["42"][:max_tokens], "stop" if max_tokens >= 1 else "length"
    if "17+25" in prompt:
        tokens = ["4"] + [" 2"] * (max_tokens - 1)
    else:
        tokens = [WORDS[i % len(WORDS)] for i in range(max_tokens)]
    return tokens[:max_tokens], "length"


def thinks(engine, req):
    """Whether a chat reply starts with thinking: inline honours chat_template_kwargs'
    enable_thinking as Qwen3's templates do; the others think whatever it says."""
    if engine.args.thinks == "inline":
        return (req.get("chat_template_kwargs") or {}).get("enable_thinking", True) is not False
    return engine.args.thinks is not None


def think_then_answer(engine, prompt, limit):
    """A thinking model's reply within `limit` tokens: (content, reasoning_content, finish_reason,
    tokens). inline keeps the thinking in the content between <think> tags, as Qwen3 does without
    a reasoning parser; untagged the same with no opening tag, which DeepSeek-R1's template wrote;
    parsed puts it in reasoning_content with content null until the answer, as vLLM's reasoning
    parsers (gpt-oss's among them) do."""
    parsed = engine.args.thinks == "parsed"
    thought = [THOUGHT[i % len(THOUGHT)] for i in range(engine.args.think_tokens)]
    if engine.args.thinks == "untagged":
        thought = thought + ["\n</think>\n\n"]
    elif not parsed:
        thought = ["<think>\n"] + thought + ["\n</think>\n\n"]
    if limit <= len(thought):
        so_far = "".join(thought[:limit])
        return (None, so_far, "length", limit) if parsed else (so_far, None, "length", limit)
    tokens, finish = answer_tokens(engine, prompt, limit - len(thought))
    if parsed:
        return "".join(tokens), "".join(thought), finish, len(thought) + len(tokens)
    return "".join(thought + tokens), None, finish, len(thought) + len(tokens)


def handler_for(engine):
    args = engine.args

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "uvicorn"
        sys_version = ""

        def log_message(self, fmt, *a):
            pass

        def access(self, code):
            phrase = {200: "OK", 401: "Unauthorized", 404: "Not Found", 400: "Bad Request", 503: "Service Unavailable"}.get(code, "")
            log(f'INFO:     {self.client_address[0]}:{self.client_address[1]} - "{self.command} {self.path} HTTP/1.1" {code} {phrase}')

        def send(self, code, body, ctype="application/json"):
            data = body if isinstance(body, bytes) else body.encode()
            self.send_response(code)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            self.access(code)

        def send_json(self, code, value):
            self.send(code, json.dumps(value))

        def authorized(self):
            if engine.key is None:
                return True
            if self.headers.get("Authorization", "") == f"Bearer {engine.key}":
                return True
            self.send_json(401, {"error": "Unauthorized"})
            return False

        def body(self):
            n = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(n) if n else b""
            try:
                return json.loads(raw or b"{}")
            except ValueError:
                return None

        def not_found(self):
            self.send_json(404, {"detail": "Not Found"})

        def warming(self):
            if engine.warm():
                return False
            self.send(503, b"", "text/plain")
            return True

        def wrong_model(self, req):
            model = req.get("model", args.served)
            if model == args.served:
                return False
            self.send_json(404, {"error": {"message": f"The model `{model}` does not exist.", "type": "NotFoundError", "param": "model", "code": 404}})
            return True

        def do_GET(self):
            path = self.path.split("?")[0]
            if path == "/health":
                self.send(200 if engine.warm() else 503, b"", "text/plain")
            elif path == "/metrics":
                self.send(200, engine.metrics(), "text/plain; version=0.0.4; charset=utf-8")
            elif path == "/openapi.json":
                if args.openapi is not None and not args.openapi:
                    return self.not_found()
                paths = args.openapi if args.openapi is not None else DEFAULT_PATHS
                doc = {"openapi": "3.1.0", "info": {"title": "vLLM", "version": "0.11.0"}, "paths": {p: {"post" if p != "/v1/models" else "get": {}} for p in paths}}
                self.send_json(200, doc)
            elif path == "/v1/models":
                if self.warming() or not self.authorized():
                    return
                model = {"id": args.served, "object": "model", "created": int(time.time()), "owned_by": "vllm", "root": args.served, "parent": None, "max_model_len": args.max_model_len}
                self.send_json(200, {"object": "list", "data": [model]})
            else:
                self.not_found()

        def do_POST(self):
            path = self.path.split("?")[0]
            if path not in ("/v1/completions", "/v1/chat/completions"):
                return self.not_found()
            if self.warming() or not self.authorized():
                return
            req = self.body()
            if req is None:
                return self.send_json(400, {"error": {"message": "Expecting value: line 1 column 1 (char 0)", "type": "BadRequestError", "param": None, "code": 400}})
            if self.wrong_model(req):
                return
            created = int(time.time())
            if path == "/v1/completions":
                prompt = str(req.get("prompt", ""))
                tokens, finish = answer_tokens(engine, prompt, int(req.get("max_tokens") or 16))
                usage = {"prompt_tokens": len(prompt.split()) + 1, "total_tokens": len(prompt.split()) + 1 + len(tokens), "completion_tokens": len(tokens)}
                engine.count(usage["prompt_tokens"], len(tokens), done=True)
                choice = {"index": 0, "text": "".join(tokens), "logprobs": None, "finish_reason": finish, "stop_reason": None}
                return self.send_json(200, {"id": f"cmpl-{created}", "object": "text_completion", "created": created, "model": args.served, "choices": [choice], "usage": usage})
            prompt = " ".join(str(m.get("content", "")) for m in req.get("messages", []) if isinstance(m, dict))
            stream = bool(req.get("stream"))
            limit = int(req.get("max_tokens") or (64 if stream else 16))
            if not stream and thinks(engine, req):
                content, reasoning, finish, n = think_then_answer(engine, prompt, limit)
            else:
                tokens, finish = answer_tokens(engine, prompt, limit)
                content, reasoning, n = "".join(tokens), None, len(tokens)
            usage = {"prompt_tokens": len(prompt.split()) + 4, "total_tokens": len(prompt.split()) + 4 + n, "completion_tokens": n}
            if not stream:
                engine.count(usage["prompt_tokens"], n, done=True)
                message = {"role": "assistant", "content": content, "tool_calls": [], "reasoning_content": reasoning}
                choice = {"index": 0, "message": message, "logprobs": None, "finish_reason": finish, "stop_reason": None}
                return self.send_json(200, {"id": f"chatcmpl-{created}", "object": "chat.completion", "created": created, "model": args.served, "choices": [choice], "usage": usage, "prompt_logprobs": None})
            self.stream(req, tokens, finish, usage, created)

        def stream(self, req, tokens, finish, usage, created):
            with_usage = bool((req.get("stream_options") or {}).get("include_usage"))
            self.send_response(200)
            self.send_header("content-type", "text/event-stream; charset=utf-8")
            self.send_header("transfer-encoding", "chunked")
            self.end_headers()
            self.access(200)

            def chunk(choices, use=None):
                c = {"id": f"chatcmpl-{created}", "object": "chat.completion.chunk", "created": created, "model": args.served, "choices": choices}
                if with_usage:
                    c["usage"] = use
                return c

            def emit(value):
                data = f"data: {value}\n\n".encode()
                self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
                self.wfile.flush()

            with engine.lock:
                engine.running += 1
            sent = 0
            try:
                emit(json.dumps(chunk([{"index": 0, "delta": {"role": "assistant", "content": ""}, "logprobs": None, "finish_reason": None}])))
                for t in tokens:
                    if args.stream_ms:
                        time.sleep(args.stream_ms / 1000)
                    emit(json.dumps(chunk([{"index": 0, "delta": {"content": t}, "logprobs": None, "finish_reason": None}])))
                    sent += 1
                    engine.count(0, 1)
                emit(json.dumps(chunk([{"index": 0, "delta": {"content": ""}, "logprobs": None, "finish_reason": finish, "stop_reason": None}])))
                if with_usage:
                    emit(json.dumps(chunk([], usage)))
                emit("[DONE]")
                self.wfile.write(b"0\r\n\r\n")
                self.wfile.flush()
                engine.count(usage["prompt_tokens"], 0, done=True)
            except (BrokenPipeError, ConnectionResetError):
                log(f"INFO {stamp()} [async_llm.py:430] Aborted request chatcmpl-{created} after {sent} tokens.")
                self.close_connection = True
            finally:
                with engine.lock:
                    engine.running -= 1

    return Handler


def contain(argv):
    """dockerd's part for one container: `--contain DIR NAME POLICY PARENT -- COMMAND...`. A session
    of its own, so the SSH session that ran `docker run` can go; the container's environment and
    none of the node's; its real exit status, whoever it was when it ended; and its restart
    policy, which `docker run -d` has long since left behind."""
    d, name, policy, parent, command = argv[0], argv[1], argv[2], int(argv[3]), argv[5:]
    os.setsid()
    # In a session of its own the shim's TERM is no danger, and a policy mustn't outlive the Standin.
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    root = os.path.join(d, name + ".root")
    env = {"PATH": os.pathsep.join([os.path.join(root, "bin"), os.environ.get("PATH", "")]), "HOME": "/root", "PYTHONDONTWRITEBYTECODE": "1"}
    cwd = root if os.path.isdir(root) else d

    def file(suffix):
        return os.path.join(d, name + suffix)

    def remove(suffix):
        try:
            os.remove(file(suffix))
        except OSError:
            pass

    def publish(suffix, value):
        # Readers must never see the empty/truncated half of a startup or restart record.
        temp = file(f"{suffix}.tmp.{os.getpid()}")
        with open(temp, "w", encoding="utf-8") as f:
            f.write(value)
        os.replace(temp, file(suffix))

    # A test can hold startup beyond the old shim's five-second success fallback. This is
    # before Popen, not model warmup: Docker must not acknowledge a container that doesn't exist.
    if os.path.exists(os.path.join(d, "hold-start")):
        publish(".start-held", "waiting\n")
        while os.path.exists(os.path.join(d, "hold-start")):
            try:
                os.kill(parent, 0)
            except ProcessLookupError:
                return
            time.sleep(0.025)

    def stay_down():
        try:
            os.kill(parent, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            pass
        return policy != "unless-stopped" or os.path.exists(file(".stopping")) or not os.path.exists(file(".id"))

    def finish_namespace():
        # macOS has no container PID namespace. When its simulated PID 1 exits, Linux
        # would kill the remaining engine, even after launch.py created a private group.
        # Only the engine registered for this exact container can be signalled here.
        try:
            pid = int(open(file(".engine-pid"), encoding="utf-8").read())
        except (OSError, ValueError):
            return

        def owned():
            result = subprocess.run(["ps", "-p", str(pid), "-o", "command="], capture_output=True, text=True)
            return "vllm_standin.py" in result.stdout and file(".seen") in result.stdout

        if owned():
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 2
            while owned():
                if time.monotonic() >= deadline:
                    raise RuntimeError("the fixture's engine outlived its container")
                time.sleep(.01)
        remove(".engine-pid")

    wait = 0.1
    restarts = 0
    with open(file(".log"), "ab", buffering=0) as out:
        while True:
            try:
                child = subprocess.Popen(command, env=env, cwd=cwd, stdin=subprocess.DEVNULL, stdout=out, stderr=out)
            except OSError as e:
                out.write(f"docker: Error response from daemon: failed to create task for container: {e}\n".encode())
                publish(".exit", "127\n")
                return
            with open(os.path.join(d, "pids"), "a", encoding="utf-8") as f:
                f.write(f"{os.getpid()}\n{child.pid}\n")
            publish(".pid", f"{child.pid}\n")
            publish(".started", f"{child.pid} {int(time.time() * 1000)}\n")
            if restarts:
                remove(".restarting")
            # `docker stop` marks it, then looks at .pid: seen here, or the stop has the new pid.
            if restarts and os.path.exists(file(".stopping")):
                child.terminate()
            code = child.wait()
            finish_namespace()
            publish(".ended", f"{int(time.time() * 1000)}\n")
            again = not stay_down()
            # Status `restarting` while it waits, and RestartCount up as it ended, both before the
            # exit status appears, so no look sees a plain `exited` in between: dockerd sets them
            # together under the container's lock.
            if again:
                restarts += 1
                publish(".restarts", f"{restarts}\n")
                open(file(".restarting"), "w", encoding="utf-8").close()
            publish(".exit", f"{128 - code if code < 0 else code}\n")
            if not again:
                return
            time.sleep(wait)
            wait *= 2
            if stay_down():
                remove(".restarting")
                return


class Rendezvous:
    """Where the ranks of one load meet: a folder per master address and port. Each rank's file
    holds its pid (fake nodes share a Mac, so a pid can be asked about from any of them)."""

    def __init__(self, root, addr, port, rank, nnodes):
        self.dir = os.path.join(root, f"{addr}-{port}")
        self.rank = rank
        self.nnodes = nnodes
        # The ranks this one has seen alive: from then on, one that goes is an error.
        self.seen = set()
        os.makedirs(self.dir, exist_ok=True)

    def path(self, name):
        return os.path.join(self.dir, name)

    def read(self, name):
        try:
            with open(self.path(name), encoding="utf-8") as f:
                return int(f.read().strip() or 0)
        except (OSError, ValueError):
            return 0

    def write(self, name, value):
        temp = self.path(f".{name}.{os.getpid()}")
        with open(temp, "w", encoding="utf-8") as f:
            f.write(f"{value}\n")
        os.replace(temp, self.path(name))

    def alive(self, rank):
        pid = self.read(f"rank-{rank}")
        if not pid:
            return False
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            pass
        return True

    def announce(self):
        if self.rank == 0:
            try:
                os.remove(self.path("joined"))
            except OSError:
                pass
        self.write(f"rank-{self.rank}", os.getpid())

    def joined(self):
        """Rank 0: every rank is here. A headless rank: rank 0 has said so."""
        if self.rank == 0:
            here = [r for r in range(self.nnodes) if self.alive(r)]
            self.seen.update(here)
            if len(here) == self.nnodes:
                self.write("joined", os.getpid())
                return True
            return False
        if self.alive(0):
            self.seen.add(0)
            if self.read("joined") == self.read("rank-0"):
                self.seen.update(range(self.nnodes))
                return True
        return False

    def gone(self):
        """A rank this one saw that isn't there any more, or None."""
        return next((r for r in sorted(self.seen) if r != self.rank and not self.alive(r)), None)

    def leave(self):
        if self.read(f"rank-{self.rank}") == os.getpid():
            try:
                os.remove(self.path(f"rank-{self.rank}"))
            except OSError:
                pass


def knob_parser():
    ap = argparse.ArgumentParser(description="A vLLM stand-in for SparkBox's fake nodes.", allow_abbrev=False, add_help=False)
    ap.add_argument("--sb-state", required=True)
    ap.add_argument("--sb-warmup", type=float, default=0.0)
    ap.add_argument("--sb-garbage", action="store_true")
    ap.add_argument("--sb-openapi", help="the paths, comma-separated, or none")
    ap.add_argument("--sb-stream-ms", type=int, default=0)
    ap.add_argument("--sb-thinks", choices=["inline", "inline-always", "parsed", "untagged"])
    ap.add_argument("--sb-think-tokens", type=int, default=40)
    ap.add_argument("--sb-parent-pid", type=int)
    ap.add_argument("--sb-rendezvous")
    ap.add_argument("--sb-hang", action="store_true")
    ap.add_argument("--sb-hold-join", help="this rank doesn't announce itself while this file exists")
    ap.add_argument("--sb-die-after", type=float)
    ap.add_argument("--sb-mq-lan")
    ap.add_argument("--sb-print-key", action="store_true")
    ap.add_argument("--sb-lan-ip", default="192.168.50.51")
    ap.add_argument("--sb-mounts")
    ap.add_argument("--sb-seen", help="what the process really got, for the test: argv, the environment's names, and whether it has a key")
    ap.add_argument("--sb-idle", action="store_true", help="no server: a container's process that only waits to be stopped")
    return ap


def vllm_parser():
    """The flags of `vllm serve` the stand-in acts on; the rest are passed over, as parse_known_args does."""
    ap = argparse.ArgumentParser(prog="vllm", allow_abbrev=False, add_help=False, exit_on_error=False)
    ap.add_argument("command")
    ap.add_argument("model")
    ap.add_argument("--served-model-name")
    ap.add_argument("--host", default="0.0.0.0")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--max-model-len", type=int, default=32768)
    ap.add_argument("--nnodes", type=int, default=1)
    ap.add_argument("--node-rank", type=int, default=0)
    ap.add_argument("--master-addr", default="127.0.0.1")
    ap.add_argument("--master-port", type=int, default=29501)
    ap.add_argument("--headless", action="store_true")
    ap.add_argument("--tensor-parallel-size", type=int, default=1)
    ap.add_argument("--pipeline-parallel-size", type=int, default=1)
    return ap


def refuse(line):
    log(f"ERROR {stamp()} [cli_args.py:1] {line}")
    sys.exit(1)


def model_is_mounted(args, mounts):
    """MODEL is inside a real -v mount and holds a config.json: None, or what vLLM would say."""
    pairs = []
    if mounts:
        with open(mounts, encoding="utf-8") as f:
            pairs = [line.rstrip("\n").split(":", 1) for line in f if ":" in line]
    for src, dst in pairs:
        if args.model == dst or args.model.startswith(dst.rstrip("/") + "/"):
            host = src + args.model[len(dst):]
            if os.path.isfile(os.path.join(host, "config.json")):
                return None
            return f"OSError: {args.model} does not appear to have a file named config.json."
    return f"OSError: {args.model} is not a local folder (it isn't mounted in this container)."


def ranks_are_right(a):
    """What vLLM refuses about its own flags, in a sentence, or None."""
    if a.command != "serve":
        return f"unknown command {a.command}"
    if a.nnodes > 1 and a.headless == (a.node_rank == 0):
        return f"--headless goes with every rank but 0 (rank {a.node_rank}, headless {a.headless})"
    if not 0 <= a.node_rank < a.nnodes:
        return f"--node-rank {a.node_rank} is outside --nnodes {a.nnodes}"
    world = a.tensor_parallel_size * a.pipeline_parallel_size
    if a.nnodes > 1 and world % a.nnodes != 0:
        return f"world_size {world} doesn't divide across {a.nnodes} nodes"
    return None


def ib_line(node_ip):
    """NCCL's transport line, per this rank's environment: NET/IB when an HCA is named and IB isn't off."""
    hcas = [h for h in os.environ.get("NCCL_IB_HCA", "").split(",") if h]
    if hcas and os.environ.get("NCCL_IB_DISABLE", "0") != "1":
        names = " ".join(f"[{i}]{h}:1/RoCE" for i, h in enumerate(hcas))
        return f"[0] NCCL INFO NET/IB : Using {names} [RO]; OOB {os.environ.get('NCCL_SOCKET_IFNAME', '')}:{node_ip}<0>"
    return f"[0] NCCL INFO NET/Socket : Using [0]{os.environ.get('NCCL_SOCKET_IFNAME', 'enP7s7')}:{node_ip}<0>"


def main():
    if sys.argv[1:2] == ["--contain"]:
        contain(sys.argv[2:])
        return

    knobs, rest = knob_parser().parse_known_args()

    if knobs.sb_seen:
        # Register the engine separately from the launcher/daemon for fixture teardown.
        # Also cover a launcher killed before this interpreter has finished starting.
        container_pid = os.getppid()
        if container_pid == 1:
            os.kill(os.getpid(), signal.SIGKILL)
        d = os.path.dirname(knobs.sb_seen)
        with open(os.path.join(d, "pids"), "a", encoding="utf-8") as f:
            f.write(f"{os.getpid()}\n")
        with open(knobs.sb_seen.removesuffix(".seen") + ".engine-pid", "w", encoding="utf-8") as f:
            f.write(str(os.getpid()))

        def watch_namespace():
            while os.getppid() == container_pid:
                time.sleep(.01)
            os.kill(os.getpid(), signal.SIGKILL)

        threading.Thread(target=watch_namespace, daemon=True).start()

    def parent_gone():
        if not knobs.sb_parent_pid:
            return False
        try:
            os.kill(knobs.sb_parent_pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            pass
        return False

    stop = threading.Event()

    def on_term(signum, frame):
        stop.set()

    signal.signal(signal.SIGTERM, on_term)
    signal.signal(signal.SIGINT, on_term)

    if knobs.sb_idle:
        while not stop.is_set() and not parent_gone():
            stop.wait(0.2)
        sys.exit(0)

    a, unknown = vllm_parser().parse_known_args(rest)
    if knobs.sb_seen:
        with open(knobs.sb_seen, "w", encoding="utf-8") as f:
            json.dump({"argv": rest, "unknown": unknown, "env": sorted(os.environ), "hasKey": "VLLM_API_KEY" in os.environ,
                       "values": {k: v for k, v in os.environ.items() if k != "VLLM_API_KEY"}}, f)
    openapi = None if knobs.sb_openapi is None else [] if knobs.sb_openapi == "none" else knobs.sb_openapi.split(",")
    args = argparse.Namespace(served=a.served_model_name or a.model, max_model_len=a.max_model_len, warmup=knobs.sb_warmup, hang=knobs.sb_hang,
                              garbage=knobs.sb_garbage, openapi=openapi, stream_ms=knobs.sb_stream_ms, thinks=knobs.sb_thinks,
                              think_tokens=knobs.sb_think_tokens)
    wrong = ranks_are_right(a)
    if wrong:
        refuse(wrong)
    if knobs.sb_mounts:
        wrong = model_is_mounted(a, knobs.sb_mounts)
        if wrong:
            refuse(wrong)
    if a.nnodes > 1 and not knobs.sb_rendezvous:
        refuse("this fake node has no rendezvous folder (Knobs.rendezvous): several ranks can't meet")

    rank = a.node_rank
    head = rank == 0
    node_ip = os.environ.get("VLLM_HOST_IP") or knobs.sb_lan_ip
    mq_ip = knobs.sb_mq_lan or node_ip
    tag = "APIServer" if head else "Worker"
    log(f"({tag} pid=1) INFO {stamp()} [api_server.py:1880] vLLM API server version 0.28.1")
    log(f"({tag} pid=1) INFO {stamp()} [utils.py:328] non-default args: {{'host': '{a.host}', 'port': {a.port}, 'served_model_name': ['{args.served}']}}")
    if a.nnodes > 1:
        log(f"({tag} pid=1) INFO {stamp()} [arg_utils.py:2252] Inferred data_parallel_rank 0 from node_rank {rank}")
        log(f"(EngineCore pid=178) INFO {stamp()} [multiproc_executor.py:153] DP group leader: node_rank={rank}, node_rank_within_dp={rank}, master_addr={a.master_addr}, mq_connect_ip={mq_ip} (local), world_size={a.nnodes}, local_world_size=1")

    if knobs.sb_print_key:
        key = os.environ.get("VLLM_API_KEY", "")
        log(f"({tag} pid=1) INFO {stamp()} [utils.py:328] non-default args: {{'api_key': ['{key}'], 'port': {a.port}}}")
        log(f"({tag} pid=1) DEBUG {stamp()} [api_server.py:1890] using key {key}")

    engine = Engine(args)
    server = None
    if head:
        # Loopback in place of every other address, so nothing listens beyond this Mac.
        host = "127.0.0.1"
        try:
            server = http.server.ThreadingHTTPServer((host, a.port), handler_for(engine))
        except OSError as e:
            log(f"ERROR {stamp()} [api_server.py:1950] {e}")
            sys.exit(1)
        server.daemon_threads = True

    shm = os.path.join(knobs.sb_state, "shm", f"vllm-{os.getpid()}")
    os.makedirs(os.path.dirname(shm), exist_ok=True)
    with open(shm, "wb") as f:
        f.write(b"\0" * 4096)
    with open(os.path.join(knobs.sb_state, "pid"), "w", encoding="utf-8") as f:
        f.write(f"{os.getpid()}\n")
    if server:
        threading.Thread(target=server.serve_forever, daemon=True).start()

    meet = Rendezvous(knobs.sb_rendezvous, a.master_addr, a.master_port, rank, a.nnodes) if a.nnodes > 1 else None
    announced = False

    def leave(code):
        if meet:
            meet.leave()
        if server:
            server.shutdown()
            server.server_close()
        try:
            os.remove(shm)
        except OSError:
            pass
        sys.exit(code)

    def on_join():
        if meet:
            log(f"(Worker pid=251) INFO {stamp()} [parallel_state.py:1798] world_size={a.nnodes} rank={rank} local_rank=0 distributed_init_method=tcp://{a.master_addr}:{a.master_port} backend=nccl")
            log(f"(Worker pid=251) INFO {stamp()} [pynccl.py:120] vLLM is using nccl==2.30.7")
            log(ib_line(node_ip))
        log(f"(Worker_TP0 pid=251) INFO {stamp()} [model_runner.py:390] Loading model from scratch...")
        engine.join()

    if not meet and not knobs.sb_hang:
        on_join()
    began = time.monotonic()
    shards = 4
    shown = -1
    while not stop.is_set() and not parent_gone():
        now = time.monotonic()
        if knobs.sb_die_after is not None and now - began >= knobs.sb_die_after:
            if knobs.sb_print_key:
                log(f"(Worker_TP0 pid=251) ERROR {stamp()} [multiproc_executor.py:944] the key was {os.environ.get('VLLM_API_KEY', '')}")
            log(f"(Worker_TP0 pid=251) ERROR {stamp()} [multiproc_executor.py:944] WorkerProc failed to start.")
            log(f"(Worker_TP0 pid=251) ERROR {stamp()} [multiproc_executor.py:944] RuntimeError: rank {rank} failed while starting")
            leave(1)
        # A rank that hangs never reaches the rendezvous: the others go on waiting for it.
        if meet and not announced and not knobs.sb_hang and not (knobs.sb_hold_join and os.path.exists(knobs.sb_hold_join)):
            meet.announce()
            announced = True
        if meet and announced and engine.started is None and not knobs.sb_hang and meet.joined():
            on_join()
        if meet:
            lost = meet.gone()
            if lost is not None:
                log(f"(Worker_TP0 pid=251) ERROR {stamp()} [multiproc_executor.py:944] Connection to rank {lost} was lost: the peer closed the connection.")
                leave(1)
        if engine.started is not None and not engine.warm_logged and not knobs.sb_hang:
            warm = engine.warm()
            elapsed = now - engine.started
            done_shards = shards if warm else min(shards - 1, int(elapsed / max(args.warmup, 0.001) * shards))
            if done_shards != shown:
                shown = done_shards
                pct = done_shards * 100 // shards
                log(f"Loading safetensors checkpoint shards: {pct:3d}% Completed | {done_shards}/{shards} [00:0{done_shards}<00:00,  1.00s/it]")
            if warm:
                if head:
                    log(f"INFO {stamp()} [launcher.py:37] Available routes are: {', '.join(DEFAULT_PATHS)}")
                    log("INFO:     Started server process [1]")
                    log("INFO:     Waiting for application startup.")
                    log("INFO:     Application startup complete.")
                engine.warm_logged = True
        stop.wait(0.05)

    log("INFO:     Shutting down")
    log("INFO:     Application shutdown complete.")
    leave(0)


if __name__ == "__main__":
    main()
