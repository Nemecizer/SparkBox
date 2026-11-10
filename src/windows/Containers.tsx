// Every container on every node, Portainer-style: select several and start, stop, restart,
// kill, pause, resume or remove them in one go, with one confirmation that says what goes
// down (a model spread over three nodes, say) and one task for the lot. Grouped by
// deployment, a group's containers are acted on in order: workers stop before their head,
// the head starts before its workers. Every row has the same actions on right-click, ⇧F10
// and their shortcuts (⌘L logs, ⌘T shell, ⌘I inspect); Return inspects. A node that goes
// offline keeps its last-known rows, greyed, "unknown · cell3 offline", until it's back.
// Deployments come from the backend (models/postmortem.rs): it also groups containers SparkBox
// didn't start, like four ranks of one launch script ("glm53 · 4 nodes · ranks 0–3 · Failed").
import { useMemo, useState, type MouseEvent, type ReactNode } from "react";
import { Boxes, ChevronDown, Disc3, EyeOff, FileText, FlaskConical, Info, Layers, List, Pause, Play, RotateCcw, Skull, SquareTerminal, Trash2 } from "lucide-react";
import { Icon, StopIcon } from "../components/Icon";
import { Table, type Column, type RowAction } from "../components/Table";
import { Badge, EmptyState, HelpTip, MiddleEllipsis, SearchField, Segmented, Spacer, StatusDot, Toolbar, ToolButton, Waiting } from "../components/ui";
import { lockedHint, useAdmin } from "../lib/admin";
import type { ContainerAction } from "../lib/api";
import { useCommands } from "../lib/commands";
import { showContextMenu, type MenuItem } from "../lib/contextMenu";
import { ackExit, isBadExit, isExitAcked, isRecentExit, useRecentExits } from "../lib/exits";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { deploymentOfContainer, groupLine, memberLine, ranFor, useDeployments, type Deployment as ClusterDeployment } from "../lib/deployments";
import { dependencyOrder, deploymentsOf, shortModel, type Deployment } from "../lib/impact";
import { copyText } from "../lib/markdown";
import { useCluster } from "../lib/store";
import { runTestContainer, testContainerOff } from "../lib/testContainer";
import { plural } from "../lib/vocabulary";
import {
  applicable, CONTAINER_ACTION_LABEL as ACTION_LABEL, inspectContainer, notApplicable, openContainerLogs, openContainerShell, rangeOf,
  runContainerAction, staleState, withLastKnown,
} from "../lib/workloadCommands";
import type { Container, NodeSnapshot } from "../lib/types";
import { useViewLink, useWindowId, useWinState, useWM, type Params } from "../wm/WindowManager";
import { relTime } from "./TasksWindow";
import "./ops.css";
import "./ai.css";
import "./workloads.css";

export { applicable } from "../lib/workloadCommands";

interface Row {
  key: string;
  node: NodeSnapshot;
  c: Container;
  deployment: Deployment | null;
  /** The backend's deployment it belongs to (with its ranks, state and postmortem). */
  cluster: ClusterDeployment | null;
  role: "head" | "worker" | "member";
  /** Its node is offline: the last-known row, state unknown. */
  stale?: boolean;
  knownAtMs?: number | null;
}

interface GroupRow {
  key: string;
  group: true;
  deployment: Deployment | null;
  cluster: ClusterDeployment | null;
  label: string;
  members: Row[];
}

type AnyRow = Row | GroupRow;
const isGroup = (r: AnyRow): r is GroupRow => "group" in r;

/** How long a container ran before it exited, when Docker kept both times. */
function ranOf(c: Container): string | null {
  return c.startedAt > 0 && c.finishedAt >= c.startedAt ? ranFor(c.finishedAt - c.startedAt) : null;
}

/** The short form other lists use: "137 · OOM-killed", "exited 1", "143 · stopped". */
export function exitReason(c: Container): string | null {
  if (c.state === "running" || c.state === "paused") return null;
  if (c.state === "restarting") return "restarting";
  if (c.oomKilled) return `${c.exitCode} · OOM-killed`;
  if (c.exitCode === 137) return "137 · killed";
  if (c.exitCode === 143) return "143 · stopped";
  if (c.exitCode === 130) return "130 · interrupted";
  return `exited ${c.exitCode}`;
}

