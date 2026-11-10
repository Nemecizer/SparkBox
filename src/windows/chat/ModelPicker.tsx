// One searchable list of every model SparkBox can reach, grouped Cluster,
// Anthropic, OpenAI and Demo, recent ones first. Cluster models show whether
// they're ready (or how far loading has got) and how long a context they take.
// One that stopped answering in the last two minutes stays listed, grayed.
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { ChevronDown } from "lucide-react";
import { Icon } from "../../components/Icon";
import { HelpTip, StatusDot } from "../../components/ui";
import { migrateLegacy, scopedKey } from "../../lib/dataSource";
import { endpointLabel, endpointView, useEndpoints, type Endpoint } from "../../lib/endpoints";
import type { ProviderView } from "../../lib/modelsApi";
import { fmtTokens } from "../models/shared";
import "../ai.css";

export interface ModelChoice {
  provider: string;
  model: string;
}

type Group = "Recent" | "Cluster" | "Anthropic" | "OpenAI" | "Demo" | "Other";

interface Item extends ModelChoice {
  key: string;
  group: Group;
  providerName: string;
  endpoint?: Endpoint;
  /** Not usable right now, and why. */
  disabled?: string;
  noKey?: boolean;
  /** Its endpoint left Ready a moment ago (llm GRACE_MS). */
  quiet?: boolean;
}

export const NOT_ANSWERING = "Not answering";

const RECENT_KEY = "sparkbox.chat.recent";
// Kept per data source: a model picked with Demo data isn't offered first with Live data.
migrateLegacy(RECENT_KEY);

/** The models picked lately with the current data source, newest first. */
export function recentChoices(): ModelChoice[] {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(RECENT_KEY)) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => x && typeof x.provider === "string" && typeof x.model === "string") : [];
  } catch {
    return [];
  }
}

export function rememberChoice(c: ModelChoice) {
  try {
    const next = [c, ...recentChoices().filter((x) => x.provider !== c.provider || x.model !== c.model)].slice(0, 5);
    localStorage.setItem(scopedKey(RECENT_KEY), JSON.stringify(next));
  } catch {
    // A convenience only.
  }
}

function groupOf(p: ProviderView): Group {
  if (p.kind === "anthropic") return "Anthropic";
  if (p.kind === "openai") return "OpenAI";
  if (p.discovered || p.id.startsWith("vllm-") || p.kind === "openaiCompatible") return "Cluster";
  if (p.kind === "demo") return "Demo";
  return "Other";
}

const ORDER: Group[] = ["Recent", "Cluster", "Anthropic", "OpenAI", "Demo", "Other"];

/** Every choice the picker offers, for the Server window's list too. */
export function useModelItems(providers: ProviderView[], withRecent = true): Item[] {
  const endpoints = useEndpoints();
  return useMemo(() => {
    const items: Item[] = [];
    const byProvider = new Map(endpoints.filter((e) => e.providerId).map((e) => [e.providerId!, e]));
    for (const p of providers.filter((x) => x.enabled)) {
      const group = groupOf(p);
      const noKey = (p.kind === "anthropic" || p.kind === "openai") && !p.hasKey;
      const quiet = !!p.notAnswering;
      for (const m of p.visible) {
        items.push({ key: `${p.id}/${m}`, provider: p.id, model: m, group, providerName: p.name, endpoint: byProvider.get(p.id), noKey, quiet, disabled: quiet ? NOT_ANSWERING : undefined });
      }
    }
    // Cluster loads still starting: listed, not pickable yet. The row already shows the model, so
    // its second name is the engine and node ("vLLM on cell2"), not the label, which for vLLM is the model too.
    for (const e of endpoints) {
      if (e.health === "ready" || (e.providerId && items.some((i) => i.provider === e.providerId))) continue;
      for (const m of e.models) {
        items.push({
          key: `${e.id}/${m}`, provider: e.providerId ?? e.id, model: m, group: "Cluster", providerName: endpointLabel({ ...e, label: undefined, models: [], containerNames: [] }), endpoint: e,
          disabled: e.health === "loading" ? `Loading${e.loadingPct != null ? ` ${e.loadingPct}%` : ""}` : e.healthReason ?? (e.health === "stopped" ? "Stopped" : "Not answering"),
        });
      }
    }
    if (withRecent) {
      const recent = recentChoices()
        .map((r) => items.find((i) => i.provider === r.provider && i.model === r.model && !i.disabled))
        .filter((i): i is Item => !!i)
        .map((i) => ({ ...i, key: `recent:${i.key}`, group: "Recent" as const }));
      items.unshift(...recent);
    }
    return items.sort((a, b) => ORDER.indexOf(a.group) - ORDER.indexOf(b.group));
  }, [providers, endpoints, withRecent]);
}

