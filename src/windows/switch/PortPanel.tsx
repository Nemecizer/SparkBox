import { useId, useRef, useState, type FocusEvent, type KeyboardEvent, type MouseEvent } from "react";
import { isCrs804, type HardwareProfile } from "../../lib/hardware";
import { fmtBits } from "../../lib/format";
import type { SwitchPort, SwitchSnapshot } from "../../lib/types";
import { errorTotal, fmtDbm, loadPct, opticTone, portState, STATE_WORD, type PortState } from "./common";

// Physical sockets, not RouterOS logical breakout interfaces. Live port names and
// LLDP/MAC mappings determine which node is shown; nothing assumes a cable order.
const W = 965;
const CH = 87;
const EAR = 32;
const CAGE_X0 = 512;
const CAGE_PITCH = 88;
const CAGE_W = 46;
const CAGE_H = 22;
const CAGE_Y = 36;
const SOCKET_W = CAGE_W;
const RJ45 = { x: 262, y: 30, w: 32, h: 28 };
const LABEL_Y = [104, 120];

interface Hover {
  port: SwitchPort;
  x: number;
  y: number;
}

/** The light a port shows (§2.1): quiet when fine, amber or red only when something's wrong. */
function led(p: SwitchPort | undefined, rising: boolean): "ok" | "warn" | "crit" | "off" | "disabled" | "absent" {
  if (!p) return "absent";
  if (p.disabled) return "disabled";
  if (!p.running) return "off";
  if (rising || p.cablingOk === false) return rising ? "crit" : "warn";
  const optic = opticTone(p.rxPowerDbm);
  return optic === "crit" ? "crit" : optic === "warn" ? "warn" : "ok";
}

/**
 * The switch's front panel. Each port shows its state by fill and light (an open dark
 * slot for no link, hatching for disabled, a red edge and mark for rising errors),
 * a thin bar for load (from 1%), and the node cabled to it underneath. Click or Enter selects its row.
 */
