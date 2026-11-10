// Readiness of every node before the first load: Docker, disk, the vLLM image, the fabric, and
// the GPU seen from inside the image. Those last readings start short containers on each node,
// so the check is a change: it needs Admin mode, and says so before it runs. The last result
// stays, with its age, across a restart: it's what the nodes said then, not now.
import { useState } from "react";
import { Stethoscope } from "lucide-react";
import { Table, type Column } from "../../components/Table";
import { EmptyState, HelpTip, Notice, Toolbar, ToolButton } from "../../components/ui";
import { requestAdmin, useAdmin } from "../../lib/admin";
import { api } from "../../lib/api";
import { scopedKey } from "../../lib/dataSource";
import { fmtBytes } from "../../lib/format";
import { inNames } from "../../lib/modelsApi";
import { useCluster } from "../../lib/store";
import { toast } from "../../lib/toasts";
import type { FabricIf, NodeCheck } from "../../lib/types";
import { Ago, errText } from "./shared";
import { ImagePreparation } from "./ImagePreparation";
import { EarlyoomPreparation } from "./EarlyoomPreparation";

/** What Check nodes does, as the button's title and the empty page say it. */
export const CHECK_TEXT = "Check nodes starts short containers on each node to read the vLLM version and see the GPU from inside the image (needs Admin). It also looks at Docker, free disk, the vLLM image and the fabric links.";

/** Whether the vLLM image is ready: a load starts it as it is, and fetches nothing. */
export function imageWord(c: Pick<NodeCheck, "imagePresent">): string {
  return c.imagePresent ? "on this node" : "not on this node; a load won't fetch it";
}

/** One fabric half's jumbo and RDMA state: "MTU 9000 · RDMA ACTIVE". sysfs writes "4: ACTIVE". */
export function fabricState(f: Pick<FabricIf, "mtu" | "rdmaState">): string {
  return `MTU ${f.mtu || "?"} · RDMA ${f.rdmaState?.replace(/^\d+:\s*/, "") || "unknown"}`;
}

/** What the full check saw of the GPU inside the image. ok null: it didn't look. */
export function gpuWord(c: Pick<NodeCheck, "gpuInContainer" | "imagePresent">): { ok: boolean | null; text: string } {
  if (c.gpuInContainer == null) return { ok: null, text: c.imagePresent ? "not checked" : "not checked: the image isn't on this node" };
  return c.gpuInContainer ? { ok: true, text: c.gpuInContainer } : { ok: false, text: "a container sees no GPU" };
}

/** The last check for this data source, and when it ran. */
export interface SavedChecks {
  atMs: number;
  checks: NodeCheck[];
}

const SAVED_KEY = "sparkbox.prepare.v1";

export function loadChecks(): SavedChecks | null {
  try {
    const v = JSON.parse(localStorage.getItem(scopedKey(SAVED_KEY)) ?? "null");
    return v && typeof v.atMs === "number" && Array.isArray(v.checks) ? v : null;
  } catch {
    return null;
  }
}

function saveChecks(saved: SavedChecks) {
  try {
    localStorage.setItem(scopedKey(SAVED_KEY), JSON.stringify(saved));
  } catch {
    // Only the next launch's starting point: this launch still shows it.
  }
}

