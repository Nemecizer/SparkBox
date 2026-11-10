import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, Cable, Plus, Trash2 } from "lucide-react";
import { Icon } from "../../components/Icon";
import { DotGlyph, FormGroup, HelpTip, Notice, type DotStatus } from "../../components/ui";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { api } from "../../lib/api";
import { fabricErrors, inventoryRequiresReconnect, isIPv4 } from "../../lib/fabricInventory";
import { FabricSection } from "./FabricSection";
export { isIPv4 } from "../../lib/fabricInventory";
import { netApi, type CheckStatus, type CheckStep } from "../../lib/netApi";
import { hostKeyState, listNames, nodeActionFor, sharingKey, shortFingerprint, testSummary, trustKey } from "../../lib/nodeStatus";
import { useCluster } from "../../lib/store";
import type { Inventory, NodeConfig, NodeSnapshot } from "../../lib/types";
import { openWindow } from "../../wm/WindowManager";
import { usePrefs } from "./GeneralSection";

// ---- One shared draft of the inventory (nodes, switches, login user), so the
// Cluster login, Nodes and Switches sections edit the same copy and one Save
// bar covers all of them.

interface InvState {
  saved: Inventory | null;
  draft: Inventory | null;
  error: string | null;
  saving: boolean;
  notice: string | null;
}

let inv: InvState = { saved: null, draft: null, error: null, saving: false, notice: null };
let loading = false;
const listeners = new Set<() => void>();

function set(patch: Partial<InvState>) {
  inv = { ...inv, ...patch };
  listeners.forEach((l) => l());
}

function load() {
  if (loading) return;
  loading = true;
  api.getInventory().then(
    (i) => set({ saved: i, draft: inv.draft && dirtyOf(inv) ? inv.draft : i, error: null }),
    (e) => set({ error: `Couldn't read the inventory: ${e}` }),
  ).finally(() => {
    loading = false;
  });
}

const dirtyOf = (s: InvState) => !!s.saved && !!s.draft && JSON.stringify(s.saved) !== JSON.stringify(s.draft);

const HOSTNAME = /^(?=.{1,253}$)[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$/;

/** A hostname or an IPv4 address. Dotted all-number strings must be valid IPv4. */
export const isHost = (s: string) => (/^[\d.]+$/.test(s) ? isIPv4(s) : HOSTNAME.test(s));
export const isPort = (n: number) => Number.isInteger(n) && n >= 1 && n <= 65535;

export type FieldErrors = Record<string, string>;

/** Problems that block saving, keyed "node:<i>:<field>" / "switch:<i>:<field>". */
export function validateInventory(i: Inventory): FieldErrors {
  const errs: FieldErrors = {};
  const names = new Map<string, number>();
  i.nodes.forEach((n, k) => {
    if (!n.name.trim()) errs[`node:${k}:name`] = "Give the node a name.";
    else if (names.has(n.name.trim().toLowerCase())) errs[`node:${k}:name`] = `Another node is already called ${n.name.trim()}.`;
    names.set(n.name.trim().toLowerCase(), k);
    if (!n.host.trim()) errs[`node:${k}:host`] = "Enter the node's hostname or IP address.";
    else if (!isHost(n.host.trim())) errs[`node:${k}:host`] = `“${n.host}” isn't a hostname or IPv4 address.`;
    if (n.ip && !isIPv4(n.ip.trim())) errs[`node:${k}:ip`] = `“${n.ip}” isn't an IPv4 address (like 192.168.50.11).`;
    if (!isPort(n.port)) errs[`node:${k}:port`] = "SSH port must be between 1 and 65535.";
    if (!n.user.trim()) errs[`node:${k}:user`] = "Set the cluster user in Cluster login.";
    for (const [field, message] of Object.entries(fabricErrors(n.fabric))) errs[`node:${k}:fabric:${field}`] = `${n.name}: ${message}`;
  });
  i.switches.forEach((s, k) => {
    if (s.host.trim() && !isHost(s.host.trim())) errs[`switch:${k}:host`] = `“${s.host}” isn't a hostname or IPv4 address.`;
  });
  return errs;
}

function slug(name: string, taken: Set<string>): string {
  const base = name.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, "") || "node";
  let id = base;
  for (let n = 2; taken.has(id); n++) id = `${base}-${n}`;
  return id;
}

