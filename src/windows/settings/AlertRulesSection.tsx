// Settings › Alerts: every alert rule as data. Thresholds are edited in place, with the
// rule's hold ("For") and "Would fire now" on a second line under its name, per-node
// overrides in a popover, and a rule that watches several counters (RoCE on a fabric
// interface) opens its counter lines below it. "Would fire now" judges the edited rules
// against the cluster as it is this second (rules_preview) before anything is saved.
// Saving needs Admin on this Mac; it's audited and pushed to the node agents.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, RotateCcw, Save, SlidersHorizontal } from "lucide-react";
import { HelpTip, InfoTip, Notice, SectionTitle, StatusDot, Switch, type DotStatus } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { inTauri } from "../../lib/api";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";
import { confirmAction } from "../../lib/session";
import "../alerts.css";

/** Mirrors health::rule::Rule (src-tauri/src/health/rule.rs). */
export interface Rule {
  id: string;
  area: string;
  title: string;
  entity: "node" | "net" | "endpoint" | "container" | "port" | "cluster";
  kind: "threshold" | "rate" | "event" | "coded";
  metric: string;
  op: ">" | "<" | "abs>";
  warn: number | null;
  crit: number | null;
  clear: number | null;
  forS: number;
  critForS: number | null;
  clearS: number;
  enabled: boolean;
  overrides: Record<string, { warn?: number | null; crit?: number | null; enabled?: boolean | null }>;
  message: string;
  unit: string;
  windowS: number | null;
  notify: boolean;
  help: string;
  /** Per-counter lines (roce-iface): per second, raised past warn, critical past crit. */
  counters?: CounterLine[];
}

export interface CounterLine {
  key: string;
  label: string;
  warn: number | null;
  crit: number | null;
  enabled: boolean;
}

export interface RuleSet {
  version: number;
  rules: Rule[];
}

interface PreviewMatch {
  label: string;
  severity: "info" | "warn" | "crit";
  message: string;
  value: number | null;
  heldS: number;
  firesInS: number;
}

interface PreviewRow {
  id: string;
  known: boolean;
  matches: PreviewMatch[];
  /** Nodes whose data lacks this node rule's reading, by name. */
  unreported?: string[];
}

export const AREAS: { id: string; label: string; help: string }[] = [
  { id: "node", label: "Nodes", help: "mon.rules.nodes" },
  { id: "disk", label: "Disk", help: "mon.rules.disk" },
  { id: "gpu", label: "GPU", help: "mon.rules.gpu" },
  { id: "fabric", label: "Fabric", help: "mon.rules.fabric" },
  { id: "containers", label: "Containers & services", help: "mon.rules.containers" },
  { id: "serving", label: "Serving", help: "mon.rules.serving" },
];

/** "60 s", "2 min", "10 min", "1 h", "24 h". */
export function fmtDur(s: number): string {
  if (s <= 0) return "at once";
  if (s < 60 || s % 60 !== 0) return `${s} s`;
  if (s < 3600 || s % 3600 !== 0) return `${s / 60} min`;
  return `${s / 3600} h`;
}

/** "60 s" / "2 min" / "1 h" / "90" → seconds, or null. */
export function parseDur(text: string): number | null {
  const m = text.trim().toLowerCase().match(/^(\d+(?:\.\d+)?)\s*(s|sec|secs|second|seconds|m|min|mins|minute|minutes|h|hr|hrs|hour|hours)?$/);
  if (!m) return null;
  const n = Number(m[1]);
  const unit = m[2] ?? "s";
  const mult = unit.startsWith("h") ? 3600 : unit.startsWith("m") ? 60 : 1;
  return Math.round(n * mult);
}

/** Whether a rule has lines the owner can move (coded rules have durations only). */
function hasLines(r: Rule): boolean {
  return r.warn !== null || r.crit !== null;
}

/** A rule whose lines are per counter (RoCE on a fabric interface). */
const hasCounters = (r: Rule) => !!r.counters?.length;

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

function same(a: Rule, b: Rule | undefined): boolean {
  return !!b && JSON.stringify(a) === JSON.stringify(b);
}

