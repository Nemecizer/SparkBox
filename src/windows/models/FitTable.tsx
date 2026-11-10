// The honest fit table: per node count, what the weights take, how long one
// request can be (never more than the model's own maximum), how many
// full-length requests fit at once, a realistic decode ceiling, and whether the
// cluster has the room right now. A model with sparse MLA attention has no kernel for a
// bf16 KV cache on a GB10: those rows say so, and a failed run of it on this cluster is noted.
import { useEffect, useState } from "react";
import { HelpTip, InfoTip, SectionTitle, Segmented } from "../../components/ui";
import { useDeployments, type Deployment } from "../../lib/deployments";
import { fmtBytes } from "../../lib/format";
import type { FitRow, ModelDetails } from "../../lib/modelsApi";
import { fmtTokens } from "./shared";

const TIPS = {
  nodes: "How many Sparks the model is split across (one GPU each).",
  weights: "Weights per node: each node holds this share. Tensor and pipeline parallel both divide them evenly.",
  room: "Room for the KV cache: what's left of the automatic share shown for this split after weights, the runtime and any vision encoder.",
  split: "Tensor parallel needs the attention heads to divide evenly; otherwise only pipeline parallel (layers split between nodes) works.",
  context: "Max context: the longest single request, what the KV cache can hold, but never more than the model's own maximum context.",
  requests: "Full-length requests at once: how many requests of the model's whole maximum context fit in the KV cache together (shorter requests fit more).",
  ceiling: "Decode ceiling for one stream, in tokens per second: the memory-bandwidth limit for a dense model (85% of 273 GB/s per node). Tensor parallel raises it; pipeline parallel doesn't. Real speed is lower.",
  now: "Fits now: whether enough nodes have this much free right now, counting what other models already hold.",
};

function Th({ tip, children, num }: { tip: string; children: React.ReactNode; num?: boolean }) {
  return <th title={tip} className={num ? "num" : undefined}><span className="ai-th-tip">{children}</span></th>;
}

function splitText(r: FitRow): React.ReactNode {
  if (r.nodes === 1) return <span className="muted">single node</span>;
  if (r.tensorParallelOk) return <>TP {r.nodes}</>;
  if (r.pipelineParallelOk) return <span title={`The attention heads don't split ${r.nodes} ways`}>PP {r.nodes} only</span>;
  return <span className="error-text">can't split {r.nodes} ways</span>;
}

/** A failed run of this model on the cluster: same model folder name as the repo's. */
export function seenRun(ds: readonly Deployment[], repo: string): Deployment | null {
  const name = (repo.split("/").pop() ?? repo).toLowerCase();
  return ds.find((d) => d.state === "failed" && [d.label, d.model ?? ""].some((x) => (x.split("/").pop() ?? "").toLowerCase() === name)) ?? null;
}

/** "Seen on your cluster: Oct 26, TP 4, KV bf16, exit 1". */
export function seenLine(d: Deployment): string {
  const pm = d.postmortem;
  const at = new Date(d.finishedMs ?? d.startedMs).toLocaleDateString([], { month: "short", day: "numeric" });
  const tp = pm?.tp ?? (d.members.length > 1 ? d.members.length : null);
  return ["Seen on your cluster: " + at, tp ? `TP ${tp}` : null, pm?.kvDtype ? `KV ${pm.kvDtype}` : null, d.exitCode != null ? `exit ${d.exitCode}` : null].filter(Boolean).join(", ");
}

