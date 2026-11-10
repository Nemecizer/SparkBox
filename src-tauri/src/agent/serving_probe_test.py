"""Tests for serving_probe.py against real local HTTP servers: a fake vLLM (/v1/models,
/health, /metrics from models/fixtures/vllm_metrics.txt, completions) and a fake Ollama
(/api/tags, /api/ps, /api/generate), with the replies in each engine's wire format
(models/fixtures/probe/). Time is passed in, so a 45 s stall takes no 45 s.

Run: cd src-tauri/src/agent && python3 -I -m unittest discover -s . -p serving_probe_test.py
"""
import datetime
import http.server
import json
import os
import re
import sys
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import serving_probe as sp  # noqa: E402

FIXTURES = os.path.join(HERE, "..", "models", "fixtures")
NOW = 1_800_000_000_000


def fixture(name):
    with open(os.path.join(FIXTURES, name), "rb") as f:
        return f.read()


class Fake:
    """One fake engine: what it serves, and every request it saw (path, headers, body)."""

    def __init__(self, engine):
        self.engine = engine
        self.gen = 184211.0
        self.running = 3.0
        self.health = 200
        self.completion = 200
        self.chat_only = False
        self.ps_expires = {"gpt-oss:20b": 240, "qwen3:32b": 90}
        self.ps_empty = False
        self.stall = False
        self.seen = []
        self.clock_s = NOW / 1000
        handler = self._handler()
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def target(self, **extra):
        t = {"id": "cell1-%s-%d" % (self.engine, self.port), "name": "llama-3.3-70b", "engine": self.engine, "port": self.port,
             "model": "" if self.engine == "ollama" else "llama-3.3-70b", "headNode": "cell1", "host": "127.0.0.1"}
        t.update(extra)
        return t

    def posts(self, path=None):
        return [s for s in self.seen if s[0] == "POST" and (path is None or s[1] == path)]

    def _metrics(self):
        text = fixture("vllm_metrics.txt").decode()
        text = re.sub(r"^(vllm:generation_tokens_total\{[^}]*\}) [0-9.e+]+$", lambda m: "%s %.1f" % (m.group(1), self.gen), text, flags=re.M)
        text = re.sub(r"^(vllm:num_requests_running\{[^}]*\}) [0-9.e+]+$", lambda m: "%s %.1f" % (m.group(1), self.running), text, flags=re.M)
        return text.encode()

    def _ps(self):
        if self.ps_empty:
            return json.dumps({"models": []}).encode()
        text = fixture("probe/ollama_ps.json").decode()
        for name, tag in (("gpt-oss:20b", "EXPIRES_GPT"), ("qwen3:32b", "EXPIRES_QWEN")):
            at = datetime.datetime.fromtimestamp(self.clock_s + self.ps_expires[name], datetime.timezone(datetime.timedelta(hours=-4)))
            text = text.replace(tag, at.strftime("%Y-%m-%dT%H:%M:%S.123456789-04:00"))
        return text.encode()

    def _handler(self):
        fake = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def reply(self, code, body, kind="application/json"):
                self.send_response(code)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                fake.seen.append(("GET", self.path, dict(self.headers), None))
                if fake.engine == "ollama":
                    if self.path == "/api/tags":
                        return self.reply(200, fixture("probe/ollama_tags.json"))
                    if self.path == "/api/ps":
                        return self.reply(200, fake._ps())
                    return self.reply(404, b"404 page not found", "text/plain")
                if self.path == "/v1/models":
                    return self.reply(200, json.dumps({"object": "list", "data": [{"id": "llama-3.3-70b", "object": "model", "max_model_len": 32768}]}).encode())
                if self.path == "/health":
                    return self.reply(fake.health, b"")
                if self.path == "/metrics":
                    return self.reply(200, fake._metrics(), "text/plain; version=0.0.4")
                return self.reply(404, json.dumps({"detail": "Not Found"}).encode())

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
                fake.seen.append(("POST", self.path, dict(self.headers), body))
                if fake.stall:
                    time.sleep(1.5)  # a hung engine never answers (the probe gives up first)
                    return self.reply(504, b"{}")
                if self.path == "/api/generate":
                    return self.reply(200, fixture("probe/ollama_generate.json"))
                if self.path == "/v1/completions" and fake.chat_only:
                    return self.reply(404, json.dumps({"detail": "Not Found"}).encode())
                if self.path in ("/v1/completions", "/v1/chat/completions"):
                    if fake.completion != 200:
                        return self.reply(fake.completion, fixture("probe/engine_dead.json"))
                    fake.gen += 1
                    return self.reply(200, fixture("probe/chat_completion.json" if "chat" in self.path else "probe/completion.json"))
                return self.reply(404, b"{}")

        return H


