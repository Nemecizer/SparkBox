"""Serving probe: proves an inference endpoint on this node still generates.

The node agent (sparkbox_agent.py) calls probe() every 15 s for each endpoint in
agent.json `endpoints` whose head is this node, so a model that stops producing
tokens is caught while SparkBox's Mac sleeps. It has the same rules as the app
(src-tauri/src/models/endpoints.rs `assess`):

  Ready = it answered (/v1/models, /api/tags or /health) within 60 s
          and generated within 120 s: a one-token test request (the canary),
          or live traffic (its generated-token counter rose while /health was ok).

  Down  = "Not answering (connection refused)"   two failed model-list probes
          "Engine not healthy: /health 503"      two non-200 /health answers
          "Stalled: 3 requests running, no tokens for 45 s"
          "Didn't generate: HTTP 500: … (2 tries)"

The canary runs every 60 s (15 s after a failure) and not at all while live
traffic is proof: POST /v1/completions {"prompt": "Hi", "max_tokens": 1,
"temperature": 0}, or /v1/chat/completions for chat-only servers. Ollama is only
tested on a model it holds in memory, with keep_alive set to what that model has
left, so the test never keeps a model loaded longer; with nothing in memory (or
under 30 s left) it is Ready and idle. The endpoint's API key, when it has one,
comes from the agent's credential (target["key"]) and goes into a header, never
into a log or the result.

Everything goes to loopback (or the one address the server binds) on this node.
Standard library only.
"""
import datetime
import json
import time
import urllib.error
import urllib.request

READY_WITHIN_MS = 60_000
PROOF_WITHIN_MS = 120_000
CANARY_EVERY_MS = 60_000
CANARY_RETRY_MS = 15_000
CANARY_TIMEOUT_S = 30
SLOW_CANARY_MS = 10_000
PASSIVE_WITHIN_MS = 60_000
STALL_AFTER_S = 45
OLLAMA_MIN_LEFT_S = 30
FAILS_DOWN = 2

#: The names each engine publishes a number under (the first present wins), as metrics.rs reads them.
GEN = ("vllm:generation_tokens_total", "sglang:generation_tokens_total", "trtllm_generation_tokens_total")
PROMPT = ("vllm:prompt_tokens_total", "sglang:prompt_tokens_total", "trtllm_prompt_tokens_total")
RUNNING = ("vllm:num_requests_running", "sglang:num_running_reqs", "trtllm_num_requests_running")
WAITING = ("vllm:num_requests_waiting", "sglang:num_queue_reqs", "trtllm_num_requests_waiting")
KV = ("vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc", "sglang:token_usage", "trtllm_kv_cache_utilization")
PREEMPT = ("vllm:num_preemptions_total", "sglang:num_retracted_reqs_total")
TTFT = ("vllm:time_to_first_token_seconds", "sglang:time_to_first_token_seconds", "trtllm_time_to_first_token_seconds")

HEALTH_PATH = {"vllm": "/health", "sglang": "/health", "trtllm": "/health", "llamacpp": "/health", "nim": "/v1/health/ready"}
METRICS = ("vllm", "sglang", "trtllm", "nim")


