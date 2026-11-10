// A cluster model that stopped answering a moment ago (P1.31) stays in the list, greyed and
// marked "Not answering", and can't be picked; a model that answers can.
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import * as endpoints from "../../lib/endpoints";
import type { Endpoint } from "../../lib/endpoints";
import type { ProviderView } from "../../lib/modelsApi";
import { mockTauri } from "../../test/tauri";
import { ModelPicker, useModelItems } from "./ModelPicker";

const cluster = (id: string, model: string, notAnswering: boolean): ProviderView => ({
  id, name: `vLLM on cell2 :${id.endsWith("8001") ? 8001 : 8000}`, kind: "openaiCompatible", baseUrl: "http://192.168.50.154:8000/v1", enabled: true,
  models: [model], defaultModel: model, hasKey: false, chatOnly: true, hidden: [], visible: [model], effectiveDefault: model,
  discovered: true, notAnswering, effortModels: [],
});

describe("the model picker", () => {
  it("lists a provider that just stopped answering as not answering, and won't pick it", async () => {
    mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { endpoints_list: () => [] }));
    const picked = vi.fn();
    const providers = [cluster("cell2-vllm-8000", "qwen3-8b", true), cluster("cell2-vllm-8001", "gemma-3-27b", false)];
    await act(async () => {
      render(<ModelPicker providers={providers} value={null} onChange={picked} />);
    });
    fireEvent.click(screen.getByRole("button", { name: /Model: none/ }));
    const quiet = screen.getByRole("option", { name: /qwen3-8b/ });
    expect(quiet.getAttribute("aria-disabled")).toBe("true");
    expect(quiet.textContent).toContain("Not answering");
    expect(quiet.title).toBe("Not answering");
    fireEvent.click(quiet);
    expect(picked).not.toHaveBeenCalled();

    const ok = screen.getByRole("option", { name: /gemma-3-27b/ });
    expect(ok.getAttribute("aria-disabled")).toBe("false");
    fireEvent.click(ok);
    expect(picked).toHaveBeenCalledWith({ provider: "cell2-vllm-8001", model: "gemma-3-27b" });
  });
});

describe("a cluster server with no provider", () => {
  const ollama = (over: Partial<Endpoint>): Endpoint => ({
    id: "cell3-ollama-11434", engine: "ollama", models: ["gemma4:latest"], nodes: ["cell3"], headNode: "cell3", port: 11434, url: "http://127.0.0.1:11434/v1",
    managedBy: "external", health: "idle", parallel: null, containerIds: [], expectedNodes: ["cell3"], missingNodes: [],
    ...over,
  } as Endpoint);

  afterEach(() => vi.restoreAllMocks());

  it("is named as everywhere else, not by its slug", () => {
    vi.spyOn(endpoints, "useEndpoints").mockReturnValue([ollama({ label: "Ollama on cell3" }), ollama({ id: "cell1-ollama-11434", headNode: "cell1" })]);
    const { result } = renderHook(() => useModelItems([], false));
    expect(result.current.map((i) => i.providerName)).toEqual(["Ollama on cell3", "Ollama on cell1"]);
  });

  it("calls a load still starting by its engine, not by the model it's already listed as", () => {
    // As the backend sends it: label_of gives a vLLM load its served name.
    vi.spyOn(endpoints, "useEndpoints").mockReturnValue([ollama({ id: "cell2-vllm-8000", engine: "vllm", label: "gemma-3-27b-it", models: ["gemma-3-27b-it"], headNode: "cell2", health: "loading", containerNames: ["sparkbox-gemma"] })]);
    const { result } = renderHook(() => useModelItems([], false));
    expect(result.current.map((i) => [i.model, i.providerName])).toEqual([["gemma-3-27b-it", "vLLM on cell2"]]);
  });
});
