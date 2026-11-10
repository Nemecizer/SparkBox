import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { channelSend, mockCalls, mockTauri } from "../test/tauri";
import { metaSequence, OptionTracker, terminalShortcut, type KeyLike } from "../windows/terminal/keys";
import { act, render, renderHook, screen } from "@testing-library/react";
import { createElement } from "react";
import { LockedBanner } from "../windows/terminal/LockedBanner";
import { _resetAdminForTests, lockAdmin } from "./admin";
import { useDialogQueue } from "./session";
import { connectTo } from "../windows/terminal/QuickConnect";
import {
  authenticateTab, broadcastCount, canFollowAgain, closeAll, closeTab, getTerminals, liveSessionCount, logEndLine, openTerminal, REFUSED_INPUT, setBroadcast, setPaused,
  skipMarker, terminalGuardReason, toggleInBroadcast,
} from "./terminals";
import type { ClusterSnapshot } from "./types";
import { TerminalWindow } from "../windows/TerminalWindow";

const ADMIN = { unlocked: true, expiresAtMs: Date.now() + 600_000, idleMinutes: 15, requiresAuth: false };

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const calls = (cmd: string) => mockCalls.filter((c) => c.cmd === cmd).map((c) => c.args);

let channels: Record<string, unknown> = {};

function backend(extra: Record<string, (a: any) => unknown> = {}) {
  let n = 0;
  channels = {};
  mockTauri({
    terminal_open: (a) => {
      const id = `t${++n}`;
      channels[id] = a.onEvent;
      return id;
    },
    terminal_input: () => null,
    terminal_resize: () => null,
    terminal_close: () => null,
    terminal_ack: () => null,
    ...extra,
  });
}

beforeEach(() => {
  _resetAdminForTests(ADMIN);
  backend();
});
afterEach(() => {
  closeAll();
  _resetAdminForTests();
});

describe("tab lifecycle", () => {
  it("reuses a connecting log follower for the same node and container", async () => {
    const pending = deferred<string>(); backend({ terminal_open: () => pending.promise });
    const first = openTerminal("cell1", { kind: "containerLogs", container: "abcdef012345" }, { title: "head" });
    const again = openTerminal("cell1", { kind: "containerLogs", container: "abcdef012345" }, { title: "head" });
    expect(again).toBe(first); expect(getTerminals().tabs).toHaveLength(1);
    await flush(); expect(calls("terminal_open")).toHaveLength(1);
    pending.resolve("log-1"); await flush();
    openTerminal("cell2", { kind: "containerLogs", container: "abcdef012345" }); await flush();
    expect(getTerminals().tabs).toHaveLength(2);
  });

  it("closes the session a tab opened if the tab was closed while connecting", async () => {
    const pending = deferred<string>();
    backend({ terminal_open: () => pending.promise });
    const key = openTerminal("cell1");
    expect(getTerminals().tabs[0].phase).toBe("connecting");
    closeTab(key);
    expect(getTerminals().tabs).toHaveLength(0);
    pending.resolve("t42");
    await flush();
    expect(calls("terminal_close")).toEqual([{ id: "t42" }]);
  });

  it("counts open sessions and says which are running", async () => {
    openTerminal("cell1");
    openTerminal("cell2");
    await flush();
    expect(liveSessionCount()).toBe(2);
    expect(terminalGuardReason()).toBe("2 SSH sessions are open");
    const t = getTerminals().tabs[0];
    channelSend(channels[t.id!], { kind: "data", data: btoa("building...\r\n") });
    await flush();
    expect(terminalGuardReason()).toBe("2 SSH sessions are open, 1 is running a command");
    expect(terminalGuardReason(Date.now() + 6000)).toBe("2 SSH sessions are open");
  });

  it("acknowledges output once xterm has drawn it", async () => {
    openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs[0];
    channelSend(channels[t.id!], { kind: "data", data: btoa("x".repeat(40_000)) });
    await new Promise((r) => setTimeout(r, 60));
    const acked = calls("terminal_ack").reduce((s, a) => s + (a.bytes as number), 0);
    expect(acked).toBe(40_000);
  });
});