/**
 * The Exit reason column: "Exited 1 after 54 s", "Out of memory (137) after 3 min", "Stopped (143)".
 * A member of a failed deployment points at it instead: "Exited 1 · see deployment".
 */
export function exitText(c: Container, inFailed = false): string | null {
  if (c.state === "running" || c.state === "paused") return null;
  if (c.state === "restarting") return "Restarting";
  if (c.state === "created") return "Never started";
  if (inFailed && c.exitCode !== 0) return `Exited ${c.exitCode} · see deployment`;
  const after = ranOf(c) ? ` after ${ranOf(c)}` : "";
  if (c.oomKilled) return `Out of memory (${c.exitCode})${after}`;
  if (c.exitCode === 137) return `Killed (137)${after}`;
  if (c.exitCode === 143) return "Stopped (143)";
  if (c.exitCode === 130) return "Interrupted (130)";
  if (c.exitCode === 0) return `Exited 0${after}`;
  return `Exited ${c.exitCode}${after}`;
}

const STATE_DOT: Record<string, "online" | "offline" | "connecting" | "warn" | "error"> = {
  running: "online", exited: "offline", created: "offline", restarting: "connecting", paused: "warn", dead: "error",
};

function portsText(c: Container): string {
  return c.ports.map((p) => {
    const [host, inner] = p.split("->");
    const h = host.split(":").pop();
    return inner && h !== inner.split("/")[0] ? `${h}→${inner.split("/")[0]}` : h ?? p;
  }).join(", ");
}

/** How many characters of an image to keep at the end: its last path segment ("/vllm:26.05-py3"), at most 20. */
export function imageTail(image: string): number {
  const slash = image.lastIndexOf("/");
  return Math.min(20, slash > 0 ? image.length - slash : image.length);
}

/** An image with its middle path folded: "nvcr.io/nvidia/vllm:26.05-py3" → "nvcr.io/…/vllm:26.05-py3". */
export function shortImage(image: string): string {
  const parts = image.split("/");
  return parts.length > 2 ? `${parts[0]}/…/${parts[parts.length - 1]}` : image;
}

/**
 * The Image column's cut: the whole tag always shows (":glm53-flash-arm64-cu130"), else the last
 * path segment; the start of the repo shows before the one ellipsis: "vllm/vllm-o…:glm53-flash-arm64-cu130".
 */
export function imageKeepEnd(image: string): number {
  const slash = image.lastIndexOf("/");
  const colon = image.lastIndexOf(":");
  if (colon > slash) return image.length - colon;
  return Math.min(24, slash > 0 ? image.length - slash : image.length);
}

/** "6 containers in 2 groups" */
export function groupsFooter(containers: number, groups: number): string {
  return `${plural(containers, "container")} in ${plural(groups, "group")}`;
}

