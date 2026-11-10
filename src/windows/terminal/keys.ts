// Keyboard handling for terminal tabs, kept pure so it can be tested: Option as Meta
// (per side, like iTerm2), and the window's own shortcuts. ⌘K (palette), ⌘+ / ⌘− (zoom)
// and ⌘W (close, through the window's close request) are left to the app.
import type { OptionKey } from "./prefs";

export interface KeyLike {
  type: string;
  code: string;
  key: string;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
}

export type Side = "left" | "right" | null;

/** Which Option key is down, from AltLeft / AltRight presses (KeyboardEvent.altKey can't tell). */
export class OptionTracker {
  left = false;
  right = false;

  observe(e: Pick<KeyLike, "type" | "code" | "altKey">) {
    const down = e.type === "keydown";
    if (e.code === "AltLeft") this.left = down;
    else if (e.code === "AltRight") this.right = down;
    else if (!e.altKey) this.left = this.right = false; // released while focus was elsewhere
  }

  reset() {
    this.left = this.right = false;
  }

  get side(): Side {
    return this.right ? "right" : this.left ? "left" : null;
  }
}

export function optionIsMeta(setting: OptionKey, side: Side): boolean {
  if (side === null) return false;
  return setting === "both" || setting === side;
}

/** Physical key → [unshifted, shifted] on a US layout: Meta sequences are layout-independent. */
const US: Record<string, [string, string]> = {
  Backquote: ["`", "~"], Minus: ["-", "_"], Equal: ["=", "+"], BracketLeft: ["[", "{"], BracketRight: ["]", "}"],
  Backslash: ["\\", "|"], Semicolon: [";", ":"], Quote: ["'", "\""], Comma: [",", "<"], Period: [".", ">"], Slash: ["/", "?"],
  Space: [" ", " "],
  Digit1: ["1", "!"], Digit2: ["2", "@"], Digit3: ["3", "#"], Digit4: ["4", "$"], Digit5: ["5", "%"],
  Digit6: ["6", "^"], Digit7: ["7", "&"], Digit8: ["8", "*"], Digit9: ["9", "("], Digit0: ["0", ")"],
};
for (let c = 65; c <= 90; c++) {
  const ch = String.fromCharCode(c);
  US[`Key${ch}`] = [ch.toLowerCase(), ch];
}

/**
 * What an Option-modified key sends when that Option key is Meta: Esc + the key (readline's
 * Meta-b, Meta-f, Meta-. …). Null leaves the key to xterm, which types the Option character
 * (so @ and { still work on layouts that need Option for them).
 */
export function metaSequence(e: KeyLike, setting: OptionKey, side: Side): string | null {
  if (e.type !== "keydown" || !e.altKey || e.metaKey || !optionIsMeta(setting, side)) return null;
  switch (e.code) {
    case "Backspace":
      return "\x1b\x7f";
    case "ArrowLeft":
      return "\x1bb";
    case "ArrowRight":
      return "\x1bf";
    case "Enter":
      return "\x1b\r";
  }
  const keys = US[e.code];
  if (!keys) return null;
  const ch = e.shiftKey ? keys[1] : keys[0];
  if (e.ctrlKey && /^[a-z]$/i.test(ch)) return `\x1b${String.fromCharCode(ch.toUpperCase().charCodeAt(0) - 64)}`;
  return `\x1b${ch}`;
}

export type TermShortcut = "find" | "next-tab" | "prev-tab" | "clear-scrollback" | "new-tab" | "find-next" | "find-prev";

/** The Terminal window's own keys while a tab has focus. */
export function terminalShortcut(e: KeyLike): TermShortcut | null {
  if (e.type !== "keydown") return null;
  if (e.ctrlKey && !e.metaKey && !e.altKey && e.code === "Tab") return e.shiftKey ? "prev-tab" : "next-tab";
  if (!e.metaKey || e.ctrlKey) return null;
  if (e.shiftKey && !e.altKey && e.code === "BracketRight") return "next-tab";
  if (e.shiftKey && !e.altKey && e.code === "BracketLeft") return "prev-tab";
  if (e.altKey && !e.shiftKey && e.code === "KeyK") return "clear-scrollback";
  if (!e.altKey && !e.shiftKey && e.code === "KeyF") return "find";
  if (!e.altKey && !e.shiftKey && e.code === "KeyT") return "new-tab";
  if (!e.altKey && e.code === "KeyG") return e.shiftKey ? "find-prev" : "find-next";
  return null;
}