export function useInventoryDraft() {
  const s = useSyncExternalStore((l) => {
    listeners.add(l);
    if (!inv.saved && !inv.error) load();
    return () => listeners.delete(l);
  }, () => inv);
  return {
    ...s,
    dirty: dirtyOf(s),
    errors: s.draft ? validateInventory(s.draft) : {},
    edit: (fn: (d: Inventory) => Inventory) => inv.draft && set({ draft: fn(inv.draft), notice: null }),
    revert: () => set({ draft: inv.saved, notice: null }),
    reload: () => {
      set({ error: null });
      load();
    },
    save: async () => {
      if (!inv.draft || Object.keys(validateInventory(inv.draft)).length) return;
      set({ saving: true });
      try {
        // The inventory is Mac-side state: Touch ID or the password, asked once per unlock.
        const draft = inv.draft;
        await withAdmin(() => api.saveInventory(draft), { scope: "mac", action: "save_inventory" });
        set({ saved: draft, saving: false, error: null, notice: "Saved." });
      } catch (e) {
        set({ saving: false, error: e === ADMIN_REFUSED ? null : `Couldn't save: ${(e as { message?: string })?.message ?? e}` });
      }
    },
  };
}

/** Shown under any inventory section while it has unsaved edits. */
export function InventorySaveBar() {
  const d = useInventoryDraft();
  const { prefs } = usePrefs();
  const reconnect = prefs?.dataSource === "live" && !!d.draft && !!d.saved && inventoryRequiresReconnect(d.draft, d.saved);
  const problems = Object.keys(d.errors).length;
  if (!d.dirty) return d.notice ? <p className="settings-saved muted" role="status">{d.notice}</p> : null;
  return (
    <div className="settings-savebar" role="region" aria-label="Unsaved changes">
      <span className="settings-savebar-text">
        {problems
          ? `${problems} field${problems === 1 ? " needs" : "s need"} fixing before saving.`
          : reconnect
            ? "Unsaved changes. Applying reconnects to the nodes."
            : "Unsaved changes to the inventory. Saving keeps connections open."}
      </span>
      <button type="button" className="btn" onClick={d.revert} disabled={d.saving}>Revert</button>
      <button type="button" className="btn primary" onClick={d.save} disabled={d.saving || problems > 0}>
        {d.saving ? "Saving…" : reconnect ? "Apply & reconnect" : "Save"}
      </button>
    </div>
  );
}

