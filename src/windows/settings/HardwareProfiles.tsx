import { useState } from "react";
import { FormGroup, FormRow, HelpTip, Notice, Switch } from "../../components/ui";
import { api } from "../../lib/api";
import { ADMIN_REFUSED, withAdmin } from "../../lib/admin";
import { useHardwareProfiles } from "../../lib/hardware";
import { setPrefs, usePrefs } from "./GeneralSection";

export function HardwareProfiles() {
  const { prefs } = usePrefs();
  const { view, error } = useHardwareProfiles(prefs?.dataSource ?? "demo");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const change = async (on: boolean) => {
    setBusy(true); setFailure(null);
    try { setPrefs(await withAdmin(() => api.prefsPatch({ hardwareAutoLookup: on }), { scope: "mac", action: "prefs_set" })); }
    catch (e) { if (e !== ADMIN_REFUSED) setFailure(String(e)); }
    finally { setBusy(false); }
  };
  return <FormGroup title="Equipment profiles" footnote="Uses the provider and model selected in Settings › Models › AI feasibility check. New equipment is recognized from a reported board or system model after it is added to the inventory and monitored. Unknown models stay unresolved; no network scan is added.">
    {(failure || error) && <Notice tone="error">{failure || error}</Notice>}
    <FormRow label={<>Automatic AI lookup <HelpTip topic="start.switches.hardware" /></>} htmlFor="hardware-lookup">
      <Switch id="hardware-lookup" label="Automatic AI hardware lookup" checked={!!prefs?.hardwareAutoLookup} disabled={busy || !prefs} onChange={(on) => void change(on)} />
    </FormRow>
    <div className="hardware-details">
    <p className="muted">Off by default. With Live data, sends the reported equipment model to Anthropic or OpenAI and reads a supported manufacturer's public page. Up to two paid AI requests per new model; successful profiles refresh after 30 days, failures retry after 24 hours. Demo and look-only runs make no lookup requests.</p>
    <p className="muted">Profiles describe hardware capacity. Observed speeds, cabling and inventory expectations stay authoritative. AI profiles are labeled and linked to their source.</p>
    {view?.devices.map((d) => <div key={d.id} className="hardware-profile">
      <strong>{d.name}</strong> · {d.identity}
      {d.entry?.profile ? <>
        <p>{d.entry.profile.summary}</p>
        <p>{d.entry.profile.groups.map((g) => `${g.count} × ${g.connector}, up to ${g.maxGbps} Gb/s`).join(" · ")}</p>
        <a href={d.entry.profile.sourceUrl} target="_blank" rel="noreferrer">Manufacturer specification</a>
        <span className="muted"> · {d.builtin ? "Bundled profile" : `AI profile · ${d.entry.provider}/${d.entry.model} · ${new Date(d.entry.checkedAtMs).toLocaleDateString()}`}</span>
      </> : <p className="muted">{d.entry?.error ?? (view.demo ? "Demo: no AI lookup" : view.enabled ? "Waiting for model lookup" : "Automatic lookup is off")}</p>}
    </div>)}
    </div>
  </FormGroup>;
}
