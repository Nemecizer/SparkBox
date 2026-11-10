import { describe, expect, it, vi } from "vitest";
import { agentTone, applyWatchState, clockWords, listWords, pushAgain, pushWords, testPageConfirm, watchBadge, watchLabel, type AgentStatus, type WatchState } from "./agent";
import { clearsSecretsNote, deliveryLine, outcomeLabel, type Channel, type Delivery } from "./notifyApi";
import type { Task } from "./opsApi";

// Admin mode is open (the unlock sheet says yes): the push goes straight to the backend. Every
// unlock goes through confirmUnlock (the sheet first); requestAdmin alone would skip it in Demo.
const unlock = vi.hoisted(() => ({ answer: true, asked: [] as string[], keys: [] as (string | undefined)[], silent: 0 }));
vi.mock("./admin", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./admin")>()),
  confirmUnlock: vi.fn(async (action: string, scope = "node", key?: string) => (unlock.asked.push(`${scope}: ${action}`), unlock.keys.push(key), unlock.answer)),
  requestAdmin: vi.fn(async () => (unlock.silent++, unlock.answer)),
  withAdmin: vi.fn((fn: () => Promise<unknown>) => fn()),
}));

const NOW = 1_800_000_000_000;
const NODES = ["cell1", "cell2", "cell3", "gx10-0000"];

function agent(node: string, p: Partial<AgentStatus> = {}): AgentStatus {
  return {
    node, name: node, installed: true, state: "running", version: "1.1.0", lastBeatMs: NOW - 2_000, historyFromMs: NOW - 14 * 86_400_000, firing: 0,
    lastDeliveryMs: null, lastDeliveryOk: null, uptimeS: 3600, metricsPort: null, error: null, simulated: false,
    configHash: "h", expectedHash: "h", configCurrent: true, pushError: null, ...p,
  };
}

/** A fully proven 24/7: four current agents, one phone channel, a test page 2 h ago, a dead-man's switch. */
function proven(p: Partial<WatchState> = {}): WatchState {
  return {
    mode: "agents",
    deliveryOwner: "agents",
    agents: NODES.map((n) => agent(n)),
    channels: [
      { id: "mac", name: "This Mac", kind: "This Mac", remote: false, macOnly: false, enabled: true, lastOkMs: null, lastError: null, lastErrorMs: null, simulated: false },
      { id: "phone", name: "Phone (ntfy)", kind: "ntfy", remote: true, macOnly: false, enabled: true, lastOkMs: NOW - 7_200_000, lastError: null, lastErrorMs: null, simulated: false },
    ],
    endToEnd: { testedAtMs: NOW - 7_200_000, ok: true, cells: NODES.map((n) => ({ node: n, channel: "phone", channelName: "Phone (ntfy)", ok: true, error: null, ms: 800, atMs: NOW - 7_200_000 })) },
    changedMs: NOW - 86_400_000,
    drift: [],
    pushFailures: [],
    deadman: { configured: true, service: "healthchecks.io", lastPingMs: NOW - 20_000, by: "cell1" },
    watched: { nodes: true, gpus: true, fabric: true, containers: true, endpoints: ["llama-3.3-70b", "Ollama"], switch: false, unwatched: ["CRS804 DDQ", "Firewalla"] },
    silences: 2,
    pagingFailure: null,
    ...p,
  };
}