class VllmProbe(unittest.TestCase):
    def setUp(self):
        self.fake = Fake("vllm")
        self.addCleanup(self.fake.close)

    def test_a_token_comes_back_and_it_is_ready(self):
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertTrue(r["ok"], r)
        self.assertEqual(r["health"], "ready")
        self.assertTrue(r["healthOk"])
        self.assertTrue(r["canary"]["ok"])
        self.assertIsInstance(r["canary"]["ms"], int)
        (_, path, _, body), = self.fake.posts()
        self.assertEqual(path, "/v1/completions")
        self.assertEqual(body, {"model": "llama-3.3-70b", "prompt": "Hi", "max_tokens": 1, "temperature": 0})
        self.assertEqual((r["running"], r["waiting"], r["kvPct"]), (3.0, 1.0, 21.78))
        # The next minute needs no second test; after it, one more.
        self.fake.gen += 40
        sp.probe(self.fake.target(), state, NOW + 15_000)
        self.assertEqual(len(self.fake.posts()), 1)

    def test_live_traffic_is_proof_and_skips_the_test(self):
        state = {}
        sp.probe(self.fake.target(), state, NOW)
        for i in range(1, 6):
            self.fake.gen += 25
            r = sp.probe(self.fake.target(), state, NOW + i * 15_000)
            self.assertTrue(r["ok"], r)
            # The first interval also holds the test's own token.
            self.assertAlmostEqual(r["genTps"], (25 + (i == 1)) / 15.0, places=3)
        self.assertEqual(len(self.fake.posts()), 1, "only the first round tested: tokens kept rising since")
        self.assertEqual(state["passiveOkMs"], NOW + 75_000)

    def test_a_failing_test_twice_is_down_with_the_engines_words(self):
        self.fake.completion = 500
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertEqual((r["health"], r["error"]), ("loading", "Checking that it generates…"))
        self.assertFalse(r["canary"]["ok"])
        r = sp.probe(self.fake.target(), state, NOW + 15_000)
        self.assertFalse(r["ok"])
        self.assertEqual(r["error"], "Didn't generate: HTTP 500: EngineCore encountered an issue. See stack trace (above) for the root cause. (2 tries)")
        # It recovers: the next test passes and it's Ready again.
        self.fake.completion = 200
        r = sp.probe(self.fake.target(), state, NOW + 30_000)
        self.assertTrue(r["ok"], r)

    def test_requests_running_with_no_tokens_is_stalled_after_45_s(self):
        timeout, sp.CANARY_TIMEOUT_S = sp.CANARY_TIMEOUT_S, 1
        self.addCleanup(setattr, sp, "CANARY_TIMEOUT_S", timeout)
        state = {}
        self.assertTrue(sp.probe(self.fake.target(), state, NOW)["ok"])
        # It hangs: 3 requests stay running, the counter stops (after the test's own token).
        self.fake.stall = True
        for s in (15, 30, 45):
            r = sp.probe(self.fake.target(), state, NOW + s * 1000)
            self.assertTrue(r["ok"], (s, r))
        r = sp.probe(self.fake.target(), state, NOW + 60_000)
        self.assertEqual((r["ok"], r["error"], r["stalledS"]), (False, "Stalled: 3 requests running, no tokens for 45 s", 45))

        # The rule's own wait, when agent.json carries it; the test token hangs meanwhile.
        self.fake.stall = False
        state = {}
        sp.probe(self.fake.target(stallAfterS=90), state, NOW)
        self.fake.stall = True
        for s in range(15, 105, 15):
            r = sp.probe(self.fake.target(stallAfterS=90), state, NOW + s * 1000)
            self.assertTrue(r["ok"] or s >= 90, (s, r))
        self.assertEqual(r["canary"], {"ok": False, "ms": None, "error": "no reply in 1 s"})
        r = sp.probe(self.fake.target(stallAfterS=90), state, NOW + 105_000)
        self.assertEqual(r["error"], "Stalled: 3 requests running, no tokens for 90 s")

        # Nothing running: a flat counter is just idle.
        self.fake.stall = False
        self.fake.running = 0
        state = {}
        for s in (0, 30, 60, 90):
            r = sp.probe(self.fake.target(), state, NOW + s * 1000)
        self.assertIsNone(r["stalledS"])
        self.assertTrue(r["ok"])

    def test_an_engine_that_died_behind_its_api_server_is_down(self):
        self.fake.health = 503
        state = {}
        sp.probe(self.fake.target(), state, NOW)
        r = sp.probe(self.fake.target(), state, NOW + 15_000)
        self.assertEqual(r["error"], "Engine not healthy: /health 503")

    def test_a_chat_only_server_is_tested_on_chat(self):
        self.fake.chat_only = True
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertTrue(r["ok"], r)
        self.assertEqual([p[1] for p in self.fake.posts()], ["/v1/completions", "/v1/chat/completions"])
        self.assertTrue(state["chatOnly"])
        sp.probe(self.fake.target(), state, NOW + 61_000)
        self.assertEqual(self.fake.posts()[-1][1], "/v1/chat/completions", "straight to chat next time")

    def test_the_key_goes_in_a_header_and_nowhere_else(self):
        state = {}
        r = sp.probe(self.fake.target(key="sbk-0123456789abcdef"), state, NOW)
        self.assertEqual(self.fake.posts()[0][2].get("Authorization"), "Bearer sbk-0123456789abcdef")
        self.assertNotIn("sbk-0123456789abcdef", json.dumps(r))
        self.assertNotIn("sbk-0123456789abcdef", json.dumps(state))

    def test_a_server_that_wants_a_key_is_ready_untested_and_asked_once_a_minute(self):
        state = {}
        calls = []

        def http(method, url, headers, body, timeout):
            calls.append((method, url))
            if url.endswith("/v1/completions"):
                return 401, b'{"error": "Unauthorized"}', 0.01
            return sp.http_request(method, url, headers, body, timeout)

        r = sp.probe(self.fake.target(), state, NOW, http=http)
        self.assertTrue(r["ok"], r)
        self.assertTrue(state["untestable"])
        sp.probe(self.fake.target(), state, NOW + 15_000, http=http)
        self.assertEqual(sum(1 for _, u in calls if u.endswith("/v1/completions")), 1, "not again within the minute")
        sp.probe(self.fake.target(), state, NOW + 61_000, http=http)
        self.assertEqual(sum(1 for _, u in calls if u.endswith("/v1/completions")), 2)

    def test_nothing_answering_is_down_after_two_probes(self):
        target = self.fake.target()
        self.fake.close()
        self.addCleanup(lambda: None)
        state = {}
        r = sp.probe(target, state, NOW)
        self.assertEqual(r["health"], "loading")
        r = sp.probe(target, state, NOW + 15_000)
        self.assertEqual((r["ok"], r["error"]), (False, "Not answering (connection refused)"))

    def test_rates_and_ttft_come_from_the_metrics(self):
        s = sp.parse_metrics(fixture("vllm_metrics.txt").decode())
        self.assertEqual(sp.first(s, sp.GEN), 184211.0)
        self.assertEqual(sp.first(s, sp.KV), 0.2178)
        p95 = sp.quantile(0.95, sp.buckets(s, sp.TTFT))
        self.assertTrue(0.25 <= p95 <= 10.0, p95)


