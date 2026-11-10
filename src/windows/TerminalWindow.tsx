// Terminal: tabs of SSH sessions (shells, container shells, container logs and journals).
// Sessions live in lib/terminals.ts, so minimizing keeps them running; closing the window
// asks first when sessions are open. ⌘T quick-connects, ⌘W closes a tab (the window when
// it's the last), ⌘F finds in the scrollback, ⇧⌘[ ⇧⌘] and ⌃Tab switch tabs, ⌥⌘K clears.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ChevronDown, Eraser, FileDown, Pause, Play, Plus, Radio, RotateCcw, RotateCw, Search, Settings2, WrapText,
} from "lucide-react";
import "@xterm/xterm/css/xterm.css";
import { HelpTip, Spacer, Toolbar, ToolButton, ToolSeparator } from "../components/ui";
import { LOOK_ONLY_TIP, useAdmin, useAdminState } from "../lib/admin";
import { api, inTauri } from "../lib/api";
import { showContextMenu } from "../lib/contextMenu";
import { fmtBytes } from "../lib/format";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import {
  attachPasteGuard, broadcastCount, canFollowAgain, clearScrollback, closeTab, isRunning, lastShellNode, logContainer, reconnect, restartFollower, setBroadcast,
  setFindOpen, setPaused, setSince, setWrap, tabText, tabTitle, terminalGuardReason, unlockForShell, useTerminals, windowMounted, type LogSince, type TermTab,
} from "../lib/terminals";
import { cssVar } from "../lib/theme";
import { toast } from "../lib/toasts";
import { useCloseGuard, useWindowEvent, useWindowId } from "../wm/WindowManager";
import { FindBar } from "./terminal/FindBar";
import { IdleWarning, LockedBanner } from "./terminal/LockedBanner";
import { nodeMenu } from "./terminal/menus";
import { useTermPrefs } from "./terminal/prefs";
import { connectTo, QuickConnect } from "./terminal/QuickConnect";
import { TabBar } from "./terminal/TabBar";
import { terminalTheme, themeIsDark } from "./terminal/theme";
import "./terminal/terminal.css";

/** Columns a log tab gets when lines don't wrap (it scrolls sideways instead). */
const NOWRAP_COLS = 320;

