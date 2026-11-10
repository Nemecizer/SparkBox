// Settings › Security & privacy: look-only mode, how Admin mode locks itself, the SSH tunnels
// open now, how long the activity log is kept, and exporting it. Changing these is a change on
// this Mac: it asks for Touch ID or the Mac password (Mac scope), Demo data included, and is
// written to the activity log. Turning look-only on asks nothing; turning it off asks. In
// look-only mode only that switch stays usable here: the exports write into Downloads, which
// look-only holds (admin.rs LOOK_ONLY_HOLDS).
import { useEffect, useState } from "react";
import { CacheGrant } from "./CacheGrant";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FormGroup, FormRow, HelpTip, Notice, Segmented, Switch } from "../../components/ui";
import { ADMIN_REFUSED, LOOK_ONLY_TEXT, lockAdmin, requestAdmin, useAdminState, withAdmin } from "../../lib/admin";
import { api, inTauri, invoke } from "../../lib/api";
import { fmtCountdown, fmtRelative } from "../../lib/format";
import { showError } from "../../lib/errors";
import { opsApi, type AdminState, type SecuritySettings } from "../../lib/opsApi";
import { toast } from "../../lib/toasts";
import "../ops.css";

const DEFAULTS: SecuritySettings = { idleMinutes: 15, retentionDays: 90, shellIdleMinutes: 60 };

/** What turning look-only on did to running tasks (admin.rs HeldTasks). */
interface HeldTasks {
  stopped: number;
  stopping: number;
  /** What they started runs on the node by itself (an upgrade, a load's containers): it keeps running. */
  onNode?: number;
}

/** What look_only_set answers: the state, and what turning it on did to running tasks (admin.rs LookOnlyView). */
type LookOnlyView = AdminState & { held?: HeldTasks };

/** The toast's body when look-only comes on: what it means, the tasks it stopped, and the work it can't. */
export function lookOnlyOnBody(held?: HeldTasks): string {
  const n = (k: number) => (k === 1 ? "1 task" : `${k} tasks`);
  const its = (k: number) => (k === 1 ? "its" : "their");
  const parts: string[] = [];
  if (held?.stopped) parts.push(`${n(held.stopped)} stopped before ${its(held.stopped)} next step.`);
  if (held?.stopping) parts.push(`${n(held.stopping)} can't be stopped part-way; look-only refuses ${its(held.stopping)} next change.`);
  if (held?.onNode) {
    const k = held.onNode;
    parts.push(`${n(k)} started work that keeps running on ${its(k)} node${k === 1 ? "" : "s"} (an upgrade, a model loading); SparkBox can't stop it in look-only.`);
  }
  return [LOOK_ONLY_TEXT, ...parts].join(" ");
}

interface TunnelView {
  node: string;
  port: number;
  page: string;
  localPort: number;
  openedMs: number;
  lastUsedMs: number;
  active: number;
}

/** The SSH tunnels to node pages open now (the DGX Dashboard), each with Close. */
function Tunnels() {
  const [list, setList] = useState<TunnelView[]>([]);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!inTauri) return;
    let alive = true;
    const load = () => invoke<TunnelView[]>("tunnels_list").then((l) => alive && (setList(l), setNow(Date.now())), () => {});
    void load();
    const t = window.setInterval(load, 2000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);
  const close = async (t: TunnelView) => {
    await api.tunnelClose(t.node, t.port).catch((e) => showError(e, { action: `Close the ${t.page} tunnel to ${t.node}` }));
    setList((l) => l.filter((x) => !(x.node === t.node && x.port === t.port)));
  };
  return (
    <FormGroup
      title={<>SSH tunnels <HelpTip topic="start.security.tunnels" label="About SSH tunnels" /></>}
    >
      {list.length === 0 ? (
        <FormRow label="Open now">
          <span className="muted ops-sec-text">None</span>
        </FormRow>
      ) : (
        list.map((t) => (
          <FormRow key={`${t.node}:${t.port}`} label={`${t.page} · ${t.node}`} hint={`127.0.0.1:${t.localPort} · opened ${fmtRelative(t.openedMs, now)}${t.active ? ` · ${t.active} connection${t.active === 1 ? "" : "s"} open` : ` · last used ${fmtRelative(t.lastUsedMs, now)}`}`}>
            <button type="button" className="btn small" onClick={() => void close(t)}>Close</button>
          </FormRow>
        ))
      )}
    </FormGroup>
  );
}

