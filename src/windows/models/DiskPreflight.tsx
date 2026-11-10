import { HelpTip, Notice } from "../../components/ui";
import { api } from "../../lib/api";
import { fmtBytes } from "../../lib/format";
import { alertInfo } from "../../lib/session";
import type { DiskCheck, DiskRequest } from "../../lib/types";

export function DiskPreflight({ checks }: { checks: DiskCheck[] }) {
  return <div className="col gap-sm">
    <span className="strong">Destination disk <HelpTip topic="models.download.disk" /></span>
    <p>The full transfer plus 5% and 20 GiB must be available. Partial files do not reduce this reserve. Space is checked again when the job starts; this check does not reserve space.</p>
    {checks.map((check) => <div className="col gap-sm" key={check.node}>
      <span className="strong">{check.node} · {check.ok ? "Enough space" : "Not enough space"}</span>
      <span className="mono wrap">{check.dest}</span>
      <span>{fmtBytes(check.availableBytes)} available · {fmtBytes(check.requiredBytes)} required · {fmtBytes(check.transferBytes)} of files</span>
      <span className="muted">Checked filesystem at {check.probe}</span>
      {check.warning && <Notice tone="warn">{check.warning}</Notice>}
    </div>)}
  </div>;
}

/** Refusal is informational: no confirmation or start is offered with insufficient space. */
export async function checkTransferDisk(request: DiskRequest): Promise<DiskCheck[] | null> {
  const checks = await api.modelDiskPreflight(request);
  if (!checks.length) throw new Error("No destination disk reading was returned. Nothing was started.");
  if (checks.some((check) => !check.ok)) {
    await alertInfo({ title: "Not enough destination space", body: <DiskPreflight checks={checks} /> });
    return null;
  }
  return checks;
}
