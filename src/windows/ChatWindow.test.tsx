// Chat with Claude stays append-only (P1.29): once a conversation has a reply, a changed system
// prompt goes in as a system turn after the next question and the top-level one never changes.
// OpenAI's reasoning models get an effort picker and no temperature box (P1.30).
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { defaultHandlers } from "../dev/mockBackend";
import { MOCKS } from "../dev/mocks";
import { _resetAdminForTests, adminState, LOOK_ONLY_TIP } from "../lib/admin";
import { _resetDataSourceForTests } from "../lib/dataSource";
import type { ChatMessage, Conversation, ProviderView } from "../lib/modelsApi";
import { channelEnd, channelSend, extendMock, mockTauri } from "../test/tauri";
import { WindowManagerProvider } from "../wm/WindowManager";
import { ChatWindow } from "./ChatWindow";

const provider = (id: string, kind: ProviderView["kind"], models: string[], effortModels: string[]): ProviderView => ({
  id, name: id === "anthropic" ? "Anthropic (Claude)" : "OpenAI", kind, baseUrl: "", enabled: true, models, defaultModel: models[0],
  hasKey: true, chatOnly: true, hidden: [], visible: models, effectiveDefault: models[0], discovered: false, notAnswering: false, effortModels,
});

const PROVIDERS = [
  provider("anthropic", "anthropic", ["claude-opus-5-5", "claude-sonnet-5"], ["claude-opus-5-5", "claude-sonnet-5"]),
  provider("openai", "openai", ["gpt-5.5", "gpt-4.1"], ["gpt-5.5"]),
  provider("local-qwen", "openaiCompatible", ["qwen3-0.6b"], []),
];

const blocks = [{ type: "thinking", thinking: "Plan.", signature: "s1" }, { type: "text", text: "Apple" }];

/** A Claude conversation with one question and its reply, under "Be brief." */
function claudeChat(): Conversation {
  return {
    id: "c1", title: "Fruit", createdAt: 1, updatedAt: 2, provider: "anthropic", model: "claude-opus-5-5", system: "Be brief.", params: {},
    messages: [
      { id: "u1", parentId: null, role: "user", content: "Name a fruit", createdAt: 1 },
      { id: "a1", parentId: "u1", role: "assistant", content: "Apple", anthropicBlocks: blocks, provider: "anthropic", model: "claude-opus-5-5", stopReason: "end_turn", createdAt: 2 },
    ],
    leaf: "a1",
  };
}

interface Sent {
  system: string;
  model: string;
  params: { effort?: string | null; temperature?: number | null };
  messages: ChatMessage[];
}

type ChatSendMock = (args: { request: Sent & { provider: string }; onEvent: unknown }) => unknown;

async function show(conv: Conversation, chatSend?: ChatSendMock) {
  _resetDataSourceForTests("live");
  const sent: Sent[] = [];
  const saved: Conversation[] = [];
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, {
    admin_state: () => adminState(),
    providers_list: () => PROVIDERS,
    conversations_list: () => [],
    conversation_get: () => structuredClone(conv),
    conversation_save: ({ conversation }: { conversation: Conversation }) => {
      saved.push(structuredClone(conversation));
      return null;
    },
    chat_send: ({ request, onEvent }: { request: Sent & { provider: string }; onEvent: unknown }) => {
      sent.push(structuredClone(request));
      if (chatSend) return chatSend({ request, onEvent });
      channelEnd(onEvent);
      return { message: { role: "assistant", content: "Pomme", provider: request.provider, model: request.model, stopReason: "end_turn", createdAt: 3 }, error: null };
    },
  }));
  let r!: ReturnType<typeof render>;
  await act(async () => {
    r = render(<WindowManagerProvider sizeOf={() => [1000, 700]} isKnown={() => true}><ChatWindow params={{ id: conv.id }} /></WindowManagerProvider>);
  });
  for (let i = 0; i < 40 && !r.container.querySelector(".msg.assistant"); i++) await act(async () => new Promise((res) => setTimeout(res, 25)));
  return { root: r.container, sent, saved, unmount: r.unmount };
}

const type = (el: HTMLTextAreaElement | HTMLInputElement, value: string) => fireEvent.change(el, { target: { value } });

async function send(root: HTMLElement, text: string) {
  type(root.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!, text);
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await new Promise((res) => setTimeout(res, 30));
  });
}

