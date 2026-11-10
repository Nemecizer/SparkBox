// Benchmarks: a concurrency sweep with exact-length prompts against any vLLM or SGLang
// endpoint, streamed as a task you can cancel; results as a throughput-versus-
// latency chart (runs overlaid) and a table of p50/p90/p99, saved as CSV or JSON.
import { useEffect, useMemo, useState } from "react";
import { useDataSource } from "../../lib/dataSource";
import { FileJson, FileSpreadsheet, Gauge } from "lucide-react";
import { Icon } from "../../components/Icon";
import { Table, type Column } from "../../components/Table";
import { EmptyState, FormGroup, FormRow, HelpTip, InfoTip, Notice, Segmented, Spacer, StopIcon } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { useEndpoints } from "../../lib/endpoints";
import { modelsApi, type BenchLevel, type BenchRun, type BenchSpec } from "../../lib/modelsApi";
import { cancelTask, useTask } from "../../lib/tasks";
import { toast } from "../../lib/toasts";
import { Ago, errText, fmtMs, fmtTps } from "./shared";

const PROFILES: Record<Exclude<BenchSpec["profile"], "custom">, { label: string; input: number; output: number; what: string }> = {
  chat: { label: "Chat", input: 512, output: 256, what: "short prompt, medium answer" },
  rag: { label: "RAG", input: 4096, output: 256, what: "documents in, short answer" },
  long: { label: "Long", input: 32768, output: 512, what: "long context" },
};
const LEVELS = [1, 2, 4, 8, 16, 32];
/** Runs overlaid at once; their colours are the node-identity hues, in order. */
const MAX_OVERLAY = 4;
const RUN_COLORS = ["var(--node-1, #1a73d5)", "var(--node-2, #e86518)", "var(--node-3, #00a580)", "var(--node-4, #992154)"];

function runLabel(b: BenchRun): string {
  const split = b.nodes.length > 1 ? ` ${b.tensorParallel > 1 ? `TP ${b.tensorParallel}` : ""}${b.pipelineParallel > 1 ? `PP ${b.pipelineParallel}` : ""}` : "";
  return `${b.servedName}${split} · ${b.spec ? `${b.spec.inputTokens}→${b.spec.outputTokens}` : "quick"}`;
}

/** Throughput (x) against time per output token (y): down and to the right is better. */
function SweepChart({ runs }: { runs: BenchRun[] }) {
  const [hover, setHover] = useState<{ run: number; level: BenchLevel; x: number; y: number } | null>(null);
  const W = 640, H = 300, L = 56, R = 120, T = 14, B = 40;
  const all = runs.flatMap((r) => r.levels);
  const xMax = Math.max(1, ...all.map((l) => l.outputTps)) * 1.08;
  const yMax = Math.max(1, ...all.map((l) => l.tpotMs.p50)) * 1.1;
  const sx = (v: number) => L + (v / xMax) * (W - L - R);
  const sy = (v: number) => T + (1 - v / yMax) * (H - T - B);
  const ticks = (max: number) => {
    const step = 10 ** Math.floor(Math.log10(max / 4));
    const nice = [1, 2, 5, 10].map((m) => m * step).find((s) => max / s <= 6) ?? step * 10;
    return Array.from({ length: Math.floor(max / nice) + 1 }, (_, i) => i * nice);
  };
  return (
    <div className="ai-chart">
      {runs.length > 1 && (
        <div className="ai-chart-legend" aria-label="Runs">
          {runs.map((r, i) => (
            <span key={r.at}><i style={{ background: RUN_COLORS[i] }} />{runLabel(r)}</span>
          ))}
        </div>
      )}
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Output tokens per second against time per output token, per concurrency level" onPointerLeave={() => setHover(null)}>
        {ticks(xMax).map((t) => (
          <g key={`x${t}`}>
            <line x1={sx(t)} x2={sx(t)} y1={T} y2={H - B} className="ai-grid" />
            <text x={sx(t)} y={H - B + 14} textAnchor="middle" className="ai-axis">{t}</text>
          </g>
        ))}
        {ticks(yMax).map((t) => (
          <g key={`y${t}`}>
            <line x1={L} x2={W - R} y1={sy(t)} y2={sy(t)} className="ai-grid" />
            <text x={L - 6} y={sy(t) + 3} textAnchor="end" className="ai-axis">{t}</text>
          </g>
        ))}
        <text x={(L + W - R) / 2} y={H - 6} textAnchor="middle" className="ai-axis-title">Output tokens per second, all requests</text>
        <text x={12} y={(T + H - B) / 2} textAnchor="middle" className="ai-axis-title" transform={`rotate(-90 12 ${(T + H - B) / 2})`}>ms per output token (p50)</text>
        {runs.map((r, i) => {
          const pts = [...r.levels].sort((a, b) => a.concurrency - b.concurrency);
          const last = pts[pts.length - 1];
          return (
            <g key={r.at}>
              <polyline points={pts.map((l) => `${sx(l.outputTps)},${sy(l.tpotMs.p50)}`).join(" ")} fill="none" stroke={RUN_COLORS[i]} strokeWidth={2} strokeLinejoin="round" />
              {pts.map((l) => (
                <g key={l.concurrency}>
                  <circle cx={sx(l.outputTps)} cy={sy(l.tpotMs.p50)} r={4} fill={RUN_COLORS[i]} stroke="var(--surface, #fff)" strokeWidth={2} />
                  <circle cx={sx(l.outputTps)} cy={sy(l.tpotMs.p50)} r={12} fill="transparent"
                    onPointerEnter={() => setHover({ run: i, level: l, x: sx(l.outputTps), y: sy(l.tpotMs.p50) })} />
                </g>
              ))}
              {last && runs.length <= MAX_OVERLAY && (
                <text x={sx(last.outputTps) + 8} y={sy(last.tpotMs.p50) + 3} className="ai-direct-label">{r.servedName}</text>
              )}
            </g>
          );
        })}
        {hover && (
          <g pointerEvents="none">
            <line x1={hover.x} x2={hover.x} y1={T} y2={H - B} className="ai-crosshair" />
          </g>
        )}
      </svg>
      {hover && (
        <div className="ai-tooltip" style={{ left: `${(hover.x / W) * 100}%`, top: `${(hover.y / H) * 100}%` }} role="status">
          <div className="strong">{hover.level.concurrency} at once · {runs[hover.run].servedName}</div>
          <div>{fmtTps(hover.level.outputTps)} tok/s total · {hover.level.reqPerS.toFixed(2)} req/s</div>
          <div>per token p50 {fmtMs(hover.level.tpotMs.p50)} · first token p50 {fmtMs(hover.level.ttftMs.p50)}</div>
        </div>
      )}
    </div>
  );
}