function Field({ value, onChange, label, error, className, type, inputMode, placeholder }: {
  value: string;
  onChange: (v: string) => void;
  label: string;
  error?: string;
  className?: string;
  type?: string;
  inputMode?: "numeric" | "decimal" | "text";
  placeholder?: string;
}) {
  return (
    <input
      className={className}
      type={type}
      inputMode={inputMode}
      value={value}
      placeholder={placeholder}
      aria-label={label}
      aria-invalid={error ? true : undefined}
      title={error}
      spellCheck={false}
      autoComplete="off"
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

/** GB10 machines, by the name on the box. "Other…" keeps whatever was typed before. */
export const MODELS = [
  "MSI EdgeXpert", "ASUS Ascent GX10", "NVIDIA DGX Spark", "Dell Pro Max GB10", "Lenovo ThinkStation PGX", "HP ZGX Nano", "Gigabyte AI TOP ATOM",
] as const;
const OTHER = "Other…";

function ModelSelect({ value, label, onChange }: { value: string; label: string; onChange: (v: string) => void }) {
  const known = (MODELS as readonly string[]).includes(value);
  const [custom, setCustom] = useState(!known && !!value);
  if (custom) {
    return (
      <span className="inv-model-other">
        <Field value={value} label={label} placeholder="Model name" onChange={onChange} />
        <button type="button" className="tool icon-only" title="Choose from the list" aria-label={`${label}: choose from the list`} onClick={() => {
          setCustom(false);
          if (!known) onChange(MODELS[0]);
        }}>…</button>
      </span>
    );
  }
  return (
    <select aria-label={label} value={known ? value : ""} onChange={(e) => {
      if (e.target.value === OTHER) setCustom(true);
      else onChange(e.target.value);
    }}>
      {!known && <option value="" disabled>Choose…</option>}
      {MODELS.map((m) => <option key={m} value={m}>{m}</option>)}
      <option value={OTHER}>{OTHER}</option>
    </select>
  );
}

const CHECK_DOT: Record<CheckStatus, DotStatus> = { ok: "online", warn: "warn", fail: "error", skip: "unconfigured" };
const CHECK_WORD: Record<CheckStatus, string> = { ok: "Passed", warn: "Warning", fail: "Failed", skip: "Skipped" };

interface Probe {
  node: string;
  busy: boolean;
  steps?: CheckStep[];
  error?: string;
}

/** The checklist from "Test": DNS, TCP, host key, login, sudo, Python, Docker, GPU, fabric and Wake-on-LAN. */
function ProbeResult({ probe, onClose, onRetest, cluster }: { probe: Probe; onClose: () => void; onRetest: () => void; cluster: NodeSnapshot[] }) {
  const summary = probe.steps ? testSummary(probe.steps) : null;
  const trust = async (s: CheckStep) => {
    if (!s.address || !s.fingerprint) return;
    const ok = await trustKey({ name: probe.node, address: s.address, fingerprint: s.fingerprint, sharedWith: sharingKey(cluster, s.fingerprint, probe.node), changed: s.status === "fail" });
    if (ok) onRetest();
  };
  const keyStep = probe.steps?.find((s) => s.id === "hostkey" && s.fingerprint && s.address);
  return (
    <FormGroup
      title={`Test of ${probe.node}`}
      footnote={probe.busy ? "Connecting with the settings shown above (nothing is saved)…" : summary && (
        <span className={`probe-summary probe-summary-${summary.tone}`} role="status">
          {summary.text}
          {summary.trust && keyStep && <> <button type="button" className="btn small" onClick={() => void trust(keyStep)}>Trust…</button></>}
        </span>
      )}
    >
      {probe.error && <div className="fg-row"><span className="error-text">{probe.error}</span></div>}
      {probe.busy && !probe.steps && <div className="fg-row"><span className="muted">Running checks…</span></div>}
      {probe.steps?.map((s) => (
        <div key={s.id} className="fg-row probe-row">
          <DotGlyph status={CHECK_DOT[s.status]} word={s.status === "skip" && !s.blocked ? "Not readable" : CHECK_WORD[s.status]} />
          <span className="probe-label">{s.label}</span>
          <span className="probe-detail">
            {s.detail}
            {s.hint && <span className="fg-hint">{s.hint}</span>}
            {s.fingerprint && <span className="mono fg-hint">{s.fingerprint}</span>}
          </span>
          {s.id === "hostkey" && s.fingerprint && s.address && (
            <button type="button" className="btn small probe-action" onClick={() => void trust(s)}>{s.status === "fail" ? "Review…" : "Trust…"}</button>
          )}
        </div>
      ))}
      {!probe.busy && <div className="fg-row probe-foot"><button type="button" className="btn small" onClick={onClose}>Done</button></div>}
    </FormGroup>
  );
}

/** Wake-on-LAN as the node's collector could read it, in grey words: unknown is not off. */
export function wakeWords(w: { enabled: boolean | null; detail?: string | null } | undefined): { text: string; title: string } {
  if (w?.enabled === true) return { text: "Wake: On", title: `Wake-on-LAN: ${w.detail ?? "wakes on a magic packet"}` };
  if (w?.enabled === false) return { text: "Wake: Off", title: `Wake-on-LAN: ${w.detail ?? "off on the LAN port"}` };
  return { text: "Wake: Unknown · needs sudo", title: `Wake-on-LAN: ${w?.detail ?? "Not readable without sudo (ethtool needs root)"}` };
}

/** "cell1, cell2 and cell3 share one host key (a cloned image) · trusted by you". */
export function sharedKeyNote(nodes: NodeSnapshot[]): string | null {
  const groups = new Map<string, string[]>();
  for (const n of nodes) {
    const fp = n.hostKey?.fingerprint;
    if (fp && (n.hostKey?.sharedWith?.length ?? 0) > 0) groups.set(fp, [...(groups.get(fp) ?? []), n.name]);
  }
  const [fp, names] = [...groups.entries()].find(([, v]) => v.length > 1) ?? [];
  if (!fp || !names) return null;
  const trusted = names.every((name) => nodes.find((n) => n.name === name)?.status === "online");
  return `${listNames(names)} share one host key, ${shortFingerprint(fp)} (a cloned image)${trusted ? " · trusted by you" : ""}`;
}

/**
 * Each node's LAN MAC and Wake-on setting as Wake-on-LAN sees them (node_wol_status: the
 * collector's MAC, remembered while the node is off). Falls back to the live snapshot.
 */
function useWakeInfo(ids: string[]): Record<string, { mac: string | null; enabled: boolean | null; detail: string | null }> {
  const [info, setInfo] = useState<Record<string, { mac: string | null; enabled: boolean | null; detail: string | null }>>({});
  const key = ids.join(",");
  useEffect(() => {
    let alive = true;
    for (const id of ids) {
      invoke<{ mac?: string | null; enabled?: boolean | null; detail?: string | null }>("node_wol_status", { node: id })
        .then((w) => alive && setInfo((m) => ({ ...m, [id]: { mac: w?.mac ?? null, enabled: w?.enabled ?? null, detail: w?.detail ?? null } })))
        .catch(() => {
          // An older core without Wake-on-LAN: the snapshot's MAC below.
        });
    }
    return () => {
      alive = false;
    };
  }, [key]);
  return info;
}

export function NodesSection() {
  const d = useInventoryDraft();
  const cluster = useCluster();
  const wake = useWakeInfo(d.draft?.nodes.map((n) => n.id) ?? []);
  /** The LAN MAC Wake-on-LAN sends its packet to. */
  const lanMac = (id: string) => wake[id]?.mac ?? cluster?.nodes.find((n) => n.id === id)?.nets.find((x) => x.role === "lan" && x.mac)?.mac ?? null;
  const macTitle = (id: string) => (lanMac(id) ? "The LAN port's MAC address, reported by the node's collector; Wake sends its packet there" : "The LAN port's MAC address: not reported yet, the node hasn't been reached");
  const snapOf = (id: string) => cluster?.nodes.find((n) => n.id === id);
  const keyNote = sharedKeyNote(cluster?.nodes ?? []);
  const [removed, setRemoved] = useState<string | null>(null);
  const [probe, setProbe] = useState<Probe | null>(null);
  if (!d.draft) {
    return d.error ? <Notice tone="error" onRetry={d.reload}>{d.error}</Notice> : <p className="muted">Loading the inventory…</p>;
  }
  const nodes = d.draft.nodes;
  const editNode = (i: number, patch: Partial<NodeConfig>) =>
    d.edit((x) => ({ ...x, nodes: x.nodes.map((n, j) => (j === i ? { ...n, ...patch } : n)) }));
  const move = (i: number, by: -1 | 1) =>
    d.edit((x) => {
      const next = [...x.nodes];
      [next[i], next[i + by]] = [next[i + by], next[i]];
      return { ...x, nodes: next };
    });
  const add = () =>
    d.edit((x) => {
      const taken = new Set(x.nodes.map((n) => n.id));
      const name = `spark${x.nodes.length + 1}`;
      const id = slug(name, taken);
      const template = x.nodes[x.nodes.length - 1];
      return {
        ...x,
        nodes: [...x.nodes, { id, name, host: "", ip: null, model: template?.model ?? "NVIDIA DGX Spark", port: 22, user: template?.user ?? "" }],
      };
    });
  const remove = (i: number) => {
    setRemoved(nodes[i].name);
    d.edit((x) => ({ ...x, nodes: x.nodes.filter((_, j) => j !== i) }));
  };
  const errs = Object.entries(d.errors).filter(([k]) => k.startsWith("node:"));
  const test = async (n: NodeConfig) => {
    setProbe({ node: n.name, busy: true });
    try {
      // The test logs in with this Mac's keys and the saved password: Mac scope.
      setProbe({ node: n.name, busy: false, steps: await withAdmin(() => netApi.nodeProbe(n), { scope: "mac", action: "node_probe" }) });
    } catch (e) {
      if (e === ADMIN_REFUSED) return setProbe(null);
      setProbe({ node: n.name, busy: false, error: String((e as { message?: string })?.message ?? e) });
    }
  };

  return (
    <>
      {d.error && <Notice tone="error" onRetry={d.reload}>{d.error}</Notice>}
      <FormGroup
        title={<>Inventory <HelpTip topic="start.nodes.inventory" label="About the inventory" /></>}
      >
        <div className="inv-table-wrap">
          <table className="grid inv-table">
            <colgroup>
              <col className="inv-c-name" />
              <col className="inv-c-host" />
              <col className="inv-c-ip" />
              <col className="inv-c-model" />
              <col className="inv-c-port" />
              <col className="inv-c-actions" />
            </colgroup>
            <thead>
              <tr>
                <th><span className="th-label">Name</span></th>
                <th><span className="th-label">Hostname</span> <HelpTip topic="start.nodes.hostname" /></th>
                <th><span className="th-label">IP address</span> <HelpTip topic="start.nodes.ip-fallback" /></th>
                <th><span className="th-label">Model</span></th>
                <th className="align-right"><span className="th-label">SSH port</span> <HelpTip topic="start.nodes.ssh-port" /></th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {nodes.map((n, i) => {
                const who = n.name || `Node ${i + 1}`;
                return (
                  <tr key={n.id}>
                    <td><Field value={n.name} label={`${who} name`} error={d.errors[`node:${i}:name`]} onChange={(v) => editNode(i, { name: v })} /></td>
                    <td>
                      <Field value={n.host} label={`${who} hostname`} error={d.errors[`node:${i}:host`]} placeholder="cell1.local" onChange={(v) => editNode(i, { host: v })} />
                      {/* Read-only, so it rides under the hostname instead of taking a column the inputs need. */}
                      <div className="inv-mac mono" title={macTitle(n.id)}>{lanMac(n.id) ? `MAC ${lanMac(n.id)}` : "MAC not reported yet"}</div>
                      <div className="inv-wol" title={wakeWords(wake[n.id]).title}>{wakeWords(wake[n.id]).text}{i === 0 && <> <HelpTip topic="start.nodes.wake" /></>}</div>
                    </td>
                    <td><Field value={n.ip ?? ""} label={`${who} IP address`} error={d.errors[`node:${i}:ip`]} placeholder="Optional" inputMode="decimal" onChange={(v) => editNode(i, { ip: v.trim() ? v : null })} /></td>
                    <td><ModelSelect value={n.model} label={`${who} model`} onChange={(v) => editNode(i, { model: v })} /></td>
                    <td>
                      <Field
                        className="narrow num"
                        value={Number.isFinite(n.port) ? String(n.port) : ""}
                        label={`${who} SSH port`}
                        inputMode="numeric"
                        error={d.errors[`node:${i}:port`]}
                        onChange={(v) => editNode(i, { port: v.trim() === "" ? Number.NaN : Number(v) })}
                      />
                    </td>
                    <td className="inv-actions">
                      {(() => {
                        const live = snapOf(n.id);
                        const action = live && hostKeyState(live) ? nodeActionFor(live) : null;
                        return action && <button type="button" className="btn small" onClick={() => action.run()}>{action.label}</button>;
                      })()}
                      <button
                        type="button"
                        className="btn small"
                        aria-label={`Test ${who}`}
                        title={`Connect to ${who} with these settings and check SSH, sudo, Docker, the GPU and the fabric`}
                        disabled={!!d.errors[`node:${i}:host`] || !n.host.trim() || probe?.busy}
                        onClick={() => test(n)}
                      >
                        Test
                      </button>
                      {i === 0 && <HelpTip topic="start.nodes.test" />}
                      <button type="button" className="tool icon-only" aria-label={`Move ${who} up`} title="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
                        <Icon icon={ArrowUp} size={14} />
                      </button>
                      <button type="button" className="tool icon-only" aria-label={`Move ${who} down`} title="Move down" disabled={i === nodes.length - 1} onClick={() => move(i, 1)}>
                        <Icon icon={ArrowDown} size={14} />
                      </button>
                      <button type="button" className="tool icon-only danger" aria-label={`Remove ${who}`} title="Remove from the inventory" disabled={nodes.length <= 1} onClick={() => remove(i)}>
                        <Icon icon={Trash2} size={14} />
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <div className="inv-foot">
          <button type="button" className="btn" onClick={add}>
            <Icon icon={Plus} size={14} />Add node
          </button>
          <button type="button" className="btn" onClick={() => openWindow("connect")}>
            <Icon icon={Cable} size={14} />Connect my Sparks…
          </button>
          {removed && d.dirty && <span className="muted" role="status">Removed {removed}. Revert to bring it back.</span>}
        </div>
        {keyNote && <div className="inv-keynote muted" title="Compare it on a node: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub">{keyNote}</div>}
      </FormGroup>
      <FabricSection nodes={nodes} snapshots={cluster?.nodes ?? []} editNode={editNode} />
      {probe && (
        <ProbeResult
          probe={probe}
          cluster={cluster?.nodes ?? []}
          onClose={() => setProbe(null)}
          onRetest={() => {
            const n = nodes.find((x) => x.name === probe.node);
            if (n) void test(n);
          }}
        />
      )}
      {errs.length > 0 && (
        <ul className="field-errors" aria-live="polite">
          {errs.map(([k, v]) => <li key={k}>{v}</li>)}
        </ul>
      )}
      <InventorySaveBar />
    </>
  );
}
