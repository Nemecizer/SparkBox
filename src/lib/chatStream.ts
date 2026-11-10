import { Channel, type invoke } from "@tauri-apps/api/core";
import type { ChatEvent, ChatMessage, ChatParams } from "./types";

export interface ChatRequest {
  requestId: string;
  provider: string;
  model: string;
  system: string;
  params: ChatParams;
  messages: ChatMessage[];
}

export interface ChatCompletion {
  message: ChatMessage | null;
  error: string | null;
}

/** Deliver terminal values before resolving, regardless of Channel/invoke ordering.
 * A late progress callback belongs to the old request and must not reach a new one.
 * Both Chat and How To use this boundary; incomplete drafts must still be refused.
 */
export async function streamChat(request: ChatRequest, onEvent: (event: ChatEvent) => void, send: typeof invoke): Promise<void> {
  let receiving = true;
  let text = "";
  let thinking = "";
  const channel = new Channel<ChatEvent>((event) => {
    if (!receiving || event.type === "done" || event.type === "error") return;
    if (event.type === "text") text += event.text;
    if (event.type === "thinking") thinking += event.text;
    onEvent(event);
  });
  let completion: ChatCompletion;
  try {
    completion = await send<ChatCompletion>("chat_send", { request, onEvent: channel });
    if (!completion || (!completion.message && !completion.error)) {
      throw new Error("The reply ended without a result. Check the connection before asking again.");
    }
  } catch (error) {
    // If IPC itself failed, keep only output already received. Do not invent usage,
    // provider signatures or a successful finish, and never resend automatically.
    receiving = false;
    if (text || thinking) onEvent({ type: "done", message: {
      role: "assistant", content: text, thinking: thinking || null,
      provider: request.provider, model: request.model, stopReason: "interrupted",
      createdAt: Math.floor(Date.now() / 1000),
    } });
    throw error;
  } finally {
    receiving = false;
  }
  if (completion.message) onEvent({ type: "done", message: completion.message });
  if (completion.error) onEvent({ type: "error", message: completion.error });
}
