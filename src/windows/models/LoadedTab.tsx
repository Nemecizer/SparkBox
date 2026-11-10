// What's serving right now, as a compact list: every endpoint on the cluster
// (SparkBox's loads and external ones), its health and speed, with Endpoints as
// the detailed view. A deployment that failed in the last week (found on the
// cluster, whoever started it) gets a row too, with Explain. Loads that failed or
// stopped (with what the last unload found), and the vLLM log, follow. A load that failed to
// start says why and where it's left, and its log pane shows each rank's last lines.
import { useState } from "react";
import { ExternalLink, Gauge, Info, Lock, Trash2 } from "lucide-react";
import { Icon } from "../../components/Icon";
import { Table, type Column } from "../../components/Table";
import { CodeBlock, HelpTip, Notice, SectionTitle, Spacer, Toolbar, ToolButton, Truncate } from "../../components/ui";
import { lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, useAdminState, withAdmin } from "../../lib/admin";
import { latestFailed, useDeployments, type Deployment as ClusterDeployment } from "../../lib/deployments";
import { endpointLabel, endpointView, useEndpoints, type Endpoint } from "../../lib/endpoints";
import { showError } from "../../lib/errors";
import { inNames, modelsApi, type Deployment } from "../../lib/modelsApi";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";
import type { Stage } from "../../lib/types";
import { useWM } from "../../wm/WindowManager";
import { explainDeployment, whenAgo } from "./DeploymentSummary";
import { HealthPill, useEndpointActions } from "./EndpointCard";
import { Ago, ENGINE_LABEL, fmtTps } from "./shared";

export function stageText(d: Pick<Deployment, "state" | "stage" | "message">): string {
  if (d.state !== "starting") return d.message;
  const s: Stage | null = d.stage;
  if (!s) return d.message;
  switch (s.kind) {
    case "joining": return `${s.have} of ${s.need} nodes up`;
    case "compiling": return `Compiling: ${s.what}`;
    case "loading": return `Loading weights ${s.percent}%`;
    case "warming": return "Capturing CUDA graphs";
    default: return d.message;
  }
}

/** A failed start's log pane: each rank's last lines, labelled. Without a record, the load's own tail. */
export function failureLog(d: Pick<Deployment, "logTail" | "failure">, nameOf: (node: string) => string = (n) => n): string {
  const ranks = d.failure?.ranks ?? [];
  if (!ranks.length) return d.logTail;
  return ranks.map((r) => `── rank ${r.rank}, ${r.role} on ${nameOf(r.node)}${r.exit == null ? "" : `, exit ${r.exit}`} ──\n${r.tail}`).join("\n\n");
}

/** A row: an endpoint, or a deployment that failed in the last week. */
type Row = { kind: "endpoint"; e: Endpoint } | { kind: "failed"; d: ClusterDeployment };

const ENGINE_OF: Record<string, string> = { vllm: "vLLM", sglang: "SGLang", other: "Containers" };

/** A Forget button's tooltip under the lock rule: look-only's words, or its own and "needs Admin". */
function forgetTip(lock: "locked" | "lookOnly" | null, tip: string): string {
  return lock === "lookOnly" ? LOOK_ONLY_TIP : lock === "locked" ? `${tip} · ${NEEDS_ADMIN_TIP}` : tip;
}

