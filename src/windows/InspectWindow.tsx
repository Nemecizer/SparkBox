// docker inspect for one container: a summary (what it runs, mounts, ports, environment) and
// the raw JSON. The backend masks the values of secret names (API_KEY, HF_TOKEN; not a count
// like MTP_TOKENS) before they get here; each one shows as "•••• hidden" with Reveal, which
// needs Admin mode and is logged. A value over 200 characters starts folded.
import { useEffect, useState, type ReactNode } from "react";
import { Copy, Eye, RefreshCw } from "lucide-react";
import { HelpTip, Notice, Spacer, Toolbar, ToolButton } from "../components/ui";
import { isLookOnly, LOOK_ONLY_TIP, requestAdmin, useAdmin } from "../lib/admin";
import { api } from "../lib/api";
import { showError } from "../lib/errors";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { copyText } from "../lib/markdown";
import { relTime } from "./TasksWindow";
import { useDeployments } from "../lib/deployments";
import { explainDeployment } from "./models/DeploymentSummary";
import type { Params } from "../wm/WindowManager";
import "./workloads.css";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

/** What the backend puts in place of a secret's value (live/audit.rs REDACTED). */
export const MASKED = "‹redacted›";

/** Docker's "2026-10-26T09:14:02.123456789Z" as a local date and how long ago; "0001-…" means never. */
export function dockerTime(iso: string | undefined | null): string {
  if (!iso || iso.startsWith("0001-")) return "—";
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return iso;
  return `${fmtDateTime(ms)} (${relTime(ms)})`;
}

/** "NAME=value" split, with whether the backend masked the value. */
export function envEntry(line: string): { name: string; value: string; masked: boolean } {
  const i = line.indexOf("=");
  const name = i < 0 ? line : line.slice(0, i);
  const value = i < 0 ? "" : line.slice(i + 1);
  return { name, value, masked: value === MASKED };
}

/** Values longer than this start folded ("Show all (2,967 characters)"). */
export const LONG_VALUE = 200;

/** An environment value: whole when short, else its first 200 characters and Show all. */
export function EnvValue({ value }: { value: string }) {
  const [open, setOpen] = useState(false);
  if (value.length <= LONG_VALUE || open) return <span className="inspect-env-value">{value}</span>;
  return (
    <>
      <span className="inspect-env-value">{value.slice(0, LONG_VALUE)}…</span>{" "}
      <button type="button" className="btn small inspect-env-more" onClick={() => setOpen(true)} title="Show the whole value">
        Show all ({value.length.toLocaleString("en-US")} characters)
      </button>
    </>
  );
}

const JSON_TOKEN = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;

/** Pretty-printed JSON with keys, strings, numbers and literals coloured like the editor's. */
export function JsonView({ value }: { value: unknown }) {
  const text = JSON.stringify(value, null, 2) ?? "";
  const parts: ReactNode[] = [];
  let at = 0;
  for (const m of text.matchAll(JSON_TOKEN)) {
    const i = m.index ?? 0;
    if (i > at) parts.push(text.slice(at, i));
    const cls = m[1] ? (m[2] ? "json-key" : "json-str") : m[3] ? "json-lit" : "json-num";
    parts.push(<span key={i} className={cls}>{m[1] ?? m[0]}</span>);
    if (m[2]) parts.push(m[2]);
    at = i + m[0].length;
  }
  parts.push(text.slice(at));
  return <pre className="cmd fill-scroll json-view">{parts}</pre>;
}

