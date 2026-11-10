import { useEffect, useMemo, useState, type ChangeEvent, type MouseEvent, type ReactNode } from "react";
import { Copy, Eye, EyeOff, FileUp, Globe, Plug, PlugZap, Plus, RefreshCw, Trash2, Unplug } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Table, type Column } from "../components/Table";
import { Badge, HelpTip, InfoTip, Notice, SectionTitle, Segmented, Sparkline, Spacer, StatusDot, Switch, Toolbar, ToolButton } from "../components/ui";
import { unlockedFor, useAdminState, withAdmin } from "../lib/admin";
import { api } from "../lib/api";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { netApi } from "../lib/netApi";
import { fmtBytes, fmtMs, fmtRate } from "../lib/format";
import { confirmAction } from "../lib/session";
import { useCluster } from "../lib/store";
import { toast } from "../lib/toasts";
import type { Inventory, Prefs, VpnForward, VpnLogEntry, VpnMode, VpnReach, VpnView } from "../lib/types";
import { homePinOf, maskAddress, pathLine, protocolName, setVpnStatus, useVpnStatus, vpnSetHomePin, VPN_DOT, VPN_LABEL } from "../lib/vpn";
import { useWM, useWinState } from "../wm/WindowManager";
import "./net.css";

const MODES: { value: VpnMode; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "always", label: "Always" },
  { value: "off", label: "Off" },
];

const HOME_PIN_HELP = "When no node answers, keep Auto direct on a matching local subnet or while this Mac has no network. Leave this off when away: another network may use the same addresses. Connect still starts the tunnel.";

const LOG_KIND: Record<VpnLogEntry["kind"], string> = { state: "State", network: "Network", action: "You", helper: "Tunnel" };

