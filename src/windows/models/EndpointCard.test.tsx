import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { _resetAdminForTests, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP } from "../../lib/admin";
import type { AdminState } from "../../lib/opsApi";
import * as session from "../../lib/session";
import { endpointView, fmtLatency, noteRecoveries, proofLine, protocolsLine, readinessLine, type Endpoint, type EndpointMetrics } from "../../lib/endpoints";
import { getToasts, toast } from "../../lib/toasts";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { EndpointCard } from "./EndpointCard";
import { recoveryFor, restartEndpoint } from "./endpointActions";
import { curlFor } from "./shared";
import { endpointCommands } from "../../lib/modelCommands";

const metrics: EndpointMetrics = {
  genTps: 5.31, promptTps: 41.2, running: 3, waiting: 0, kvPct: 24.5, kvKnown: true,
  ttftP50Ms: 412, ttftP95Ms: 951, itlP50Ms: 607.5, e2eP50Ms: null, e2eP95Ms: null, reqPerS: 0.02, prefixHitPct: 46, updatedMs: 0,
  history: { ts: [1, 2, 3], genTps: [4, 5, 6], running: [2, 3, 3], kvPct: [20, 22, 24], promptTps: [0, 40, 41], waiting: [0, 0, 0], ttftP50Ms: [400, 420, 410] },
};

function llama(over: Partial<Endpoint> = {}): Endpoint {
  return {
    id: "cell1-vllm-8000", engine: "vllm", models: ["llama-3.3-70b"], nodes: ["cell1", "cell2", "cell3"], headNode: "cell1", port: 8000,
    url: "http://192.168.50.51:8000/v1", managedBy: "external", health: "ready", parallel: { tp: 1, pp: 3 }, containerIds: ["3f9a1c27b0de"],
    containerNames: ["vllm-head"], expectedNodes: ["cell1", "cell2", "cell3"], missingNodes: [], metrics, providerId: "cell1-vllm-8000",
    ...over,
  };
}

const down = llama({ health: "down", healthReason: "Pipeline stage 3 of 3 on cell3 is offline", message: "Pipeline stage 3 of 3 on cell3 is offline", missingNodes: ["cell3"] });

function show(e: Endpoint, variant: "full" | "compact") {
  return render(
    <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}>
      <EndpointCard endpoint={e} variant={variant} />
    </WindowManagerProvider>,
  );
}

describe("endpoint state", () => {
  it("maps each health to its word, tone and reason", () => {
    expect(endpointView(llama())).toEqual({ word: "Ready", tone: "ok", reason: null, pct: null, stale: false });
    expect(endpointView(llama({ health: "loading", loadingPct: 41.6, healthReason: "Loading weights" }))).toMatchObject({ word: "Loading 42%", tone: "progress", pct: 42 });
    expect(endpointView(llama({ health: "loading", healthReason: "Checking that it answers…" }))).toMatchObject({ word: "Loading", pct: null, reason: "Checking that it answers…" });
    expect(endpointView(down)).toMatchObject({ word: "Down", tone: "crit", reason: "Pipeline stage 3 of 3 on cell3 is offline", stale: true });
    expect(endpointView(llama({ health: "stopped", healthReason: "Stopped 3 min ago" }))).toMatchObject({ word: "Stopped", tone: "off" });
    // An older backend's message still explains it.
    expect(endpointView(llama({ health: "down", message: "Not answering (connection refused)" })).reason).toBe("Not answering (connection refused)");
  });

  it("quotes a histogram bucket as a range, never a made-up precise number", () => {
    expect(fmtLatency(412.4)).toBe("412 ms");
    expect(fmtLatency(1840)).toBe("1.8 s");
    expect(fmtLatency(375, [250, 500])).toBe("250–500 ms");
    expect(fmtLatency(1200, [1000, 2500])).toBe("1–2.5 s");
    expect(fmtLatency(null)).toBe("—");
  });
});