/** p50, then p90 and p99 in grey: "266 / 274 / 279 ms", with one unit when all three are under a second. */
const pctCell = (p: { p50: number; p90: number; p99: number }) => {
  const title = `p50 ${fmtMs(p.p50)} · p90 ${fmtMs(p.p90)} · p99 ${fmtMs(p.p99)}`;
  if ([p.p50, p.p90, p.p99].every((v) => Number.isFinite(v) && v < 1000)) {
    return <span className="num" title={title}>{Math.round(p.p50)} <span className="muted">/ {Math.round(p.p90)} / {Math.round(p.p99)}</span> ms</span>;
  }
  return <span className="num" title={title}>{fmtMs(p.p50)} <span className="muted">/ {fmtMs(p.p90)} / {fmtMs(p.p99)}</span></span>;
};

const levelColumns: Column<BenchLevel>[] = [
  { key: "c", label: "At once", align: "right", sort: (l) => l.concurrency, render: (l) => <span className="num">{l.concurrency}</span> },
  { key: "done", label: "Done", align: "right", priority: 3, render: (l) => <span className="num">{l.completed}{l.failed ? <span className="error-text"> ({l.failed} failed)</span> : null}</span> },
  { key: "rps", label: "Req/s", align: "right", sort: (l) => l.reqPerS, render: (l) => <span className="num">{l.reqPerS.toFixed(l.reqPerS >= 1 ? 2 : 3)}</span> },
  { key: "tps", label: "Output tok/s", align: "right", sort: (l) => l.outputTps, render: (l) => <span className="num">{fmtTps(l.outputTps)}</span> },
  { key: "ttft", label: "First token", align: "right", render: (l) => pctCell(l.ttftMs), title: () => "Time to first token" },
  { key: "tpot", label: "Per token", align: "right", render: (l) => pctCell(l.tpotMs), title: () => "Time per output token after the first (TPOT)" },
  { key: "itl", label: "Between tokens", align: "right", priority: 2, render: (l) => pctCell(l.itlMs), title: () => "Inter-token latency: every gap between streamed tokens" },
];

