// A pop-out: one window's body in a native macOS window of its own, titled like the
// inner window was. It keeps the shell's dialogs, menus and toasts; links in it open
// on the desk, and Return to desk moves it back there.
import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CornerUpLeft } from "lucide-react";
import { ConfirmHost } from "../components/ConfirmHost";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { Toasts } from "../components/Toasts";
import { InfoTip } from "../components/ui";
import { WindowErrorBoundary } from "../components/WindowErrorBoundary";
import { inTauri } from "../lib/api";
import { registerCommands } from "../lib/commands";
import { suppressNativeMenus } from "../lib/contextMenu";
import { installKeymap } from "../lib/keymap";
import { installModality } from "../lib/modality";
import { helpOnDesk, openOnDesk, type PopoutRoute } from "../lib/popout";
import { useClusterSelector } from "../lib/store";
import { helpOf, iconOf, isKnown, minSizeOf, sizeOf, titleOf, WindowBody } from "../windows/registry";
import { refreshTitles, useTitleVersion, WindowManagerProvider } from "./WindowManager";
import "./wm.css";
import "../components/shell.css";

export function PopoutRoot({ route }: { route: PopoutRoute }) {
  const known = isKnown(route.kind);
  const title = known ? titleOf(route) : route.kind;
  const Icon = iconOf(route.kind);
  const help = helpOf(route.kind, route.params);
  useTitleVersion();
  // A node's or switch's name can arrive after the first frame.
  const names = useClusterSelector((s) => [...(s?.nodes ?? []), ...(s?.switches ?? [])].map((x) => `${x.id}=${x.name}`).join(","));
  useEffect(() => refreshTitles(), [names]);

  useEffect(() => {
    document.title = title;
    if (inTauri) getCurrentWindow().setTitle(title).catch(() => {});
  }, [title]);
  useEffect(() => suppressNativeMenus(), []);
  useEffect(() => installModality(), []);
  // ⇧F10 and the context-menu key work here too; the menu bar's shortcuts belong to the desk.
  useEffect(() => installKeymap({ nativeMenu: true }), []);
  // ⌘? never reaches a menu, so the keymap runs this. Help opens on the desk, like any link here.
  useEffect(() => registerCommands([helpOnDesk]), []);

  const backToDesk = async () => {
    await openOnDesk(route);
    if (inTauri) await getCurrentWindow().close().catch(() => {});
  };

  return (
    <WindowManagerProvider
      sizeOf={sizeOf}
      minSizeOf={minSizeOf}
      isKnown={isKnown}
      titleOf={titleOf}
      iconOf={iconOf}
      helpOf={helpOf}
      openElsewhere={(kind, params) => void openOnDesk({ kind, params })}
    >
      <div className="popout">
        <div className="popout-bar">
          {Icon && <Icon size={13} aria-hidden className="popout-icon" />}
          <span className="popout-title">{title}</span>
          {help && <InfoTip topic={help} />}
          <span className="spacer" />
          <button type="button" className="btn small popout-return" onClick={() => void backToDesk()} title="Put it back on SparkBox's desk">
            <CornerUpLeft size={13} aria-hidden /> Return to desk
          </button>
        </div>
        <div className="popout-body win-body">
          {known ? (
            <WindowErrorBoundary id={`pop:${route.kind}`} title={title}>
              <WindowBody kind={route.kind} params={route.params} />
            </WindowErrorBoundary>
          ) : (
            <div className="pad muted">This build has no “{route.kind}” window.</div>
          )}
        </div>
        <div className="popout-toasts"><Toasts /></div>
      </div>
      <ConfirmHost />
      <ContextMenuHost />
    </WindowManagerProvider>
  );
}