describe("watchBadge", () => {
  it("1. no agent installed: this Mac only", () => {
    const b = watchBadge({ mode: "mac-only", deliveryOwner: "mac", agents: NODES.map((n) => agent(n, { installed: false, state: "not-installed" })) }, NOW);
    expect(b).toEqual({ label: "This Mac only", tone: "neutral", lines: ["Alerts reach you only while SparkBox runs on this Mac."], action: { label: "Set up…", kind: "settings" } });
    expect(watchLabel({ mode: "mac-only", deliveryOwner: "mac", agents: [] }, NOW)).toBeNull();
  });

  it("2. agents but no phone channel: not paging", () => {
    const b = watchBadge(proven({ channels: [proven().channels![0]] }), NOW);
    expect([b.label, b.tone]).toEqual(["Not paging", "warn"]);
    expect(b.lines).toEqual(["Agents are watching, but no phone channel is set up. Add ntfy, Pushover, a webhook or email."]);
    // Only the dead-man's switch is not a phone channel either.
    const hb = { ...proven().channels![1], id: "hc", name: "healthchecks", kind: "Dead-man's switch", remote: false };
    expect(watchBadge(proven({ channels: [hb] }), NOW).label).toBe("Not paging");
    // Every phone channel set to "this Mac only": the agents have none.
    const macOnly = proven({ channels: [{ ...proven().channels![1], macOnly: true }] });
    expect(watchBadge(macOnly, NOW).lines[0]).toBe("Agents are watching, but every phone channel sends from this Mac only.");
  });

  it("3. an agent isn't healthy: 24/7 on 3 of 4, naming it", () => {
    const agents = NODES.map((n) => (n === "cell2" ? agent(n, { lastBeatMs: NOW - 180_000, state: "failed" }) : agent(n)));
    const b = watchBadge(proven({ agents, mode: "partial" }), NOW);
    expect([b.label, b.tone]).toEqual(["24/7 on 3 of 4", "warn"]);
    expect(b.lines).toEqual(["Watched only while this Mac is awake: cell2 (agent not answering)."]);
    const stale = NODES.map((n) => (n === "gx10-0000" ? agent(n, { lastBeatMs: NOW - 180_000 }) : n === "cell3" ? agent(n, { installed: false, state: "not-installed" }) : agent(n)));
    expect(watchBadge(proven({ agents: stale }), NOW).lines[0]).toBe("Watched only while this Mac is awake: cell3 (no agent), gx10-0000 (no heartbeat for 3 min).");
  });

  it("4. settings not on a node: push again", () => {
    const at = NOW - 600_000;
    const agents = NODES.map((n) => (n === "cell3" ? agent(n, { configCurrent: false, pushError: { atMs: at, message: "sudo: a password is required", attempts: 2 } }) : agent(n)));
    const b = watchBadge(proven({ agents, drift: [{ node: "cell3", name: "cell3", reason: "push failed: sudo: a password is required" }], pushFailures: [{ node: "cell3", name: "cell3", atMs: at, message: "sudo: a password is required", attempts: 2 }] }), NOW);
    expect([b.label, b.tone, b.action]).toEqual(["24/7 · settings not on cell3", "warn", { label: "Push again", kind: "push" }]);
    expect(b.lines).toEqual([`cell3 has old alert settings (push failed ${clockWords(at)}: sudo refused). SparkBox pages for cell3 itself while this Mac is awake.`]);
    // An agent too old to report its settings drifts too.
    const old = proven({ drift: [{ node: "cell1", name: "cell1", reason: "agent 1.0.0 doesn't report its settings: reinstall it" }, { node: "cell2", name: "cell2", reason: "has older settings than this Mac" }] });
    const o = watchBadge(old, NOW);
    expect(o.label).toBe("24/7 · settings not on cell1 and cell2");
    expect(o.lines[0]).toBe("cell1 has old alert settings (agent 1.0.0 doesn't report its settings: reinstall it). SparkBox pages for cell1 itself while this Mac is awake.");
  });

  it("5. a delivery failed in the last hour, or a test page didn't arrive: paging failed", () => {
    const at = NOW - 20 * 60_000;
    const b = watchBadge(proven({ pagingFailure: { atMs: at, channel: "Phone (ntfy)", message: "HTTP 401 — check the token", by: "mac" } }), NOW);
    expect([b.label, b.tone]).toEqual(["24/7 · paging failed", "warn"]);
    expect(b.lines).toEqual([`Phone (ntfy) failed ${clockWords(at)}: HTTP 401 — check the token`]);
    expect(watchBadge(proven({ pagingFailure: { atMs: NOW - 2 * 3_600_000, channel: "Phone (ntfy)", message: "x", by: "mac" } }), NOW).label).toBe("24/7");
    const outbox = watchBadge(proven({ pagingFailure: { atMs: at, channel: "", message: "3 message(s) waiting in its outbox since 10:22", by: "cell2" } }), NOW);
    expect(outbox.lines).toEqual(["cell2: 3 message(s) waiting in its outbox since 10:22"]);
    const cells = proven().endToEnd!.cells.map((c) => (c.node === "cell2" ? { ...c, ok: false, error: "HTTP 401 — check the token" } : c));
    expect(watchBadge(proven({ endToEnd: { testedAtMs: NOW - 60_000, ok: false, cells } }), NOW).lines).toEqual(["The test page from cell2 didn't reach Phone (ntfy): HTTP 401 — check the token"]);
  });

  it("6. no test page since the settings changed: untested", () => {
    const b = watchBadge(proven({ endToEnd: { ...proven().endToEnd!, ok: false }, changedMs: NOW - 60_000 }), NOW);
    expect([b.label, b.tone, b.action]).toEqual(["24/7 · untested", "warn", { label: "Send a test page…", kind: "test" }]);
    expect(b.lines).toEqual(["The channels the agents send through changed after the last test page (2 h ago)."]);
    expect(watchBadge(proven({ endToEnd: { testedAtMs: null, ok: false, cells: [] } }), NOW).lines).toEqual(["No test page has gone out from every node yet."]);
    // Nothing known about tests (an older backend) isn't proof either.
    expect(watchBadge(proven({ endToEnd: undefined }), NOW).label).toBe("24/7 · untested");
  });

  it("7. proven: 24/7 with what it rests on", () => {
    const b = watchBadge(proven(), NOW);
    expect([b.label, b.tone, b.action]).toEqual(["24/7", "neutral", undefined]);
    expect(b.lines).toEqual([
      "4 agents on current settings · test page 2 h ago via Phone (ntfy)",
      "Watching: nodes, GPUs, fabric, containers, llama-3.3-70b, Ollama (one-token check every minute)",
      "Not watched while this Mac sleeps: CRS804 DDQ, Firewalla",
      "Dead-man's switch: healthchecks.io, pinged by cell1 20 s ago",
      "2 silences active",
    ]);
    // The leader checks the fabric switch: only the Firewallas wait for this Mac.
    const sw = watchBadge(proven({ watched: { ...proven().watched!, switch: true, unwatched: ["Firewalla"] } }), NOW);
    expect(sw.lines.slice(1, 3)).toEqual([
      "Watching: nodes, GPUs, fabric, the fabric switch, containers, llama-3.3-70b, Ollama (one-token check every minute)",
      "Not watched while this Mac sleeps: Firewalla",
    ]);
    expect(watchLabel(proven(), NOW)).toBe("24/7");
    expect(watchLabel(proven({ endToEnd: undefined }), NOW)).toBeNull();
  });

  it("8. proven but nobody watches the watchers: amber until a dead-man's switch is set up", () => {
    const bare = watchBadge(proven({ deadman: { configured: false, service: null, lastPingMs: null, by: null }, silences: 0, demo: true }), NOW);
    expect([bare.label, bare.tone, bare.action]).toEqual(["24/7 · no dead-man's switch", "warn", { label: "Set up…", kind: "settings" }]);
    expect(bare.lines[0]).toBe("If the power or the internet goes, or every agent stops at once, nothing pages you. A dead-man's switch (healthchecks.io, Uptime Kuma) pages when the agents' pings stop.");
    expect(bare.lines.at(-1)).toBe("Demo: the agents, their phone channel, the dead-man's switch and the test page are simulated.");
    expect(watchLabel(proven({ deadman: { configured: false, service: null, lastPingMs: null, by: null } }), NOW)).toBeNull();
  });

  it("a silence leaves 24/7 alone: the test page proves the channels, not the silences", () => {
    // The backend keeps endToEnd.ok while what each node runs is what was tested; the badge only lists the silences.
    const b = watchBadge(proven({ silences: 3, changedMs: NOW - 60_000 }), NOW);
    expect([b.label, b.tone]).toEqual(["24/7", "neutral"]);
    expect(b.lines).toContain("3 silences active");
  });

  it("checks in order: an unhealthy agent outranks drift, drift outranks a failed delivery", () => {
    const agents = NODES.map((n) => (n === "cell2" ? agent(n, { lastBeatMs: NOW - 120_000 }) : n === "cell3" ? agent(n, { configCurrent: false }) : agent(n)));
    const failing = { atMs: NOW - 60_000, channel: "Phone (ntfy)", message: "x", by: "mac" };
    expect(watchBadge(proven({ agents, pagingFailure: failing }), NOW).label).toBe("24/7 on 3 of 4");
    expect(watchBadge(proven({ agents: NODES.map((n) => (n === "cell3" ? agent(n, { configCurrent: false }) : agent(n))), drift: undefined, pagingFailure: failing }), NOW).label).toBe("24/7 · settings not on cell3");
  });
});