function useCountdown(until: number | null): string | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    if (!until) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [until]);
  if (!until) return null;
  return fmtCountdown((until - now) / 1000);
}

export function SecuritySection() {
  const admin = useAdminState();
  const [settings, setSettings] = useState<SecuritySettings | null>(inTauri ? null : DEFAULTS);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const left = useCountdown(admin.unlocked ? admin.expiresAtMs : null);
  const macLeft = useCountdown(admin.authenticated ? admin.macExpiresAtMs ?? null : null);

  useEffect(() => {
    if (inTauri) opsApi.securityGet().then(setSettings, (e) => setError(String(e)));
  }, []);

  const save = async (next: SecuritySettings) => {
    const before = settings;
    setSettings(next);
    if (!inTauri) return;
    setSaving(true);
    try {
      setSettings(await withAdmin(() => opsApi.securitySet(next), { scope: "mac", action: "security_set" }));
      setError(null);
    } catch (e) {
      setSettings(before);
      if (e !== ADMIN_REFUSED) setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setSaving(false);
    }
  };

  // Look-only: on is immediate (it only takes rights away); off asks for Touch ID in the backend.
  const [lookBusy, setLookBusy] = useState(false);
  const setLookOnly = async (on: boolean) => {
    if (lookBusy) return;
    setLookBusy(true);
    try {
      const v = (await opsApi.lookOnlySet(on)) as LookOnlyView;
      toast({ kind: "info", title: on ? "Look-only mode is on" : "Look-only mode is off", body: on ? lookOnlyOnBody(v?.held) : "Admin mode can be unlocked again." });
    } catch (e) {
      if ((e as { kind?: string })?.kind !== "cancelled") showError(e, { action: on ? "Turn on look-only mode" : "Turn off look-only mode" });
    } finally {
      setLookBusy(false);
    }
  };

  const exportLog = async (format: "csv" | "json") => {
    try {
      const path = await opsApi.activityExport(format);
      toast({ kind: "success", title: "Activity log exported", body: path.split("/").pop(), actions: [{ label: "Show in Finder", run: () => void revealItemInDir(path) }] });
    } catch (e) {
      showError(e, { action: "Export the activity log" });
    }
  };

  return (
    <>
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      <FormGroup
        title={<>Look only <HelpTip topic="start.security.look-only" label="About look-only mode" /></>}
      >
        <FormRow label="Look-only mode" hint={admin.lookOnlyForced ? "Set by SPARKBOX_LOOK_ONLY for this run" : admin.lookOnly ? "Turning it off asks for Touch ID or your Mac password" : "SparkBox refuses changes until you turn it off"}>
          <Switch checked={!!admin.lookOnly} onChange={(on) => void setLookOnly(on)} label="Look-only mode" disabled={lookBusy || !!admin.lookOnlyForced || !inTauri} />
        </FormRow>
      </FormGroup>
      <fieldset className="settings-fieldset" disabled={!!admin.lookOnly || saving}>
      <FormGroup
        title={<>Admin mode <HelpTip topic="start.security.admin" label="About Admin mode" /></>}
      >
        <FormRow label="Right now" hint={admin.unlocked && left ? `Locks in ${left} unless you make a change` : undefined}>
          <span className="ops-sec-state">
            <span className={admin.unlocked ? "badge warn" : "badge neutral"}>{admin.lookOnly ? "Look only" : admin.unlocked ? (admin.authenticated ? "Unlocked · confirmed it's you" : "Unlocked · nodes only") : "Locked"}</span>
            {admin.unlocked
              ? <button type="button" className="btn small" onClick={() => void lockAdmin()}>Lock now</button>
              : !admin.lookOnly && <button type="button" className="btn small" onClick={() => void requestAdmin()}>Unlock…</button>}
          </span>
        </FormRow>
        <FormRow label={<>Lock after <HelpTip topic="start.security.lock-after" /></>} hint="Without a change to the cluster">
          <select
            aria-label="Lock after"
            value={String(settings?.idleMinutes ?? 15)}
            disabled={!settings}
            onChange={(e) => settings && void save({ ...settings, idleMinutes: Number(e.target.value) as SecuritySettings["idleMinutes"] })}
          >
            <option value="5">5 minutes</option>
            <option value="15">15 minutes</option>
            <option value="60">1 hour</option>
            <option value="120">2 hours</option>
            <option value="240">4 hours</option>
            <option value="480">8 hours</option>
            <option value="1440">24 hours</option>
          </select>
        </FormRow>
        <FormRow label={<>Changes on this Mac <HelpTip topic="start.security.mac-changes" /></>} hint={admin.authenticated && admin.macExpiresAtMs ? `Confirmed: ${macLeft ?? ""} left` : "Touch ID or your Mac password for 5 minutes"}>
          <span className="muted ops-sec-text">
            {admin.authenticated ? "Allowed until the 5 minutes since Touch ID are up; nothing extends them." : "Each confirmation allows them for 5 minutes. Keychain changes ask every time."}
          </span>
        </FormRow>
        <FormRow label={<>Asking who you are <HelpTip topic="start.security.asking" /></>} hint={admin.requiresAuth ? "Live data: every unlock" : "Demo data: for changes on this Mac"}>
          <span className="muted ops-sec-text">
            {admin.requiresAuth
              ? "Unlocking asks for Touch ID or your Mac password."
              : "Demo unlocks for the simulated nodes at once; changes on this Mac still ask for Touch ID or your Mac password."}
          </span>
        </FormRow>
      </FormGroup>
      <Tunnels />
      <CacheGrant />
      <FormGroup
        title={<>Activity log <HelpTip topic="start.security.activity-log" label="About the activity log" /></>}
      >
        <FormRow label={<>Keep entries for <HelpTip topic="start.security.retention" /></>}>
          <Segmented<string>
            label="Keep entries for"
            value={String(settings?.retentionDays ?? 90)}
            onChange={(v) => settings && void save({ ...settings, retentionDays: Number(v) as SecuritySettings["retentionDays"] })}
            options={[{ value: "30", label: "30 days" }, { value: "90", label: "90 days" }, { value: "365", label: "1 year" }]}
          />
        </FormRow>
        <FormRow label="Export" hint="Saved to Downloads">
          <span className="ops-sec-state">
            <button type="button" className="btn small" disabled={!inTauri} onClick={() => void exportLog("csv")}>CSV…</button>
            <button type="button" className="btn small" disabled={!inTauri} onClick={() => void exportLog("json")}>JSON…</button>
          </span>
        </FormRow>
      </FormGroup>
      <FormGroup
        title={<>Diagnostics <HelpTip topic="start.security.diagnostics" label="About diagnostics" /></>}
      >
        <FormRow label="Export diagnostics" hint="Saved to Downloads">
          <button type="button" className="btn small" disabled={!inTauri} onClick={async () => {
            try {
              const path = await invoke<string>("diagnostics_export");
              toast({ kind: "success", title: "Diagnostics exported", body: path.split("/").pop(), actions: [{ label: "Show in Finder", run: () => void revealItemInDir(path) }] });
            } catch (e) {
              showError(e, { action: "Export diagnostics" });
            }
          }}>Export…</button>
        </FormRow>
      </FormGroup>
      </fieldset>
    </>
  );
}