export function BenchTab({ onRuns }: { onRuns?: (n: number) => void }) {
  // A sweep needs token-id prompts on /v1/completions: vLLM (and NIM), SGLang, TensorRT-LLM.
  const endpoints = useEndpoints().filter((e) => ["vllm", "sglang", "nim", "trtllm"].includes(e.engine) && e.health === "ready" && e.models.length);
  const [runs, setRuns] = useState<BenchRun[]>([]);
  const [target, setTarget] = useState<string>("");
  const [profile, setProfile] = useState<BenchSpec["profile"]>("chat");
  const [custom, setCustom] = useState({ input: "2048", output: "256" });
  const [levels, setLevels] = useState<number[]>([1, 2, 4, 8]);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [shown, setShown] = useState<number[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const task = useTask(taskId);

  const load = () => modelsApi.benchmarksList().then((list) => {
    const sweeps = list.filter((b) => b.levels?.length);
    setRuns(sweeps);
    onRuns?.(sweeps.length);
    setShown((s) => (s.length ? s.filter((at) => sweeps.some((b) => b.at === at)) : sweeps.slice(-2).map((b) => b.at)));
    setSelected((s) => s ?? sweeps[sweeps.length - 1]?.at ?? null);
  }, () => {});
  // The list is the data source's own (bench.rs): switching Demo and Live reloads it, selection and all.
  const source = useDataSource();
  useEffect(() => {
    setShown([]);
    setSelected(null);
    load();
  }, [source]);
  // A finished sweep is saved: show it.
  useEffect(() => {
    if (task && task.state !== "running") {
      load().then(() => modelsApi.benchmarksList().then((l) => {
        const newest = l.filter((b) => b.levels?.length).pop();
        if (newest) {
          setSelected(newest.at);
          setShown((s) => [...s.filter((x) => x !== newest.at), newest.at].slice(-MAX_OVERLAY));
        }
      }));
    }
  }, [task?.state]);

  const pick = endpoints.find((e) => `${e.id}|${e.models[0]}` === target) ?? endpoints[0];
  const spec: BenchSpec = profile === "custom"
    ? { profile, inputTokens: Number(custom.input) || 0, outputTokens: Number(custom.output) || 0, concurrency: levels }
    : { profile, inputTokens: PROFILES[profile].input, outputTokens: PROFILES[profile].output, concurrency: levels };

  const run = async () => {
    if (!pick) return;
    setError(null);
    try {
      // A sweep loads the server for minutes: it needs Admin mode like any other change.
      setTaskId(await withAdmin(() => modelsApi.benchRun(pick.id, pick.models[0], spec), { action: "bench_run" }));
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(errText(e));
    }
  };

  const exportRun = async (b: BenchRun, format: "csv" | "json") => {
    try {
      const path = await modelsApi.benchExport(b.at, format);
      toast({ kind: "success", title: `Saved ${format.toUpperCase()}`, body: path });
    } catch (e) {
      toast({ kind: "error", title: "Couldn't save it", body: errText(e) });
    }
  };

  const overlay = runs.filter((b) => shown.includes(b.at));
  const current = runs.find((b) => b.at === selected) ?? null;
  const running = task?.state === "running";
  const etaRequests = useMemo(() => levels.reduce((a, c) => a + Math.max(4, 2 * c), 0), [levels]);

  return (
    <div className="ai-bench">
      <div className="ai-bench-form">
        <FormGroup
          title={
            <>
              Run a sweep{" "}
              <InfoTip label="About the sweep" topic="models-benchmarks">Each test sends a prompt of exactly {spec.inputTokens.toLocaleString()} tokens and gets a reply of exactly {spec.outputTokens} tokens, so runs compare fairly. One warm-up request goes first, then {etaRequests} requests in all. On the chart, down and to the right is better. Other apps using the same server change the result.</InfoTip>
            </>
          }
        >
          <FormRow label="Endpoint">
            {endpoints.length ? (
              <select value={pick ? `${pick.id}|${pick.models[0]}` : ""} onChange={(e) => setTarget(e.target.value)}>
                {endpoints.map((e) => <option key={e.id} value={`${e.id}|${e.models[0]}`}>{e.models[0]} — {e.headNode} :{e.port}{e.managedBy === "external" ? " (external)" : ""}</option>)}
              </select>
            ) : <span className="muted">No vLLM or SGLang endpoint is ready.</span>}
          </FormRow>
          <FormRow label={<>Profile <HelpTip topic="models.bench.profile" /></>} hint={profile === "custom" ? "Your own lengths" : `${PROFILES[profile].input.toLocaleString()} → ${PROFILES[profile].output} tokens`}>
            <Segmented<BenchSpec["profile"]> label="Profile" value={profile} onChange={setProfile}
              options={[...Object.entries(PROFILES).map(([v, p]) => ({ value: v as BenchSpec["profile"], label: p.label, title: `${p.input} → ${p.output} tokens: ${p.what}` })), { value: "custom", label: "Custom" }]} />
          </FormRow>
          {profile === "custom" && (
            <FormRow label="Tokens in → out">
              <span className="ai-inline">
                <input className="narrow" type="number" min={1} value={custom.input} onChange={(e) => setCustom({ ...custom, input: e.target.value })} aria-label="Prompt tokens" />
                <span className="muted">→</span>
                <input className="narrow" type="number" min={1} value={custom.output} onChange={(e) => setCustom({ ...custom, output: e.target.value })} aria-label="Output tokens" />
              </span>
            </FormRow>
          )}
          <FormRow label={<>Requests at once <HelpTip topic="models.bench.requests" /></>}>
            <span className="ai-inline">
              {LEVELS.map((c) => (
                <label key={c} className="check">
                  <input type="checkbox" checked={levels.includes(c)} onChange={(e) => setLevels(e.target.checked ? [...levels, c].sort((a, b) => a - b) : levels.filter((x) => x !== c))} />{c}
                </label>
              ))}
            </span>
          </FormRow>
          <div className="ai-pad ai-inline">
            {running ? (
              <button className="btn" onClick={() => taskId && cancelTask(taskId)}><Icon icon={StopIcon} size={12} />Stop</button>
            ) : (
              <button className="btn primary" disabled={!pick || !levels.length} onClick={run}><Icon icon={Gauge} size={12} />Run benchmark</button>
            )}
            {task && (
              <span className={task.state === "failed" ? "error-text" : "muted"}>
                {running ? `${Math.round((task.progress ?? 0) * 100)}% · ${task.detail ?? task.steps[task.steps.length - 1]?.label ?? "starting"}` : task.summary ?? task.error}
              </span>
            )}
          </div>
          {running && <div className="meter ok ai-pad"><div className="meter-track"><div className="meter-fill" style={{ width: `${Math.round((task?.progress ?? 0) * 100)}%` }} /></div></div>}
          {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
        </FormGroup>
        <FormGroup
          title={
            <>
              Runs{" "}
              <InfoTip label="About runs" topic="models.bench.runs">Tick up to {MAX_OVERLAY} to compare them in the chart.</InfoTip>
            </>
          }
        >
          {runs.length === 0 && <div className="muted ai-pad">No sweeps yet.</div>}
          {[...runs].reverse().map((b) => {
            const i = overlay.findIndex((x) => x.at === b.at);
            return (
              <div key={b.at} className={`ai-run${selected === b.at ? " on" : ""}`} onClick={() => setSelected(b.at)}>
                <input type="checkbox" checked={shown.includes(b.at)} disabled={!shown.includes(b.at) && shown.length >= MAX_OVERLAY}
                  onClick={(e) => e.stopPropagation()} aria-label={`Show ${runLabel(b)} in the chart`}
                  onChange={(e) => setShown(e.target.checked ? [...shown, b.at] : shown.filter((x) => x !== b.at))} />
                {i >= 0 && <i className="ai-run-swatch" style={{ background: RUN_COLORS[i] }} />}
                <span className="ai-run-name">{runLabel(b)}</span>
                <span className="muted"><Ago ts={b.at} /></span>
              </div>
            );
          })}
        </FormGroup>
      </div>
      <div className="ai-bench-results">
        {overlay.length ? <SweepChart runs={overlay} /> : <EmptyState icon={Gauge} title="No run picked" body="Run a sweep, or tick runs on the left to compare them." />}
        {current && (
          <section>
            <div className="ai-section-head">
              <span className="section-title">{runLabel(current)}</span>
              <span className="muted ai-section-sub" title={`${current.nodes.join(" + ")}${current.external ? " · started outside SparkBox" : ""} · latencies are p50 / p90 / p99`}>
                {current.nodes.join(" + ")}{current.external ? " · started outside SparkBox" : ""} · latencies are p50 / p90 / p99
              </span>
              <Spacer />
              <button className="btn small" onClick={() => exportRun(current, "csv")}><Icon icon={FileSpreadsheet} size={12} />Save CSV</button>
              <button className="btn small" onClick={() => exportRun(current, "json")}><Icon icon={FileJson} size={12} />Save JSON</button>
            </div>
            <Table columns={levelColumns} rows={current.levels} rowKey={(l) => String(l.concurrency)} tableId="models.bench.levels" ariaLabel="Results per level" />
          </section>
        )}
      </div>
    </div>
  );
}