describe("words", () => {
  it("lists, push errors and the agent row's tone", () => {
    expect(listWords(["cell1", "cell2", "cell3", "gx10-0000"])).toBe("cell1, cell2, cell3 and gx10-0000");
    expect(pushWords("sudo: a password is required")).toBe("sudo refused");
    expect(pushWords("cell3 isn't connected.")).toBe("cell3 isn't connected.");
    expect(agentTone(agent("cell1"), NOW)).toBe("neutral");
    expect(agentTone(agent("cell3", { configCurrent: false }), NOW)).toBe("warn");
    expect(agentTone(agent("cell2", { lastBeatMs: NOW - 120_000 }), NOW)).toBe("warn");
    expect(agentTone(agent("cell2", { installed: false }), NOW)).toBe("neutral");
  });
});

describe("channel edits and delivery lines", () => {
  const ntfy: Channel = { id: "phone", kind: "ntfy", name: "Phone (ntfy)", enabled: true, criticalOnly: false, server: "https://ntfy.sh", topic: "a", secrets: { token: true }, last: null };

  it("says a changed destination clears the saved secret, unless a new one is typed", () => {
    expect(clearsSecretsNote(ntfy, { ...ntfy, server: "https://ntfy.example.com" }, {})).toBe("Changing the server clears the saved access token. Enter it again.");
    expect(clearsSecretsNote(ntfy, { ...ntfy, server: "https://ntfy.example.com" }, { token: "tk" })).toBeNull();
    expect(clearsSecretsNote(ntfy, { ...ntfy, topic: "b", name: "Phone" }, {})).toBeNull();
    expect(clearsSecretsNote(null, ntfy, {})).toBeNull();
    const mail: Channel = { id: "m", kind: "email", name: "Email", enabled: true, criticalOnly: false, host: "smtp.a.com", port: 587, user: "a", secrets: { password: true }, last: null };
    expect(clearsSecretsNote(mail, { ...mail, user: "b" }, {})).toBe("Changing the user clears the saved password. Enter it again.");
    // A webhook's URL is typed as a secret: a new host clears the signing secret, the same host doesn't.
    const hook: Channel = { id: "h", kind: "webhook", name: "Team", enabled: true, criticalOnly: false, urlHost: "hooks.slack.com", secrets: { url: true, secret: true }, last: null };
    expect(clearsSecretsNote(hook, hook, { url: "https://discord.com/api/webhooks/1/x/slack" })).toBe("Changing the URL clears the saved signing secret. Enter it again.");
    expect(clearsSecretsNote(hook, hook, { url: "https://hooks.slack.com/services/T0/B0/new" })).toBeNull();
    expect(clearsSecretsNote(hook, hook, { url: "https://discord.com/api/webhooks/1/x", secret: "s" })).toBeNull();
  });

  it("reads a delivery's outcome", () => {
    const d: Delivery = { id: "d", tsMs: 0, channel: "phone", channelName: "Phone (ntfy)", kind: "ntfy", title: "t", body: "", severity: "crit", ok: false, error: "HTTP 503", via: "mac", demo: false, test: false, attempts: 1, outcome: "failed", nextRetryMs: 30_000, maxAttempts: 4 };
    const ago = () => "2 min ago";
    expect(deliveryLine(d, ago)).toBe("Failed · retrying 2 of 4");
    expect(deliveryLine({ ...d, nextRetryMs: undefined, maxAttempts: undefined, attempts: 4 }, ago)).toBe("Failed: HTTP 503");
    expect(deliveryLine({ ...d, ok: true, outcome: "sent", error: null }, ago)).toBe("Sent 2 min ago");
    expect(outcomeLabel({ ...d, channel: "", outcome: "held", reason: "Quiet hours, 22:00 to 07:00 · America/New_York" })).toBe("Held · Quiet hours, 22:00 to 07:00 · America/New_York");
    expect(outcomeLabel({ ...d, outcome: "agent", via: "agent:cell1" })).toBe("Sent by the agent on cell1");
    // A round-two row without an outcome still reads right.
    expect(deliveryLine({ ...d, outcome: undefined, ok: true, demo: true }, ago)).toBe("Demo — not sent · 2 min ago");
  });
});

