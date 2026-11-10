// Every inference server on the cluster in one place, whoever started it, with
// its health (from evidence: nodes, containers, probes and a one-token test) and
// live tokens per second, queue, KV cache and latency. Each one's History opens
// in place: the last hour to 14 days, with its nodes' GPUs and fabric beside it.
import { useEffect, useMemo, useState } from "react";
import { Play, RefreshCw } from "lucide-react";
import { HelpTip, SearchField, Segmented, Spacer, Tile, Tiles, Toolbar, ToolButton } from "../components/ui";
import { endpointLabel, refreshEndpoints, useEndpoints, type Engine } from "../lib/endpoints";
import { toast } from "../lib/toasts";
import { useWM, useWindowEvent, useWinState, type Params } from "../wm/WindowManager";
import { OpenHistoryContext } from "./models/EndpointCard";
import { EndpointHistory } from "./models/EndpointHistory";
import { EndpointList } from "./models/EndpointList";
import { ENGINE_LABEL, errText, fmtTps } from "./models/shared";

type Filter = "all" | "managed" | "external" | Engine;

/** params.id: an endpoint to show first (from the Dashboard's serving cards); params.view "history": its History. */
export function EndpointsWindow({ params }: { params: Params }) {
  const all = useEndpoints();
  const [focus, setFocus] = useState<string | null>(params.id ?? null);
  const [historyOf, setHistoryOf] = useWinState<string | null>("historyOf", params.view === "history" ? params.id ?? null : null);
  useEffect(() => {
    if (params.id) setFocus(params.id);
    if (params.view === "history" && params.id) setHistoryOf(params.id);
  }, [params.id, params.view, params.nav]);
  useEffect(() => {
    if (focus) document.getElementById(`endpoint-${focus}`)?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [focus, all.length]);
  const wm = useWM();
  const [filter, setFilter] = useWinState<Filter>("filter", "all");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    setBusy(true);
    try {
      await refreshEndpoints();
    } catch (e) {
      toast({ kind: "error", title: "Couldn't check the endpoints", body: errText(e) });
    } finally {
      setBusy(false);
    }
  };
  useWindowEvent("refresh", () => void refresh());

  const engines = useMemo(() => [...new Set(all.map((e) => e.engine))].filter((e) => e !== "other").sort(), [all]);
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return all.filter((e) => {
      if (filter === "managed" && e.managedBy !== "sparkbox") return false;
      if (filter === "external" && e.managedBy !== "external") return false;
      if (!["all", "managed", "external"].includes(filter) && e.engine !== filter) return false;
      if (!q) return true;
      return [endpointLabel(e), e.url, e.engine, ...e.models, ...e.nodes].some((x) => x.toLowerCase().includes(q));
    });
  }, [all, filter, query]);

  const ready = all.filter((e) => e.health === "ready");
  const down = all.filter((e) => e.health === "down");
  const loading = all.filter((e) => e.health === "loading");
  // At rest, vanished or planned: counted, never as a problem.
  const idle = all.filter((e) => e.health === "idle");
  const paused = all.filter((e) => e.health === "paused");
  const gone = all.filter((e) => e.health === "gone");
  const unknown = all.filter((e) => e.health === "unknown");
  const gen = ready.reduce((a, e) => a + (e.metrics?.genTps ?? 0), 0);
  const running = ready.reduce((a, e) => a + (e.metrics?.running ?? 0), 0);
  const waiting = ready.reduce((a, e) => a + (e.metrics?.waiting ?? 0), 0);
  const genTrend = useMemo(() => {
    // The total of the endpoints serving now, from each one's own history (aligned at the end).
    const series = ready.map((e) => e.metrics?.history.genTps ?? []);
    const len = Math.min(100, ...series.map((s) => s.length).concat(100));
    return Array.from({ length: len }, (_, i) => series.reduce((a, s) => a + (s[s.length - len + i] ?? 0), 0));
  }, [ready]);
  const notReady = [
    down.length && `${down.length} down`,
    loading.length && `${loading.length} loading`,
    paused.length && `${paused.length} paused`,
    idle.length && `${idle.length} idle`,
    gone.length && `${gone.length} not found`,
    unknown.length && `${unknown.length} unknown`,
  ].filter(Boolean).join(" · ");

  const shownHistory = historyOf ? all.find((e) => e.id === historyOf) : undefined;
  if (shownHistory) return <EndpointHistory endpoint={shownHistory} onBack={() => setHistoryOf(null)} />;

  return (
    <div className="col fill ai-endpoints">
      <Toolbar label="Endpoints">
        <ToolButton icon={RefreshCw} label={busy ? "Checking…" : "Check now"} onClick={refresh} disabled={busy}
          title="Ask every server for its models now (otherwise every 15 s; metrics every 3 s)" />
        <ToolButton icon={Play} label="Load a model…" onClick={() => wm.open("models", { tab: "load" })} priority={2} />
        <Spacer />
        <Segmented<Filter>
          label="Show"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "All" },
            { value: "managed", label: "SparkBox" },
            { value: "external", label: "External" },
            ...engines.map((e) => ({ value: e as Filter, label: ENGINE_LABEL[e] })),
          ]}
        />
        <SearchField value={query} onChange={setQuery} placeholder="Filter by model or node" count={shown.length} />
      </Toolbar>
      <div className="ai-endpoints-body">
        <Tiles>
          <Tile label={<>Ready <HelpTip topic="ai.endpoints.ready" /></>} value={`${ready.length}`} unit={` of ${all.length}`} sub={notReady || "all answering"} tone={down.length ? "crit" : undefined} trendKind="none" />
          <Tile label={<>Generating <HelpTip topic="ai.endpoints.generating" /></>} value={fmtTps(gen)} unit=" tok/s" trend={genTrend} color="var(--m-gpu)" sub={ready.length === 0 ? "none is ready" : ready.length === all.length ? "all endpoints together" : ready.length === 1 ? "the 1 that's ready" : `the ${ready.length} that are ready`} />
          <Tile label={<>Requests <HelpTip topic="ai.endpoints.requests" /></>} value={`${running.toFixed(0)}`} unit=" running" sub={waiting ? `${waiting.toFixed(0)} waiting` : "none waiting"} tone={waiting > 0 ? "warn" : undefined} trendKind="none" />
        </Tiles>
        <OpenHistoryContext.Provider value={setHistoryOf}>
          <EndpointList
            endpoints={shown}
            selected={focus}
            onSelect={setFocus}
            empty={all.length ? { title: "Nothing matches", body: "Clear the filter to see every endpoint." } : undefined}
          />
        </OpenHistoryContext.Provider>
      </div>
    </div>
  );
}