export function Containers({ params }: { params: Params }) {
  const cluster = useCluster();
  const admin = useAdmin();
  const winId = useWindowId();
  const [nodeFilter, setNodeFilter] = useWinState("node", params.node ?? "all");
  useViewLink(params, () => setNodeFilter(params.node ?? "all"));
  const [grouped, setGrouped] = useWinState("grouped", params.view === "deployments");
  const wm = useWM();
  // A failed deployment's card opens this window on its Deployments view.
  useViewLink(params, () => params.view === "deployments" && setGrouped(true));
  const [showStopped, setShowStopped] = useWinState("showStopped", true);
  const [text, setText] = useState("");
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const recentExits = useRecentExits();

  const known = useDeployments();
  const deployments = useMemo(() => (cluster ? deploymentsOf(cluster, known) : []), [cluster, known]);
  const needle = text.trim().toLowerCase();
  const listed = useMemo(() => (cluster ? withLastKnown(cluster) : []), [cluster]);
  const rows: Row[] = useMemo(() => {
    const observed: Row[] = listed
    .flatMap(({ node, stale, knownAtMs }) => node.containers.map((c) => {
      const d = deployments.find((d) => d.members.some((m) => m.node === node.id && m.id === c.id)) ?? null;
      const role = d?.members.find((m) => m.node === node.id && m.id === c.id)?.role ?? "member";
      return { key: `${node.id}/${c.id}`, node, c, deployment: d, cluster: deploymentOfContainer(known, node.id, c.id), role, stale, knownAtMs };
    }));
    for (const d of known) for (const m of d.members) {
      if ((m.state !== "unknown" && m.state !== "missing") || observed.some((r) => r.node.id === m.node && r.cluster?.id === d.id)) continue;
      const node = listed.find((l) => l.node.id === m.node)?.node;
      if (!node) continue;
      const id = m.id || `unknown-${d.id}-${m.node}`;
      const c: Container = { id, name: m.container || `Rank ${m.rank ?? "?"}`, image: d.image, state: m.state, startedAt: m.startedMs / 1000, finishedAt: 0, exitCode: 0, cpuPct: 0, mem: 0, gpuMem: 0, ports: [], project: null, command: "" };
      observed.push({ key: `${m.node}/${id}`, node, c, cluster: d, deployment: null, role: m.role, stale: true, knownAtMs: m.lastSeenMs });
    }
    return observed.filter((r) => nodeFilter === "all" || r.node.id === nodeFilter)
    .filter((r) => showStopped || r.stale || r.c.state === "running" || r.c.state === "paused" || r.c.state === "restarting")
    .filter((r) => !needle || `${r.c.name} ${r.c.image} ${r.c.project ?? ""} ${r.node.name} ${r.deployment?.id ?? ""}`.toLowerCase().includes(needle));
  }, [listed, deployments, known, nodeFilter, showStopped, needle]);

  const display: AnyRow[] = useMemo(() => {
    if (!grouped || !cluster) return rows;
    const out: AnyRow[] = [];
    const byGroup = new Map<string, Row[]>();
    for (const r of rows) {
      const k = r.cluster?.id ?? r.deployment?.id ?? "";
      byGroup.set(k, [...(byGroup.get(k) ?? []), r]);
    }
    const keys = [...byGroup.keys()].sort((a, b) => (a === "" ? 1 : b === "" ? -1 : a.localeCompare(b)));
    for (const k of keys) {
      const cd = byGroup.get(k)![0].cluster;
      // The backend's deployments list their members by rank, head first; others head first.
      const members = cd
        ? [...byGroup.get(k)!].sort((a, b) => cd.members.findIndex((m) => m.node === a.node.id && a.c.id.startsWith(m.id.slice(0, 12))) - cd.members.findIndex((m) => m.node === b.node.id && b.c.id.startsWith(m.id.slice(0, 12))))
        : dependencyOrder(byGroup.get(k)!, "start");
      const d = members[0].deployment;
      const label = cd ? groupLine(cd) : d ? `${d.id} · ${members.length}` : `Not in a deployment · ${members.length}`;
      out.push({ key: `g:${k}`, group: true, deployment: d, cluster: cd, label, members });
      out.push(...members);
    }
    for (const d of known) {
      if (byGroup.has(d.id) || d.state !== "unknown" || (nodeFilter !== "all" && !d.nodes.includes(nodeFilter)) || (needle && !`${d.id} ${d.label} ${d.nodes.join(" ")}`.toLowerCase().includes(needle))) continue;
      out.push({ key: `g:${d.id}`, group: true, deployment: null, cluster: d, label: groupLine(d), members: [] });
    }
    return out;
  }, [grouped, rows, cluster, known, nodeFilter, needle]);

  // A container the other SparkBox started (its sparkbox.instance label isn't this build's):
  // read it, never act on it (P1.2). The glm53 containers have no such label, so they're ours.
  const myInstance = import.meta.env.DEV ? "dev" : "release";
  const otherSparkbox = (c: Container) => {
    const inst = c.labels?.["sparkbox.instance"];
    return inst != null && inst !== myInstance;
  };
  const OTHER_HINT = "managed by the other SparkBox";
  // Rows of an offline node, or the other SparkBox's, can be selected and read, never acted on.
  const chosen = rows.filter((r) => selection.has(r.key) && !r.stale && !otherSparkbox(r.c));
  const picked = rows.filter((r) => selection.has(r.key));
  const one = chosen.length === 1 ? chosen[0] : null;

  // ⌘T is the menu bar's New Terminal Tab; here, with a running container selected, it opens a shell in it.
  useCommands(one && one.c.state === "running" ? [{
    id: "term:new-tab", title: `Shell in ${one.c.name}`, group: "Containers", icon: SquareTerminal, needsAdmin: true,
    subtitle: `On ${one.node.name}`, run: () => openContainerShell(one.node, one.c),
  }] : [], [one?.key, one?.c.state], { scope: winId });

  if (!cluster) return <Waiting />;
  const nowS = cluster.tsMs / 1000;
  const locked = admin ? undefined : lockedHint("change containers");
  const can = (a: ContainerAction) => chosen.some((r) => applicable(a, r.c));
  /** Why a toolbar action is greyed, in words that name the state. */
  const whyNot = (a: ContainerAction): string | undefined => {
    if (can(a)) return undefined;
    if (!picked.length) return "Select a container";
    if (!chosen.length) return picked.some((r) => otherSparkbox(r.c)) ? OTHER_HINT : `${picked[0].node.name} is offline`;
    return notApplicable(a, chosen.map((r) => r.c));
  };

  const onSelection = (keys: Set<string>) => {
    // Choosing a group row chooses its containers.
    const next = new Set<string>();
    for (const k of keys) {
      const g = display.find((r) => r.key === k);
      if (g && isGroup(g)) g.members.forEach((m) => next.add(m.key));
      else next.add(k);
    }
    setSelection(next);
  };

  // One request per action and set of containers: a triple-click asks once and runs once.
  const act = (action: ContainerAction, targets: Row[] = chosen) => runContainerAction(action, targets, { origin: winId });

  /** An exit worth a red word: bad, in the last 24 h, and not hidden. */
  const freshExit = (r: Row) => isBadExit(r.c) && isRecentExit(r.c, nowS) && !isExitAcked(r.node.id, r.c.id, r.c.finishedAt);
  /** Its deployment failed: the deployment carries the story (its card, its postmortem). */
  const inFailed = (r: Row) => r.cluster?.state === "failed" && r.cluster.members.length > 1;

  const offline = (r: Row) => (r.stale ? r.c.state === "missing" ? "Container was not found in the latest report" : `${r.node.name} is ${r.node.status === "online" ? "not reporting" : "offline"}` : false);
  const actions = (r: Row): RowAction[] => [
    { label: "Logs", shortcut: "⌘L", disabled: offline(r), run: () => openContainerLogs(r.node, r.c) },
    { label: "Shell", shortcut: "⌘T", disabled: offline(r) || (otherSparkbox(r.c) ? OTHER_HINT : false) || (r.c.state !== "running" ? `It isn't running (${r.c.state})` : false), run: () => void openContainerShell(r.node, r.c) },
    { label: "Inspect", shortcut: "⌘I", disabled: offline(r), run: () => inspectContainer(r.node, r.c) },
  ];

  const menu = (row: AnyRow, e: MouseEvent, keys: Set<string>) => {
    const all = isGroup(row) ? row.members : keys.has(row.key) ? rows.filter((r) => keys.has(r.key)) : [row];
    if (!all.length) return;
    const targets = all.filter((t) => !t.stale && !otherSparkbox(t.c));
    const single = all.length === 1 ? all[0] : null;
    const item = (a: ContainerAction, icon?: typeof Play): MenuItem => ({
      label: targets.length > 1 ? `${ACTION_LABEL[a]} ${targets.filter((t) => applicable(a, t.c)).length} containers…` : `${ACTION_LABEL[a]}${a === "start" || a === "unpause" ? "" : "…"}`,
      icon,
      danger: a === "kill" || a === "remove",
      hint: !admin ? "Admin" : undefined,
      disabled: !targets.length ? (all.some((t) => otherSparkbox(t.c)) ? OTHER_HINT : `${all[0].node.name} is offline`) : !targets.some((t) => applicable(a, t.c)) ? notApplicable(a, targets.map((t) => t.c)) : false,
      run: () => void act(a, targets),
    });
    const icons: Record<string, typeof Play> = { Logs: FileText, Shell: SquareTerminal, Inspect: Info };
    showContextMenu(e, [
      ...(single ? actions(single).map((a): MenuItem => ({ label: a.label, shortcut: a.shortcut, icon: icons[a.label], disabled: a.disabled, run: a.run })) : []),
      ...(single ? [{ separator: true } as const] : []),
      item("start", Play), item("stop", StopIcon), item("restart", RotateCcw), item("kill", Skull),
      item("pause", Pause), item("unpause", Play), item("remove", Trash2),
      ...(single && isBadExit(single.c)
        ? [{ separator: true } as const, {
            label: "Hide this exit", icon: EyeOff,
            disabled: isExitAcked(single.node.id, single.c.id, single.c.finishedAt) ? "Already hidden" : false,
            run: () => ackExit(single.node.id, single.c.id, single.c.finishedAt),
          }]
        : []),
      { separator: true },
      { label: "Copy ID", disabled: !single, run: () => single && void copyText(single.c.id) },
      { label: "Copy name", disabled: !single, run: () => single && void copyText(single.c.name) },
    ]);
  };

  const cell = (r: AnyRow, f: (r: Row) => ReactNode): ReactNode => (isGroup(r) ? null : f(r));
  /** A live number: a dash while the node is offline (the last one would be a guess). */
  const live = (r: AnyRow, f: (r: Row) => ReactNode): ReactNode => cell(r, (x) => (x.stale ? <span className="muted">—</span> : f(x)));
  const sortable = !grouped;
  const columns: Column<AnyRow>[] = [
    {
      // Grouped, a member's line ("glm53 · rank 0 · head") needs the room.
      key: "name", label: "Name", priority: 1, minWidth: grouped ? 200 : 150, sort: sortable ? (r) => (isGroup(r) ? r.label : r.c.name) : undefined,
      // A group's line ("glm53 · 4 nodes · ranks 0–3 · Failed") takes the Node cell too: its members name their nodes.
      absorb: (r) => (isGroup(r) ? ["node"] : null),
      title: (r) => (isGroup(r)
        ? `${r.label}${r.cluster?.inferred ? `\nLikely one deployment: ${r.cluster.how}` : ""}${r.cluster?.state === "unknown" ? `\n${r.cluster.members.map((m) => `${m.node}: ${m.state}${m.lastSeenMs ? ` · last observed ${fmtDateTime(m.lastSeenMs)}` : ""}`).join("\n")}` : ""}${r.cluster?.postmortem?.title ? `\nLikely cause: ${r.cluster.postmortem.title}` : ""}`
        : `${r.c.name}${r.deployment ? ` · ${r.deployment.id} (${r.role})` : ""}`),
      render: (r) => {
        if (isGroup(r)) return <span className="ops-group-label"><Icon icon={Layers} size={12} /> {r.label}</span>;
        const member = grouped ? r.cluster?.members.find((m) => m.node === r.node.id && r.c.id.startsWith(m.id.slice(0, 12))) : undefined;
        return (
          <span className={`ops-name-cell${grouped ? " child" : ""}`}>
            <span className="strong ai-name-whole">{r.c.name}</span>
            {member ? <span className="muted ai-member-line">· {memberLine(member, false)}</span> : r.deployment && r.role !== "member" && <span className="muted ai-member-line">· {r.role}</span>}
            {otherSparkbox(r.c) && <span className="muted ai-member-line" title={OTHER_HINT}>· {OTHER_HINT}</span>}
          </span>
        );
      },
    },
    {
      key: "node", label: "Node", width: 90, priority: 2, sort: sortable ? (r) => (isGroup(r) ? "" : r.node.name) : undefined,
      render: (r) => (isGroup(r) ? <span className="muted">{rangeOf(cluster, r.cluster?.nodes ?? [...new Set(r.members.map((m) => m.node.id))])}</span> : r.node.name),
    },
    {
      // Wide enough for a whole tag (":glm53-flash-arm64-cu130") after the start of the repo.
      key: "image", label: "Image", priority: 3, width: 230, minWidth: 220, fit: "flex", ellipsis: "middle", sort: sortable ? (r) => (isGroup(r) ? "" : r.c.image) : undefined,
      title: (r) => (isGroup(r) ? r.deployment?.model ?? "" : r.c.image),
      render: (r) => (isGroup(r)
        ? (r.deployment?.model ? <span className="strong">{shortModel(r.deployment.model)}</span> : null)
        : <MiddleEllipsis className="mono" text={r.c.image} keepEnd={imageKeepEnd(r.c.image)} title={r.c.image} />),
    },
    {
      key: "state", label: "State", width: 110, minWidth: 110, fit: "content", priority: 1, sort: sortable ? (r) => (isGroup(r) ? "" : r.c.state) : undefined,
      render: (r) => {
        if (!isGroup(r) && r.stale && r.c.state === "missing") return <span className="muted">Not found</span>;
        if (!isGroup(r) && r.stale) return <span className="muted ops-stale" title={`Last reported ${r.c.state}; ${r.node.name} isn't reporting, so its state now is unknown${r.knownAtMs != null ? ` · Last observed ${fmtDateTime(r.knownAtMs)}` : ''}`}>{staleState(r.node)}</span>;
        if (!isGroup(r)) return <StatusDot status={STATE_DOT[r.c.state] ?? "offline"} text={r.c.state} />;
        const expected = r.cluster?.members;
        const up = expected ? expected.filter((m) => m.state === "running").length : r.members.filter((m) => !m.stale && m.c.state === "running").length;
        const unknown = expected ? expected.filter((m) => m.state === "unknown").length : r.members.filter((m) => m.stale).length;
        return <span className="muted">{up}/{expected?.length ?? r.members.length} running{unknown > 0 ? ` · ${unknown} unknown` : ""}</span>;
      },
    },
    {
      key: "health", label: "Health", width: 90, fit: "content", priority: 2, sort: sortable ? (r) => (isGroup(r) ? "" : r.c.health ?? "") : undefined,
      render: (r) => cell(r, (x) => {
        if (x.stale || x.c.state !== "running" || !x.c.health) return <span className="muted">—</span>;
        // Healthy is quiet; only a failing check is coloured.
        if (x.c.health === "healthy") return <span className="muted">healthy</span>;
        return <Badge tone={x.c.health === "starting" ? "info" : "crit"}>{x.c.health}</Badge>;
      }),
    },
    {
      key: "exit", label: "Exit reason", help: "ops.containers.exits", width: 150, fit: "content", priority: 2, sort: sortable ? (r) => (isGroup(r) ? "" : exitText(r.c, inFailed(r)) ?? "") : undefined,
      title: (r) => (isGroup(r) || r.c.state === "running" || !r.c.finishedAt ? "" : `Exited ${fmtDateTime(r.c.finishedAt * 1000)} (${relTime(r.c.finishedAt * 1000, nowS * 1000)})${isExitAcked(r.node.id, r.c.id, r.c.finishedAt) ? " · hidden" : ""}`),
      render: (r) => {
        if (isGroup(r)) {
          const pm = r.cluster?.postmortem;
          return r.cluster?.state === "failed" ? <span className="muted" title={pm?.title ?? undefined}>{pm?.title ? `Likely: ${pm.title}` : `Exited ${r.cluster.exitCode ?? ""}`.trim()}</span> : null;
        }
        if (r.stale) return <span className="muted">—</span>;
        const why = exitText(r.c, inFailed(r));
        if (!why) return <span className="muted">—</span>;
        // Only an exit from the last 24 h that nobody has hidden is red; older ones, and members
        // of a failed deployment (it has the red pill), are history.
        return <span className={freshExit(r) && !inFailed(r) ? "crit-text" : "muted"}>{why}</span>;
      },
    },
    { key: "restarts", label: "Restarts", short: "Rst", width: 74, align: "right", priority: 3, sort: sortable ? (r) => (isGroup(r) ? 0 : r.c.restartCount ?? 0) : undefined, render: (r) => cell(r, (x) => <span className={`num${(x.c.restartCount ?? 0) > 3 ? " crit-text" : ""}`}>{x.c.restartCount ?? 0}</span>) },
    { key: "cpu", label: "CPU", width: 60, align: "right", priority: 1, sort: sortable ? (r) => (isGroup(r) ? 0 : r.c.cpuPct) : undefined, render: (r) => live(r, (x) => (x.c.state === "running" ? <span className="num">{x.c.cpuPct.toFixed(0)}%</span> : <span className="muted">—</span>)) },
    { key: "mem", label: "Memory", short: "Mem", width: 84, align: "right", priority: 2, sort: sortable ? (r) => (isGroup(r) ? 0 : r.c.mem) : undefined, render: (r) => live(r, (x) => (x.c.state === "running" ? <span className="num">{fmtBytes(x.c.mem)}</span> : <span className="muted">—</span>)) },
    { key: "gpu", label: "GPU memory", short: "GPU", width: 96, align: "right", priority: 2, sort: sortable ? (r) => (isGroup(r) ? 0 : r.c.gpuMem) : undefined, render: (r) => live(r, (x) => (x.c.gpuMem > 0 ? <span className="num">{fmtBytes(x.c.gpuMem)}</span> : <span className="muted">—</span>)) },
    { key: "ports", label: "Ports", width: 110, fit: "content", priority: 3, render: (r) => cell(r, (x) => (portsText(x.c) ? <span className="mono">{portsText(x.c)}</span> : "—")) },
    {
      // Running: when it started. Exited: when it exited (a start two days ago says nothing about now).
      key: "started", label: "Started", width: 104, fit: "content", priority: 3, sort: sortable ? (r) => (isGroup(r) ? 0 : r.c.state === "running" ? r.c.startedAt : r.c.finishedAt || r.c.startedAt) : undefined,
      title: (r) => (isGroup(r) || !r.c.startedAt ? "" : `Started ${fmtDateTime(r.c.startedAt * 1000)}${r.c.state !== "running" && r.c.finishedAt ? `\nExited ${fmtDateTime(r.c.finishedAt * 1000)}` : ""}`),
      render: (r) => cell(r, (x) => {
        if (x.c.state !== "running" && x.c.state !== "paused" && x.c.finishedAt) return <span className="muted">Exited {relTime(x.c.finishedAt * 1000, nowS * 1000)}</span>;
        return x.c.startedAt ? relTime(x.c.startedAt * 1000, nowS * 1000) : "—";
      }),
    },
  ];

  const all = listed.flatMap((l) => l.node.containers);
  const running = listed.filter((l) => !l.stale).flatMap((l) => l.node.containers).filter((c) => c.state === "running").length;
  const offlineNodes = listed.filter((l) => l.stale && l.node.containers.length).map((l) => l.node.name);
  const foot = (
    <span className="containers-foot">
      {!grouped && `${running} of ${all.length} running`}
      {offlineNodes.length > 0 && <span className="muted">{!grouped && " · "}{offlineNodes.join(", ")} offline</span>}
      {recentExits.length > 0 && <span className="muted"> · {recentExits.length} exited today</span>}
    </span>
  );
  // Run test container: SparkBox's own busybox on the one node the Node filter picks (Live only).
  const testNode = cluster.nodes.find((n) => n.id === nodeFilter);
  const testOff = testContainerOff(!!cluster.demo, testNode ? nodeFilter : "all");
  const groups = display.filter(isGroup).length;
  const bulk = (a: ContainerAction) => chosen.filter((r) => applicable(a, r.c)).length;
  const label = (a: ContainerAction) => (chosen.length > 1 && bulk(a) > 0 ? `${ACTION_LABEL[a]} ${bulk(a)}` : ACTION_LABEL[a]);

  return (
    <div className="col fill ops-window containers-window">
      <Toolbar label="Containers">
        <ToolButton icon={Play} label={label("start")} title={whyNot("start")} disabled={!can("start")} locked={!admin} onClick={() => act("start")} />
        <ToolButton icon={StopIcon} label={`${label("stop")}…`} title={whyNot("stop")} disabled={!can("stop")} locked={!admin} onClick={() => act("stop")} />
        <ToolButton icon={RotateCcw} label={`${label("restart")}…`} title={whyNot("restart")} disabled={!can("restart")} locked={!admin} onClick={() => act("restart")} />
        <ToolButton icon={Trash2} label={`${label("remove")}…`} danger priority={2} title={whyNot("remove") ?? "Delete stopped containers (their images stay)"} disabled={!can("remove")} locked={!admin} onClick={() => act("remove")} />
        <button type="button" className="tool" disabled={!chosen.length} aria-label="More actions" title={locked ?? "Force-stop, pause or resume"}
          onClick={(e) => showContextMenu(e, (["kill", "pause", "unpause"] as ContainerAction[]).map((a) => ({
            label: `${label(a)}${a === "unpause" ? "" : "…"}`,
            icon: a === "kill" ? Skull : a === "pause" ? Pause : Play,
            danger: a === "kill",
            hint: !admin ? "Admin" : undefined,
            disabled: whyNot(a) ?? false,
            run: () => void act(a),
          })))}>
          <span className="tool-label">More</span><Icon icon={ChevronDown} size={12} />
        </button>
        <ToolButton icon={Disc3} label="Images…" priority={2} onClick={() => wm.open("images", nodeFilter !== "all" ? { node: nodeFilter } : {})} title="The images on each node, their sizes and which containers use them" />
        <ToolButton icon={FlaskConical} label="Run test container" priority={3} locked={!admin} disabled={!!testOff}
          title={testOff ?? `Start sparkbox-test-busybox (busybox, sleeps an hour) on ${testNode?.name ?? nodeFilter}`}
          onClick={() => runTestContainer(nodeFilter, testNode?.name ?? nodeFilter)} />
        <HelpTip topic="ops.containers.test" />
        <Spacer />
        <Segmented<"list" | "grouped"> value={grouped ? "grouped" : "list"} onChange={(v) => setGrouped(v === "grouped")} label="Arrange" options={[
          { value: "list", label: "List", icon: List, title: "One row per container" },
          { value: "grouped", label: "Deployments", icon: Layers, title: "Grouped by deployment or Compose project; actions run in dependency order" },
        ]} />
        <HelpTip topic="ops.containers.views" />
        <select value={showStopped ? "all" : "running"} onChange={(e) => setShowStopped(e.target.value === "all")} aria-label="Show">
          <option value="all">All states</option>
          <option value="running">Running only</option>
        </select>
        <select value={nodeFilter} onChange={(e) => setNodeFilter(e.target.value)} aria-label="Node">
          <option value="all">All nodes</option>
          {cluster.nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
        </select>
        <SearchField value={text} onChange={setText} placeholder="Filter" count={rows.length} />
      </Toolbar>
      {all.length === 0 && !display.length ? (
        <EmptyState icon={Boxes} title="No containers" body="Containers started with docker on any node show up here, with what they serve and how they're doing." />
      ) : (
        <Table<AnyRow>
          key={grouped ? "grouped" : "flat"}
          tableId="containers"
          ariaLabel="Containers"
          columns={columns}
          rows={display}
          rowKey={(r) => r.key}
          selectionMode="multi"
          selection={selection}
          onSelectionChange={onSelection}
          onActivate={(r) => !isGroup(r) && !r.stale && inspectContainer(r.node, r.c)}
          rowActions={(r) => (isGroup(r) ? [] : actions(r))}
          onRowContextMenu={(r, e, keys) => menu(r, e, keys)}
          initialSort={grouped ? null : { key: "node", dir: 1 }}
          rowClass={(r) => (isGroup(r) ? "ops-group-row"
            : r.stale ? "row-dim row-stale"
            : (r.c.oomKilled || r.c.state === "dead") && freshExit(r) ? "row-crit"
            : r.c.health === "unhealthy" && r.c.state === "running" ? "row-crit"
            : r.c.state === "restarting" ? "row-warn"
            : r.c.state !== "running" ? "row-dim" : undefined)}
          footer={grouped ? undefined : foot}
          itemNoun={["container", "containers"]}
          empty="No containers match."
          onKeyAction={(a, rs) => a === "delete" && void act("remove", rs.filter((r): r is Row => !isGroup(r)))}
        />
      )}
      {grouped && all.length > 0 && (
        // Grouped, the table's own count would count group rows too: this one counts containers.
        <div className="table-footer" role="status">
          <span>{groupsFooter(rows.length, groups)}{chosen.length > 0 && ` · ${chosen.length} selected`}</span>
          <span className="table-footer-slot">{foot}</span>
        </div>
      )}
      {one && (
        <div className="detail-strip mono" title={one.c.command}>
          {one.c.id.slice(0, 12)} · {one.c.command}
        </div>
      )}
    </div>
  );
}
