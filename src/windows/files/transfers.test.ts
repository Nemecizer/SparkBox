import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import macRs from "../../../src-tauri/src/files/mac.rs?raw";
import { _resetAdminForTests } from "../../lib/admin";
import type { Preflight, TransferSpec } from "../../lib/filesApi";
import type { ConfirmOptions } from "../../lib/session";
import { dismissToast, getToasts } from "../../lib/toasts";
import { channelSend, mockCalls, mockTauri } from "../../test/tauri";
import {
  _resetTransfersForTests, canRetry, decideConflicts, enqueueTransfer, getTransfers, idleSummary, isNodeSecret, NODE_SECRETS, SECRET_DIRS, SECRET_REFUSED,
  startGatedTransfer, stopTransfer, transferGate, transferGuardReason, transferLocked, type Transfer,
} from "../../lib/transfers";

const asked: ConfirmOptions[] = [];
vi.mock("../../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/session")>()),
  confirmAction: vi.fn(async (o: ConfirmOptions) => (asked.push(o), true)),
}));

const GiB = 1024 ** 3;
const flush = () => new Promise((r) => setTimeout(r, 0));
const spec = (names: string[], extra: Partial<TransferSpec> = {}): TransferSpec => ({
  from: "mac", dir: "/Users/me/models", names, to: "cell1", toDir: "/home/sparkuser/models", op: "copy", policies: {}, ...extra,
});

let channels: unknown[] = [];
let preflight: Preflight = { bytes: 4 * GiB, entries: 12, conflicts: [], free: 2000 * GiB, route: "direct" };

beforeEach(() => {
  _resetTransfersForTests();
  channels = [];
  preflight = { bytes: 4 * GiB, entries: 12, conflicts: [], free: 2000 * GiB, route: "direct" };
  mockTauri({
    files_preflight: () => preflight,
    files_transfer: (a) => {
      channels.push(a.onEvent);
      return `x${channels.length}`;
    },
    files_cancel: () => null,
  });
});
afterEach(() => _resetTransfersForTests());

const names = { fromName: "This Mac", toName: "cell1" };

describe("transfer queue", () => {
  it("runs at most two at once per route and starts the next when one lands", async () => {
    enqueueTransfer(spec(["a"]), names);
    enqueueTransfer(spec(["b"]), names);
    enqueueTransfer(spec(["c"]), names);
    enqueueTransfer(spec(["d"], { to: "cell2" }), { ...names, toName: "cell2" });
    await flush();
    await flush();
    const started = () => mockCalls.filter((c) => c.cmd === "files_transfer").map((c) => (c.args.spec as TransferSpec).names[0]);
    expect(started().sort()).toEqual(["a", "b", "d"]);
    expect(getTransfers().find((t) => t.spec.names[0] === "c")?.state).toBe("queued");
    channelSend(channels[0], { kind: "done", bytes: 100, files: 1, placed: [{ name: "a", action: "copied", as: "a" }] });
    await flush();
    await flush();
    expect(started()).toContain("c");
    expect(getTransfers().find((t) => t.spec.names[0] === "a")?.message).toMatch(/^Copied a/);
  });

  it("waits for a decision when names are taken, then sends the choices", async () => {
    preflight = {
      ...preflight,
      conflicts: [{ name: "config.json", source: { kind: "file", bytes: 900, entries: 1, mtime: 200, tarBytes: 1536 }, dest: { kind: "file", bytes: 880, entries: 1, mtime: 100, tarBytes: 1536 } }],
    };
    const id = enqueueTransfer(spec(["config.json"]), names);
    await flush();
    expect(getTransfers()[0].state).toBe("needsDecision");
    expect(mockCalls.some((c) => c.cmd === "files_transfer")).toBe(false);
    decideConflicts(id, { "config.json": "replaceIfNewer" });
    await flush();
    const sent = mockCalls.find((c) => c.cmd === "files_transfer")!.args.spec as TransferSpec;
    expect(sent.policies).toEqual({ "config.json": "replaceIfNewer" });
  });

  it("refuses up front when the destination is too full", async () => {
    preflight = { ...preflight, bytes: 141 * GiB, free: 80 * GiB };
    enqueueTransfer(spec(["Llama-3.3-70B-Instruct"]), names);
    await flush();
    const t = getTransfers()[0];
    expect(t.state).toBe("failed");
    expect(t.message).toBe("Not enough space on cell1: this needs 141 GiB and 80.0 GiB is free.");
  });

  it("reports progress with a smoothed speed and an ETA, and explains itself for the close guard", async () => {
    enqueueTransfer(spec(["big"]), names);
    await flush();
    await flush();
    const ch = channels[0];
    channelSend(ch, { kind: "progress", done: 0, total: 4 * GiB, files: 0, filesTotal: 12, current: null });
    await new Promise((r) => setTimeout(r, 220));
    channelSend(ch, { kind: "progress", done: 1 * GiB, total: 4 * GiB, files: 3, filesTotal: 12, current: "big/model-00001-of-00004.safetensors" });
    await flush();
    const t = getTransfers()[0];
    expect(t.rateBps).toBeGreaterThan(1 * GiB);
    expect(t.etaS).toBeGreaterThan(0);
    expect(t.current).toBe("big/model-00001-of-00004.safetensors");
    expect(transferGuardReason()).toBe("1 transfer in progress (1.0 GiB of 4.0 GiB)");
    stopTransfer(t.id);
    expect(mockCalls.some((c) => c.cmd === "files_cancel")).toBe(true);
    channelSend(ch, { kind: "cancelled" });
    await flush();
    expect(getTransfers()[0].state).toBe("cancelled");
    expect(transferGuardReason()).toBeNull();
  });
});

