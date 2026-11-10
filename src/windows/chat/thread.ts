// Conversations as trees: an edited question or a regenerated reply becomes a
// sibling under the same parent, and `leaf` picks the branch that's shown.
// Older conversations (a flat list) get ids and parents on first open.
import type { ChatMessage, Conversation } from "../../lib/modelsApi";

export function newId(): string {
  return crypto.randomUUID();
}

/** Gives a flat, older conversation ids and parent links, in order. */
export function ensureTree(c: Conversation): Conversation {
  if (c.messages.every((m) => m.id)) {
    return { ...c, leaf: c.leaf ?? c.messages[c.messages.length - 1]?.id ?? null };
  }
  let parent: string | null = null;
  const messages = c.messages.map((m) => {
    const id = m.id ?? newId();
    const out = { ...m, id, parentId: m.id ? m.parentId ?? null : parent };
    parent = id;
    return out;
  });
  return { ...c, messages, leaf: messages[messages.length - 1]?.id ?? null };
}

/** The messages from the root down to `leaf` (the branch on screen, and what's sent). */
export function thread(c: Conversation, leaf: string | null | undefined = c.leaf): ChatMessage[] {
  const byId = new Map(c.messages.map((m) => [m.id, m]));
  const out: ChatMessage[] = [];
  let cur = leaf ? byId.get(leaf) : undefined;
  const seen = new Set<string>();
  while (cur && !seen.has(cur.id ?? "")) {
    seen.add(cur.id ?? "");
    out.push(cur);
    cur = cur.parentId ? byId.get(cur.parentId) : undefined;
  }
  return out.reverse();
}

/** Messages that share `m`'s parent (itself included), oldest first. */
export function siblings(c: Conversation, m: ChatMessage): ChatMessage[] {
  return c.messages.filter((x) => (x.parentId ?? null) === (m.parentId ?? null) && x.role === m.role).sort((a, b) => a.createdAt - b.createdAt);
}

/** The newest branch below `id`: follows the latest child at every step. */
export function latestLeaf(c: Conversation, id: string): string {
  let cur = id;
  for (;;) {
    const kids = c.messages.filter((m) => m.parentId === cur).sort((a, b) => b.createdAt - a.createdAt || 0);
    if (!kids.length || !kids[0].id) return cur;
    cur = kids[0].id;
  }
}

/** Adds `m` under `parentId` and shows it. */
export function append(c: Conversation, m: ChatMessage, parentId: string | null): Conversation {
  const msg = { ...m, id: m.id ?? newId(), parentId };
  return { ...c, messages: [...c.messages, msg], leaf: msg.id };
}
