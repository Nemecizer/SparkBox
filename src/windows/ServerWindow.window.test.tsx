// The Server window as it reads with Live data: its snippets, a saved alias whose model has gone,
// and a key name already used.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, adminState } from "../lib/admin";
import { _resetDataSourceForTests, type Source } from "../lib/dataSource";
import type { ProviderView, ServerView } from "../lib/modelsApi";
import { mockCalls, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { ServerWindow } from "./ServerWindow";

const CELL2: ProviderView = {
  id: "cell2-vllm-8000", name: "vLLM on cell2 :8000", kind: "openaiCompatible", baseUrl: "http://192.168.50.154:8000/v1", enabled: true, models: ["gemma"], defaultModel: "gemma",
  hasKey: false, chatOnly: true, hidden: [], visible: ["gemma"], effectiveDefault: "gemma", discovered: true, notAnswering: false,
};

const view = (aliases: Record<string, string> = {}): ServerView => ({
  settings: { port: 8484, requireKey: true, autostart: false, aliases, allowedOrigins: [] },
  running: false, url: null, error: null, clients: [{ name: "Claude Code", createdAt: 1, preview: "sbx-0123…" }],
});

async function show(source: Source, opts: { providers?: ProviderView[]; server?: ServerView; handlers?: Record<string, (a: never) => unknown> } = {}) {
  _resetDataSourceForTests(source);
  _resetAdminForTests({ unlocked: true, authenticated: true, expiresAtMs: null, macExpiresAtMs: null, idleMinutes: 15, requiresAuth: false, lookOnly: false });
  const server = opts.server ?? view();
  const base = defaultHandlers();
  mockTauri(Object.assign({}, base, ...MOCKS, {
    admin_state: () => adminState(),
    get_snapshot: () => ({ ...(base.get_snapshot as () => object)(), demo: source === "demo" }),
    providers_list: () => opts.providers ?? [],
    endpoints_list: () => [],
    server_get: () => structuredClone(server),
    server_logs: () => [],
    server_save: ({ settings }: { settings: ServerView["settings"] }) => ({ ...server, settings }),
  }, opts.handlers));
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1000, 800]} isKnown={() => true}><ServerWindow /></WindowManagerProvider>);
  });
  for (let i = 0; i < 40 && !screen.queryByText("Access keys"); i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
  return r.container;
}

const codeOf = (root: HTMLElement) => root.querySelector(".code-block code")?.textContent ?? "";

afterEach(() => {
  _resetAdminForTests();
  _resetDataSourceForTests("demo");
});

describe("the Server's snippets with no model picked", () => {
  it("reads provider/model and says to pick one, with Live data", async () => {
    const root = await show("live");
    expect(codeOf(root)).toContain("export ANTHROPIC_MODEL=provider/model");
    expect(codeOf(root)).not.toContain("demo/echo");
    expect(screen.getByText("Pick a model above")).toBeInTheDocument();
  });

  it("keeps demo/echo in Demo", async () => {
    const root = await show("demo");
    expect(codeOf(root)).toContain("export ANTHROPIC_MODEL=demo/echo");
    expect(screen.queryByText("Pick a model above")).toBeNull();
  });

  it("names the model once one is picked", async () => {
    const root = await show("live", { providers: [CELL2] });
    expect(codeOf(root)).toContain("export ANTHROPIC_MODEL=cell2-vllm-8000/gemma");
    expect(screen.queryByText("Pick a model above")).toBeNull();
  });
});

describe("a saved alias whose model has gone", () => {
  it("warns on its row and doesn't stop Save", async () => {
    await show("live", { providers: [CELL2], server: view({ fast: "cell3-vllm-8000/gone" }) });
    expect(screen.getByText("cell3-vllm-8000/gone isn't offered right now")).toBeInTheDocument();
    expect(screen.queryByText(/Fix the aliases to save/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Port"), { target: { value: "8485" } });
    const save = screen.getByRole("button", { name: "Save settings" });
    expect(save).toBeEnabled();
    await act(async () => {
      fireEvent.click(save);
      await new Promise((res) => setTimeout(res, 20));
    });
    const sent = mockCalls.find((c) => c.cmd === "server_save")?.args.settings as ServerView["settings"];
    expect(sent.aliases).toEqual({ fast: "cell3-vllm-8000/gone" });
  });

  it("stops Save when one is pointed at a model nobody offers", async () => {
    await show("live", { providers: [CELL2], server: view({ fast: "cell3-vllm-8000/gone" }) });
    fireEvent.change(screen.getByLabelText("Alias"), { target: { value: "quick" } });
    expect(screen.getByRole("alert")).toHaveTextContent("Unknown target: no provider offers this model now");
    expect(screen.getByRole("button", { name: "Save settings" })).toBeDisabled();
    expect(screen.getByText("Fix the aliases to save")).toBeInTheDocument();
  });
});

describe("a key name already used", () => {
  it("shows the refusal in the window's notice, and no new key", async () => {
    const refusal = "A key named “Claude Code” already exists. Revoke it first, or use another name.";
    await show("live", { handlers: { server_create_key: () => { throw refusal; } } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create key" }));
      await new Promise((res) => setTimeout(res, 20));
    });
    expect(screen.getByRole("alert")).toHaveTextContent(refusal);
    expect(document.querySelector(".new-key")).toBeNull();
  });
});