export function InspectWindow({ params }: { params: Params }) {
  const failed = useDeployments().find((d) => d.state === "failed" && d.members.some((m) => m.node === params.node && m.id === params.id));
  const [info, setInfo] = useState<Json | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [raw, setRaw] = useState(false);
  /** Values revealed this time the window is open; never saved. */
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const admin = useAdmin();

  const load = () => {
    api.containerInspect(params.node, params.id).then(
      (v) => {
        setInfo(v);
        setError(null);
      },
      (e) => setError(String(e)),
    );
  };
  useEffect(load, [params.node, params.id]);

  if (error) return <Notice tone="error">{error}</Notice>;
  if (!info) return <div className="pad muted">Reading docker inspect…</div>;

  const cfg = info.Config ?? {};
  const host = info.HostConfig ?? {};
  const env: string[] = cfg.Env ?? [];
  // Docker lists mounts in no fixed order (a refresh shuffled them): sorted by where they appear inside.
  const mounts: Json[] = [...(info.Mounts ?? [])].sort((a: Json, b: Json) => String(a.Destination ?? "").localeCompare(String(b.Destination ?? "")));
  const ports = Object.entries(info.NetworkSettings?.Ports ?? {}) as [string, Json[] | null][];
  const labels = Object.entries(cfg.Labels ?? {}) as [string, string][];
  const gpus = (host.DeviceRequests ?? []).some((d: Json) => JSON.stringify(d.Capabilities ?? []).includes("gpu"));
  const envs = env.map(envEntry);
  const masked = envs.filter((e) => e.masked).length;

  const reveal = async (name: string) => {
    if (!(await requestAdmin("container_reveal"))) return;
    try {
      const value = await api.containerReveal(params.node, params.id, name);
      setRevealed((r) => ({ ...r, [name]: value }));
    } catch (e) {
      showError(e, { action: `Reveal ${name}` });
    }
  };

  return (
    <div className="col fill">
      <Toolbar>
        {failed && <ToolButton icon={Eye} label="Why it failed…" onClick={() => explainDeployment(failed)} />}
        <div className="seg" role="group" aria-label="View">
          <button className={!raw ? "on" : undefined} onClick={() => setRaw(false)}>Summary</button>
          <button className={raw ? "on" : undefined} onClick={() => setRaw(true)}>Raw JSON</button>
        </div>
        <ToolButton icon={Copy} label="Copy JSON" title="Copies it as shown, with hidden values left hidden" onClick={() => copyText(JSON.stringify(info, null, 2))} />
        <Spacer />
        <ToolButton icon={RefreshCw} label="Refresh" onClick={load} />
        <HelpTip topic="inspect-window" />
      </Toolbar>
      {raw ? (
        <JsonView value={info} />
      ) : (
        <div className="pad settings col gap">
          {info.SparkBoxDemo && <Notice>Demo data: this is what docker inspect shows for a container like this one.</Notice>}
          <dl className="kv">
            <div><dt>Name</dt><dd className="strong">{String(info.Name ?? "").replace(/^\//, "")}</dd></div>
            <div><dt>ID</dt><dd className="mono">{String(info.Id ?? "").slice(0, 24)}…</dd></div>
            <div><dt>Image</dt><dd className="mono">{cfg.Image}</dd></div>
            <div><dt>State</dt><dd>{info.State?.Status}{info.State?.Status === "exited" ? ` (exit ${info.State?.ExitCode}${info.State?.OOMKilled ? ", killed for running out of memory" : ""})` : ""}{info.State?.Health?.Status ? ` · ${info.State.Health.Status}` : ""}</dd></div>
            <div><dt>Started</dt><dd>{dockerTime(info.State?.StartedAt)}</dd></div>
            {info.State?.Status !== "running" && <div><dt>Finished</dt><dd>{dockerTime(info.State?.FinishedAt)}</dd></div>}
            <div><dt>Created</dt><dd>{dockerTime(info.Created)}</dd></div>
            {typeof info.RestartCount === "number" && <div><dt>Restarts</dt><dd className="num">{info.RestartCount}</dd></div>}
            <div><dt>Command</dt><dd className="mono wrap">{[...(cfg.Entrypoint ?? []), ...(cfg.Cmd ?? [])].join(" ") || "—"}</dd></div>
            <div><dt>Network</dt><dd>{host.NetworkMode ?? "—"}{host.IpcMode ? ` · IPC ${host.IpcMode}` : ""}{host.ShmSize ? ` · shm ${fmtBytes(host.ShmSize)}` : ""}</dd></div>
            <div><dt>GPUs</dt><dd>{gpus ? "all (--gpus)" : "none"}</dd></div>
            <div><dt>Restart</dt><dd>{host.RestartPolicy?.Name || "no"}</dd></div>
            <div><dt>Ports</dt><dd className={ports.length ? "mono" : undefined}>{ports.length ? ports.map(([p, b]) => `${p}${b?.length ? ` → ${b.map((x) => `${x.HostIp || "*"}:${x.HostPort}`).join(", ")}` : ""}`).join("  ") : host.NetworkMode === "host" ? "host network (all of the node's ports)" : "—"}</dd></div>
          </dl>
          <section>
            <h4>Mounts</h4>
            {mounts.length ? (
              <table className="grid">
                <thead><tr><th>Source on the node</th><th>Inside the container</th><th>Mode</th></tr></thead>
                <tbody>{mounts.map((m, i) => <tr key={i}><td className="mono">{m.Source ?? m.Name}</td><td className="mono">{m.Destination}</td><td>{m.RW ? "read-write" : "read-only"}</td></tr>)}</tbody>
              </table>
            ) : <span className="muted">None</span>}
          </section>
          <section>
            <h4>Environment{masked > 0 && <span className="muted"> · {masked} hidden</span>}</h4>
            {envs.length ? (
              <ul className="cmd inspect-env" aria-label="Environment">
                {envs.map((e) => (
                  <li key={e.name}>
                    <span className="inspect-env-name">{e.name}</span>=
                    {!e.masked ? <EnvValue value={e.value} />
                      : revealed[e.name] !== undefined ? <span className="inspect-env-revealed">{revealed[e.name]}</span>
                      : (
                        <>
                          <span className="inspect-env-hidden">•••• hidden</span>
                          <button type="button" className="btn small inspect-reveal" onClick={() => void reveal(e.name)} disabled={!admin && isLookOnly()}
                            title={admin ? `Show ${e.name}'s value; it's logged in the Activity log` : isLookOnly() ? LOOK_ONLY_TIP : `Requires Admin · click to unlock, then shows ${e.name}`}>
                            <Eye size={12} aria-hidden /> Reveal
                          </button>
                        </>
                      )}
                  </li>
                ))}
              </ul>
            ) : <span className="muted">None</span>}
          </section>
          {labels.length > 0 && (
            <section>
              <h4>Labels</h4>
              <pre className="cmd">{labels.map(([k, v]) => `${k}=${v}`).join("\n")}</pre>
            </section>
          )}
        </div>
      )}
    </div>
  );
}