class RecordedQwenMetrics(unittest.TestCase):
    def test_real_cell1_metrics_keep_live_and_idle_gauges_distinct(self):
        for name, generated, running, kv, first_tokens in [
                ("idle", 12306.0, 0.0, 0.0, 7.0),
                ("generating", 15860.0, 1.0, 0.010149013290374542, 10.0),
                ("after", 20514.0, 0.0, 0.0, 12.0)]:
            with self.subTest(name):
                samples = sp.parse_metrics(fixture("qwen3-06b-cell1-2026-11-07/" + name + ".prom").decode())
                self.assertEqual(sp.first(samples, sp.GEN), generated)
                self.assertEqual(sp.first(samples, sp.RUNNING), running)
                self.assertEqual(sp.first(samples, sp.WAITING), 0.0)
                self.assertEqual(sp.first(samples, sp.KV), kv)
                self.assertEqual(sp.first(samples, sp.PREEMPT), 0.0)
                self.assertEqual(sp.buckets(samples, sp.TTFT)[-1], (float("inf"), first_tokens))
                self.assertGreater(sp.quantile(0.95, sp.buckets(samples, sp.TTFT)), 0)
                self.assertNotIn("vllm:corrupted_requests_total", samples)
                self.assertFalse(any("spec_decode" in key for key in samples))


class OllamaProbe(unittest.TestCase):
    def setUp(self):
        self.fake = Fake("ollama")
        self.addCleanup(self.fake.close)

    def test_the_test_keeps_residency_as_it_is(self):
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertTrue(r["ok"], r)
        (_, path, _, body), = self.fake.posts()
        self.assertEqual(path, "/api/generate")
        # The model with the most time left, kept exactly that long more: residency doesn't change.
        self.assertEqual(body, {"model": "gpt-oss:20b", "prompt": "Hi", "stream": False, "options": {"num_predict": 1}, "keep_alive": "240s"})

    def test_a_model_about_to_unload_isnt_tested(self):
        self.fake.ps_expires = {"gpt-oss:20b": 20, "qwen3:32b": 10}
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertTrue(r["ok"], "nothing worth testing: Ready, idle")
        self.assertEqual(self.fake.posts(), [])
        self.assertEqual(sp.note(state), "Idle: no model in memory")

    def test_nothing_in_memory_is_ready_and_idle(self):
        self.fake.ps_empty = True
        state = {}
        r = sp.probe(self.fake.target(), state, NOW)
        self.assertEqual((r["ok"], r["health"]), (True, "ready"))
        self.assertEqual(self.fake.posts(), [])


if __name__ == "__main__":
    unittest.main()
