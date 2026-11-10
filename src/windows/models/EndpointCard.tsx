// One inference server, the same way everywhere: what it serves, where, whether it
// is healthy (and if not, why, in one red line), and the same six numbers for every
// engine — tokens out and in, requests running and queued, KV cache, time to first
// token and between tokens — with "—" where a number doesn't apply or can't be
// trusted. `full` is the Endpoints card with its actions; `compact` is the
// Dashboard's, which opens the full one.
import { createContext, useContext, useEffect, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { Boxes, Copy, FileText, History, Lock, MessageSquare, Pencil, PowerOff, RotateCcw, SquareTerminal, Terminal, Trash2, Wrench, type LucideIcon } from "lucide-react";
import { Icon } from "../../components/Icon";
import { HelpTip, Sparkline, StopIcon, Truncate } from "../../components/ui";
import { lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, useAdmin, useAdminState } from "../../lib/admin";
import { showContextMenu, type MenuItem } from "../../lib/contextMenu";
import { endpointLabel, endpointView, fmtLatency, proofLine, protocolsLine, reachLine, readinessLine, type Endpoint, type EndpointMetrics } from "../../lib/endpoints";
import { fmtBytes } from "../../lib/format";
import { useCluster } from "../../lib/store";
import { openTerminal, type TermTargetEx } from "../../lib/terminals";
import { toast } from "../../lib/toasts";
import { useWindowId, useWM } from "../../wm/WindowManager";
import { canRestart, forgetEndpoint, restartEndpoint, stopEndpoint, unloadEndpoint } from "./endpointActions";
import { Ago, ENGINE_LABEL, curlFor, errText, fmtTokens, fmtTps, runsOnNode, useOpenChat } from "./shared";

async function copy(text: string, what: string, note?: string | null) {
  try {
    await navigator.clipboard.writeText(text);
    toast({ kind: "success", title: `${what} copied`, body: note ?? undefined });
  } catch (e) {
    toast({ kind: "error", title: `Couldn't copy the ${what.toLowerCase()}`, body: errText(e) });
  }
}

/** Ticks every second while mounted (for "next try in 1:45"). */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

function countdown(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * Where its log is: its container's, or for an Ollama run as a systemd service (no container, as
 * on cell1 and cell3) the ollama.service journal. Both tabs only read.
 */
export function logTarget(e: Pick<Endpoint, "engine" | "containerNames">): { target: TermTargetEx; what: string } | null {
  const container = e.containerNames?.[0];
  if (container) return { target: { kind: "containerLogs", container }, what: `docker logs -f ${container}` };
  if (e.engine === "ollama") return { target: { kind: "journal", unit: "ollama.service" }, what: "journalctl -u ollama.service" };
  return null;
}

/** Inside the Endpoints window, History opens in place; elsewhere it opens Endpoints on it. */
export const OpenHistoryContext = createContext<((id: string) => void) | null>(null);

/** The actions an endpoint offers, for its buttons and its context menu. */
export function useEndpointActions() {
  const wm = useWM();
  const admin = useAdmin();
  const chat = useOpenChat();
  const origin = useWindowId() || undefined;

  const logs = (e: Endpoint) => {
    const log = logTarget(e);
    if (!log) return;
    openTerminal(e.headNode, log.target);
    wm.open("terminal");
  };
  const unload = (e: Endpoint) => unloadEndpoint(e, { origin });
  const stop = (e: Endpoint) => stopEndpoint(e, { origin });
  const restart = (e: Endpoint) => restartEndpoint(e, { origin });
  const forget = (e: Endpoint) => forgetEndpoint(e, { origin });
  const copyCurl = (e: Endpoint) => copy(curlFor(e), "curl command", runsOnNode(e));

  const edit = (e: Endpoint) => e.deploymentId && wm.open("models", { tab: "load", edit: e.deploymentId });
  const inPlace = useContext(OpenHistoryContext);
  const history = (e: Endpoint) => (inPlace ? inPlace(e.id) : wm.open("endpoints", { id: e.id, view: "history", nav: String(Date.now()) }));

  const menu = (ev: ReactMouseEvent, e: Endpoint) => {
    const model = e.engine === "ollama" ? e.ollama?.[0]?.name : e.models[0];
    const items: MenuItem[] = [
      { label: "Chat with this", icon: MessageSquare, disabled: !e.providerId || e.health !== "ready" ? "Not ready for Chat" : false, run: () => e.providerId && model && chat(e.providerId, model) },
      { separator: true },
      { label: "Copy URL", icon: Copy, run: () => copy(e.url, "URL") },
      { label: "Copy curl", icon: Terminal, run: () => copyCurl(e) },
      { label: "Show logs", icon: FileText, disabled: logTarget(e) ? false : "Runs as a service SparkBox can't name", run: () => logs(e) },
      { label: "History", icon: History, run: () => history(e) },
      { separator: true },
      ...(canRestart(e) ? [{ label: "Restart…", icon: RotateCcw, locked: !admin, run: () => restart(e) }] : []),
      ...(e.managedBy === "sparkbox" ? [{ label: "Edit & reload…", icon: Pencil, run: () => edit(e) }] : []),
      e.managedBy === "sparkbox"
        ? { label: "Unload…", icon: PowerOff, danger: true, locked: !admin, run: () => unload(e) }
        : { label: "Stop containers…", icon: StopIcon, danger: true, locked: !admin, disabled: e.containerIds.length && !e.forgettable ? false : "Not running in a container", run: () => stop(e) },
      ...(e.forgettable ? [{ label: "Forget", icon: Trash2, locked: !admin, run: () => forget(e) }] : []),
    ];
    showContextMenu(ev, items);
  };

  return { admin, chat, logs, unload, stop, restart, forget, edit, history, menu, copyCurl };
}

// ---- Pieces

/**
 * A change on the card under the lock rule (admin.ts lockOf): off in look-only with look-only's
 * words, and when Admin is locked it stays on with a trailing lock, and a click asks to unlock.
 */
function ChangeButton({ lock, icon, label, title, onClick }: {
  lock: ReturnType<typeof lockOf>;
  icon: LucideIcon;
  label: string;
  title: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="btn small" disabled={lock === "lookOnly"} onClick={onClick}
      title={lock === "lookOnly" ? LOOK_ONLY_TIP : lock ? `${title} · ${NEEDS_ADMIN_TIP}` : title}>
      <Icon icon={icon} size={12} />{label}{lock && <Icon icon={Lock} size={12} className="tool-lock" />}
    </button>
  );
}

/** Ready is a neutral dot and the word; Loading shows its progress; Down is red; Idle a hollow dot; Paused a wrench. */
export function HealthPill({ e }: { e: Pick<Endpoint, "health" | "healthReason" | "message" | "loadingPct"> }) {
  const v = endpointView(e);
  return (
    <span className={`ai-pill ai-pill-${v.tone}`} title={v.reason ?? undefined}>
      {v.tone === "ok" && <i className="ai-pill-dot" aria-hidden />}
      {v.tone === "paused" && <Icon icon={Wrench} size={12} />}
      <span className="ai-pill-word">{v.word}</span>
      {v.pct != null && (
        <span className="ai-pill-bar" role="progressbar" aria-valuenow={v.pct} aria-valuemin={0} aria-valuemax={100} aria-label="Loading weights">
          <i style={{ width: `${v.pct}%` }} />
        </span>
      )}
    </span>
  );
}

/** Its nodes, head first; the ones it's missing say so. */
function Nodes({ e }: { e: Endpoint }) {
  const missing = new Set(e.missingNodes ?? []);
  const snap = useCluster();
  const nameOf = (id: string) => snap?.nodes.find((n) => n.id === id)?.name ?? id;
  return (
    <span className="ai-card-nodes">
      {(e.expectedNodes?.length ? e.expectedNodes : e.nodes).map((n, i) => (
        <span key={n} className={`ai-node${missing.has(n) && e.health !== "unknown" ? " missing" : ""}`} title={`${nameOf(n)}: ${i === 0 ? "head, serves the API" : "worker"}${missing.has(n) ? e.health === "unknown" ? " — status unknown: node unavailable" : " — missing" : ""}`}>
          {nameOf(n)}
        </span>
      ))}
    </span>
  );
}

function split(e: Pick<Endpoint, "parallel">): string | null {
  const p = e.parallel;
  if (!p || p.tp * p.pp <= 1) return null;
  return [p.tp > 1 && `TP ${p.tp}`, p.pp > 1 && `PP ${p.pp}`].filter(Boolean).join(" × ");
}

function last<T>(a: T[] | undefined, n: number): T[] {
  if (!a) return [];
  return a.length > n ? a.slice(a.length - n) : a;
}

/** A trend worth drawing: it moves. */
function moves(values: number[]): boolean {
  if (values.length < 3) return false;
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  return hi - lo > Math.max(1e-6, Math.abs(hi) * 0.02);
}

function Cell({ label, info, value, unit, sub, trend, max, color, title, stale }: {
  label: string;
  /** Its Help tip, on the full card only. */
  info?: ReactNode;
  value: string;
  /** The last value, kept in gray because it's no longer live. */
  stale?: boolean;
  unit?: string;
  sub?: string | null;
  trend?: number[];
  max?: number;
  color: string;
  title: string;
}) {
  const dash = value === "—";
  return (
    <div className="ai-cell" title={title}>
      <span className="ai-cell-label">{label}{info}</span>
      <span className={`ai-cell-value num${dash || stale ? " muted" : ""}`} title={stale ? "The last value before it stalled" : undefined}>
        {value}
        {unit && !dash && <span className="ai-cell-unit">{unit}</span>}
      </span>
      {sub && <span className="ai-cell-sub num">{sub}</span>}
      {trend && moves(trend) && <Sparkline values={trend} max={max ?? Math.max(1, ...trend)} width={120} height={18} color={color} curve="monotone" label={`${label}, recent minutes`} />}
    </div>
  );
}

/** The six numbers, the same for every engine; "—" when a number doesn't apply or it isn't serving. */
function Cells({ e, compact }: { e: Endpoint; compact?: boolean }) {
  const m: EndpointMetrics | null | undefined = e.metrics;
  const live = !!m && e.health === "ready";
  // Stalled: requests are stuck, so the last Running and Waiting are the story (in gray: not live).
  const stalled = !!m && e.health === "down" && e.stalledS != null;
  const n = compact ? 100 : 300;
  const h = m?.history;
  const dash = "—";
  const tps = (v: number | undefined) => (live && v != null ? fmtTps(v) : dash);
  const kv = live && m!.kvKnown !== false && e.engine !== "ollama" && e.engine !== "llamacpp" ? m!.kvPct : null;
  const ttft = live ? fmtLatency(m!.ttftP50Ms, m!.ttftP50Range) : dash;
  const ttft95 = live && (m!.ttftP95Ms != null || m!.ttftP95Range) ? `p95 ${fmtLatency(m!.ttftP95Ms, m!.ttftP95Range)}` : null;
  const itl = live ? fmtLatency(m!.itlP50Ms, m!.itlP50Range) : dash;
  const perStream = live && m!.itlP50Ms && !m!.itlP50Range ? `${(1000 / m!.itlP50Ms).toFixed(1)} tok/s a stream` : null;
  const counted = e.metricsSource === "sparkbox" ? " (only requests SparkBox sent or relayed)" : "";
  const cells = [
    <Cell key="gen" label="Generate" info={!compact && <HelpTip topic="ai.card.generate" />} value={tps(m?.genTps)} unit=" tok/s" trend={live ? last(h?.genTps, n) : undefined} color="var(--m-gpu)"
      title={`Tokens generated per second, all requests together${counted}`} />,
    !compact && (
      <Cell key="prompt" label="Prompt" info={<HelpTip topic="ai.card.prompt" />} value={tps(m?.promptTps)} unit=" tok/s" trend={live ? last(h?.promptTps, n) : undefined} color="var(--m-cpu)"
        title={`Prompt tokens processed per second${counted}`} />
    ),
    // The compact card is too narrow for "Running · waiting"; it says "+N waiting" only when something waits.
    // The queue wears the chartreuse: the one family that clears the normal-vision floor beside both the
    // Prompt blue and the KV-cache cyan in every theme (a teal or steel tone reads as the cyan's family).
    <Cell key="run" label={compact ? "Running" : "Running · waiting"} info={!compact && <HelpTip topic="ai.card.running" />} stale={stalled}
      value={live || stalled ? (compact ? m!.running.toFixed(0) : `${m!.running.toFixed(0)} · ${m!.waiting.toFixed(0)}`) : dash}
      unit={live && compact && m!.waiting > 0 ? ` +${m!.waiting.toFixed(0)} waiting` : undefined}
      sub={live && !compact && m!.reqPerS > 0 ? `${m!.reqPerS.toFixed(m!.reqPerS >= 1 ? 1 : 2)} req/s done` : null}
      trend={live ? last(h?.running, n) : undefined} color="var(--m-power)" title="Requests being served now, and requests queued for a slot or KV-cache room" />,
    <Cell key="kv" label="KV cache" info={!compact && <HelpTip topic="ai.card.kv-cache" />} value={kv != null ? kv.toFixed(kv < 10 ? 1 : 0) : dash} unit="%" max={100}
      sub={kv != null && !compact && m!.prefixHitPct != null ? `${m!.prefixHitPct.toFixed(0)}% prefix hits` : null}
      trend={kv != null ? last(h?.kvPct, n) : undefined} color="var(--m-mem)"
      title={e.engine === "ollama" || e.engine === "llamacpp" ? `${ENGINE_LABEL[e.engine]} publishes no KV-cache gauge` : "Share of the KV cache in use. Near 100% new requests wait."} />,
    <Cell key="ttft" label={compact ? "TTFT p50" : "TTFT p50 · p95"} info={!compact && <HelpTip topic="ai.card.ttft" />} value={ttft} sub={compact ? null : ttft95}
      trend={live && !compact ? last(h?.ttftP50Ms, n).map((v) => v ?? 0) : undefined} color="var(--m-ttft)"
      title={m?.ttftP50Range ? "Time to first token over the last minute. The engine's histogram only says which bucket it fell in, so that's the range shown." : "Time to first token, median and 95th percentile over the last minute"} />,
    !compact && (
      <Cell key="itl" label="ITL p50" info={<HelpTip topic="ai.card.itl" />} value={itl} sub={perStream} color="var(--m-itl)"
        title={m?.itlP50Range ? "Inter-token latency over the last minute: the histogram bucket it fell in" : "Inter-token latency, median over the last minute"} />
    ),
  ];
  return <div className={`ai-cells${compact ? " compact" : ""}`}>{cells}</div>;
}

/** "Answered 12 s ago · 1 token in 84 ms", "Generating · live traffic", or the Ready note. */
function ProofLine({ e }: { e: Endpoint }) {
  const now = useNow(e.health === "ready");
  const p = proofLine(e, now);
  if (!p) return null;
  // One icon on a card: a SparkBox load has it on its readiness line, below.
  return (
    <div className="ai-card-line">
      <div className={`ai-card-proof${p.note ? " note" : ""}`} title={p.title}>{p.text}</div>
      {!readinessLine(e) && <HelpTip topic="tell-if-a-model-is-working" />}
    </div>
  );
}

function SupervisorLine({ e }: { e: Endpoint }) {
  const s = e.supervisor;
  const now = useNow(!!s?.nextRetryMs);
  if (!s?.enabled || s.state === "gave-up") return null;
  const times = s.restarts === 1 ? "once" : `${s.restarts} times`;
  const held = s.held ? ` · ${s.held}` : "";
  const text = s.state === "restarting" && s.nextRetryMs
    ? `Keep running${held} · restarting in ${countdown(s.nextRetryMs - now)}${s.restarts ? ` · restarted ${times} in 30 min` : ""}`
    : `Keep running${held}${s.restarts ? ` · restarted ${times} in 30 min` : ""}`;
  return <div className="ai-card-sup muted">{text}</div>;
}

/** A SparkBox load: what its readiness check proved, and which native APIs its engine serves. */
function ReadinessLines({ e }: { e: Endpoint }) {
  const ready = readinessLine(e);
  const native = e.managedBy === "sparkbox" ? protocolsLine(e) : null;
  if (!ready && !native) return null;
  return (
    <>
      {ready && (
        <div className="ai-card-line">
          <div className="ai-card-proof" title="Checked when the load started: it answered, not just /health">{ready}</div>
          <HelpTip topic="tell-if-a-model-is-working" />
        </div>
      )}
      {native && <div className="ai-card-sup muted" title="Read once from the engine's /openapi.json when the load became Ready">{native}</div>}
    </>
  );
}

/** "On disk: gemma4:latest · 8.9 GiB": what an Ollama has to load (none of it in memory while idle). */
function OllamaDisk({ e }: { e: Endpoint }) {
  if (e.engine !== "ollama" || !e.onDisk) return null;
  if (!e.onDisk.length) return <div className="ai-card-disk muted">No models on disk</div>;
  return (
    <div className="ai-card-disk">
      <span className="muted">On disk</span>
      {e.onDisk.map((name, i) => (
        <span key={name}>
          <span className="mono">{name}</span>
          {e.onDiskBytes?.[i] ? <span className="muted num"> · {fmtBytes(e.onDiskBytes[i])}</span> : null}
        </span>
      ))}
    </div>
  );
}

function OllamaModels({ e }: { e: Endpoint }) {
  if (!e.ollama?.length || e.health !== "ready") return null;
  return (
    <div className="ai-card-ollama">
      <span className="muted">In memory</span>
      {e.ollama.map((m) => (
        <span key={m.name} className="ai-ollama-model" title={m.contextLength ? `Context ${m.contextLength.toLocaleString()} tokens` : undefined}>
          <span className="mono">{m.name}</span>
          <span className="muted num"> · {fmtBytes(m.vramBytes)}</span>
          {m.expiresAt && <span className="muted"> · unloads <Ago ts={m.expiresAt} /></span>}
        </span>
      ))}
    </div>
  );
}

/** The title: its name (endpointLabel), never an id. Ollama's models in memory have their own line. */
function title(e: Endpoint): string {
  return endpointLabel(e);
}

export function EndpointCard({ endpoint: e, variant = "full", selected, onSelect }: {
  endpoint: Endpoint;
  variant?: "full" | "compact";
  selected?: boolean;
  onSelect?: () => void;
}) {
  const a = useEndpointActions();
  const adminNow = useAdminState();
  // Every change here follows the lock rule (admin.ts lockOf).
  const lock = lockOf(!adminNow.unlocked, adminNow);
  const wm = useWM();
  const v = endpointView(e);
  const name = title(e);
  const proof = proofLine(e, Date.now());
  const where = [split(e), e.port ? `:${e.port}` : null].filter(Boolean).join(" · ");
  const stateClass = v.tone === "crit" ? " ai-card-crit" : v.tone === "off" ? " ai-card-off" : v.tone === "paused" ? " ai-card-paused" : v.tone === "idle" ? " ai-card-idle" : "";
  const reach = reachLine(e);
  // "Loading" next to "Loading 42%" says nothing; any other reason is worth its line.
  const reason = v.reason && !(v.tone === "progress" && /^Loading( \d+%)?$/.test(v.reason)) && (
    <div className={`ai-card-reason${v.tone === "crit" ? " crit" : ""}`} title={v.reason}>
      {v.reason}
    </div>
  );

  if (variant === "compact") {
    return (
      <button
        type="button"
        className={`ai-card compact${stateClass}`}
        onClick={() => wm.open("endpoints", { id: e.id, nav: String(Date.now()) })}
        onContextMenu={(ev) => a.menu(ev, e)}
        title={proof ? `${proof.text}. Open in Endpoints` : "Open in Endpoints"}
        aria-label={`${name}, ${v.word}${v.reason ? `: ${v.reason}` : ""}`}
      >
        <header className="ai-card-head">
          <span className="ai-card-engine">{ENGINE_LABEL[e.engine]}</span>
          <Truncate text={name} className="ai-card-model" />
          <HealthPill e={e} />
        </header>
        <div className="ai-card-where">
          <Nodes e={e} />
          {where && <span className="muted">{where}</span>}
        </div>
        {reason}
        {reach && <div className="ai-card-reach" title={reach}>Reachable from {e.headNode} only</div>}
        {e.health !== "idle" && <Cells e={e} compact />}
      </button>
    );
  }

  const model = e.engine === "ollama" ? e.ollama?.[0]?.name : e.models[0];
  const canChat = !!e.providerId && e.health === "ready" && !!model;
  const restartable = canRestart(e);
  const down = !!e.forgettable;
  const log = logTarget(e);
  return (
    <article
      id={`endpoint-${e.id}`}
      className={`ai-card${selected ? " sel" : ""}${stateClass}`}
      onClick={onSelect}
      onContextMenu={(ev) => a.menu(ev, e)}
      aria-label={`${name}, ${v.word}${v.reason ? `: ${v.reason}` : ""}`}
    >
      <header className="ai-card-head">
        <span className="ai-card-engine">{ENGINE_LABEL[e.engine]}</span>
        <Truncate text={name} className="ai-card-model strong" />
        <HealthPill e={e} />
        <span className="spacer" />
        <span className="ai-card-where">
          <Nodes e={e} />
          {where && <span className="muted">{where}</span>}
          {e.maxContext != null && <span className="muted" title="Longest request it accepts">{fmtTokens(e.maxContext)} context</span>}
          <span className="muted" title={e.managedBy === "sparkbox" ? "Loaded from Models: Unload removes exactly its containers" : e.source === "manual" ? "Added by hand in Providers" : "Found running on the cluster; SparkBox didn't start it"}>
            {e.managedBy === "sparkbox" ? "SparkBox" : e.source === "manual" ? "Added by hand" : "External"}
          </span>
        </span>
      </header>
      {reason}
      <ProofLine e={e} />
      <SupervisorLine e={e} />
      <ReadinessLines e={e} />
      <div className="ai-card-url">
        <span className="mono">{e.url}</span>
        <button type="button" className="ai-icon-btn" onClick={(ev) => { ev.stopPropagation(); copy(e.url, "URL"); }} title="Copy the URL" aria-label="Copy the URL">
          <Icon icon={Copy} size={12} />
        </button>
        {e.listen === "lan" && <span className="muted">LAN only</span>}
      </div>
      {reach && <div className="ai-card-reach">{reach}</div>}
      {e.health !== "idle" && <Cells e={e} />}
      <OllamaModels e={e} />
      <OllamaDisk e={e} />
      <footer className="ai-card-actions" onClick={(ev) => ev.stopPropagation()}>
        <button type="button" className={`btn small${canChat ? " primary" : ""}`} disabled={!canChat} onClick={() => model && e.providerId && a.chat(e.providerId, model)}
          title={canChat ? `New chat with ${model}` : reach ? `Reachable from ${e.headNode} only` : e.health === "idle" ? "Nothing in memory to chat with" : e.health !== "ready" ? "Not ready" : "Not reachable from this Mac"}>
          <Icon icon={MessageSquare} size={12} />Chat with this
        </button>
        <button type="button" className="btn small" onClick={() => a.copyCurl(e)}><Icon icon={Terminal} size={12} />Copy curl</button>
        <button type="button" className="btn small" disabled={!log} onClick={() => a.logs(e)} title={log ? `${log.what} on ${e.headNode}` : "Runs as a service SparkBox can't name"}>
          <Icon icon={SquareTerminal} size={12} />Logs
        </button>
        {e.engine === "ollama" && (
          <button type="button" className="btn small" onClick={() => wm.open("ollama", { endpoint: e.id })} title="Pull, list and delete Ollama's models">
            <Icon icon={Boxes} size={12} />Models…
          </button>
        )}
        <button type="button" className="btn small" onClick={() => a.history(e)} title="Tokens per second, latency, test tokens, queue and cache over the last hour to 14 days, with its nodes' GPUs and fabric">
          <Icon icon={History} size={12} />History
        </button>
        <span className="spacer" />
        {e.managedBy === "sparkbox" && (
          <button type="button" className="btn small" onClick={() => a.edit(e)} title="Change its settings and load it again">
            <Icon icon={Pencil} size={12} />Edit & reload…
          </button>
        )}
        {restartable && (
          <ChangeButton lock={lock} icon={RotateCcw} label="Restart…" onClick={() => a.restart(e)}
            title={e.nodes.length > 1 ? "Stop and start it in order, then check it answers" : "Stop and start it, then check it answers"} />
        )}
        {e.managedBy === "sparkbox" ? (
          <ChangeButton lock={lock} icon={PowerOff} label="Unload…" onClick={() => a.unload(e)} title="Stop its containers on every node" />
        ) : down ? (
          <ChangeButton lock={lock} icon={Trash2} label="Forget" onClick={() => a.forget(e)} title="Stop listing it here (it's listed again if it starts)" />
        ) : e.containerIds.length ? (
          <ChangeButton lock={lock} icon={StopIcon} label="Stop containers…" onClick={() => a.stop(e)} title="Stop the containers behind it" />
        ) : null}
      </footer>
    </article>
  );
}
