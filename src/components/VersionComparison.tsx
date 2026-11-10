import { Copy } from "lucide-react";
import { Badge, HelpTip, Spacer, ToolButton } from "./ui";
import { copyText } from "../lib/markdown";
import type { VersionReport, VersionState } from "../lib/types";

const words: Record<VersionState, string> = { matching: "Matching", mismatched: "Mismatched", unknown: "Unknown" };
const cell = (s: string) => s.replace(/\|/g, "\\|").replace(/[\r\n]+/g, " ");

export function versionTable(report: VersionReport, nodes: { id: string; name: string }[]) {
  return `| What | Verdict | ${nodes.map((n) => cell(n.name)).join(" | ")} |\n|---|---|${nodes.map(() => "---").join("|")}|\n` + report.rows.map((r) =>
    `| ${cell(r.label)} | ${words[r.state]}${r.note ? `: ${cell(r.note)}` : ""} | ${nodes.map((n) => {
      const v = r.values.find((v) => v.node === n.id);
      return cell(v?.value ?? `Unknown: ${v?.reason ?? "Not reported"}`);
    }).join(" | ")} |`).join("\n") + "\n";
}

/** Both views render the same backend comparison; the browser never invents a majority. */
export function VersionComparison({ report, nodes, load = false, demo = false }: {
  report?: VersionReport | null;
  nodes: { id: string; name: string }[];
  load?: boolean;
  demo?: boolean;
}) {
  if (!report) return <p className="muted" role="status">Version consistency: Unknown — waiting for a comparison.</p>;
  const different = report.rows.filter((r) => r.state === "mismatched");
  const unknown = report.rows.filter((r) => r.state === "unknown");
  const table = <div className="version-comparison-table">
    <table className="grid" aria-label="Versions by node">
      <thead><tr><th>What</th><th>Verdict</th>{nodes.map((n) => <th key={n.id}>{n.name}</th>)}</tr></thead>
      <tbody>{report.rows.map((r) => <tr key={r.key}>
        <td className="strong" title={r.note ?? undefined}>{r.label}{r.note && " *"}</td>
        <td><Badge tone={r.state === "mismatched" ? "warn" : r.state === "matching" ? "good" : "neutral"}>{words[r.state]}</Badge></td>
        {nodes.map((n) => {
          const value = r.values.find((v) => v.node === n.id);
          return <td key={n.id} className={r.state === "mismatched" && value?.value ? "mono fleet-drift" : "mono"} title={value?.value ?? value?.reason ?? "Not reported"}>
            {value?.value ? (r.key === "imageId" ? `${value.value.slice(0, 19)}…` : value.value) : <span className="muted">Unknown<span className="version-row-note">{value?.reason ?? "Not reported"}</span></span>}
          </td>;
        })}
      </tr>)}</tbody>
    </table>
    {report.rows.filter((r) => r.note).map((r) => <p className="muted version-row-note" key={r.key}>* {r.label}: {r.note}</p>)}
  </div>;
  return <section className="version-comparison" aria-label="Version consistency">
    <div className="fleet-bar">
      <strong role="status">Version consistency: {words[report.state]}</strong>
      <HelpTip topic="ops.fleet.versions" label="About version consistency" />
      <Spacer /><ToolButton icon={Copy} label="Copy as table" title="Copy full values, verdicts and unknown readings as Markdown" onClick={() => void copyText(versionTable(report, nodes))} />
    </div>
    <p className="muted">{different.length ? `Mismatched: ${different.map((r) => r.label).join(", ")}. ` : ""}{unknown.length ? `Unknown: ${unknown.map((r) => r.label).join(", ")}.` : !different.length ? "All reported values match across comparable nodes." : ""}</p>
    <p className="muted">{demo ? "Demo readings are simulated. " : ""}{load ? "Selected image; NCCL is its declared version. Differences and unknown readings are advisory; existing image identity checks still apply." : "Running vLLM/SGLang containers; NCCL is their declared version. No single serving image means Unknown."} Matching versions alone do not prove compatibility.</p>
    {load ? <details><summary>Compare version details</summary>{table}</details> : table}
  </section>;
}
