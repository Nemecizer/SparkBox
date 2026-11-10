// Docker images on every node, read only: what's taking the disk, what uses it, and what
// hasn't run for months. Each node answers `docker image ls` and `docker system df` (both
// reads, logged as SparkBox's own checks); which containers use an image, and when one last
// ran, come from the snapshot. Demo shows the simulated containers' images at their real sizes.
import { useCallback, useEffect, useMemo, useState } from "react";
import { Layers, RefreshCw } from "lucide-react";
import { Table, type Column } from "../components/Table";
import { EmptyState, HelpTip, InfoTip, Notice, SearchField, Segmented, Spacer, Toolbar, ToolButton } from "../components/ui";
import { inTauri } from "../lib/api";
import { useDataSource } from "../lib/dataSource";
import { showError } from "../lib/errors";
import { fmtBytes, fmtDateTime, fmtRelative } from "../lib/format";
import { opsApi, type DockerImage, type NodeImages } from "../lib/opsApi";
import { useCluster } from "../lib/store";
import { useWinState, type Params } from "../wm/WindowManager";
import "./ops.css";

type Show = "all" | "unused" | "dangling";

const DAY = 86_400_000;
/** Not run for this long (or never, here) counts as unused. */
export const UNUSED_AFTER_DAYS = 30;

export const imageRef = (i: Pick<DockerImage, "repository" | "tag" | "id" | "dangling">) =>
  i.dangling ? `<none> (${i.id})` : `${i.repository}:${i.tag}`;

/** No container running it, and none has run it for 30 days (or ever, on this node). */
export function isUnused(i: DockerImage, now: number): boolean {
  return i.running === 0 && (i.lastUsedMs === null || now - i.lastUsedMs > UNUSED_AFTER_DAYS * DAY);
}

/**
 * "gx10-0000: 6 images · 85.6 GiB · 2 unused for 186 days · 3 never used here" (the node's name
 * first): images whose containers last ran long ago apart from ones no container here ever ran.
 */
export function nodeSummary(name: string, n: NodeImages, now: number): string {
  if (n.error) return `${name}: ${n.error.replace(new RegExp(`^${name}: `), "")}`;
  const count = n.images.length;
  const parts = [`${count} image${count === 1 ? "" : "s"}`, fmtBytes(n.total)];
  const unused = n.images.filter((i) => isUnused(i, now));
  const ages = unused.filter((i) => i.lastUsedMs !== null).map((i) => Math.floor((now - i.lastUsedMs!) / DAY));
  if (ages.length) parts.push(`${ages.length} unused for ${Math.min(...ages)}${new Set(ages).size > 1 ? "+" : ""} days`);
  const never = unused.length - ages.length;
  if (never) parts.push(`${never} never used here`);
  return `${name}: ${parts.join(" · ")}`;
}

/** "glm53", "glm53, workbench-proxy", "adoring_northcutt +2". */
function usedByText(names: string[]): string {
  if (names.length <= 2) return names.join(", ");
  return `${names[0]} +${names.length - 1}`;
}