export function PrepareTab() {
  const admin = useAdmin();
  const [saved, setSaved] = useState<SavedChecks | null>(loadChecks);
  const checks = saved?.checks ?? null;
  const [busy, setBusy] = useState(false);
  // A saved result keeps the id it was read under; the page says the node's name today.
  const snap = useCluster();
  const nameOf = (id: string) => snap?.nodes.find((n) => n.id === id)?.name ?? id;

  const run = async () => {
    if (!(await requestAdmin("check the nodes (it starts a short container on each)"))) return;
    setBusy(true);
    try {
      const next = { atMs: Date.now(), checks: await api.clusterCheck() };
      saveChecks(next);
      setSaved(next);
    } catch (e) {
      toast({ kind: "error", title: "Couldn't check the nodes", body: errText(e) });
    } finally {
      setBusy(false);
    }
  };

  const yes = (ok: boolean, text = ok ? "yes" : "no") => <span className={ok ? "copy-ok" : "error-text"}>{ok ? "✓" : "✗"} {text}</span>;
  const columns: Column<NodeCheck>[] = [
    { key: "node", label: "Node", render: (c) => <span className="strong">{nameOf(c.node)}</span> },
    { key: "docker", label: "Docker", render: (c) => (c.error ? <span className="error-text" title={inNames(c.error, snap?.nodes ?? [])}>not checked</span> : yes(c.docker)) },
    { key: "folder", label: "Model folder", render: (c) => (c.error ? "" : <span title={c.rootPath}>{yes(c.rootExists, c.rootExists ? "exists" : "will be created")}</span>) },
    { key: "free", label: "Free disk", align: "right", render: (c) => (c.error ? "" : <span className="num">{fmtBytes(c.freeBytes)}</span>) },
    { key: "image", label: "vLLM image", help: "models.prepare.image", render: (c) => (c.error ? "" : c.imagePresent ? yes(true, imageWord(c)) : imageWord(c)) },
    { key: "vllm", label: "vLLM", priority: 3, render: (c) => c.vllmVersion || "—" },
    {
      key: "fabric", label: "Fabric (ConnectX-7)", help: "models.prepare.fabric",
      render: (c) => {
        if (c.error) return "";
        if (!c.fabric.length) return <span className="error-text">✗ no address</span>;
        return c.fabric.map((f) => (
          <span key={f.iface} className={f.speedMbps < 200000 ? "warn-text" : undefined} title={`${f.iface} · RDMA ${f.rdma || "none"}`}>
            {f.ip}/{f.prefix} @ {f.speedMbps >= 1000 ? `${f.speedMbps / 1000} Gb/s` : "link down"}{"  "}
          </span>
        ));
      },
    },
    {
      key: "halves", label: "MTU and RDMA", help: "models.prepare.fabric",
      render: (c) => (c.error ? "" : c.fabric.map((f) => <div key={f.iface} title={`${f.iface} · ${f.operstate || "state unknown"} · ${f.rdmaPhys || "no physical state"}`}>{fabricState(f)}</div>)),
    },
    {
      key: "gpu", label: "GPU in container", priority: 2,
      render: (c) => {
        if (c.error) return "";
        const g = gpuWord(c);
        return g.ok === null ? <span className="muted">{g.text}</span> : yes(g.ok, g.text);
      },
    },
    { key: "rsync", label: "rsync", priority: 3, render: (c) => (c.error ? "" : yes(c.rsync)) },
  ];

  const slow = checks?.some((c) => c.fabric.some((f) => f.speedMbps > 0 && f.speedMbps < 200000));
  const off = checks?.some((c) => !c.error && !c.fabric.some((f) => f.ip.startsWith("192.168.100.")));
  return (
    <div className="col fill">
      <Toolbar label="Prepare cluster">
        <ToolButton icon={Stethoscope} label={busy ? "Checking…" : "Check nodes"} onClick={run} disabled={busy} locked={!admin}
          title={CHECK_TEXT} />
        {saved && <span className="toolbar-text">Checked <Ago ts={saved.atMs / 1000} /></span>}
        {checks?.some((c) => c.vllmVersion) && <span className="toolbar-text">Started short containers on each node to read the vLLM version and see the GPU; they're removed when they end.</span>}
        <HelpTip topic="models-prepare-cluster" />
      </Toolbar>
      <div className="scroll fill">
      <ImagePreparation />
      <EarlyoomPreparation />
      {checks?.some((c) => c.error) && <Notice tone="error">{checks.filter((c) => c.error).map((c) => `${nameOf(c.node)}: ${inNames(c.error ?? "", snap?.nodes ?? [])}`).join(" · ")}</Notice>}
      {slow && <Notice tone="warn">A fabric link is below 200 Gb/s. On the CRS804, set those ports to 200G-baseCR4 if auto-negotiation came up short.</Notice>}
      {off && <Notice tone="warn">A node has no 192.168.100.x address. SparkBox uses whatever ConnectX-7 address it has, but every node should be on the same subnet.</Notice>}
      {checks
        ? <Table columns={columns} rows={checks} rowKey={(c) => c.node} tableId="models.prepare" />
        : (
          <EmptyState icon={Stethoscope} title="Check the nodes before the first load"
            body={`${CHECK_TEXT} The first check takes about a minute.`}
            action={{ label: busy ? "Checking…" : admin ? "Check nodes" : "Check nodes (needs Admin)…", onClick: run, disabled: busy }} />
        )}
      </div>
    </div>
  );
}