function ItemStatus({ i }: { i: Item }) {
  const e = i.endpoint;
  if (i.group !== "Cluster" && !(i.group === "Recent" && e)) return null;
  if (!e) return null;
  const status = e.health === "ready" ? "online" : e.health === "loading" ? "connecting" : e.health === "stopped" ? "offline" : "error";
  const word = e.health === "loading" && e.loadingPct != null ? `${e.loadingPct}%` : endpointView(e).word;
  return (
    <span className="ai-pick-status">
      <StatusDot status={status} text={word} />
      {e.maxContext != null && <span className="muted num">{fmtTokens(e.maxContext)} ctx</span>}
    </span>
  );
}

export function ModelPicker({ providers, value, onChange, disabled, id, label = "Model" }: {
  providers: ProviderView[];
  value: ModelChoice | null;
  onChange: (c: ModelChoice) => void;
  disabled?: boolean;
  id?: string;
  label?: string;
}) {
  const items = useModelItems(providers);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listId = useId();
  // The list floats above everything (toolbars clip their contents), placed under the button.
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const r = rootRef.current?.getBoundingClientRect();
      if (!r) return;
      const width = Math.min(460, window.innerWidth * 0.8);
      setPos({ left: Math.max(8, Math.min(r.left, window.innerWidth - width - 8)), top: r.bottom + 4, maxHeight: Math.max(200, window.innerHeight - r.bottom - 16) });
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [open]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q ? items.filter((i) => i.group !== "Recent" && `${i.model} ${i.providerName} ${i.group}`.toLowerCase().includes(q)) : items;
    return list;
  }, [items, query]);
  const enabled = shown.filter((i) => !i.disabled);

  useEffect(() => {
    if (!open) return;
    setActive(Math.max(0, enabled.findIndex((i) => i.provider === value?.provider && i.model === value?.model)));
    inputRef.current?.focus();
    const close = (e: PointerEvent) => {
      if (!(e.target instanceof Node)) return;
      if (rootRef.current?.contains(e.target) || popRef.current?.contains(e.target)) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", close, true);
    return () => window.removeEventListener("pointerdown", close, true);
  }, [open]);

  useEffect(() => setActive(0), [query]);

  const current = items.find((i) => i.provider === value?.provider && i.model === value?.model && i.group !== "Recent");
  const pick = (i: Item) => {
    if (i.disabled) return;
    onChange({ provider: i.provider, model: i.model });
    rememberChoice({ provider: i.provider, model: i.model });
    setOpen(false);
    setQuery("");
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(enabled.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (enabled[active]) pick(enabled[active]);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      setOpen(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    popRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, open]);

  let lastGroup: Group | null = null;
  return (
    <div className="ai-picker" ref={rootRef}>
      <button
        type="button"
        id={id}
        className="ai-picker-button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`${label}: ${value ? `${value.model}, ${current?.providerName ?? value.provider}` : "none"}`}
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        <span className="ai-picker-model">{value?.model || "Pick a model"}</span>
        <span className="ai-picker-provider muted">{current?.providerName ?? value?.provider ?? ""}</span>
        {current?.endpoint && <ItemStatus i={current} />}
        <Icon icon={ChevronDown} size={12} />
      </button>
      {open && pos && createPortal(
        <div className="ai-picker-pop" ref={popRef} role="dialog" aria-label={`Choose a ${label.toLowerCase()}`}
          style={{ left: pos.left, top: pos.top, maxHeight: Math.min(420, pos.maxHeight) }}>
          <div className="ai-picker-head">
            <input
              ref={inputRef}
              className="ai-picker-search"
              type="search"
              placeholder="Search models"
              value={query}
              role="combobox"
              aria-expanded
              aria-controls={listId}
              aria-activedescendant={enabled[active] ? `${listId}-${active}` : undefined}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onKey}
            />
            <HelpTip topic="ai.chat.picker" />
          </div>
          <div className="ai-picker-list" role="listbox" id={listId} aria-label="Models">
            {shown.length === 0 && <div className="ai-picker-empty muted">No model matches “{query}”.</div>}
            {shown.map((i) => {
              const header = i.group !== lastGroup ? i.group : null;
              lastGroup = i.group;
              const idx = enabled.indexOf(i);
              const selected = value?.provider === i.provider && value?.model === i.model;
              return (
                <div key={i.key}>
                  {header && <div className="ai-picker-group" role="presentation">{header}</div>}
                  <div
                    id={idx >= 0 ? `${listId}-${idx}` : undefined}
                    data-index={idx}
                    role="option"
                    aria-selected={selected}
                    aria-disabled={!!i.disabled}
                    className={`ai-picker-item${idx === active ? " active" : ""}${selected ? " current" : ""}${i.disabled ? " disabled" : ""}`}
                    onPointerEnter={() => idx >= 0 && setActive(idx)}
                    onClick={() => pick(i)}
                    title={i.disabled ?? (i.noKey ? `Add the ${i.providerName} API key in AI › Providers first` : undefined)}
                  >
                    <span className="ai-picker-item-model">{i.model}</span>
                    <span className="ai-picker-item-provider muted">{i.providerName}{i.noKey ? " · no key" : ""}</span>
                    <ItemStatus i={i} />
                    {i.disabled && (!i.endpoint || i.quiet) && <span className="muted">{i.disabled}</span>}
                  </div>
                </div>
              );
            })}
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}
