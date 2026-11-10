// Small pieces the Models, Endpoints, Chat and Server windows share.
import { useEffect, useMemo, useState } from "react";
import { isLookOnly, LOOK_ONLY_TIP } from "../../lib/admin";
import { useCluster } from "../../lib/store";
import type { Endpoint, Engine } from "../../lib/endpoints";
import { openChatWith } from "../../lib/endpoints";
import { useWM } from "../../wm/WindowManager";
import "../ai.css";

export interface NodeRef {
  id: string;
  name: string;
  online: boolean;
}

export function useNodes(): NodeRef[] {
  const cluster = useCluster();
  return useMemo(() => (cluster?.nodes ?? []).map((n) => ({ id: n.id, name: n.name, online: n.status === "online" })), [cluster]);
}

export function errText(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e).replace(/^Error: /, "");
}

export const locked = (admin: boolean) => (admin ? undefined : isLookOnly() ? LOOK_ONLY_TIP : "Unlock admin mode (top bar) to change models");

// ---- Time

let tickers = 0;
const tickListeners = new Set<() => void>();
let tickTimer: number | undefined;

/** Re-renders every 15 s so relative times stay true. */
function useMinuteTick() {
  const [, set] = useState(0);
  useEffect(() => {
    const l = () => set((n) => n + 1);
    tickListeners.add(l);
    if (tickers++ === 0) tickTimer = window.setInterval(() => tickListeners.forEach((f) => f()), 15_000);
    return () => {
      tickListeners.delete(l);
      if (--tickers === 0) window.clearInterval(tickTimer);
    };
  }, []);
}

export function relTime(unixS: number, now = Date.now() / 1000): string {
  if (unixS - now > 30) {
    const ahead = unixS - now;
    if (ahead < 3600) return `in ${Math.round(ahead / 60)} min`;
    if (ahead < 86400) return `in ${Math.round(ahead / 3600)} h`;
    return `on ${new Date(unixS * 1000).toLocaleDateString([], { day: "numeric", month: "short" })}`;
  }
  const d = Math.max(0, now - unixS);
  if (d < 45) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 7 * 86400) return `${Math.round(d / 86400)} d ago`;
  return new Date(unixS * 1000).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
}

export function absTime(unixS: number): string {
  return new Date(unixS * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "medium" });
}

/** "5 min ago", with the date and time in the tooltip. `ts` is unix seconds. */
export function Ago({ ts }: { ts: number }) {
  useMinuteTick();
  if (!ts) return <span className="muted">—</span>;
  return (
    <time dateTime={new Date(ts * 1000).toISOString()} title={absTime(ts)}>
      {relTime(ts)}
    </time>
  );
}

// ---- Numbers

export function fmtTps(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  if (v === 0) return "0";
  return v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
}

export function fmtMs(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v >= 1000 ? `${(v / 1000).toFixed(v >= 10_000 ? 0 : 1)} s` : `${Math.round(v)} ms`;
}

export function fmtTokens(n: number | null | undefined): string {
  if (n == null) return "—";
  // Context lengths are powers of two: 32768 reads as 32k, as model cards write it.
  if (n >= 1024 && n % 1024 === 0 && n < 10_485_760) return `${n / 1024}k`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k`;
  return n.toLocaleString();
}

// ---- Endpoint pieces

export const ENGINE_LABEL: Record<Engine, string> = {
  vllm: "vLLM",
  ollama: "Ollama",
  sglang: "SGLang",
  llamacpp: "llama.cpp",
  trtllm: "TensorRT-LLM",
  nim: "NIM",
  other: "Engine",
};

/** Opens Chat on a model; prefers the window manager when the caller has it. */
export function useOpenChat(): (providerId: string, model: string) => void {
  const wm = useWM();
  return (providerId, model) => {
    openChatWith(providerId, model);
    wm.open("chat");
  };
}

/** Where a copied request has to run: a server on loopback answers on its own node only. */
export function runsOnNode(e: Pick<Endpoint, "listen" | "headNode">): string | null {
  return e.listen === "local" ? `Run this on ${e.headNode}: it listens on 127.0.0.1 there.` : null;
}

/** A copy-and-paste request for an endpoint. The key stays in the Keychain: Chat and the Server use it for you. */
export function curlFor(e: Pick<Endpoint, "url" | "models" | "managedBy" | "deploymentId" | "listen" | "headNode">, model?: string): string {
  const m = model ?? e.models[0] ?? "model";
  const lines: string[] = [];
  const where = runsOnNode(e);
  if (where) lines.push(`# ${where}`);
  if (e.managedBy === "sparkbox") lines.push(`# This load needs its API key (Keychain item llm:vllm-${e.deploymentId ?? "<id>"}), or call it through SparkBox Server.`);
  lines.push(`curl ${e.url.replace(/\/$/, "")}/chat/completions \\`, `  -H "Content-Type: application/json" \\`);
  if (e.managedBy === "sparkbox") lines.push(`  -H "Authorization: Bearer $VLLM_API_KEY" \\`);
  lines.push(`  -d '{"model": "${m}", "messages": [{"role": "user", "content": "Hello"}]}'`);
  return lines.join("\n");
}
