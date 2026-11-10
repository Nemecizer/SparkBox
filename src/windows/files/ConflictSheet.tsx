import { useEffect, useMemo, useRef, useState } from "react";
import { File, Folder } from "lucide-react";
import { HelpTip } from "../../components/ui";
import type { Conflict, Policy } from "../../lib/filesApi";
import { fmtBytes, fmtDateTime } from "../../lib/format";
import { decideConflicts, type Transfer } from "../../lib/transfers";

const LABEL: Record<Policy, string> = {
  replace: "Replace",
  merge: "Merge",
  skip: "Skip",
  keepBoth: "Keep both",
  replaceIfNewer: "Replace if newer",
};

const HELP: Record<Policy, string> = {
  replace: "Swap the existing item out completely",
  merge: "Add to the folder there; files with the same name are replaced",
  skip: "Leave the existing item alone and don't copy this one",
  keepBoth: "Copy it alongside, as “name 2”",
  replaceIfNewer: "Only replace files that are older than the incoming ones",
};

const bothDirs = (c: Conflict) => c.source.kind === "dir" && c.dest.kind === "dir";
const choices = (c: Conflict): Policy[] => (bothDirs(c) ? ["merge", "replace", "replaceIfNewer", "keepBoth", "skip"] : ["replace", "replaceIfNewer", "keepBoth", "skip"]);
const defaultFor = (c: Conflict): Policy => (bothDirs(c) ? "merge" : "keepBoth");

function Side({ side, newer }: { side: Conflict["source"]; newer: boolean }) {
  return (
    <span className="sb-conflict-side">
      <span className="num">{side.kind === "dir" ? `${fmtBytes(side.bytes)} · ${side.entries.toLocaleString()} items` : fmtBytes(side.bytes)}</span>
      <span className="muted">{fmtDateTime(side.mtime * 1000)}</span>
      {newer && <span className="badge info">Newer</span>}
    </span>
  );
}

/**
 * Names already taken where a transfer is going: each conflict with both sides' size and
 * date, a choice per item, and "Apply to all". Continue queues the transfer.
 */
export function ConflictSheet({ t }: { t: Transfer }) {
  const conflicts = useMemo(() => t.preflight?.conflicts.filter((c) => !t.spec.policies[c.name]) ?? [], [t]);
  const [choice, setChoice] = useState<Record<string, Policy>>(() => Object.fromEntries(conflicts.map((c) => [c.name, defaultFor(c)])));
  const first = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    first.current?.focus();
  }, [t.id]);

  const applyAll = (p: Policy) =>
    setChoice(Object.fromEntries(conflicts.map((c) => [c.name, choices(c).includes(p) ? p : p === "merge" ? "replace" : defaultFor(c)])));

  const verb = t.spec.op === "move" ? "Moving" : "Copying";
  const replacing = conflicts.filter((c) => choice[c.name] === "replace").length;
  return (
    <div className="sb-files-sheet-back" onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); decideConflicts(t.id, null); } }}>
      <div className="sb-files-sheet" role="dialog" aria-modal="true" aria-labelledby={`conflict-${t.id}`}>
        <div className="sb-files-sheet-title">
          <span id={`conflict-${t.id}`}>{conflicts.length === 1 ? `“${conflicts[0].name}” already exists on ${t.toName}` : `${conflicts.length} items already exist on ${t.toName}`}</span>
          {" "}<HelpTip topic="desk.files.conflict" />
        </div>
        <p className="sb-files-sheet-sub">
          {verb} {t.what} from {t.fromName} into <span className="mono">{t.spec.toDir}</span>. Choose what happens to each one.
        </p>
        <div className="sb-conflict-list" role="table" aria-label="Conflicts">
          <div className="sb-conflict-row head" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader">Incoming ({t.fromName})</span>
            <span role="columnheader">There now ({t.toName})</span>
            <span role="columnheader">Action</span>
          </div>
          {conflicts.map((c) => (
            <div key={c.name} className="sb-conflict-row" role="row">
              <span className="sb-conflict-name" role="cell" title={c.name}>
                {bothDirs(c) ? <Folder size={13} className="ico-dir" /> : <File size={13} className="ico-file" />}
                {c.name}
              </span>
              <span role="cell"><Side side={c.source} newer={c.source.mtime > c.dest.mtime} /></span>
              <span role="cell"><Side side={c.dest} newer={c.dest.mtime > c.source.mtime} /></span>
              <span role="cell">
                <select value={choice[c.name]} aria-label={`What to do with ${c.name}`} title={HELP[choice[c.name]]}
                  onChange={(e) => setChoice({ ...choice, [c.name]: e.target.value as Policy })}>
                  {choices(c).map((p) => <option key={p} value={p}>{LABEL[p]}</option>)}
                </select>
              </span>
            </div>
          ))}
        </div>
        <div className="sb-files-sheet-actions">
          {conflicts.length > 1 && (
            <label className="sb-conflict-all">
              Apply to all:
              <select value="" onChange={(e) => e.target.value && applyAll(e.target.value as Policy)} aria-label="Apply one action to all">
                <option value="">Choose…</option>
                {(["replace", "merge", "replaceIfNewer", "keepBoth", "skip"] as Policy[]).map((p) => <option key={p} value={p}>{LABEL[p]}</option>)}
              </select>
            </label>
          )}
          <span className="sb-files-sheet-note muted">{replacing ? `${replacing} item${replacing === 1 ? "" : "s"} will be replaced; the old ones are removed only after the new ones have arrived.` : HELP[choice[conflicts[0]?.name] ?? "keepBoth"]}</span>
          <button type="button" className="btn" onClick={() => decideConflicts(t.id, null)}>Cancel {t.spec.op === "move" ? "move" : "copy"}</button>
          <button type="button" className="btn primary" ref={first} onClick={() => decideConflicts(t.id, choice)}>Continue</button>
        </div>
      </div>
    </div>
  );
}
