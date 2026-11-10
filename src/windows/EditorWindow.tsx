// A text editor window for one file on this Mac or a node (Files › Edit). CodeMirror with
// find and replace, go to line, YAML and JSON errors inline, and a theme from the app's
// tokens. Saves are atomic and only replace the version that was opened: if the file
// changed meanwhile, the editor offers Reload, Show diff or Overwrite. Root-owned files on a
// node are saved with sudo after a "Review changes" diff. The buffer lives outside React,
// so minimizing loses nothing; closing with unsaved changes asks the Mac way: Save (⌘S),
// Don't Save or Cancel.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { FileClock, FileLock2, FileWarning, History, Lock, RefreshCw, Save, Search, ShieldCheck, Undo2, WrapText } from "lucide-react";
import { HelpTip, Notice, Spacer, Toolbar, ToolButton, ToolSeparator } from "../components/ui";
import { confirmUnlock, isLookOnly, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, unlockedFor, useAdminState } from "../lib/admin";
import { showContextMenu } from "../lib/contextMenu";
import {
  bufferKey, bufferText, clearBufferError, closeBuffer, dismissConflict, encodingWords, ensureBuffer, isDirty, loadDiskText, readBufferAsRoot, reloadBuffer,
  restoreBackup, saveBuffer, setBufferState, useBuffer,
} from "../lib/editorBuffers";
import { MAC } from "../lib/filesApi";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { useCluster } from "../lib/store";
import { isDarkTheme, onThemeChange, useDarkMode } from "../lib/theme";
import { toast } from "../lib/toasts";
import { useCloseGuard, useWindowEvent, useWindowId, useWindowTitle, useWMActions, type Params } from "../wm/WindowManager";
import { DiffView } from "./files/editor/DiffView";
import { LANGUAGE_LABEL, languageExtension, languageFor, parseProblems, type LanguageId } from "./files/editor/languages";
import { editorTheme, extensions, languageSlot, openSearchPanel, readOnlySlot, themeSlot, wrapSlot } from "./files/editor/setup";
import { baseName } from "./files/paths";
import "./files/editor/editor.css";

const WRAP_KEY = "sparkbox.editor.wrap";

function readWrap(): boolean {
  try {
    return localStorage.getItem(WRAP_KEY) === "1";
  } catch {
    return false;
  }
}

