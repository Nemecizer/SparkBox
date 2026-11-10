// Terminal colour themes. SparkBox Dark and Light are built from the app's tokens: the
// background and text from the neutral ramp, and the 16 ANSI colours tuned to the accent
// and status hues (red is the critical red, green the good green, and so on). When the
// app shows the same theme, the live token values are used, so contrast settings carry over.
// Under the app's Black theme the dark terminal is SPARK_BLACK: true black with the
// Black tokens' brighter colours.
import type { ITheme } from "@xterm/xterm";
import { cssVar } from "../../lib/theme";
import type { TermThemeId } from "./prefs";

type Palette = Required<Pick<ITheme,
  "background" | "foreground" | "cursor" | "cursorAccent" | "selectionBackground" | "selectionInactiveBackground" |
  "black" | "red" | "green" | "yellow" | "blue" | "magenta" | "cyan" | "white" |
  "brightBlack" | "brightRed" | "brightGreen" | "brightYellow" | "brightBlue" | "brightMagenta" | "brightCyan" | "brightWhite">>;

// Token values from styles.css (dark and light), for when the app shows the other theme.
const SPARK_DARK: Palette = {
  background: "#15181c", foreground: "#e4e7eb", cursor: "#5fa1f3", cursorAccent: "#15181c",
  selectionBackground: "#29486f", selectionInactiveBackground: "#3a3e44",
  black: "#25282c", red: "#e54c4a", green: "#3fc168", yellow: "#f3b01d",
  blue: "#5fa1f3", magenta: "#a07ae6", cyan: "#3ab8cf", white: "#c8ccd2",
  brightBlack: "#71767d", brightRed: "#f07270", brightGreen: "#66d488", brightYellow: "#f8c95a",
  brightBlue: "#86baf7", brightMagenta: "#bb9cf0", brightCyan: "#62cde0", brightWhite: "#f5f7f9",
};

// The Black theme's tokens: true black behind the text, colours a step brighter than Dark.
const SPARK_BLACK: Palette = {
  background: "#000000", foreground: "#eceff2", cursor: "#62a7fd", cursorAccent: "#000000",
  selectionBackground: "#163f78", selectionInactiveBackground: "#393d43",
  black: "#171a1e", red: "#f53c41", green: "#30d882", yellow: "#f9b818",
  blue: "#62a7fd", magenta: "#b675db", cyan: "#25d2c7", white: "#c7cbd0",
  brightBlack: "#5f646a", brightRed: "#ff8680", brightGreen: "#65e99c", brightYellow: "#ffc84b",
  brightBlue: "#64afff", brightMagenta: "#d29cf0", brightCyan: "#7fe2da", brightWhite: "#eceff2",
};

const SPARK_LIGHT: Palette = {
  background: "#fbfeff", foreground: "#13161b", cursor: "#186bc5", cursorAccent: "#fbfeff",
  selectionBackground: "#cee3ff", selectionInactiveBackground: "#dce0e5",
  black: "#13161b", red: "#cc3336", green: "#137738", yellow: "#946200",
  blue: "#186bc5", magenta: "#7544cd", cyan: "#00798f", white: "#a0a5ac",
  brightBlack: "#646970", brightRed: "#e0484b", brightGreen: "#25984d", brightYellow: "#b27a00",
  brightBlue: "#1a73d5", brightMagenta: "#8a5ad6", brightCyan: "#0094ab", brightWhite: "#d2d6db",
};

const SOLARIZED_DARK: Palette = {
  background: "#002b36", foreground: "#93a1a1", cursor: "#93a1a1", cursorAccent: "#002b36",
  selectionBackground: "#0d4a58", selectionInactiveBackground: "#073642",
  black: "#073642", red: "#dc322f", green: "#859900", yellow: "#b58900",
  blue: "#268bd2", magenta: "#d33682", cyan: "#2aa198", white: "#eee8d5",
  brightBlack: "#586e75", brightRed: "#cb4b16", brightGreen: "#93a1a1", brightYellow: "#657b83",
  brightBlue: "#839496", brightMagenta: "#6c71c4", brightCyan: "#93a1a1", brightWhite: "#fdf6e3",
};

const ONE_DARK: Palette = {
  background: "#282c34", foreground: "#abb2bf", cursor: "#528bff", cursorAccent: "#282c34",
  selectionBackground: "#3e4451", selectionInactiveBackground: "#353a44",
  black: "#3f4451", red: "#e06c75", green: "#98c379", yellow: "#e5c07b",
  blue: "#61afef", magenta: "#c678dd", cyan: "#56b6c2", white: "#d7dae0",
  brightBlack: "#5c6370", brightRed: "#ff7b86", brightGreen: "#b1e18b", brightYellow: "#efb074",
  brightBlue: "#67cdff", brightMagenta: "#e48bff", brightCyan: "#63d4e0", brightWhite: "#ffffff",
};

/** The app's effective theme right now (Black counts as dark). */
export function appIsDark(): boolean {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "dark" || attr === "black") return true;
  if (attr === "light") return false;
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** The dark SparkBox palette for the theme the app is showing: SPARK_BLACK under Black. */
function sparkDark(): Palette {
  return document.documentElement.getAttribute("data-theme") === "black" ? SPARK_BLACK : SPARK_DARK;
}

/** Live token values over the stored palette, for the theme the app is showing. */
function fromTokens(base: Palette, dark: boolean): Palette {
  const v = (name: string, fallback: string) => cssVar(name) || fallback;
  return {
    ...base,
    cursor: v("--accent", base.cursor),
    blue: v("--accent", base.blue),
    red: v("--critical", base.red),
    green: dark ? v("--good", base.green) : v("--good-text", base.green),
    yellow: dark ? v("--warning", base.yellow) : base.yellow,
    magenta: dark ? base.magenta : v("--m-rdma", base.magenta),
    selectionBackground: dark ? base.selectionBackground : v("--sel", base.selectionBackground),
    selectionInactiveBackground: v("--sel-inactive", base.selectionInactiveBackground),
  };
}

export function terminalTheme(id: TermThemeId): ITheme {
  const dark = appIsDark();
  switch (id) {
    case "solarized-dark":
      return SOLARIZED_DARK;
    case "one-dark":
      return ONE_DARK;
    case "sparkbox-dark":
      return dark ? fromTokens(sparkDark(), true) : SPARK_DARK;
    case "sparkbox-light":
      return dark ? SPARK_LIGHT : fromTokens(SPARK_LIGHT, false);
    default:
      return dark ? fromTokens(sparkDark(), true) : fromTokens(SPARK_LIGHT, false);
  }
}

/** Whether a theme has a dark background (the tab strip and chrome follow it). */
export function themeIsDark(id: TermThemeId): boolean {
  if (id === "follow") return appIsDark();
  return id !== "sparkbox-light";
}

export const PREVIEW_ORDER: (keyof Palette)[] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