export function AlertRulesSection() {
  const [saved, setSaved] = useState<RuleSet | null>(null);
  const [draft, setDraft] = useState<RuleSet | null>(null);
  const [preview, setPreview] = useState<Record<string, PreviewRow>>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [overrideFor, setOverrideFor] = useState<{ id: string; anchor: DOMRect } | null>(null);
  const [countersOpen, setCountersOpen] = useState<Set<string>>(new Set());
  const cluster = useCluster();
  const nodes = useMemo(() => (cluster?.nodes ?? []).map((n) => ({ id: n.id, name: n.name })), [cluster?.nodes]);

  useEffect(() => {
    if (!inTauri) return;
    invoke<RuleSet>("rules_get").then((r) => {
      setSaved(r);
      setDraft(clone(r));
    }, (e) => setError(String(e)));
  }, []);

  // "Would fire now": the draft against the current data, a moment after each edit and every 5 s.
  const key = JSON.stringify(draft);
  useEffect(() => {
    if (!draft || !inTauri) return;
    let alive = true;
    const run = () =>
      invoke<PreviewRow[]>("rules_preview", { rules: draft })
        .then((rows) => alive && setPreview(Object.fromEntries(rows.map((r) => [r.id, r]))))
        .catch(() => {});
    const first = window.setTimeout(run, 250);
    const every = window.setInterval(run, 5000);
    return () => {
      alive = false;
      window.clearTimeout(first);
      window.clearInterval(every);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const update = (id: string, patch: Partial<Rule>) => {
    setDraft((d) => (d ? { ...d, rules: d.rules.map((r) => (r.id === id ? { ...r, ...patch } : r)) } : d));
  };

  const changed = useMemo(() => {
    if (!draft || !saved) return [];
    const before = new Map(saved.rules.map((r) => [r.id, r]));
    return draft.rules.filter((r) => !same(r, before.get(r.id)));
  }, [draft, saved]);

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      const r = await withAdmin(() => invoke<RuleSet>("rules_set", { rules: draft }), { scope: "mac", action: "rules_set" });
      setSaved(r);
      setDraft(clone(r));
      setError(null);
      toast({ kind: "success", title: `Saved ${changed.length === 1 ? changed[0].title : `${changed.length} rules`}`, body: "Node agents get the new rules too." });
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setSaving(false);
    }
  };

  const reset = async () => {
    const ok = await confirmAction({ title: "Reset every rule to its default?", body: "Thresholds, durations, switches and per-node overrides go back to how SparkBox ships them. Nothing is saved until you choose Save.", confirmLabel: "Reset" });
    if (!ok) return;
    try {
      setDraft(await invoke<RuleSet>("rules_defaults"));
    } catch (e) {
      setError(String(e));
    }
  };

  // Every match the rows list under "Would fire now", whether at once or after its For time.
  const firing = Object.values(preview).reduce((n, r) => n + r.matches.filter((m) => m.severity !== "info").length, 0);
  return (
    <>
      <SectionTitle
        title="Alert rules"
        infoTopic="mon.rules.about"
      />
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <div className="ar-toolbar">
        <button type="button" className="btn primary" disabled={!changed.length || saving} onClick={save}>
          <Save size={13} aria-hidden /> {saving ? "Saving…" : changed.length ? `Save ${changed.length} change${changed.length === 1 ? "" : "s"}` : "Saved"}
        </button>
        <button type="button" className="btn" disabled={!changed.length || saving} onClick={() => saved && setDraft(clone(saved))}>Revert</button>
        <span className="spacer" />
        <span className="muted" title="What these rules raise against the cluster as it is now, at once or after their For time">{draft ? `Would fire now: ${firing}` : ""}</span>
        {draft && <HelpTip topic="mon.rules.would-fire" />}
        <button type="button" className="btn" disabled={!draft} onClick={reset}><RotateCcw size={13} aria-hidden /> Reset to defaults…</button>
      </div>
      {!draft && !error && <p className="muted">Loading the rules…</p>}
      {draft && AREAS.map((area) => {
        const rules = draft.rules.filter((r) => (r.area === area.id) || (area.id === "fabric" && !AREAS.some((a) => a.id === r.area)));
        if (!rules.length) return null;
        return (
          <section key={area.id} className="form-group">
            <h3 className="section-title">{area.label} <InfoTip topic={area.help} label={`About ${area.label}`} /></h3>
            <div className="form-group-card">
              <table className="ar-table">
                <colgroup>
                  <col style={{ width: 44 }} />
                  <col />
                  <col style={{ width: 108 }} />
                  <col style={{ width: 108 }} />
                  <col style={{ width: 104 }} />
                </colgroup>
                <thead>
                  <tr>
                    <th aria-label="Enabled" />
                    <th>Rule</th>
                    <th>Warning</th>
                    <th>Critical</th>
                    <th aria-label="Per-node overrides">{area.id === "node" && <HelpTip topic="mon.rules.per-node" />}</th>
                  </tr>
                </thead>
                <tbody>
                  {rules.map((r, i) => (
                    <RuleRow
                      key={r.id}
                      r={r}
                      first={area.id === "node" && i === 0}
                      changed={!same(r, saved?.rules.find((x) => x.id === r.id))}
                      preview={preview[r.id]}
                      onChange={(patch) => update(r.id, patch)}
                      onOverrides={(anchor) => setOverrideFor({ id: r.id, anchor })}
                      countersOpen={countersOpen.has(r.id)}
                      onToggleCounters={() => setCountersOpen((s) => {
                        const next = new Set(s);
                        if (next.has(r.id)) next.delete(r.id);
                        else next.add(r.id);
                        return next;
                      })}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        );
      })}
      {overrideFor && draft && (
        <Overrides
          rule={draft.rules.find((r) => r.id === overrideFor.id)!}
          unreported={preview[overrideFor.id]?.unreported ?? []}
          nodes={nodes}
          anchor={overrideFor.anchor}
          onChange={(overrides) => update(overrideFor.id, { overrides })}
          onClose={() => setOverrideFor(null)}
        />
      )}
    </>
  );
}

function NumberField({ value, onChange, unit, label, disabled }: { value: number | null; onChange: (v: number | null) => void; unit: string; label: string; disabled?: boolean }) {
  const [text, setText] = useState(value === null ? "" : String(value));
  useEffect(() => setText(value === null ? "" : String(value)), [value]);
  return (
    <span className="ar-num">
      <input
        className="field"
        inputMode="decimal"
        aria-label={label}
        value={text}
        disabled={disabled}
        placeholder="—"
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const t = text.trim();
          if (t === "") return onChange(null);
          const n = Number(t);
          if (Number.isFinite(n)) onChange(n);
          else setText(value === null ? "" : String(value));
        }}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
      {unit && <span className="unit">{unit}</span>}
    </span>
  );
}

function DurationField({ value, onChange, label, disabled }: { value: number; onChange: (v: number) => void; label: string; disabled?: boolean }) {
  const [text, setText] = useState(fmtDur(value));
  useEffect(() => setText(fmtDur(value)), [value]);
  return (
    <span className="ar-num ar-dur">
      <input
        className="field"
        aria-label={label}
        value={text}
        disabled={disabled}
        onFocus={() => setText(value <= 0 ? "0" : fmtDur(value))}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const s = text.trim() === "at once" ? 0 : parseDur(text);
          if (s !== null) onChange(s);
          setText(fmtDur(s ?? value));
        }}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
    </span>
  );
}