export function FitTable({ details }: { details: ModelDetails }) {
  const [kv, setKv] = useState<"bf16" | "fp8">("bf16");
  useEffect(() => {
    setKv(details.fit.some((r) => r.noKernel) && details.fitFp8.some((r) => !r.noKernel) ? "fp8" : "bf16");
  }, [details.model.id]);
  const seen = seenRun(useDeployments(), details.model.id);
  const rows = kv === "fp8" ? details.fitFp8 : details.fit;
  const noKernel = rows.find((r) => r.noKernel)?.noKernel;
  const maxCtx = details.arch.maxContext;
  const a = details.arch;
  const isCapped = (r: FitRow) => maxCtx != null && r.maxContextTokens === maxCtx && (r.kvCapacityTokens ?? 0) > maxCtx;
  // When every row that fits is capped, say it once under the table instead of on each row.
  const allCapped = rows.some((r) => r.fits) && rows.filter((r) => r.fits).every(isCapped);
  return (
    <div className="ai-fit">
      <div className="ai-fit-head">
        <SectionTitle
          title={`Fit on ${details.nodesAvailable} Spark${details.nodesAvailable === 1 ? "" : "s"}`}
          suffix={`(${fmtBytes(details.nodeMemory)} memory each, automatic shares)`}
          info={
          <InfoTip label="About the fit table" topic="models-hugging-face#fit-table">
            Counts the {fmtBytes(details.model.weightBytes)} of weights vLLM serves.{" "}
            {a.stateBytesPerSeq > 0 && <>{a.linearLayers} linear-attention layers keep {fmtBytes(a.stateBytesPerSeq)} of state per request on top of the KV cache. </>}
            {a.visionBytes > 0 && <>Includes {fmtBytes(a.visionBytes)} for the vision encoder. </>}
            {details.kvBytesPerToken != null && <>KV cache: {fmtBytes(details.kvBytesPerToken * (kv === "fp8" ? 0.5 : 1))} per token. </>}
            {allCapped && maxCtx != null && <>Max context is the model's own limit ({maxCtx.toLocaleString()} tokens); the cache could hold more. </>}
            {a.sparseMla && <>It uses sparse MLA attention. Memory estimates do not establish model/image compatibility. </>}
            Prefill (reading the prompt) is compute-bound and not shown here.
          </InfoTip>
          }
        />
        <Segmented<"bf16" | "fp8">
          label="KV cache type"
          value={kv}
          onChange={setKv}
          options={[
            { value: "bf16", label: "KV bf16", title: "The model's own precision for the cache" },
            { value: "fp8", label: "KV fp8", title: "--kv-cache-dtype fp8: half the memory per token, a little accuracy" },
          ]}
        />
        <HelpTip topic="models.fit.kv-type" />
      </div>
      {maxCtx != null && (
        <dl className="ai-fit-facts">
          <div>
            <dt>Model's max context</dt>
            <dd className="num">{maxCtx.toLocaleString()} tokens</dd>
          </div>
          <div>
            <dt>Requests</dt>
            <dd>full-length ({fmtTokens(maxCtx)} each)</dd>
          </div>
        </dl>
      )}
      {noKernel && <p className="error-text" role="status">{noKernel}</p>}
      <div className="ai-fit-scroll">
      <table className="grid fit">
        <thead>
          <tr>
            <Th tip={TIPS.nodes} num>Nodes</Th>
            <Th tip={TIPS.weights} num>Weights</Th>
            <Th tip="Automatic share for the model maximum context and four requests, using the Load memory plan." num>Share</Th>
            <Th tip={TIPS.room} num>KV room</Th>
            <Th tip={TIPS.split}>Split</Th>
            <Th tip={TIPS.context} num>Context</Th>
            <Th tip={TIPS.requests} num>Requests</Th>
            <Th tip={TIPS.ceiling} num>Decode t/s</Th>
            <Th tip={TIPS.now}>Fits now</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const capped = !allCapped && isCapped(r);
            // No kernel: the reason spans the rest of the row (beside four "—" it pushed the
            // table past the pane's edge).
            if (r.noKernel) return (
              <tr key={r.nodes} className="row-dim">
                <td className="num">{r.nodes}</td>
                <td className="num">{fmtBytes(r.weightsPerNode)}</td>
                <td className="num">{r.share?.toFixed(2) ?? "—"}</td>
                <td className="num">{r.kvRoomPerNode > 0 ? fmtBytes(r.kvRoomPerNode) : <span className="error-text">none</span>}</td>
                <td colSpan={5}><span className="error-text" title={r.noKernel}>Unavailable</span></td>
              </tr>
            );
            return (
              <tr key={r.nodes} className={r.fits ? undefined : "row-dim"}>
                <td className="num">{r.nodes}</td>
                <td className="num">{fmtBytes(r.weightsPerNode)}</td>
                <td className="num">{r.share?.toFixed(2) ?? "—"}</td>
                <td className="num">{r.kvRoomPerNode > 0 ? fmtBytes(r.kvRoomPerNode) : <span className="error-text">none</span>}</td>
                <td>{r.fits ? splitText(r) : <span className="error-text">✗ doesn't fit</span>}</td>
                <td className="num" title={r.kvCapacityTokens ? `The cache holds ${r.kvCapacityTokens.toLocaleString()} tokens in all` : undefined}>
                  {r.fits && r.maxContextTokens != null ? (
                    <>
                      {r.maxContextTokens.toLocaleString()}
                      {capped && <span className="muted" title="Capped at the model's own maximum context"> · model max</span>}
                    </>
                  ) : "—"}
                </td>
                <td className="num">
                  {r.fits && r.fullRequests != null && maxCtx ? (
                    <span className={r.fullRequests < 1 ? "warn-text" : undefined} title={`${r.fullRequests.toFixed(1)} requests of ${maxCtx.toLocaleString()} tokens at once`}>
                      ≈{r.fullRequests.toFixed(r.fullRequests >= 10 ? 0 : 1)}
                    </span>
                  ) : "—"}
                </td>
                <td className="num">
                  {r.decodeCeilingTps != null ? (
                    <span title={r.pipelineOnly ? "Pipeline parallel splits the layers, so one stream runs no faster than on a single node" : "The bandwidth ceiling for one stream; real speed is lower"}>
                      ≤ {r.decodeCeilingTps.toFixed(r.decodeCeilingTps >= 10 ? 0 : 1)}
                      {r.pipelineOnly && <span className="muted"> · no speedup</span>}
                    </span>
                  ) : r.fits && a.experts ? <span className="muted ai-th-tip" title="Mixture of experts: speed depends on which experts each token uses">varies</span> : "—"}
                </td>
                <td className="ai-fit-now">
                  {!r.fits ? "—" : r.fitsNow ? (
                    // One word on the row; which nodes have room is in the tooltip.
                    <span className={r.fitsNow.ok ? undefined : "warn-text"} title={`${r.fitsNow.text[0].toUpperCase()}${r.fitsNow.text.slice(1)}${r.fitsNow.freeNodes.length ? ` (free: ${r.fitsNow.freeNodes.join(", ")})` : ""}`}>
                      {r.fitsNow.ok ? "Yes" : r.fitsNow.freeNodes.length ? `${r.fitsNow.freeNodes.length} free` : "No"}
                    </span>
                  ) : <span className="muted">unknown</span>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      </div>
      {seen && (
        <p className="ai-fit-seen muted" title={seen.postmortem?.title ? `Likely cause: ${seen.postmortem.title}` : undefined}>
          {seenLine(seen)}{seen.postmortem?.title ? ` · ${seen.postmortem.title}` : ""}
        </p>
      )}
    </div>
  );
}
