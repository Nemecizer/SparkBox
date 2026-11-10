import { HardwareProfiles } from "./HardwareProfiles";
import { useEffect, useState } from "react";
import { Lock } from "lucide-react";
import { Icon } from "../../components/Icon";
import { DotGlyph, FormGroup, FormRow, HelpTip, Notice } from "../../components/ui";
import { ADMIN_REFUSED, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, requestAdmin, unlockedFor, useAdminState, withAdmin } from "../../lib/admin";
import { routerOsAccount } from "../../lib/api";
import { firewallaApi, MSP_TOKEN_ACCOUNT, type MspTestResult } from "../../lib/firewalla";
import { certToTrust, netApi, type SwitchTestResult } from "../../lib/netApi";
import type { SwitchConfig, SwitchKind } from "../../lib/types";
import { KeychainControl, useCredentialStatus } from "./LoginSection";
import { InventorySaveBar, useInventoryDraft } from "./NodesSection";

const KIND_LABEL: Record<SwitchKind, string> = {
  routerOs: "MikroTik RouterOS switch",
  firewalla: "Firewalla switch",
  gateway: "Gateway",
};

const KIND_NOTE: Record<SwitchKind, string> = {
  routerOs: "SparkBox reads ports, optics and counters from the RouterOS REST API. On the CRS804, enable the www-ssl service and create a user for SparkBox: the read and rest-api policies are enough to watch it, and port actions also need write and test. RouterOS 7 serves REST at https://<switch>/rest.",
  firewalla: "SparkBox checks that the switch answers on TCP 443, then 80, and reads its ports from LLDP, your labels and, if you set it up below, Firewalla's cloud. Firewalla switches have no local API for port counters.",
  gateway: "The router and VPN endpoint. SparkBox checks that it answers on TCP 443, then 80, and times the reply. Leave the address empty if you don't want it monitored.",
};

type TestState = { busy: boolean; result?: SwitchTestResult; error?: string };

/**
 * "Test connection": reachability, and for RouterOS the REST login, board and version. Uses the
 * saved settings. A certificate it doesn't know yet, or a changed one, offers Trust…, which pins it and tests again.
 */
function SwitchTest({ sw, dirty }: { sw: SwitchConfig; dirty: boolean }) {
  const [t, setT] = useState<TestState | null>(null);
  const adminNow = useAdminState();
  const run = async () => {
    setT({ busy: true });
    try {
      setT({ busy: false, result: await netApi.switchTest(sw.id) });
    } catch (e) {
      setT({ busy: false, error: String(e) });
    }
  };
  const trust = async (fingerprint: string) => {
    if (!(await requestAdmin("switch_trust", "mac"))) return;
    setT({ busy: true });
    try {
      setT({ busy: false, result: await netApi.switchTrust(sw.id, fingerprint) });
    } catch (e) {
      setT({ busy: false, error: String(e) });
    }
  };
  const r = t?.result;
  const cert = r ? certToTrust(r) : null;
  const lock = lockOf(!unlockedFor("mac", adminNow), adminNow);
  return (
    <span className="switch-test">
      {t && !t.busy && (
        <span className={`switch-test-result${r?.ok ? "" : " error-text"}`} role="status">
          <DotGlyph status={r?.ok ? "online" : "error"} word={r?.ok ? "Connected" : "Failed"} />
          {r ? (r.ok ? [r.board, r.version && `RouterOS ${r.version}`].filter(Boolean).join(" · ") || r.message : r.message) : t.error}
        </span>
      )}
      {cert && (
        <button type="button" className="btn" disabled={t?.busy || lock === "lookOnly"} data-lock={lock ?? undefined}
          title={lock === "lookOnly" ? LOOK_ONLY_TIP : lock ? `Trust this certificate · ${NEEDS_ADMIN_TIP}` : "Trust this certificate"}
          aria-label={lock === "lookOnly" ? "Trust… — look-only" : lock ? "Trust… — needs Admin" : undefined}
          onClick={() => void trust(cert)}>
          Trust…{lock && <Icon icon={Lock} size={12} className="tool-lock" />}
        </button>
      )}
      <button type="button" className="btn" onClick={run} disabled={t?.busy || !sw.host.trim()} title={dirty ? "Tests the saved settings; save first to test your edits" : undefined}>
        {t?.busy ? "Testing…" : "Test connection"}
      </button>
    </span>
  );
}

