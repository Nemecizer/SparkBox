import { useEffect, useState } from "react";
import { Check, CircleAlert, CircleDashed, CircleX, Loader, Plus, Trash2 } from "lucide-react";
import { Badge, FormGroup, FormRow, HelpTip, Notice, Spacer } from "../components/ui";
import { ADMIN_REFUSED, requestAdmin, withAdmin } from "../lib/admin";
import { api, routerOsAccount, SSH_ACCOUNT } from "../lib/api";
import { certToTrust, netApi, type CheckStep, type SwitchTestResult } from "../lib/netApi";
import { sharingKey, testSummary, trustKey } from "../lib/nodeStatus";
import { useCluster } from "../lib/store";
import type { Inventory, NodeConfig, Prefs } from "../lib/types";
import { protocolName, useVpnStatus, VPN_LABEL } from "../lib/vpn";
import { useWindowTitle, useWM } from "../wm/WindowManager";
import { NetworkDiscovery } from "./NetworkDiscovery";
import "./net.css";

const STEPS = [
  { id: "account", label: "Account" },
  { id: "nodes", label: "Nodes" },
  { id: "check", label: "Checks" },
  { id: "vpn", label: "VPN" },
  { id: "switches", label: "Switches" },
  { id: "done", label: "Done" },
] as const;
type StepId = (typeof STEPS)[number]["id"];

const STATUS_ICON = { ok: Check, warn: CircleAlert, fail: CircleX, skip: CircleDashed } as const;
const STATUS_WORD = { ok: "Passed", warn: "Needs a look", fail: "Failed", skip: "Not checked" } as const;

function describe(e: unknown): string {
  if (e === ADMIN_REFUSED) return "Admin mode stays locked, so nothing was saved.";
  return typeof e === "string" ? e : (e as { message?: string })?.message ?? String(e);
}