def http_request(method, url, headers=None, body=None, timeout=3.0):
    """(status, body bytes, seconds). Status 0 when nothing answered; the reason in the body."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers=dict(headers or {}))
    if data is not None:
        req.add_header("Content-Type", "application/json")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read(), time.monotonic() - started
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.read(), time.monotonic() - started
    except (TimeoutError, OSError) as e:
        reason = getattr(e, "reason", e)
        text = "timed out" if isinstance(reason, TimeoutError) or "timed out" in str(reason) else (
            "connection refused" if "refused" in str(reason).lower() else str(reason))
        return 0, text.encode(), time.monotonic() - started


# ---- Prometheus text

def parse_metrics(text):
    """{name: [(labels dict, value)]}, skipping comments and anything malformed."""
    out = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        labels = {}
        if "{" in line:
            name, rest = line.split("{", 1)
            if "}" not in rest:
                continue
            inner, value = rest.rsplit("}", 1)
            for part in _split_labels(inner):
                if "=" in part:
                    k, v = part.split("=", 1)
                    labels[k.strip()] = v.strip().strip('"')
        else:
            bits = line.split()
            if len(bits) < 2:
                continue
            name, value = bits[0], bits[1]
        try:
            v = float(value.split()[0])
        except (ValueError, IndexError):
            continue
        out.setdefault(name.strip(), []).append((labels, v))
    return out


def _split_labels(s):
    parts, cur, quoted = [], "", False
    for ch in s:
        if ch == '"':
            quoted = not quoted
        if ch == "," and not quoted:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    if cur:
        parts.append(cur)
    return parts


def first(samples, names):
    for n in names:
        if n in samples:
            return sum(v for _, v in samples[n] if v == v and abs(v) != float("inf"))
    return None


def buckets(samples, names):
    """Cumulative (le, count) pairs, ascending, summed across label sets."""
    for n in names:
        rows = samples.get(n + "_bucket")
        if not rows:
            continue
        by = {}
        for labels, v in rows:
            le = labels.get("le")
            if le is None:
                continue
            bound = float("inf") if le in ("+Inf", "Inf") else float(le)
            by[bound] = by.get(bound, 0.0) + v
        return sorted(by.items())
    return []


def quantile(q, bks):
    """Prometheus histogram_quantile: linear inside the bucket holding the rank."""
    if not bks or bks[-1][1] <= 0:
        return None
    rank = q * bks[-1][1]
    lo_bound, lo_count = 0.0, 0.0
    for le, count in bks:
        if count >= rank:
            if le == float("inf"):
                return lo_bound
            span = count - lo_count
            return lo_bound + (le - lo_bound) * ((rank - lo_count) / span if span > 0 else 1.0)
        lo_bound, lo_count = le, count
    return None


def bucket_delta(now, then):
    if not then or len(now) != len(then):
        return now
    out = []
    for (le, a), (le2, b) in zip(now, then):
        if le != le2 or a < b:
            return now  # the server restarted: what it has since then
        out.append((le, a - b))
    return out


# ---- The probe

def _first_line(text):
    for line in str(text).splitlines():
        if line.strip():
            return line.strip()
    return ""


def _error_words(body):
    try:
        v = json.loads(body or b"{}")
    except ValueError:
        return None
    if not isinstance(v, dict):
        return None
    e = v.get("error")
    text = e.get("message") if isinstance(e, dict) else e
    text = text or v.get("detail") or v.get("message")
    return _first_line(text)[:120] if isinstance(text, str) and text else None


def _fresh(ms, now_ms, within):
    return ms is not None and now_ms - ms <= within


def canary_due(state, now_ms):
    """Every minute, sooner after a failure, never while live traffic proves it generates."""
    if _fresh(state.get("passiveOkMs"), now_ms, PASSIVE_WITHIN_MS):
        return False
    if state.get("untestable") and _fresh(state.get("triedMs"), now_ms, CANARY_EVERY_MS - 1):
        return False  # it wants a key we don't have: ask again in a minute, not every probe
    c = state.get("canary")
    if not c:
        return True
    if not c.get("ok"):
        return now_ms - c["atMs"] >= CANARY_RETRY_MS
    if state.get("canaryOkMs") is None:
        return True
    return now_ms - c["atMs"] >= CANARY_EVERY_MS


def _ollama_target(loaded, now_s):
    """The model in memory with the most time left (at least 30 s) and that time, or None."""
    best = None
    for m in loaded:
        name = m.get("name") or m.get("model")
        if not name:
            continue
        left = None
        exp = m.get("expires_at")
        if isinstance(exp, (int, float)):
            left = max(0, int(exp - now_s))
        elif isinstance(exp, str) and exp:
            at = _rfc3339(exp)
            left = max(0, int(at - now_s)) if at is not None else None
        if left is not None and left < OLLAMA_MIN_LEFT_S:
            continue
        key = left if left is not None else float("inf")
        if best is None or key > best[2]:
            best = (name, left, key)
    return (best[0], best[1]) if best else None


def _rfc3339(s):
    """Unix seconds from "2026-10-28T10:15:00.123-04:00"; None when it can't be read."""
    try:
        t = s.replace("Z", "+00:00")
        if "." in t:
            head, rest = t.split(".", 1)
            tz = ""
            for i, ch in enumerate(rest):
                if ch in "+-":
                    tz = rest[i:]
                    break
            t = head + tz
        return datetime.datetime.fromisoformat(t).timestamp()
    except ValueError:
        return None


