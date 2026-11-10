// Settings › Fence: the place to let the test build make changes on chosen nodes for a limited
// time, the key it trusts, who holds each node's controller lease, and which machine each node is
// pinned as. The release build needs no fence for its own changes (Admin mode is enough): it opens
// and closes them for the test build (Touch ID that names the fence). The test build only reads
// them, and can trust the key the release build made. Demo ignores fences.
import { useEffect, useState } from "react";
import { FormGroup, FormRow, HelpTip } from "../../components/ui";
import { ADMIN_REFUSED, isLookOnly, LOOK_ONLY_TIP, useAdminState, withAdmin } from "../../lib/admin";
import { inTauri, invoke } from "../../lib/api";
import { useDataSource } from "../../lib/dataSource";
import { showError } from "../../lib/errors";
import {
  AUTONOMY_CLASSES, fenceApi, fmtLeft, ONLY_CLASSES, OWNER_CLASSES, presets, specSummary, useFence,
  type FenceClass, type FenceSpec, type FenceView, type IdentityView, type LeaseView,
} from "../../lib/fence";
import { fmtDateTime } from "../../lib/format";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";

const STATE_WORDS: Record<string, string> = {
  none: "No fence",
  valid: "Open",
  expired: "Expired",
  failed: "Failed its check: the test build's changes are refused until a new fence is opened",
  untrusted: "This build doesn't trust the fence key yet",
};

/** The time left on an open fence, and whose it is. One from before the release build stopped needing them does nothing. */
function openWords(view: FenceView): string {
  const left = `${fmtLeft(view.remainingMs)} left`;
  if (view.mine) return view.canOpen ? `${left} · for this build, which doesn't need one` : left;
  return `${left} · for the ${view.record?.instance === "dev" ? "test" : "release"} build`;
}

function leaseText(l: LeaseView): string {
  if (l.error) return l.error;
  if (!l.heldBy) return "Free";
  const who = l.mine ? "This build" : `SparkBox (${l.heldBy})`;
  return `${who} · ${l.tag ?? ""} · ${l.mac ?? ""} · ${l.ageS ?? 0} s ago`;
}

function pinText(v: IdentityView): string {
  if (!v.pin) return "Not pinned yet (pinned at its first data after its host key is trusted)";
  return [v.pin.hostname, v.pin.lanMac, v.pin.machineId ? `machine-id ${v.pin.machineId.slice(0, 8)}…` : null].filter(Boolean).join(" · ");
}

