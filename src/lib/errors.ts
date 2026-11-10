// Turns an error from the backend into a sentence and the button that fixes it. Errors come
// as an SbError {kind, message, node?, detail?} (commands that return SbError, and every gate
// refusal: string-returning commands send it as JSON, which commandError() reads back) or as
// a plain sentence from the Live layer, whose kind kindOf() works out.
import { createElement } from "react";
import { searchHelp } from "../help/nav";
import { commandError, CommandError, invoke } from "./api";
import { LOOK_ONLY_TEXT, requestAdmin } from "./admin";
import type { SbError } from "./opsApi";
import { redact } from "./redact";
import { alertInfo } from "./session";
import { toast } from "./toasts";
import { getVpnStatus } from "./vpn";

export interface ErrorAction {
  label: string;
  run: () => void | Promise<void>;
}

export interface Described {
  kind: SbError["kind"];
  title: string;
  message: string;
  detail?: string;
  actions: ErrorAction[];
}

export interface ErrorContext {
  /** What was being done, e.g. "Restart docker on cell2" — becomes the title. */
  action?: string;
  node?: string;
}

function openWindow(kind: string, params: Record<string, string> = {}) {
  window.dispatchEvent(new CustomEvent("sparkbox:open-window", { detail: { kind, params } }));
}

const openSettings = (section: string) => () => openWindow("settings", { section });

/** Works out the kind of a plain error string (the same rules as SbError::classify in Rust). */
export function kindOf(message: string): SbError["kind"] {
  const m = message.toLowerCase();
  if (m.includes("look-only mode")) return "lookOnly";
  if (m.includes("open fence") || m.includes("the fence ") || m.includes("fence record") || m.includes("fence key") || m.includes("no fence is open")) return "fenced";
  if (m.includes("is controlling") || m.includes("managed by the other sparkbox")) return "otherController";
  if (m.includes(" answers as ") || m.includes("pinned which machine")) return "wrongNode";
  if (m.includes("admin mode is locked")) return "locked";
  if (m.includes("to change settings on this mac") || m.includes("changes a saved password or key")) return "needs_auth";
  if (m.includes("sudo") && (m.includes("rejected") || m.includes("incorrect password") || m.includes("sudoers") || m.includes("can't use sudo"))) return "sudoRejected";
  if (m.includes("save the cluster password")) return "noPassword";
  if (m.includes("didn't accept the password") || m.includes("authentication failed") || m.includes("password was rejected")) return "authRejected";
  if (m.includes("timed out") || m.includes("timeout") || m.includes("no reply")) return "timeout";
  if (m.includes("isn't connected") || m.includes("not connected") || m.includes("unreachable") || m.includes("connection refused")) return "unreachable";
  if (m.includes("demo mode") || m.includes("switch to live")) return "demoOnly";
  if (m.includes("cancelled") || m.includes("canceled")) return "cancelled";
  if (m.includes("already exited") || m.includes("no such process") || m.includes("no such container")) return "gone";
  return "failed";
}

export { CommandError };

/** Any error from a command as an SbError (a gate refusal's JSON included). */
export function asSbError(e: unknown): SbError {
  e = commandError(e);
  if (e && typeof e === "object" && "kind" in e && "message" in e) return { kind: (e as SbError).kind, message: (e as SbError).message, node: (e as SbError).node, detail: (e as SbError).detail };
  if (e instanceof Error) return { kind: kindOf(e.message), message: e.message };
  const message = String(e ?? "Something went wrong.").replace(/^Error:\s*/, "");
  return { kind: kindOf(message), message };
}

/** Every error's way to Help, after its own fix. The words on screen are what Help's entries quote. */
const explain = (message: string): ErrorAction => ({ label: "What does this mean?", run: () => searchHelp(message) });

export function describeError(e: unknown, ctx: ErrorContext = {}): Described {
  const err = asSbError(e);
  const d = described(err, ctx);
  return err.kind === "cancelled" ? d : { ...d, actions: [...d.actions, explain(d.message)] };
}