/** Shows one tab's xterm. The terminal object outlives this component, so it moves in and out. */
function TermHost({ tab, visible }: { tab: TermTab; visible: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const prefs = useTermPrefs();

  const refit = () => {
    const el = ref.current;
    if (!el || el.offsetParent === null) return;
    try {
      const dims = tab.fit.proposeDimensions();
      if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return;
      const cols = tab.readOnly && !tab.wrap ? Math.max(NOWRAP_COLS, dims.cols) : dims.cols;
      if (cols !== tab.term.cols || dims.rows !== tab.term.rows) tab.term.resize(Math.max(2, cols), Math.max(2, dims.rows));
    } catch {
      // Not laid out yet.
    }
  };

  useEffect(() => {
    const el = ref.current!;
    if (tab.term.element) {
      el.appendChild(tab.term.element);
    } else {
      tab.term.open(el);
      // The GPU renderer, falling back to the DOM one if WebGL isn't there or is lost.
      import("@xterm/addon-webgl")
        .then(({ WebglAddon }) => {
          if (!tab.term.element) return;
          const gl = new WebglAddon();
          gl.onContextLoss(() => gl.dispose());
          tab.term.loadAddon(gl);
        })
        .catch(() => {});
      attachPasteGuard(tab);
    }
    const ro = new ResizeObserver(() => refit());
    ro.observe(el);
    refit();
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  useEffect(() => {
    if (!visible) return;
    const raf = requestAnimationFrame(() => {
      refit();
      tab.term.focus();
    });
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, tab, tab.wrap, prefs.fontSize, prefs.lineHeight, prefs.font]);

  return <div ref={ref} className={`sb-term-host${tab.readOnly && !tab.wrap ? " nowrap" : ""}`} style={{ display: visible ? "block" : "none" }} />;
}

const SINCE_LABEL: [LogSince | "", string][] = [["", "Last lines"], ["10m", "10 minutes"], ["1h", "1 hour"], ["6h", "6 hours"], ["24h", "24 hours"], ["boot", "This boot"]];

/** Highlights lines matching the filter (search addon decorations), again as new lines arrive. */
function LogFilter({ tab }: { tab: TermTab }) {
  const [q, setQ] = useState("");
  const [count, setCount] = useState<number | null>(null);
  useEffect(() => {
    const sub = tab.search.onDidChangeResults((r) => setCount(r ? r.resultCount : null));
    return () => sub.dispose();
  }, [tab]);
  useEffect(() => {
    if (!q) {
      tab.search.clearDecorations();
      setCount(null);
      return;
    }
    const run = () => {
      try {
        tab.search.findNext(q, {
          incremental: true,
          decorations: {
            matchBackground: cssVar("--warning") || "#efa810",
            matchOverviewRuler: cssVar("--warning") || "#efa810",
            activeMatchBackground: cssVar("--warning") || "#efa810",
            activeMatchColorOverviewRuler: cssVar("--warning") || "#efa810",
          },
        });
      } catch {
        // An unfinished pattern.
      }
    };
    run();
    const t = window.setInterval(run, 1500);
    return () => window.clearInterval(t);
  }, [q, tab]);
  return (
    <span className="sb-term-logfilter">
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Highlight…" aria-label="Highlight lines containing" spellCheck={false}
        onKeyDown={(e) => { if (e.key === "Escape") setQ(""); }} />
      {q && <span className="muted">{count === null ? "" : `${count.toLocaleString()} match${count === 1 ? "" : "es"}`}</span>}
    </span>
  );
}

function LogBar({ tab }: { tab: TermTab }) {
  const since = (tab.target as { since?: LogSince }).since ?? "";
  // Re-read every snapshot: whether the container still runs decides Follow again.
  useCluster();
  const stopped = tab.phase === "closed" && !tab.reconnect;
  const followAgain = stopped && canFollowAgain(tab);
  const ended = logContainer(tab);
  const save = async () => {
    const text = tabText(tab.key);
    if (!inTauri) {
      toast({ kind: "info", title: "Saving needs the SparkBox app", body: "The browser preview can't write files." });
      return;
    }
    try {
      const path = await invoke<string>("terminal_save_text", { name: tabTitle(tab), text });
      toast({
        kind: "success",
        title: "Saved to Downloads",
        body: path.split("/").pop(),
        actions: [{ label: "Show in Finder", run: () => void revealItemInDir(path) }],
      });
    } catch (e) {
      toast({ kind: "error", title: "Couldn't save the log", body: String(e).replace(/^Error: /, "") });
    }
  };
  return (
    <div className="sb-term-logbar" role="toolbar" aria-label="Log">
      {!stopped && (
        <button type="button" className={`btn small${tab.paused ? " primary" : ""}`} onClick={() => setPaused(tab.key, !tab.paused)}
          title={tab.paused ? "Draw what arrived and keep following" : "Hold new lines so you can read (the follower waits)"}>
          {tab.paused ? <Play size={12} /> : <Pause size={12} />}
          {tab.paused ? "Follow" : "Pause"}
        </button>
      )}
      <button type="button" className={`btn small${tab.wrap ? "" : " on"}`} aria-pressed={!tab.wrap} onClick={() => setWrap(tab.key, !tab.wrap)}
        title={tab.wrap ? "Keep long lines on one line and scroll sideways" : "Wrap long lines"}>
        <WrapText size={12} />
        {tab.wrap ? "Wrap" : "No wrap"}
      </button>
      <label className="sb-term-since" title="How far back the tab starts; changing it restarts the follower">
        From
        <select value={since} onChange={(e) => setSince(tab.key, (e.target.value || null) as LogSince | null)} aria-label="Start from">
          {SINCE_LABEL.filter(([v]) => v !== "boot" || tab.target.kind === "journal").map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </select>
      </label>
      <LogFilter tab={tab} />
      {followAgain && (
        <button type="button" className="btn small" onClick={() => restartFollower(tab.key)} title={`${ended?.name ?? "The container"} is still running: follow its log again`}>
          <RotateCw size={12} />
          Follow again
        </button>
      )}
      <button type="button" className="btn small" onClick={() => void save()} title="Save what this tab shows to a new file in Downloads">
        <FileDown size={12} />
        Save to Downloads
      </button>
      <span className="spacer" />
      <span className="sb-term-logbar-state">
        {tab.paused
          ? `Paused${tab.held ? ` · ${fmtBytes(tab.held, { decimals: 1 })} waiting` : ""}`
          : tab.phase === "open" ? "Following · read-only" : tab.phase === "connecting" ? "Connecting…" : stopped && !followAgain && tab.target.kind === "containerLogs" ? "End of log" : "Stopped"}
        {!tab.paused && tab.skipped && (
          <span className="sb-term-skipped" title="SparkBox keeps reading from the node; a tab that can't take output keeps the newest 2 MiB">
            {` · ${fmtBytes(tab.skipped.bytes, { decimals: 1 })} skipped${tab.skipped.whilePaused ? " while paused" : ""}`}
          </span>
        )}
      </span>
    </div>
  );
}

export function TerminalWindow() {
  const { tabs, active, broadcast, picker, findOpen } = useTerminals();
  const cluster = useCluster();
  const admin = useAdmin();
  const lookOnly = !!useAdminState().lookOnly;
  const prefs = useTermPrefs();
  const winId = useWindowId();
  const current = tabs.find((t) => t.key === active) ?? null;
  const [user, setUser] = useState<string | null>(null);
  const [, setTick] = useState(0);

  useEffect(() => windowMounted(), []);
  useEffect(() => {
    if (inTauri) api.getInventory().then((inv) => setUser(inv.nodes[0]?.user ?? null), () => {});
    else setUser("sparkuser");
  }, []);
  // Keeps "connected 12m" and the reconnect countdown fresh.
  useEffect(() => {
    const t = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(t);
  }, []);

  useCloseGuard(() => terminalGuardReason());
  useWindowEvent("find", () => {
    if (current) setFindOpen(true);
  });

  const requestCloseTab = async (key: string) => {
    const tab = tabs.find((t) => t.key === key);
    if (!tab) return;
    if (!tab.readOnly && isRunning(tab)) {
      const ok = await confirmAction({
        title: `Close “${tabTitle(tab)}”?`,
        body: "It's running a command (output in the last few seconds). Closing the tab ends it.",
        confirmLabel: "Close tab",
        danger: true,
        winId,
      });
      if (!ok) return;
    }
    closeTab(key);
  };

  // ⌘W closes the active tab; the window closes with its last one.
  useWindowEvent("close-request", (e) => {
    if (tabs.length > 1 && current) {
      e.preventDefault();
      void requestCloseTab(current.key);
    }
  });

  const newShell = async () => {
    const node = lastShellNode() ?? cluster?.nodes.find((n) => n.status === "online")?.id ?? cluster?.nodes[0]?.id;
    if (node) await connectTo({ node, target: { kind: "shell" } });
  };

  const receiving = broadcastCount();
  const activeReceives = broadcast && !!current && current.inBroadcast && !current.readOnly;
  const darkChrome = themeIsDark(prefs.theme);
  const lastNode = lastShellNode();

  return (
    <div className={`sb-term col fill${darkChrome ? " dark-chrome" : " light-chrome"}`}>
      <Toolbar label="Terminal">
        <div className="sb-term-split" role="group" aria-label="New tab">
          <button type="button" className="tool" onClick={() => void newShell()} disabled={lookOnly}
            title={lookOnly ? LOOK_ONLY_TIP : `New shell${lastNode ? ` on ${lastNode}` : ""}${admin ? "" : ". Needs Admin mode."}`}>
            <Plus size={14} />
            <span className="tool-label">New shell</span>
          </button>
          <button type="button" className="tool sb-term-split-more" aria-label="Choose a node and what to open" title="Shell, container shell, logs or journal on a node"
            onClick={(e) => showContextMenu(e, nodeMenu(cluster, admin, lookOnly))}>
            <ChevronDown size={12} />
          </button>
        </div>
        <ToolSeparator />
        <ToolButton icon={Radio} label="Broadcast" active={broadcast} onClick={() => setBroadcast(!broadcast)} disabled={!tabs.some((t) => !t.readOnly)}
          title="Type once, send the same keystrokes to every ticked shell tab (log tabs never receive)" />
        <HelpTip topic="desk.terminal.broadcast" />
        {broadcast && (
          <span className="sb-term-chip" role="status">
            <Radio size={11} />
            Broadcasting to {receiving}
          </span>
        )}
        <ToolButton icon={Search} label="Find" priority={2} disabled={!current} onClick={() => setFindOpen(true)} title="Find in the scrollback (⌘F)" />
        <ToolButton icon={Eraser} label="Clear" priority={3} disabled={!current} onClick={() => current && clearScrollback(current.key)} title="Clear the scrollback (⌥⌘K)" />
        <ToolButton icon={RotateCcw} label="Reconnect" priority={2} disabled={current?.phase !== "closed" || (current.target.kind === "containerLogs" && !canFollowAgain(current))}
          locked={!!current && !current.readOnly && current.phase === "closed" && !admin}
          onClick={() => current && reconnect(current.key)}
          title={current?.target.kind === "containerLogs" && current.phase === "closed" && !canFollowAgain(current) ? "This container has stopped: its whole log is in the tab" : "Open this tab's session again"} />
        <Spacer />
        <ToolButton icon={Settings2} label="Terminal settings" priority={3}
          onClick={() => window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind: "settings", params: { section: "terminal" } } }))}
          title="Theme, font, cursor, scrollback and the Option key" />
      </Toolbar>
      {tabs.length > 0 && <TabBar tabs={tabs} active={active} broadcast={broadcast} user={user} onClose={(k) => void requestCloseTab(k)} />}
      {current?.readOnly && <LogBar tab={current} />}
      <div className={`sb-term-area${activeReceives ? " broadcasting" : ""}`} style={{ background: tabs.length ? terminalTheme(prefs.theme).background : undefined }}>
        {tabs.map((t) => <TermHost key={t.key} tab={t} visible={t.key === active} />)}
        {current && findOpen && <FindBar tab={current} />}
        {current?.locked && current.phase === "open" && <LockedBanner tab={current} />}
        {current?.idleWarning && !current.locked && current.phase === "open" && <IdleWarning text={current.idleWarning} />}
        {current?.reconnect && (
          <div className="sb-term-banner" role="status">
            <RotateCw size={13} className={current.phase === "connecting" ? "spin" : undefined} />
            <span>
              {current.reconnect.needsAdmin
                ? current.connectedMs ? "Connection lost. Reconnecting this shell needs Admin mode." : "This shell opens once Admin mode is unlocked."
                : current.phase === "connecting"
                  ? `Reconnecting… (attempt ${current.reconnect.attempt})`
                  : `Connection lost. Reconnecting in ${Math.max(1, Math.ceil((current.reconnect.atMs - Date.now()) / 1000))} s (attempt ${current.reconnect.attempt})`}
            </span>
            <span className="spacer" />
            {current.reconnect.needsAdmin
              ? <button type="button" className="btn small" onClick={() => void unlockForShell(current.node)}>Unlock…</button>
              : <button type="button" className="btn small" onClick={() => reconnect(current.key)} disabled={current.phase === "connecting"}>Reconnect now</button>}
          </div>
        )}
        {tabs.length === 0 && <QuickConnect />}
      </div>
      {picker && <QuickConnect floating />}
      {tabs.length > 0 && (
        <div className="sb-term-hint" aria-hidden>
          ⌘T new tab · ⌘W close tab · ⇧⌘[ ⇧⌘] switch · ⌘F find · ⌥⌘K clear · Option key: {prefs.optionKey === "normal" ? "types characters" : prefs.optionKey === "both" ? "Meta" : `${prefs.optionKey} is Meta`}
        </div>
      )}
    </div>
  );
}