describe("EndpointCard", () => {
  it("shows the same six numbers for every engine, with — where one doesn't apply", () => {
    show(llama(), "full");
    const card = screen.getByRole("article");
    for (const label of ["Generate", "Prompt", "Running · waiting", "KV cache", "TTFT p50 · p95", "ITL p50"]) expect(within(card).getByText(label)).toBeInTheDocument();
    expect(card).toHaveTextContent("5.31 tok/s");
    expect(card).toHaveTextContent("412 ms");
    expect(card).toHaveTextContent("p95 951 ms");
    expect(card).toHaveTextContent("Ready");
  });

  it("gives Ollama the same cells, with KV cache —", () => {
    show(llama({ id: "gx10-0000-ollama-11434", engine: "ollama", nodes: ["gx10-0000"], parallel: null, metrics: { ...metrics, kvKnown: false } }), "full");
    const kv = screen.getByText("KV cache").closest(".ai-cell") as HTMLElement;
    expect(kv).toHaveTextContent("KV cache—");
    expect(screen.getByText("ITL p50")).toBeInTheDocument();
  });

  it("says why it's down, in red, flags the missing node and shows no numbers", () => {
    show(down, "compact");
    const card = screen.getByRole("button", { name: /llama-3.3-70b, Down: Pipeline stage 3 of 3 on cell3 is offline/ });
    expect(card).toHaveClass("ai-card-crit");
    expect(within(card).getByText("Pipeline stage 3 of 3 on cell3 is offline")).toHaveClass("crit");
    expect(within(card).getByText("cell3")).toHaveClass("missing");
    expect(card).not.toHaveTextContent("5.31");
    expect(within(card).getByText("Generate").closest(".ai-cell")).toHaveTextContent("—");
  });

  it("says when Keep running won't restart a load that failed to start", () => {
    const failed = llama({ health: "down", healthReason: "vLLM didn't become ready within 90 minutes.", supervisor: { enabled: true, state: "watching", restarts: 0, nextRetryMs: null, held: "It failed to start: won't restart it" } });
    const { container } = show(failed, "full");
    expect(container.querySelector(".ai-card-sup")?.textContent).toBe("Keep running · It failed to start: won't restart it");
  });

  it("keeps a reason that says more than the pill, and only a bare Loading goes", () => {
    const quiet = "Loading weights 40% · no new progress for 21 min (limit 90 min)";
    for (const variant of ["full", "compact"] as const) {
      const { container, unmount } = show(llama({ health: "loading", loadingPct: 40, healthReason: quiet }), variant);
      expect(container.querySelector(".ai-card-reason")).toHaveTextContent(quiet);
      unmount();
      for (const [loadingPct, healthReason] of [[null, "Loading"], [42, "Loading 42%"]] as const) {
        const bare = show(llama({ health: "loading", loadingPct, healthReason }), variant);
        expect(bare.container.querySelector(".ai-card-reason"), healthReason).toBeNull();
        bare.unmount();
      }
    }
  });

  it("offers the palette's model verbs for what each endpoint can do", () => {
    const titles = (e: Endpoint) => endpointCommands(e).map((c) => c.title);
    expect(titles(llama())).toEqual(["Open endpoint llama-3.3-70b", "Chat with llama-3.3-70b", "Benchmark llama-3.3-70b", "Restart llama-3.3-70b…"]);
    expect(titles(llama({ managedBy: "sparkbox", deploymentId: "m1" }))).toContain("Unload llama-3.3-70b");
    const chat = endpointCommands(down).find((c) => c.title.startsWith("Chat with"))!;
    expect(chat.when?.()).toBe("llama-3.3-70b isn't ready");
    expect(endpointCommands(llama())[0].subtitle).toBe("vLLM on cell1, cell2, cell3");
    // Ollama is one server with many models: the server is restarted, each model is chatted with.
    const ollama = llama({ id: "gx10-0000-ollama-11434", engine: "ollama", nodes: ["gx10-0000"], headNode: "gx10-0000", models: ["gpt-oss:20b"], parallel: null,
      ollama: [{ name: "gpt-oss:20b" }, { name: "qwen3:32b" }] as Endpoint["ollama"] });
    expect(titles(ollama)).toEqual(["Open endpoint Ollama on gx10-0000", "Chat with gpt-oss:20b", "Chat with qwen3:32b", "Restart Ollama on gx10-0000…"]);
  });

  it("says how it proved it generates: a test token, or live traffic", () => {
    const now = 1_800_000_000_000;
    expect(proofLine(llama({ canary: { atMs: now - 12_000, ok: true, latencyMs: 84 } }), now)?.text).toBe("Answered 12 s ago · 1 token in 84 ms");
    expect(proofLine(llama({ canary: { atMs: now - 70_000, ok: true, latencyMs: 612 } }), now)?.text).toBe("Answered 1 min ago · 1 token in 612 ms");
    expect(proofLine(llama({ passiveOkMs: now - 4_000, canary: { atMs: now - 100_000, ok: true, latencyMs: 600 } }), now)?.text).toBe("Generating · live traffic");
    expect(proofLine(llama({ passiveOkMs: now - 25_000 }), now)?.text).toBe("Generated 25 s ago · live traffic");
    // Tokens a moment ago but nothing running now: it generated, it isn't generating.
    expect(proofLine(llama({ passiveOkMs: now - 8_000, metrics: { ...metrics, running: 0 } }), now)?.text).toBe("Generated 8 s ago · live traffic");
    expect(proofLine(llama({ healthNote: "Slow: 1 token took 12.4 s" }), now)).toMatchObject({ text: "Slow: 1 token took 12.4 s", note: true, title: "Ready, with a note: it answered and generated within the last two minutes" });
    // Busy SSH channels and held test tokens don't claim it generated.
    expect(proofLine(llama({ healthNote: "Not checked since 14:02: SSH channels busy on cell1" }), now)?.title).toBe("Ready when last checked: no SSH channel on its node was free to check it since");
    for (const healthNote of ["Not tested since 14:02: SSH channels busy on cell1", "Not tested: look-only mode holds test tokens"]) {
      expect(proofLine(llama({ healthNote }), now)?.title).toBe("Ready: it answered within the last minute; the note says why no recent test token shows it generates");
    }
    expect(proofLine(llama({ stalledS: 20 }), now)?.text).toBe("No tokens for 20 s · 3 requests running");
    expect(proofLine(llama({ stalledS: 20, metrics: { ...metrics, running: 0, waiting: 2 } }), now)?.text).toBe("No tokens for 20 s · 2 requests waiting");
    expect(proofLine(down, now)).toBeNull();
    show(llama({ canary: { atMs: Date.now() - 2_000, ok: true, latencyMs: 84 } }), "full");
    expect(screen.getByRole("article")).toHaveTextContent("Answered just now · 1 token in 84 ms");
    expect(screen.getByRole("button", { name: "History" })).toBeInTheDocument();
  });

  it("replaces a stale failure toast when the model answers again", () => {
    // Only the toast keyed to this endpoint: one that merely mentions the model is about something else.
    const other = toast({ kind: "error", title: "Couldn't copy llama-3.3-70b's folder", body: "llama-3.3-70b: no space left" });
    const id = toast({ kind: "error", key: "endpoint:cell1-vllm-8000", title: "Restarted, but llama-3.3-70b didn't come back", body: "llama-3.3-70b is down: No answer for 90 s" });
    noteRecoveries([down], [llama({ canary: { atMs: Date.now(), ok: true, latencyMs: 640 } })]);
    const t = getToasts().find((x) => x.id === id)!;
    expect(t).toMatchObject({ kind: "success", title: "llama-3.3-70b is answering again", key: "endpoint:cell1-vllm-8000" });
    expect(t.body).toMatch(/^Answered just now · 1 token in 640 ms$/);
    expect(getToasts().find((x) => x.id === other)).toMatchObject({ kind: "error", title: "Couldn't copy llama-3.3-70b's folder" });
    // Nothing to replace: no new toast for a recovery nobody was told about.
    const before = getToasts().length;
    noteRecoveries([llama({ id: "x", health: "down" })], [llama({ id: "x", models: ["qwen3"] })]);
    expect(getToasts().length).toBe(before);
  });

  it("says what a SparkBox load proved when it became ready, and which native APIs it serves", () => {
    const summary = "Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (first run of this recipe and image)";
    const gemma = llama({
      id: "cell2-vllm-8000", models: ["gemma-3-27b-it"], nodes: ["cell2"], headNode: "cell2", parallel: null, managedBy: "sparkbox", deploymentId: "d1",
      source: "deployment", image: "vllm/vllm-openai:glm53-flash-arm64-cu130",
      readiness: { health: true, modelsListed: true, token: true, sanity: { ok: true, answer: "42", atMs: 1 }, ok: true, why: null, atMs: 1, tokenMs: 412, summary },
      protocols: { messages: true, countTokens: true, responses: false, checkedMs: 2 },
    });
    expect(readinessLine(gemma)).toBe(summary);
    expect(protocolsLine(gemma)).toBe("Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗");
    show(gemma, "full");
    const card = screen.getByRole("article");
    expect(within(card).getByText(summary)).toHaveClass("ai-card-proof");
    expect(within(card).getByText("Native API (from /openapi.json): Messages ✓ · count_tokens ✓ · Responses ✗")).toBeInTheDocument();
  });

  it("shows no readiness or protocol line before a load proves it answers, nor for servers SparkBox didn't start", () => {
    const checking = llama({ managedBy: "sparkbox", health: "loading", healthReason: "Up; checking it answers… (Look-only mode holds the test token; it's checked once look-only ends.)",
      readiness: { health: true, modelsListed: true, token: null, ok: false, why: "Look-only mode holds the test token; it's checked once look-only ends.", atMs: 1, summary: "" } });
    expect(readinessLine(checking)).toBeNull();
    expect(protocolsLine(checking)).toBeNull();
    show(checking, "full");
    expect(screen.getByRole("article")).not.toHaveTextContent("Native API");
    expect(screen.getByRole("article")).toHaveTextContent("Up; checking it answers…");
    // The Ollamas and other external servers: no lines, and no probe of /openapi.json.
    expect(readinessLine(llama())).toBeNull();
    expect(protocolsLine(llama())).toBeNull();
  });

  it("drops the readiness line once the load is down or restarting", () => {
    const summary = "Ready: /health 200, gemma-3-27b-it listed, a test token in 0.4 s, 17+25 → 42 (first run of this recipe and image)";
    const readiness = { health: true, modelsListed: true, token: true, sanity: { ok: true, answer: "42", atMs: 1 }, ok: true, why: null, atMs: 1, tokenMs: 412, summary };
    for (const [health, reason] of [["down", "Head exited (code 1) just now"], ["loading", "Restarting…"]] as const) {
      const gemma = llama({ id: "cell2-vllm-8000", models: ["gemma-3-27b-it"], nodes: ["cell2"], headNode: "cell2", parallel: null, managedBy: "sparkbox", deploymentId: "d1",
        source: "deployment", health, healthReason: reason, readiness });
      expect(readinessLine(gemma)).toBeNull();
      const { unmount } = show(gemma, "full");
      const card = screen.getByRole("article");
      expect(card).toHaveTextContent(reason);
      expect(card).not.toHaveTextContent("Ready: /health 200");
      unmount();
    }
  });
});