function ago(ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function handshake(sec: number | null | undefined): string {
  if (sec == null) return "None yet";
  if (sec < 120) return `${sec} s ago`;
  return `${Math.floor(sec / 60)} min ${sec % 60} s ago`;
}

/** Problems with the exposed ports, by row: errors stop saving, warnings don't. */
function validateForwards(forwards: VpnForward[]): { errors: Map<number, string>; warnings: Map<number, string> } {
  const errors = new Map<number, string>();
  const warnings = new Map<number, string>();
  const seen = new Map<number, number>();
  forwards.forEach((f, i) => {
    if (!Number.isInteger(f.localPort) || f.localPort < 1 || f.localPort > 65535) errors.set(i, "Use a port from 1 to 65535.");
    else if (seen.has(f.localPort)) errors.set(i, `Port ${f.localPort} is already used by "${forwards[seen.get(f.localPort)!].label}".`);
    else if (f.localPort < 1024) warnings.set(i, "Ports below 1024 are usually taken by macOS services.");
    if (!/^[^\s:]+:\d{1,5}$/.test(f.target.trim())) errors.set(i, errors.get(i) ?? "Use a home address like 192.168.50.51:3000.");
    seen.set(f.localPort, i);
  });
  return { errors, warnings };
}

/** A row of "Reaching the cluster": a probe result, or the Gold before it has an address. */
interface ReachRow extends VpnReach {
  unset?: boolean;
}

export function VpnWindow() {
  const live = useVpnStatus();
  const cluster = useCluster();
  const adminState = useAdminState();
  const wm = useWM();
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [view, setView] = useState<VpnView | null>(null);
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [host, setHost] = useState("");
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const [forwards, setForwards] = useState<VpnForward[]>([]);
  const [dirtyForwards, setDirtyForwards] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reveal, setReveal] = useState(false);
  const [profileOpen, setProfileOpen] = useWinState("profileOpen", false);
  const [now, setNow] = useState(Date.now());

  const apply = (v: VpnView, resetForms = true) => {
    setView(v);
    setVpnStatus(v.status);
    if (resetForms) {
      setHost(v.settings.profile?.endpointHost ?? "");
      setForwards(v.settings.forwards);
      setDirtyForwards(false);
    }
  };

  useEffect(() => {
    api.vpnGet().then((v) => apply(v), (e) => setError(String(e)));
    api.prefsGet().then(setPrefs, () => {});
    api.getInventory().then(setInventory, () => {});
    // The log, reachability and rates aren't pushed; refresh them while the window is open.
    const t = setInterval(() => {
      setNow(Date.now());
      api.vpnGet().then((v) => setView((old) => (old ? { ...v, settings: old.settings } : v)), () => {});
    }, 2000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const run = async (fn: () => Promise<VpnView>, resetForms = true) => {
    setBusy(true);
    try {
      apply(await fn(), resetForms);
      setError(null);
    } catch (e) {
      // Cancelling the Touch ID prompt isn't an error worth showing.
      if ((e as { kind?: string })?.kind !== "cancelled") setError((e as { message?: string })?.message ?? String(e));
    } finally {
      setBusy(false);
    }
  };
  /** VPN settings are state on this Mac: unlock for the Mac first (Touch ID), then go on. */
  const runMac = (action: string, fn: () => Promise<VpnView>, resetForms = true) => run(() => withAdmin(fn, { scope: "mac", action }), resetForms);

  const pickFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (f) {
      setFile({ name: f.name, text: await f.text() });
      // A replacement profile supplies its own current endpoint; an old address
      // shown for the previous profile must not silently override it.
      setHost("");
    }
  };

  const check = useMemo(() => validateForwards(forwards), [forwards]);

  if (!view) return <div className="pad muted">{error ?? "Loading…"}</div>;

  const status = live ? { ...view.status, ...live } : view.status;
  const profile = view.settings.profile;
  const hostChanged = !!profile && host.trim() !== profile.endpointHost;
  const safe = !!prefs?.screenshotSafe;
  const shown = reveal && !safe;
  const tunnel = status.state === "connected" || status.state === "connecting";
  // Direct or off, the tunnel's settings and counters describe nothing in use right now.
  const idle = status.state === "direct" || status.state === "off";
  const unused = (v: ReactNode) => (idle ? <span className="muted">{v} · not in use</span> : v);
  const rates = view.rates ?? [];
  const rateMax = Math.max(1, ...rates.map((r) => Math.max(r.rxBps, r.txBps)));
  const locked = !unlockedFor("mac", adminState);

  const remove = async () => {
    const ok = await confirmAction({
      title: "Remove the VPN profile?",
      body: "SparkBox deletes its copy of the keys from the Keychain. You'd need the .conf file to set it up again.",
      confirmLabel: "Remove",
      danger: true,
    });
    if (ok) {
      await runMac("vpn_remove", api.vpnRemove);
      setFile(null);
    }
  };

  const override = (action: "connect" | "disconnect" | null) => runMac("vpn_override", () => netApi.vpnOverride(action), false);

  const usedPorts = new Set(forwards.map((f) => f.localPort));
  const freePort = (want: number) => {
    let p = want;
    while (usedPorts.has(p) || p < 1024) p = p < 1024 ? p + 8000 : p + 1;
    return p;
  };
  const addForward = (label: string, target: string, remotePort: number) => {
    setForwards([...forwards, { label, target, localPort: freePort(remotePort), enabled: true }]);
    setDirtyForwards(true);
  };
  const editForward = (i: number, patch: Partial<VpnForward>) => {
    setForwards(forwards.map((f, j) => (j === i ? { ...f, ...patch } : f)));
    setDirtyForwards(true);
  };

  const fromCluster = (e: MouseEvent) => {
    const nodes = cluster?.nodes ?? [];
    const items: MenuItem[] = nodes
      .filter((n) => n.listeners.some((l) => l.addr !== "127.0.0.1" && l.addr !== "127.0.0.53" && l.port !== 22))
      .map((n) => {
        const ip = n.nets.find((x) => x.role === "lan")?.ip ?? n.host;
        const listeners = n.listeners.filter((l) => l.addr !== "127.0.0.1" && l.addr !== "127.0.0.53" && l.port !== 22);
        return {
          label: n.name,
          submenu: listeners.map((l) => {
            const owner = n.containers.find((c) => c.ports.some((p) => p.includes(`:${l.port}->`)) || (c.state === "running" && n.processes.some((p) => p.container === c.name && p.ports.includes(l.port))));
            const what = owner?.name ?? l.process ?? `port ${l.port}`;
            return { label: `${what} · ${l.port}`, run: () => addForward(`${what} on ${n.name}`, `${ip}:${l.port}`, l.port) };
          }),
        };
      });
    showContextMenu(e, items.length ? items : [{ label: "No listening ports reported yet", disabled: true }]);
  };

  // Every node and the Gold. The Gold is listed even before it has an address, so it's never silently missing.
  const reach: ReachRow[] = [...(view.reach ?? [])];
  for (const g of inventory?.switches.filter((s) => s.kind === "gateway") ?? []) {
    if (!reach.some((r) => r.id === g.id)) reach.push({ id: g.id, name: g.name, addr: "", via: "direct", ok: false, rttMs: null, error: null, checkedMs: 0, unset: true });
  }
  const sshState = (r: ReachRow) => {
    if (r.unset) return { text: "Not set up", status: "unconfigured" as const };
    const node = cluster?.nodes.find((n) => n.id === r.id);
    if (node?.status === "online" && !cluster?.demo) return { text: "Logged in", status: "online" as const };
    if (r.ok) return { text: "Port answers", status: "online" as const };
    return { text: "No answer", status: "offline" as const };
  };
  const reachColumns: Column<ReachRow>[] = [
    { key: "name", label: "Device", sort: (r) => r.name, render: (r) => <span className="strong">{r.name}</span>, fit: "content" },
    { key: "addr", label: "Address", sort: (r) => r.addr, render: (r) => (r.unset ? <span className="muted">—</span> : <span className="mono">{r.addr}</span>), ellipsis: "middle" },
    { key: "via", label: "Path", sort: (r) => r.via, render: (r) => (r.unset ? <span className="muted">—</span> : <span>{r.via === "tunnel" ? "Through the tunnel" : "Direct"}</span>), priority: 2 },
    {
      key: "rtt", label: "Connect time", short: "Connect", align: "right", sort: (r) => r.rttMs ?? 1e9, fit: "content",
      title: (r) => (r.rttMs != null ? `A TCP connection to ${r.addr} took ${fmtMs(r.rttMs)}. The status bar shows the round trip of SparkBox's last command over SSH, which includes the node's work.` : ""),
      render: (r) => <span className="num">{r.rttMs != null ? fmtMs(r.rttMs) : "—"}</span>,
    },
    {
      key: "ssh", label: "Answers", sort: (r) => (r.unset ? 2 : r.ok ? 0 : 1),
      render: (r) => (r.unset ? (
        <button type="button" className="link-plain vpn-unset" onClick={() => wm.open("settings", { section: "switches" })} title="Add its address in Settings › Switches & gateway">
          <StatusDot status="unconfigured" text="Not set up" />
        </button>
      ) : <StatusDot status={sshState(r).status} text={sshState(r).text} />),
      title: (r) => r.error ?? (r.unset ? "Add the Gold's LAN address in Settings › Switches & gateway to test it" : ""),
    },
    { key: "checked", label: "Checked", sort: (r) => -r.checkedMs, render: (r) => <span className="muted">{r.unset ? "—" : ago(r.checkedMs, now)}</span>, priority: 2, fit: "content" },
  ];

  return (
    <div className="col fill vpn-win">
      <Toolbar label="VPN actions">
        <StatusDot status={VPN_DOT[status.state]} text={VPN_LABEL[status.state]} />
        {tunnel && <span className="toolbar-text">{protocolName(status.protocol)}</span>}
        <Spacer />
        {status.override ? (
          <ToolButton icon={RefreshCw} label="Follow mode" onClick={() => override(null)} disabled={busy} locked={locked} scope="mac" title={`Go back to ${MODES.find((m) => m.value === view.settings.mode)?.label} mode`} />
        ) : null}
        {status.state === "connected" || status.state === "connecting" ? (
          <ToolButton icon={Unplug} label="Disconnect" onClick={() => override("disconnect")} disabled={busy || !profile} locked={locked} scope="mac" title="Stop the tunnel until you connect again" />
        ) : (
          <ToolButton icon={Plug} label="Connect" onClick={() => override("connect")} disabled={busy || !profile} locked={locked} scope="mac" title="Use the tunnel now, even if the cluster is reachable directly" />
        )}
        <ToolButton icon={RefreshCw} label="Reconnect" onClick={() => run(api.vpnReconnect, false)} disabled={busy || !profile || status.state === "off"} priority={2} />
        <ToolButton
          icon={EyeOff}
          label="Screenshot-safe"
          active={safe}
          priority={3}
          disabled={!prefs}
          title="Mask public addresses and DNS here, so screenshots don't give them away"
          onClick={async () => prefs && setPrefs(await api.prefsPatch({ screenshotSafe: !safe }))}
        />
        <HelpTip topic="mon.vpn.screenshot-safe" />
      </Toolbar>

      <div className="vpn-scroll">
        {cluster?.demo && (
          <span className="vpn-real-chip" role="note" title="Demo data changes the cluster only; the tunnel, its keys and the exposed ports are real.">
            Your real VPN, also in Demo
          </span>
        )}
        {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}

        <section className={`vpn-card state-${status.state}`} aria-label="VPN status">
          <div className="vpn-card-main">
            <div className="vpn-state">
              <StatusDot status={VPN_DOT[status.state]} />
              <span className="vpn-state-word">{VPN_LABEL[status.state]}</span>
              {status.override && <Badge tone="info">{status.override === "connect" ? "Connected by you" : "Disconnected by you"}</Badge>}
            </div>
            <div className="vpn-path">
              <span>{pathLine(status, shown, now)}</span>
              {(status.message || status.network) && (
                <InfoTip topic="mon.vpn.connection" label="About this connection">
                  {status.message && <p>{status.message}</p>}
                  {status.network && <p>This Mac: {status.network}</p>}
                </InfoTip>
              )}
              {status.state === "connected" && (
                <button
                  type="button"
                  className="tool vpn-eye"
                  onClick={() => setReveal(!reveal)}
                  disabled={safe}
                  aria-pressed={shown}
                  aria-label={shown ? "Hide the Firewalla's address" : "Show the Firewalla's address"}
                  title={safe ? "Screenshot-safe mode is on (Settings)" : shown ? "Hide the address" : "Show the address"}
                >
                  {shown ? <EyeOff size={13} /> : <Eye size={13} />}
                </button>
              )}
            </div>
            {status.message && status.state === "error" && <p className="vpn-message error-text">{status.message}</p>}
          </div>
          <dl className="vpn-facts">
            <div><dt>Handshake</dt><dd className="num">{tunnel ? handshake(status.handshakeAgeSec) : idle ? <span className="muted">Not in use</span> : "—"}</dd></div>
            <div><dt>Connect time</dt><dd className="num" title="The fastest TCP connection to a node in the last round of probes">{status.latencyMs != null ? fmtMs(status.latencyMs) : "—"}</dd></div>
            <div><dt>Protocol</dt><dd>{profile ? unused(protocolName(profile.protocol)) : "—"}</dd></div>
            <div><dt>MTU</dt><dd className="num">{profile ? unused(profile.mtu ?? "Automatic") : "—"}</dd></div>
            <div><dt>DNS</dt><dd className="mono">{profile?.dns.length ? unused(safe ? "••••" : profile.dns.join(", ")) : "—"}</dd></div>
            <div><dt>Received / sent</dt><dd className="num">{idle ? <span className="muted">Not in use</span> : `${fmtBytes(status.rxBytes)} / ${fmtBytes(status.txBytes)}`}</dd></div>
          </dl>
          {tunnel ? (
            <div className="vpn-rates">
              <div className="vpn-rate">
                <span className="muted">↓</span>
                <Sparkline values={rates.map((r) => r.rxBps)} max={rateMax} width={150} height={26} color="var(--m-rx)" fill label="Receive rate through the tunnel" />
                <span className="num">{fmtRate(status.rxRateBps ?? 0)}</span>
              </div>
              <div className="vpn-rate">
                <span className="muted">↑</span>
                <Sparkline values={rates.map((r) => r.txBps)} max={rateMax} width={150} height={26} color="var(--m-tx)" fill label="Send rate through the tunnel" />
                <span className="num">{fmtRate(status.txRateBps ?? 0)}</span>
              </div>
            </div>
          ) : (
            <p className="vpn-no-traffic muted">{status.state !== "direct" ? `No tunnel traffic (${VPN_LABEL[status.state].toLowerCase()})` : status.latencyMs != null ? "Tunnel not in use: the cluster answers directly" : status.message ? `Tunnel not in use. ${status.message}` : "Tunnel not in use"}</p>
          )}
        </section>

        <section>
          <SectionTitle
            title="Reaching the cluster"
            infoTopic="mon.vpn.reaching"
          />
          <Table columns={reachColumns} rows={reach} rowKey={(r) => r.id} tableId="vpn.reach" ariaLabel="Reachability of the nodes and the Gold" empty="Checking…" />
        </section>

        <section>
          <SectionTitle
            title="When to use the tunnel"
            infoTopic="mon.vpn.when"
          />
          <div className="vpn-mode-row">
            <Segmented<VpnMode> label="VPN mode" value={view.settings.mode} options={MODES.map((m) => ({ value: m.value, label: m.label }))} onChange={(m) => runMac("vpn_set_mode", () => api.vpnSetMode(m))} />
            <Switch
              label="Direct at home"
              showLabel
              checked={homePinOf(view.settings)}
              disabled={busy || view.settings.mode !== "auto"}
              description={view.settings.mode === "auto" ? HOME_PIN_HELP : "Only Auto decides by itself when to use the tunnel."}
              onChange={(on) => runMac("vpn_set_mode", () => vpnSetHomePin(view.settings.mode, on))}
            />
            <HelpTip topic="mon.vpn.direct-at-home" />
          </div>
        </section>

        <section>
          <div className="vpn-section-head">
            <SectionTitle title="Expose to this Mac" infoTopic="mon.vpn.expose" />
            <Spacer />
            <button className="btn" onClick={fromCluster}><Plus size={12} /> Add from cluster…</button>
            <button className="btn" onClick={() => addForward("New port", "", 8080)}><Plus size={12} /> Add port</button>
          </div>
          <table className="grid form-grid vpn-forwards">
            <thead>
              <tr><th>On</th><th>Label</th><th>Local port</th><th>Home address</th><th>Status</th><th /></tr>
            </thead>
            <tbody>
              {forwards.map((f, i) => {
                const st = status.forwards.find((s) => s.localPort === f.localPort);
                const err = check.errors.get(i);
                const warn = check.warnings.get(i);
                const url = `http://localhost:${f.localPort}`;
                return (
                  <tr key={i} className={err ? "row-crit" : undefined}>
                    <td><input type="checkbox" checked={f.enabled} aria-label={`Expose ${f.label}`} onChange={(e) => editForward(i, { enabled: e.target.checked })} /></td>
                    <td><input value={f.label} aria-label="Label" onChange={(e) => editForward(i, { label: e.target.value })} /></td>
                    <td>
                      <input className="narrow num" type="number" min={1} max={65535} value={f.localPort} aria-label="Local port" aria-invalid={!!err}
                        onChange={(e) => editForward(i, { localPort: Number(e.target.value) || 0 })} />
                    </td>
                    <td><input className="mono" value={f.target} placeholder="192.168.50.51:3000" aria-label="Home address" onChange={(e) => editForward(i, { target: e.target.value })} /></td>
                    <td className="vpn-forward-status">
                      {err ? <span className="error-text">{err}</span> : !f.enabled ? <span className="muted">Off</span> : st?.error ? <span className="error-text">{st.error}</span> : st?.active ? (
                        <span className="vpn-listening">
                          <StatusDot status="online" text="Listening" />
                          <button type="button" className="link-plain" onClick={() => openUrl(url).catch(() => {})} title={`Open ${url}`}><Globe size={11} /> Open</button>
                          <button type="button" className="link-plain" onClick={() => navigator.clipboard.writeText(url).then(() => toast({ kind: "success", title: "Copied", body: url }))} title="Copy the URL"><Copy size={11} /> Copy URL</button>
                        </span>
                      ) : status.state === "direct" ? <span className="muted">Not needed (direct)</span> : <span className="muted">Waiting for the tunnel</span>}
                      {warn && !err && <div className="warn-text">{warn}</div>}
                    </td>
                    <td>
                      <button className="tool" title="Remove" aria-label={`Remove ${f.label}`} onClick={() => { setForwards(forwards.filter((_, j) => j !== i)); setDirtyForwards(true); }}>
                        <Trash2 size={13} />
                      </button>
                    </td>
                  </tr>
                );
              })}
              {forwards.length === 0 && <tr><td colSpan={6} className="empty">No ports exposed. Add one from the cluster's listening ports.</td></tr>}
            </tbody>
          </table>
          <div className="form-row">
            <Spacer />
            {dirtyForwards && <button className="btn" onClick={() => { setForwards(view.settings.forwards); setDirtyForwards(false); }}>Revert</button>}
            <button className="btn primary" disabled={busy || !dirtyForwards || check.errors.size > 0} title={locked ? "Asks for Touch ID or your Mac password first" : undefined} onClick={() => runMac("vpn_set_forwards", () => api.vpnSetForwards(forwards))}>Save ports</button>
          </div>
        </section>

        <section>
          <h4 className="section-title">Connection log</h4>
          <ol className="vpn-log" aria-label="VPN connection log, newest first">
            {(view.log ?? []).length === 0 && <li className="muted">Nothing yet.</li>}
            {[...(view.log ?? [])].reverse().map((e, i) => (
              <li key={`${e.tsMs}-${i}`} className={`vpn-log-${e.kind}`}>
                <time className="num muted" dateTime={new Date(e.tsMs).toISOString()}>{new Date(e.tsMs).toLocaleTimeString()}</time>
                <span className="vpn-log-kind">{LOG_KIND[e.kind]}</span>
                <span className="vpn-log-text">{safe ? e.text.replace(/\b\d{1,3}(\.\d{1,3}){3}\b/g, (ip) => maskAddress(ip)) : e.text}</span>
              </li>
            ))}
          </ol>
        </section>

        <details className="vpn-profile" open={profileOpen} onToggle={(e) => setProfileOpen((e.currentTarget as HTMLDetailsElement).open)}>
          <summary>Profile{profile ? ` · ${protocolName(profile.protocol)}, imported ${new Date(profile.importedAt * 1000).toLocaleDateString()}` : " · none imported"}</summary>
          <div className="form-row">
            <label htmlFor="vpn-host">Firewalla address</label>
            <input id="vpn-host" placeholder="Public IP, e.g. 203.0.113.24" value={shown || !profile ? host : maskAddress(host)} readOnly={!!profile && !shown}
              onChange={(e) => setHost(e.target.value)} onFocus={() => profile && !safe && setReveal(true)} />
            {profile && <span className="muted">UDP port {profile.endpointPort} (from the file)</span>}
            {hostChanged && <button className="btn primary" disabled={busy} onClick={() => runMac("vpn_set_endpoint", () => api.vpnSetEndpoint(host))}>Save address</button>}
          </div>
          <div className="form-row">
            <label>Profile file</label>
            <label className="btn file-btn">
              <FileUp size={13} /> {file ? file.name : profile ? "Replace .conf…" : "Choose .conf…"}
              <input type="file" accept=".conf,text/plain" onChange={pickFile} hidden />
            </label>
            <button className="btn primary" disabled={busy || !file} onClick={() => file && runMac("vpn_import", () => api.vpnImport(file.text, host))}>
              {profile ? "Import replacement" : "Import"}
            </button>
          </div>
          <span className="vpn-profile-help">
            <span className="muted">Where the profile comes from</span>
            <HelpTip topic="mon.vpn.profile" label="About the VPN profile" />
          </span>
          {profile && (
            <>
              <dl className="kv">
                <div><dt>Protocol</dt><dd>{protocolName(profile.protocol)}{profile.protocol === "amneziawg" && ` (${Object.keys(profile.obfuscation).length} obfuscation settings)`}</dd></div>
                <div><dt>Tunnel address</dt><dd className="mono">{profile.addresses.join(", ")}</dd></div>
                <div><dt>Routes through the tunnel</dt><dd className="mono">{profile.allowedIps.join(", ")} <span className="muted">(SparkBox's traffic only)</span></dd></div>
                <div><dt>Keepalive</dt><dd>{profile.keepalive ?? 25} s</dd></div>
                <div><dt>Keys</dt><dd>In the Keychain{profile.hasPresharedKey ? " (private and preshared)" : ""}</dd></div>
              </dl>
              <div className="form-row">
                <Spacer />
                <button className="btn" onClick={remove} disabled={busy}><Trash2 size={12} /> Remove profile</button>
              </div>
            </>
          )}
        </details>
        {!profile && (
          <Notice tone="info" action={{ label: "Import a profile…", onClick: () => setProfileOpen(true) }}>
            <PlugZap size={12} /> No VPN profile yet. At home SparkBox reaches the cluster directly; import one to reach it from anywhere.
          </Notice>
        )}
      </div>
    </div>
  );
}