function OpenForm({ nodes, onOpened, held }: { nodes: { id: string; name: string }[]; onOpened: () => void; held: boolean }) {
  const [spec, setSpec] = useState<FenceSpec>({ nodes: [], classes: [], only: {}, minutes: 180, instance: "dev", note: "" });
  const [only, setOnly] = useState<Partial<Record<FenceClass, string>>>({});
  const [busy, setBusy] = useState(false);
  const toggle = <T,>(list: T[], x: T, on: boolean) => (on ? [...list.filter((y) => y !== x), x] : list.filter((y) => y !== x));
  const classBox = (c: FenceClass) => (
    <label key={c} className="check">
      <input type="checkbox" checked={spec.classes.includes(c)} onChange={(e) => setSpec({ ...spec, classes: toggle(spec.classes, c, e.target.checked) })} />
      <span className="mono">{c}</span>
    </label>
  );
  const usePreset = (p: FenceSpec) => {
    setSpec(p);
    setOnly(Object.fromEntries(Object.entries(p.only).map(([k, v]) => [k, (v ?? []).join(", ")])));
  };
  const open = async () => {
    const full: FenceSpec = {
      ...spec,
      only: Object.fromEntries(
        spec.classes.filter((c) => ONLY_CLASSES[c] && only[c]?.trim()).map((c) => [c, only[c]!.split(",").map((s) => s.trim()).filter(Boolean)]),
      ),
    };
    const ok = await confirmAction({
      title: `Open a fence: ${specSummary(full)}`,
      body: `The test build may make changes of these classes on ${full.nodes.join(", ")} until the fence runs out or you close it. It asks for Touch ID.`,
      confirmLabel: "Open fence…",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    setBusy(true);
    try {
      // The Touch ID sheet names this fence: SparkBox words it from the spec (exit.rs fence_confirm).
      await invoke("fence_confirm", { spec: full });
      await fenceApi.open(full);
      toast({ kind: "success", title: "Fence open", body: specSummary(full) });
      onOpened();
    } catch (e) {
      if (e !== ADMIN_REFUSED) showError(e, { action: "Open a fence" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormGroup title={<>Open a fence <HelpTip topic="start.fence.open" /></>} footnote="Lets the test build make these changes on these nodes for a limited time, 8 hours at most. This app doesn't need a fence: its changes need Admin mode.">
      <FormRow label="Presets">
        <span className="fg-inline">
          {presets(nodes.map((n) => n.id)).map((p) => (
            <button key={p.label} type="button" className="btn small" onClick={() => usePreset(p.spec)}>{p.label}</button>
          ))}
        </span>
      </FormRow>
      <FormRow label="Nodes">
        <span className="fence-checks">
          {nodes.map((n) => (
            <label key={n.id} className="check">
              <input type="checkbox" checked={spec.nodes.includes(n.id)} onChange={(e) => setSpec({ ...spec, nodes: toggle(spec.nodes, n.id, e.target.checked) })} />
              {n.name}
            </label>
          ))}
        </span>
      </FormRow>
      <FormRow label={<>With you present <HelpTip topic="start.fence.classes" /></>} hint="First runs, sudo, power, apt, network">
        <span className="fence-checks">{OWNER_CLASSES.map(classBox)}</span>
      </FormRow>
      <FormRow label="Agents alone" hint="Repeats agents may run by themselves">
        <span className="fence-checks">{AUTONOMY_CLASSES.map(classBox)}</span>
      </FormRow>
      {spec.classes.filter((c) => ONLY_CLASSES[c]).map((c) => (
        <FormRow key={c} label={`Only (${c})`} htmlFor={`fence-only-${c}`} hint={`${ONLY_CLASSES[c]}, comma-separated; empty allows any`}>
          <input id={`fence-only-${c}`} className="mono" value={only[c] ?? ""} spellCheck={false} onChange={(e) => setOnly({ ...only, [c]: e.target.value })} />
        </FormRow>
      ))}
      <FormRow label={<>Hours <HelpTip topic="start.fence.hours" /></>} htmlFor="fence-hours">
        <input id="fence-hours" type="number" min={1} max={8} step={1} value={spec.minutes / 60} onChange={(e) => setSpec({ ...spec, minutes: Math.round(Math.min(8, Math.max(0, Number(e.target.value) || 0)) * 60) })} />
      </FormRow>
      <FormRow label="Note" htmlFor="fence-note">
        <input id="fence-note" value={spec.note} onChange={(e) => setSpec({ ...spec, note: e.target.value })} />
      </FormRow>
      <FormRow label="">
        <button type="button" className="btn primary" onClick={() => void open()} disabled={held || busy || !spec.nodes.length || !spec.classes.length || !spec.minutes} title={held ? LOOK_ONLY_TIP : undefined}>
          {busy ? "Opening…" : "Open fence…"}
        </button>
      </FormRow>
    </FormGroup>
  );
}

export function FenceSection() {
  const live = useDataSource() === "live";
  // Look-only: what gives rights is off (with the reason); closing the fence only takes them away.
  const held = isLookOnly(useAdminState());
  const cluster = useCluster();
  const { view, refresh } = useFence(live);
  const [leases, setLeases] = useState<LeaseView[]>([]);
  const [ids, setIds] = useState<IdentityView[]>([]);
  const nodes = (cluster?.nodes ?? []).map((n) => ({ id: n.id, name: n.name }));

  useEffect(() => {
    if (!inTauri || !live) return;
    let alive = true;
    const load = () => {
      fenceApi.leases().then((l) => alive && setLeases(l), () => {});
      fenceApi.identity().then((v) => alive && setIds(v), () => {});
    };
    load();
    const t = window.setInterval(load, 15_000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [live, view?.state, view?.record?.id]);

  if (!live) {
    return (
      <FormGroup title={<>Fence <HelpTip topic="start.fence.about" label="About the fence" /></>}>
        <FormRow label="Fence">
          <span className="muted">Available with Live data</span>
        </FormRow>
      </FormGroup>
    );
  }

  const close = async () => {
    const ok = await confirmAction({ title: "Close the fence?", body: "The test build's changes on the cluster are refused again until a new fence is opened.", confirmLabel: "Close fence", cancelLabel: "Cancel" });
    if (!ok) return;
    try {
      await fenceApi.close();
      refresh();
    } catch (e) {
      showError(e, { action: "Close the fence" });
    }
  };
  const trust = async () => {
    try {
      await withAdmin(() => fenceApi.trustKey(), { scope: "mac", action: "fence_trust_key", fresh: true });
      refresh();
    } catch (e) {
      if (e !== ADMIN_REFUSED) showError(e, { action: "Trust this Mac's fence key" });
    }
  };
  const repin = async (v: IdentityView) => {
    const ok = await confirmAction({
      title: `Re-pin ${v.name}?`,
      body: `SparkBox accepts the machine answering as ${v.name} now${v.now ? ` (${v.now.hostname}${v.now.lanMac ? `, ${v.now.lanMac}` : ""})` : ""} as ${v.name}. Do this only after you moved or rebuilt it. Type its name to confirm.`,
      confirmLabel: "Re-pin…",
      cancelLabel: "Cancel",
      danger: true,
      requireTyping: v.name,
    });
    if (!ok) return;
    try {
      await withAdmin(() => fenceApi.repin(v.node, v.name), { scope: "mac", action: "node_identity_repin", fresh: true });
      setIds(await fenceApi.identity());
    } catch (e) {
      if (e !== ADMIN_REFUSED) showError(e, { action: `Re-pin ${v.name}` });
    }
  };

  const r = view?.record;
  return (
    <>
      <FormGroup
        title={<>Fence <HelpTip topic="start.fence.about" label="About the fence" /></>}
      >
        <FormRow label={<>State <HelpTip topic="start.fence.state" /></>} hint={view?.why ?? (view?.canOpen && view.state === "none" ? "The test build can't change anything on the cluster." : undefined)}>
          <span className={view?.state === "failed" ? "crit-text" : undefined}>
            {view ? STATE_WORDS[view.state] : "Reading…"}
            {view?.state === "valid" && r ? ` · ${openWords(view)}` : ""}
          </span>
        </FormRow>
        {r && (
          <>
            <FormRow label="Nodes"><span className="mono">{r.nodes.join(", ")}</span></FormRow>
            <FormRow label="Classes"><span className="mono">{r.classes.join(", ")}</span></FormRow>
            {Object.entries(r.only).map(([c, items]) => (
              <FormRow key={c} label={`Only (${c})`}><span className="mono">{(items ?? []).join(", ")}</span></FormRow>
            ))}
            <FormRow label="Opened" hint={`${r.id} · on ${r.mac}${r.note ? ` · ${r.note}` : ""}`}>
              <span>{fmtDateTime(r.createdMs)} · until {fmtDateTime(r.expiresMs)}</span>
            </FormRow>
          </>
        )}
        <FormRow label={<>Key <HelpTip topic="start.fence.key" /></>} hint={view?.keyKind === "secureEnclave" ? "Secure Enclave key" : "Software key in the release build's Keychain"}>
          <span className="mono">{view?.keyId ?? "None yet (made when the release build opens its first fence)"}</span>
        </FormRow>
        <FormRow label="Trusted here" hint={`This is the ${view?.instance ?? ""} build`}>
          <span className="mono">{view?.trustedKeyId ?? "No key trusted yet"}</span>
        </FormRow>
        {view?.canOpen ? (
          (view.state === "valid" || view.state === "expired" || view.state === "failed") && (
            <FormRow label="">
              <button type="button" className="btn" onClick={() => void close()}>Close fence</button>
            </FormRow>
          )
        ) : (
          <FormRow label="" hint={view?.canTrust ? "This test build needs a fence for every change: open it in the release SparkBox." : "This test build needs a fence for every change: open it in the release SparkBox. This build shares its settings, so it trusts the key the release build pinned."}>
            <span className="fg-inline">
              {view?.canTrust && <button type="button" className="btn" onClick={() => void trust()} disabled={held || !view.keyId || view.keyId === view.trustedKeyId} title={held ? LOOK_ONLY_TIP : undefined}>Trust this Mac's fence key</button>}
              {(view?.state === "valid" || view?.state === "expired") && <button type="button" className="btn" onClick={() => void close()}>Close fence</button>}
            </span>
          </FormRow>
        )}
      </FormGroup>
      {view?.canOpen && <OpenForm nodes={nodes} onOpened={refresh} held={held} />}
      <FormGroup title={<>Controller lease <HelpTip topic="leases-and-identity#controller-lease" /></>} footnote="Before a change, SparkBox claims the node; while the other SparkBox's claim is under 45 s old, changes from this one are refused.">
        {leases.map((l) => (
          <FormRow key={l.node} label={nodes.find((n) => n.id === l.node)?.name ?? l.node}>
            <span className={l.error ? "muted" : undefined}>{leaseText(l)}</span>
          </FormRow>
        ))}
      </FormGroup>
      <FormGroup title={<>Identity <HelpTip topic="leases-and-identity#identity-pin" /></>} footnote="Changes are refused on a node that answers as another machine. cell1, cell2 and cell3 share one host key, so the key alone doesn't say which Spark answered.">
        {ids.map((v) => (
          <FormRow key={v.node} label={v.name} hint={[v.note, v.matches === false && v.now ? `Answers as ${v.now.hostname}${v.now.lanMac ? ` · ${v.now.lanMac}` : ""}: changes refused` : null].filter(Boolean).join(" · ") || undefined}>
            <span className="fg-inline">
              <span className={v.matches === false ? "crit-text mono" : "mono"}>{pinText(v)}</span>
              <button type="button" className="btn small" onClick={() => void repin(v)} disabled={held || !v.now} title={held ? LOOK_ONLY_TIP : undefined}>Re-pin identity…</button>
            </span>
          </FormRow>
        ))}
      </FormGroup>
    </>
  );
}