describe("a Claude conversation with a reply", () => {
  it("keeps its top-level system prompt and sends a change as a system turn after the next question", async () => {
    const { root, sent, saved } = await show(claudeChat());
    fireEvent.click(screen.getByRole("button", { name: /System/ }));
    const box = root.querySelector<HTMLTextAreaElement>('textarea[aria-label="System prompt"]')!;
    expect(box.value).toBe("Be brief.");
    expect(box.placeholder).toContain("a change applies from your next message");
    type(box, "Answer in French.");
    await send(root, "And a green one?");

    expect(sent).toHaveLength(1);
    expect(sent[0].system).toBe("Be brief.");
    expect(sent[0].messages.map((m) => [m.role, m.content])).toEqual([
      ["user", "Name a fruit"], ["assistant", "Apple"], ["user", "And a green one?"], ["system", "Answer in French."],
    ]);
    expect(sent[0].messages[1].anthropicBlocks).toEqual(blocks);
    // Claude takes an effort, never a temperature.
    expect(sent[0].params.temperature).toBeNull();
    expect(root.querySelector('input[aria-label="Temperature"]')).toBeNull();

    const last = saved[saved.length - 1];
    expect(last.system).toBe("Be brief.");
    const change = last.messages.find((m) => (m.role as string) === "system")!;
    const reply = last.messages[last.messages.length - 1];
    expect([change.parentId, reply.parentId, reply.content]).toEqual([last.messages.find((m) => m.content === "And a green one?")!.id, change.id, "Pomme"]);
    expect(root.textContent).toContain("System prompt changed from here: Answer in French.");

    // The next question needs no new change: the one in effect is already there.
    await send(root, "Merci");
    expect(sent[1].system).toBe("Be brief.");
    expect(sent[1].messages.map((m) => m.role)).toEqual(["user", "assistant", "user", "system", "assistant", "user"]);
  });

  it("regenerates after a system change with the change before the new reply", async () => {
    const { root, sent } = await show(claudeChat());
    fireEvent.click(screen.getByRole("button", { name: /System/ }));
    type(root.querySelector<HTMLTextAreaElement>('textarea[aria-label="System prompt"]')!, "Answer in German.");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Regenerate/ }));
      await new Promise((res) => setTimeout(res, 30));
    });
    expect(sent[0].system).toBe("Be brief.");
    expect(sent[0].messages.map((m) => [m.role, m.content])).toEqual([["user", "Name a fruit"], ["system", "Answer in German."]]);
    // Both answers to the question are versions of each other: the first reply and the one after the change.
    expect(root.textContent).toContain("2 / 2");
  });
});

describe("OpenAI models", () => {
  it("offer an effort and no temperature for a reasoning model, and a temperature for one that samples", async () => {
    const conv: Conversation = { ...claudeChat(), provider: "openai", model: "gpt-5.5", messages: [], leaf: null };
    const { root, sent } = await show(conv);
    const effort = root.querySelector<HTMLSelectElement>('select[aria-label="Effort"]')!;
    expect([...effort.options].map((o) => o.value)).toEqual(["", "low", "medium", "high"]);
    expect(root.querySelector('input[aria-label="Temperature"]')).toBeNull();
    fireEvent.change(effort, { target: { value: "high" } });
    await send(root, "Hi");
    expect(sent[0].params).toMatchObject({ effort: "high", temperature: null });
  });

  it("keep the temperature box for gpt-4.1", async () => {
    const conv: Conversation = { ...claudeChat(), provider: "openai", model: "gpt-4.1", messages: [], leaf: null };
    const { root, sent } = await show(conv);
    expect(root.querySelector('select[aria-label="Effort"]')).toBeNull();
    type(root.querySelector<HTMLInputElement>('input[aria-label="Temperature"]')!, "0.4");
    await send(root, "Hi");
    expect(sent[0].params).toMatchObject({ effort: null, temperature: 0.4 });
  });
});

describe("a Claude conversation switched to another model", () => {
  // Every request after the switch still carries the first model's reasoning, and Claude says it
  // left it out on every reply (once per reply it left out). The window says so once per switch.
  it("says once, on the first reply after the switch, that the earlier reasoning was left out", async () => {
    const conv: Conversation = { ...claudeChat(), model: "claude-sonnet-5" };
    const replies: (() => void)[] = [];
    const { root, sent } = await show(conv, ({ request, onEvent }) => new Promise((done) => {
      for (const path of ["messages.1.content.0", "messages.3.content.0"]) {
        channelSend(onEvent, { type: "notice", text: `Earlier reasoning (${path}) was left out: it came from another model.` });
      }
      channelSend(onEvent, { type: "text", text: "Pear" });
      replies.push(() => {
        channelEnd(onEvent);
        done({ message: { role: "assistant", content: "Pear", provider: request.provider, model: request.model, stopReason: "end_turn", createdAt: 3 }, error: null });
      });
    }));
    const notes = () => [...root.querySelectorAll(".msg-note")].map((n) => n.textContent ?? "").filter((t) => t.includes("another model"));
    const finish = async () => {
      await act(async () => {
        replies.shift()!();
        await new Promise((res) => setTimeout(res, 30));
      });
    };

    await send(root, "And a green one?");
    expect(sent[0].model).toBe("claude-sonnet-5");
    expect(notes()).toEqual(["Earlier reasoning (messages.1.content.0) was left out: it came from another model."]);
    await finish();
    await send(root, "Merci");
    expect(root.textContent).toContain("Pear");
    expect(notes()).toEqual([]);
    await finish();
  });
});

