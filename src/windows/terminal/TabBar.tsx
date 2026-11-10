import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Bell, Box, Lock, ScrollText, Settings2, SquareTerminal, X, type LucideIcon } from "lucide-react";
import { fmtDuration } from "../../lib/format";
import { getCluster } from "../../lib/store";
import { logContainer, moveTab, renameTab, setActive, tabTitle, toggleInBroadcast, type TermTab } from "../../lib/terminals";

const ICON: Record<TermTab["target"]["kind"], LucideIcon> = {
  shell: SquareTerminal,
  containerShell: Box,
  containerLogs: ScrollText,
  journal: Settings2,
};

const KIND_WORD: Record<TermTab["target"]["kind"], string> = {
  shell: "Shell",
  containerShell: "Container shell",
  containerLogs: "Container logs (read-only)",
  journal: "Journal (read-only)",
};

/** "sparkuser@192.168.50.51 · 0.4 ms · connected 12m 3s · last exit 0". */
export function tooltip(t: TermTab, user: string | null): string {
  const node = getCluster()?.nodes.find((n) => n.id === t.node);
  const parts = [`${KIND_WORD[t.target.kind]} on ${node?.name ?? t.node}`];
  parts.push(`${user ?? "user"}@${node?.host ?? t.node}`);
  if (node?.latencyMs != null) parts.push(`${node.latencyMs.toFixed(node.latencyMs < 10 ? 1 : 0)} ms`);
  if (t.phase === "open" && t.connectedMs) parts.push(`connected ${fmtDuration((Date.now() - t.connectedMs) / 1000)}`);
  if (t.phase === "connecting") parts.push("connecting");
  if (t.target.kind === "containerLogs") {
    const c = logContainer(t);
    parts.push(c ? `container ${c.state}${c.state === "exited" && c.exitCode != null ? ` · exit ${c.exitCode}` : ""}` : "container state unknown");
    if (t.exitCode != null && t.exitCode !== 0) parts.push(`log command exit ${t.exitCode}`);
  } else if (t.exitCode !== undefined) parts.push(`last exit ${t.exitCode === null ? "none (closed)" : t.exitCode}`);
  if (t.phase === "closed") parts.push(t.readOnly ? "stopped" : "closed: press Return in it to reconnect");
  return parts.join(" · ");
}

function TabTitle({ tab }: { tab: TermTab }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (editing) input.current?.select();
  }, [editing]);
  if (editing) {
    return (
      <input
        ref={input}
        className="sb-term-tab-rename"
        value={value}
        aria-label="Tab name"
        placeholder={tab.oscTitle ?? tab.baseTitle}
        onChange={(e) => setValue(e.target.value)}
        onPointerDown={(e) => e.stopPropagation()}
        onBlur={() => {
          renameTab(tab.key, value);
          setEditing(false);
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") {
            renameTab(tab.key, value);
            setEditing(false);
          } else if (e.key === "Escape") {
            setEditing(false);
          }
        }}
      />
    );
  }
  return (
    <span
      className="sb-term-tab-title"
      onDoubleClick={(e) => {
        e.stopPropagation();
        setValue(tab.customTitle ?? tabTitle(tab));
        setEditing(true);
      }}
    >
      {tabTitle(tab)}
    </span>
  );
}

/** The tab strip: click to switch, double-click to rename, drag to reorder, middle-click to close. */
export function TabBar({ tabs, active, broadcast, user, onClose }: {
  tabs: TermTab[];
  active: string | null;
  broadcast: boolean;
  user: string | null;
  onClose: (key: string) => void;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<{ key: string; x: number; index: number } | null>(null);

  const startDrag = (t: TermTab) => (e: ReactPointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, input")) return;
    const x0 = e.clientX;
    let moving = false;
    let index = tabs.indexOf(t);
    const rects = [...(strip.current?.querySelectorAll<HTMLElement>("[data-tab]") ?? [])].map((el) => el.getBoundingClientRect());
    const onMove = (ev: PointerEvent) => {
      if (!moving && Math.abs(ev.clientX - x0) < 5) return;
      moving = true;
      // The slot whose middle the pointer has passed.
      index = rects.filter((r) => ev.clientX > r.left + r.width / 2).length;
      setDrag({ key: t.key, x: ev.clientX, index });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("keydown", onKey, true);
      if (moving) {
        const from = tabs.indexOf(t);
        moveTab(t.key, index > from ? index - 1 : index);
      }
      setDrag(null);
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      moving = false;
      onUp();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("keydown", onKey, true);
  };

  const markerLeft = (() => {
    if (!drag || !strip.current) return null;
    const els = [...strip.current.querySelectorAll<HTMLElement>("[data-tab]")];
    const base = strip.current.getBoundingClientRect().left - strip.current.scrollLeft;
    if (drag.index >= els.length) return els.length ? els[els.length - 1].offsetLeft + els[els.length - 1].offsetWidth : 0;
    return els[drag.index].getBoundingClientRect().left - base;
  })();

  return (
    <div className="sb-term-tabs" role="tablist" aria-label="Terminal tabs" ref={strip}>
      {tabs.map((t) => {
        const Icon = ICON[t.target.kind];
        const receiving = broadcast && t.inBroadcast && !t.readOnly;
        const state = t.phase === "open" ? "online" : t.phase === "connecting" || t.reconnect ? "connecting" : "offline";
        return (
          <div
            key={t.key}
            data-tab={t.key}
            role="tab"
            aria-selected={t.key === active}
            tabIndex={t.key === active ? 0 : -1}
            className={[
              "sb-term-tab", t.key === active && "on", receiving && "receiving", t.readOnly && "readonly",
              drag?.key === t.key && "dragging", t.phase !== "open" && "down", t.locked && "locked",
            ].filter(Boolean).join(" ")}
            onClick={() => setActive(t.key)}
            onPointerDown={startDrag(t)}
            onAuxClick={(e) => e.button === 1 && onClose(t.key)}
            title={tooltip(t, user)}
          >
            {broadcast && !t.readOnly && (
              <input
                type="checkbox"
                className="sb-term-tab-check"
                checked={t.inBroadcast}
                onClick={(e) => e.stopPropagation()}
                onChange={() => toggleInBroadcast(t.key)}
                aria-label={`Send broadcast keystrokes to ${tabTitle(t)}`}
                title={t.inBroadcast ? "Receives broadcast keystrokes" : "Left out of the broadcast"}
              />
            )}
            <Icon size={13} className="sb-term-tab-icon" aria-hidden />
            <span className={`dot ${state}`} aria-label={t.phase} />
            <TabTitle tab={t} />
            {t.locked && <Lock key={t.refusedAtMs ?? 0} size={11} className={`sb-term-tab-lock${t.refusedAtMs ? " flash" : ""}`} aria-label="Locked" />}
            {t.bell && <Bell size={11} className="sb-term-tab-bell" aria-label="Bell" />}
            {t.activity && !t.bell && <span className="sb-term-tab-activity" aria-label="New output" title="New output" />}
            <button type="button" className="sb-term-tab-close" title="Close tab (⌘W)" aria-label={`Close ${tabTitle(t)}`} onClick={(e) => { e.stopPropagation(); onClose(t.key); }}>
              <X size={12} />
            </button>
          </div>
        );
      })}
      {drag && markerLeft !== null && <span className="sb-term-tab-marker" style={{ left: markerLeft }} aria-hidden />}
    </div>
  );
}