/** Firewalla's cloud (MSP) API, optional: the Switch X's real link speeds and counters. */
function MspGroup() {
  const creds = useCredentialStatus([MSP_TOKEN_ACCOUNT]);
  const [saved, setSaved] = useState("");
  const [domain, setDomain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<{ busy: boolean; result?: MspTestResult; error?: string } | null>(null);
  useEffect(() => {
    firewallaApi.mspSettings().then((s) => {
      setSaved(s.domain ?? "");
      setDomain(s.domain ?? "");
    }, (e) => setError(String(e)));
  }, []);
  const save = async () => {
    setError(null);
    try {
      // Where the saved token goes, so it confirms every time, as the token does.
      const s = await withAdmin(() => firewallaApi.mspSetDomain(domain), { scope: "mac", action: "firewalla_msp_set_domain", fresh: true });
      setSaved(s.domain ?? "");
      setDomain(s.domain ?? "");
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(String((e as { message?: string })?.message ?? e));
    }
  };
  const run = async () => {
    setTest({ busy: true });
    try {
      setTest({ busy: false, result: await firewallaApi.mspTest() });
    } catch (e) {
      setTest({ busy: false, error: String((e as { message?: string })?.message ?? e) });
    }
  };
  const r = test?.result;
  return (
    <FormGroup
      title="Firewalla cloud (MSP)"
      footnote="Optional. With an MSP login, SparkBox reads the Switch X's real link speeds and counters from Firewalla's cloud, at most every 15 minutes, and only with Live data. It only reads. Firewalla doesn't document these calls. A community project uses them on the Switch SE, and they aren't confirmed on the Switch X."
    >
      {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
      {creds.error && <Notice tone="error" onRetry={creds.refresh}>{creds.error}</Notice>}
      <FormRow label={<>MSP domain <HelpTip topic="start.switches.msp-domain" /></>} htmlFor="msp-domain" hint={saved ? undefined : "Empty means SparkBox doesn't use the cloud"}>
        <span className="keychain">
          <input id="msp-domain" value={domain} placeholder="dn-abc123.firewalla.net" spellCheck={false} autoComplete="off" onChange={(e) => setDomain(e.target.value)} onKeyDown={(e) => e.key === "Enter" && domain !== saved && save()} />
          <button type="button" className="btn primary" disabled={domain.trim() === saved} onClick={save}>Save</button>
        </span>
      </FormRow>
      <FormRow label={<>API token <HelpTip topic="start.switches.msp-token" /></>} htmlFor="msp-token">
        <KeychainControl account={MSP_TOKEN_ACCOUNT} id="msp-token" label="Firewalla MSP token" saved={creds.status[MSP_TOKEN_ACCOUNT]} onChange={creds.refresh} />
      </FormRow>
      <FormRow label={<>Connection <HelpTip topic="start.switches.msp-test" /></>} hint={saved ? undefined : "Save a domain first"}>
        <span className="switch-test">
          {test && !test.busy && (
            <span className={`switch-test-result${r?.ok ? "" : " error-text"}`} role="status">
              <DotGlyph status={r?.ok ? "online" : "error"} word={r?.ok ? "Connected" : "Failed"} />
              {r ? r.message : test.error}
            </span>
          )}
          <button type="button" className="btn" onClick={run} disabled={test?.busy || !saved}>{test?.busy ? "Testing…" : "Test connection"}</button>
        </span>
      </FormRow>
    </FormGroup>
  );
}

export function SwitchesSection() {
  const d = useInventoryDraft();
  const switches = d.draft?.switches ?? [];
  const creds = useCredentialStatus(switches.filter((s) => s.kind === "routerOs").map((s) => routerOsAccount(s.id)));
  if (!d.draft) {
    return d.error ? <Notice tone="error" onRetry={d.reload}>{d.error}</Notice> : <p className="muted">Loading the inventory…</p>;
  }
  const edit = (i: number, patch: Partial<SwitchConfig>) =>
    d.edit((x) => ({ ...x, switches: x.switches.map((s, j) => (j === i ? { ...s, ...patch } : s)) }));

  return (
    <>
      {d.error && <Notice tone="error" onRetry={d.reload}>{d.error}</Notice>}
      {creds.error && <Notice tone="error" onRetry={creds.refresh}>{creds.error}</Notice>}
      {switches.map((s, i) => {
        const hostErr = d.errors[`switch:${i}:host`];
        const base = `sw-${s.id}`;
        return (
          <FormGroup key={s.id} title={s.name} footnote={KIND_NOTE[s.kind] ?? undefined}>
            <FormRow label="Kind"><span className="text-2">{KIND_LABEL[s.kind] ?? s.kind}</span></FormRow>
            <FormRow label={<>Address{i === 0 && <> <HelpTip topic="start.switches.address" /></>}</>} htmlFor={`${base}-host`} hint={hostErr ?? (s.host ? undefined : "Empty means not monitored")}>
              <input
                id={`${base}-host`}
                value={s.host}
                placeholder="IP address or hostname"
                spellCheck={false}
                autoComplete="off"
                aria-invalid={hostErr ? true : undefined}
                onChange={(e) => edit(i, { host: e.target.value })}
              />
            </FormRow>
            {s.kind === "routerOs" && (
              <>
                <FormRow label={<>REST user <HelpTip topic="start.switches.rest-user" /></>} htmlFor={`${base}-user`}>
                  <input
                    id={`${base}-user`}
                    value={s.user ?? ""}
                    placeholder="sparkbox"
                    spellCheck={false}
                    autoComplete="off"
                    onChange={(e) => edit(i, { user: e.target.value || null })}
                  />
                </FormRow>
                <FormRow label={<>REST password <HelpTip topic="start.switches.rest-password" /></>} htmlFor={`${base}-pass`}>
                  <KeychainControl
                    account={routerOsAccount(s.id)}
                    id={`${base}-pass`}
                    label={`${s.name} REST password`}
                    saved={creds.status[routerOsAccount(s.id)]}
                    onChange={creds.refresh}
                  />
                </FormRow>
              </>
            )}
            <FormRow label={<>Connection{i === 0 && <> <HelpTip topic="start.switches.test" /></>}</>} hint={s.host.trim() ? undefined : "Set an address first"}>
              <SwitchTest sw={d.saved?.switches.find((x) => x.id === s.id) ?? s} dirty={d.dirty} />
            </FormRow>
          </FormGroup>
        );
      })}
      <HardwareProfiles />
      <MspGroup />
      <InventorySaveBar />
    </>
  );
}
