import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from "react";
import { Cloud, Laptop, Maximize2, Network, Router, Server } from "lucide-react";
import { EntityLink } from "../components/EntityLink";
import { glyphOfStatus, HelpTip, Segmented, Sparkline, Spacer, StateGlyph, Toolbar, ToolButton, Waiting, type GlyphState } from "../components/ui";
import { useEndpoints, type Endpoint } from "../lib/endpoints";
import { showContextMenu, nodeMenu } from "../lib/contextMenu";
import { fmtBits, fmtBytesPair, fmtRatePair } from "../lib/format";
import { offlineReason } from "../lib/nodeStatus";
import { shortPort, useFabricHealth, type FabricState } from "../lib/fabric";
import { displayStatus, DISPLAY_LABEL, getHealth, switchDisplayStatus, useHealth, type DisplayStatus, type HealthState } from "../lib/health";
import { dirUtil, engines, fabricCapBps, fabricRx, fabricTx, gpuError, lanRx, lanTx, memPct, nodeAlerts, nodeNotes, nodeTone, FABRIC_LINK_BPS, type NodeNote } from "../lib/metrics";
import { recent, useCluster } from "../lib/store";
import { useAppearance } from "../lib/theme";
import type { NetStats, NodeSnapshot, SwitchPort, SwitchSnapshot, VpnStatus } from "../lib/types";
import { useVpnStatus, VPN_LABEL } from "../lib/vpn";
import { useWM, useWinState } from "../wm/WindowManager";
import "./monitoring.css";

// ---- Layout (pure, tested in src/lib/metrics.test.ts)

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type TopoMode = "wide" | "medium" | "compact";

export interface TopoLayout {
  mode: TopoMode;
  width: number;
  height: number;
  internet: Rect;
  gateway: Rect;
  lanSwitch: Rect;
  fabricSwitch: Rect;
  mac: Rect;
  nodes: Rect[];
}

const MARGIN = 16;
const GAP = 16;
const NODE_H = 104;
const BOX_H = 48;
const SMALL_W = 150;
/** The fabric switch is taller: a row of port names above its name. */
const FABRIC_H = 64;

/**
 * Where everything goes for a given width. Wide (900 px and up) and medium put the nodes in a
 * row between the LAN switch above and the fabric switch below; compact (under 640 px) stacks
 * the nodes in a column with the fabric switch running down the right-hand side. With `fitHeight`,
 * the vertical spacing shrinks (never the boxes or the text) to fit that height where it can.
 */
export function topologyLayout(nodeCount: number, width: number, mac: "lan" | "vpn", fitHeight?: number): TopoLayout {
  const w = Math.max(320, width);
  const mode: TopoMode = w >= 900 ? "wide" : w >= 640 ? "medium" : "compact";
  const n = Math.max(1, nodeCount);
  if (mode === "compact") {
    const colW = Math.min(240, w - 2 * MARGIN - 110);
    const top = MARGIN;
    const internet = { x: MARGIN, y: top, w: 120, h: 36 };
    const macBox = mac === "vpn" ? { x: internet.x + internet.w + GAP, y: top, w: Math.min(SMALL_W, w - internet.w - 2 * MARGIN - GAP), h: 36 } : { x: 0, y: 0, w: 0, h: 0 };
    const gateway = { x: MARGIN, y: top + 36 + 28, w: Math.min(220, colW), h: BOX_H };
    const lanSwitch = { x: MARGIN, y: gateway.y + BOX_H + 28, w: Math.min(220, colW), h: BOX_H };
    if (mac === "lan") Object.assign(macBox, { x: lanSwitch.x + lanSwitch.w + GAP, y: lanSwitch.y + 6, w: Math.min(SMALL_W, w - lanSwitch.w - 2 * MARGIN - GAP), h: 36 });
    const firstNode = lanSwitch.y + BOX_H + 40;
    const nodes = Array.from({ length: n }, (_, i) => ({ x: MARGIN + 18, y: firstNode + i * (NODE_H + GAP), w: colW - 18, h: NODE_H }));
    const last = nodes[n - 1];
    const fabricSwitch = { x: w - MARGIN - 64, y: firstNode - 8, w: 64, h: last.y + NODE_H + 8 - (firstNode - 8) };
    return { mode, width: w, height: fabricSwitch.y + fabricSwitch.h + MARGIN + 20, internet, gateway, lanSwitch, fabricSwitch, mac: macBox, nodes };
  }
  const inner = w - 2 * MARGIN;
  const perRow = mode === "wide" ? n : Math.min(n, Math.max(1, Math.floor((inner + GAP) / (170 + GAP))));
  const rows = Math.ceil(n / perRow);
  // Vertical gaps: internet→gateway, gateway→LAN switch, LAN switch→nodes, nodes→fabric switch.
  const roomy = [30, 36, 96, 110];
  const tight = [12, 14, 40, 50];
  const fixed = 2 * MARGIN + 36 + 2 * BOX_H + rows * NODE_H + (rows - 1) * 28 + FABRIC_H + 6;
  const spare = fitHeight ? fitHeight - fixed : Infinity;
  const k = Math.max(0, Math.min(1, (spare - tight.reduce((a, b) => a + b)) / (roomy.reduce((a, b) => a + b) - tight.reduce((a, b) => a + b))));
  const gap = roomy.map((r, i) => Math.round(tight[i] + (r - tight[i]) * k));
  const nodeW = Math.min(230, (inner - GAP * (perRow - 1)) / perRow);
  const cx = w / 2;
  const internet = { x: cx - 70, y: MARGIN, w: 140, h: 36 };
  const gateway = { x: cx - 120, y: internet.y + 36 + gap[0], w: 240, h: BOX_H };
  const lanSwitch = { x: cx - 120, y: gateway.y + BOX_H + gap[1], w: 240, h: BOX_H };
  const macBox =
    mac === "vpn"
      ? { x: Math.max(MARGIN, internet.x - 60 - SMALL_W), y: internet.y, w: SMALL_W, h: 36 }
      : { x: Math.min(w - MARGIN - SMALL_W, lanSwitch.x + lanSwitch.w + 70), y: lanSwitch.y + 6, w: SMALL_W, h: 36 };
  const nodesTop = lanSwitch.y + BOX_H + gap[2];
  const nodes = Array.from({ length: n }, (_, i) => {
    const row = Math.floor(i / perRow);
    const inRow = row < rows - 1 ? perRow : n - perRow * (rows - 1);
    const rowW = inRow * nodeW + (inRow - 1) * GAP;
    const col = i - row * perRow;
    return { x: cx - rowW / 2 + col * (nodeW + GAP), y: nodesTop + row * (NODE_H + 28), w: nodeW, h: NODE_H };
  });
  const lastRowBottom = nodes[n - 1].y + NODE_H;
  const fabricSwitch = { x: cx - Math.min(inner, 560) / 2, y: lastRowBottom + gap[3], w: Math.min(inner, 560), h: FABRIC_H };
  return { mode, width: w, height: fabricSwitch.y + fabricSwitch.h + MARGIN + 6, internet, gateway, lanSwitch, fabricSwitch, mac: macBox, nodes };
}