describe("which scope a transfer needs", () => {
  const spec = (over: Partial<TransferSpec>): TransferSpec => ({ from: "cell1", dir: "/home/sparkuser", names: ["run.sh"], to: "mac", toDir: "~/Documents", op: "copy", policies: {}, ...over });
  it("anything landing on this Mac needs Mac scope, except a new copy into ~/Downloads", async () => {
    const { transferNeeds, isDownloads } = await import("../../lib/transfers");
    expect(transferNeeds(spec({}))).toEqual(["mac"]);
    expect(transferNeeds(spec({ toDir: "~/Downloads" }))).toEqual([]);
    expect(transferNeeds(spec({ toDir: "/Users/sparkuser/Downloads/models" }))).toEqual([]);
    expect(transferNeeds(spec({ toDir: "~/Downloads", policies: { "run.sh": "replace" } }))).toEqual(["mac"]);
    expect(transferNeeds(spec({ toDir: "~/Downloads", op: "move" }))).toEqual(["mac", "cell1"]);
    expect(transferNeeds(spec({ from: "mac", to: "cell1", toDir: "/tmp" }))).toEqual(["cell1"]);
    expect(isDownloads("~/Downloadsx")).toBe(false);
  });
});

describe("Transfers per data source, one line when idle (r4 F5)", () => {
  const t = (state: Transfer["state"], source: "demo" | "live") => ({ state, source }) as Transfer;

  it("Retry only for a finished transfer from the source in use", () => {
    expect(canRetry(t("failed", "demo"), "live")).toBe(false);
    expect(canRetry(t("failed", "live"), "live")).toBe(true);
    expect(canRetry(t("done", "live"), "live")).toBe(false);
    expect(canRetry(t("running", "live"), "live")).toBe(false);
  });

  it("sums up a queue with nothing running in one line", () => {
    expect(idleSummary([t("done", "live"), t("done", "live"), t("failed", "live")])).toBe("No transfers running · 2 finished, 1 failed");
    expect(idleSummary([t("running", "live"), t("done", "live")])).toBeNull();
    expect(idleSummary([])).toBeNull();
  });
});

