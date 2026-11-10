// Which language the editor uses for a file, by its name: the config files a Spark is full
// of (YAML netplan, JSON, Python, shell, systemd units and other INI-style files, TOML).
import { json, jsonParseLinter } from "@codemirror/lang-json";
import { python } from "@codemirror/lang-python";
import { yaml } from "@codemirror/lang-yaml";
import { StreamLanguage } from "@codemirror/language";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { linter, lintGutter, type Diagnostic } from "@codemirror/lint";
import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { parseDocument } from "yaml";

export type LanguageId = "yaml" | "json" | "python" | "shell" | "ini" | "toml" | "text";

export const LANGUAGE_LABEL: Record<LanguageId, string> = {
  yaml: "YAML", json: "JSON", python: "Python", shell: "Shell", ini: "INI", toml: "TOML", text: "Plain text",
};

const INI_EXT = ["ini", "conf", "cfg", "service", "socket", "timer", "mount", "target", "path", "slice", "network", "netdev", "link", "desktop", "properties", "env"];
const SHELL_NAMES = [".bashrc", ".bash_profile", ".profile", ".zshrc", ".zprofile", ".bash_aliases", ".bash_logout", "bashrc", "profile", "environment"];

export function languageFor(path: string): LanguageId {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  if (ext === "yaml" || ext === "yml") return "yaml";
  if (ext === "json" || ext === "jsonc" || name === ".sparkbox.json") return "json";
  if (ext === "py" || ext === "pyi") return "python";
  if (ext === "toml" || name === "pipfile") return "toml";
  if (ext === "sh" || ext === "bash" || ext === "zsh" || SHELL_NAMES.includes(name)) return "shell";
  if (INI_EXT.includes(ext)) return "ini";
  return "text";
}

/** YAML parse errors as inline diagnostics (netplan refuses a file that doesn't parse). */
function yamlLinter(view: EditorView): Diagnostic[] {
  const text = view.state.doc.toString();
  const doc = parseDocument(text, { prettyErrors: false });
  const out: Diagnostic[] = [];
  for (const [list, severity] of [[doc.errors, "error"], [doc.warnings, "warning"]] as const) {
    for (const e of list) {
      const from = Math.min(e.pos[0], text.length);
      const to = Math.min(Math.max(e.pos[1], from + 1), text.length);
      out.push({ from, to: Math.max(from, to), severity, message: e.message.split("\n")[0], source: "YAML" });
    }
  }
  return out;
}

export function languageExtension(id: LanguageId): Extension {
  switch (id) {
    case "yaml":
      return [yaml(), linter(yamlLinter, { delay: 300 }), lintGutter()];
    case "json":
      return [json(), linter(jsonParseLinter(), { delay: 300 }), lintGutter()];
    case "python":
      return python();
    case "shell":
      return StreamLanguage.define(shell);
    case "ini":
      return StreamLanguage.define(properties);
    case "toml":
      return StreamLanguage.define(toml);
    default:
      return [];
  }
}

/** How many parse errors a YAML or JSON text has (0 for other languages), for a warning before saving. */
export function parseProblems(id: LanguageId, text: string): number {
  if (id === "yaml") return parseDocument(text, { prettyErrors: false }).errors.length;
  if (id === "json") {
    try {
      JSON.parse(text);
    } catch {
      return 1;
    }
  }
  return 0;
}
