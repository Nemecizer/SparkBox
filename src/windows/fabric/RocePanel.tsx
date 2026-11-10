// A node's fabric at a glance, for its Network tab: each cabled half's RoCE verdict,
// counter rates, switch port and cabling, with a way into the Fabric window.
import { ArrowUpRight } from "lucide-react";
import { halfLabel, shortPort, useFabricHealth } from "../../lib/fabric";
import { HelpTip } from "../../components/ui";
import { fmtRatePair } from "../../lib/format";
import type { IfaceHealth } from "../../lib/fabric";
import { useCluster } from "../../lib/store";
import { useWM } from "../../wm/WindowManager";
import { lldpView } from "../node/NetworkTab";
import { ChipRow, ecnNote, quietChips, useHourStats, VerdictMark } from "./common";
import "./fabric.css";

export function RocePanel({ node }: { node: string }) {
  const state = useFabricHealth();
  const wm = useWM();
  const cluster = useCluster();
  const snap = cluster?.nodes.find((n) => n.id === node);
  // With no switch port mapped, what LLDP heard on the half, as Port 2's header says it.
  const heard = (iface: string) => {
    const x = snap?.nets.find((n) => n.iface === iface);
    return x && snap ? lldpView([x], snap, cluster?.nodes ?? [snap]) : null;
  };
  const mine = (state?.ifaces ?? []).filter((i) => i.node === node).sort((a, b) => a.half - b.half);
  const transport = (state?.transport ?? []).filter((t) => t.nodes.includes(node));
  if (!state) return <p className="muted roce-panel-empty">Waiting for the first fabric reading…</p>;
  if (!mine.length) return <p className="muted roce-panel-empty">No addressed fabric interface on this node.</p>;
  return (
    <section className="roce-panel" aria-label="RoCE">
      {mine.map((i, n) => {
        const loud = i.verdict.level === "warn" || i.verdict.level === "crit";
        return (
          <div key={i.iface} className={`roce-row level-${i.verdict.level}`}>
            <span className="roce-half">
              <span className="strong">{halfLabel(i.half)}{n === 0 && <> <HelpTip topic="fabric-and-rdma" /></>}</span>
              <span className="muted mono">{i.iface}</span>
            </span>
            <span className={`fab-verdict${loud ? ` tone-${i.verdict.level}` : ""}`} title={i.verdict.detail}>
              <VerdictMark level={i.verdict.level} title={i.verdict.title} kind={i.verdict.kind} />
              <span>{i.verdict.title}</span>
            </span>
            <span className="num muted roce-rate">
              {fmtRatePair(i.rxBps + (i.rdmaRxBps ?? 0), i.txBps + (i.rdmaTxBps ?? 0), { arrows: true })}
              {i.rdmaShare != null && ` · RDMA ${Math.round(i.rdmaShare * 100)}%`}
            </span>
            <RoceChips i={i} />
            <span className="muted roce-port">
              {i.switchPort ? `Switch ${shortPort(i.switchPort)}` : <span title={heard(i.iface)?.title}>{heard(i.iface)?.text ?? "Switch port unknown"}</span>}
              {i.cablingOk === false && <span className="warn-text"> · expected {shortPort(i.expectedPort)}</span>}
            </span>
            <button type="button" className="link-plain roce-open" onClick={() => wm.open("fabric", { cell: `${i.node}/${i.iface}` })} aria-label={`Open ${halfLabel(i.half).toLowerCase()} in Fabric`} title="Open in Fabric">
              <ArrowUpRight size={14} />
            </button>
          </div>
        );
      })}
      {transport.map((t) => (
        <p key={t.name} className={`roce-transport${t.level === "warn" ? " warn-text" : " muted"}`}>
          {t.name}: {t.message}
        </p>
      ))}
    </section>
  );
}

/** The half's counters that moved in the last hour; one quiet line when none did. */
function RoceChips({ i }: { i: IfaceHealth }) {
  const hour = useHourStats(i);
  return <ChipRow items={quietChips(i, hour)} quiet={ecnNote(i)} label={`${halfLabel(i.half)} counters that moved in the last hour`} />;
}