describe("a copy of keys or credentials", () => {
  const HOME = "/Users/sparkuser";
  const mac = (dir: string, name: string, op: "copy" | "move" = "copy"): TransferSpec => spec([name], { dir, op, to: "cell1", toDir: "/tmp" });
  const fromNode = (dir: string, name: string, extra: Partial<TransferSpec> = {}): TransferSpec => spec([name], { from: "cell1", dir, to: "mac", toDir: `${HOME}/Documents`, ...extra });
  const secrets: [string, TransferSpec][] = [
    ["~/.ssh", mac(HOME, ".ssh")],
    ["a file in ~/.ssh", mac(`${HOME}/.ssh`, "id_ed25519")],
    ["~ with ~ for the home", mac("~", ".aws")],
    ["the Keychains", mac(`${HOME}/Library`, "Keychains")],
    ["~/.config/gh", mac("~/.config", "gh")],
    ["~/.config/gcloud", mac("~/.config", "gcloud")],
    ["~/.azure", mac(HOME, ".azure")],
    ["~/.netrc", mac(HOME, ".netrc")],
    ["~/.git-credentials", mac(HOME, ".git-credentials")],
    ["~/.cache: it holds the Hugging Face token", mac(HOME, ".cache")],
    ["~/.docker: it holds config.json", mac(HOME, ".docker")],
    ["the whole of ~", mac("/Users", "sparkuser")],
    ["another case", mac(HOME, ".SSH")],
    ["a node's ~/.ssh", fromNode("/home/sparkuser", ".ssh")],
    ["a node's hf_token.txt", fromNode("~", "hf_token.txt")],
    ["a node's API key, into Downloads", fromNode("/home/sparkuser/glm53", "api_key", { toDir: `${HOME}/Downloads` })],
  ];

  beforeEach(() => {
    asked.length = 0;
    getToasts().forEach((t) => dismissToast(t.id));
    _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true });
  });

  it("is refused by the gate, Copy and Move alike, with nothing to unlock", () => {
    for (const [label, s] of secrets) {
      for (const op of ["copy", "move"] as const) {
        const gate = transferGate({ ...s, op }, HOME);
        expect(gate.refused, `${op} ${label}`).toBe("SparkBox won't copy a folder that may hold keys or credentials.");
        expect(gate.needs, `${op} ${label}`).toEqual([]);
        expect(transferLocked(gate), `${op} ${label}`).toBe(false);
      }
    }
  });

  it("no prompt, no backend, no queue entry: one toast says why", async () => {
    for (const [label, s] of secrets) {
      mockCalls.length = 0;
      getToasts().forEach((t) => dismissToast(t.id));
      expect(await startGatedTransfer(s, names, "copy it", HOME), label).toBeNull();
      expect(asked, `${label}: no sheet`).toEqual([]);
      expect(mockCalls.map((c) => c.cmd), `${label}: no admin_authenticate, no files_preflight`).toEqual([]);
      expect(getTransfers(), label).toEqual([]);
      expect(getToasts().map((t) => t.body), label).toEqual([SECRET_REFUSED]);
    }
  });

  it("a folder that only sits near one still goes", () => {
    const fine: [string, TransferSpec][] = [
      ["~/Documents", mac(HOME, "Documents")],
      ["~/.config/nvim", mac("~/.config", "nvim")],
      ["~/.cache/huggingface/hub", mac(`${HOME}/.cache/huggingface`, "hub")],
      ["~/.docker/cli-plugins", mac(`${HOME}/.docker`, "cli-plugins")],
      ["a node's models", fromNode("/home/sparkuser", "models")],
      ["a node's ~: only this Mac's holds-a-key rule reads a folder whole", fromNode("/home", "sparkuser")],
      ["a node's other file in glm53", fromNode("/home/sparkuser/glm53", "inner.sh")],
    ];
    for (const [label, s] of fine) expect(transferGate(s, HOME).refused, label).toBeUndefined();
  });

  it("reads a node's paths as the backend does", () => {
    for (const p of ["~/.ssh", "~/.ssh/id_rsa", "/root/.aws/credentials", "/home/sparkuser/hf_token.txt", "/home/x/.docker/config.json"]) expect(isNodeSecret(p), p).toBe(true);
    for (const p of ["~", "/home/sparkuser", "/home/sparkuser/.docker", "/etc/ssh", "/opt/.ssh", "/home/sparkuser/.SSH"]) expect(isNodeSecret(p), p).toBe(false);
  });

  it("the lists here are mac.rs's lists", () => {
    const list = (name: string) => [...macRs.match(new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`))![1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect(SECRET_DIRS).toEqual(list("SECRET_DIRS"));
    expect(NODE_SECRETS).toEqual(list("NODE_SECRETS"));
  });
});