describe("one test-page confirmation", () => {
  it("counts what it sends", () => {
    expect(testPageConfirm(proven())).toEqual({
      title: "Send a test page from every node?",
      body: "Each node's agent sends one test through each phone channel, so you know a page reaches you while this Mac sleeps: 4 nodes × 1 channel = 4 messages.",
      confirmLabel: "Send test pages",
    });
  });
});

describe("pushAgain", () => {
  it("a double click starts one push and ends in one toast, never success then failure", async () => {
    const { mockCalls, mockTauri } = await import("../test/tauri");
    const { ingestTask } = await import("./tasks");
    const { getToasts } = await import("./toasts");
    mockTauri({ agent_push_config: () => ({ ...proven(), pushTask: "t-push" }), agent_status: () => proven() });
    const first = pushAgain();
    const second = pushAgain();
    expect(second).toBe(first);
    await vi.waitFor(() => expect(mockCalls.filter((c) => c.cmd === "agent_push_config")).toHaveLength(1));
    expect(getToasts(), "nothing until the push is over").toEqual([]);
    const task: Task = {
      id: "t-push", kind: "agent-config", title: "Push alert settings to 4 nodes", nodes: NODES, source: "ui", cancellable: false, state: "failed",
      startedMs: NOW, finishedMs: NOW + 4000, progress: null, detail: null, rateBps: null, etaS: null, steps: [], log: [], summary: null,
      error: "Pushed to 3 of 4 · Failed on cell3: sudo: a password is required", cancelRequested: false, retry: null, opId: "t-push",
    };
    ingestTask(task);
    await first;
    expect(getToasts().map((t) => [t.kind, t.title, t.body, t.key])).toEqual([["error", "Pushed to 3 of 4", "Failed on cell3: sudo: a password is required", "agent-push"]]);
    expect(mockCalls.filter((c) => c.cmd === "agent_push_config")).toHaveLength(1);
    // Done: the next click is a new push, whose result replaces this one.
    mockTauri({ agent_push_config: () => ({ ...proven(), pushTask: "t-push-2" }), agent_status: () => proven() });
    const third = pushAgain();
    expect(third).not.toBe(first);
    await vi.waitFor(() => expect(mockCalls.filter((c) => c.cmd === "agent_push_config")).toHaveLength(1));
    ingestTask({ ...task, id: "t-push-2", opId: "t-push-2", state: "ok", error: null, summary: "Pushed to 4 of 4" });
    await third;
    expect(getToasts().map((t) => [t.kind, t.title, t.key])).toEqual([["success", "Pushed to 4 of 4", "agent-push"]]);
  });

  it("locked, asks to unlock in words first (the in-app sheet before Touch ID), and Cancel pushes nothing", async () => {
    const { mockCalls, mockTauri } = await import("../test/tauri");
    const { getToasts, dismissToast } = await import("./toasts");
    getToasts().forEach((t) => dismissToast(t.id));
    mockTauri({ agent_push_config: () => ({ ...proven(), pushTask: "t-push-3" }), agent_status: () => proven() });
    unlock.asked.length = 0;
    unlock.keys.length = 0;
    unlock.silent = 0;
    unlock.answer = false;
    try {
      await pushAgain();
      // Live, with a channel secret on its way to cell3: the Mac scope up front, one prompt.
      applyWatchState({ ...proven(), secretCopies: [{ node: "cell3", name: "cell3", items: ["Phone (ntfy) token"] }] });
      await pushAgain(["cell3"]);
      await pushAgain(["cell1"]);
      // Demo copies nothing real: never the Mac scope.
      applyWatchState({ ...proven(), demo: true, secretCopies: [{ node: "cell3", name: "cell3", items: ["Phone (ntfy) token"] }] });
      await pushAgain();
    } finally {
      unlock.answer = true;
    }
    const words = "push the alert settings to the node agents";
    expect(unlock.asked).toEqual([`node: ${words}`, `mac: ${words}`, `node: ${words}`, `node: ${words}`]);
    expect(unlock.keys, "Touch ID names the push (admin.rs PROMPTS agent_push_config)").toEqual(Array(4).fill("agent_push_config"));
    expect(unlock.silent, "Push again never unlocks without the sheet (round 4 did, with Demo data)").toBe(0);
    expect(mockCalls.filter((c) => c.cmd === "agent_push_config")).toHaveLength(0);
    expect(getToasts()).toEqual([]);
  });
});

