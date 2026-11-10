import { useEffect, useRef, useState } from "react";
import { HelpTip, Notice, Toolbar } from "../../components/ui";
import { useDataSource } from "../../lib/dataSource";
import { plural } from "../../lib/vocabulary";
import { modelsApi, type DeploymentHistorySource, type DeploymentHistoryView, type HistoricalLoad } from "../../lib/modelsApi";

const MAX_BYTES = 4 * 1024 * 1024;
const date = (ms: number | null | undefined) => ms ? new Date(ms).toLocaleString() : "Not recorded";
const verdict = (value: boolean) => value ? "Passed" : "Warning";
function sourceDescription(source: DeploymentHistorySource): string {
  if (source.kind === "recovery") {
    return source.sha256.length === 64 && source.name === `${source.sha256}.json`
      ? "Recovery copy: contents match the filename checksum."
      : "Recovery copy: checksum could not be verified.";
  }
  return source.kind === "demo" ? "Made-up sample data." : "Opened for this view only. Original location and completeness are not verified.";
}

/** Kept separate from all endpoint/load components: archived identities have no actions. */
export function HistoryTab() {
  const source = useDataSource();
  return <HistoryContents key={source} source={source} />;
}

function HistoryContents({ source }: { source: "live" | "demo" }) {
  const [view, setView] = useState<DeploymentHistoryView | null>(null);
  const [opened, setOpened] = useState<DeploymentHistorySource | null>(null);
  const [selected, setSelected] = useState("");
  const [query, setQuery] = useState("");
  const [warnings, setWarnings] = useState(false);
  const [busy, setBusy] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  const [error, setError] = useState("");
  const [fileError, setFileError] = useState("");
  const alive = useRef(true);
  const request = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);

  const refresh = async () => {
    const n = ++request.current;
    setBusy(true); setError(""); setView(null);
    try {
      const result = await modelsApi.deploymentHistory();
      if (alive.current && n === request.current) {
        if (result.demo !== (source === "demo")) throw new Error("The data source changed. Refresh to read its history.");
        setView(result);
      }
    } catch (e) { if (alive.current && n === request.current) setError(String(e)); }
    finally { if (alive.current && n === request.current) setBusy(false); }
  };
  useEffect(() => {
    alive.current = true;
    void refresh();
    return () => { alive.current = false; request.current++; };
  }, []);

  const openFile = async (file: File | undefined) => {
    if (!file || source === "demo") return;
    setFileError(""); setFileBusy(true);
    try {
      if (file.size > MAX_BYTES) throw new Error("This file exceeds the 4 MiB viewing limit. The file was not changed.");
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      if (!alive.current) return;
      const result = await modelsApi.deploymentHistoryFile(file.name, bytes);
      if (alive.current) { setOpened(result); setSelected("opened"); setQuery(""); setWarnings(false); }
    } catch (e) { if (alive.current) setFileError(String(e)); }
    finally { if (alive.current) setFileBusy(false); }
  };

  const sources = (view?.sources ?? []).map((s, i) => ({ key: `copy:${s.name}`, label: `${view?.demo ? "Demo sample" : "Recovery copy"} ${i + 1} · ${plural(s.records.length, "record")}`, value: s }));
  if (opened) sources.push({ key: "opened", label: `Opened file · ${plural(opened.records.length, "record")}`, value: opened });
  const current = sources.find((s) => s.key === selected) ?? sources[0];
  const records = current?.value.records ?? [];
  const warningCount = records.filter((r) => r.unloadReport?.ok === false).length;
  const visible = records.filter((r) => (!warnings || r.unloadReport?.ok === false) &&
    [r.id, r.model, r.nodes.join(" "), r.savedState, r.message, r.failure, r.unloadReport?.summary].join(" ").toLowerCase().includes(query.toLowerCase()));

  return <div className="col fill deployment-history">
    <Toolbar>
      <button className="btn" disabled={busy} onClick={() => void refresh()}>{busy ? "Reading…" : "Refresh copies"}</button>
      <button className="btn" disabled={fileBusy || source === "demo"} onClick={() => fileInput.current?.click()}>Open report file…</button>
      <input ref={fileInput} type="file" accept=".json,application/json" hidden aria-label="Report JSON file"
        onChange={(e) => { const f = e.target.files?.[0]; e.target.value = ""; void openFile(f); }} />
      {opened && <button className="btn" disabled={fileBusy} onClick={() => { setOpened(null); setSelected(""); setFileError(""); }}>Close file</button>}
      <HelpTip topic="models.history" />
    </Toolbar>
    <div className="scroll pad">
      <Notice tone="info">Read-only history. These are past observations, not the current state of your nodes. Viewing never restores a load or changes a file.</Notice>
      {source === "demo" && <p className="muted">Demo samples only. Choose Live to view recovery copies or open an older report on this Mac.</p>}
      {error && <Notice tone="warn">{error}</Notice>}
      {fileError && <Notice tone="warn">{fileError}</Notice>}
      {fileBusy && <p role="status">Reading the selected file…</p>}
      {view?.issues.map((issue, i) => <Notice tone="warn" key={i}>{issue}</Notice>)}
      {view?.folder && <p className="history-path">Recovery folder: {view.folder}</p>}
      {view && !sources.length && !view.issues.length && <p>No recovery copies yet. You can open an older JSON report file to inspect it here.</p>}
      {!!sources.length && <>
        <label className="history-source">Source <select aria-label="History source" value={current?.key ?? ""} onChange={(e) => { setSelected(e.target.value); setQuery(""); setWarnings(false); }}>
          {sources.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
        </select></label>
        {current && <>
          <p className="history-path"><strong>{current.value.name}</strong><br />
            {sourceDescription(current.value)}
            {current.value.issues.length > 0 && " Read the file warnings below."}
          </p>
          <details><summary>File details</summary><p className="history-path">SHA-256: {current.value.sha256 || "Not available"}<br />Bytes read: {current.value.bytes.toLocaleString()}</p></details>
          {current.value.issues.map((issue, i) => <Notice tone="warn" key={i}>{issue}</Notice>)}
          <div className="history-filters">
            <label>Find <input aria-label="Find history" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Load, node or report text" /></label>
            <label><input type="checkbox" checked={warnings} onChange={(e) => setWarnings(e.target.checked)} /> Unload warnings only</label>
          </div>
          <p role="status">{visible.length} of {plural(records.length, "record")} · {warningCount} unload {warningCount === 1 ? "warning" : "warnings"} in this file</p>
          <p className="muted">The same load may appear in several copies. Each file keeps its own version; records are not merged.</p>
          {visible.map((r, i) => <HistoryRecord key={`${current.key}:${i}`} record={r} />)}
          {!visible.length && <p>{records.length ? "No records match these filters." : current.value.issues.length ? "No readable records in this file. See the warnings above." : "This saved file contains no records."}</p>}
        </>}
      </>}
    </div>
  </div>;
}