export function Images({ params }: { params: Params }) {
  const cluster = useCluster();
  const source = useDataSource();
  const [data, setData] = useState<NodeImages[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [show, setShow] = useWinState<Show>("show", "all");
  const [node, setNode] = useWinState<string>("node", params.node ?? "all");
  const [text, setText] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const now = cluster?.tsMs || Date.now();
  const nodeName = (id: string) => cluster?.nodes.find((n) => n.id === id)?.name ?? id;

  const load = useCallback(async () => {
    if (!inTauri) return;
    setBusy(true);
    try {
      setData(await opsApi.imagesList());
    } catch (e) {
      showError(e, { action: "Read the nodes' images" });
    } finally {
      setBusy(false);
    }
  }, []);
  // Read once on open, and again when the data source changes (Demo and Live have their own).
  useEffect(() => {
    setData(null);
    void load();
  }, [load, source]);
  useEffect(() => {
    if (params.node) setNode(params.node);
  }, [params.node, params.nav]);

  const all = useMemo(() => (data ?? []).flatMap((n) => n.images), [data]);
  const needle = text.trim().toLowerCase();
  const rows = useMemo(() => all.filter((i) => {
    if (node !== "all" && i.node !== node) return false;
    if (show === "unused" && !isUnused(i, now)) return false;
    if (show === "dangling" && !i.dangling) return false;
    if (needle && !`${imageRef(i)} ${i.id} ${i.usedBy.join(" ")}`.toLowerCase().includes(needle)) return false;
    return true;
  }), [all, node, show, needle, now]);
  const shownNodes = (data ?? []).filter((n) => node === "all" || n.node === node);
  const total = shownNodes.reduce((s, n) => s + n.total, 0);
  const reclaimable = shownNodes.reduce((s, n) => s + (n.reclaimable ?? 0), 0);

  const columns: Column<DockerImage>[] = [
    {
      key: "image", label: "Image", minWidth: 220, priority: 1, ellipsis: "middle", fit: "flex",
      sort: (i) => imageRef(i), title: (i) => `${imageRef(i)}\nid ${i.id}`,
      render: (i) => <span className={`mono${i.dangling ? " muted" : ""}`}>{imageRef(i)}</span>,
    },
    {
      key: "node", label: "Node", width: 104, priority: 1, sort: (i) => nodeName(i.node),
      render: (i) => nodeName(i.node), narrow: (i) => nodeName(i.node).replace(/^cell(\d+)$/, "c$1").replace(/^gx10-.*/, "gx10"),
    },
    { key: "size", label: "Size", width: 84, align: "right", priority: 1, sort: (i) => i.size, render: (i) => <span className="num">{fmtBytes(i.size)}</span> },
    {
      key: "created", label: "Created", width: 112, priority: 2, sort: (i) => i.createdMs ?? 0,
      title: (i) => (i.createdMs ? fmtDateTime(i.createdMs) : "Docker didn't say"),
      render: (i) => (i.createdMs ? fmtRelative(i.createdMs, now) : <span className="muted">—</span>),
    },
    {
      key: "used", label: "Last used", width: 118, priority: 1, sort: (i) => (i.running ? now : i.lastUsedMs ?? -1),
      title: (i) => (i.running ? "A container from it is running" : i.lastUsedMs ? `Its last container stopped ${fmtDateTime(i.lastUsedMs)}` : "No container on this node uses it"),
      render: (i) => i.running ? "running now" : i.lastUsedMs ? <span className={isUnused(i, now) ? "ops-images-old" : undefined}>{fmtRelative(i.lastUsedMs, now)}</span> : <span className="muted">never here</span>,
    },
    {
      key: "usedBy", label: "Used by", minWidth: 120, priority: 2, sort: (i) => i.usedBy.length,
      title: (i) => (i.usedBy.length ? i.usedBy.join("\n") : "No container"),
      render: (i) => (i.usedBy.length ? <span className="mono">{usedByText(i.usedBy)}</span> : <span className="muted">—</span>),
    },
    {
      key: "dangling", label: "Dangling", width: 84, priority: 3, sort: (i) => (i.dangling ? 1 : 0),
      title: (i) => (i.dangling ? "No tag: left behind by a pull or a build" : ""),
      render: (i) => (i.dangling ? <span className="ops-tag">dangling</span> : ""),
    },
  ];

  const errors = (data ?? []).filter((n) => n.error && (node === "all" || n.node === node));
  return (
    <div className="col fill ops-window ops-images">
      <Toolbar label="Images">
        <Segmented<Show> value={show} onChange={setShow} label="Show" options={[
          { value: "all", label: "All" },
          { value: "unused", label: "Unused", title: `No container has run it for ${UNUSED_AFTER_DAYS} days, or ever on that node` },
          { value: "dangling", label: "Dangling", title: "Untagged images left behind by pulls and builds" },
        ]} />
        <HelpTip topic="ops.images.show" />
        <Spacer />
        <select value={node} onChange={(e) => setNode(e.target.value)} aria-label="Node">
          <option value="all">All nodes</option>
          {(cluster?.nodes ?? []).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
        </select>
        <SearchField value={text} onChange={setText} placeholder="Filter images" count={rows.length} />
        <ToolButton icon={RefreshCw} label={busy ? "Reading…" : "Refresh"} priority={2} disabled={busy} onClick={() => void load()} title="Read the images again from every node" />
        <InfoTip label="About images" topic="ops.images.about">
          <p>Each node answers <code>docker image ls</code> and <code>docker system df</code>. Nothing here changes a node.</p>
          <p>Used by and Last used come from the containers on the node: an image no container has run for {UNUSED_AFTER_DAYS} days, or ever, counts as unused.</p>
          <p>Sizes are Docker's: layers two images share are counted in both, so the column can add up to more than Docker's total.</p>
        </InfoTip>
      </Toolbar>
      {data && data.length > 0 && (
        <ul className="ops-images-nodes" aria-label="Images per node">
          {(data ?? []).filter((n) => node === "all" || n.node === node).map((n) => (
            <li key={n.node} className={n.error ? "muted" : undefined}>
              <button type="button" className="ops-images-node" onClick={() => setNode(node === n.node ? "all" : n.node)} aria-pressed={node === n.node}
                title={node === n.node ? "Show every node" : `Show only ${nodeName(n.node)}`}>
                {nodeSummary(nodeName(n.node), n, now)}
              </button>
            </li>
          ))}
        </ul>
      )}
      {errors.length > 0 && errors.length === shownNodes.length && (
        <Notice tone="info">No node answered with its images. {errors[0].error}</Notice>
      )}
      {data && !all.length && !errors.length ? (
        <EmptyState icon={Layers} title="No images" body="None of the nodes has a Docker image." />
      ) : (
        <>
          <Table
            columns={columns}
            rows={rows}
            rowKey={(i) => `${i.node}:${i.id}:${i.repository}:${i.tag}`}
            selected={selected}
            onSelect={setSelected}
            tableId="images"
            ariaLabel="Docker images"
            initialSort={{ key: "size", dir: -1 }}
            empty={!data ? "Reading each node's images…" : show === "unused" ? "Every image here has run in the last 30 days." : show === "dangling" ? "No dangling images." : "Nothing matches these filters."}
          />
          <div className="table-footer" role="status">
            <span>
              {rows.length} image{rows.length === 1 ? "" : "s"}
              {data ? ` · ${fmtBytes(total)} on ${shownNodes.filter((n) => !n.error).length} node${shownNodes.filter((n) => !n.error).length === 1 ? "" : "s"}` : ""}
              {reclaimable > 0 ? ` · Docker could free ${fmtBytes(reclaimable)}` : ""}
            </span>
          </div>
        </>
      )}
    </div>
  );
}