describe("broadcast", () => {
  it("defaults to shells only and never reaches log or journal tabs", async () => {
    openTerminal("cell1");
    openTerminal("cell2", { kind: "containerShell", container: "0123456789ab" }, { title: "vllm-head" });
    openTerminal("cell1", { kind: "containerLogs", container: "0123456789ab" }, { title: "vllm-head" });
    openTerminal("cell3", { kind: "journal", unit: "docker.service" });
    await flush();
    const [shell, cshell, logs, journal] = getTerminals().tabs;
    expect([shell.inBroadcast, cshell.inBroadcast, logs.inBroadcast, journal.inBroadcast]).toEqual([true, true, false, false]);
    expect(logs.readOnly && journal.readOnly).toBe(true);
    expect(logs.term.options.disableStdin && journal.term.options.disableStdin).toBe(true);
    expect(logs.baseTitle).toBe("vllm-head");

    setBroadcast(true);
    expect(broadcastCount()).toBe(2);
    shell.term.input("uptime\r", true);
    await flush();
    const sentTo = calls("terminal_input").map((a) => a.id).sort();
    expect(sentTo).toEqual([shell.id, cshell.id].sort());

    // A log tab can't join, and typing in it (Ctrl-C included) sends nothing.
    toggleInBroadcast(logs.key);
    expect(getTerminals().tabs[2].inBroadcast).toBe(false);
    const before = calls("terminal_input").length;
    logs.term.input("\x03", true);
    await flush();
    expect(calls("terminal_input")).toHaveLength(before);
  });

  it("keeps typing into shells when Admin mode relocks; the backend says when a shell locks", async () => {
    backend({ admin_lock: () => ({ ...ADMIN, unlocked: false, expiresAtMs: null }) });
    openTerminal("cell1");
    openTerminal("cell1", { kind: "journal", unit: "docker.service" });
    await flush();
    const [shell, journal] = getTerminals().tabs;
    await lockAdmin();
    expect(shell.locked).toBe(false);
    shell.term.input("uptime\r", true);
    await flush();
    expect(calls("terminal_input")).toEqual([{ id: shell.id, data: "uptime\r" }]);
    expect(journal.locked).toBe(false);
    // An explicit lock reaches the shell as a `locked` event.
    channelSend(channels[shell.id!], { kind: "locked", reason: "Locked from the top bar" });
    await flush();
    expect(shell.locked).toBe(true);
    shell.term.input("rm -rf /tmp/x\r", true);
    await flush();
    expect(calls("terminal_input")).toHaveLength(1);
  });
});

const key = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  type: "keydown", code, key: code.replace(/^Key/, "").toLowerCase(), altKey: false, shiftKey: false, metaKey: false, ctrlKey: false, ...mods,
});

describe("option as meta", () => {
  it("sends Esc+key only for the Option key chosen as Meta", () => {
    const opt = key("KeyB", { altKey: true });
    expect(metaSequence(opt, "normal", "left")).toBeNull();
    expect(metaSequence(opt, "left", "left")).toBe("\x1bb");
    expect(metaSequence(opt, "left", "right")).toBeNull(); // right Option still types characters (@ { } on other layouts)
    expect(metaSequence(opt, "right", "right")).toBe("\x1bb");
    expect(metaSequence(opt, "both", "right")).toBe("\x1bb");
    expect(metaSequence(key("KeyB", { altKey: true, shiftKey: true }), "both", "left")).toBe("\x1bB");
    expect(metaSequence(key("Period", { altKey: true }), "both", "left")).toBe("\x1b.");
    expect(metaSequence(key("Backspace", { altKey: true }), "both", "left")).toBe("\x1b\x7f");
    expect(metaSequence(key("ArrowLeft", { altKey: true }), "both", "left")).toBe("\x1bb");
    expect(metaSequence(key("KeyA", { altKey: true, ctrlKey: true }), "both", "left")).toBe("\x1b\x01");
    expect(metaSequence(key("KeyB"), "both", "left")).toBeNull();
    expect(metaSequence(key("KeyB", { altKey: true, metaKey: true }), "both", "left")).toBeNull();
  });

  it("tracks which Option key is down", () => {
    const t = new OptionTracker();
    t.observe({ type: "keydown", code: "AltRight", altKey: true });
    expect(t.side).toBe("right");
    t.observe({ type: "keyup", code: "AltRight", altKey: false });
    t.observe({ type: "keydown", code: "AltLeft", altKey: true });
    expect(t.side).toBe("left");
    t.observe({ type: "keydown", code: "KeyX", altKey: false });
    expect(t.side).toBeNull();
  });

  it("keeps ⌘K, zoom and ⌘W for the app", () => {
    expect(terminalShortcut(key("KeyK", { metaKey: true }))).toBeNull();
    expect(terminalShortcut(key("Equal", { metaKey: true }))).toBeNull();
    expect(terminalShortcut(key("KeyW", { metaKey: true }))).toBeNull();
    expect(terminalShortcut(key("KeyK", { metaKey: true, altKey: true }))).toBe("clear-scrollback");
    expect(terminalShortcut(key("BracketRight", { metaKey: true, shiftKey: true }))).toBe("next-tab");
    expect(terminalShortcut(key("Tab", { ctrlKey: true, shiftKey: true }))).toBe("prev-tab");
    expect(terminalShortcut(key("KeyF", { metaKey: true }))).toBe("find");
  });
});