const sevDot = (s: PreviewMatch["severity"]): DotStatus => (s === "info" ? "unconfigured" : s);

/** "Would fire now", on the rule's second line: blank while nothing would. */
function WouldFire({ p, rule }: { p?: PreviewRow; rule: Rule }) {
  if (!rule.enabled || !p) return null;
  const missing = p.unreported ?? [];
  if (!p.known && !p.matches.length) {
    // Unknown is grey and says why, never "No data" or 0.
    const who = missing.length === 1 ? missing[0] : rule.entity === "node" ? "any node" : rule.entity === "port" ? "the switch" : rule.entity === "endpoint" ? "any model server" : "the cluster";
    return <span className="quiet" title={`Nothing in the current data has this reading (${rule.metric}), so this rule can't fire${missing.length > 1 ? ` on ${missing.join(", ")}` : ""}. Per node… shows which.`}>Not reported by {who}</span>;
  }
  if (!p.matches.length) {
    // Some nodes report it and some don't: say which don't.
    return missing.length ? <span className="quiet" title={`These nodes' data has no ${rule.metric}, so this rule can't fire there.`}>Not reported by {missing.join(", ")}</span> : null;
  }
  const shown = p.matches.slice(0, 2);
  return (
    <span className="ar-fire">
      <span className="quiet">Would fire now:</span>
      {shown.map((m, i) => (
        <span key={i} title={m.message}>
          <StatusDot status={sevDot(m.severity)} text={`${m.label}${m.firesInS > 0 ? ` · in ${fmtDur(m.firesInS)}` : ""}`} />
        </span>
      ))}
      {p.matches.length > shown.length && <span className="quiet">and {p.matches.length - shown.length} more</span>}
    </span>
  );
}