describe("Settings › Notifications", () => {
  it("says who can read a channel's secrets on the nodes, and new channels stay on this Mac", async () => {
    const { render, screen, within } = await import("@testing-library/react");
    const { mockTauri } = await import("../test/tauri");
    const { handlers } = await import("../dev/mocks/paging");
    const { NotificationsSection } = await import("../windows/settings/NotificationsSection");
    const { SECRET_DISCLOSURE, SCOPED_CREDENTIALS } = await import("./agent");
    mockTauri({ ...handlers, admin_state: () => ({ unlocked: false, lookOnly: true }) });
    const { createElement } = await import("react");
    render(createElement(NotificationsSection));
    const note = await screen.findByRole("note");
    expect(note).toHaveTextContent("New channels send from this Mac only.");
    expect(note).toHaveTextContent(SECRET_DISCLOSURE.slice(1));
    expect(note).toHaveTextContent(SCOPED_CREDENTIALS);
    // Demo's dead-man's switch is a row of the channel list, marked Demo, and the checklist reads it.
    const row = (await screen.findByText("Dead-man's switch (Demo)")).closest(".ntf-row") as HTMLElement;
    expect(within(row).getByText(/Pinged by the node agents · last ping .* · Demo — never saved, nothing is pinged/)).toBeInTheDocument();
    expect(await screen.findByText(/^Dead-man's switch · healthchecks\.io · pinged by cell1/)).toBeInTheDocument();
  });
});
