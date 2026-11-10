// The line under each reply. Speed comes from the backend's shared rule
// (src-tauri/src/llm/throughput.rs): only a stream long enough to measure gets a
// tokens-per-second figure; anything else reads "70 tokens in 1.6 s".
import type { ChatStats } from "../../lib/modelsApi";

const STOP: Record<string, string> = {
  end_turn: "finished",
  stop: "finished",
  completed: "finished",
  stop_sequence: "hit a stop sequence",
  length: "hit the length limit",
  max_tokens: "hit the length limit",
  incomplete: "hit the length limit",
  model_context_window_exceeded: "ran out of context",
  tool_use: "wants to use a tool",
  tool_calls: "wants to use a tool",
  refusal: "declined",
  pause_turn: "paused",
  stopped: "stopped by you",
};

export function stopWord(reason: string | null | undefined): string | null {
  if (!reason) return null;
  return STOP[reason] ?? reason.replace(/_/g, " ");
}

function secs(ms: number): string {
  return ms >= 10_000 ? `${Math.round(ms / 1000)} s` : `${(ms / 1000).toFixed(1)} s`;
}

export function statsLine(s: ChatStats | null | undefined, stopReason?: string | null): string {
  if (!s) return stopWord(stopReason) ?? "";
  const parts: string[] = [];
  if (s.ttftMs != null) parts.push(`${secs(s.ttftMs)} to first token`);
  if (s.tokensPerSec != null && s.outputTokens != null) {
    parts.push(`${s.tokensPerSec >= 100 ? s.tokensPerSec.toFixed(0) : s.tokensPerSec.toFixed(1)} tok/s`);
    parts.push(`${s.outputTokens.toLocaleString()} tokens`);
    parts.push(`${secs(s.totalMs)} total`);
  } else if (s.outputTokens != null) {
    parts.push(`${s.outputTokens.toLocaleString()} tokens in ${secs(s.totalMs)}`);
  } else {
    parts.push(secs(s.totalMs));
  }
  if (s.inputTokens != null) parts.push(`${s.inputTokens.toLocaleString()} in`);
  const stop = stopWord(stopReason);
  if (stop) parts.push(stop);
  return parts.join(" · ");
}