describe("r2: no stalls and revoked shells", () => {
  it("shows what was skipped while a log tab was paused, in order, once it resumes", async () => {
    openTerminal("cell1", { kind: "containerLogs", container: "vllm-head" });
    await flush();
    const t = getTerminals().tabs[0];
    setPaused(t.key, true);
    channelSend(channels[t.id!], { kind: "data", data: btoa("held\r\n") });
    channelSend(channels[t.id!], { kind: "skipped", bytes: 3_355_443 });
    await flush();
    expect(t.held).toBe(6);
    const written: string[] = [];
    const write = t.term.write.bind(t.term);
    t.term.write = ((data: string | Uint8Array, cb?: () => void) => {
      written.push(typeof data === "string" ? data : new TextDecoder().decode(data));
      return write(data, cb);
    }) as typeof t.term.write;
    setPaused(t.key, false);
    expect(written.join("")).toContain("held\r\n");
    expect(written.join("").indexOf("held")).toBeLessThan(written.join("").indexOf("skipped while paused"));
    expect(skipMarker(3_355_443)).toContain("… 3.2 MiB skipped while paused …");
  });

  it("locks a shell the backend revoked, and authenticating lets the same session type again", async () => {
    backend({ admin_authenticate: () => ({ ...ADMIN, authenticated: true }), terminal_authorize: () => null });
    openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs[0];
    channelSend(channels[t.id!], { kind: "locked", reason: "Admin mode relocked (idle)" });
    await flush();
    expect(t.locked && t.lockReason).toBeTruthy();
    expect(await authenticateTab(t.key)).toBe(true);
    expect(calls("terminal_authorize")).toEqual([{ id: t.id }]);
    expect(t.locked).toBe(false);
    expect(t.id).toBe(getTerminals().tabs[0].id);
  });
});

describe("r3: shells that keep working", () => {
  it("a refused keystroke isn't sent, flashes the tab and shows the banner", async () => {
    openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs[0];
    channelSend(channels[t.id!], { kind: "locked", reason: "Locked: nothing typed for 60 min." });
    await flush();
    expect(t.refusedAtMs).toBeNull();
    t.term.input("ls\r", true);
    await flush();
    expect(calls("terminal_input")).toHaveLength(0);
    expect(t.refusedAtMs).not.toBeNull();
    render(createElement(LockedBanner, { tab: t }));
    expect(screen.getByRole("alert")).toHaveTextContent(REFUSED_INPUT);
    expect(screen.getByRole("alert")).toHaveTextContent("Locked: nothing typed for 60 min.");
    expect(REFUSED_INPUT).toBe("Locked — authenticate to keep typing. Keystrokes typed while locked aren't sent.");
  });

  it("a keystroke the backend refuses is never dropped silently", async () => {
    backend({ terminal_input: () => { throw JSON.stringify({ kind: "locked", message: "Locked — authenticate to keep typing. Keystrokes typed while locked aren't sent." }); } });
    openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs[0];
    t.term.input("x", true);
    await flush();
    await flush();
    expect(t.locked).toBe(true);
    expect(t.refusedAtMs).not.toBeNull();
  });

  it("shows the idle warning until something is typed", async () => {
    openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs[0];
    const text = "This shell locks in 60 s: nothing typed for 59 min. Type anything to keep it open.";
    channelSend(channels[t.id!], { kind: "idleWarning", text });
    await flush();
    expect(t.idleWarning).toBe(text);
    t.term.input("x", true);
    await flush();
    expect(t.idleWarning).toBeNull();
  });

  it("asks before a Demo unlock for a shell, never silently, and writes nothing into the tab", async () => {
    _resetAdminForTests({ ...ADMIN, unlocked: false, expiresAtMs: null });
    const key = openTerminal("cell1");
    await flush();
    const t = getTerminals().tabs.find((x) => x.key === key)!;
    expect(t.phase).toBe("closed");
    expect(t.reconnect?.needsAdmin).toBe(true);
    expect(calls("admin_unlock")).toHaveLength(0);
    expect(calls("terminal_open")).toHaveLength(0);
    let text = "";
    t.term.write("", () => {});
    const buf = t.term.buffer.active;
    for (let i = 0; i < buf.length; i++) text += buf.getLine(i)?.translateToString(true) ?? "";
    expect(text).not.toContain("Shells need Admin mode");
  });
});