function RuleRow({ r, first, changed, preview, onChange, onOverrides, countersOpen, onToggleCounters }: {
  r: Rule;
  /** The first row of the first area carries the Help tip for For. */
  first: boolean;
  changed: boolean;
  preview?: PreviewRow;
  onChange: (patch: Partial<Rule>) => void;
  onOverrides: (anchor: DOMRect) => void;
  countersOpen: boolean;
  onToggleCounters: () => void;
}) {
  const lines = hasLines(r);
  const counters = hasCounters(r);
  const overrides = Object.keys(r.overrides).length;
  const setCounter = (key: string, patch: Partial<CounterLine>) => onChange({ counters: r.counters?.map((c) => (c.key === key ? { ...c, ...patch } : c)) });
  return (
    <>
      <tr className={`${r.enabled ? "" : "off"}${changed ? " changed" : ""}`}>
        <td><Switch label={`${r.title} on`} checked={r.enabled} onChange={(on) => onChange({ enabled: on })} /></td>
        <td>
          <div className="ar-title">
            <span title={r.message}>
              {r.title}
              <InfoTip topic={`mon.rule.${r.id}`} label={`About ${r.title}`} />
            </span>
          </div>
          <div className="ar-sub">
            <span className="ar-for">
              <span className="quiet">For</span>
              <DurationField value={r.forS} onChange={(v) => onChange({ forS: v })} label={`${r.title}: how long before it fires`} />
              {first && <HelpTip topic="mon.rules.for" />}
            </span>
            {counters && (
              <button type="button" className="btn small ar-counters-toggle" aria-expanded={countersOpen} onClick={onToggleCounters}>
                {countersOpen ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />} {r.counters!.length} counters
              </button>
            )}
            {counters && <HelpTip topic="mon.rules.counters" label="About these counters" />}
            <WouldFire p={preview} rule={r} />
          </div>
        </td>
        {counters ? (
          <td colSpan={2}><span className="quiet">Per counter</span></td>
        ) : (
          <>
            <td>{lines ? <NumberField value={r.warn} onChange={(v) => onChange({ warn: v })} unit={r.unit} label={`${r.title} warning line`} /> : <span className="ar-dash">—</span>}</td>
            <td>{lines ? <NumberField value={r.crit} onChange={(v) => onChange({ crit: v })} unit={r.unit} label={`${r.title} critical line`} /> : <span className="ar-dash">—</span>}</td>
          </>
        )}
        <td>
          <button type="button" className="btn small ar-pernode" aria-label={`Per node overrides for ${r.title}${overrides ? ` (${overrides})` : ""}`}
            title={overrides ? `${overrides} node${overrides === 1 ? "" : "s"} differ` : "Different lines or off on some nodes"}
            onClick={(e) => onOverrides((e.currentTarget as HTMLElement).getBoundingClientRect())}>
            <SlidersHorizontal size={12} aria-hidden /> Per node…{overrides ? ` (${overrides})` : ""}
          </button>
        </td>
      </tr>
      {counters && countersOpen && r.counters!.map((c) => (
        <tr key={c.key} className={`ar-counter${r.enabled && c.enabled ? "" : " off"}`}>
          <td><Switch label={`${c.label} counts`} checked={c.enabled} onChange={(on) => setCounter(c.key, { enabled: on })} /></td>
          <td><span className="ar-counter-name">{c.label}</span></td>
          <td><NumberField value={c.warn} onChange={(v) => setCounter(c.key, { warn: v })} unit={r.unit} label={`${c.label} warning line`} /></td>
          <td><NumberField value={c.crit} onChange={(v) => setCounter(c.key, { crit: v })} unit={r.unit} label={`${c.label} critical line`} /></td>
          <td />
        </tr>
      ))}
    </>
  );
}

