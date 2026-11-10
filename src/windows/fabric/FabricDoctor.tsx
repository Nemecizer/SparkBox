// The Fabric doctor ("Test fabric"): pick nodes and checks, run it as a task, and read
// the MTU audit, the jumbo-ping matrix and perftest bandwidth and latency per pair.
import { useEffect, useMemo, useState } from "react";
import { CircleStop, Play, RotateCcw } from "lucide-react";
import { HelpTip, InfoTip, Notice, SectionTitle, Spacer, StatusDot, ToolButton } from "../../components/ui";
import { ADMIN_REFUSED, requestAdmin, useAdmin, useAdminState, unlockedFor, withAdmin } from "../../lib/admin";
import { fabricApi, halfLabel, shortPort, type BwCell, type DoctorChecks, type DoctorReport, type DoctorStatus, type PingCell } from "../../lib/fabric";
import { fmtDateTime } from "../../lib/format";
import { confirmAction } from "../../lib/session";
import { cancelTask, useTask } from "../../lib/tasks";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";
import { useWinState } from "../../wm/WindowManager";

const CHECKS: { key: keyof DoctorChecks; label: string; help: string }[] = [
  { key: "mtu", label: "MTU audit", help: "mon.fabric.check-mtu" },
  { key: "ping", label: "Jumbo-frame ping", help: "mon.fabric.check-ping" },
  { key: "bandwidth", label: "Bandwidth (ib_write_bw)", help: "mon.fabric.check-bandwidth" },
  { key: "latency", label: "Latency (ib_send_lat)", help: "mon.fabric.check-latency" },
  { key: "nccl", label: "All-reduce (nccl-tests)", help: "mon.fabric.check-allreduce" },
];

const STATUS_DOT: Record<DoctorStatus, "online" | "warn" | "crit" | "unconfigured" | "connecting"> = {
  pass: "online",
  warn: "warn",
  fail: "crit",
  skipped: "unconfigured",
  running: "connecting",
};

function describe(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e);
}