describe("quick connect", () => {
  it("asks before unlocking Admin mode for a shell in Demo, and opens nothing when cancelled", async () => {
    _resetAdminForTests({ ...ADMIN, unlocked: false });
    const dialogs = renderHook(() => useDialogQueue());
    let done!: Promise<void>;
    await act(async () => {
      done = connectTo({ node: "cell1", target: { kind: "shell" } });
      await flush();
    });
    const ask = dialogs.result.current.at(-1);
    expect(ask?.title).toBe("Unlock Admin mode?");
    await act(async () => {
      ask!.settle(false);
      await done;
    });
    expect(getTerminals().tabs).toHaveLength(0);
    expect(calls("admin_unlock")).toHaveLength(0);
  });
});

describe("r4: a log tab ends honestly", () => {
  const now = 1_793_274_235_000;
  const cluster = (state: string, finishedAt = 1_793_053_280) =>
    ({ nodes: [{ id: "cell1", containers: [{ id: "7da4d848a41a", name: "glm53", state, finishedAt }] }] }) as unknown as ClusterSnapshot;
  const logs = { node: "cell1", target: { kind: "containerLogs" as const, container: "glm53" } };

  it("says how a stopped container's log ended, and never asks to restart a follower", () => {
    expect(logEndLine(logs, null, cluster("exited"), now)).toBe("— End of log · glm53 exited 2 days ago —");
    expect(logEndLine(logs, null, cluster("running"), now)).toBe("— The log stream ended · glm53 is still running —");
    expect(logEndLine(logs, null, null, now)).toBe("— End of log · glm53 is no longer on cell1 —");
    expect(logEndLine({ node: "cell1", target: { kind: "journal", unit: "ollama.service" } }, null, null, now)).toBe("— End of journal —");
    expect(logEndLine(logs, "Connection to cell1 closed: the node stopped answering", cluster("running"), now)).toBe("— Connection to cell1 closed: the node stopped answering —");
    expect(canFollowAgain(logs, cluster("exited"))).toBe(false);
    expect(canFollowAgain(logs, cluster("running"))).toBe(true);
  });

  it("ends the tab with that line when docker logs finishes, releasing what was held", async () => {
    const key = openTerminal("cell1", { kind: "containerLogs", container: "glm53" }, { show: false });
    await flush();
    const t = getTerminals().tabs.find((x) => x.key === key)!;
    setPaused(key, true);
    channelSend(channels.t1, { kind: "data", data: btoa("RuntimeError: Engine core initialization failed.\r\n") });
    channelSend(channels.t1, { kind: "exit", code: 0, message: null });
    await flush();
    await new Promise<void>((r) => t.term.write("", r));
    const buf = t.term.buffer.active;
    let text = "";
    for (let i = 0; i < buf.length; i++) text += `${buf.getLine(i)?.translateToString(true) ?? ""}\n`;
    expect(text).toContain("RuntimeError: Engine core initialization failed.");
    // (No snapshot in this test: the container isn't in the cluster the tab can see.)
    expect(text).toContain("— End of log · glm53 is no longer on cell1 —");
    expect(text).not.toContain("Restart the follower");
    expect(text).not.toContain("ended with code 0");
    expect(text.indexOf("RuntimeError")).toBeLessThan(text.indexOf("— End of log"));
    expect(t.paused).toBe(false);
    expect(t.phase).toBe("closed");
  });

  it("hides Pause once stopped and offers Follow again only for a running container", async () => {
    openTerminal("cell1", { kind: "containerLogs", container: "glm53" }, { show: false });
    await flush();
    channelSend(channels.t1, { kind: "exit", code: 0, message: null });
    await flush();
    render(createElement(TerminalWindow));
    expect(screen.queryByRole("button", { name: /Pause/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Follow again/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Restart follower/ })).toBeNull();
    expect(screen.getByText("End of log")).toBeInTheDocument();
  });

  it("counts log streams apart from shells when asking before Close all", async () => {
    openTerminal("cell1", { kind: "containerLogs", container: "glm53" }, { show: false });
    await flush();
    expect(terminalGuardReason()).toBe("1 log stream is open");
    openTerminal("cell2", { kind: "shell" }, { show: false });
    await flush();
    expect(terminalGuardReason()).toBe("1 SSH session and 1 log stream are open");
  });
});