export function LoadedTab({ deployments, refresh, onLoad, onBenchmark }: {
  deployments: Deployment[];
  refresh: () => void;
  onLoad: () => void;
  onBenchmark: () => void;
}) {
  const endpoints = useEndpoints();
  const cluster = useDeployments();
  const snap = useCluster();
  const wm = useWM();
  const a = useEndpointActions();
  const admin = useAdminState();
  // Forget follows the lock rule (admin.ts lockOf): off in look-only, asks to unlock when locked.
  const forgetLock = lockOf(!admin.unlocked, admin);
  const [logOf, setLogOf] = useState<string | null>(null);
  const ended = deployments.filter((d) => (d.state === "failed" && !d.recovery) || d.state === "stopped");
  const groups = [...new Set(deployments.filter((d) => d.spec.group && d.state !== "stopped").map((d) => d.spec.group!))];
  const withLogs = deployments.filter((d) => (d.logTail || d.failure?.ranks.length) && (d.state === "starting" || d.state === "ready" || d.state === "failed"));
  const shownLog = withLogs.find((d) => d.spec.id === logOf) ?? withLogs[0];
  const open = (e: Endpoint) => wm.open("endpoints", { id: e.id, nav: String(Date.now()) });
  const nameOf = (id: string) => snap?.nodes.find((n) => n.id === id)?.name ?? id;
  /** Rust's sentences say a node's id; the tab says its name. */
  const named = (text: string) => inNames(text, snap?.nodes ?? []);

  /** The nodes the snapshot shows a load's containers on. */
  const holding = (d: Deployment) => (snap?.nodes ?? []).filter((n) => n.containers.some((c) => c.labels?.["sparkbox.deployment"] === d.spec.id)).map((n) => n.id);

  // A load whose containers are still there is unloaded first (one task), then forgotten.
  const forget = async (d: Deployment) => {
    const on = holding(d);
    const ok = await confirmAction({
      title: `Forget ${d.spec.servedName}?`,
      body: (on.length ? `Its containers are still on ${on.map(nameOf).join(", ")}: this unloads them first. ` : "") +
        (d.demo ? "This removes the simulated note. Saved live history stays unchanged." :
          "This removes the note from Loaded. SparkBox keeps a recovery copy of the deployment and unload report on this Mac; it does not restore that copy automatically."),
      confirmLabel: on.length ? "Unload and forget" : "Forget note",
      danger: true,
      dedupeKey: `forget:${d.spec.id}`,
    });
    if (!ok) return;
    try {
      const task = await withAdmin(() => modelsApi.deploymentForget(d.spec.id), { action: "deployment_forget" });
      if (task) toast({ kind: "info", opId: task, title: `Unloading ${d.spec.servedName}`, body: "It's forgotten once its containers are gone. Tasks shows each step." });
    } catch (e) {
      showError(e, { action: `Forget ${d.spec.servedName}` });
    }
    refresh();
  };

  const forgetAction = (d: Deployment) => (
    <>
      <button type="button" className="btn small" disabled={forgetLock === "lookOnly"} onClick={() => void forget(d)}
        title={forgetTip(forgetLock, holding(d).length ? `Unload its containers on ${holding(d).map(nameOf).join(", ")}, then take it off this list` : "Take it off this list")}>
        <Icon icon={Trash2} size={12} />{holding(d).length ? "Forget…" : "Forget"}{forgetLock && <Icon icon={Lock} size={12} className="tool-lock" />}
      </button>{" "}
      <HelpTip topic="models.loaded.forget" />
    </>
  );

  const failed = latestFailed(cluster, 7);
  const rows: Row[] = [...endpoints.map((e): Row => ({ kind: "endpoint", e })), ...(failed ? [{ kind: "failed", d: failed } as Row] : [])];
  const rowKey = (r: Row) => (r.kind === "endpoint" ? r.e.id : `failed:${r.d.id}`);
  const sortName = (r: Row) => (r.kind === "endpoint" ? endpointLabel(r.e) : r.d.label);
  const columns: Column<Row>[] = [
    {
      key: "model",
      label: "Model",
      render: (r) => r.kind === "endpoint"
        ? <Truncate text={r.e.engine === "ollama" && r.e.ollama?.length ? `${endpointLabel(r.e)} · ${r.e.ollama.map((m) => m.name).join(", ")}` : endpointLabel(r.e)} />
        : <Truncate text={r.d.label} title={`${r.d.label} (${r.d.id}${r.d.inferred ? `, grouped by SparkBox: ${r.d.how}` : ""})`} />,
      sort: sortName,
      fit: "flex",
    },
    { key: "engine", label: "Engine", render: (r) => (r.kind === "endpoint" ? ENGINE_LABEL[r.e.engine] : ENGINE_OF[r.d.engine] ?? r.d.engine), sort: (r) => (r.kind === "endpoint" ? r.e.engine : r.d.engine), priority: 2 },
    {
      key: "nodes",
      label: "Nodes",
      render: (r) => {
        if (r.kind === "failed") return <span className="ai-card-nodes" title={r.d.nodes.map(nameOf).join(", ")}>{r.d.nodes.length} nodes</span>;
        const missing = new Set(r.e.missingNodes ?? []);
        return (
          <span className="ai-card-nodes">
            {r.e.nodes.map((n) => <span key={n} className={`ai-node${missing.has(n) ? " missing" : ""}`}>{nameOf(n)}</span>)}
          </span>
        );
      },
      sort: (r) => (r.kind === "endpoint" ? r.e.nodes.join(",") : r.d.nodes.join(",")),
      fit: "content",
    },
    {
      key: "health",
      label: "State",
      render: (r) => {
        if (r.kind === "failed") {
          const at = r.d.finishedMs ?? r.d.startedMs;
          return (
            <span className="ai-state-cell ai-loaded-failed">
              <span className="ai-failed-pill">Failed</span>
              <span className="muted" title={r.d.postmortem?.title ? `Likely cause: ${r.d.postmortem.title}` : undefined}>
                Exited {r.d.exitCode ?? "?"} · {whenAgo(at).replace(" days ago", " d ago")}
              </span>
            </span>
          );
        }
        const v = endpointView(r.e);
        return (
          <span className="ai-state-cell">
            <HealthPill e={r.e} />
            {v.reason && v.tone !== "progress" && <Truncate text={v.reason} className={v.tone === "crit" ? "error-text" : "muted"} />}
          </span>
        );
      },
      sort: (r) => (r.kind === "failed" ? -1 : ["down", "loading", "paused", "stopped", "gone", "idle", "ready"].indexOf(r.e.health)),
      title: (r) => (r.kind === "failed" ? r.d.postmortem?.title ?? "Failed" : endpointView(r.e).reason ?? endpointView(r.e).word),
      fit: "flex",
    },
    {
      key: "tps",
      label: "Generate",
      align: "right",
      render: (r) => (r.kind === "endpoint" && r.e.health === "ready" && r.e.metrics ? `${fmtTps(r.e.metrics.genTps)} tok/s` : "—"),
      sort: (r) => (r.kind === "endpoint" && r.e.health === "ready" ? r.e.metrics?.genTps ?? 0 : -1),
    },
    {
      key: "open",
      label: "",
      render: (r) => (r.kind === "endpoint" ? (
        <button type="button" className="btn small" onClick={(ev) => { ev.stopPropagation(); open(r.e); }} title="Its card, metrics and actions">
          <Icon icon={ExternalLink} size={12} />Open in Endpoints
        </button>
      ) : (
        <>
          <button type="button" className="btn small" onClick={(ev) => { ev.stopPropagation(); explainDeployment(r.d); }} title="Why it failed: the decisive log lines, the launch script and the likely fix">
            <Icon icon={Info} size={12} />Explain…
          </button>
          <HelpTip topic="fix-a-load-that-failed" />
        </>
      )),
      hideable: false,
      fit: "content",
    },
  ];

  return (
    <div className="col fill">
      <Toolbar label="Loaded">
        <ToolButton icon={Gauge} label="Benchmark…" onClick={onBenchmark} disabled={!endpoints.some((e) => e.engine === "vllm" && e.health === "ready")}
          title="Throughput and latency at 1 to 32 requests at once" />
        <Spacer />
        <button type="button" className="btn small" onClick={onLoad}>Load a model…</button>
        <HelpTip topic="models-loaded" />
      </Toolbar>
      <div className="ai-endpoints-body">
        {groups.map((group) => {
          const members = deployments.filter((d) => d.spec.group === group && d.state !== "stopped");
          return <Notice key={group} tone="info"><strong>{members[0].spec.servedName} · {members.length} replicas</strong>
            <span> · group {group}. Each endpoint has its own readiness, key and recovery.</span>{" "}
            <button className="btn small" disabled={forgetLock === "lookOnly"} onClick={() => void withAdmin(async () => {
              if (!await confirmAction({ title: `Unload ${members.length} replicas?`, body: members.map((d) => `${d.spec.id}: ${d.spec.nodes.map(nameOf).join(", ")}`).join("; ") + ". Each owned load is unloaded independently, with its usual checks.", confirmLabel: "Unload group", danger: true })) return;
              for (const d of members) await modelsApi.deploymentUnload(d.spec.id);
              refresh();
            }, { action: "deployment_unload" }).catch((e) => showError(e))}>Unload group…</button>
          </Notice>;
        })}
        {deployments.filter((d) => d.recovery && d.state !== "stopped").map((d) => (
          <Notice key={d.spec.id} tone={d.recovery === "checking" || d.recovery === "waiting" ? "info" : "warn"}>
            <strong>{d.spec.servedName} · {d.recovery === "cleanup" ? "Interrupted startup" : d.recovery === "maintenance" ? "Paused for maintenance" : d.recovery === "ownerAction" ? "Owner review needed" : "Recovering saved load"}</strong>
            {d.recovery === "maintenance" && <button className="btn small" onClick={() => wm.open("updates")}>Software Updates…</button>}
            <div>{named(d.message)}</div>
            <HelpTip topic="models.loaded.recovery" />
            {d.recovery === "cleanup" && <button className="btn small" onClick={() => void withAdmin(async () => {
              if (!await confirmAction({ title: `Resume ${d.spec.servedName}?`, body: "Recheck ownership, the saved image and memory on every node, then start only missing ranks (workers before the head). Existing ranks must still be running. A stopped rank requires Restart in order or cleanup.", confirmLabel: "Resume missing ranks" })) return;
              await modelsApi.deploymentResume(d.spec.id);
            }, { action: "deployment_resume" }).catch((e) => showError(e))}>Resume missing ranks…</button>}
            {d.state === "failed" && forgetAction(d)}
          </Notice>
        ))}
        <Table<Row>
          tableId="models-loaded"
          ariaLabel="Serving now"
          columns={columns}
          rows={rows}
          rowKey={rowKey}
          onActivate={(r) => (r.kind === "endpoint" ? open(r.e) : explainDeployment(r.d))}
          onRowContextMenu={(r, ev) => r.kind === "endpoint" && a.menu(ev, r.e)}
          empty="Nothing is serving. Pick a model in the Library and press Load, or start vLLM, SGLang, llama.cpp or Ollama on a node: SparkBox finds them by itself."
          itemNoun={["endpoint", "endpoints"]}
        />
        {deployments.filter((d) => d.transportWarning && (d.state === "starting" || d.state === "ready")).map((d) => (
          <Notice key={`transport:${d.spec.id}`} tone="warn"><span className="strong">{d.spec.servedName}</span>: {d.transportWarning}</Notice>
        ))}
        {ended.map((d) => {
          const endedAt = d.state === "failed" ? d.failure?.atMs : d.unloadReport?.atMs;
          return (
            <Notice key={d.spec.id} tone={d.state === "failed" ? "error" : d.unloadReport && !d.unloadReport.ok ? "warn" : "info"}>
              <span className="strong">{d.spec.servedName}</span> on {d.spec.nodes.map(nameOf).join(", ")} {d.state === "failed" ? "failed" : "was unloaded"}{endedAt ? <> (<Ago ts={endedAt / 1000} />)</> : null}.{" "}
              <span className="plain">{named(d.state === "stopped" && d.unloadReport ? `Last unload: ${d.unloadReport.summary}` : d.failure?.cause ?? stageText(d))}</span>{" "}
              {d.state === "failed" && d.failure && d.failure.leftOn.length > 0 && <span className="plain">Left on {d.failure.leftOn.map(nameOf).join(", ")}: Forget unloads them first. </span>}
              {d.state === "failed" && d.failure?.logDir && <span className="plain">Logs on this Mac: <span className="mono">{d.failure.logDir}</span> </span>}
              {forgetAction(d)}
            </Notice>
          );
        })}
        {shownLog && (
          <section>
            <SectionTitle
              title={shownLog.spec.engine === "sglang" ? "SGLang log" : "vLLM log"}
              actions={
                <>
                  {withLogs.length > 1 && (
                    <select value={shownLog.spec.id} onChange={(e) => setLogOf(e.target.value)} aria-label="Load">
                      {withLogs.map((d) => <option key={d.spec.id} value={d.spec.id}>{d.spec.servedName}</option>)}
                    </select>
                  )}
                  <span className="muted">{shownLog.spec.servedName} · {named(stageText(shownLog))}</span>
                </>
              }
            />
            <CodeBlock text={failureLog(shownLog, nameOf)} wrap={false} maxHeight={200} label={`sparkbox-${shownLog.spec.id}`} />
          </section>
        )}
      </div>
    </div>
  );
}