export function FabricDoctor() {
  const cluster = useCluster();
  const admin = useAdmin();
  const adminState = useAdminState();
  const [report, setReport] = useState<DoctorReport | null>(null);
  const online = useMemo(() => (cluster?.nodes ?? []).filter((n) => n.status === "online").map((n) => n.id), [cluster?.nodes]);
  const [nodes, setNodes] = useWinState<string[] | null>("doctor.nodes", null);
  const [checks, setChecks] = useWinState<DoctorChecks>("doctor.checks", { mtu: true, ping: true, bandwidth: true, latency: true, nccl: false });
  const [expected, setExpected] = useWinState<string>("doctor.expected", "92");
  const task = useTask(report?.taskId);
  const running = !!report && report.finishedMs == null && task?.state !== "failed" && task?.state !== "cancelled";
  const chosen = nodes ?? online;

  useEffect(() => {
    let alive = true;
    void fabricApi.doctorReport().then((r) => alive && r && setReport(r)).catch(() => {});
    const un = fabricApi.onDoctor((r) => alive && setReport(r));
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, []);

  const name = (id: string) => cluster?.nodes.find((n) => n.id === id)?.name ?? id;
  const toggleNode = (id: string) => setNodes(chosen.includes(id) ? chosen.filter((x) => x !== id) : [...chosen, id]);

  const start = async () => {
    const gbps = Number(expected);
    try {
      await fabricApi.doctorStart({ nodes: chosen, checks, expectedGbps: Number.isFinite(gbps) && gbps > 0 ? gbps : null });
    } catch (e) {
      toast({ kind: "error", title: "The fabric test didn't start", body: describe(e) });
    }
  };

  const install = async (node: string) => {
    if (!unlockedFor("node", adminState) && !(await requestAdmin("install perftest", "node"))) return;
    const ok = await confirmAction({
      title: `Install perftest on ${name(node)}?`,
      body: "Runs sudo apt-get install -y perftest. It adds ib_write_bw, ib_send_lat and the other RDMA tests; nothing else changes.",
      confirmLabel: "Install",
    });
    if (!ok) return;
    try {
      await fabricApi.perftestInstall([node]);
    } catch (e) {
      toast({ kind: "error", title: "perftest wasn't installed", body: describe(e) });
    }
  };

  const clearBaselines = async () => {
    const ok = await confirmAction({
      title: "Clear the bandwidth baselines?",
      body: "The next run where every pair passes becomes the new baseline.",
      confirmLabel: "Clear baselines",
    });
    if (!ok) return;
    try {
      await withAdmin(() => fabricApi.baselinesReset(), { scope: "mac", action: "fabric_baselines_reset" });
      toast({ kind: "success", title: "Baselines cleared" });
    } catch (e) {
      if (e !== ADMIN_REFUSED) toast({ kind: "error", title: "Couldn't clear the baselines", body: describe(e) });
    }
  };

  return (
    <div className="fab-doctor">
      <div className="fab-doctor-bar" role="toolbar" aria-label="Fabric test">
        {running ? (
          <ToolButton icon={CircleStop} label="Stop" onClick={() => report?.taskId && cancelTask(report.taskId)} danger />
        ) : (
          <ToolButton icon={Play} label="Test fabric" onClick={start} disabled={!chosen.length || !Object.values(checks).some(Boolean)} locked={!admin} />
        )}
        <HelpTip topic="mon.fabric.doctor" />
        <Spacer />
        <ToolButton icon={RotateCcw} label="Clear baselines…" onClick={clearBaselines} locked={!unlockedFor("mac", adminState)} scope="mac" priority={3} disabled={running} />
        <HelpTip topic="mon.fabric.baselines" />
      </div>
      <div className="fab-doctor-setup">
        <fieldset className="fab-fs">
          <legend>Nodes</legend>
          {(cluster?.nodes ?? []).map((n) => (
            <label key={n.id} className={`fab-check${n.status !== "online" ? " off" : ""}`}>
              <input type="checkbox" checked={chosen.includes(n.id)} disabled={running || n.status !== "online"} onChange={() => toggleNode(n.id)} />
              <span>{n.name}</span>
              {n.status !== "online" && <span className="muted"> · offline</span>}
            </label>
          ))}
        </fieldset>
        <fieldset className="fab-fs">
          <legend>Checks</legend>
          {CHECKS.map((c) => (
            <span key={c.key} className="fab-check">
              <label>
                <input type="checkbox" checked={checks[c.key]} disabled={running} onChange={(e) => setChecks({ ...checks, [c.key]: e.target.checked })} />
                <span>{c.label}</span>
              </label>
              <InfoTip topic={c.help} label={`About ${c.label}`} />
            </span>
          ))}
        </fieldset>
        <fieldset className="fab-fs">
          <legend>Expected per half</legend>
          <label className="fab-expected">
            <input type="number" min={1} max={400} step={1} value={expected} disabled={running} onChange={(e) => setExpected(e.target.value)} aria-label="Expected Gb/s per half" />
            <span className="muted">Gb/s</span>
          </label>
          <HelpTip topic="mon.fabric.expected" label="About the expected bandwidth" />
        </fieldset>
      </div>
      {report ? <Report report={report} name={name} onInstall={install} running={running} /> : <p className="muted fab-doctor-empty">No test has run yet.</p>}
    </div>
  );
}

function Report({ report, name, onInstall, running }: { report: DoctorReport; name: (id: string) => string; onInstall: (node: string) => void; running: boolean }) {
  const halves = ([0, 1] as const).filter((h) => report.ping.some((p) => p.half === h) || report.bandwidth.some((b) => b.half === h));
  return (
    <div className="fab-report">
      <div className="fab-report-head">
        <span className="strong">{running ? report.current ?? "Running…" : report.cancelled ? "Stopped" : report.findings.length ? `${report.findings.length} finding${report.findings.length === 1 ? "" : "s"}` : "Everything passed"}</span>
        <span className="muted">{fmtDateTime(report.startedMs)}{report.demo ? " · Demo" : ""}</span>
        {report.baselineSaved && <span className="muted">· saved as the baseline</span>}
      </div>
      {report.prereqs.map((p) => (
        <Notice key={p.node} tone="warn" action={p.package ? { label: "Install…", onClick: () => onInstall(p.node) } : undefined}>
          {p.message}: {p.missing.join(" and ")} {p.missing.length === 1 ? "is" : "are"} missing, so its pairs were skipped.
        </Notice>
      ))}
      {/* A missing-tool finding already has its notice (with Install…) above: list only the rest. */}
      {(() => {
        const rest = report.findings.filter((f) => !report.prereqs.some((p) => f.startsWith(p.message)));
        return rest.length > 0 && (
          <ul className="fab-findings">
            {rest.map((f) => <li key={f}>{f}</li>)}
          </ul>
        );
      })()}
      {report.mtu.length > 0 && (
        <>
          <SectionTitle title="MTU audit" />
          <table className="grid compact fab-mtu">
            <thead>
              <tr><th>Node</th><th>Half</th><th>Interface</th><th className="num">MTU</th><th>Switch port</th><th className="num">Port L2MTU</th><th>Result</th></tr>
            </thead>
            <tbody>
              {report.mtu.map((m) => (
                <tr key={`${m.node}/${m.iface}`}>
                  <td>{name(m.node)}</td>
                  <td>{halfLabel(m.half)}</td>
                  <td className="mono">{m.iface}</td>
                  <td className={`num${m.status === "fail" ? " crit-text" : ""}`}>{m.mtu ?? "—"}</td>
                  <td>{m.switchPort ? shortPort(m.switchPort) : "—"}</td>
                  <td className="num">{m.switchMtu ?? "—"}</td>
                  <td><StatusDot status={STATUS_DOT[m.status]} text={m.status === "pass" ? "OK" : m.message} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {halves.map((h) => (
        <PairMatrix key={h} half={h} report={report} name={name} />
      ))}
      {report.bandwidth.length > 0 && <Aggregate report={report} name={name} />}
      {report.nccl && (
        <>
          <SectionTitle title="All-reduce" />
          <p className="fab-nccl">
            <StatusDot status={STATUS_DOT[report.nccl.status]} text={report.nccl.message} />
          </p>
        </>
      )}
    </div>
  );
}

/** Rows send, columns receive: the jumbo ping result and, for measured pairs, Gb/s and µs. */
function PairMatrix({ half, report, name }: { half: 0 | 1; report: DoctorReport; name: (id: string) => string }) {
  const nodes = report.nodes.filter((n) => report.ping.some((p) => p.half === half && (p.from === n || p.to === n)) || report.bandwidth.some((b) => b.half === half && (b.from === n || b.to === n)));
  const ping = (a: string, b: string): PingCell | undefined => report.ping.find((p) => p.half === half && p.from === a && p.to === b);
  const bw = (a: string, b: string): BwCell | undefined => report.bandwidth.find((c) => c.half === half && ((c.from === a && c.to === b) || (c.from === b && c.to === a)));
  return (
    <>
      <SectionTitle
        title={halfLabel(half)}
        suffix={half === 0 ? "192.168.100.x" : "192.168.101.x"}
        infoTopic="mon.fabric.results"
      />
      <div className="fab-pairs-wrap">
        <table className="grid compact fab-pairs">
          <thead>
            <tr>
              <th>From ↓ · to →</th>
              {nodes.map((n) => <th key={n}>{name(n)}</th>)}
            </tr>
          </thead>
          <tbody>
            {nodes.map((a) => (
              <tr key={a}>
                <th scope="row">{name(a)}</th>
                {nodes.map((b) => {
                  if (a === b) return <td key={b} className="fab-pair self" aria-label="Same node">—</td>;
                  const p = ping(a, b);
                  const c = bw(a, b);
                  const bad = p?.status === "fail" || c?.status === "fail";
                  const warn = c?.status === "warn";
                  return (
                    <td key={b} className={`fab-pair${bad ? " tone-crit" : warn ? " tone-warn" : ""}`} title={[p?.detail, c?.message].filter(Boolean).join(" · ") || undefined}>
                      <span className={p && p.result !== "ok" ? "crit-text" : "muted"}>{p ? (p.result === "ok" ? `ping ok · ${p.rttMs?.toFixed(2)} ms` : p.result) : "—"}</span>
                      {c && (
                        <span className="num fab-pair-bw">
                          {c.status === "running" ? "measuring…" : c.gbps != null ? `${c.gbps.toFixed(1)} Gb/s` : c.status === "skipped" ? "skipped" : "failed"}
                          {c.latUs != null && <span className="muted"> · {c.latUs.toFixed(2)} µs</span>}
                        </span>
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

/** Both halves together per pair, against twice the expected rate. */
function Aggregate({ report, name }: { report: DoctorReport; name: (id: string) => string }) {
  const pairs = [...new Set(report.bandwidth.map((c) => `${c.from}>${c.to}`))];
  return (
    <>
      <SectionTitle title="Both halves" infoTopic="mon.fabric.both-halves" />
      <table className="grid compact fab-agg">
        <thead>
          <tr><th>Pair</th><th className="num">Half 1</th><th className="num">Half 2</th><th className="num">Together</th><th className="num">Expected</th><th>Result</th></tr>
        </thead>
        <tbody>
          {pairs.map((k) => {
            const [a, b] = k.split(">");
            const h = (half: 0 | 1) => report.bandwidth.find((c) => c.from === a && c.to === b && c.half === half);
            const [h1, h2] = [h(0), h(1)];
            const sum = h1?.gbps != null && h2?.gbps != null ? h1.gbps + h2.gbps : null;
            const want = report.expectedGbps * 2;
            const worst: DoctorStatus = [h1, h2].some((c) => c?.status === "fail") ? "fail" : [h1, h2].some((c) => c?.status === "warn") ? "warn" : [h1, h2].some((c) => c?.status === "running") ? "running" : sum != null ? "pass" : "skipped";
            return (
              <tr key={k}>
                <td>{name(a)} → {name(b)}</td>
                <td className="num">{h1?.gbps != null ? h1.gbps.toFixed(1) : "—"}</td>
                <td className="num">{h2?.gbps != null ? h2.gbps.toFixed(1) : "—"}</td>
                <td className="num strong">{sum != null ? `${sum.toFixed(1)} Gb/s` : "—"}</td>
                <td className="num muted">{want.toFixed(0)} Gb/s</td>
                <td><StatusDot status={STATUS_DOT[worst]} text={worst === "pass" ? "Pass" : worst === "running" ? "Measuring" : worst === "skipped" ? "Skipped" : [h1, h2].find((c) => c && c.status !== "pass")?.message ?? ""} /></td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}