def _canary(target, base, req, http, key):
    """One test token: ({ok, ms, error}, chat_only, untestable)."""
    engine = target.get("engine", "vllm")
    headers = {"Authorization": "Bearer " + key} if key else {}
    if engine == "ollama":
        body = {"model": req["model"], "prompt": "Hi", "stream": False, "options": {"num_predict": 1}}
        if req.get("keepAliveS") is not None:
            body["keep_alive"] = "%ds" % req["keepAliveS"]
        routes = [("/api/generate", body)]
    else:
        completion = {"model": req["model"], "prompt": "Hi", "max_tokens": 1, "temperature": 0}
        chat = {"model": req["model"], "messages": [{"role": "user", "content": "Hi"}], "max_tokens": 1, "temperature": 0}
        routes = [("/v1/chat/completions", chat)] if req.get("chat") else [("/v1/completions", completion), ("/v1/chat/completions", chat)]
    chat_only = False
    for i, (path, body) in enumerate(routes):
        status, reply, secs = http("POST", base + path, headers, body, CANARY_TIMEOUT_S)
        if status == 200:
            try:
                v = json.loads(reply or b"{}")
            except ValueError:
                v = {}
            if engine == "ollama":
                token = v.get("done") is True and isinstance(v.get("response"), str)
            else:
                token = bool(v.get("choices"))
            if token:
                return {"ok": True, "ms": int(round(secs * 1000)), "error": None}, chat_only, False
            return {"ok": False, "ms": None, "error": _error_words(reply) or "the reply had no token"}, chat_only, False
        if status in (401, 403):
            return None, chat_only, True
        words = _error_words(reply)
        not_here = status in (404, 405) or (status == 400 and words and "chat" in words.lower())
        if engine != "ollama" and not_here and i + 1 < len(routes):
            chat_only = True
            continue
        if status == 0:
            why = reply.decode(errors="replace") if isinstance(reply, bytes) else str(reply)
            error = "no reply in %d s" % CANARY_TIMEOUT_S if "timed out" in why else (why or "no reply")
        elif not_here:
            error = "it serves neither completions nor chat"
        else:
            error = "HTTP %d: %s" % (status, words) if words else "HTTP %d" % status
        return {"ok": False, "ms": None, "error": error}, chat_only, False
    return {"ok": False, "ms": None, "error": "no route answered"}, chat_only, False