function Overrides({ rule, unreported, nodes, anchor, onChange, onClose }: {
  rule: Rule;
  unreported: string[];
  nodes: { id: string; name: string }[];
  anchor: DOMRect;
  onChange: (o: Rule["overrides"]) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.max(8, Math.min(anchor.right - w, window.innerWidth - w - 8));
    const top = anchor.bottom + 6 + h > window.innerHeight - 8 ? Math.max(8, anchor.top - h - 6) : anchor.bottom + 6;
    setPos({ left, top });
  }, [anchor]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const onDown = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && onClose();
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [onClose]);
  const set = (node: string, patch: { warn?: number | null; crit?: number | null; enabled?: boolean | null }) => {
    const next = { ...rule.overrides, [node]: { ...rule.overrides[node], ...patch } };
    const o = next[node];
    if ((o.warn ?? null) === null && (o.crit ?? null) === null && (o.enabled ?? null) === null) delete next[node];
    onChange(next);
  };
  const lines = hasLines(rule);
  return createPortal(
    <div ref={ref} className="ar-pop" role="dialog" aria-label={`${rule.title}: per-node overrides`} style={pos ?? { visibility: "hidden", left: 0, top: 0 }}>
      <h4>{rule.title} per node</h4>
      <div className="ar-overrides" style={lines ? undefined : { gridTemplateColumns: "max-content auto" }}>
        <span className="head">Node</span>
        {lines && <span className="head">Warning</span>}
        {lines && <span className="head">Critical</span>}
        <span className="head">On</span>
        {nodes.map((n) => {
          const o = rule.overrides[n.id] ?? {};
          return (
            <OverrideRow key={n.id} name={n.name} unreported={unreported.includes(n.name)} lines={lines} unit={rule.unit} o={o} rule={rule}
              onWarn={(v) => set(n.id, { warn: v })} onCrit={(v) => set(n.id, { crit: v })}
              onEnabled={(on) => set(n.id, { enabled: on === rule.enabled ? null : on })} />
          );
        })}
      </div>
      <div className="ar-pop-actions">
        <button type="button" className="btn" disabled={!Object.keys(rule.overrides).length} onClick={() => onChange({})}>Clear all</button>
        <button type="button" className="btn primary" onClick={onClose}>Done</button>
      </div>
    </div>,
    document.body,
  );
}

function OverrideRow({ name, unreported, lines, unit, o, rule, onWarn, onCrit, onEnabled }: {
  name: string;
  /** Its data has no reading for this rule. */
  unreported: boolean;
  lines: boolean;
  unit: string;
  o: { warn?: number | null; crit?: number | null; enabled?: boolean | null };
  rule: Rule;
  onWarn: (v: number | null) => void;
  onCrit: (v: number | null) => void;
  onEnabled: (on: boolean) => void;
}) {
  return (
    <>
      <span className="ar-node">
        {name}
        {unreported && <span className="quiet" title={`${name}'s data has no ${rule.metric}, so this rule can't fire there.`}>Not reported by this node</span>}
      </span>
      {lines && <NumberField value={o.warn ?? null} onChange={onWarn} unit={unit} label={`${name} warning line (default ${rule.warn ?? "none"})`} />}
      {lines && <NumberField value={o.crit ?? null} onChange={onCrit} unit={unit} label={`${name} critical line (default ${rule.crit ?? "none"})`} />}
      <Switch label={`${rule.title} on ${name}`} checked={o.enabled ?? rule.enabled} onChange={onEnabled} />
    </>
  );
}
