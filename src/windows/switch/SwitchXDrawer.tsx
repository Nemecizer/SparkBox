import { useEffect, useRef, useState, type ReactNode } from "react";
import { ExternalLink, X } from "lucide-react";
import { HelpTip, Notice } from "../../components/ui";
import { ADMIN_REFUSED, isLookOnly, LOOK_ONLY_TIP, useAdminState, withAdmin } from "../../lib/admin";
import { firewallaApi, type LabelKind, type MspPort, type MspStatus, type PortLabel } from "../../lib/firewalla";
import { fmtBits, fmtBytes, fmtDateTime, fmtRelative } from "../../lib/format";
import type { ClusterSnapshot } from "../../lib/types";
import { useWM } from "../../wm/WindowManager";
import { behindText, flaps, fmtMbps, labelText, type XPort } from "./switchX";

/** Where a fact came from and how old it is. */
interface Source {
  from: string;
  atMs?: number | null;
}

/** One fact and its source. */
function Fact({ label, src, children }: { label: string; src?: Source | null; children: ReactNode }) {
  const age = src?.atMs ? fmtRelative(src.atMs) : "";
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        {children}
        {src && <span className="sx-src">{src.from}{age && ` · ${age}`}</span>}
      </dd>
    </div>
  );
}

const count = (n: number | null | undefined) => (n == null ? "—" : n.toLocaleString());
const bytes = (n: number | null | undefined) => (n == null ? "—" : fmtBytes(n));
const pair = (a: number | null | undefined, b: number | null | undefined) => (a == null && b == null ? "—" : `${count(a)} in, ${count(b)} out`);

const CHOICES: { value: string; label: string }[] = [
  { value: "crs804", label: "CRS804" },
  { value: "gold", label: "Firewalla Gold" },
  { value: "mac", label: "This Mac" },
  { value: "text", label: "Something else…" },
  { value: "empty", label: "Empty" },
];

const choiceOf = (l: PortLabel | null) => (!l ? "" : l.kind === "node" ? `node:${l.node}` : l.kind);