def probe(target, state, now_ms, http=None):
    """Probes one endpoint, updates `state` (the agent keeps it between calls) and returns
    {id, ok, health, healthOk, canary:{ok, ms, error}, genTps, promptTps, running, waiting,
    kvPct, ttftP95, preemptions, stalledS, error}. `ok` is the Ready verdict; `error` says
    why not. Numbers it couldn't read are None."""
    http = http or http_request
    engine = target.get("engine", "vllm")
    base = "http://%s:%d" % (target.get("host") or "127.0.0.1", int(target["port"]))
    key = target.get("key")
    stall_after = int(target.get("stallAfterS") or STALL_AFTER_S)
    out = {"id": target["id"], "ok": False, "health": "loading", "healthOk": None, "canary": None, "genTps": None,
           "promptTps": None, "running": None, "waiting": None, "kvPct": None, "ttftP95": None, "preemptions": None,
           "stalledS": None, "error": None}
    headers = {"Authorization": "Bearer " + key} if key else {}

    # The model list: two failures in a row and it's Down.
    status, body, _ = http("GET", base + ("/api/tags" if engine == "ollama" else "/v1/models"), headers, None, 3)
    if status == 200 or status in (401, 403):
        state["lastOkMs"] = now_ms
        state["fails"] = 0
        state["lastError"] = None
    else:
        state["fails"] = state.get("fails", 0) + 1
        state["lastError"] = "HTTP %d" % status if status else (body.decode(errors="replace") if isinstance(body, bytes) else str(body)) or "no answer"
    models = []
    if status == 200:
        try:
            v = json.loads(body or b"{}")
            models = [m.get("id") or m.get("name") for m in (v.get("data") or v.get("models") or []) if isinstance(m, dict)]
        except ValueError:
            pass

    # The engine's own health check.
    path = HEALTH_PATH.get(engine)
    if path and state.get("fails", 0) == 0:
        hs, _, _ = http("GET", base + path, headers, None, 3)
        if hs == 200:
            state["healthOkMs"] = now_ms
            state["healthFails"] = 0
            state["healthError"] = None
        elif hs:
            state["healthFails"] = state.get("healthFails", 0) + 1
            state["healthError"] = "%s %d" % (path, hs)
        out["healthOk"] = hs == 200
    elif not path:
        out["healthOk"] = status == 200

    # /metrics: rates, queue, cache, TTFT p95, and the token counter (live traffic, stalls).
    if engine in METRICS and state.get("fails", 0) == 0:
        ms_status, text, _ = http("GET", base + "/metrics", {}, None, 3)
        if ms_status == 200:
            s = parse_metrics(text.decode(errors="replace") if isinstance(text, bytes) else text)
            gen, prompt = first(s, GEN), first(s, PROMPT)
            running, waiting, kv, pre = first(s, RUNNING), first(s, WAITING), first(s, KV), first(s, PREEMPT)
            ttft = buckets(s, TTFT)
            prev = state.get("scrape")
            if prev:
                secs = (now_ms - prev["ms"]) / 1000.0
                rate = lambda a, b: (a - b if a >= b else a) / secs if a is not None and b is not None and secs > 0 else None
                out["genTps"], out["promptTps"] = rate(gen, prev.get("gen")), rate(prompt, prev.get("prompt"))
                p95 = quantile(0.95, bucket_delta(ttft, [tuple(b) for b in prev.get("ttft") or []]))
                out["ttftP95"] = round(p95 * 1000, 1) if p95 is not None else None
            # The counter, as the app keeps it: the endpoint-preempt rule counts its growth.
            out["running"], out["waiting"], out["preemptions"] = running, waiting, pre
            out["kvPct"] = round(kv * 100, 2) if kv is not None else None
            _note_tokens(state, gen, running or 0.0, bool(path), now_ms)
            state["scrape"] = {"ms": now_ms, "gen": gen, "prompt": prompt, "pre": pre, "ttft": ttft}

    # A test token, when it answers and one is due.
    answering = state.get("fails", 0) == 0
    req = None
    if engine == "ollama" and status == 200:
        ps_status, ps, _ = http("GET", base + "/api/ps", headers, None, 3)
        loaded = []
        if ps_status == 200:
            try:
                loaded = json.loads(ps or b"{}").get("models") or []
            except ValueError:
                loaded = []
        pick = _ollama_target(loaded, now_ms / 1000.0)
        state["idle"] = pick is None
        if pick:
            req = {"model": pick[0], "keepAliveS": pick[1]}
    elif engine != "ollama":
        model = target.get("model") or (models[0] if models else None)
        if model:
            req = {"model": model, "chat": bool(target.get("chat") or state.get("chatOnly"))}
    if answering and req and canary_due(state, now_ms):
        result, chat_only, untestable = _canary(target, base, req, http, key)
        state["triedMs"] = now_ms
        state["chatOnly"] = state.get("chatOnly", False) or chat_only
        if untestable:
            state["untestable"] = True
        elif result is not None:
            result = dict(result, atMs=now_ms)
            state["canary"] = result
            if result["ok"]:
                state["canaryFails"] = 0
                state["canaryOkMs"] = now_ms
                state["untestable"] = False
            else:
                state["canaryFails"] = state.get("canaryFails", 0) + 1
    if state.get("canary"):
        c = state["canary"]
        out["canary"] = {"ok": c["ok"], "ms": c.get("ms"), "error": c.get("error")}

    flat = state.get("flatSinceMs")
    out["stalledS"] = int((now_ms - flat) / 1000) if flat is not None else None
    health, reason = _verdict(state, out, engine, stall_after, now_ms)
    out["health"], out["ok"], out["error"] = health, health == "ready", reason
    if health != "ready":
        # Proof is about the run that gave it: once it isn't Ready it has to generate again.
        state["canaryOkMs"] = None
        state["passiveOkMs"] = None
    return out


