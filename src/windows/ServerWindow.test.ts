// The text the Server window hands over ends up in files and shell histories, so it never holds a key.
import { describe, expect, it } from "vitest";
import { aliasProblem, snippet, snippetModel } from "./ServerWindow";

const TABS = ["claude", "codex", "openai", "anthropic", "curl"] as const;

describe("the Server's snippets", () => {
  it.each(TABS)("%s names the variable and holds no key", (tab) => {
    const text = snippet(tab, "http://127.0.0.1:8484", "cell2/gemma");
    expect(text).toContain("SPARKBOX_API_KEY");
    expect(text).not.toMatch(/sbx-|your SparkBox key/);
    expect(text).toContain("cell2/gemma");
  });

  it("Python reads the variable, and shells expand it", () => {
    expect(snippet("openai", "u", "m")).toContain('api_key=os.environ["SPARKBOX_API_KEY"]');
    expect(snippet("anthropic", "u", "m")).toContain('api_key=os.environ["SPARKBOX_API_KEY"]');
    expect(snippet("openai", "u", "m").split("\n")[0]).toBe("import os");
    expect(snippet("anthropic", "u", "m").split("\n")[0]).toBe("import os");
    expect(snippet("claude", "u", "m")).toContain("export ANTHROPIC_AUTH_TOKEN=$SPARKBOX_API_KEY");
    expect(snippet("curl", "u", "m")).toContain('"Authorization: Bearer $SPARKBOX_API_KEY"');
  });

  it("Codex's text is only its settings file", () => {
    const text = snippet("codex", "u", "m");
    expect(text).toContain('env_key = "SPARKBOX_API_KEY"');
    expect(text).not.toMatch(/export|in your shell/);
  });
});

describe("the snippets' model with none picked", () => {
  it("is a stand-in that says so with Live data, and the demo model in Demo", () => {
    expect(snippetModel("", "live")).toBe("provider/model");
    expect(snippetModel("", "demo")).toBe("demo/echo");
    expect(snippetModel("cell2-vllm-8000/gemma", "live")).toBe("cell2-vllm-8000/gemma");
  });
});

describe("an alias row", () => {
  const known = new Set(["cell2/gemma"]);
  const saved = { fast: "cell2/gemma", old: "cell3/gone" };
  const check = (name: string, target: string, others: { name: string; target: string }[] = []) => aliasProblem({ name, target }, [{ name, target }, ...others], known, saved);

  it("is fine on a model that's offered", () => {
    expect(check("fast", "cell2/gemma")).toEqual({ error: null, warning: null });
    expect(check("new", "cell2/gemma")).toEqual({ error: null, warning: null });
  });

  it("warns, and doesn't stop Save, when a saved alias's model has gone", () => {
    expect(check("old", "cell3/gone")).toEqual({ error: null, warning: "cell3/gone isn't offered right now" });
    expect(check(" old ", "cell3/gone").error).toBeNull();
  });

  it("stops Save for a new or changed alias on a model nobody offers", () => {
    const unknown = "Unknown target: no provider offers this model now";
    expect(check("new", "cell3/gone")).toEqual({ error: unknown, warning: null });
    expect(check("old", "cell3/other").error).toBe(unknown);
  });

  it("stops Save for a bad shape, gone or not", () => {
    expect(check("", "cell2/gemma").error).toBe("Give it a name");
    expect(check("a/b", "cell2/gemma").error).toBe("Letters, digits and . _ : - only");
    expect(check("new", "").error).toBe("Pick a model");
    expect(check("old", "cell3/gone", [{ name: "old", target: "cell2/gemma" }]).error).toBe("Used twice");
  });
});