/** The owner says what's plugged in. It changes a file on this Mac, never the switch. */
function LabelEditor({ p, switchId, cluster, onSaved }: { p: XPort; switchId: string; cluster: ClusterSnapshot; onSaved: () => void }) {
  const [choice, setChoice] = useState(choiceOf(p.label));
  const [text, setText] = useState(p.label?.text ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const lookOnly = isLookOnly(useAdminState());
  // Another port, or a label saved from elsewhere.
  useEffect(() => {
    setChoice(choiceOf(p.label));
    setText(p.label?.text ?? "");
    setError(null);
  }, [p.n, p.label]);

  const save = async () => {
    const node = choice.startsWith("node:") ? choice.slice(5) : null;
    const label: PortLabel | null = !choice ? null : { kind: (node ? "node" : choice) as LabelKind, node, text: choice === "text" ? text : null };
    setBusy(true);
    setError(null);
    try {
      await withAdmin(() => firewallaApi.labelSet(switchId, p.n, label), { scope: "mac", action: "firewalla_label_set" });
      onSaved();
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setBusy(false);
    }
  };

  const changed = choice !== choiceOf(p.label) || (choice === "text" && text !== (p.label?.text ?? ""));
  return (
    <div className="sx-editor">
      <label htmlFor="sx-plugged">What's plugged in</label>
      <select id="sx-plugged" value={choice} onChange={(e) => setChoice(e.target.value)}>
        <option value="">Not labeled</option>
        <optgroup label="Nodes">
          {cluster.nodes.map((n) => <option key={n.id} value={`node:${n.id}`}>{n.name}</option>)}
        </optgroup>
        <optgroup label="Other">
          {CHOICES.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
        </optgroup>
      </select>
      {choice === "text" && (
        <input aria-label="What's plugged in, in your words" value={text} maxLength={60} placeholder="Printer, NAS, access point…" spellCheck={false} onChange={(e) => setText(e.target.value)} />
      )}
      <button type="button" className="btn primary" disabled={!changed || busy || lookOnly} title={lookOnly ? LOOK_ONLY_TIP : undefined} onClick={save}>
        {busy ? "Saving…" : "Save label"}
      </button>
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
    </div>
  );
}

/** One port's details: every fact, with where it came from and how old it is. */
export function SwitchXDrawer({ p, switchId, cluster, msp, onClose, onSaved }: {
  p: XPort;
  switchId: string;
  cluster: ClusterSnapshot;
  msp: MspStatus | undefined;
  onClose: () => void;
  onSaved: () => void;
}) {
  const wm = useWM();
  const panel = useRef<HTMLElement>(null);
  // Keyboard users land here when a port opens it, and Esc goes back.
  useEffect(() => panel.current?.focus(), [p.n]);

  const m: MspPort | null = p.msp;
  const unused = p.mspUnused;
  const cloudSrc: Source = { from: unused ? "Firewalla cloud, not lighting this port" : "Firewalla cloud", atMs: msp?.lastOkMs };
  const lldpSrc: Source = { from: "LLDP", atMs: p.lldp?.readMs };
  const nodeSrc: Source | null = p.node ? { from: `${p.node.name}'s own report`, atMs: p.node.lastSeenMs ?? cluster.tsMs } : null;
  const labelSrc: Source = { from: "Your label", atMs: p.label?.setAtMs };
  const behind = p.lldp ? behindText(p.lldp) : null;
  const speedSrc = p.speedFrom === "firewalla" ? cloudSrc : p.speedFrom === "node" ? nodeSrc : p.light !== "off" && m?.linkUp ? cloudSrc : p.lldp ? lldpSrc : null;
  const linkSrc = m?.linkUp != null && !unused ? cloudSrc : p.node && p.nic ? nodeSrc : p.lldp ? lldpSrc : p.label && p.label.kind !== "empty" ? labelSrc : m?.linkUp != null ? cloudSrc : null;
  const noData = p.label && !p.node && p.label.kind !== "empty" && p.label.kind !== "node";

  const speedValue = p.speedText ?? (m?.linkSpeedMbps && !unused ? fmtMbps(m.linkSpeedMbps) : p.light === "off" ? "—" : "Not known");
  const since = m?.statsSinceS ? fmtDateTime(m.statsSinceS * 1000) : null;
  const nic = p.nic;
  const poeKnown = m && (m.poeStatus || m.poeMode || m.poeWatts != null);

  return (
    <aside
      className="port-drawer sx-drawer"
      aria-label={`Port ${p.n} details`}
      ref={panel}
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <header className="port-drawer-head">
        <div className="port-drawer-titles">
          <div className="port-drawer-title">Port {p.n}</div>
          <span className="muted">{p.kind === "rj45" ? "10G RJ45 · PoE++" : "10G SFP+"}</span>
        </div>
        <button type="button" className="tool" onClick={onClose} aria-label="Close details" title="Close details">
          <X size={14} />
        </button>
      </header>
      <div className="port-drawer-body">
        {p.notes.map((n) => <Notice key={n} tone="warn">{n}</Notice>)}

        <dl className="kv sx-facts">
          <Fact label="Link" src={linkSrc}>{p.linkWord}</Fact>
          <Fact label="Speed" src={speedSrc}>{speedValue}</Fact>
          {behind && !p.speedText && <Fact label="Behind it" src={lldpSrc}>{behind}</Fact>}
          {p.lldp && (
            <Fact label="LLDP port id" src={lldpSrc}>
              <span className="mono">{p.lldp.rawIds.join(", ")}</span>
              <span className="sx-src">SparkBox reads this as port {p.n}. Whether it's the same as the number on the front isn't confirmed.</span>
            </Fact>
          )}
        </dl>

        <h4 className="section-title">What's connected</h4>
        {p.label && p.label.kind !== "empty" ? (
          <>
            <dl className="kv sx-facts">
              <Fact label="You said" src={labelSrc}>{labelText(p.label, cluster)}</Fact>
            </dl>
            {noData && <p className="muted sx-plain">SparkBox has no live data for this device.</p>}
          </>
        ) : p.label?.kind === "empty" ? (
          <p className="muted sx-plain">You marked this port empty.</p>
        ) : p.lldp ? (
          <p className="muted sx-plain">Nothing is labeled here. LLDP can't say what's on the cable: it hears the whole LAN segment behind this port.</p>
        ) : p.light !== "off" ? (
          <p className="muted sx-plain">Something's connected. Firewalla's cloud doesn't list devices by port.</p>
        ) : (
          <p className="muted sx-plain">Nothing known.</p>
        )}
        {p.node && (
          <button type="button" className="btn" onClick={() => wm.open("node", { id: p.node!.id })}>
            <ExternalLink size={12} /> Open {p.node.name}
          </button>
        )}

        {p.lldp && (
          <>
            <h4 className="section-title">Heard through this port <HelpTip topic="mon.switch.x-heard" /></h4>
            <p className="muted sx-plain">These share the LAN segment behind this port. That doesn't mean each is plugged into it.</p>
            <table className="grid compact sx-heard" aria-label="Heard through this port">
              <thead>
                <tr><th>Device</th><th>Its port</th><th>Heard by</th></tr>
              </thead>
              <tbody>
                {p.lldp.heard.map((d) => (
                  <tr key={d.key} title={d.chassis ?? undefined}>
                    <td>{d.name}</td>
                    <td className="mono">{d.port ?? "—"}</td>
                    <td>{d.heardBy.length === p.lldp!.reporters.length && d.heardBy.length > 1 ? "All" : d.heardBy.join(", ")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <span className="sx-src">LLDP · {fmtRelative(p.lldp.readMs ?? cluster.tsMs)}</span>
          </>
        )}

        {(m || p.node) && <h4 className="section-title">Traffic and errors</h4>}
        {m && (
          <dl className="kv sx-facts">
            <Fact label="Data" src={cloudSrc}>{m.rxBytes == null && m.txBytes == null ? "—" : `${bytes(m.rxBytes)} in, ${bytes(m.txBytes)} out`}{since && <span className="sx-src">Since {since}</span>}</Fact>
            <Fact label="Errors" src={cloudSrc}>{pair(m.rxErrors, m.txErrors)}</Fact>
            <Fact label="Discards" src={cloudSrc}>{pair(m.rxDiscards, m.txDiscards)}</Fact>
          </dl>
        )}
        {p.node && !nic && <p className="muted sx-plain">{p.node.name} reports no LAN link.</p>}
        {nic && (
          <dl className="kv sx-facts">
            <Fact label={`${p.node!.name}'s link`} src={nodeSrc}>{nic.up ? `Up, ${fmtMbps(nic.speedMbps)}` : "Down"}</Fact>
            <Fact label="Traffic" src={nodeSrc}>{`${fmtBits(nic.rxBps)} in, ${fmtBits(nic.txBps)} out`}</Fact>
            <Fact label="Errors" src={nodeSrc}>{pair(nic.rxErrors, nic.txErrors)}</Fact>
            <Fact label="Link flaps" src={nodeSrc}>{count(flaps(nic))}</Fact>
          </dl>
        )}

        {(poeKnown || m?.sfp || m?.stpState || m?.uplink) && <h4 className="section-title">Port details</h4>}
        {m && (poeKnown || m.sfp || m.stpState || m.uplink) && (
          <dl className="kv sx-facts">
            {m.uplink && <Fact label="Role" src={cloudSrc}>Uplink to the Firewalla box</Fact>}
            {poeKnown && <Fact label="PoE" src={cloudSrc}>{[m.poeStatus, m.poeMode, m.poeWatts != null ? `${m.poeWatts} W` : null].filter(Boolean).join(", ")}</Fact>}
            {m.sfp && <Fact label="SFP module" src={cloudSrc}>{m.sfp.present === false ? "None" : [m.sfp.connector, m.sfp.bitRate].filter((v) => v && v !== "N/A").join(", ") || "Present"}</Fact>}
            {m.stpState && <Fact label="Spanning tree" src={cloudSrc}>{[m.stpState, m.stpRole].filter(Boolean).join(", ")}</Fact>}
          </dl>
        )}

        <h4 className="section-title">Your label <HelpTip topic="mon.switch.x-label" /></h4>
        <LabelEditor p={p} switchId={switchId} cluster={cluster} onSaved={onSaved} />

        <p className="muted sx-plain">Changes to this port are made in the Firewalla app. This panel reads, and never changes the switch.</p>
      </div>
    </aside>
  );
}