def _note_tokens(state, gen, running, has_health, now_ms):
    """The token counter: when it last rose, and since when requests run with it flat."""
    old = state.get("gen")
    rose = gen is not None and old is not None and (gen > old or (gen < old and gen > 0))
    if rose:
        state["genRoseMs"] = now_ms
    if running > 0 and gen is not None and old is not None and not rose:
        if state.get("flatSinceMs") is None:
            state["flatSinceMs"] = state.get("scrapeMs") or now_ms
    else:
        state["flatSinceMs"] = None
    healthy = state.get("healthOkMs") if has_health else state.get("lastOkMs")
    if rose and _fresh(healthy, now_ms, READY_WITHIN_MS):
        state["passiveOkMs"] = now_ms
    if gen is not None:
        state["gen"] = gen
    state["running"] = running
    state["scrapeMs"] = now_ms


def _verdict(state, out, engine, stall_after, now_ms):
    """("ready" | "loading" | "down", why not) by endpoints.rs `assess`'s rules."""
    if state.get("fails", 0) >= FAILS_DOWN:
        return "down", "Not answering (%s)" % (state.get("lastError") or "no answer")
    if state.get("healthFails", 0) >= FAILS_DOWN:
        return "down", "Engine not healthy: %s" % (state.get("healthError") or "/health failed")
    stalled = out.get("stalledS")
    if stalled is not None and stalled >= stall_after:
        n = int(round(state.get("running") or 0))
        return "down", "Stalled: %d request%s running, no tokens for %d s" % (n, "" if n == 1 else "s", stalled)
    if state.get("canaryFails", 0) >= FAILS_DOWN:
        err = (state.get("canary") or {}).get("error") or "no token came back"
        return "down", "Didn't generate: %s (%d tries)" % (err, state["canaryFails"])
    answered = max([t for t in (state.get("lastOkMs"), state.get("healthOkMs")) if t is not None], default=None)
    if not _fresh(answered, now_ms, READY_WITHIN_MS):
        if answered is None:
            return "loading", "Not answering yet (%s)" % (state.get("lastError") or "no answer")
        return "down", "No answer for %d s" % ((now_ms - answered) // 1000)
    proof = max([t for t in (state.get("canaryOkMs"), state.get("passiveOkMs")) if t is not None], default=None)
    if _fresh(proof, now_ms, PROOF_WITHIN_MS) or state.get("idle") or state.get("untestable"):
        return "ready", None
    if proof is not None:
        return "down", "No token for %d s" % ((now_ms - proof) // 1000)
    return "loading", "Checking that it generates…"


def note(state):
    """The Ready note: "Slow: 1 token took 12.4 s", "Idle: no model in memory", or None."""
    if state.get("idle"):
        return "Idle: no model in memory"
    c = state.get("canary") or {}
    if c.get("ok") and (c.get("ms") or 0) > SLOW_CANARY_MS:
        return "Slow: 1 token took %.1f s" % (c["ms"] / 1000.0)
    return None