export function PortPanel({ sw, selected, onSelect, rising, nodeName, profile }: {
  sw: SwitchSnapshot;
  profile?: HardwareProfile | null;
  selected: string | null;
  onSelect: (port: string) => void;
  rising: (port: string) => boolean;
  nodeName: (id: string | null | undefined) => string | null;
}) {
  const hatch = useId().replace(/:/g, "");
  const wrap = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<Hover | null>(null);

  const sensors = sw.sensors ?? [];
  const psu = (n: number) => sensors.find((s) => s.name === `PSU${n}`);
  const fansOk = sensors.filter((s) => s.unit === "RPM").every((s) => s.ok);
  const online = sw.status === "online";

  const show = (p: SwitchPort, e: MouseEvent | { clientX: number; clientY: number }) => {
    const r = wrap.current?.getBoundingClientRect();
    if (r) setHover({ port: p, x: e.clientX - r.left, y: e.clientY - r.top });
  };

  const interactive = (p: SwitchPort | undefined, name: string) => {
    const state: PortState | "absent" = p ? portState(p, rising(p.name)) : "absent";
    const pct = p ? loadPct(p) : 0;
    const optic = p?.running ? opticTone(p.rxPowerDbm) : null;
    const weak = optic === "warn" || optic === "crit";
    const label = p
      ? `${name}${nodeName(p.node) ? `, ${nodeName(p.node)}` : p.comment ? `, ${p.comment}` : ""}: ${STATE_WORD[state as PortState]}${p.running ? `, ${pct.toFixed(0)}% of ${p.capacityMbps ? p.capacityMbps / 1000 : 200}G` : ""}${weak ? `, ${optic === "crit" ? "failing" : "low"} receive power (${fmtDbm(p?.rxPowerDbm)})` : ""}${p.management ? ", carries SparkBox's connection" : ""}`
      : `${name}: not present`;
    const act = () => p && onSelect(name);
    return {
      state,
      pct,
      props: {
        role: "button",
        tabIndex: p ? 0 : -1,
        "aria-label": label,
        "aria-pressed": selected === name,
        onClick: act,
        onKeyDown: (e: KeyboardEvent) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            act();
          }
        },
        onMouseMove: (e: MouseEvent) => p && show(p, e),
        onMouseLeave: () => setHover(null),
        onFocus: (e: FocusEvent<SVGGElement>) => {
          const r = e.currentTarget.getBoundingClientRect();
          if (p) show(p, { clientX: r.left + r.width / 2, clientY: r.bottom });
        },
        onBlur: () => setHover(null),
      },
    };
  };

  /** One entire physical socket, with logical interfaces retained in the Ports table. */
  const socket = (cage: number, x: number) => {
    const members = sw.ports.filter((p) => new RegExp(`^qsfp56-dd-${cage}(?:-\\d+)?$`).test(p.name));
    const p = members.find((p) => p.running) ?? members.find((p) => p.name === selected) ?? members[0];
    const name = p?.name ?? `qsfp56-dd-${cage}`;
    const row = 0;
    const { state, pct, props } = interactive(p, name);
    const on = selected === name;
    const light = led(p, p ? rising(p.name) : false);
    const node = [...new Set(members.filter((m) => m.running || m.node).map((m) => nodeName(m.node) ?? m.comment).filter(Boolean))].join(", ");
    const cx = x + SOCKET_W / 2;
    return (
      <g key={name} data-physical-socket={cage} className={`pp-port pp-${state}${on ? " on" : ""}`} {...props}>
        <rect className="pp-half" x={x + 1} y={CAGE_Y + 2} width={SOCKET_W - 2} height={CAGE_H - 4} rx={1.5} fill={state === "disabled" ? `url(#${hatch})` : undefined} />
        {state === "up" || state === "errors" ? <rect className="pp-module" x={x + 3} y={CAGE_Y + 5} width={SOCKET_W - 6} height={CAGE_H - 12} rx={1} /> : null}
        {p?.running && pct >= 1 && <rect className="pp-load" x={x + 3} y={CAGE_Y + CAGE_H - 6} width={Math.max(1, ((SOCKET_W - 6) * Math.min(100, pct)) / 100)} height={2} rx={1} />}
        <circle className={`pp-led led-${light}`} cx={cx} cy={CAGE_Y - 9} r={3.2} />
        {on && <rect className="pp-sel" x={x - 1} y={CAGE_Y - 15} width={SOCKET_W + 2} height={CAGE_H + 17} rx={3} />}
        {state === "errors" && <text className="pp-mark" x={cx} y={CAGE_Y + CAGE_H / 2 + 3.5} textAnchor="middle" aria-hidden>!</text>}
        <line className="pp-leader" x1={cx} y1={CAGE_Y + CAGE_H + 4} x2={cx} y2={LABEL_Y[row] - 9} />
        <text className="pp-label" x={cx} y={LABEL_Y[row]} textAnchor="middle">
          <tspan className="pp-label-port">{cage}</tspan>
          <tspan className={`pp-label-node${node ? "" : " none"}`} dx={4}>{node || "No mapped node"}</tspan>
        </text>
        <text className="pp-label" x={cx} y={120} textAnchor="middle">{members.filter((m) => m.running).map((m) => m.speed || "speed unknown").join(" + ") || "No active link"}</text>
        {members.length > 1 && <title>{`Socket ${cage}: ${members.map((m) => m.name).join(", ")}. Logical interfaces share this physical socket.`}</title>}
      </g>
    );
  };


  if (!isCrs804(sw.board)) return <div className="port-panel">
    <p>{profile ? `${profile.manufacturer} ${profile.model}` : "Physical layout not identified"}</p>
    {profile ? <>{profile.groups.map((g, i) => <div key={i}><p>{g.count} × {g.connector} · up to {g.maxGbps} Gb/s each</p><svg viewBox={`0 0 640 ${Math.ceil(g.count / 16) * 28}`} role="img" aria-label={`${g.count} physical ${g.connector} sockets; cable mapping unconfirmed`}>{Array.from({ length: g.count }, (_, n) => <g key={n}><rect className="pp-opening" x={(n % 16) * 40 + 2} y={Math.floor(n / 16) * 28 + 2} width={32} height={20} rx={2}/><text className="pp-tiny" x={(n % 16) * 40 + 18} y={Math.floor(n / 16) * 28 + 16} textAnchor="middle">{n + 1}</text></g>)}</svg></div>)}<p className="muted">{profile.summary}</p></> : <p className="muted">The table shows reported interfaces. Their number does not establish the number of physical sockets.</p>}
  </div>;

  return (
    <div className="port-panel" ref={wrap}>
      <svg viewBox={`0 0 ${W} 128`} role="group" aria-label={`${sw.name} front panel`} preserveAspectRatio="xMidYMid meet">
        <defs>
          <pattern id={hatch} width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <rect width="5" height="5" className="pp-hatch-bg" />
            <line x1="0" y1="0" x2="0" y2="5" className="pp-hatch" strokeWidth="1.8" />
          </pattern>
        </defs>
        {/* Chassis and rack ears with their mounting slots. */}
        <rect className="pp-chassis" x={0.5} y={0.5} width={W - 1} height={CH} rx={2} />
        {[0, W - EAR].map((x) => (
          <g key={x}>
            <rect className="pp-ear" x={x + 0.5} y={0.5} width={EAR - 1} height={CH} rx={2} />
            <rect className="pp-slot" x={x + 9} y={17} width={14} height={7} rx={3.5} />
            <rect className="pp-slot" x={x + 9} y={63} width={14} height={7} rx={3.5} />
          </g>
        ))}
        {/* Model plate. */}
        <rect className="pp-plate" x={46} y={14} width={150} height={38} rx={2} />
        <text className="pp-brand" x={56} y={31}>MikroTik</text>
        <text className="pp-model" x={56} y={45}>CRS804 DDQ</text>
        {/* Status lights: power, the two supplies, the fans. */}
        {[
          { k: "PWR", s: online ? "ok" : "off" },
          { k: "PSU1", s: psu(1) ? (psu(1)!.ok ? "ok" : "crit") : "off" },
          { k: "PSU2", s: psu(2) ? (psu(2)!.ok ? "ok" : "crit") : "off" },
          { k: "FAN", s: online ? (fansOk ? "ok" : "crit") : "off" },
        ].map((l, i) => (
          <g key={l.k}>
            <circle className={`pp-led led-${l.s}`} cx={56 + i * 36} cy={64} r={3} />
            <text className="pp-tiny" x={56 + i * 36} y={78} textAnchor="middle">{l.k}</text>
          </g>
        ))}
        {/* Two physical management sockets, including an unreported second socket. */}
        {[1, 2].map((number) => {
          const ether = sw.ports.find((p) => p.name === `ether${number}`);
          const etherBits = interactive(ether, `ether${number}`);
          return <g key={number} transform={`translate(${(number - 1) * 100},0)`}>
          <g className={`pp-port pp-${etherBits.state}${selected === ether?.name ? " on" : ""}`} {...etherBits.props}>
            <text className="pp-tiny" x={RJ45.x + RJ45.w / 2} y={RJ45.y - 8} textAnchor="middle">MGMT {number}</text>
            <path
              className="pp-rj45"
              d={`M${RJ45.x},${RJ45.y} h${RJ45.w} v${RJ45.h - 8} h-8 v8 h-${RJ45.w - 16} v-8 h-8 Z`}
              fill={etherBits.state === "disabled" ? `url(#${hatch})` : undefined}
            />
            <rect className={`pp-led-sq led-${led(ether, ether ? rising(ether.name) : false)}`} x={RJ45.x + 3} y={RJ45.y + 3} width={6} height={4} />
            <rect className={`pp-led-sq led-${ether?.running && ether.rxBps + ether.txBps > 0 ? "ok" : "off"}`} x={RJ45.x + RJ45.w - 9} y={RJ45.y + 3} width={6} height={4} />
            {selected === ether?.name && <rect className="pp-sel" x={RJ45.x - 4} y={RJ45.y - 16} width={RJ45.w + 8} height={RJ45.h + 20} rx={3} />}
            <line className="pp-leader" x1={RJ45.x + RJ45.w / 2} y1={RJ45.y + RJ45.h + 4} x2={RJ45.x + RJ45.w / 2} y2={LABEL_Y[0] - 9} />
            <text className="pp-label" x={RJ45.x + RJ45.w / 2} y={LABEL_Y[0]} textAnchor="middle">
              <tspan className="pp-label-port">{`ether${number}`}</tspan>
              <tspan className="pp-label-node" x={RJ45.x + RJ45.w / 2} dy={16}>{ether?.management ? "SparkBox's path" : ether ? "LAN" : "Not reported"}</tspan>
            </text>
          </g>
          </g>;
        })}
        {/* Four physical QSFP56-DD sockets. Breakout subinterfaces never add sockets. */}
        {[1, 2, 3, 4].map((cage) => {
          const x = CAGE_X0 + (cage - 1) * CAGE_PITCH;
          return <g key={cage}>
            <text className="pp-tiny" x={x + CAGE_W / 2} y={CAGE_Y - 18} textAnchor="middle">{cage}</text>
            <rect className="pp-cage" x={x - 4} y={CAGE_Y - 3} width={CAGE_W + 8} height={CAGE_H + 6} rx={2.5} />
            <rect className="pp-opening" x={x} y={CAGE_Y} width={CAGE_W} height={CAGE_H} rx={1.5} />
            {socket(cage, x)}
          </g>;
        })}
        <text className="pp-tiny pp-caption" x={CAGE_X0 - 16} y={CAGE_Y + 8} textAnchor="end">QSFP56-DD</text>
        <text className="pp-tiny pp-caption" x={CAGE_X0 - 16} y={CAGE_Y + 20} textAnchor="end">4 physical sockets</text>
      </svg>
      <p className="muted pp-note">{[1, 2, 3, 4].filter((c) => sw.ports.some((p) => p.running && new RegExp(`^qsfp56-dd-${c}(?:-\\d+)?$`).test(p.name))).length} / 4 sockets active · up to 400 Gb/s per socket. Speeds above are observed links; breakout interfaces stay in the table.</p>
      {hover && <PortTip p={hover.port} x={hover.x} y={hover.y} node={nodeName(hover.port.node)} rising={rising(hover.port.name)} wrapWidth={wrap.current?.clientWidth ?? 800} />}
    </div>
  );
}

