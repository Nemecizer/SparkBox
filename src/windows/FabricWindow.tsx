import { useEffect, useMemo } from "react";
import { Cable, CircleAlert, Network, Stethoscope, TriangleAlert, X } from "lucide-react";
import { AdvisoryGlyph, EmptyState, HelpTip, InfoTip, Notice, SectionTitle, Spacer, Sparkline, Tabs, Toolbar, ToolButton, Truncate, Waiting } from "../components/ui";
import { ChartGrid, TimeChart } from "../components/TimeChart";
import { unlockedFor, useAdminState, withAdmin } from "../lib/admin";
import { counterInfo, fabricApi, halfLabel, shortPort, useFabricHealth, type FabricState, type IfaceHealth } from "../lib/fabric";
import { fmtBits, fmtRatePair } from "../lib/format";
import { useHistory, type HistoryMetric } from "../lib/historyApi";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { toast } from "../lib/toasts";
import { useWindowId, useWinState, type Params } from "../wm/WindowManager";
import { ChipRow, ecnNote, fmtPerS, fmtTotal, quietChips, sortCabling, useFabricRing, useHourStats, VerdictMark } from "./fabric/common";
import { FabricDoctor } from "./fabric/FabricDoctor";
import { lldpView } from "./node/NetworkTab";
import "./fabric/fabric.css";

/** RoceCounters keys → the series history keeps for `net:<node>/<iface>`. */
const HISTORY_OF: Record<string, string> = {
  ecnMarked: "ecn",
  cnpSent: "cnpSent",
  cnpHandled: "cnpHandled",
  outOfSequence: "oos",
  packetSeqErr: "seqErr",
  localAckTimeoutErr: "timeouts",
  rxPause: "pauseRx",
  txPause: "pauseTx",
  rxDiscardsPhy: "discards",
  rxCrcErrorsPhy: "crc",
};
const NET_METRICS = Object.values(HISTORY_OF) as unknown as HistoryMetric[];

const byWord = (m: string | null | undefined) => (m === "mac" ? "MAC" : m === "lldp" ? "LLDP" : m === "comment" ? "comment" : "—");

function describe(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e);
}