describe("Copy curl on a server that listens on 127.0.0.1", () => {
  const local = llama({ id: "cell3-ollama-11434", engine: "ollama", url: "http://127.0.0.1:11434/v1", nodes: ["cell3"], headNode: "cell3", parallel: null, listen: "local", containerIds: [], containerNames: [] });
  const NOTE = "Run this on cell3: it listens on 127.0.0.1 there.";
  afterEach(() => vi.unstubAllGlobals());

  it("says where to run it, in its first line and in the toast", async () => {
    expect(curlFor(local).split("\n")[0]).toBe(`# ${NOTE}`);
    expect(curlFor(local).split("\n")[1]).toBe("curl http://127.0.0.1:11434/v1/chat/completions \\");
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    show(local, "full");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy curl" }));
    });
    expect(writeText.mock.calls[0][0]).toBe(curlFor(local));
    expect(getToasts().at(-1)).toMatchObject({ kind: "success", title: "curl command copied", body: NOTE });
  });

  it("says nothing for a server this Mac can reach", async () => {
    expect(curlFor(llama({ listen: "all" }))).not.toContain("#");
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    show(llama({ listen: "all" }), "full");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy curl" }));
    });
    expect(getToasts().at(-1)?.body).toBeUndefined();
  });
});

describe("Unload…, Restart…, Stop containers… and Forget under the lock rule", () => {
  const LOCKED: AdminState = { unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
  const gemma = llama({ id: "cell2-vllm-8000", models: ["gemma-3-27b-it"], nodes: ["cell2"], headNode: "cell2", parallel: null, managedBy: "sparkbox", deploymentId: "d1", source: "deployment" });
  const gone = llama({ health: "down", healthReason: "Head exited (code 1) 2 min ago", forgettable: true });
  // Every change an endpoint's card offers, with what its tooltip says when nothing's in the way.
  const CHANGES: [string, Endpoint, string][] = [
    ["Restart…", llama(), "Stop and start it in order, then check it answers"],
    ["Restart…", gemma, "Stop and start it, then check it answers"],
    ["Unload…", gemma, "Stop its containers on every node"],
    ["Stop containers…", llama(), "Stop the containers behind it"],
    ["Forget", gone, "Stop listing it here (it's listed again if it starts)"],
  ];
  afterEach(() => {
    _resetAdminForTests();
    vi.restoreAllMocks();
  });

  it.each(CHANGES)("%s on the card: off in look-only, a lock and a click to unlock when locked", (name, e, tip) => {
    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    const off = show(e, "full");
    const held = screen.getByRole("button", { name });
    expect(held).toBeDisabled();
    expect(held).toHaveAttribute("title", LOOK_ONLY_TIP);
    expect(held.querySelector(".tool-lock")).not.toBeNull();
    off.unmount();

    _resetAdminForTests(LOCKED);
    const on = show(e, "full");
    const locked = screen.getByRole("button", { name });
    expect(locked).toBeEnabled();
    expect(locked).toHaveAttribute("title", `${tip} · ${NEEDS_ADMIN_TIP}`);
    expect(locked.querySelector(".tool-lock")).not.toBeNull();
    on.unmount();

    _resetAdminForTests({ ...LOCKED, unlocked: true });
    show(e, "full");
    const open = screen.getByRole("button", { name });
    expect(open).toHaveAttribute("title", tip);
    expect(open.querySelector(".tool-lock")).toBeNull();
  });

  it.each(CHANGES)("%s in the context menu: off in look-only, a lock and a click to unlock when locked", async (name, e) => {
    const openMenu = () => {
      const r = render(
        <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}>
          <EndpointCard endpoint={e} variant="full" />
          <ContextMenuHost />
        </WindowManagerProvider>,
      );
      fireEvent.contextMenu(screen.getByRole("article"));
      return r;
    };
    const row = () => screen.getByRole("menuitem", { name: new RegExp(`^${name}`) });

    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    const off = openMenu();
    expect(row()).toHaveAttribute("aria-disabled", "true");
    expect(row()).toHaveAttribute("title", LOOK_ONLY_TIP);
    fireEvent.click(row());
    expect(screen.queryByRole("dialog")).toBeNull();
    off.unmount();

    _resetAdminForTests(LOCKED);
    const asked = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const on = openMenu();
    expect(row()).toHaveAttribute("title", NEEDS_ADMIN_TIP);
    expect(row().querySelector(".ctx-lock")).not.toBeNull();
    await act(async () => {
      fireEvent.click(row());
    });
    expect(asked.mock.calls[0][0], "the unlock sheet first").toMatchObject({ title: "Unlock Admin mode?" });
    on.unmount();

    _resetAdminForTests({ ...LOCKED, unlocked: true });
    openMenu();
    expect(row().querySelector(".ctx-lock")).toBeNull();
    expect(row()).not.toHaveAttribute("aria-disabled", "true");
  });

  it("leaves what changes nothing alone", () => {
    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    render(
      <WindowManagerProvider sizeOf={() => [600, 400]} isKnown={() => true}>
        <EndpointCard endpoint={llama()} variant="full" />
        <ContextMenuHost />
      </WindowManagerProvider>,
    );
    fireEvent.contextMenu(screen.getByRole("article"));
    for (const name of ["Copy URL", "Copy curl", "History"]) expect(screen.getByRole("menuitem", { name })).not.toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Copy curl" })).toBeEnabled();
  });

  it("is off in look-only, with look-only's words, and asks to unlock when locked", () => {
    _resetAdminForTests({ ...LOCKED, lookOnly: true });
    const { unmount } = show(gemma, "full");
    const unload = screen.getByRole("button", { name: "Unload…" });
    expect(unload).toBeDisabled();
    expect(unload).toHaveAttribute("title", LOOK_ONLY_TIP);
    fireEvent.click(unload);
    expect(screen.queryByRole("dialog")).toBeNull();
    unmount();

    _resetAdminForTests(LOCKED);
    show(gemma, "full");
    const locked = screen.getByRole("button", { name: "Unload…" });
    expect(locked).toBeEnabled();
    expect(locked).toHaveAttribute("title", "Stop its containers on every node · needs Admin (click to unlock)");
  });
});

