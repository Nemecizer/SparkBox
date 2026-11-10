import reports from "../../test/fixtures/deployment-history-reports.json";
import recovery from "../../test/fixtures/deployment-history-recovery.json";
import type { DeploymentHistorySource, DeploymentHistoryView } from "../../lib/modelsApi";

// Same data-only inputs as Rust Demo; never sourced from the owner's saved files.
function sample(name: string, rows: Array<any>): DeploymentHistorySource {
  return { name, kind: "demo", sha256: "Demo sample", bytes: new TextEncoder().encode(JSON.stringify(rows)).length, issues: [],
    records: rows.map((r) => ({ id: r.spec?.id ?? r.id, model: r.spec?.servedName ?? null, nodes: r.spec?.nodes ?? r.unloadReport.nodes.map((n: {node: string}) => n.node),
      savedState: r.state, startedAt: r.startedAt ?? null, generation: r.generation ?? null, message: r.message ?? null, failure: null, unloadReport: r.unloadReport ?? null, reportOnly: !r.spec })) };
}
export function demoHistory(): DeploymentHistoryView {
  return { demo: true, folder: null, sources: [sample("recovery-copy-demo.json", recovery), sample("older-reports-demo.json", reports)], issues: [] };
}