/** Reconnecting can only help when the VPN is in use or failing: at home it's Off or Direct. */
function vpnInPlay(): boolean {
  const state = getVpnStatus()?.state;
  return !!state && state !== "off" && state !== "direct";
}

function described(err: SbError, ctx: ErrorContext): Described {
  const node = err.node ?? ctx.node ?? err.message.match(/\b(cell\d+|gx10-[a-z0-9]+)\b/)?.[1];
  const title = ctx.action ? `Couldn't ${ctx.action.charAt(0).toLowerCase()}${ctx.action.slice(1)}` : "That didn't work";
  const base = { kind: err.kind, title, detail: err.detail };
  switch (err.kind) {
    case "lookOnly":
      return { ...base, title: "Look only", message: LOOK_ONLY_TEXT, actions: [] };
    case "locked":
      return {
        ...base, title: "Admin mode is locked", message: "Unlock it to make changes to the cluster.",
        actions: [{ label: "Unlock", run: () => void requestAdmin(ctx.action) }],
      };
    case "needs_auth":
      return {
        ...base, title: "Confirm it's you", message: "Changes on this Mac need Touch ID or your Mac password, even with Demo data.",
        actions: [{ label: "Confirm", run: () => void requestAdmin(ctx.action, "mac") }],
      };
    case "authRejected":
      return {
        ...base, message: node ? `${node} didn't accept the password. Check the cluster password in Settings.` : err.message,
        actions: [{ label: "Open Settings", run: openSettings("login") }],
      };
    case "sudoRejected":
      return {
        ...base, message: node ? `sudo on ${node} didn't accept the saved cluster password.` : err.message,
        actions: [{ label: "Fix password", run: openSettings("login") }],
      };
    case "noPassword":
      return { ...base, message: "Save the cluster password first; SparkBox uses it for SSH and sudo.", actions: [{ label: "Open Settings", run: openSettings("login") }] };
    case "timeout":
    case "unreachable":
      return {
        ...base, message: err.message,
        actions: vpnInPlay() ? [{ label: "Reconnect VPN", run: () => void invoke("vpn_reconnect").catch(() => {}) }] : [],
      };
    case "demoOnly":
      return { ...base, message: err.message, actions: [{ label: "Switch to Live…", run: openSettings("data") }] };
    case "fenced":
      return { ...base, title: "Outside the fence", message: `${err.message} The test build's changes need an open fence that names the node and the kind of change.`, actions: [{ label: "Open Settings › Fence", run: openSettings("fence") }] };
    case "otherController":
      return { ...base, title: "The other SparkBox is in charge", message: `${err.message} Wait for it to finish, or close it, then try again.`, actions: [{ label: "Open Settings › Fence", run: openSettings("fence") }] };
    case "wrongNode":
      return { ...base, title: "Not the machine SparkBox pinned", message: err.message, actions: [{ label: "Open Settings › Fence", run: openSettings("fence") }] };
    case "gone":
    case "cancelled":
    case "invalid":
    case "failed":
    default:
      return { ...base, message: err.message, actions: [] };
  }
}

/** The node's own words (stderr, an exit code), in a dialog: the toast's sentence is the short version. */
function detailsAction(title: string, detail: string): ErrorAction {
  const text = redact(detail);
  return {
    label: "Details",
    run: () => void alertInfo({ title, body: createElement("div", { className: "code-block" }, createElement("pre", null, createElement("code", null, text))) }),
  };
}

/** Shows the error as a toast that stays, with its fix as a button, and Details when it carries more. */
export function showError(e: unknown, ctx: ErrorContext = {}): Described {
  const d = describeError(e, ctx);
  if (d.kind === "cancelled") return d;
  const actions = d.detail ? [...d.actions, detailsAction(d.title, d.detail)] : d.actions;
  toast({ kind: "error", title: d.title, body: d.message, actions: actions.length ? actions : undefined });
  return d;
}
