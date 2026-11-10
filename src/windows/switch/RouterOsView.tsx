import { crs804Profile, isCrs804, useHardwareProfiles } from "../../lib/hardware";
import { useEffect, useMemo, useState } from "react";
import { Ban, CircleCheck, Lightbulb, Lock, PanelRight, Pencil, PlugZap, RefreshCw, RotateCcw } from "lucide-react";
import { Table, type Column } from "../../components/Table";
import { Badge, HelpTip, Notice, Spacer, Sparkline, StatusDot, Tabs, Tile, Tiles, Toolbar, ToolButton, ToolSeparator } from "../../components/ui";
import { SwitchState } from "./SwitchState";
import { confirmPhrase, fabricApi, type PortChange } from "../../lib/fabric";
import { NeighborsTab } from "./NeighborsTab";
import { ADMIN_REFUSED, isLookOnly, requestAdmin, useAdmin, withAdmin } from "../../lib/admin";
import { fmtBits, fmtDuration } from "../../lib/format";
import { deploymentsOf, shortModel } from "../../lib/impact";
import { netApi, type SwitchTestResult } from "../../lib/netApi";
import { confirmAction, promptText } from "../../lib/session";
import { toast } from "../../lib/toasts";
import type { ClusterSnapshot, SwitchPort, SwitchSnapshot } from "../../lib/types";
import { useWinState } from "../../wm/WindowManager";
import { capacityBps, errorTotal, fmtDbm, loadPct, opticTone, portState, shortPort, STATE_WORD, usePortRing } from "./common";
import { PortDrawer } from "./PortDrawer";
import { PortPanel } from "./PortPanel";

function describe(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : (e as { message?: string })?.message ?? String(e);
}