function HistoryRecord({ record: r }: { record: HistoricalLoad }) {
  const report = r.unloadReport;
  return <article className="history-record" aria-label={`Historical load ${r.id}`}>
    <h3>{r.model || r.id}</h3>
    <p>Load ID: <code>{r.id}</code> · Saved state: <strong>{r.savedState}</strong> · Current state: not checked</p>
    <p>Nodes: {r.nodes.length ? r.nodes.join(", ") : "Not recorded"}</p>
    {r.reportOnly ? <p className="muted">Older report only. Model name, load settings and start time were not retained.</p> :
      <p>Started: {date(r.startedAt == null ? null : r.startedAt * 1000)}</p>}
    {r.message && <p className="history-text">Saved message: {r.message}</p>}
    {r.failure && <Notice tone="warn">Saved failure: {r.failure}</Notice>}
    {report ? <>
      <p><strong>Recorded unload: {verdict(report.ok)}</strong> · {date(report.atMs)}</p>
      <Notice tone={report.ok ? "info" : "warn"}><span className="history-text">{report.summary}</span></Notice>
      <details><summary>Recorded node checks</summary>
        {report.nodes.map((n, i) => <div key={i} className="history-node">
          <strong>{n.node}</strong>
          <p>Containers stopped: {verdict(n.stopped)} · Memory returned: {verdict(n.memBack)} · Port closed: {verdict(n.portClosed)}</p>
          <p>Available memory: {n.memAvailable == null ? "Not recorded" : `${(n.memAvailable / 1024 ** 3).toFixed(1)} GiB (${n.memAvailable.toLocaleString()} bytes)`}</p>
          <p className="history-text">New shared-memory entries: {n.newShm.length ? n.newShm.join(", ") : "None recorded"}</p>
        </div>)}
        {!report.nodes.length && <p>No per-node checks recorded.</p>}
      </details>
    </> : <p>No unload report recorded in this copy.</p>}
  </article>;
}
