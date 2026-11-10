import { X } from "lucide-react";
import { TimeChart } from "../../components/TimeChart";
import { HelpTip, InfoTip, Truncate } from "../../components/ui";
import { fmtBits, fmtDateTime } from "../../lib/format";
import { useHistory, type HistoryMetric } from "../../lib/historyApi";
import type { NetStats, NodeSnapshot, SwitchPort } from "../../lib/types";
import { capacityBps, errorTotal, fmtDbm, opticTone, portState, STATE_WORD, type PortSample } from "./common";

const bitsFmt = (v: number) => fmtBits(v);

/** The node's side of the cable: its fabric netdevs that point at this port. */
function nodeSide(node: NodeSnapshot | undefined, switchId: string, port: string): NetStats[] {
  return node?.nets.filter((n) => n.switchPort === `${switchId}/${port}`) ?? [];
}

/** Detail for one port: five minutes per direction, its counters and optics, and the node's view of the same link. */
export function PortDrawer({ switchId, port, node, samples, startedAt, rising, onClose }: {
  switchId: string;
  port: SwitchPort;
  node: NodeSnapshot | undefined;
  samples: PortSample[];
  startedAt: number;
  rising: boolean;
  onClose: () => void;
}) {
  const ts = samples.map((s) => s.ts);
  const now = ts.length ? ts[ts.length - 1] : Date.now() / 1000;
  const range: [number, number] = [now - 300, now];
  const cap = capacityBps(port);
  const nets = nodeSide(node, switchId, port.name);
  const nodeRx = nets.reduce((a, n) => a + n.rxBps + (n.rdmaRxBps ?? 0), 0);
  const nodeTx = nets.reduce((a, n) => a + n.txBps + (n.rdmaTxBps ?? 0), 0);
  const nodeUp = nets.length ? nets.every((n) => n.up) : null;
  const nodeErrors = nets.reduce((a, n) => a + (n.rxErrors ?? 0) + (n.txErrors ?? 0), 0);
  const carrier = nets.reduce((a, n) => Math.max(a, n.carrierChanges ?? 0), 0);
  const state = portState(port, rising);
  const lanes = port.laneRxPowerDbm ?? [];
  // The last hour from SparkBox's history (port:<switch>/<port>), when it has some.
  const hour = useHistory(`port:${switchId}/${port.name}`, ["rxBps", "txBps", "errors"] as unknown as HistoryMetric[], 3600);
  const hourTs = (hour?.ts ?? []).map((t) => t / 1000);
  const mapped = port.mappedBy === "mac" ? "its MAC in the bridge host table" : port.mappedBy === "lldp" ? "its LLDP announcement" : port.mappedBy === "comment" ? "the port comment" : null;

  const rows: [string, string, string][] = [
    ["Link", STATE_WORD[state], nodeUp == null ? "—" : nodeUp ? "Up" : "Down"],
    ["Speed", port.speed || "—", nets[0] ? (nets[0].speedMbps ? `${nets[0].speedMbps / 1000}G` : "—") : "—"],
    ["Data in (to the switch)", fmtBits(port.rxBps), nets.length ? `${fmtBits(nodeTx)} sent` : "—"],
    ["Data out (to the node)", fmtBits(port.txBps), nets.length ? `${fmtBits(nodeRx)} received` : "—"],
    ["MTU", port.mtu ? `${port.mtu} (L2)` : "—", nets[0]?.mtu ? `${nets[0].mtu}` : "—"],
    ["Errors", errorTotal(port).toLocaleString(), nets.length ? nodeErrors.toLocaleString() : "—"],
    ["Flaps", `${(port.linkDowns ?? 0).toLocaleString()} link-downs`, nets.length ? `${carrier} carrier changes` : "—"],
  ];

  return (
    <aside className="port-drawer" aria-label={`${port.name} details`}>
      <header className="port-drawer-head">
        <div className="port-drawer-titles">
          <div className="port-drawer-title">{port.name}</div>
          <Truncate className="muted" text={`${node ? node.name : port.comment || "Not cabled to a known node"} · ${STATE_WORD[state]}`} />
        </div>
        <button type="button" className="tool" onClick={onClose} aria-label="Close details" title="Close details">
          <X size={14} />
        </button>
      </header>
      <div className="port-drawer-body">
        <TimeChart
          title="↓ Receive"
          subtitle={`of ${cap / 1e9} Gb/s`}
          ts={ts}
          range={range}
          relative
          startsAt={startedAt}
          height={130}
          fmt={bitsFmt}
          series={[{ label: "Receive", color: "--m-rx", values: samples.map((s) => s.rx) }]}
        />
        <TimeChart
          title="↑ Transmit"
          subtitle={`of ${cap / 1e9} Gb/s`}
          ts={ts}
          range={range}
          relative
          startsAt={startedAt}
          height={130}
          fmt={bitsFmt}
          series={[{ label: "Transmit", color: "--m-tx", values: samples.map((s) => s.tx) }]}
        />

        {(port.management || port.cablingOk === false || mapped) && (
          <dl className="kv port-counters port-identity">
            {mapped && node && <div><dt>Found by</dt><dd><Truncate text={mapped} /></dd></div>}
            {port.cablingOk === false && <div><dt>Cabling</dt><dd className="warn-text"><Truncate text={`Expected ${port.expectedNode ?? "another node"} here`} /></dd></div>}
            {port.management && (
              <div>
                <dt>SparkBox's path</dt>
                <dd>
                  <span className="port-mgmt">
                    <Truncate text="Disable and Bounce need typing" />
                    <InfoTip topic="mon.switch.guarded" label="Why this port is guarded"><p>{port.management} Disabling or bouncing it can cut SparkBox off from the switch. So you must type the exact phrase, and the switch always undoes it by itself.</p></InfoTip>
                  </span>
                </dd>
              </div>
            )}
          </dl>
        )}
        {hour && hourTs.length > 1 && (
          <TimeChart
            title="Last hour"
            subtitle="receive and transmit"
            ts={hourTs}
            range={[Date.now() / 1000 - 3600, Date.now() / 1000]}
            relative
            robust="auto"
            height={120}
            fmt={bitsFmt}
            startsAt={hour.startsAtMs != null ? hour.startsAtMs / 1000 : null}
            series={[
              { label: "Receive", color: "--m-rx", values: hour.series.rxBps ?? [] },
              { label: "Transmit", color: "--m-tx", values: hour.series.txBps ?? [] },
            ]}
          />
        )}
        <h4 className="section-title">Switch says / node says <HelpTip topic="mon.switch.both-sides" /></h4>
        {nets.length > 0 && <Truncate className="muted port-compare-note" text={`${node?.name} sees this cable as ${nets.map((n) => n.iface).join(" and ")}.`} />}
        <table className="grid compact port-compare">
          <thead>
            <tr>
              <th />
              <th>Switch</th>
              <th title={nets.map((n) => n.iface).join(" + ") || undefined}>{node ? node.name : "Node"}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(([label, sw, nd]) => (
              <tr key={label}>
                <th scope="row">{label}</th>
                <td className="num">{sw}</td>
                <td className="num">{nd}</td>
              </tr>
            ))}
          </tbody>
        </table>

        <h4 className="section-title">Counters</h4>
        <dl className="kv port-counters">
          <div><dt>RX errors</dt><dd className="num">{port.rxErrors.toLocaleString()}</dd></div>
          <div><dt>TX errors</dt><dd className="num">{port.txErrors.toLocaleString()}</dd></div>
          <div><dt>FCS errors</dt><dd className="num">{(port.fcsErrors ?? 0).toLocaleString()}</dd></div>
          <div><dt>RX drops</dt><dd className="num">{(port.rxDrops ?? 0).toLocaleString()}</dd></div>
          <div><dt>TX drops</dt><dd className="num">{(port.txDrops ?? 0).toLocaleString()}</dd></div>
          <div><dt>Pause frames in</dt><dd className="num">{(port.rxPause ?? 0).toLocaleString()}</dd></div>
          <div><dt>Link-downs</dt><dd className="num">{(port.linkDowns ?? 0).toLocaleString()}</dd></div>
          <div><dt>Last link up</dt><dd>{port.lastLinkUpMs ? fmtDateTime(port.lastLinkUpMs) : "—"}</dd></div>
        </dl>

        <h4 className="section-title">Optics</h4>
        {lanes.length === 0 ? (
          <p className="muted">{port.running ? "No optical readings for this port." : "No link, so no optical readings."}</p>
        ) : (
          <>
            <div className="lane-bars" role="list" aria-label="Receive power per lane">
              {lanes.map((v, i) => {
                const tone = opticTone(v);
                // −14 dBm (nothing) to +2 dBm (strong) across the bar.
                const w = Math.max(2, Math.min(100, ((v + 14) / 16) * 100));
                return (
                  <div key={i} className="lane-bar" role="listitem" aria-label={`Lane ${i + 1}: ${v.toFixed(1)} dBm${tone !== "good" ? `, ${tone === "crit" ? "failing" : "marginal"}` : ""}`}>
                    <span className="lane-name">Lane {i + 1}</span>
                    <span className="lane-track">
                      <span className={`lane-fill tone-${tone}`} style={{ width: `${w}%` }} />
                      <span className="lane-mark warn" style={{ left: `${((-7 + 14) / 16) * 100}%` }} />
                      <span className="lane-mark crit" style={{ left: `${((-10 + 14) / 16) * 100}%` }} />
                    </span>
                    <span className={`lane-value num tone-${tone}`}>{v.toFixed(1)} dBm</span>
                  </div>
                );
              })}
            </div>
            <dl className="kv port-counters">
              <div><dt>Transmit power</dt><dd className="num">{fmtDbm(port.txPowerDbm)}</dd></div>
              <div><dt>Module</dt><dd className="num">{port.moduleTempC != null ? `${port.moduleTempC.toFixed(0)} °C` : "—"}</dd></div>
              <div><dt>Vendor</dt><dd><Truncate text={port.sfpVendor ?? "—"} /></dd></div>
              <div><dt>Part</dt><dd className="mono"><Truncate text={port.sfpPart ?? "—"} /></dd></div>
            </dl>
            <span className="optics-legend">
              <span className="muted">Receive power per lane</span>
              <HelpTip topic="mon.switch.rx-power" label="About receive power" />
            </span>
          </>
        )}
      </div>
    </aside>
  );
}