/** First run (and re-runs): who logs in, which nodes, are they reachable and trusted, the VPN, the switches, then Live. */
export function ConnectWizard() {
  const wm = useWM();
  const vpn = useVpnStatus();
  const [step, setStep] = useState<StepId>("account");
  const [inv, setInv] = useState<Inventory | null>(null);
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [saved, setSaved] = useState<Record<string, boolean>>({});
  const [checks, setChecks] = useState<Record<string, CheckStep[] | "running">>({});
  const cluster = useCluster();
  const [tests, setTests] = useState<Record<string, SwitchTestResult | "running">>({});
  const [restPassword, setRestPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useWindowTitle(`Connect my Sparks — ${STEPS.find((s) => s.id === step)?.label}`);

  const refreshCreds = (i: Inventory) =>
    api.credentialStatus([SSH_ACCOUNT, ...i.switches.filter((s) => s.kind === "routerOs").map((s) => routerOsAccount(s.id))]).then(setSaved, () => {});

  useEffect(() => {
    api.getInventory().then((i) => {
      setInv(i);
      setUser(i.nodes[0]?.user ?? "");
      refreshCreds(i);
    }, (e) => setError(describe(e)));
    api.prefsGet().then(setPrefs, () => {});
  }, []);

  if (!inv) return <div className="pad muted">{error ?? "Loading…"}</div>;

  const at = STEPS.findIndex((s) => s.id === step);
  const go = (d: 1 | -1) => setStep(STEPS[Math.min(STEPS.length - 1, Math.max(0, at + d))].id);
  const editNode = (i: number, patch: Partial<NodeConfig>) => setInv({ ...inv, nodes: inv.nodes.map((n, j) => (j === i ? { ...n, ...patch } : n)) });
  const live = prefs?.dataSource === "live";

  const savePassword = async () => {
    if (!password) return;
    try {
      // A Keychain write: Touch ID or the password for this change itself.
      await withAdmin(() => api.setCredential(SSH_ACCOUNT, password), { scope: "mac", action: "set_credential" });
      setPassword("");
      await refreshCreds(inv);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(describe(e));
    }
  };

  const saveInventory = async (next = inv) => {
    const withUser = { ...next, nodes: next.nodes.map((n) => ({ ...n, user: user.trim() || n.user, id: n.id || n.name, name: n.name || n.id })) };
    // The inventory is Mac-side state: Touch ID or the password, asked once per unlock.
    await withAdmin(() => api.saveInventory(withUser), { scope: "mac", action: "save_inventory" });
    setInv(withUser);
    return withUser;
  };

  const runChecks = async () => {
    setBusy(true);
    setError(null);
    try {
      // The checks log in with this Mac's keys and the saved password: Mac scope, asked once.
      if (!(await requestAdmin("node_probe", "mac"))) return;
      const saved = await saveInventory();
      setChecks(Object.fromEntries(saved.nodes.map((n) => [n.id, "running" as const])));
      await Promise.all(
        saved.nodes.map(async (n) => {
          try {
            const steps = await netApi.nodeProbe(n);
            setChecks((c) => ({ ...c, [n.id]: steps }));
          } catch (e) {
            setChecks((c) => ({ ...c, [n.id]: [{ id: "tcp", label: "Reach SSH", status: "fail", detail: describe(e), hint: null }] }));
          }
        }),
      );
    } catch (e) {
      setError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  const trust = async (node: NodeConfig, s: CheckStep) => {
    if (!s.fingerprint || !s.address) return;
    // The sheet shows the key, how to compare it, and which nodes present the same one.
    const sharedWith = (inv?.nodes ?? []).filter((n) => n.id !== node.id && (checks[n.id] as CheckStep[] | undefined)?.some?.((x) => x.fingerprint === s.fingerprint)).map((n) => n.name);
    const known = sharingKey(cluster?.nodes ?? [], s.fingerprint, node.name);
    if (!(await trustKey({ name: node.name, address: s.address, fingerprint: s.fingerprint, sharedWith: [...new Set([...known, ...sharedWith])], changed: s.status === "fail" }))) return;
    try {
      setChecks((c) => ({ ...c, [node.id]: "running" }));
      const steps = await netApi.nodeProbe(node);
      setChecks((c) => ({ ...c, [node.id]: steps }));
    } catch (e) {
      setError(describe(e));
    }
  };

  const testSwitch = async (id: string) => {
    setTests((t) => ({ ...t, [id]: "running" }));
    try {
      await saveInventory();
      const r = await netApi.switchTest(id);
      setTests((t) => ({ ...t, [id]: r }));
    } catch (e) {
      setTests((t) => ({ ...t, [id]: { ok: false, board: null, version: null, fingerprint: null, errorKind: "other", message: describe(e) } }));
    }
  };

  const trustSwitch = async (id: string, fp: string) => {
    if (!(await requestAdmin("switch_trust", "mac"))) return;
    try {
      setTests((t) => ({ ...t, [id]: "running" }));
      const r = await netApi.switchTrust(id, fp);
      setTests((t) => ({ ...t, [id]: r }));
    } catch (e) {
      setError(describe(e));
    }
  };

  const saveRestPassword = async (id: string) => {
    if (!restPassword) return;
    try {
      await withAdmin(() => api.setCredential(routerOsAccount(id), restPassword), { scope: "mac", action: "set_credential" });
      setRestPassword("");
      await refreshCreds(inv);
    } catch (e) {
      if (e !== ADMIN_REFUSED) setError(describe(e));
    }
  };

  const finish = async (toLive: boolean) => {
    setBusy(true);
    try {
      await saveInventory();
      if (prefs && toLive !== live) setPrefs(await withAdmin(() => api.dataSourceSet(toLive ? "live" : "demo"), { scope: "mac", action: "data_source_set" }));
      wm.open("dashboard");
    } catch (e) {
      setError(describe(e));
    } finally {
      setBusy(false);
    }
  };

  const allChecks = Object.values(checks).filter((c): c is CheckStep[] => c !== "running");
  const failing = allChecks.flat().filter((s) => s.status === "fail").length;
  const warnings = allChecks.flat().filter((s) => s.status === "warn").length;

  return (
    <div className="wizard">
      <nav className="wizard-rail" aria-label="Steps">
        <ol>
          {STEPS.map((s, i) => (
            <li key={s.id}>
              <button type="button" className={`wizard-step${s.id === step ? " on" : ""}${i < at ? " done" : ""}`} aria-current={s.id === step ? "step" : undefined} onClick={() => setStep(s.id)}>
                <span className="wizard-num" aria-hidden>{i < at ? <Check size={11} /> : i + 1}</span>
                {s.label}
              </button>
            </li>
          ))}
        </ol>
      </nav>
      <div className="wizard-main">
        <div className="wizard-body">
          {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}

          {step === "account" && (
            <>
              <h2 className="wizard-title">The account SparkBox logs in with <HelpTip topic="start.connect.account" /></h2>
              <p className="wizard-lede">One user and password for every Spark, as NVIDIA's setup creates them. SparkBox logs in with your SSH keys first (ssh-agent, 1Password, Secretive or a key file, set in Settings › Cluster login) and this password last. sudo always uses the password. SparkBox runs sudo when you make a change, read a protected file with sudo, or run the Doctor.</p>
              <FormGroup title={<span style={{ display: "inline-flex", alignItems: "center", gap: "var(--sp-2, 4px)" }}>Cluster account<HelpTip topic="start.connect.password" label="Where the password goes" /></span>}>
                <FormRow label="User" htmlFor="wiz-user">
                  <input id="wiz-user" value={user} onChange={(e) => setUser(e.target.value)} autoComplete="username" />
                </FormRow>
                <FormRow label="Password" htmlFor="wiz-pw" hint={saved[SSH_ACCOUNT] ? "Saved in the Keychain. Type a new one to replace it." : undefined}>
                  <div className="wizard-inline">
                    <input id="wiz-pw" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" placeholder={saved[SSH_ACCOUNT] ? "••••••••" : ""} />
                    <button className="btn" disabled={!password} onClick={savePassword}>Save to Keychain</button>
                    {saved[SSH_ACCOUNT] && <Badge tone="good">Saved</Badge>}
                  </div>
                </FormRow>
              </FormGroup>
            </>
          )}

          {step === "nodes" && (
            <>
              <h2 className="wizard-title">Your Sparks <HelpTip topic="start.connect.nodes" /></h2>
              <p className="wizard-lede">Find Sparks on your local network, or enter their LAN addresses. .local names work at home, but not through the VPN.</p>
              <NetworkDiscovery inventory={inv} user={user} onUse={setInv} />
              <table className="grid form-grid wizard-nodes">
                <thead>
                  <tr><th>Name</th><th>Address (IP) <HelpTip topic="start.nodes.ip-fallback" /></th><th>Hostname <HelpTip topic="start.nodes.hostname" /></th><th>SSH port <HelpTip topic="start.nodes.ssh-port" /></th><th /></tr>
                </thead>
                <tbody>
                  {inv.nodes.map((n, i) => (
                    <tr key={i}>
                      <td><input value={n.name} aria-label="Name" onChange={(e) => editNode(i, { name: e.target.value, id: n.id || e.target.value })} /></td>
                      <td><input className="mono" value={n.ip ?? ""} aria-label="Address" placeholder="192.168.50.x" onChange={(e) => editNode(i, { ip: e.target.value || null })} /></td>
                      <td><input className="mono" value={n.host} aria-label="Hostname" onChange={(e) => editNode(i, { host: e.target.value })} /></td>
                      <td><input className="narrow num" type="number" value={n.port} aria-label="SSH port" onChange={(e) => editNode(i, { port: Number(e.target.value) || 22 })} /></td>
                      <td>
                        <button className="tool" aria-label={`Remove ${n.name}`} title="Remove" onClick={() => setInv({ ...inv, nodes: inv.nodes.filter((_, j) => j !== i) })}>
                          <Trash2 size={13} />
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <div className="form-row">
                <button className="btn" onClick={() => setInv({ ...inv, nodes: [...inv.nodes, { id: "", name: "", host: "", ip: null, model: "DGX Spark", port: 22, user }] })}>
                  <Plus size={12} /> Add node
                </button>
              </div>
            </>
          )}

          {step === "check" && (
            <>
              <h2 className="wizard-title">Checking each node <HelpTip topic="check-your-nodes" /></h2>
              <p className="wizard-lede">
                SparkBox connects to every node and checks what it needs: SSH, the host key, the login and how it logged in, sudo, Python, Docker, the GPU driver, the fabric addresses and whether it can be woken remotely.
                {prefs?.dataSource === "demo" && " With Demo data these answers come from the simulator."}
              </p>
              <div className="form-row">
                <button className={`btn${Object.keys(checks).length ? "" : " primary"}`} onClick={runChecks} disabled={busy}>{Object.keys(checks).length ? "Check again" : "Check all nodes"}</button>
                {allChecks.length > 0 && (
                  <span className="muted">{allChecks.map((c) => testSummary(c)).find((x) => x.tone !== "ok")?.text ?? `${failing ? `${failing} failed` : "Nothing failed"}${warnings ? ` · ${warnings} to look at` : ""}`}</span>
                )}
              </div>
              <div className="wizard-checks">
                {inv.nodes.map((n) => {
                  const c = checks[n.id];
                  return (
                    <section key={n.id} className="wizard-node" aria-label={`${n.name} checks`}>
                      <header>
                        <span className="strong">{n.name}</span>
                        <span className="mono muted">{n.ip ?? n.host}:{n.port}</span>
                        <Spacer />
                        {c === "running" && <span className="muted"><Loader size={12} className="spin" /> Checking…</span>}
                      </header>
                      {Array.isArray(c) && (
                        <ul className="check-list">
                          {c.map((s) => {
                            const Icon = STATUS_ICON[s.status];
                            return (
                              <li key={s.id} className={`check-${s.status}`}>
                                <Icon size={13} aria-hidden className="check-icon" />
                                <span className="check-label">{s.label}</span>
                                <span className="sr-only">{STATUS_WORD[s.status]}:</span>
                                <span className="check-detail">
                                  {s.detail}
                                  {s.hint && s.status !== "ok" && <span className="check-hint">{s.hint}</span>}
                                  {s.fingerprint && (
                                    <span className="check-trust">
                                      <code className="fingerprint">{s.fingerprint}</code>
                                      <button className="btn" onClick={() => trust(n, s)}>{s.status === "fail" ? "Review…" : "Trust…"}</button>
                                    </span>
                                  )}
                                </span>
                              </li>
                            );
                          })}
                        </ul>
                      )}
                      {!c && <p className="muted check-idle">Not checked yet.</p>}
                    </section>
                  );
                })}
              </div>
            </>
          )}

          {step === "vpn" && (
            <>
              <h2 className="wizard-title">Reaching the cluster from anywhere <HelpTip topic="start.connect.vpn" /></h2>
              <p className="wizard-lede">At home SparkBox talks to the nodes directly. Away, it can use its own WireGuard or AmneziaWG tunnel to your Firewalla Gold, for SparkBox only.</p>
              <FormGroup>
                <FormRow label="Right now">
                  <span>{vpn ? `${VPN_LABEL[vpn.state]}${vpn.state === "connected" ? ` (${protocolName(vpn.protocol)})` : ""}` : "—"}{vpn?.message ? ` · ${vpn.message}` : ""}</span>
                </FormRow>
              </FormGroup>
              <div className="form-row">
                <button className="btn" onClick={() => wm.open("vpn")}>Open VPN to import a profile…</button>
                <span className="muted">Optional: skip this if you only use SparkBox at home.</span>
              </div>
            </>
          )}

          {step === "switches" && (
            <>
              <h2 className="wizard-title">Switches and the gateway <HelpTip topic="add-switches-and-gateway" /></h2>
              <p className="wizard-lede">SparkBox reads the CRS804 through its REST API (HTTPS, a user with the read and rest-api policies) and watches the Firewalla devices by probing them. Leave an address empty to skip a device.</p>
              {inv.switches.map((sw, i) => {
                const t = tests[sw.id];
                const cert = t && t !== "running" ? certToTrust(t) : null;
                return (
                  <FormGroup key={sw.id} title={sw.name}>
                    <FormRow label="Address" htmlFor={`wiz-sw-${sw.id}`}>
                      <div className="wizard-inline">
                        <input id={`wiz-sw-${sw.id}`} className="mono" value={sw.host} placeholder={sw.kind === "gateway" ? "192.168.50.1" : sw.kind === "routerOs" ? "192.168.50.2" : "192.168.50.3"}
                          onChange={(e) => setInv({ ...inv, switches: inv.switches.map((s, j) => (j === i ? { ...s, host: e.target.value } : s)) })} />
                        <button className="btn" disabled={!sw.host.trim() || t === "running"} onClick={() => testSwitch(sw.id)}>{t === "running" ? "Testing…" : "Test"}</button>
                      </div>
                    </FormRow>
                    {sw.kind === "routerOs" && (
                      <>
                        <FormRow label={<>REST user <HelpTip topic="start.switches.rest-user" /></>} htmlFor={`wiz-swu-${sw.id}`}>
                          <input id={`wiz-swu-${sw.id}`} value={sw.user ?? ""} placeholder="sparkbox" onChange={(e) => setInv({ ...inv, switches: inv.switches.map((s, j) => (j === i ? { ...s, user: e.target.value || null } : s)) })} />
                        </FormRow>
                        <FormRow label={<>REST password <HelpTip topic="start.switches.rest-password" /></>} htmlFor={`wiz-swp-${sw.id}`} hint={saved[routerOsAccount(sw.id)] ? "Saved in the Keychain." : undefined}>
                          <div className="wizard-inline">
                            <input id={`wiz-swp-${sw.id}`} type="password" value={restPassword} onChange={(e) => setRestPassword(e.target.value)} />
                            <button className="btn" disabled={!restPassword} onClick={() => saveRestPassword(sw.id)}>Save to Keychain</button>
                          </div>
                        </FormRow>
                      </>
                    )}
                    {t && t !== "running" && (
                      <Notice tone={t.ok ? "success" : "error"} action={cert ? { label: "Trust…", onClick: () => trustSwitch(sw.id, cert) } : undefined}>
                        {t.message}
                      </Notice>
                    )}
                  </FormGroup>
                );
              })}
            </>
          )}

          {step === "done" && (
            <>
              <h2 className="wizard-title">{failing ? "Almost there" : "Ready"} <HelpTip topic="connect-your-cluster" /></h2>
              <p className="wizard-lede">
                {allChecks.length === 0
                  ? "You haven't run the node checks yet; you can still switch to Live and SparkBox shows what it finds."
                  : failing
                    ? `${failing} check${failing === 1 ? "" : "s"} failed. Live data works for the nodes that passed; the others show why they can't connect.`
                    : live
                      ? "Every node passed. Switch to Live to see the real cluster; Demo stays one click away in Settings › Data source."
                      : "The simulated checks passed; nothing real was tested. Switch to Live to check your nodes. Demo stays one click away in Settings › Data source."}
              </p>
              <div className="form-row">
                <button className="btn primary" disabled={busy} onClick={() => finish(true)}>{live ? "Save and stay on Live" : "Switch to Live data"}</button>
                <button className="btn" disabled={busy} onClick={() => finish(false)}>{live ? "Switch back to Demo" : "Stay in Demo"}</button>
              </div>
            </>
          )}
        </div>
        <footer className="wizard-foot">
          <button className="btn" onClick={() => go(-1)} disabled={at === 0}>Back</button>
          <Spacer />
          <span className="muted">Step {at + 1} of {STEPS.length}</span>
          {at < STEPS.length - 1 && (
            <button
              className="btn primary"
              disabled={busy}
              onClick={async () => {
                if (step === "nodes") {
                  try {
                    await saveInventory();
                  } catch (e) {
                    return setError(describe(e));
                  }
                }
                go(1);
                if (step === "nodes" && !Object.keys(checks).length) void runChecks();
              }}
            >
              Next
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