/** Is the RoCE fabric lossless, is NCCL on RDMA, does every link deliver? */
export function FabricWindow({ params }: { params: Params }) {
  const state = useFabricHealth();
  const cluster = useCluster();
  const adminState = useAdminState();
  const winId = useWindowId();
  const [tab, setTab] = useWinState<string>("tab", params?.tab ?? "matrix");
  const [sel, setSel] = useWinState<string | null>("cell", params?.cell ?? null);
  const ring = useFabricRing(state);
  // A link (the palette, a node's RoCE panel) asks for a tab or a cell: follow it.
  useEffect(() => {
    if (params.tab) setTab(params.tab);
    if (params.cell) {
      setSel(params.cell);
      setTab("matrix");
    }
    // Only when a link asks, not on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [params.tab, params.cell]);

  if (!cluster || !state) return <Waiting label="Waiting for the first fabric reading…" />;
  if (!state.ifaces.length) return <EmptyState icon={Network} title="No fabric interfaces" body="No node reports an addressed ConnectX-7 interface yet." />;

  const selected = state.ifaces.find((i) => `${i.node}/${i.iface}` === sel) ?? null;
  const accept = async () => {
    const ok = await confirmAction({
      title: "Accept the current cabling?",
      body: "SparkBox remembers which switch port each node's fabric cable is in now, and stops flagging these moves.",
      details: state.cabling.length ? <ul className="impact-list">{sortCabling(state.cabling, cluster.nodes.map((n) => n.id)).map((c) => <li key={`${c.node}/${c.iface}`}>{c.message}</li>)}</ul> : undefined,
      confirmLabel: "Accept cabling",
    });
    if (!ok) return;
    try {
      const n = await withAdmin(() => fabricApi.acceptCabling(), { scope: "mac", action: "fabric_cabling_accept" });
      toast({ kind: "success", title: "Cabling accepted", body: `${n} fabric interfaces recorded`, origin: winId || undefined });
    } catch (e) {
      if ((e as { kind?: string })?.kind === "cancelled") return;
      toast({ kind: "error", title: "Couldn't accept the cabling", body: describe(e) });
    }
  };

  return (
    <div className="fab-win">
      <Toolbar label="Fabric actions">
        <ToolButton icon={Stethoscope} label="Test fabric…" onClick={() => setTab("doctor")} title="Open the doctor: MTU, jumbo ping, bandwidth and latency" />
        <ToolButton icon={Cable} label="Accept current cabling…" onClick={accept} disabled={!state.cabling.length} locked={!unlockedFor("mac", adminState)} scope="mac" priority={2} />
        <Spacer />
        {cluster.demo && <span className="fab-demo muted">Demo data</span>}
      </Toolbar>
      <VerdictBand state={state} />
      <div className="fab-main">
        <div className="fab-content">
          <Tabs
            label="Fabric views"
            value={tab}
            onChange={setTab}
            tabs={[
              { id: "matrix", label: "Matrix" },
              { id: "doctor", label: "Doctor" },
              { id: "history", label: "History" },
            ]}
          >
            {tab === "matrix" && <Matrix state={state} ring={ring} selected={sel} onSelect={setSel} />}
            {tab === "doctor" && <FabricDoctor />}
            {tab === "history" && <History state={state} selected={selected} onSelect={setSel} />}
          </Tabs>
        </div>
        {tab === "matrix" && selected && <CellDrawer i={selected} onClose={() => setSel(null)} />}
      </div>
    </div>
  );
}

/** Quiet when healthy; an amber or red band with the evidence and what to do otherwise. */
function VerdictBand({ state }: { state: FabricState }) {
  const v = state.verdict;
  const a = state.advisory;
  if (a && (v.level === "ok" || v.level === "info")) {
    // Known configuration (PFC off): one grey ⓘ banner with what it costs, never a warning.
    return (
      <div className="fab-band level-info fab-advisory" role="status">
        <AdvisoryGlyph title={a.short} size={14} />
        <div className="fab-band-text">
          <div className="fab-band-title"><b>{a.title}</b> {a.body}</div>
        </div>
      </div>
    );
  }
  if (v.level === "ok" || v.level === "info") {
    return (
      <div className={`fab-band level-${v.level}`} role="status">
        <VerdictMark level={v.level} title={v.title} />
        <span className="fab-band-title">{state.summary}</span>
        <InfoTip topic="mon.fabric.verdict" label="About the fabric verdict">
          <p>{v.detail}</p>
          {v.action && <p>{v.action}</p>}
          <p>A healthy fabric loses nothing. A little congestion under heavy traffic is normal. Packets sent again, damaged data and long pauses aren't.</p>
        </InfoTip>
      </div>
    );
  }
  const Icon = v.level === "crit" ? CircleAlert : TriangleAlert;
  return (
    <div className={`fab-band level-${v.level}`} role="alert">
      <Icon size={16} className="fab-band-icon" aria-hidden />
      <div className="fab-band-text">
        <div className="fab-band-title">{v.title}</div>
        <div className="fab-band-detail">{v.detail}{v.action ? ` ${v.action}` : ""}</div>
      </div>
    </div>
  );
}

function Matrix({ state, ring, selected, onSelect }: { state: FabricState; ring: ReturnType<typeof useFabricRing>; selected: string | null; onSelect: (k: string | null) => void }) {
  const cluster = useCluster();
  const nodes = useMemo(() => {
    const ids = [...new Set(state.ifaces.map((i) => i.node))];
    const order = cluster?.nodes.map((n) => n.id) ?? [];
    return ids.sort((a, b) => order.indexOf(a) - order.indexOf(b));
  }, [state.ifaces, cluster?.nodes]);
  const subnet = (half: 0 | 1) => (half === 0 ? "192.168.100.x" : "192.168.101.x");
  return (
    <div className="fab-matrix-wrap">
      <div className="fab-matrix" role="grid" aria-label="Fabric interfaces by node and half">
        <div className="fab-mh" role="row">
          <span role="columnheader" className="fab-mh-node">Node</span>
          {([0, 1] as const).map((h) => (
            <span role="columnheader" key={h} className="fab-mh-half">
              <span className="strong">Port 2 · {halfLabel(h).toLowerCase()}</span>
              <span className="muted mono">{subnet(h)}</span>
            </span>
          ))}
        </div>
        {nodes.map((id) => {
          const node = cluster?.nodes.find((n) => n.id === id);
          const offline = node && node.status !== "online";
          return (
            <div key={id} className="fab-mrow" role="row">
              <span role="rowheader" className="fab-mnode">
                <span className="strong">{node?.name ?? id}</span>
                {offline ? <span className="crit-text">Offline</span> : <span className="muted small">{node?.system.model ?? ""}</span>}
              </span>
              {([0, 1] as const).map((h) => {
                const i = state.ifaces.find((x) => x.node === id && x.half === h);
                if (!i) return <span key={h} role="gridcell" className="fab-cell empty muted">Not cabled</span>;
                const key = `${i.node}/${i.iface}`;
                return <Cell key={h} i={i} values={ring.values(key)} on={selected === key} onClick={() => onSelect(selected === key ? null : key)} />;
              })}
            </div>
          );
        })}
      </div>
      <Transport state={state} />
    </div>
  );
}

/** The least a cell's traffic line is scaled to, in b/s. */
const SPARK_FLOOR_BPS = 10e6;

function Cell({ i, values, on, onClick }: { i: IfaceHealth; values: (number | null)[]; on: boolean; onClick: () => void }) {
  const v = i.verdict;
  const loud = v.level === "warn" || v.level === "crit";
  const mtuBad = i.mtu != null && i.mtu !== 9000;
  const share = i.rdmaShare != null ? `RDMA ${Math.round(i.rdmaShare * 100)}%` : "RDMA —";
  // At rest a 200G port carries a few hundred b/s of LLDP and neighbour discovery: scaled to that
  // alone it would draw a full-height wave, so the line's top is never under 10 Mb/s.
  const peak = Math.max(SPARK_FLOOR_BPS, ...values.filter((x): x is number => x != null));
  const port = i.switchPort ? shortPort(i.switchPort) : null;
  const hour = useHourStats(i);
  // No switch port mapped: what LLDP heard on the half, as the node's RoCE panel says it.
  const cluster = useCluster();
  const snap = cluster?.nodes.find((n) => n.id === i.node);
  const net = snap?.nets.find((x) => x.iface === i.iface);
  const heard = snap && net ? lldpView([net], snap, cluster?.nodes ?? [snap]) : null;
  return (
    <button type="button" role="gridcell" className={`fab-cell level-${v.level}${on ? " on" : ""}`} onClick={onClick} aria-pressed={on} aria-label={`${i.nodeName} ${halfLabel(i.half).toLowerCase()}: ${v.title}. ${v.detail}`}>
      <span className="fab-cell-top">
        <span className={`fab-verdict${loud ? ` tone-${v.level}` : ""}`}>
          <VerdictMark level={v.level} title={v.title} kind={v.kind} />
          <span>{v.title}</span>
        </span>
        <span className="fab-link num">
          {i.up ? `${i.speedMbps / 1000}G` : <span className="crit-text">No link</span>}
          <span className={mtuBad ? "crit-text" : "muted"}> · MTU {i.mtu ?? "—"}</span>
        </span>
      </span>
      <span className="fab-cell-rates num">
        <span title="Received and sent (IP + RDMA)">{fmtRatePair(i.rxBps + (i.rdmaRxBps ?? 0), i.txBps + (i.rdmaTxBps ?? 0), { arrows: true })}</span>
        <span className="muted" title="Share of this interface's bytes that went over RDMA">{share}</span>
      </span>
      <span className="fab-cell-spark">
        {i.up ? <Sparkline values={values} max={peak} width={200} height={20} color="var(--m-rx)" label={`${i.nodeName} ${halfLabel(i.half).toLowerCase()} traffic, last 3 minutes`} /> : <span className="muted">—</span>}
      </span>
      <ChipRow items={quietChips(i, hour)} quiet={ecnNote(i)} label="Counters that moved in the last hour" />
      <span className="fab-cell-port">
        {port ? (
          <span className="muted">
            Switch <span className="strong">{port}</span> · by {byWord(i.mappedBy)}
          </span>
        ) : (
          <span className="muted" title={heard?.title}>{heard?.text ?? "Switch port unknown"}</span>
        )}
        {i.cablingOk === false && <span className="warn-text">Expected {shortPort(i.expectedPort)}</span>}
      </span>
    </button>
  );
}

function Transport({ state }: { state: FabricState }) {
  const cluster = useCluster();
  if (!state.transport.length && !state.cabling.length) return null;
  const cabling = sortCabling(state.cabling, cluster?.nodes.map((n) => n.id) ?? []);
  return (
    <div className="fab-transport">
      {state.transport.length > 0 && (
        <>
          <SectionTitle
            title="NCCL transport"
            infoTopic="mon.fabric.nccl"
          />
          {state.transport.map((t) => (
            <div key={t.name} className={`fab-tr level-${t.level}`}>
              <VerdictMark level={t.level} title={t.message} />
              <span className="strong">{t.name}</span>
              <span className="muted">{t.nodes.join(", ")}</span>
              <span className={t.level === "warn" ? "warn-text" : undefined}>{t.message}</span>
              {t.envIssues.length > 0 && <ul className="fab-tr-issues">{t.envIssues.map((e) => <li key={e}>{e}</li>)}</ul>}
            </div>
          ))}
        </>
      )}
      {state.cabling.length > 0 && (
        <>
          <SectionTitle title="Cabling" infoTopic="mon.fabric.cabling" />
          <ul className="fab-cabling">
            {cabling.map((c) => <li key={`${c.node}/${c.iface}`} className="warn-text">{c.message}</li>)}
          </ul>
        </>
      )}
    </div>
  );
}

function CellDrawer({ i, onClose }: { i: IfaceHealth; onClose: () => void }) {
  const hist = useHistory(`net:${i.node}/${i.iface}`, NET_METRICS, 3600);
  // No switch port mapped: what LLDP heard on the half, in the Network tab's words.
  const cluster = useCluster();
  const snap = cluster?.nodes.find((n) => n.id === i.node);
  const net = snap?.nets.find((x) => x.iface === i.iface);
  const heard = snap && net ? lldpView([net], snap, cluster?.nodes ?? [snap]) : null;
  // The same last-hour tally as the cell's chips: the recorded history, or SparkBox's own count
  // when it's higher (it restarts from 0 with the app).
  const lossHour = Math.max(i.lossLastHour, useHourStats(i).loss?.count ?? 0);
  const series = (key: string) => {
    const k = HISTORY_OF[key];
    return k ? hist?.series[k] ?? null : null;
  };
  return (
    <aside className="fab-drawer" aria-label={`${i.nodeName} ${halfLabel(i.half).toLowerCase()} counters`}>
      <header className="fab-drawer-head">
        <div className="fab-drawer-titles">
          <div className="fab-drawer-title">{i.nodeName} · {halfLabel(i.half).toLowerCase()}</div>
          <Truncate className="muted mono" text={`${i.iface} · ${i.rdmaDev ?? "no RDMA device"} · ${i.ip ?? ""}`} />
        </div>
        <button type="button" className="tool" onClick={onClose} aria-label="Close counters" title="Close counters">
          <X size={14} />
        </button>
      </header>
      <div className="fab-drawer-body">
        <div className={`fab-drawer-verdict level-${i.verdict.level}`}>
          <div className="strong">{i.verdict.title}</div>
          <div>{i.verdict.detail}</div>
        </div>
        {i.verdict.action && (
          <>
            <h4 className="section-title">Suggested action</h4>
            <p className="fab-action">{i.verdict.action}</p>
          </>
        )}
        <dl className="kv fab-kv">
          <div><dt>Link</dt><dd>{i.up ? `${i.speedMbps / 1000}G` : "No link"}</dd></div>
          <div><dt>MTU</dt><dd className="num">{i.mtu ?? "—"}</dd></div>
          <div><dt>Received</dt><dd className="num">{fmtBits(i.rxBps + (i.rdmaRxBps ?? 0))}</dd></div>
          <div><dt>Sent</dt><dd className="num">{fmtBits(i.txBps + (i.rdmaTxBps ?? 0))}</dd></div>
          <div><dt>Switch port</dt><dd title={heard?.title}>{i.switchPort ? `${shortPort(i.switchPort)} (by ${byWord(i.mappedBy)})` : heard?.text ?? "—"}</dd></div>
          <div><dt>Cabling</dt><dd>{i.cablingOk === false ? `Expected ${shortPort(i.expectedPort)}` : i.cablingOk ? "As learned" : "—"}</dd></div>
          <div><dt>Loss, last hour</dt><dd className="num">{lossHour.toLocaleString()}</dd></div>
        </dl>
        {i.counters.length === 0 ? (
          <p className="muted">This node reports no RoCE counters.</p>
        ) : (
          <table className="grid compact fab-counters" aria-label="Counters">
            <colgroup>
              <col className="fab-col-name" />
              <col className="fab-col-total" />
              <col className="fab-col-now" />
              <col className="fab-col-hour" />
            </colgroup>
            <thead>
              <tr>
                <th>Counter</th>
                <th className="num" title="Cumulative: what the counter holds since the driver last reset it (usually the node's boot)">Since reset</th>
                <th className="num">Now</th>
                <th title="Rate over the last hour">1 h</th>
              </tr>
            </thead>
            <tbody>
              {i.counters.map((c) => {
                const h = series(c.key);
                const moving = c.perS != null && c.perS > 0;
                const bad = moving && (c.group === "loss" || c.group === "phy" || c.group === "drop");
                return (
                  <tr key={c.key}>
                    <th scope="row">
                      <span className="fab-counter-name">
                        <Truncate text={c.label} />
                        {counterInfo(c.key) && <InfoTip topic="mon.fabric.counter" label={`About ${c.label}`}><p>{counterInfo(c.key)}</p></InfoTip>}
                      </span>
                    </th>
                    <td className="num" title={c.total != null ? c.total.toLocaleString() : undefined}>{fmtTotal(c.total)}</td>
                    <td className={`num${bad ? " crit-text" : ""}`}>{c.total == null ? "—" : fmtPerS(c.perS)}</td>
                    <td>
                      {h ? <Sparkline values={h} max={Math.max(1, ...h.filter((x): x is number => x != null))} width={56} height={16} color="var(--m-rx)" label={`${c.label}, last hour`} /> : <span className="muted">—</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </aside>
  );
}

/** Counter rates over the last hour for one interface, p98-clamped (bursts get a ▲ at the edge). */
function History({ state, selected, onSelect }: { state: FabricState; selected: IfaceHealth | null; onSelect: (k: string) => void }) {
  const i = selected ?? state.ifaces.find((x) => x.verdict.level === "crit" || x.verdict.level === "warn") ?? state.ifaces[0];
  const key = `${i.node}/${i.iface}`;
  const hist = useHistory(`net:${key}`, NET_METRICS, 3600);
  const ts = (hist?.ts ?? []).map((t) => t / 1000);
  const s = (k: string) => hist?.series[k] ?? [];
  const now = Date.now() / 1000;
  const common = { ts, range: [now - 3600, now] as [number, number], relative: true, height: 140, robust: "auto" as const, fmt: (v: number) => fmtPerS(v), startsAt: hist?.startsAtMs != null ? hist.startsAtMs / 1000 : null, backfilled: (hist?.agentRanges ?? []).map(([a, b]) => [a / 1000, b / 1000] as [number, number]) };
  return (
    <div className="fab-history">
      <div className="fab-history-pick">
        <label className="muted" htmlFor="fab-hist-iface">Interface</label>
        <select id="fab-hist-iface" value={key} onChange={(e) => onSelect(e.target.value)}>
          {state.ifaces.map((x) => (
            <option key={`${x.node}/${x.iface}`} value={`${x.node}/${x.iface}`}>
              {x.nodeName} · {halfLabel(x.half).toLowerCase()} ({x.iface})
            </option>
          ))}
        </select>
        <HelpTip topic="mon.fabric.history" />
      </div>
      {!hist ? (
        <Notice tone="info">History is kept by SparkBox while it runs; it appears here after the first minute.</Notice>
      ) : (
        <ChartGrid syncKey="fabric-history">
        <div className="fab-charts">
          <TimeChart title="Retransmits" {...common} series={[
            { label: "Out of sequence", color: "--m-rx", values: s("oos") },
            { label: "Sequence errors", color: "--m-tx", values: s("seqErr") },
            { label: "ACK timeouts", color: "--m-ttft", values: s("timeouts") },
          ]} />
          <TimeChart title="PHY errors and drops" {...common} series={[
            { label: "CRC errors", color: "--m-rx", values: s("crc") },
            { label: "Discards", color: "--m-tx", values: s("discards") },
          ]} />
          <TimeChart title="Congestion" {...common} series={[
            { label: "ECN marked", color: "--m-rx", values: s("ecn") },
            { label: "CNPs sent", color: "--m-tx", values: s("cnpSent") },
            { label: "CNPs handled", color: "--m-itl", values: s("cnpHandled") },
          ]} />
          <TimeChart title="Pause frames" {...common} thresholds={[{ value: 10_000, label: "Storm 10k/s", tone: "warn" }]} series={[
            { label: "Sent", color: "--m-rx", values: s("pauseTx") },
            { label: "Received", color: "--m-tx", values: s("pauseRx") },
          ]} />
        </div>
        </ChartGrid>
      )}
    </div>
  );
}
