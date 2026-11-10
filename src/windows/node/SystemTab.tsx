import { TriangleAlert } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { Icon } from "../../components/Icon";
import { Table, type Column } from "../../components/Table";
import { AdvisoryGlyph, Badge, CodeBlock, HelpTip, SectionTitle } from "../../components/ui";
import { inTauri } from "../../lib/api";
import { bootsApi, endedWords, seenWords, useBoots, type Boot } from "../../lib/boots";
import { showError } from "../../lib/errors";
import { fmtDateTime, fmtDuration, fmtMs, fmtRelative } from "../../lib/format";
import { listNames, shortFingerprint } from "../../lib/nodeStatus";
import { useCluster } from "../../lib/store";
import type { NodeSnapshot } from "../../lib/types";
import { agentHealthy, useWatchState } from "../../lib/agent";
import { useWM } from "../../wm/WindowManager";
import { loginApi, type NodeLogin } from "../settings/LoginSection";

/** Clock drift beyond this breaks NCCL timeouts and TLS; it warns. */
const NTP_WARN_MS = 100;

function Rows({ title, rows }: { title: ReactNode; rows: [string, ReactNode][] }) {
  return (
    <section className="mon-section">
      <div className="section-title">{title}</div>
      <div className="boxed mon-kv-box">
        <dl className="kv">
          {rows.map(([k, v]) => (
            <div key={k}>
              <dt>{k}</dt>
              <dd>{v ?? <span className="muted">Not reported</span>}</dd>
            </div>
          ))}
        </dl>
      </div>
    </section>
  );
}

const unknown = <span className="muted">Not reported</span>;

const DAY_MS = 86_400_000;

/** "2 in the last 24 h · last: Xid 79 (GPU fell off the bus), 3 h ago", linking to Alerts › Events. */
function KernelEvents({ node }: { node: NodeSnapshot }) {
  const wm = useWM();
  const events = node.kernelEvents;
  if (events === undefined) return unknown;
  const now = Date.now();
  const recent = events.filter((e) => now - e.tsMs < DAY_MS);
  if (!recent.length) return <span>None in the last 24 h</span>;
  const last = [...recent].sort((a, b) => b.tsMs - a.tsMs)[0];
  const crit = recent.some((e) => e.severity === "crit");
  // Link changes (a cable plugged or a port coming up at boot) are information: no amber.
  const info = recent.every((e) => e.severity === "info");
  const what = last.kind === "xid" ? `Xid ${last.code ?? "?"}` : last.kind === "oom" ? "Out of memory" : last.kind === "hung" ? "Hung task" : last.kind === "link" ? "a link change" : "mlx5";
  return (
    <span className={crit ? "crit-text" : info ? undefined : "warn-text"}>
      {!info && <Icon icon={TriangleAlert} size={12} />} {recent.length} in the last 24 h · last {what}, {fmtRelative(last.tsMs, now)}{" "}
      <button type="button" className="mon-link" onClick={() => wm.open("alerts", { tab: "events", node: node.id })}>Show in Alerts</button>
    </span>
  );
}

/** The always-on agent on this node: its status from the agents' state, else its heartbeat in the snapshot. */
function AgentRow({ node }: { node: NodeSnapshot }) {
  const watch = useWatchState();
  const now = Date.now();
  const st = watch.agents.find((a) => a.node === node.id);
  if (st) {
    if (!st.installed) return <span>Not installed · SparkBox watches this node only while it's open</span>;
    const ok = agentHealthy(st, now);
    return (
      <span className={ok ? undefined : "warn-text"} title={st.error ?? (st.lastBeatMs ? `Last heartbeat ${fmtDateTime(st.lastBeatMs, now)}` : undefined)}>
        {!ok && <Icon icon={TriangleAlert} size={12} />} {ok ? "Watching" : st.state === "running" ? "Not heard from" : st.state === "failed" ? "Failed" : "Stopped"}
        {st.version ? ` · version ${st.version}` : ""}
        {st.lastBeatMs ? ` · heartbeat ${fmtRelative(st.lastBeatMs, now)}` : ""}
        {st.historyFromMs ? ` · history since ${fmtDateTime(st.historyFromMs, now)}` : ""}
        {st.firing ? ` · ${st.firing} firing` : ""}
        {st.simulated ? " · simulated" : ""}
      </span>
    );
  }
  const a = node.agent;
  if (a === undefined) return unknown;
  if (a === null) return <span>Not installed · SparkBox watches this node only while it's open</span>;
  const late = now - a.lastBeatMs > 90_000;
  return (
    <span className={late ? "warn-text" : undefined} title={`Last heartbeat ${fmtDateTime(a.lastBeatMs, now)}`}>
      {late && <Icon icon={TriangleAlert} size={12} />} {late ? "Not heard from" : "Watching"} · version {a.version} · heartbeat {fmtRelative(a.lastBeatMs, now)}
      {a.historyFromMs ? ` · history since ${fmtDateTime(a.historyFromMs, now)}` : ""}
      {a.firing ? ` · ${a.firing} firing` : ""}
    </span>
  );
}