/** True when two rectangles overlap (touching edges don't count). */
export function overlaps(a: Rect, b: Rect): boolean {
  return a.w > 0 && b.w > 0 && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

// ---- Level of detail (§D8): a diagram at full size, or a list; never shrunken text

/** The smallest text the diagram draws, in its own units (1 unit = 1 px × the text scale). */
export const TOPO_TEXT_MIN = 11;
/** Below this window size the diagram can't hold its labels: the list shows instead. */
export const TOPO_MIN_W = 640;
export const TOPO_MIN_H = 420;

export interface TopoScale {
  list: boolean;
  /** Screen px per diagram unit: the text scale, never less (the diagram never shrinks). */
  scale: number;
  /** The smallest text on screen, in px. */
  textPx: number;
  layout: TopoLayout;
}

/**
 * Picks the diagram or the list for a window of `width` × `height` px. The diagram is drawn at
 * the user's text scale and never scaled down to fit: when it wouldn't fit at that size (even
 * with the spacing tightened), or the window is under 640 × 420, the list topology shows.
 * So the smallest diagram text is always 11 px × the text scale.
 */
export function topologyScale(width: number, height: number, nodeCount: number, mac: "lan" | "vpn", textScale = 1, fitHeight = true): TopoScale {
  const s = Math.max(1, textScale);
  const layout = topologyLayout(nodeCount, width / s, mac, fitHeight ? height / s : undefined);
  const tooSmall = width < TOPO_MIN_W || height < TOPO_MIN_H || layout.mode === "compact";
  const overflows = fitHeight && layout.height * s > height + 1;
  const list = tooSmall || overflows || layout.width * s > width + 1;
  return { list, scale: s, textPx: TOPO_TEXT_MIN * s, layout };
}

/** Why a node isn't reporting, short enough for a box (lib/nodeStatus, one wording everywhere). */
export { offlineReason };

/** "2 min", "12 s", "3 h": how long since a node last answered. */
export function sinceShort(ms: number | null | undefined, now = Date.now()): string | null {
  if (!ms) return null;
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86400)} d`;
}

/**
 * When a node went quiet: its last reading, or else when the health engine first saw it
 * offline (the Demo drops the last-seen time with the node).
 */
export function quietSinceMs(n: NodeSnapshot, health: HealthState | null = getHealth()): number | null {
  if (n.lastSeenMs) return n.lastSeenMs;
  const a = health?.alerts.find((x) => (x.rule === "node-offline" || x.rule === "node-stale") && x.entity.kind === "node" && x.entity.id === n.id);
  return a?.sinceMs ?? null;
}

/** "Offline · 2 min", "Restarting · 40 s", "Not updating · 9 s". */
export function stateWord(n: NodeSnapshot, now = Date.now(), health: HealthState | null = getHealth()): string {
  const word = n.powerState === "rebooting" ? "Restarting" : n.powerState === "off" ? "Shut down" : n.powerState === "waking" ? "Waking"
    : n.status === "connecting" ? "Connecting" : n.status === "error" ? "Error" : n.stale ? "Not updating" : "Offline";
  const ago = sinceShort(quietSinceMs(n, health), now);
  return ago ? `${word} · ${ago}` : word;
}

// ---- Load

/** What This Mac's box says: the VPN when the Mac goes through it, its problem when it has one, else the LAN. */
export function macWords(vpn: VpnStatus | null): string {
  if (vpn?.state === "connected") return `VPN${vpn.latencyMs != null ? ` · ${vpn.latencyMs.toFixed(0)} ms` : ""}`;
  return vpn?.state === "error" ? VPN_LABEL.error : "On this network";
}

/** Utilisation stops (share of the negotiated speed), for the "Color nodes by" tint. */
export const RAMP = [0, 0.25, 0.5, 0.75, 0.9] as const;
export const rampStep = (u: number) => RAMP.reduce<number>((step, stop, i) => (u >= stop ? i : step), 0);
const rampVar = (u: number) => `var(--topo-u${rampStep(u)})`;
/** Link thickness carries the load: 2 px idle to 8 px at full speed. Colour is kept for health. */
export const loadWidth = (u: number) => 2 + 6 * Math.sqrt(Math.max(0, Math.min(1, u)));
/** Dash-flow period: faster the more traffic, slow and calm when light. */
const flowSeconds = (bps: number) => Math.max(0.35, Math.min(3, 3.2 - Math.log10(Math.max(1, bps / 1e6)) * 0.6));

// ---- Pieces

interface FabricLink {
  node: NodeSnapshot;
  rx: number;
  tx: number;
  cap: number;
  /** null: nobody can say (the node isn't reporting and its switch port isn't known). */
  up: boolean | null;
  nics: string[];
  port: SwitchPort | null;
  errors: number;
  /** "Expected 1-2 · found 2-1" when the cabling baseline disagrees. */
  cabling: string | null;
  /** The link's RoCE health: lossless is quiet, congested amber, lossy / PHY errors / down red. */
  health: "ok" | "warn" | "crit";
  /** The fabric monitor's words for it ("RoCE is retransmitting"). */
  verdict: string | null;
}

const short = (port: string) => port.replace(/^qsfp56-dd-/, "");

/** The switch port each node was last seen on, so an offline node's cable can still be read from the switch. */
const knownPort = new Map<string, string>();

function fabricLink(n: NodeSnapshot, crs: SwitchSnapshot | undefined, fabric: FabricState | null): FabricLink {
  const nets = n.nets.filter((x) => x.role === "fabric");
  const up = nets.filter((x) => x.up);
  const port = crs?.ports.find((p) => p.node === n.id)
    ?? crs?.ports.find((p) => nets.some((x) => x.switchPort?.endsWith(`/${p.name}`)))
    ?? crs?.ports.find((p) => p.name === knownPort.get(n.id))
    // The port's own comment names the node (a cable labelled on the switch).
    ?? crs?.ports.find((p) => !!p.comment && [n.id, n.name].some((x) => p.comment.trim().toLowerCase() === x.toLowerCase()))
    ?? null;
  if (port && n.status === "online") knownPort.set(n.id, port.name);
  const errors = nets.reduce((a, x) => a + (x.rxErrors ?? 0) + (x.txErrors ?? 0), 0) + (port ? port.rxErrors + port.txErrors + (port.fcsErrors ?? 0) : 0);
  // A node that isn't reporting can't say whether its cable is up; the switch can.
  const reporting = n.status === "online" && !n.stale;
  const viaSwitch = !reporting && !!port;
  const expected = crs?.ports.find((p) => p.expectedNode === n.id) ?? null;
  const miscabled = !!port && ((port.expectedNode != null && port.expectedNode !== n.id) || (expected != null && expected.name !== port.name));
  // The fabric monitor's verdicts for this node's halves; the worst one colours the link.
  const halves = (fabric?.ifaces ?? []).filter((i) => i.node === n.id);
  const worst = halves.reduce<(typeof halves)[number] | null>((w, i) => (!w || rank(i.verdict.level) > rank(w.verdict.level) ? i : w), null);
  const misplaced = halves.find((i) => i.cablingOk === false && i.expectedPort && i.switchPort);
  const cabling = misplaced
    ? `Expected ${shortPort(misplaced.expectedPort)} · found ${shortPort(misplaced.switchPort)}`
    : miscabled && port ? `Expected ${expected ? short(expected.name) : "another port"} · found ${short(port.name)}` : null;
  return {
    node: n,
    rx: viaSwitch ? port!.txBps : fabricRx(n),
    tx: viaSwitch ? port!.rxBps : fabricTx(n),
    cap: fabricCapBps(n) || (port?.capacityMbps ? port.capacityMbps * 1e6 : FABRIC_LINK_BPS),
    up: reporting ? up.length > 0 && (port ? port.running : true) : port ? port.running : null,
    nics: (up.length ? up : nets).map((x: NetStats) => x.iface),
    port,
    errors,
    cabling,
    health: worst && worst.verdict.level === "crit" ? "crit" : worst && worst.verdict.level === "warn" ? "warn" : "ok",
    verdict: worst && (worst.verdict.level === "warn" || worst.verdict.level === "crit") ? worst.verdict.title : null,
  };
}

const rank = (l: string) => (l === "crit" ? 3 : l === "warn" ? 2 : l === "info" ? 1 : 0);

/** "↓ 62.6 Mb/s ↑ 940 kb/s" on a wire: each direction in its own unit, so a trickle never reads 0. */
const fmtPair = (rx: number, tx: number) => fmtRatePair(rx, tx, { arrows: true });

/** "12%", "3.4%", "<0.1%": a link's share of its speed. */
export function fmtShare(u: number): string {
  if (u <= 0) return "0%";
  if (u < 0.001) return "<0.1%";
  return `${(u * 100).toFixed(u < 0.1 ? 1 : 0)}%`;
}

type ColorBy = "health" | "gpu" | "temp" | "mem";

/** The colour-policy tone of a status: quiet when healthy, amber, or red (offline is red, never dimmed). */
type Tone = "ok" | "warn" | "crit" | "unset";
const toneOf = (st: DisplayStatus): Tone =>
  st === "online" ? "ok" : st === "unconfigured" ? "unset" : st === "warn" || st === "stale" || st === "connecting" ? "warn" : "crit";
const toneOfGlyph = (g: GlyphState): Tone => (g === "warn" ? "warn" : g === "crit" || g === "offline" ? "crit" : "ok");

/**
 * A node as Topology draws it (M11): its own colour (nodeTone: its link and its own alerts,
 * never a model's), planned while SparkBox restarts it, and the notes for models it serves
 * that aren't Ready, the same on every member of a pipeline.
 */
function nodeLook(n: NodeSnapshot, health: HealthState | null, nodes: NodeSnapshot[], endpoints: Endpoint[]): { glyph: GlyphState; tone: Tone; notes: NodeNote[]; issues: { summary: string; severity: string }[] } {
  const notes = nodeNotes(n.id, health, endpoints);
  const tone = nodeTone(n.id, health, { nodes });
  const glyph: GlyphState = n.status !== "online" && n.maintenance ? "planned" : tone !== "ok" ? tone : notes.length ? "affected" : "ok";
  return { glyph, tone: toneOfGlyph(glyph), notes, issues: nodeAlerts(n.id, health) as { summary: string; severity: string }[] };
}

function nodeTint(n: NodeSnapshot, by: ColorBy): { value: number; label: string } | null {
  if (by === "health" || n.status !== "online" || n.stale) return null;
  if ((by === "gpu" || by === "temp") && gpuError(n)) return null;
  if (by === "gpu") return { value: n.gpu.utilPct / 100, label: `GPU ${n.gpu.utilPct.toFixed(0)}%` };
  if (by === "temp") return { value: Math.max(0, (n.gpu.tempC - 30) / 60), label: `${n.gpu.tempC.toFixed(0)} °C` };
  return { value: memPct(n) / 100, label: `Memory ${memPct(n).toFixed(0)}%` };
}

interface Hover {
  x: number;
  y: number;
  body: ReactNode;
}

/** Diagram text sizes, in diagram units (scaled with the whole drawing, never below 11). */
const FS = { title: 13, sub: 12, base: 12.5, small: 11 } as const;

/** A labelled usage bar; `color` is the metric it measures ("var(--m-mem)"), worn where meters are coloured (Black). */
function Bar({ x, y, w, value, label, color }: { x: number; y: number; w: number; value: number; label: string; color?: string }) {
  return (
    <g style={color ? ({ "--meter-color": color } as CSSProperties) : undefined}>
      <text x={x} y={y} className="topo-small">{label}</text>
      <rect x={x} y={y + 4} width={w} height={4} rx={2} className="topo-track" />
      <rect x={x} y={y + 4} width={Math.max(0, Math.min(1, value)) * w} height={4} rx={2} className="topo-bar" />
    </g>
  );
}

/**
 * One line of SVG text that fits `maxW`: measured with getComputedTextLength and cut with "…"
 * when it doesn't, the whole text in its <title>.
 */
function FitText({ x, y, text, maxW, className, fontSize }: { x: number; y: number; text: string; maxW: number; className?: string; fontSize?: number }) {
  const ref = useRef<SVGTextElement>(null);
  const [shown, setShown] = useState(text);
  useLayoutEffect(() => {
    const el = ref.current;
    const svg = el?.ownerSVGElement;
    if (!el || !svg) return;
    // A scratch copy beside it does the measuring, so React's own text node is never touched.
    const probe = el.cloneNode(false) as SVGTextElement;
    probe.setAttribute("visibility", "hidden");
    probe.removeAttribute("id");
    svg.appendChild(probe);
    const width = (t: string) => {
      probe.textContent = t;
      return typeof probe.getComputedTextLength === "function" ? probe.getComputedTextLength() : t.length * 6;
    };
    let next = text;
    if (width(text) > maxW) {
      let lo = 0;
      let hi = text.length;
      while (lo < hi) {
        const mid = Math.ceil((lo + hi) / 2);
        if (width(`${text.slice(0, mid).trimEnd()}…`) <= maxW) lo = mid;
        else hi = mid - 1;
      }
      next = `${text.slice(0, lo).trimEnd()}…`;
    }
    probe.remove();
    setShown(next);
  }, [text, maxW]);
  return (
    <text ref={ref} x={x} y={y} className={className} fontSize={fontSize}>
      {shown !== text && <title>{text}</title>}
      {shown}
    </text>
  );
}

/** A state mark inside the SVG (§2.3): a quiet dot when healthy, else StateGlyph's shape. */
function Glyph({ x, y, state, label }: { x: number; y: number; state: GlyphState; label?: string }) {
  if (state === "ok") return <circle cx={x} cy={y} r={3} className="topo-ok-dot"><title>Healthy</title></circle>;
  return (
    <g transform={`translate(${x - 6} ${y - 6})`} className="topo-glyph">
      <StateGlyph state={state} label={label} size={12} />
    </g>
  );
}

/** "Serving llama-3.3-70b · down": neutral, the model's state word in its tone, linking to the model. */
function NoteChip({ x, y, w, note, onOpen }: { x: number; y: number; w: number; note: NodeNote; onOpen: () => void }) {
  const head = note.text.slice(0, note.text.length - note.state.length);
  const room = Math.floor(w / 6.4) - 2;
  const shown = head.length + note.state.length > room ? `${head.slice(0, Math.max(4, room - note.state.length - 1))}…` : head;
  return (
    <g
      transform={`translate(${x} ${y})`}
      className="topo-note"
      role="link"
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
    >
      <title>{note.reason ? `${note.text}: ${note.reason}` : note.text}</title>
      <rect width={Math.min(w, (shown.length + note.state.length) * 6.4 + 16)} height={16} rx={8} className="topo-chip" />
      <text x={8} y={11.5} className="topo-chip-text">
        {shown}
        <tspan className={`topo-note-state tone-${note.tone}`}>{note.state}</tspan>
      </text>
    </g>
  );
}

/** A box's state band: the top of the box, tinted amber or red. */
function Band({ r, h, tone }: { r: Rect; h: number; tone: Tone }) {
  if (tone !== "warn" && tone !== "crit") return null;
  const k = 8;
  return <path d={`M${r.x} ${r.y + h} V${r.y + k} Q${r.x} ${r.y} ${r.x + k} ${r.y} H${r.x + r.w - k} Q${r.x + r.w} ${r.y} ${r.x + r.w} ${r.y + k} V${r.y + h} Z`} className={`topo-band ${tone}`} />;
}

function Pill({ x, y, text, tone, dim }: { x: number; y: number; text: string; tone?: "down" | "warn"; dim?: boolean }) {
  const w = text.length * 6.3 + 14;
  return (
    <g className={`topo-pill${tone ? ` ${tone}` : ""}${dim ? " dim" : ""}`} transform={`translate(${x - w / 2} ${y - 9})`}>
      <rect width={w} height={18} rx={9} />
      <text x={w / 2} y={12.5} textAnchor="middle">{text}</text>
    </g>
  );
}

// ---- List topology (narrow, short or tiled windows)

function ListRow({ level, status, glyph, name, state, detail, onOpen, children }: {
  level: 0 | 1;
  status: DisplayStatus;
  /** The node's own glyph (nodeLook), when it isn't the status's. */
  glyph?: GlyphState;
  name: ReactNode;
  state: string;
  detail?: ReactNode;
  onOpen?: () => void;
  children?: ReactNode;
}) {
  const g = glyph ?? glyphOfStatus(status);
  const tone = glyph ? toneOfGlyph(glyph) : toneOf(status);
  return (
    <li className={`topo-li l${level} tone-${tone}`}>
      <span className="topo-li-mark">{g === "ok" ? <span className="ok-dot" role="img" aria-label="Healthy" /> : <StateGlyph state={g} label={glyph ? undefined : DISPLAY_LABEL[status]} />}</span>
      <span className="topo-li-name">{onOpen ? <button type="button" className="entity-link" onClick={onOpen}>{name}</button> : name}</span>
      <span className={`topo-li-state ${tone}`} title={state}>{state}</span>
      <span className="topo-li-detail" title={typeof detail === "string" ? detail : undefined}>{detail}</span>
      {children}
    </li>
  );
}

function TopologyList({ nodes, links, crs, lanSw, gold, health, endpoints, macLabel, onSwitch }: {
  nodes: NodeSnapshot[];
  endpoints: Endpoint[];
  links: FabricLink[];
  crs: SwitchSnapshot | undefined;
  lanSw: SwitchSnapshot | undefined;
  gold: SwitchSnapshot | undefined;
  health: HealthState | null;
  macLabel: string;
  onSwitch: (sw: SwitchSnapshot | undefined) => void;
}) {
  const now = Date.now();
  const swRow = (sw: SwitchSnapshot | undefined, fallback: string, what: string) => {
    const st: DisplayStatus = sw ? switchDisplayStatus(sw, health) : "unconfigured";
    const issue = sw ? health?.switches[sw.id]?.issues[0]?.summary : undefined;
    const state = st === "unconfigured" ? "Not set up" : sw?.status === "online" ? issue ?? (sw.latencyMs != null ? `${what} · ${sw.latencyMs.toFixed(1)} ms` : what) : "Unreachable";
    return { st, state, name: sw?.name ?? fallback };
  };
  const g = swRow(gold, "Gateway", "Router · VPN endpoint");
  const l = swRow(lanSw, "Firewalla Switch X", "LAN · 10 GbE");
  const c = swRow(crs, "CRS804 DDQ", "Fabric · 200G per node");
  const nodeState = (n: NodeSnapshot) => {
    const st = displayStatus(n, health);
    const look = nodeLook(n, health, nodes, endpoints);
    const note = look.notes.map((x) => x.text).join(" · ");
    if (n.status !== "online" || n.stale) return { st, glyph: look.glyph, state: stateWord(n, now), reason: [offlineReason(n), note].filter(Boolean).join(" · ") || null };
    return { st, glyph: look.glyph, state: look.issues[0]?.summary ?? (note || "Healthy"), reason: null };
  };
  return (
    <div className="topo-list" role="region" aria-label="Topology as a list">
      <ul className="topo-ul" aria-label="LAN">
        <ListRow level={0} status={g.st} name={g.name} state={g.state} onOpen={() => onSwitch(gold)} />
        <ListRow level={0} status={l.st} name={l.name} state={l.state} onOpen={() => onSwitch(lanSw)} />
        {nodes.map((n) => {
          const s = nodeState(n);
          const lan = n.nets.find((x) => x.role === "lan");
          const reporting = n.status === "online" && !n.stale;
          const detail = lan ? (lan.up ? `${lan.speedMbps >= 1000 ? `${lan.speedMbps / 1000}G` : `${lan.speedMbps}M`} · ${reporting ? fmtRatePair(lanRx(n), lanTx(n)) : "—"}` : "LAN down") : "—";
          return (
            <ListRow key={n.id} level={1} status={s.st} glyph={s.glyph} name={<EntityLink kind="node" id={n.id} />} state={s.reason ? `${s.state} · ${s.reason}` : s.state} detail={detail} />
          );
        })}
        <ListRow level={1} status="online" name="This Mac" state={macLabel} />
      </ul>
      <ul className="topo-ul" aria-label="Fabric">
        <ListRow level={0} status={c.st} name={c.name} state={c.state} onOpen={() => onSwitch(crs)} />
        {links.map((lk) => {
          const tone = lk.up === null ? "ok" : !lk.up || lk.health === "crit" ? "crit" : lk.cabling || lk.health === "warn" ? "warn" : "ok";
          const status: DisplayStatus = lk.up === null ? "unconfigured" : tone === "crit" ? "crit" : tone === "warn" ? "warn" : "online";
          const reporting = lk.node.status === "online" && !lk.node.stale;
          const state = lk.up === null ? "No reading" : !lk.up ? "Link down" : lk.cabling ?? lk.verdict ?? `Up · ${Math.round(lk.cap / 1e9)}G`;
          return (
            <ListRow key={lk.node.id} level={1} status={status} name={<>{lk.port ? <span className="mono">{short(lk.port.name)}</span> : "—"} <span className="muted">→</span> <EntityLink kind="node" id={lk.node.id} /></>}
              state={state} detail={lk.up ? `${fmtRatePair(lk.rx, lk.tx)} · ${fmtShare(dirUtil(Math.max(lk.rx, lk.tx), lk.cap))}${reporting ? "" : " (switch)"}` : "—"} />
          );
        })}
      </ul>
    </div>
  );
}

// ---- Window

export function Topology() {
  const cluster = useCluster();
  const health = useHealth();
  const endpoints = useEndpoints();
  const vpn = useVpnStatus();
  const wm = useWM();
  const { textScale } = useAppearance();
  const fabric = useFabricHealth();
  const wrapRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(960);
  const [height, setHeight] = useState(540);
  const [fit, setFit] = useWinState<boolean>("fit", true);
  const [colorBy, setColorBy] = useWinState<ColorBy>("colorBy", "health");
  const [hover, setHover] = useState<Hover | null>(null);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    let frame = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        setWidth(el.clientWidth);
        setHeight(el.clientHeight);
      });
    });
    ro.observe(el);
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
    };
  }, []);

  const macVia: "lan" | "vpn" = vpn?.state === "connected" ? "vpn" : "lan";
  const lod = useMemo(() => topologyScale(width, height, cluster?.nodes.length ?? 4, macVia, textScale, fit), [cluster?.nodes.length, width, height, macVia, fit, textScale]);
  if (!cluster) return <Waiting />;

  const now = Date.now();
  const nodes = cluster.nodes;
  const crs = cluster.switches.find((s) => s.kind === "routerOs");
  const lanSw = cluster.switches.find((s) => s.kind === "firewalla");
  const gold = cluster.switches.find((s) => s.kind === "gateway");
  const links = nodes.map((n) => fabricLink(n, crs, fabric));
  const L = lod.layout;
  const macLabel = macWords(vpn);
  const openSwitch = (sw: SwitchSnapshot | undefined) => (!sw || sw.configured === false ? wm.open("settings", { section: "switches" }) : wm.open("switch", { id: sw.id, name: sw.name }));

  const showHover = (e: MouseEvent, body: ReactNode) => {
    const box = wrapRef.current?.getBoundingClientRect();
    if (!box) return;
    setHover({ x: e.clientX - box.left + (wrapRef.current?.scrollLeft ?? 0), y: e.clientY - box.top + (wrapRef.current?.scrollTop ?? 0), body });
  };
  const hide = () => setHover(null);
  const keyOpen = (open: () => void) => (e: React.KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      open();
    }
  };

  const center = (r: Rect) => ({ x: r.x + r.w / 2, y: r.y + r.h / 2 });
  const compact = L.mode === "compact";

  // Where each node's fabric wire meets the fabric switch.
  const portPoint = (i: number) => {
    const r = L.fabricSwitch;
    if (compact) {
      const nb = L.nodes[i];
      return { x: r.x, y: nb.y + nb.h / 2 };
    }
    const slots = nodes.length;
    return { x: r.x + ((i + 0.5) / slots) * r.w, y: r.y };
  };
  const nodeFabricPoint = (i: number) => {
    const nb = L.nodes[i];
    return compact ? { x: nb.x + nb.w, y: nb.y + nb.h / 2 } : { x: nb.x + nb.w / 2, y: nb.y + nb.h };
  };
  const nodeLanPoint = (i: number) => {
    const nb = L.nodes[i];
    return compact ? { x: nb.x, y: nb.y + nb.h / 2 } : { x: nb.x + nb.w / 2, y: nb.y };
  };

  // ---- Wires first
  const wires: ReactNode[] = [];
  const labels: ReactNode[] = [];

  const wanUp = gold?.configured !== false && gold?.status === "online";
  wires.push(
    <line key="wan" x1={center(L.internet).x} y1={L.internet.y + L.internet.h} x2={center(L.gateway).x} y2={L.gateway.y}
      className={`topo-wire wan${wanUp ? "" : " unknown"}`} />,
  );
  wires.push(<line key="gw-lan" x1={center(L.gateway).x} y1={L.gateway.y + L.gateway.h} x2={center(L.lanSwitch).x} y2={L.lanSwitch.y} className="topo-wire lan" />);

  // LAN: Switch X to every node (thin, dashed, neutral).
  nodes.forEach((n, i) => {
    const a = compact ? { x: L.lanSwitch.x + 10, y: L.lanSwitch.y + L.lanSwitch.h } : { x: center(L.lanSwitch).x + (i - (nodes.length - 1) / 2) * 14, y: L.lanSwitch.y + L.lanSwitch.h };
    const b = nodeLanPoint(i);
    const path = compact ? `M${a.x} ${a.y} V${b.y} H${b.x}` : `M${a.x} ${a.y} L${b.x} ${b.y}`;
    // A node that isn't reporting can't say whether its LAN port is up: that wire is drawn as unknown, not down.
    const reportingLan = n.status === "online" && !n.stale;
    const up = n.nets.some((x) => x.role === "lan" && x.up);
    wires.push(<path key={`lan-${n.id}`} d={path} className={`topo-wire lan${!reportingLan ? " unknown" : up ? "" : " down"}`} />);
    if (n.status === "online" && !n.stale && !compact) {
      const t = { x: a.x + (b.x - a.x) * 0.62, y: a.y + (b.y - a.y) * 0.62 };
      labels.push(<Pill key={`lan-l-${n.id}`} x={t.x} y={t.y} text={fmtPair(lanRx(n), lanTx(n))} dim />);
    }
  });

  // Management: CRS804 ether1 up to Switch X, round the side; only for a switch SparkBox talks
  // to (an unconfigured one has no ports to know it by).
  if (crs?.configured) {
    const fs = L.fabricSwitch;
    const ls = L.lanSwitch;
    const ether = crs.ports.find((p) => p.name === "ether1");
    const sideX = Math.max(6, Math.min(fs.x, L.nodes[0].x) - 14);
    const d = compact
      ? `M${fs.x + fs.w / 2} ${fs.y} V${ls.y + ls.h / 2} H${ls.x + ls.w}`
      : `M${fs.x} ${fs.y + fs.h / 2} H${sideX} V${ls.y + ls.h / 2} H${ls.x}`;
    wires.push(<path key="mgmt" d={d} className={`topo-wire mgmt${ether && !ether.running ? " down" : ""}`} />);
    labels.push(<text key="mgmt-l" x={sideX + 6} y={ls.y + ls.h + 18} className="topo-small muted">ether1 · management</text>);
  }

  // This Mac: on the LAN, or through the VPN tunnel to the gateway.
  const macCenter = center(L.mac);
  if (macVia === "vpn") {
    const gw = center(L.gateway);
    const macLeft = L.mac.x < L.internet.x;
    const from = macLeft ? L.mac.x + L.mac.w : L.mac.x;
    const to = macLeft ? L.internet.x : L.internet.x + L.internet.w;
    wires.push(<path key="vpn" d={`M${from} ${macCenter.y} H${to} M${center(L.internet).x} ${L.internet.y + L.internet.h} L${gw.x - 12} ${L.gateway.y}`} className="topo-wire vpn" />);
    if (Math.abs(to - from) > 90) {
      labels.push(<Pill key="vpn-l" x={(from + to) / 2} y={macCenter.y - 14} text={`Tunnel${vpn?.latencyMs != null ? ` · ${vpn.latencyMs.toFixed(0)} ms` : ""}`} />);
    }
  } else {
    wires.push(<line key="mac-lan" x1={L.lanSwitch.x + L.lanSwitch.w} y1={macCenter.y} x2={L.mac.x} y2={macCenter.y} className="topo-wire lan" />);
  }

  // Fabric: two half-links per node; thickness is each direction's load, colour is the link's health.
  links.forEach((lk, i) => {
    const a = nodeFabricPoint(i);
    const b = portPoint(i);
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len = Math.hypot(dx, dy) || 1;
    const nx = -dy / len;
    const ny = dx / len;
    const reporting = lk.node.status === "online" && !lk.node.stale;
    const uTx = dirUtil(lk.tx, lk.cap);
    const uRx = dirUtil(lk.rx, lk.cap);
    const tone = lk.up === null ? "unknown" : !lk.up || lk.health === "crit" ? "down" : lk.cabling || lk.health === "warn" ? "warn" : "ok";
    const half = (dir: "tx" | "rx", u: number, bps: number) => {
      const w = lk.up ? loadWidth(u) : 2;
      const off = (dir === "tx" ? -1 : 1) * (w / 2 + 1.5);
      const from = dir === "tx" ? a : b;
      const to = dir === "tx" ? b : a;
      const d = `M${from.x + nx * off} ${from.y + ny * off} L${to.x + nx * off} ${to.y + ny * off}`;
      const moving = lk.up && reporting && u > 0.01;
      return (
        <g key={`${dir}-${lk.node.id}`}>
          <path d={d} className={`topo-fabric ${tone}`} style={{ strokeWidth: w }} />
          {moving && <path d={d} className="topo-flow" style={{ strokeWidth: Math.max(1.5, w - 2), animationDuration: `${flowSeconds(bps)}s` }} />}
        </g>
      );
    };
    const hoverBody = (
      <div className="topo-hover">
        <b>{lk.node.name} ↔ {crs?.name ?? "fabric switch"}</b>
        {lk.up === null ? (
          <div className="muted">No reading: {lk.node.name} isn't reporting and its switch port isn't known</div>
        ) : lk.up ? (
          <>
            <div className="num">↓ {fmtBits(lk.rx)} · {fmtShare(uRx)} <span className="muted">received</span></div>
            <div className="num">↑ {fmtBits(lk.tx)} · {fmtShare(uTx)} <span className="muted">sent</span></div>
            <div className="muted">of {fmtBits(lk.cap)}</div>
          </>
        ) : (
          <div className="crit-text">Link down</div>
        )}
        {lk.verdict && <div className={lk.health === "crit" ? "crit-text" : "warn-text"}>{lk.verdict}</div>}
        {lk.cabling && <div className="warn-text">{lk.cabling}</div>}
        <div>Node: <span className="mono">{lk.nics.join(" + ") || "—"}</span></div>
        <div>Switch: <span className="mono">{lk.port?.name ?? "not mapped"}</span></div>
        <div>Errors since boot: <span className="num">{lk.errors.toLocaleString()}</span></div>
        {lk.port?.laneRxPowerDbm?.length ? (
          <div>Worst lane: <span className="num">{Math.min(...lk.port.laneRxPowerDbm).toFixed(1)} dBm</span></div>
        ) : null}
      </div>
    );
    const openPort = () => crs && wm.open("switch", { id: crs.id, name: crs.name, ...(lk.port ? { port: lk.port.name } : {}) });
    wires.push(
      <g
        key={`fab-${lk.node.id}`}
        className="topo-link"
        role="button"
        tabIndex={0}
        aria-label={`Fabric link ${lk.node.name}: ${lk.up === null ? "no reading" : lk.up ? `receiving ${fmtBits(lk.rx)}, sending ${fmtBits(lk.tx)}` : "down"}${lk.cabling ? `, ${lk.cabling}` : ""}`}
        onClick={openPort}
        onKeyDown={keyOpen(openPort)}
        onMouseMove={(e) => showHover(e, hoverBody)}
        onMouseLeave={hide}
      >
        <path d={`M${a.x} ${a.y} L${b.x} ${b.y}`} className="topo-hit" />
        {half("tx", uTx, lk.tx)}
        {half("rx", uRx, lk.rx)}
      </g>,
    );
    if (lk.port) {
      labels.push(
        <g key={`port-${lk.node.id}`}>
          <rect x={b.x - 4} y={b.y} width={8} height={5} rx={1.5} className={`topo-stub${lk.up ? "" : " down"}`} />
          <text x={b.x} y={b.y + 18} textAnchor="middle" className="topo-small topo-port">{short(lk.port.name)}</text>
        </g>,
      );
    }
    // The node's interface names live in the hover: drawn here they collided with the rate pill.
    const mid = { x: a.x + dx * 0.42, y: a.y + dy * 0.42 };
    const util = Math.max(uRx, uTx);
    labels.push(
      <Pill key={`fab-l-${lk.node.id}`} x={mid.x} y={mid.y}
        text={lk.up === null ? "No reading" : !lk.up ? "Link down" : lk.cabling ?? lk.verdict ?? `${fmtPair(lk.rx, lk.tx)} · ${fmtShare(util)}${reporting ? "" : " (switch)"}`}
        tone={lk.up === null ? undefined : !lk.up || lk.health === "crit" ? "down" : lk.cabling || lk.health === "warn" ? "warn" : undefined} dim={lk.up === null} />,
    );
  });

  // ---- Boxes
  const switchBox = (r: Rect, sw: SwitchSnapshot | undefined, fallback: string, sub: string, icon: ReactNode) => {
    const st: DisplayStatus = sw ? switchDisplayStatus(sw, health) : "unconfigured";
    const tone = toneOf(st);
    const unset = st === "unconfigured";
    const open = () => openSwitch(sw);
    const name = sw?.name ?? fallback;
    const issue = sw ? health?.switches[sw.id]?.issues[0]?.summary : undefined;
    const line = unset ? "Not set up" : sw?.status === "online" ? issue ?? `${sub}${sw.latencyMs != null ? ` · ${sw.latencyMs.toFixed(1)} ms` : ""}` : `Unreachable${sw?.error ? "" : ""}`;
    return (
      <g
        className={`topo-box tone-${tone}`}
        role="button"
        tabIndex={0}
        aria-label={`${name}: ${DISPLAY_LABEL[st]}`}
        onClick={open}
        onKeyDown={keyOpen(open)}
        onMouseMove={(e) => showHover(e, <div className="topo-hover"><b>{name}</b><div>{DISPLAY_LABEL[st]}</div>{sw?.version && <div className="muted">RouterOS {sw.version}</div>}{(health?.switches[sw?.id ?? ""]?.issues ?? []).map((i) => <div key={i.alertId} className={i.severity === "crit" ? "crit-text" : "warn-text"}>{i.summary}</div>)}</div>)}
        onMouseLeave={hide}
      >
        <rect x={r.x} y={r.y} width={r.w} height={r.h} rx={8} />
        <Band r={r} h={r.h} tone={tone} />
        <g transform={`translate(${r.x + 10} ${r.y + r.h - 39})`}>{icon}</g>
        <text x={r.x + 32} y={r.y + r.h - 27} className="topo-title" fontSize={FS.title}>{name}</text>
        <Glyph x={r.x + r.w - 14} y={r.y + r.h - 32} state={glyphOfStatus(st)} label={DISPLAY_LABEL[st]} />
        <FitText x={r.x + 32} y={r.y + r.h - 10} text={line} maxW={r.w - 44} className={`topo-sub${tone === "crit" ? " crit-fill" : tone === "warn" ? " warn-fill" : ""}`} fontSize={FS.sub} />
      </g>
    );
  };

  const nodeBox = (n: NodeSnapshot, r: Rect) => {
    const st = displayStatus(n, health);
    const look = nodeLook(n, health, nodes, endpoints);
    const tone = look.tone;
    const issues = look.issues;
    const note = look.notes[0];
    const openNote = (x: NodeNote) => wm.open(x.link.kind, x.link.params);
    const reporting = n.status === "online" && !n.stale;
    const tint = nodeTint(n, colorBy);
    const serving = engines(n);
    const open = () => wm.open("node", { id: n.id });
    const barW = r.w - 24;
    const reason = reporting ? null : offlineReason(n);
    const hoverBody = (
      <div className="topo-hover">
        <b>{n.name}</b>
        <div>{issues.map((i) => i.summary).join(" · ") || (look.glyph === "ok" || look.glyph === "affected" ? DISPLAY_LABEL.online : DISPLAY_LABEL[st])}</div>
        {look.notes.map((x) => <div key={x.text}>{x.text}{x.reason ? <span className="muted"> · {x.reason}</span> : null}</div>)}
        {reporting ? (
          <div className="topo-hover-sparks">
            <span>GPU</span><Sparkline values={recent(n.id, "gpu", 120)} width={110} height={18} color="var(--m-gpu)" curve="monotone" />
            <span>CPU</span><Sparkline values={recent(n.id, "cpu", 120)} width={110} height={18} color="var(--m-cpu)" curve="monotone" />
            <span>Fabric</span><Sparkline values={recent(n.id, "fabricRx", 120)} max={Math.max(1, ...recent(n.id, "fabricRx", 120))} width={110} height={18} color="var(--m-rx)" curve="monotone" />
          </div>
        ) : (
          <div>{stateWord(n, now)}{n.error ? <div className="muted">{n.error}</div> : null}</div>
        )}
      </div>
    );
    const head = 30;
    return (
      <g
        key={n.id}
        className={`topo-box topo-node tone-${tone}`}
        role="button"
        tabIndex={0}
        aria-label={`${n.name}: ${reporting ? (issues.length ? DISPLAY_LABEL[look.tone === "crit" ? "crit" : "warn"] : DISPLAY_LABEL.online) : stateWord(n, now)}${reason ? `, ${reason}` : ""}${issues.length ? `, ${issues.map((i) => i.summary).join(", ")}` : ""}${look.notes.length ? `, ${look.notes.map((x) => x.text).join(", ")}` : ""}`}
        onClick={open}
        onKeyDown={keyOpen(open)}
        onContextMenu={(e) => {
          hide();
          showContextMenu(e, nodeMenu(n));
        }}
        onMouseMove={(e) => showHover(e, hoverBody)}
        onMouseLeave={hide}
      >
        <rect x={r.x} y={r.y} width={r.w} height={r.h} rx={8} style={tint ? { fill: `color-mix(in srgb, ${rampVar(tint.value)} var(--topo-tint, 22%), var(--surface))` } : undefined} />
        <Band r={r} h={head} tone={tone} />
        <g transform={`translate(${r.x + 10} ${r.y + 7})`}><Server size={16} className="topo-icon" /></g>
        <text x={r.x + 32} y={r.y + 20} className="topo-title" fontSize={FS.title}>{n.name}</text>
        <Glyph x={r.x + r.w - 14} y={r.y + 15} state={look.glyph} />
        {reporting ? (
          <>
            {tone === "warn" && issues[0] ? (
              <FitText x={r.x + 12} y={r.y + 46} text={issues[0].summary} maxW={barW} className="topo-small warn-fill" />
            ) : (
              <Bar x={r.x + 12} y={r.y + 44} w={barW} value={gpuError(n) ? 0 : n.gpu.utilPct / 100} label={gpuError(n) ? "GPU —" : `GPU ${n.gpu.utilPct.toFixed(0)}% · ${n.gpu.tempC.toFixed(0)} °C`} color="var(--m-gpu)" />
            )}
            <Bar x={r.x + 12} y={r.y + 66} w={barW} value={n.mem.used / Math.max(1, n.mem.total)} label={`Memory ${fmtBytesPair(n.mem.used, n.mem.total)}`} color="var(--m-mem)" />
            {note ? (
              <NoteChip x={r.x + 12} y={r.y + 82} w={barW} note={note} onOpen={() => openNote(note)} />
            ) : (
              <g transform={`translate(${r.x + 12} ${r.y + 82})`}>
                <rect width={Math.min(barW, (serving[0] ?? "Idle").length * 6.4 + 16)} height={16} rx={8} className="topo-chip" />
                <text x={8} y={11.5} className="topo-chip-text">{serving.length ? serving.join(", ").slice(0, Math.floor(barW / 6.4) - 2) : "Idle"}</text>
              </g>
            )}
            {tint && <text x={r.x + r.w - 12} y={r.y + 94} textAnchor="end" className="topo-small">{tint.label}</text>}
          </>
        ) : (
          <>
            <text x={r.x + 12} y={r.y + 52} className={`topo-state ${tone}`} fontSize={FS.base}>{stateWord(n, now)}</text>
            {reason && <FitText x={r.x + 12} y={r.y + 72} text={reason} maxW={barW} className={`topo-small ${tone === "crit" ? "crit-fill" : "warn-fill"}`} />}
            {note ? (
              <NoteChip x={r.x + 12} y={r.y + 82} w={barW} note={note} onOpen={() => openNote(note)} />
            ) : (
              <text x={r.x + 12} y={r.y + 92} className="topo-small muted">{n.system.model ? n.system.model.slice(0, Math.floor(barW / 6)) : ""}</text>
            )}
          </>
        )}
      </g>
    );
  };

  return (
    <div className="col fill mon-topo">
      <Toolbar label="Topology">
        {!lod.list && (
          <Segmented<ColorBy>
            label="Color nodes by"
            value={colorBy}
            onChange={setColorBy}
            options={[{ value: "health", label: "Health" }, { value: "gpu", label: "GPU load" }, { value: "temp", label: "GPU temp" }, { value: "mem", label: "Memory" }]}
          />
        )}
        {!lod.list && <HelpTip topic="mon.topology.color-by" />}
        {lod.list && <span className="toolbar-text">List view: the diagram doesn't fit this window</span>}
        <Spacer />
        <ToolButton icon={Maximize2} label="Fit" active={fit} onClick={() => setFit(!fit)}
          title={fit ? "Fitting the window (the list shows when the diagram can't fit at full size)" : "Fit the window"} />
      </Toolbar>
      <div ref={wrapRef} className={`topo-wrap${lod.list ? " list" : ""}`} onMouseLeave={hide}>
        {lod.list ? (
          <TopologyList nodes={nodes} links={links} crs={crs} lanSw={lanSw} gold={gold} health={health} endpoints={endpoints} macLabel={macLabel} onSwitch={openSwitch} />
        ) : (
          <svg
            width={L.width * lod.scale}
            height={L.height * lod.scale}
            viewBox={`0 0 ${L.width} ${L.height}`}
            className="topo"
            fontSize={FS.small}
            role="group"
            aria-label="Network topology: the gateway, the LAN switch, the nodes and the fabric switch"
          >
            <g className="topo-wires">{wires}</g>
            <g className="topo-boxes">
              <g className="topo-box topo-cloud" aria-label="Internet">
                <rect x={L.internet.x} y={L.internet.y} width={L.internet.w} height={L.internet.h} rx={18} />
                <g transform={`translate(${L.internet.x + 14} ${L.internet.y + 10})`}><Cloud size={16} className="topo-icon" /></g>
                <text x={L.internet.x + 38} y={L.internet.y + 23} className="topo-title" fontSize={FS.title}>Internet</text>
              </g>
              {switchBox(L.gateway, gold, "Gateway", "Router · VPN endpoint", <Router size={16} className="topo-icon" />)}
              {switchBox(L.lanSwitch, lanSw, "Firewalla Switch X", "LAN · 10 GbE", <Network size={16} className="topo-icon" />)}
              {crs && switchBox(L.fabricSwitch, crs, "CRS804 DDQ", "Fabric · 200G per node", <Network size={16} className="topo-icon" />)}
              {nodes.map((n, i) => nodeBox(n, L.nodes[i]))}
              <g className="topo-box topo-mac" role="button" tabIndex={0} aria-label={`This Mac: ${macLabel}`}
                onClick={() => wm.open("vpn")} onKeyDown={keyOpen(() => wm.open("vpn"))}>
                <rect x={L.mac.x} y={L.mac.y} width={L.mac.w} height={L.mac.h} rx={8} />
                <g transform={`translate(${L.mac.x + 10} ${L.mac.y + 10})`}><Laptop size={16} className="topo-icon" /></g>
                <text x={L.mac.x + 32} y={L.mac.y + 16} className="topo-title" fontSize={FS.sub}>This Mac</text>
                <text x={L.mac.x + 32} y={L.mac.y + 30} className="topo-small muted">{macLabel}</text>
              </g>
            </g>
            <g className="topo-labels">{labels}</g>
          </svg>
        )}
        {hover && !lod.list && (
          <div className="topo-hovercard" style={{ left: Math.min(hover.x + 14, (wrapRef.current?.scrollWidth ?? 9999) - 250), top: hover.y + 14 }}>
            {hover.body}
          </div>
        )}
      </div>
      {!lod.list && (
        <div className="topo-legend" aria-label="Legend">
          <span><i className="lg fabric" />Fabric</span>
          <span><i className="lg lan" />LAN</span>
          {crs?.configured && <span><i className="lg mgmt" />Management</span>}
          <span><i className="lg vpn" />VPN tunnel</span>
          <HelpTip topic="mon.topology.diagram" label="About the diagram" />
        </div>
      )}
    </div>
  );
}
