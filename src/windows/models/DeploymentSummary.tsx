// A deployment that failed, the way the Dashboard, Models and Containers show it: a neutral
// card with a red Failed pill (it's past, not an outage, and counts nowhere), how and when it
// exited, the likely cause in a few words and the decisive log line. Explain… gives the whole
// postmortem; Show log opens the head's log at that line. Everything here was only read.
// IdleServersLine says, in one quiet line, which servers are up with nothing in memory.
import { FileText, Info, Layers } from "lucide-react";
import { Icon } from "../../components/Icon";
import { HelpTip } from "../../components/ui";
import { deploymentLabel, exitSentence, refreshDeployments, nodeList, rankSpan, useDeployments, type Deployment } from "../../lib/deployments";
import { getCluster } from "../../lib/store";
import { copyText } from "../../lib/markdown";
import { endpointLabel, reachLine, type Endpoint } from "../../lib/endpoints";
import { fmtDateTime } from "../../lib/format";
import { alertInfo, singleFlight } from "../../lib/session";
import { getTerminals, openTerminal, showTerminalWindow } from "../../lib/terminals";
import { useWM } from "../../wm/WindowManager";
import "../ai.css";

/** "2 days ago", "yesterday", "3 h ago". */
export function whenAgo(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)} h ago`;
  const d = Math.floor(s / 86_400);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

/** "Oct 26 at 6:21 PM (2 days ago)". */
export function exitedAt(d: Pick<Deployment, "finishedMs" | "startedMs">, now = Date.now()): string {
  const at = d.finishedMs ?? d.startedMs;
  return `${fmtDateTime(at, now)} (${whenAgo(at, now)})`;
}

/** The title of a deployment's card: "GLM-5.3-Flash-NVFP4 · 4 nodes · Failed". */
export function failedTitle(d: Deployment): string {
  return `${deploymentLabel(d)} · ${d.nodes.length} node${d.nodes.length === 1 ? "" : "s"} · Failed`;
}

/** The decisive line: the root-cause exception (the last of the decisive lines). */
export function decisiveLine(d: Deployment): string | null {
  const pm = d.postmortem;
  return pm?.lines.length ? pm.lines[pm.lines.length - 1] : null;
}

/** The whole postmortem in a sheet: what happened, the decisive lines, the script line, the likely fix. */
export function explainDeployment(d: Deployment) {
  void singleFlight(`explain:${d.id}:${d.finishedMs ?? 0}`, () => alertInfo({ title: `Why ${deploymentLabel(d)} failed`, body: <Explanation initial={d} />, okLabel: "Done" }));
}

function Explanation({ initial }: { initial: Deployment }) {
  const d = useDeployments().find((d) => d.id === initial.id) ?? initial;
  const pm = d.postmortem;
  const head = d.members[0];
  return (
    <div className="ai-pm-sheet">
      <p>
        {exitSentence(d)}, {exitedAt(d)}. {d.inferred ? <>SparkBox grouped these containers ({d.how}).</> : null}
      </p>
      {pm?.title && <p className="strong">Likely cause: {pm.title}</p>}
      {pm?.explanation && <p>{pm.explanation}</p>}
      {pm?.lines.length ? (
        <>
          <p className="muted">From the log of {head?.container ?? "the head"} on {pm.logNode || head?.node}{d.members.length > 1 ? " (rank 0, the head)" : ""}:</p>
          <pre className="ai-pm-lines">{pm.lines.map((l, i) => <span key={i} className={i === pm.lines.length - 1 ? "decisive" : undefined}>{l}{"\n"}</span>)}</pre>
        </>
      ) : (
        <p className="muted">{pm?.error ?? "SparkBox is reading its log…"}</p>
      )}
      {pm?.scriptLine && (
        <>
          <p>Its launch script sets it:</p>
          <pre className="ai-pm-lines">{pm.scriptLine}</pre>
        </>
      )}
      {pm?.fix && <p>{pm.fix}</p>}
      <div className="ai-card-actions">
        <button className="btn small" onClick={() => showDeploymentLog(d)}>Show log</button>
        {pm?.scriptPath && <button className="btn small" onClick={() => window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind: "editor", params: { place: pm.logNode || head?.node, path: pm.scriptPath, line: String(pm.scriptLineNumber ?? 1), readOnly: "1" } } }))}>Open the script at the line</button>}
        <button className="btn small" onClick={() => copyText([failedTitle(d), exitSentence(d), pm?.title, ...(pm?.lines ?? []), pm?.fix].filter(Boolean).join("\n"))}>Copy summary</button>
        {pm?.retryable && <button className="btn small" onClick={() => void refreshDeployments(d.id)}>Try again</button>}
      </div>
      <p className="muted">SparkBox didn't change anything: it read the containers' state, the head's log and its launch script.</p>
    </div>
  );
}

/**
 * Opens the head's log in Terminal and finds the decisive line in it (the tab's own search).
 */
export function showDeploymentLog(d: Deployment) {
  const head = d.members[0];
  if (!head) return;
  const node = getCluster()?.nodes.find((n) => n.id === head.node)?.name ?? head.node;
  const key = openTerminal(head.node, { kind: "containerLogs", container: head.id || head.container }, { title: `${head.container} · ${node}` });
  showTerminalWindow();
  const line = decisiveLine(d);
  if (!line) return;
  // The start of the exception names it uniquely ("ValueError: No valid attention backend found").
  const needle = line.slice(0, 48);
  let tries = 0;
  const find = () => {
    const tab = getTerminals().tabs.find((t) => t.key === key);
    if (!tab) return;
    if (tab.search.findNext(needle, { caseSensitive: true, incremental: false }) || ++tries > 25) return;
    window.setTimeout(find, 200);
  };
  window.setTimeout(find, 300);
}

/**
 * A failed deployment: neutral card, red Failed pill, exits and time, the likely cause and the
 * decisive line (end-cut, whole in the tooltip). `compact` is the Dashboard's one-block form.
 */
export function FailedDeploymentCard({ d, compact }: { d: Deployment; compact?: boolean }) {
  const wm = useWM();
  const pm = d.postmortem;
  const line = decisiveLine(d);
  const span = rankSpan(d);
  return (
    <article className={`ai-card ai-dep-card${compact ? " compact-dep" : ""}`} aria-label={`${failedTitle(d)}${pm?.title ? `. Likely cause: ${pm.title}` : ""}`}>
      <header className="ai-card-head">
        <span className="ai-card-engine">{d.engine === "vllm" ? "vLLM" : d.engine === "sglang" ? "SGLang" : "Containers"}</span>
        <span className="ai-card-model strong" title={d.model ?? deploymentLabel(d)}>{deploymentLabel(d)}</span>
        <span className="muted">· {d.nodes.length} node{d.nodes.length === 1 ? "" : "s"}</span>
        <span className="ai-failed-pill" title="It isn't running: past, not an outage, and not counted">Failed</span>
        <HelpTip topic="fix-a-load-that-failed" />
        {!compact && <span className="spacer" />}
        {!compact && <span className="muted ai-dep-where" title={d.inferred ? `Grouped by SparkBox: ${d.how}` : "SparkBox's own deployment"}>{d.id}{span ? ` · ${span}` : ""}{d.inferred ? " · likely" : ""}</span>}
      </header>
      <div className="ai-dep-exit" title={d.members.map((m) => `${m.node}: ${m.state === "running" ? "running" : `exit ${m.exitCode}${m.oom ? " (out of memory)" : ""}`}, started ${fmtDateTime(m.startedMs)}${m.finishedMs ? `, ended ${fmtDateTime(m.finishedMs)}` : ""}`).join("\n")}>
        {exitSentence(d)} · {exitedAt(d)}
      </div>
      {pm?.title ? (
        <div className="ai-dep-cause"><span className="muted">Likely cause:</span> {pm.title}</div>
      ) : (
        <div className="ai-dep-cause muted">{pm ? pm.error ?? "Its log names no known cause; the first error is below." : "Reading its log…"}</div>
      )}
      {line && <div className="mono ai-dep-line" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={line}>{line}</div>}
      <footer className="ai-card-actions">
        {pm?.retryable && <button type="button" className="btn small" onClick={() => void refreshDeployments(d.id)}>Try again</button>}
        <button type="button" className="btn small" onClick={() => explainDeployment(d)} title="The whole postmortem: the decisive lines, the launch script and the likely fix">
          <Icon icon={Info} size={12} />Explain…
        </button>
        <button type="button" className="btn small" onClick={() => showDeploymentLog(d)} title={`docker logs of ${d.members[0]?.container ?? "the head"} on ${d.members[0]?.node ?? "its node"}, at the decisive line`}>
          <Icon icon={FileText} size={12} />Show log
        </button>
        {!compact && (
          <button type="button" className="btn small" onClick={() => wm.open("containers", { view: "deployments", nav: String(Date.now()) })} title={`Its ${d.members.length} containers on ${nodeList(d.nodes)}`}>
            <Icon icon={Layers} size={12} />Containers
          </button>
        )}
      </footer>
    </article>
  );
}

/**
 * "Ollama on cell1 and cell3 · idle, nothing in memory": servers that answer with nothing loaded,
 * in one neutral line (none of them is serving, none of them is a problem). Null when there are none.
 */
export function IdleServersLine({ endpoints }: { endpoints: readonly Endpoint[] }) {
  const wm = useWM();
  const idle = endpoints.filter((e) => e.health === "idle");
  if (!idle.length) return null;
  const ollama = idle.filter((e) => e.engine === "ollama");
  const others = idle.filter((e) => e.engine !== "ollama");
  const names = [
    ...(ollama.length ? [`Ollama on ${nodeList(ollama.map((e) => e.headNode))}`] : []),
    ...others.map((e) => endpointLabel(e)),
  ];
  const tip = idle.map((e) => `${endpointLabel(e)}: idle, nothing in memory${e.onDisk?.length ? ` (${e.onDisk.length} model${e.onDisk.length === 1 ? "" : "s"} on disk)` : ""}. ${reachLine(e) ?? ""}`.trim()).join("\n");
  return (
    <button type="button" className="ai-idle-line" title={tip} onClick={() => wm.open("endpoints", { id: idle[0].id, nav: String(Date.now()) })}>
      <i className="ai-idle-dot" aria-hidden />
      <span>{nodeList(names)}</span>
      <span className="muted"> · idle, nothing in memory</span>
    </button>
  );
}