/** `linkedPort` comes from a link (Topology, a node's Network tab); `nav` changes on each one. */
export function RouterOsView({ sw, cluster, linkedPort, nav }: { sw: SwitchSnapshot; cluster: ClusterSnapshot; linkedPort?: string; nav?: string }) {
  const admin = useAdmin();
  const { view: hardware } = useHardwareProfiles(`${cluster.demo}:${sw.id}:${sw.board}`);
  const equipment = hardware?.devices.find((d) => d.id === `switch:${sw.id}` && d.identity === sw.board);
  const profile = isCrs804(sw.board) ? crs804Profile : equipment?.entry?.profile;
  const [tab, setTab] = useWinState<string>("tab", "ports");
  const [selected, setSelected] = useWinState<string | null>("port", linkedPort ?? null);
  const [drawer, setDrawer] = useWinState<boolean>("drawer", !!linkedPort);

  // A link to a port selects it and opens its details.
  useEffect(() => {
    if (!linkedPort || !sw.ports.some((p) => p.name === linkedPort)) return;
    setSelected(linkedPort);
    setDrawer(true);
    // Only when a link asks, not on every snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linkedPort, nav]);
  const [test, setTest] = useState<SwitchTestResult | null>(null);
  const [busy, setBusy] = useState(false);
  /** Disabled ports whose revert is still waiting on the switch (from the snapshot, so it survives reopening). */
  const reverting = sw.ports.filter((p) => p.disabled && p.revertAtMs);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!reverting.length) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [reverting.length]);
  const ring = usePortRing(sw, cluster.tsMs);
  const port = sw.ports.find((p) => p.name === selected) ?? null;
  const nodeName = (id: string | null | undefined) => (id ? cluster.nodes.find((n) => n.id === id)?.name ?? id : null);
  const locked = !admin;

  /** The deployments a port carries: those with a member on the port's node. */
  const carried = (p: SwitchPort) => {
    if (!p.node) return [];
    return deploymentsOf(cluster).filter((d) => d.nodes.includes(p.node!) && d.members.some((m) => m.running));
  };
  const impactLines = (p: SwitchPort, what: string) =>
    carried(p).map((d) => `${d.model ? shortModel(d.model) : d.id} runs across ${d.nodes.map(nodeName).join(", ")}: it stops serving ${what}, and reloads afterwards (a minute or so).`);

  const act = async (fn: () => Promise<void>, done?: string) => {
    setBusy(true);
    try {
      await fn();
      if (done) toast({ kind: "success", title: done });
    } catch (e) {
      if (e !== ADMIN_REFUSED) toast({ kind: "error", title: "The switch refused", body: describe(e) });
    } finally {
      setBusy(false);
    }
  };

  const setDisabled = async (disabled: boolean) => {
    if (!port) return;
    let confirm: string | undefined;
    if (disabled) {
      const lines = impactLines(port, "while it's down");
      const mgmt = port.management;
      const phrase = confirmPhrase("disable", port.name);
      const ok = await confirmAction({
        title: `Disable ${port.name}${port.node ? ` (${nodeName(port.node)})` : ""}?`,
        body: mgmt
          ? `This port carries SparkBox's connection to ${sw.name}. ${mgmt} SparkBox will lose the switch until it re-enables the port by itself, 60 seconds from now.`
          : port.node
            ? `${nodeName(port.node)} loses its 200G fabric link. The switch re-enables the port after 60 seconds unless you choose Keep disabled.`
            : "Whatever is plugged into this port loses its link. The switch re-enables it after 60 seconds unless you choose Keep disabled.",
        details: lines.length ? <ul className="impact-list">{lines.map((l) => <li key={l}>{l}</li>)}</ul> : undefined,
        confirmLabel: "Disable port",
        danger: true,
        requireTyping: mgmt ? phrase : undefined,
      });
      if (!ok) return;
      if (mgmt) confirm = phrase;
    }
    setBusy(true);
    try {
      const change: PortChange = await fabricApi.portSet(sw.id, port.name, disabled, confirm);
      if (!disabled) toast({ kind: "success", title: change.message });
    } catch (e) {
      toast({ kind: "error", title: "The switch refused", body: describe(e) });
    } finally {
      setBusy(false);
    }
  };

  const keepDisabled = async (name: string) => {
    setBusy(true);
    try {
      const change = await fabricApi.portKeepDisabled(sw.id, name);
      toast({ kind: "success", title: change.message });
    } catch (e) {
      toast({ kind: "error", title: "The revert stays", body: describe(e) });
    } finally {
      setBusy(false);
    }
  };

  const bounce = async () => {
    if (!port) return;
    const lines = impactLines(port, "for a few seconds");
    const mgmt = port.management;
    const phrase = confirmPhrase("bounce", port.name);
    const ok = await confirmAction({
      title: `Bounce ${port.name}?`,
      body: mgmt
        ? `This port carries SparkBox's connection to ${sw.name}. The switch takes it down for 3 seconds and brings it back by itself; SparkBox reconnects after.`
        : "The switch takes the port down for 3 seconds and brings it back by itself: the usual first step for a link that misbehaves.",
      details: lines.length ? <ul className="impact-list">{lines.map((l) => <li key={l}>{l}</li>)}</ul> : undefined,
      confirmLabel: "Bounce port",
      danger: carried(port).length > 0 || !!mgmt,
      requireTyping: mgmt ? phrase : undefined,
    });
    if (!ok) return;
    await act(async () => {
      const change = await fabricApi.portBounce(sw.id, port.name, mgmt ? phrase : undefined);
      toast({ kind: "success", title: change.message });
    });
  };

  const resetCounters = async () => {
    if (!port) return;
    const ok = await confirmAction({
      title: `Reset the counters on ${port.name}?`,
      body: "Errors, drops, pause frames and link-downs start again from zero, so new problems are easy to see. The old totals are gone.",
      confirmLabel: "Reset counters",
    });
    if (ok) await act(() => netApi.switchPortResetCounters(sw.id, port.name), `Counters on ${port.name} reset`);
  };

  const identify = () => port && act(() => netApi.switchPortBlink(sw.id, port.name), `${port.name}'s light blinks for 10 seconds`);

  const editComment = async () => {
    if (!port) return;
    const value = await promptText({
      title: `Comment for ${port.name}`,
      body: "SparkBox matches the comment to a node to know what's cabled where.",
      label: "Comment",
      initial: port.comment,
      placeholder: "e.g. cell2",
      confirmLabel: "Save",
      validate: (v) => (v.length > 120 ? "Keep it under 120 characters." : null),
    });
    if (value !== null && value !== port.comment) await act(() => netApi.switchPortComment(sw.id, port.name, value.trim()), "Comment saved");
  };

  const runTest = async () => {
    setBusy(true);
    try {
      setTest(await netApi.switchTest(sw.id));
    } catch (e) {
      setTest({ ok: false, board: null, version: null, fingerprint: null, errorKind: "other", message: describe(e) });
    } finally {
      setBusy(false);
    }
  };

  const trust = async () => {
    if (!test?.fingerprint) return;
    const ok = await confirmAction({
      title: `Trust this certificate for ${sw.name}?`,
      body: "Only if it matches the switch: in WinBox or WebFig, System › Certificates shows the fingerprint of the one www-ssl uses.",
      details: <code className="fingerprint">{groupFp(test.fingerprint)}</code>,
      confirmLabel: "Trust",
    });
    // The trusted fingerprint is saved on this Mac: Touch ID or the password first.
    const fp = test.fingerprint;
    if (ok) await act(async () => setTest(await withAdmin(() => netApi.switchTrust(sw.id, fp), { scope: "mac", action: "switch_trust" })), "Certificate trusted");
  };

  const sensors = sw.sensors ?? [];
  const sensor = (name: string) => sensors.find((s) => s.name === name);
  const psu = (n: number) => sensor(`PSU${n}`);
  const fans = sensors.filter((s) => s.unit === "RPM");

  const columns: Column<SwitchPort>[] = useMemo(
    () => [
      { key: "name", label: "Port", help: "mon.switch.ports", sort: (p) => p.name, priority: 1, width: 132, minWidth: 128, render: (p) => <span className="strong port-name">{p.name}</span>, title: (p) => p.name },
      {
        key: "node", label: "Node", sort: (p) => nodeName(p.node) ?? p.comment, priority: 1, ellipsis: "end",
        render: (p) => {
          const n = nodeName(p.node);
          if (p.management) return <span className="mgmt-tag"><Lock size={11} aria-hidden /> SparkBox's path</span>;
          if (!n) return p.comment ? <span className="muted">{p.comment}</span> : <span className="muted">—</span>;
          return (
            <span className={p.cablingOk === false ? "warn-text" : undefined}>
              {n}
              {p.cablingOk === false && p.expectedNode && <span> · expected {nodeName(p.expectedNode)}</span>}
            </span>
          );
        },
        title: (p) => [p.mappedBy ? `Found by ${p.mappedBy === "mac" ? "MAC (bridge host table)" : p.mappedBy === "lldp" ? "LLDP neighbor" : "port comment"}` : null, p.comment ? `Comment: ${p.comment}` : null, p.management].filter(Boolean).join(" · "),
      },
      {
        key: "state", label: "State", sort: (p) => ["up", "errors", "down", "disabled"].indexOf(portState(p, ring.rising(p.name))), priority: 1, width: 116,
        render: (p) => {
          const s = portState(p, ring.rising(p.name));
          const text = s === "up" || s === "errors" ? `${STATE_WORD[s]}${p.speed ? ` · ${p.speed}` : ""}` : STATE_WORD[s];
          return <StatusDot status={s === "up" ? "online" : s === "errors" ? "crit" : s === "disabled" ? "error" : "offline"} text={text} />;
        },
      },
      { key: "rx", label: "↓ Receive", align: "right", sort: (p) => p.rxBps, priority: 1, render: (p) => (p.running ? <span className="num">{fmtBits(p.rxBps)}</span> : <span className="muted">—</span>) },
      { key: "tx", label: "↑ Transmit", align: "right", sort: (p) => p.txBps, priority: 1, render: (p) => (p.running ? <span className="num">{fmtBits(p.txBps)}</span> : <span className="muted">—</span>) },
      {
        key: "load", label: "Load", sort: (p) => loadPct(p), priority: 2, width: 96, minWidth: 80,
        // Under 1% a bar says nothing: the number alone.
        render: (p) => (!p.running ? <span className="muted">—</span> : loadPct(p) >= 1 ? <LoadBar pct={loadPct(p)} cap={capacityBps(p)} /> : <span className="num muted">{loadPct(p) > 0 ? "<1%" : "0%"}</span>),
      },
      {
        key: "spark", label: "Last 60 s", priority: 2, width: 96,
        render: (p) => {
          if (!p.running) return <span className="muted">—</span>;
          const s = ring.samples(p.name, 60);
          const values = s.map((x) => Math.max(x.rx, x.tx));
          return <Sparkline values={values} max={Math.max(1, ...values)} width={84} height={18} color="var(--m-rx)" label={`${p.name} traffic, last minute`} />;
        },
      },
      {
        key: "errors", label: "Errors", align: "right", sort: errorTotal, priority: 2,
        render: (p) => <span className={`num${ring.rising(p.name) ? " crit-text" : errorTotal(p) ? "" : " muted"}`}>{errorTotal(p).toLocaleString()}</span>,
      },
      { key: "flaps", label: "Flaps", align: "right", sort: (p) => p.linkDowns ?? 0, priority: 3, title: () => "Link-downs since the counters were reset", render: (p) => <span className="num">{(p.linkDowns ?? 0).toLocaleString()}</span> },
      { key: "mtu", label: "MTU", align: "right", sort: (p) => p.mtu ?? 0, priority: 3, render: (p) => <span className="num">{p.mtu ?? "—"}</span> },
      {
        key: "rxp", label: "Worst RX", align: "right", sort: (p) => p.rxPowerDbm ?? 99, priority: 2, width: 100,
        render: (p) => {
          const tone = opticTone(p.rxPowerDbm);
          return <span className={`num optic tone-${tone}`} title={tone === "warn" ? "Marginal: below −7 dBm" : tone === "crit" ? "Failing: below −10 dBm" : undefined}>{tone === "warn" || tone === "crit" ? "▼ " : ""}{fmtDbm(p.rxPowerDbm)}</span>;
        },
      },
      { key: "temp", label: "Temp", align: "right", sort: (p) => p.moduleTempC ?? -1, priority: 3, render: (p) => <span className="num">{p.moduleTempC != null ? `${p.moduleTempC.toFixed(0)} °C` : "—"}</span> },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ring.version, cluster.nodes],
  );

  const optics = sw.ports.filter((p) => p.name.startsWith("qsfp"));
  const lanesMax = Math.max(4, ...optics.map((p) => p.laneRxPowerDbm?.length ?? 0));

  const select = (name: string) => {
    setSelected(name);
    if (!drawer) setDrawer(true);
  };

  const testNotice = test && (
    <Notice tone={test.ok ? "success" : "error"} onDismiss={() => setTest(null)} action={test.fingerprint && !test.ok && admin ? { label: "Trust…", onClick: trust } : undefined}>
      {test.message}
      {test.fingerprint && !test.ok && !admin && !isLookOnly() && <span className="muted"> Unlock Admin mode to trust it.</span>}
    </Notice>
  );

  return (
    <div className="switch-win">
      <Toolbar label={`${sw.name} actions`}>
        <ToolButton icon={CircleCheck} label="Enable" onClick={() => setDisabled(false)} disabled={busy || !port?.disabled} locked={locked} />
        <ToolButton icon={Ban} label="Disable…" onClick={() => setDisabled(true)} disabled={busy || !port || port.disabled} locked={locked} danger title={port?.management ? "Carries SparkBox's connection: type to confirm, and the switch re-enables it after 60 s" : "The switch re-enables it after 60 s unless you keep it disabled"} />
        <HelpTip topic="mon.switch.disable" />
        <ToolButton icon={RefreshCw} label="Bounce…" onClick={bounce} disabled={busy || !port || port.disabled} locked={locked} priority={2} title="Down for 3 s, then back up: one script on the switch" />
        <HelpTip topic="mon.switch.bounce" />
        <ToolSeparator />
        <ToolButton icon={RotateCcw} label="Reset counters" onClick={resetCounters} disabled={busy || !port} locked={locked} priority={2} />
        <ToolButton icon={Lightbulb} label="Identify" onClick={identify} disabled={busy || !port} locked={locked} priority={3} title="Blink the port's light for 10 seconds" />
        <ToolButton icon={Pencil} label="Comment…" onClick={editComment} disabled={busy || !port} locked={locked} priority={3} />
        <Spacer />
        <ToolButton icon={PlugZap} label="Test connection" onClick={runTest} disabled={busy} priority={2} />
        <ToolButton icon={PanelRight} label="Details" onClick={() => setDrawer(!drawer)} active={drawer} disabled={!port} />
      </Toolbar>

      <div className="switch-strip">
        <SwitchState sw={sw} />
        {sw.latencyMs != null && <span className="num muted">{sw.latencyMs.toFixed(1)} ms</span>}
        <span className="strip-sep" />
        <span>{sw.board ?? "RouterOS switch"}</span>
        {sw.version && <span className="muted">RouterOS {sw.version}</span>}
        {sw.uptimeS != null && <span className="muted">up {fmtDuration(sw.uptimeS)}</span>}
        <Spacer />
        <span className="mono muted">{sw.host}</span>
        {cluster.demo && <Badge tone="neutral">Demo data</Badge>}
      </div>

      {sw.error && <Notice tone={sw.status === "online" ? "warn" : "error"} onRetry={runTest}>{sw.error}</Notice>}
      {testNotice}
      {reverting.map((p) => {
        const keep = !p.management && sw.status === "online";
        return (
          <Notice key={p.name} tone="warn" action={keep ? { label: "Keep disabled", onClick: () => void requestAdmin("switch_port_keep_disabled").then((ok) => { if (ok) void keepDisabled(p.name); }) } : undefined}>
            {p.name} is disabled. The switch re-enables it in {Math.max(0, Math.ceil(((p.revertAtMs ?? now) - now) / 1000))} s
            {p.management ? ": it carries SparkBox's connection, so it can't stay disabled." : sw.status === "online" ? " unless you keep it disabled." : "; Keep disabled appears once SparkBox reaches the switch again."}
          </Notice>
        );
      })}

      <div className="switch-main">
        <div className="switch-content">
          <PortPanel profile={profile} sw={sw} selected={selected} onSelect={select} rising={ring.rising} nodeName={nodeName} />
          {profile && <p className="hardware-source muted"><a href={profile.sourceUrl} target="_blank" rel="noreferrer">Manufacturer specification</a> · {isCrs804(sw.board) ? "Bundled physical layout" : "AI hardware profile; logical interfaces are listed below"}</p>}
          <Tabs
            label="Switch views"
            value={tab}
            onChange={setTab}
            tabs={[
              { id: "overview", label: "Overview" },
              { id: "ports", label: "Ports", badge: `${sw.ports.filter((p) => p.running).length} up` },
              { id: "optics", label: "Optics", badge: optics.some((p) => opticTone(p.rxPowerDbm) === "crit" || opticTone(p.rxPowerDbm) === "warn") ? "!" : undefined },
              { id: "neighbors", label: "Neighbors" },
            ]}
          >
            {tab === "overview" && (
              <div className="switch-overview">
                <Tiles>
                  <Tile label={<>CPU <HelpTip topic="mon.switch.overview" label="About the Overview tab" /></>} value={sw.cpuPct != null ? sw.cpuPct.toFixed(0) : "—"} unit={sw.cpuPct != null ? "%" : undefined} tone={sw.cpuPct != null && sw.cpuPct > 80 ? "warn" : undefined} />
                  <Tile label="Temperature" value={sw.tempC != null ? sw.tempC.toFixed(0) : "—"} unit={sw.tempC != null ? "°C" : undefined} sub={sensors.filter((s) => s.unit === "°C").map((s) => `${s.name.replace(" temperature", "")} ${s.value.toFixed(0)}°`).join(" · ")} tone={sw.tempC != null && sw.tempC > 75 ? "warn" : undefined} />
                  {[1, 2].map((n) => {
                    const p = psu(n);
                    return <Tile key={n} label={`Power supply ${n}`} value={p ? (p.ok ? "OK" : "Failed") : "—"} tone={p && !p.ok ? "crit" : p ? "good" : undefined} sub={p ? undefined : "Not reported"} />;
                  })}
                  {fans.map((f) => <Tile key={f.name} label={f.name} value={f.value.toLocaleString()} unit="RPM" tone={f.ok ? undefined : "crit"} sub={f.ok ? undefined : "Stopped"} />)}
                  <Tile label="Uptime" value={sw.uptimeS != null ? fmtDuration(sw.uptimeS) : "—"} />
                  <Tile label="RouterOS" value={sw.version ?? "—"} />
                  <Tile label="Board" value={sw.board ?? "—"} />
                  <Tile label="Ports up" value={`${sw.ports.filter((p) => p.running).length} / ${sw.ports.length}`} sub={`${sw.ports.filter((p) => p.disabled).length} disabled`} />
                </Tiles>
              </div>
            )}
            {tab === "ports" && (
              <Table
                columns={columns}
                rows={sw.ports}
                rowKey={(p) => p.name}
                selected={selected}
                onSelect={setSelected}
                onActivate={(p) => select(p.name)}
                tableId="switch.ports"
                footer
                itemNoun={["port", "ports"]}
                ariaLabel={`${sw.name} ports`}
                rowClass={(p) => (ring.rising(p.name) ? "row-crit" : p.disabled ? "row-dim" : undefined)}
              />
            )}
            {tab === "optics" && (
              <div className="optics-wrap">
                <table className="grid optics" aria-label="Optics by port and lane">
                  <thead>
                    <tr>
                      <th>Port</th>
                      <th>Node</th>
                      {Array.from({ length: lanesMax }, (_, i) => <th key={i} className="num">Lane {i + 1} RX</th>)}
                      <th className="num">TX</th>
                      <th className="num">Temp</th>
                      <th>Module</th>
                    </tr>
                  </thead>
                  <tbody>
                    {optics.map((p) => (
                      <tr key={p.name} className={p.name === selected ? "sel" : undefined} onClick={() => setSelected(p.name)}>
                        <td className="strong">{shortPort(p.name)}</td>
                        <td>{nodeName(p.node) ?? <span className="muted">{p.comment || "—"}</span>}</td>
                        {Array.from({ length: lanesMax }, (_, i) => {
                          const v = p.laneRxPowerDbm?.[i];
                          const tone = opticTone(v);
                          return (
                            <td key={i} className={`num optic-cell tone-${tone ?? "none"}`} title={tone === "warn" ? "Marginal (below −7 dBm)" : tone === "crit" ? "Failing (below −10 dBm)" : undefined}>
                              {v != null ? v.toFixed(1) : "—"}
                              {tone === "warn" && <span className="sr-only">, marginal</span>}
                              {tone === "crit" && <span className="sr-only">, failing</span>}
                            </td>
                          );
                        })}
                        <td className="num">{p.txPowerDbm != null ? p.txPowerDbm.toFixed(1) : "—"}</td>
                        <td className="num">{p.moduleTempC != null ? `${p.moduleTempC.toFixed(0)} °C` : "—"}</td>
                        <td className="muted">{p.sfpVendor ? `${p.sfpVendor} ${p.sfpPart ?? ""}` : "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="optics-legend">
                  <span className="muted">Receive power in dBm</span>
                  <HelpTip topic="mon.switch.rx-power" label="About receive power" />
                </div>
              </div>
            )}
            {tab === "neighbors" && <NeighborsTab sw={sw} cluster={cluster} onSelect={(name) => { setSelected(name); setTab("ports"); }} />}
          </Tabs>
        </div>
        {drawer && port && (
          <PortDrawer
            switchId={sw.id}
            port={port}
            node={cluster.nodes.find((n) => n.id === port.node)}
            samples={ring.samples(port.name)}
            startedAt={ring.startedAt}
            rising={ring.rising(port.name)}
            onClose={() => setDrawer(false)}
          />
        )}
      </div>
    </div>
  );
}

function groupFp(hex: string): string {
  return hex.match(/.{1,2}/g)?.join(":").toUpperCase() ?? hex;
}

function LoadBar({ pct, cap }: { pct: number; cap: number }) {
  const tone = pct >= 90 ? "crit" : pct >= 70 ? "warn" : "good";
  return (
    <span className="load-bar" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label={`${pct.toFixed(1)}% of ${cap / 1e9} Gb/s`}>
      <span className="load-track"><span className={`load-fill tone-${tone}`} style={{ width: `${Math.max(pct > 0 ? 2 : 0, pct)}%` }} /></span>
      <span className="num load-text">{pct < 1 && pct > 0 ? "<1" : pct.toFixed(0)}%</span>
    </span>
  );
}
