import { type KeyboardEvent } from "react";
import { HelpTip } from "../../components/ui";
import { LIGHT_WORD, LIGHTS, type Light, type XPort } from "./switchX";

// The Switch X's front, from Firewalla's product photo and data sheet: a 330 × 44 mm panel, its
// name plate over a square socket at the far left, the POWER and STATUS lights, ACTION and RESET,
// the RJ45 ports 1 to 8 in a row ("10G · PoE++") and the SFP+ cages 9 to 12 ("10G SFP/SFP+"). The
// socket, the lights and the buttons are drawn to orient, and SparkBox can't read them. Speed and
// what's connected are written under the chassis, so color is never the only cue.
const W = 965;
const CH = 128;
const RJ = { x0: 258, pitch: 60, w: 50, y: 58, h: 42 };
const SFP = { x0: 762, pitch: 44, w: 40, y: 68, h: 30 };
const LABEL = [CH + 18, CH + 34, CH + 50];
const CLIP = { rj45: 9, sfp: 12 };

const lightClass = (l: Light) => `sx-l${l.replace(".", "")}`;
const clip = (s: string, max: number) => (s.length > max ? `${s.slice(0, max - 1)}…` : s);

export function SwitchXPanel({ name, ports, selected, onSelect }: { name: string; ports: XPort[]; selected: number | null; onSelect: (n: number) => void }) {
  const port = (p: XPort) => {
    const rj = p.kind === "rj45";
    const i = rj ? p.n - 1 : p.n - 9;
    const x = rj ? RJ.x0 + i * RJ.pitch : SFP.x0 + i * SFP.pitch;
    const box = rj ? { w: RJ.w, h: RJ.h, y: RJ.y } : { w: SFP.w, h: SFP.h, y: SFP.y };
    const cx = x + box.w / 2;
    const lit = p.light !== "off";
    const on = selected === p.n;
    // The cages sit too close for a word each: every other one drops a line.
    const low = !rj && i % 2 === 1;
    const act = () => onSelect(p.n);
    return (
      <g
        key={p.n}
        className={`sx-port ${lightClass(p.light)}${on ? " on" : ""}`}
        data-port={p.n}
        role="button"
        tabIndex={0}
        aria-label={p.what}
        aria-pressed={on}
        onClick={act}
        onKeyDown={(e: KeyboardEvent) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            act();
          }
        }}
      >
        <text className="pp-tiny sx-num" x={cx} y={box.y - 6} textAnchor="middle">{p.n}</text>
        {lit && <rect className="sx-halo" x={x - 1} y={box.y - 1} width={box.w + 2} height={box.h + 2} rx={4} filter="url(#sx-glow)" />}
        {rj ? (
          <path className="sx-socket" d={`M${x},${box.y} h${box.w} v${box.h - 9} h-10 v9 h-${box.w - 20} v-9 h-10 Z`} />
        ) : (
          <rect className="sx-socket" x={x} y={box.y} width={box.w} height={box.h} rx={1.5} />
        )}
        <rect className="sx-face" x={x + 4} y={box.y + 4} width={box.w - 8} height={rj ? box.h - 15 : box.h - 8} rx={1.5} />
        {on && <rect className="pp-sel" x={x - 4} y={box.y - 17} width={box.w + 8} height={box.h + 19} rx={3} />}
        <text className="sx-speed" x={cx} y={LABEL[0]} textAnchor="middle">{p.speedText ?? (lit ? "?" : "")}</text>
        {low && p.short && <line className="pp-leader" x1={cx} y1={LABEL[1] - 9} x2={cx} y2={LABEL[2] - 9} />}
        <text className={`sx-what${p.label ? "" : " heard"}`} x={cx} y={LABEL[low ? 2 : 1]} textAnchor="middle">{p.short ? clip(p.short, CLIP[p.kind]) : ""}</text>
      </g>
    );
  };

  return (
    <div className="port-panel sx-panel">
      <svg viewBox={`0 0 ${W} ${LABEL[2] + 4}`} role="group" aria-label={`${name} front panel`} preserveAspectRatio="xMidYMid meet">
        <defs>
          <filter id="sx-glow" x="-40%" y="-40%" width="180%" height="180%">
            <feGaussianBlur stdDeviation="4.5" />
          </filter>
        </defs>
        <rect className="pp-chassis" x={0.5} y={0.5} width={W - 1} height={CH} rx={3} />
        <rect className="sx-bezel" x={9} y={9} width={W - 18} height={CH - 18} rx={3} />
        <text className="sx-brand" x={22} y={38}>Firewalla</text>
        <rect className="sx-tag" x={122} y={24} width={76} height={18} rx={2} />
        <text className="sx-tag-text" x={160} y={37} textAnchor="middle">SWITCH X</text>
        {/* Not one of the twelve, and Firewalla doesn't say what it's for. */}
        <path className="sx-blank" d="M22,58 h17 v-6 h15 v6 h17 v43 h-49 Z" />
        {["POWER", "STATUS"].map((k, i) => (
          <g key={k}>
            <circle className="sx-dot" cx={108} cy={62 + i * 15} r={3} />
            <text className="pp-tiny" x={116} y={66 + i * 15}>{k}</text>
          </g>
        ))}
        <circle className="sx-dot" cx={182} cy={82} r={6} />
        <text className="pp-tiny" x={182} y={103} textAnchor="middle">ACTION</text>
        <circle className="sx-dot" cx={228} cy={82} r={2.5} />
        <text className="pp-tiny" x={228} y={103} textAnchor="middle">RESET</text>
        <path className="sx-bracket" d={`M${RJ.x0 + 25},106 v3 H${RJ.x0 + 165} M${RJ.x0 + 255},109 H${RJ.x0 + 445} v-3`} />
        <text className="pp-tiny" x={RJ.x0 + 210} y={112} textAnchor="middle">10G · PoE++</text>
        <path className="sx-bracket" d={`M${SFP.x0 + 20},106 v3 H${SFP.x0 + 48} M${SFP.x0 + 104},109 H${SFP.x0 + 152} v-3`} />
        <text className="pp-tiny" x={SFP.x0 + 76} y={112} textAnchor="middle">10G SFP+</text>
        {ports.map(port)}
      </svg>
    </div>
  );
}

/** Below the panel: what each light means, with the speed written beside its color. */
export function SwitchXLegend() {
  return (
    <div className="sx-legend">
      <ul aria-label="What the lights mean">
        {LIGHTS.map((l) => (
          <li key={l}>
            <span className={`sx-swatch ${lightClass(l)}`} aria-hidden />
            {LIGHT_WORD[l]}
          </li>
        ))}
      </ul>
      <HelpTip topic="mon.switch.x-light" label="What the lights mean" />
    </div>
  );
}