describe("look-only mode", () => {
  afterEach(() => _resetAdminForTests());
  const lookOnly = (on: boolean) => act(() => _resetAdminForTests({ unlocked: false, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: on }));
  const message = (root: HTMLElement) => root.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!;

  it("turns Send and Enter off, says why, and keeps what's typed", async () => {
    lookOnly(true);
    const { root, sent, saved } = await show(claudeChat());
    type(message(root), "And a green one?");
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).toBeDisabled();
    expect(send).toHaveAttribute("title", LOOK_ONLY_TIP);
    fireEvent.click(send);
    await act(async () => {
      fireEvent.keyDown(message(root), { key: "Enter" });
      await new Promise((res) => setTimeout(res, 30));
    });
    expect(sent).toHaveLength(0);
    expect(saved).toHaveLength(0);
    expect(message(root).value).toBe("And a green one?");
    expect(root.querySelectorAll(".msg.user"), "it isn't in the conversation").toHaveLength(1);

    // Asking again goes to a model too: Regenerate and Edit are off as well.
    expect(screen.getByRole("button", { name: /Regenerate/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Regenerate/ })).toHaveAttribute("title", LOOK_ONLY_TIP);
    expect(screen.getByRole("button", { name: "Edit and send again" })).toBeDisabled();

    // Look-only ends: what was typed goes.
    lookOnly(false);
    expect(screen.getByRole("button", { name: "Send" })).toHaveAttribute("title", "Send (Enter)");
    await act(async () => {
      fireEvent.keyDown(message(root), { key: "Enter" });
      await new Promise((res) => setTimeout(res, 30));
    });
    expect(sent).toHaveLength(1);
    expect(message(root).value).toBe("");
  });
});

describe("interrupted replies", () => {
  it("keeps the answer and warning when command completion overtakes the last channel events", async () => {
    const warning = "The connection closed before the reply finished (is cell1 still up?)";
    const partial: ChatMessage = { role: "assistant", content: "The botanist reached the garden", thinking: "A plan", provider: "local-qwen", model: "qwen3-0.6b", stopReason: "interrupted", createdAt: 3 };
    let late!: () => void;
    const { root, saved, sent, unmount } = await show({ ...claudeChat(), provider: "local-qwen", model: "qwen3-0.6b" }, ({ onEvent }) => {
      channelSend(onEvent, { type: "text", text: partial.content });
      // Tauri preserves ordering *within* a channel, not against the invoke response.
      late = () => {
        channelSend(onEvent, { type: "done", message: partial });
        channelSend(onEvent, { type: "error", message: warning });
        channelEnd(onEvent);
      };
      return { message: partial, error: warning };
    });
    await send(root, "Tell the story");
    expect(root.textContent).toContain(partial.content);
    expect(root.textContent).toContain("interrupted");
    expect(root.textContent).toContain(warning);
    const finished = saved.at(-1)!;
    expect(finished.messages.at(-1)).toMatchObject({ content: partial.content, thinking: partial.thinking, stopReason: "interrupted", provider: "local-qwen" });
    expect(sent).toHaveLength(1); // No automatic resend after reconnection.
    await act(async () => late());
    expect(saved).toHaveLength(2); // User turn, then exactly one assistant turn.
    unmount();
    const reopened = await show(finished, ({ onEvent }) => {
      channelEnd(onEvent);
      return { message: { ...partial, content: "47", thinking: null, stopReason: "stop" }, error: null };
    });
    expect(reopened.root.textContent).toContain(partial.content);
    expect(reopened.root.textContent).toContain("interrupted");
    expect(reopened.sent).toHaveLength(0); // Reopening history never starts inference.
    fireEvent.click(screen.getByRole("button", { name: "New chat" }));
    await send(reopened.root, "31 + 16?");
    expect(reopened.sent).toHaveLength(1);
    expect(reopened.saved.at(-1)?.messages.map((m) => m.content)).toEqual(["31 + 16?", "47"]);
    expect(reopened.root.textContent).not.toContain(warning);
  });

  it("keeps visible output on an IPC failure, and shows both interruption and save failures", async () => {
    const { root, sent, saved } = await show(claudeChat(), ({ onEvent }) => {
      channelSend(onEvent, { type: "text", text: "Keep this partial answer" });
      channelEnd(onEvent);
      throw new Error("Connection lost");
    });
    extendMock({ conversation_save: ({ conversation }: { conversation: Conversation }) => {
      if (conversation.messages.at(-1)?.role === "assistant") throw new Error("Disk full");
      saved.push(structuredClone(conversation));
    } });
    await send(root, "Continue");
    expect(root.textContent).toContain("Keep this partial answer");
    expect(root.textContent).toContain("interrupted");
    expect(root.textContent).toContain("Connection lost");
    expect(root.textContent).toContain("Disk full");
    expect(sent).toHaveLength(1);
    expect(saved).toHaveLength(1); // No false claim of a saved assistant turn.
  });
});