function PortTip({ p, x, y, node, rising, wrapWidth }: { p: SwitchPort; x: number; y: number; node: string | null; rising: boolean; wrapWidth: number }) {
  const state = portState(p, rising);
  const worst = p.laneRxPowerDbm?.length ? Math.min(...p.laneRxPowerDbm) : p.rxPowerDbm;
  const left = Math.min(Math.max(8, x + 12), wrapWidth - 236);
  return (
    <div className="pp-tip" role="tooltip" style={{ left, top: y + 14 }}>
      <div className="pp-tip-head">
        <span className="strong">{p.name}</span>
        <span className={`pp-tip-state ${state}`}>{STATE_WORD[state]}</span>
      </div>
      <dl>
        <dt>Node</dt>
        <dd>{node ? `${node}${p.mappedBy ? ` (by ${p.mappedBy === "mac" ? "MAC" : p.mappedBy === "lldp" ? "LLDP" : "comment"})` : ""}` : p.comment || "—"}</dd>
        {p.cablingOk === false && (
          <>
            <dt>Cabling</dt>
            <dd className="warn-text">Expected {p.expectedNode ?? "another node"}</dd>
          </>
        )}
        <dt>↓ Receive</dt>
        <dd className="num">{p.running ? fmtBits(p.rxBps) : "—"}</dd>
        <dt>↑ Transmit</dt>
        <dd className="num">{p.running ? fmtBits(p.txBps) : "—"}</dd>
        <dt>Load</dt>
        <dd className="num">{p.running ? `${loadPct(p).toFixed(1)}% of ${(p.capacityMbps || 200_000) / 1000}G` : "—"}</dd>
        <dt>Errors</dt>
        <dd className="num">{errorTotal(p).toLocaleString()}{rising ? " (rising)" : ""}</dd>
        <dt>Worst RX</dt>
        <dd className="num">{fmtDbm(worst)}</dd>
      </dl>
      {p.management && <p className="pp-tip-note">Carries SparkBox's connection to the switch.</p>}
    </div>
  );
}
