// Fake Tauri backend for unit tests and the browser preview (mock.html).
// Every `invoke` goes to a handler table; unknown commands reject with a clear
// message so a missing mock shows up as an error instead of a hang. Events go
// through Tauri's own mock event bus, so `listen()` in app code just works.
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

// Handlers take the invoke arguments as sent by app code (camelCase keys).
export type MockHandler = (args: any) => unknown;
export type MockHandlers = Record<string, MockHandler>;

export interface MockCall {
  cmd: string;
  args: Record<string, unknown>;
}

let table: MockHandlers = {};
/** Every command invoked since the last `mockTauri`, oldest first (capped). */
export const mockCalls: MockCall[] = [];
const channelIndex = new Map<number, number>();

/**
 * Installs `handlers` as the backend. Plugin commands without a handler
 * (window theme, autostart, opener…) resolve to null so UI code that pokes the
 * native side doesn't need a mock for each one.
 */
export function mockTauri(handlers: MockHandlers, opts: { events?: boolean } = {}) {
  table = { ...handlers };
  mockCalls.length = 0;
  channelIndex.clear();
  mockWindows("main");
  mockIPC(async (cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    mockCalls.push({ cmd, args: a });
    if (mockCalls.length > 500) mockCalls.shift();
    const h = table[cmd];
    if (h) return h(a);
    if (cmd.startsWith("plugin:")) return null;
    throw new Error(`No mock for "${cmd}". Add it to mockTauri() or src/dev/mocks.`);
  }, { shouldMockEvents: opts.events ?? true });
}

/** Adds or replaces handlers without resetting listeners or recorded calls. */
export function extendMock(handlers: MockHandlers) {
  Object.assign(table, handlers);
}

/** Delivers a backend event to every `listen(event)` in app code. */
export function emitMock(event: string, payload: unknown): Promise<void> {
  return emit(event, payload);
}

/** Sends one message down a `Channel` that app code passed to a command. */
export function channelSend(channel: unknown, message: unknown) {
  const id = (channel as { id: number }).id;
  const index = channelIndex.get(id) ?? 0;
  channelIndex.set(id, index + 1);
  window.__TAURI_INTERNALS__.runCallback(id, { index, message });
}

/** Closes a `Channel`, as Rust does when the command's stream ends. */
export function channelEnd(channel: unknown) {
  const id = (channel as { id: number }).id;
  window.__TAURI_INTERNALS__.runCallback(id, { index: channelIndex.get(id) ?? 0, end: true });
  channelIndex.delete(id);
}

declare global {
  interface Window {
    __TAURI_INTERNALS__: any;
  }
}
