import { useEffect, useState } from "react";
import { FormGroup, Notice } from "../../components/ui";
import { inTauri } from "../../lib/api";
import { withAdmin } from "../../lib/admin";
import { opsApi, type CacheGrant as Grant } from "../../lib/opsApi";
import { confirmAction } from "../../lib/session";
import { useCluster } from "../../lib/store";

export function CacheGrant() {
  const cluster = useCluster();
  const [grant, setGrant] = useState<Grant | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (inTauri) opsApi.cacheGrantGet().then((g) => setGrant(g ?? null), (e) => setError(String(e))); }, []);
  const change = async () => {
    const enabled = !grant;
    if (enabled && !await confirmAction({ title: "Allow conditional page-cache drops for Keep running?", body: "This app may run sync and write 3 to /proc/sys/vm/drop_caches with sudo on the current inventory nodes when a saved vLLM load needs it to restart. This permission survives idle Admin locking and app restarts. Look-only, explicit lock/sleep holds, node identity, ownership, controller leases and debug fences still apply. It grants no shell or other command. Revoke it here at any time.", confirmLabel: "Grant with Touch ID…" })) return;
    setBusy(true); setError("");
    try { setGrant(await withAdmin(() => opsApi.cacheGrantSet(enabled), { scope: "mac", action: "drop_cache_grant_set", fresh: true })); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <FormGroup title="Keep running: page-cache permission">
    <p>{grant ? `Granted for ${grant.nodes.map((id) => cluster?.nodes.find((n) => n.id === id)?.name ?? id).join(", ")}. New inventory nodes need a new grant.` : "Off by default. Without this grant, automatic recovery that needs a page-cache drop waits for Admin to be unlocked and the load's existing consent."}</p>
    <p className="muted">Only a fresh memory preflight can request a drop. Each use is audited. Debug and T2 operations still need their separately signed drop-caches fence; this permission opens no fence.</p>
    {error && <Notice tone="error">{error}</Notice>}
    <button className="btn" disabled={busy || !cluster || cluster.demo || !inTauri} onClick={() => void change()}>{busy ? "Saving…" : grant ? "Revoke with Touch ID…" : "Grant with Touch ID…"}</button>
    {cluster?.demo && <p className="muted">Demo cannot grant permission for the live cluster.</p>}
  </FormGroup>;
}