/** The CodeMirror view for a buffer; its state (and undo history) is kept in the buffer store. */
function CodeView({ bufKey, text, state, baseText, language, readOnly, wrap, onSave, onView, onCursor }: {
  bufKey: string;
  text: string;
  state: EditorState | null;
  baseText: string;
  language: LanguageId;
  readOnly: boolean;
  wrap: boolean;
  onSave: () => void;
  onView: (v: EditorView | null) => void;
  onCursor: (c: { line: number; col: number }) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const dark = useDarkMode();
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const cursorRef = useRef(onCursor);
  cursorRef.current = onCursor;
  const base = useRef(Text.of(baseText.split("\n")));

  useEffect(() => {
    const report = (s: EditorState) => {
      const head = s.selection.main.head;
      const line = s.doc.lineAt(head);
      cursorRef.current({ line: line.number, col: head - line.from + 1 });
    };
    const exts = extensions({
      language, dark, wrap, readOnly,
      onSave: () => saveRef.current(),
      onChange: (s) => {
        setBufferState(bufKey, s, !s.doc.eq(base.current));
        report(s);
      },
    });
    const v = new EditorView({ state: state ?? EditorState.create({ doc: text, extensions: exts }), parent: host.current! });
    // A kept state carries the settings it was made with; bring them up to date.
    if (state) {
      v.dispatch({
        effects: [
          themeSlot.reconfigure(editorTheme(dark)),
          wrapSlot.reconfigure(wrap ? EditorView.lineWrapping : []),
          languageSlot.reconfigure(languageExtension(language)),
          readOnlySlot.reconfigure(EditorState.readOnly.of(readOnly)),
        ],
      });
    }
    view.current = v;
    onView(v);
    report(v.state);
    v.focus();
    return () => {
      setBufferState(bufKey, v.state, !v.state.doc.eq(base.current));
      v.destroy();
      view.current = null;
      onView(null);
    };
    // Built once per document; a replaced document (reload, restore) remounts this with a new key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // After a save the text on disk is the new base.
  useEffect(() => {
    base.current = Text.of(baseText.split("\n"));
    const v = view.current;
    if (v) setBufferState(bufKey, v.state, !v.state.doc.eq(base.current));
  }, [baseText, bufKey]);
  useEffect(() => onThemeChange((t) => view.current?.dispatch({ effects: themeSlot.reconfigure(editorTheme(isDarkTheme(t.effective))) })), []);
  useEffect(() => {
    view.current?.dispatch({ effects: themeSlot.reconfigure(editorTheme(dark)) });
  }, [dark]);
  useEffect(() => {
    view.current?.dispatch({ effects: wrapSlot.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }, [wrap]);
  useEffect(() => {
    view.current?.dispatch({ effects: [languageSlot.reconfigure(languageExtension(language)), readOnlySlot.reconfigure(EditorState.readOnly.of(readOnly))] });
  }, [language, readOnly]);

  return <div ref={host} className="sb-editor-host" />;
}

/** A sheet over the editor (Review changes, Show diff, the close question). */
function Sheet({ title, tip, children, actions, onClose, onSave, small }: {
  title: string;
  /** A Help tip beside the title. */
  tip?: React.ReactNode;
  children: React.ReactNode;
  actions: React.ReactNode;
  onClose: () => void;
  /** ⌘S in the sheet. */
  onSave?: () => void;
  small?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, []);
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    } else if (onSave && e.key.toLowerCase() === "s" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.stopPropagation();
      onSave();
    }
  };
  return (
    <div className="sb-editor-sheet-back" onKeyDown={onKeyDown}>
      <div className={`sb-editor-sheet${small ? " small" : ""}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
        <div className="sb-editor-sheet-title">{title}{tip && <> {tip}</>}</div>
        <div className="sb-editor-sheet-body">{children}</div>
        <div className="sb-editor-sheet-actions">{actions}</div>
      </div>
    </div>
  );
}

export function EditorWindow({ params }: { params: Params }) {
  const place = params.place ?? MAC;
  const path = params.path ?? "";
  const key = bufferKey(place, path);
  const cluster = useCluster();
  const adminNow = useAdminState();
  const admin = adminNow.unlocked;
  const placeName = place === MAC ? "This Mac" : (cluster?.nodes.find((n) => n.id === place)?.name ?? place);

  // Opened on the first render (no flash of "Opening…" for a buffer that's already there),
  // and dropped when the window closes.
  useMemo(() => ensureBuffer(place, path), [place, path]);
  useEffect(() => {
    ensureBuffer(place, path);
    return () => closeBuffer(key);
  }, [place, path, key]);
  const buf = useBuffer(key);
  const latest = useRef(buf);
  latest.current = buf;

  const [wrap, setWrap] = useState(readWrap);
  const [language, setLanguage] = useState<LanguageId>(() => languageFor(path));
  const [view, setView] = useState<EditorView | null>(null);
  const [cursor, setCursor] = useState({ line: 1, col: 1 });
  const [sheet, setSheet] = useState<"review" | "diff" | "close" | null>(null);
  const winId = useWindowId();
  const wm = useWMActions();
  // Set once the close question is answered, so the second close goes through.
  const closing = useRef<"save" | "discard" | null>(null);

  const name = baseName(buf?.file?.path ?? path);
  const dirty = !!buf && isDirty(buf);
  useWindowTitle(`${dirty ? "● " : ""}${name} — ${placeName}`);
  useCloseGuard(() => (closing.current !== "discard" && latest.current && isDirty(latest.current) ? `${name} — ${placeName} has unsaved changes` : null));
  // Closing with unsaved changes asks here, in the window, instead of the generic dialog.
  useWindowEvent("close-request", (e) => {
    if (closing.current === "discard" || !latest.current || !isDirty(latest.current)) return;
    e.preventDefault();
    setSheet("close");
  });
  useWindowEvent("find", () => {
    if (view) openSearchPanel(view);
  });

  const onNode = place !== MAC;
  // Saving needs the file's own scope: on this Mac that's Touch ID or the password, within 5 minutes.
  const scope = onNode ? "node" : "mac";
  const saveKey = onNode ? "files_write" : "files_mac";
  const rootOwned = !!buf?.file && (buf.file.owner.startsWith("root:") || !buf.file.writable || !!buf.readAsRoot);
  const sudo = onNode && (rootOwned || !!buf?.needsSudo);
  const readOnly = buf?.status === "large" || params.readOnly === "1";
  useEffect(() => {
    const number = Number(params.line);
    if (!view || !Number.isSafeInteger(number) || number < 1 || buf?.status !== "ready") return;
    const line = view.state.doc.line(Math.min(number, view.state.doc.lines));
    view.dispatch({ selection: { anchor: line.from, head: line.to }, effects: EditorView.scrollIntoView(line.from, { y: "center" }) });
  }, [view, params.line, buf?.status]);

  const save = useCallback(async (opts: { overwrite?: boolean; reviewed?: boolean } = {}) => {
    const b = latest.current;
    if (params.readOnly === "1") return;
    if (!b || b.status !== "ready" || b.saving) return;
    if (!isDirty(b) && !opts.overwrite) return;
    if (!(await confirmUnlock(`save ${name} on ${placeName}`, scope, saveKey))) {
      closing.current = null;
      return;
    }
    // Root-owned files, and text that doesn't parse, get a look at the changes first.
    if ((sudo || parseProblems(language, bufferText(b)) > 0) && !opts.reviewed) {
      setSheet("review");
      return;
    }
    setSheet(null);
    const r = await saveBuffer(key, { sudo, overwrite: opts.overwrite });
    if (r === "saved") toast({ kind: "success", title: `Saved ${name}`, body: `${placeName}${sudo ? " · with sudo, owner and permissions kept" : ""}`, durationMs: 2500 });
    if (r === "needsSudo" && onNode) setSheet("review");
    // Saved from the close question: the window closes now.
    if (r === "saved" && closing.current === "save") wm.close(winId);
    if (r !== "saved") closing.current = null;
  }, [key, language, name, onNode, params.readOnly, placeName, saveKey, scope, sudo, wm, winId]);

  const saveAndClose = () => {
    closing.current = "save";
    void save();
  };
  const discardAndClose = () => {
    closing.current = "discard";
    setSheet(null);
    wm.close(winId);
  };

  const reload = () => void reloadBuffer(key);

  const showDiff = async () => {
    await loadDiskText(key);
    setSheet("diff");
  };

  const versionsMenu = (e: React.MouseEvent) => {
    const list = buf?.backups ?? [];
    showContextMenu(e, list.length
      ? list.map((b, i) => ({
          label: `${fmtDateTime(b.mtime * 1000)} · ${fmtBytes(b.size)}${i === 0 ? " (before the last save)" : ""}`,
          icon: FileClock,
          run: async () => {
            await restoreBackup(key, b);
            toast({ kind: "info", title: "Previous version loaded", body: "It replaces the file only when you save." });
          },
        }))
      : [{ label: "No previous versions yet", disabled: "SparkBox keeps the last five versions each time you save" }]);
  };

  const langMenu = (e: React.MouseEvent) => {
    showContextMenu(e, (Object.keys(LANGUAGE_LABEL) as LanguageId[]).map((id) => ({ label: LANGUAGE_LABEL[id], checked: id === language, run: () => setLanguage(id) })));
  };

  const setWrapPersist = (on: boolean) => {
    setWrap(on);
    try {
      localStorage.setItem(WRAP_KEY, on ? "1" : "0");
    } catch {
      // A convenience only.
    }
  };

  if (!buf || buf.status === "loading") {
    return <div className="sb-editor col fill"><div className="sb-editor-loading muted">Opening {name}…</div></div>;
  }
  if (buf.status === "error") {
    return (
      <div className="sb-editor col fill">
        <div className="sb-editor-loading">
          <Notice tone="error" onRetry={() => ensureBuffer(place, path)}>{buf.error}</Notice>
        </div>
      </div>
    );
  }
  if (buf.status === "permission") {
    return <RootOnly name={name} placeName={placeName} place={place} bufKey={key} refusal={buf.permission ?? null} error={buf.error} unlocked={admin} lookOnly={isLookOnly(adminNow)} />;
  }

  const conflict = buf.conflict;
  const encoding = encodingWords(buf);
  const text = buf.status === "large" ? largeText(buf.file?.head ?? "", buf.file?.tail ?? "", buf.file?.size ?? 0) : buf.text;
  return (
    <div className="sb-editor col fill">
      <Toolbar label="Editor">
        <ToolButton icon={sudo ? ShieldCheck : Save} label={buf.saving ? "Saving…" : sudo ? "Save with sudo" : "Save"} onClick={() => void save()}
          disabled={!dirty || buf.saving || readOnly} locked={!unlockedFor(scope, adminNow) && dirty} scope={scope} action={saveKey}
          title={sudo ? "Shows the changes, then saves as root, keeping the file's owner and permissions (⌘S)" : "Save (⌘S)"} />
        <ToolButton icon={Undo2} label="Revert" priority={2} disabled={!dirty || buf.saving} onClick={reload} title="Throw away unsaved changes and read the file again" />
        <HelpTip topic="desk.editor.versions" />
        <ToolButton icon={RefreshCw} label="Reload" priority={3} disabled={buf.saving || dirty} onClick={reload} title="Read the file from disk again" />
        <ToolSeparator />
        <ToolButton icon={Search} label="Find" priority={2} disabled={!view} onClick={() => view && openSearchPanel(view)} title="Find (⌘F) · Find and replace (⌥⌘F) · Go to line (⌘L)" />
        <ToolButton icon={WrapText} label="Wrap" priority={3} active={wrap} onClick={() => setWrapPersist(!wrap)} title="Wrap long lines" />
        <button type="button" className="tool" onClick={versionsMenu} title="Restore a previous version (the last five saves are kept)" disabled={readOnly} data-priority={2}>
          <History size={14} />
          <span className="tool-label">Previous versions</span>
        </button>
        <Spacer />
        <span className="sb-editor-where mono" title={buf.file?.path}>{placeName} · {buf.file?.path}</span>
      </Toolbar>
      {conflict && (
        <div className="sb-editor-bar warn" role="alert">
          <FileWarning size={14} />
          <span>
            {conflict.sha256 === null
              ? `${name} was deleted on ${placeName} after you opened it.`
              : `${name} changed on disk after you opened it${conflict.mtime ? ` (modified ${fmtDateTime(conflict.mtime * 1000)})` : ""}.`}{" "}
            Your changes aren't saved yet.
          </span>
          <Spacer />
          <button type="button" className="btn small" onClick={reload}>Reload</button>
          {conflict.sha256 !== null && <button type="button" className="btn small" onClick={() => void showDiff()}>Show diff</button>}
          <button type="button" className="btn small danger" onClick={() => void save({ overwrite: true, reviewed: !sudo })}>Overwrite</button>
          <button type="button" className="btn small" onClick={() => dismissConflict(key)}>Keep editing</button>
        </div>
      )}
      {buf.error && !conflict && <Notice tone="error" onDismiss={() => clearBufferError(key)}>{buf.error}</Notice>}
      {!admin && !readOnly && !conflict && (
        <div className="sb-editor-bar info sb-editor-locked" role="note">
          <Lock size={14} aria-hidden />
          <span>{isLookOnly(adminNow) ? "Read-only · look-only mode" : "Read-only · Unlock to edit"}</span>
          <HelpTip topic="desk.editor.read-only" />
          <Spacer />
          {!isLookOnly(adminNow) && (
            <button type="button" className="btn small" onClick={() => void confirmUnlock(`edit ${name} on ${placeName}`, place === MAC ? "mac" : "node", place === MAC ? "files_mac" : undefined)}>Unlock…</button>
          )}
        </div>
      )}
      {params.readOnly === "1" && <Notice tone="info">Opened read-only from the failure details.</Notice>}
      {buf.status === "large" && (
        <Notice tone="info">
          {name} is {fmtBytes(buf.file?.size ?? 0)}. Files over 2 MB open read-only here, showing the first and last 64 KB.
        </Notice>
      )}
      {!readOnly && sudo && !conflict && (
        <div className="sb-editor-bar info">
          <ShieldCheck size={14} />
          <span>{name} belongs to {buf.file?.owner.split(":")[0] || "root"}. Saving uses sudo, keeps its owner and permissions, and shows you the changes first.</span>
        </div>
      )}
      <CodeView
        key={`${key}#${buf.generation}`}
        bufKey={key}
        text={text}
        state={buf.state}
        baseText={buf.baseText}
        language={language}
        readOnly={readOnly || !admin}
        wrap={wrap}
        onSave={() => void save()}
        onView={setView}
        onCursor={setCursor}
      />
      <div className="sb-editor-status" role="status">
        <span>Ln {cursor.line}, Col {cursor.col}</span>
        <button type="button" className="sb-editor-status-btn" onClick={langMenu} title="Change the language">{LANGUAGE_LABEL[language]}</button>
        <span title={encoding.title}>{encoding.text}</span>
        <span className="mono">{buf.file?.mode} {buf.file?.owner}</span>
        <Spacer />
        <span className={dirty ? "sb-editor-dirty" : "muted"}>
          {buf.saving ? "Saving…" : dirty ? "Edited" : buf.savedAtMs ? `Saved ${fmtDateTime(buf.savedAtMs).replace(/^Today at /, "at ")}` : readOnly ? "Read-only" : "No changes"}
        </span>
      </div>
      {sheet === "review" && (() => {
        const problems = parseProblems(language, bufferText(buf));
        return (
          <Sheet
            title={`Review changes to ${name}`}
            tip={<HelpTip topic="desk.editor.review" />}
            onClose={() => { closing.current = null; setSheet(null); }}
            actions={
              <>
                <span className="muted sb-editor-sheet-note">
                  {sudo ? `Will save as root on ${placeName}, keeping owner ${buf.file?.owner} and mode ${buf.file?.mode}. The current version will be kept in Previous versions.` : "The current version will be kept in Previous versions."}
                </span>
                <button type="button" className="btn" data-autofocus={problems > 0 ? true : undefined} onClick={() => { closing.current = null; setSheet(null); }}>{problems ? "Keep editing" : "Cancel"}</button>
                <button type="button" className={`btn ${problems ? "danger" : "primary"}`} data-autofocus={problems ? undefined : true}
                  onClick={() => void save({ reviewed: true, overwrite: !!conflict })}>
                  {problems ? "Save anyway" : sudo ? "Save with sudo" : "Save"}
                </button>
              </>
            }
          >
            {problems > 0 && (
              <div className="sb-editor-bar warn sb-editor-review-warn" role="alert">
                <FileWarning size={14} />
                <span>
                  This {LANGUAGE_LABEL[language]} doesn't parse ({problems} error{problems === 1 ? "" : "s"}, underlined in the editor).
                  {name.endsWith(".yaml") && path.startsWith("/etc/netplan/") ? " netplan won't apply it, and the node could lose these links at the next boot." : ""}
                </span>
              </div>
            )}
            <DiffView before={buf.baseText} after={bufferText(buf)} beforeLabel="on disk" afterLabel="your version" />
          </Sheet>
        );
      })()}
      {sheet === "close" && (
        <Sheet
          small
          title={`Do you want to save the changes to ${name}?`}
          onClose={() => setSheet(null)}
          onSave={saveAndClose}
          actions={
            <>
              <button type="button" className="btn" onClick={discardAndClose}>Don't Save</button>
              <span className="spacer" />
              <button type="button" className="btn" onClick={() => setSheet(null)}>Cancel</button>
              <button type="button" className="btn primary" data-autofocus onClick={saveAndClose} title="⌘S">
                {sudo ? "Save with sudo" : "Save"}
                <kbd className="sb-editor-kbd">⌘S</kbd>
              </button>
            </>
          }
        >
          <p className="sb-editor-close-text">Your changes will be lost if you don't save them.{sudo ? ` Saving shows you the changes first, then saves as root on ${placeName}.` : ""}</p>
        </Sheet>
      )}
      {sheet === "diff" && conflict && (
        <Sheet
          title={`${name}: what changed on disk`}
          onClose={() => setSheet(null)}
          actions={
            <>
              <button type="button" className="btn" data-autofocus onClick={() => setSheet(null)}>Keep editing</button>
              <button type="button" className="btn" onClick={() => { setSheet(null); reload(); }}>Reload theirs</button>
              <button type="button" className="btn danger" onClick={() => void save({ overwrite: true, reviewed: !sudo })}>Overwrite with mine</button>
            </>
          }
        >
          <DiffView before={conflict.diskText ?? ""} after={bufferText(buf)} beforeLabel="on disk now" afterLabel="your version" />
        </Sheet>
      )}
    </div>
  );
}

/**
 * A file only root can read: what it is and why, and Read with sudo… (locked until Admin
 * mode is unlocked; it asks in-app first, then reads it as root). Transmit-style: the
 * permission problem is the content, not an error.
 */
function RootOnly({ name, placeName, place, bufKey, refusal, error, unlocked, lookOnly }: {
  name: string; placeName: string; place: string; bufKey: string; refusal: import("../lib/filesApi").PermissionRefusal | null; error: string | null; unlocked: boolean; lookOnly: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const owner = refusal?.owner || "root";
  const facts = [owner, refusal?.mode].filter(Boolean).join(", ");
  const readWithSudo = async () => {
    if (busy) return;
    if (!(await confirmUnlock(`read ${name} on ${placeName} with sudo`, "node"))) return;
    setBusy(true);
    await readBufferAsRoot(bufKey);
    setBusy(false);
  };
  return (
    <div className="sb-editor col fill">
      <div className="sb-editor-rootonly" role="status">
        <FileLock2 size={28} aria-hidden className="sb-editor-rootonly-icon" />
        <h2 className="sb-editor-rootonly-title">
          Only {owner} can read {name}{facts && <span className="sb-editor-rootonly-facts"> ({facts})</span>}
        </h2>
        <p className="sb-editor-rootonly-body">Reading it needs sudo, which needs Admin.</p>
        <p className="muted sb-editor-rootonly-path mono" title={refusal?.path}>{placeName} · {refusal?.path}</p>
        <div className="row gap">
          <button type="button" className={`btn${unlocked ? " primary" : ""}`} disabled={busy || place === MAC || lookOnly} onClick={() => void readWithSudo()}
            data-lock={lookOnly ? "lookOnly" : unlocked ? undefined : "locked"}
            title={lookOnly ? LOOK_ONLY_TIP : unlocked ? "Reads it as root on the node; the password goes to sudo, never on a command line" : `Read it as root · ${NEEDS_ADMIN_TIP}`}>
            {!unlocked && <Lock size={12} aria-hidden />} {busy ? "Reading…" : "Read with sudo…"}
          </button>
        </div>
        {error && <Notice tone="error">{error}</Notice>}
      </div>
    </div>
  );
}

function largeText(head: string, tail: string, size: number): string {
  const shown = new TextEncoder().encode(head).length + new TextEncoder().encode(tail).length;
  return `${head.replace(/\n?[^\n]*$/, "")}\n\n⋯  ${fmtBytes(Math.max(0, size - shown))} not shown  ⋯\n\n${tail.replace(/^[^\n]*\n/, "")}`;
}

