import { useState } from "react";
import { api } from "../../lib/api";
import { confirmAction } from "../../lib/session";
import type { DownloadTool, DownloadToolReview, DownloadTools } from "../../lib/types";

export function DownloadToolChoice({ inventory, onChoice }: { inventory: DownloadTools; onChoice: (tool: DownloadTool) => void }) {
  const [index, setIndex] = useState(0);
  const tool = inventory.options[index];
  return <div className="col gap">
    <p>Choose the tool for this attempt on {inventory.node}. No image is pulled and no other tool is chosen automatically.</p>
    <label className="col gap-sm">Download tool
      <select aria-label="Download tool" value={index} onChange={(e) => { const i = Number(e.target.value); setIndex(i); onChoice(inventory.options[i]); }}>
        {inventory.options.map((t, i) => <option key={`${t.kind}:${t.path}`} value={i}>{t.label}</option>)}
      </select>
    </label>
    {tool && <>
      <label className="col gap-sm">{tool.kind === "image" ? "Installed image ID" : "Tool location"}<input readOnly value={tool.path} aria-label="Selected tool identity" /></label>
      <p>{tool.kind === "install" ? "This needs a separate installation confirmation. It installs pinned hf and hf-xet packages with their dependencies in SparkBox's private environment, using a checksummed uv installer. It uses the node's existing Python, without sudo or apt."
        : tool.kind === "image" ? "The image must already be on this node. SparkBox checks its hf command in a temporary container before downloading; an unsuitable image stops this attempt."
        : "SparkBox checks this executable again before starting. A missing or changed tool stops this attempt."}</p>
    </>}
    {inventory.notes.map((note) => <p key={note} className="muted">{note}</p>)}
  </div>;
}

/** Read-only discovery and choice. The caller then confirms the concrete download/install. */
export async function chooseDownloadTool(node: string): Promise<DownloadToolReview | null> {
  const inventory = await api.modelDownloadTools(node);
  if (!inventory.options.length) throw new Error(inventory.notes.join(" ") || "No download tool is available on this node.");
  let chosen = inventory.options[0];
  if (!await confirmAction({ title: `Choose a download tool on ${node}`, body: <DownloadToolChoice inventory={inventory} onChoice={(tool) => { chosen = tool; }} />, confirmLabel: "Continue" })) return null;
  return { node, tool: chosen, installConsent: false };
}

export function downloadToolConsent(review: DownloadToolReview): string {
  const { tool } = review;
  return tool.kind === "install"
    ? `${tool.label} at ${tool.path}, using the pinned, checksummed uv installer from GitHub and packages from PyPI. Installation is for this attempt, uses the node's existing Python, and does not use sudo or apt. Existing tool folders are not replaced. Ordinary cancellation or failure removes the new staging folder; a forced shutdown may leave it for inspection. A completed installation remains available. Then download with that tool.`
    : `Use ${tool.label} (${tool.path}). Its identity is checked again, and there is no automatic installation or image pull.`;
}