/** How SparkBox logged in to each node (Settings › Cluster login), refreshed while the tab is open. */
function useLogins(): NodeLogin[] {
  const [logins, setLogins] = useState<NodeLogin[]>([]);
  useEffect(() => {
    if (!inTauri) return;
    let alive = true;
    const load = () => loginApi.methods().then((l) => alive && setLogins(l), () => {});
    load();
    const t = window.setInterval(load, 10_000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);
  return logins;
}

/** "Password (shared)", "SSH key · ~/.ssh/id_ed25519". */
export function loginWords(l: Pick<NodeLogin, "method" | "detail"> | undefined): ReactNode {
  if (!l?.method) return unknown;
  if (l.method === "password") return "Password (shared)";
  return `SSH key · ${(l.detail ?? "").replace(/ via agent$/, " (agent)")}`;
}

/**
 * What the shared key means, for a node pinned to the same host key as others (a cloned image):
 * "Shared with cell1, cell3 (cloned image): any node holding this key can pass as the others. Fix:
 * regenerate /etc/ssh/ssh_host_* on cell2 and cell3, then re-trust them (your call; not planned)."
 * The first of them by name keeps its key. Null when the key isn't shared.
 */
export function cloneAdvisory(name: string, sharedWith: readonly string[] | undefined): string | null {
  if (!sharedWith?.length) return null;
  const group = [name, ...sharedWith].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  return `Shared with ${sharedWith.join(", ")} (cloned image): any node holding this key can pass as the others. Fix: regenerate /etc/ssh/ssh_host_* on ${listNames(group.slice(1))}, then re-trust them (your call; not planned).`;
}

/** "SHA256:example…", and what sharing it with other nodes means: the key it presents, and who presents it too. */
export function hostKeyWords(node: NodeSnapshot): ReactNode {
  const k = node.hostKey;
  if (!k) return unknown;
  const shared = cloneAdvisory(node.name, k.sharedWith);
  return (
    <span title={`${k.fingerprint} at ${k.address}`}>
      <span className="mono">{shortFingerprint(k.fingerprint)}</span>
      {shared && <span className="muted"><br /><AdvisoryGlyph title="Worth knowing: your call, not a fault" size={12} /> {shared}</span>}
    </span>
  );
}

/** Started, ended, how it ended and how SparkBox knows of it, newest first (System › Boots). */
function bootColumns(rows: Boot[], current: string | null | undefined): Column<Boot>[] {
  const running = (b: Boot) => !!current && b.bootId === current.replace(/-/g, "").toLowerCase();
  const next = (b: Boot) => rows[rows.indexOf(b) - 1];
  return [
    { key: "start", label: "Started", fit: "content", priority: 1, sort: (b) => b.startMs, render: (b) => <span className="num">{fmtDateTime(b.startMs)}</span> },
    { key: "end", label: "Ended", fit: "content", priority: 1, sort: (b) => b.endMs ?? Number.MAX_SAFE_INTEGER, render: (b) => (running(b) ? <span className="muted">Running</span> : b.endMs ? <span className="num">{fmtDateTime(b.endMs)}</span> : <span className="muted">—</span>) },
    {
      key: "how", label: "How it ended", fit: "flex", priority: 1, sort: (b) => b.ended ?? "",
      render: (b) => <span className={b.ended === "unclean" && !running(b) ? "warn-text" : running(b) || !b.ended ? "muted" : undefined}>{endedWords(b, running(b), next(b))}</span>,
    },
    { key: "seen", label: "Seen", fit: "content", priority: 2, sort: (b) => b.source, render: (b) => seenWords(b) },
  ];
}

/** System › Boots: every boot SparkBox knows of, from the bootIds it saw and the node's journal. */
function Boots({ node }: { node: NodeSnapshot }) {
  const demo = useCluster()?.demo ?? true;
  const [view, setView] = useBoots(node.id, !demo, node.bootId);
  const [busy, setBusy] = useState(false);
  const reread = async () => {
    setBusy(true);
    try {
      setView(await bootsApi.refresh(node.id));
    } catch (e) {
      showError(e, { action: `Read ${node.name}'s journal`, node: node.id });
    } finally {
      setBusy(false);
    }
  };
  const actions = !demo && (
    <button type="button" className="mon-link" disabled={busy || node.status !== "online" || view?.reading} onClick={reread}>
      {busy || view?.reading ? "Reading the journal…" : "Read the journal again"}
    </button>
  );
  const rows = view?.boots ?? [];
  const lines = rows.filter((b) => b.lastLines?.length && b.ended);
  return (
    <section className="mon-section">
      <SectionTitle title="Boots" infoTopic="mon.node.boots" actions={actions || undefined} />
      {demo ? (
        <p className="muted">Available with Live data.</p>
      ) : (
        <>
          <div className="boxed">
            <Table tableId="node-boots" ariaLabel="Boots" columns={bootColumns(rows, node.bootId)} rows={rows} rowKey={(b) => b.bootId}
              rowClass={(b) => (b.ended === "unclean" ? "row-warn" : undefined)}
              empty={view ? "No boots known yet." : "Reading…"} />
          </div>
          {view?.currentOnly && <p className="muted">This node's journal keeps only the current boot.</p>}
          {view?.error && <p className="warn-text"><Icon icon={TriangleAlert} size={12} /> The journal couldn't be read: {view.error}</p>}
          {lines.map((b) => (
            <details key={b.bootId} className="ops-raw">
              <summary>Last lines of the boot that {b.endMs ? `ended ${fmtDateTime(b.endMs)}` : `started ${fmtDateTime(b.startMs)}`}</summary>
              <CodeBlock text={(b.lastLines ?? []).join("\n")} maxHeight={220} label="Last journal lines" />
            </details>
          ))}
        </>
      )}
    </section>
  );
}

/** DGX OS from /etc/dgx-release: the build the node was imaged with, and the last OTA with its date. */
export function dgxRows(s: NodeSnapshot["system"], demo: boolean): [string, ReactNode][] {
  const missing = demo ? <span className="muted">Available with Live data</span> : unknown;
  return [
    ["DGX OS build", s.dgxBuild ?? missing],
    ["DGX OS OTA", s.dgxOta ? (s.dgxOtaDate ? `${s.dgxOta} · installed ${s.dgxOtaDate.replace(/\s+/g, " ")}` : s.dgxOta) : missing],
  ];
}

export function SystemTab({ node }: { node: NodeSnapshot }) {
  const s = node.system;
  const demo = useCluster()?.demo ?? true;
  const logins = useLogins();
  const offset = s.clockOffsetMs;
  const ntp =
    s.ntpSynced == null ? unknown : (
      <span className={!s.ntpSynced || (offset != null && Math.abs(offset) > NTP_WARN_MS) ? "warn-text" : undefined}>
        {!s.ntpSynced && <Icon icon={TriangleAlert} size={12} />} {s.ntpSynced ? "Synchronized" : "Not synchronized"}
        {offset != null && ` · ${Math.abs(offset) < 1 ? Math.abs(offset).toFixed(2) : Math.abs(offset).toFixed(0)} ms ${offset >= 0 ? "ahead" : "behind"}`}
        {offset != null && Math.abs(offset) > NTP_WARN_MS && " (over 100 ms)"}
        {offset == null && s.ntpSynced && <span className="muted"> · offset not reported by this node</span>}
      </span>
    );
  return (
    <div className="mon-node-body">
      <Rows
        title="Machine"
        rows={[
          ["Hostname", s.hostname || unknown],
          ["Address", <span className="mono">{node.host}</span>],
          ["Model", s.model || unknown],
          ["BIOS", s.bios ?? unknown],
          ["Uptime", node.status === "online" && s.uptimeS ? `${fmtDuration(s.uptimeS)} · since ${fmtDateTime(Date.now() - s.uptimeS * 1000)}` : unknown],
          ["Restart needed", s.rebootRequired ? <Badge tone="warn">Yes, to finish installing updates</Badge> : "No"],
        ]}
      />
      <Rows
        title="Software"
        rows={[
          ["Operating system", s.os || unknown],
          ...dgxRows(s, demo),
          ["Kernel", s.kernel ? <span className="mono">{s.kernel}</span> : unknown],
          ["NVIDIA driver", s.driver || unknown],
          ["CUDA", s.cuda || unknown],
          ["ConnectX-7 firmware", s.cx7Fw ?? unknown],
          ["Docker", s.docker ?? unknown],
        ]}
      />
      <Rows
        title="Time and connection"
        rows={[
          ["Clock (NTP)", ntp],
          ["Login", loginWords(logins.find((l) => l.node === node.id))],
          ["Host key", hostKeyWords(node)],
          ["SSH round trip", node.latencyMs != null ? <span title="Round trip of SparkBox's last command over SSH">{fmtMs(node.latencyMs)}</span> : unknown],
          ["Last reading", node.lastSeenMs ? fmtDateTime(node.lastSeenMs) : unknown],
        ]}
      />
      <Rows
        title={<>Watching <HelpTip topic="mon.node.watching" /></>}
        rows={[
          ["Kernel events", <KernelEvents node={node} />],
          ["Agent", <AgentRow node={node} />],
        ]}
      />
      <Boots node={node} />
    </div>
  );
}
