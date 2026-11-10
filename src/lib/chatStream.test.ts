import { describe, expect, it } from "vitest";
import { api } from "./api";
import { chatSend } from "./modelsApi";
import type { ChatEvent } from "./types";
import { channelEnd, channelSend, mockCalls, mockTauri } from "../test/tauri";

const request = { requestId: "offline-chat", provider: "local", model: "qwen", system: "", params: {}, messages: [] };
const answer = { role: "assistant", content: "Partial story", provider: "local", model: "qwen", stopReason: "interrupted", createdAt: 1 };

describe.each([["Chat", chatSend], ["How To", api.chatSend]] as const)("%s completion boundary", (_name, send) => {
  it("delivers the authoritative answer and warning once, ignoring delayed progress and terminal callbacks", async () => {
    let late!: () => void;
    mockTauri({ chat_send: ({ onEvent }) => {
      channelSend(onEvent, { type: "text", text: "Partial" });
      late = () => {
        channelSend(onEvent, { type: "text", text: " story" });
        channelSend(onEvent, { type: "done", message: answer });
        channelSend(onEvent, { type: "error", message: "Connection closed" });
        channelEnd(onEvent);
      };
      return { message: answer, error: "Connection closed" };
    } });
    const events: ChatEvent[] = [];
    await send(request, (e) => events.push(e));
    expect(events).toEqual([{ type: "text", text: "Partial" }, { type: "done", message: answer }, { type: "error", message: "Connection closed" }]);
    late();
    expect(events).toHaveLength(3);
    expect(mockCalls.map((c) => c.cmd)).toEqual(["chat_send"]);
  });

  it("returns an error without inventing an empty assistant when nothing arrived", async () => {
    mockTauri({ chat_send: ({ onEvent }) => { channelEnd(onEvent); return { message: null, error: "Look-only" }; } });
    const events: ChatEvent[] = [];
    await send(request, (e) => events.push(e));
    expect(events).toEqual([{ type: "error", message: "Look-only" }]);
  });

  it("keeps partial text on rejected IPC, without invented usage or signatures, and ignores later events", async () => {
    let late!: () => void;
    mockTauri({ chat_send: ({ onEvent }) => {
      channelSend(onEvent, { type: "text", text: "Already visible" });
      late = () => { channelSend(onEvent, { type: "text", text: " after failure" }); channelEnd(onEvent); };
      throw new Error("IPC failed");
    } });
    const events: ChatEvent[] = [];
    await expect(send(request, (e) => events.push(e))).rejects.toThrow("IPC failed");
    expect(events).toHaveLength(2);
    expect(events[1]).toMatchObject({ type: "done", message: { content: "Already visible", stopReason: "interrupted" } });
    const message = (events[1] as Extract<ChatEvent, { type: "done" }>).message;
    expect(message.stats).toBeUndefined();
    expect(message.anthropicBlocks).toBeUndefined();
    late();
    expect(events).toHaveLength(2);
  });

  it("refuses a missing completion instead of silently discarding visible output", async () => {
    mockTauri({ chat_send: ({ onEvent }) => { channelSend(onEvent, { type: "text", text: "Visible" }); channelEnd(onEvent); return null; } });
    const events: ChatEvent[] = [];
    await expect(send(request, (e) => events.push(e))).rejects.toThrow("without a result");
    expect(events.at(-1)).toMatchObject({ type: "done", message: { content: "Visible", stopReason: "interrupted" } });
  });
});
