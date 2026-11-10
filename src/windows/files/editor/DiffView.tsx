import { useMemo } from "react";
import { diffLines, diffStats, hunks } from "./diff";

/** A unified diff with both sides' line numbers; additions and removals are marked by sign, not only colour. */
export function DiffView({ before, after, beforeLabel, afterLabel }: { before: string; after: string; beforeLabel: string; afterLabel: string }) {
  const lines = useMemo(() => diffLines(before, after), [before, after]);
  const hs = useMemo(() => hunks(lines), [lines]);
  const stats = diffStats(lines);
  return (
    <div className="sb-diff">
      <div className="sb-diff-head">
        <span className="sb-diff-del-label">− {beforeLabel}</span>
        <span className="sb-diff-add-label">+ {afterLabel}</span>
        <span className="spacer" />
        <span className="muted">
          {stats.added} line{stats.added === 1 ? "" : "s"} added, {stats.removed} removed
        </span>
      </div>
      {hs.length === 0 ? (
        <div className="sb-diff-empty muted">No differences.</div>
      ) : (
        <div className="sb-diff-body mono" role="table" aria-label="Changes">
          {hs.map((h, i) => (
            <div key={i} className="sb-diff-hunk" role="rowgroup">
              <div className="sb-diff-at" role="row">@@ −{h.aStart},{h.aLen} +{h.bStart},{h.bLen} @@</div>
              {h.lines.map((l, j) => (
                <div key={j} className={`sb-diff-line ${l.kind}`} role="row">
                  <span className="sb-diff-no" role="cell">{l.a ?? ""}</span>
                  <span className="sb-diff-no" role="cell">{l.b ?? ""}</span>
                  <span className="sb-diff-sign" role="cell" aria-label={l.kind === "add" ? "added" : l.kind === "del" ? "removed" : undefined}>
                    {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                  </span>
                  <span className="sb-diff-text" role="cell">{l.text || " "}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