describe("Restart… says its order", () => {
  const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
  const three = { nodes: ["cell1", "cell2", "cell3"], headNode: "cell1" };
  afterEach(() => {
    _resetAdminForTests();
    vi.restoreAllMocks();
  });

  async function asked(e: Endpoint) {
    _resetAdminForTests(UNLOCKED);
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    const choice = vi.spyOn(session, "confirmChoice").mockResolvedValue(null);
    await restartEndpoint(e);
    return choice.mock.calls[0]?.[0] ?? confirm.mock.calls[0][0];
  }

  it("a SparkBox load stops its head first and starts it last", async () => {
    const q = await asked(llama({ ...three, managedBy: "sparkbox", deploymentId: "m1", source: "deployment" }));
    expect(q.body).toContain("In order: the head on cell1 stops, then the workers on cell2, cell3. The workers start, then the head.");
    expect(q.body).toContain("It loads again with the same settings and key.");
    expect(q.confirmLabel).toBe("Restart llama-3.3-70b in order");
  });

  it("a server SparkBox didn't start keeps its steps", async () => {
    const q = await asked(llama(three));
    expect(q.body).toContain("In order: the workers on cell2, cell3 stop, the head on cell1 stops and starts, then the workers start and join it.");
    expect(q.confirmLabel).toBe("Restart llama-3.3-70b in order");
  });

  it("a load on one node has no order to give", async () => {
    const q = await asked(llama({ nodes: ["cell2"], headNode: "cell2", managedBy: "sparkbox", deploymentId: "d1", source: "deployment" }));
    expect(q.body).toContain("Its container on cell2 stops and starts.");
    expect(q.confirmLabel).toBe("Restart");
  });

  it("the recovery on a \"didn't come back\" warning says the same", () => {
    const e = llama({ ...three, managedBy: "sparkbox", deploymentId: "m1" });
    expect(recoveryFor("llama-3.3-70b didn't come back", [e])?.label).toBe("Restart llama-3.3-70b in order…");
  });
});

it("shows unavailable ranks neutrally with last-known proof, never live metrics", () => {
  const e = llama({ health: "unknown", healthReason: "cell3 is offline", missingNodes: ["cell3"], expectedNodes: ["cell1", "cell2", "cell3"], lastOkMs: 1_800_000_000_000 });
  show(e, "full");
  expect(screen.getByText("Unknown")).toBeInTheDocument();
  expect(screen.getByText(/Last successful check/)).toBeInTheDocument();
  expect(screen.getByTitle(/cell3:.*status unknown/)).not.toHaveClass("missing");
  expect(screen.queryByText("5.31 tok/s")).toBeNull();
});
