import { describe, expect, it } from "vitest";
import type { Conversation } from "../../lib/modelsApi";
import { append, ensureTree, latestLeaf, siblings, thread } from "./thread";

const base = (): Conversation => ({
  id: "c", title: "t", createdAt: 0, updatedAt: 0, provider: "demo", model: "echo", system: "", params: {},
  messages: [
    { role: "user", content: "q1", createdAt: 1 },
    { role: "assistant", content: "a1", createdAt: 2 },
    { role: "user", content: "q2", createdAt: 3 },
  ],
});

describe("conversation branches", () => {
  it("turns an old flat conversation into a chain", () => {
    const c = ensureTree(base());
    expect(thread(c).map((m) => m.content)).toEqual(["q1", "a1", "q2"]);
    expect(c.messages[1].parentId).toBe(c.messages[0].id);
    expect(ensureTree(c)).toEqual(c);
  });

  it("regenerating keeps both replies and shows the new one", () => {
    let c = ensureTree(base());
    const q1 = c.messages[0];
    c = append(c, { role: "assistant", content: "a1 again", createdAt: 9 }, q1.id!);
    expect(thread(c).map((m) => m.content)).toEqual(["q1", "a1 again"]);
    const replies = siblings(c, thread(c)[1]);
    expect(replies.map((m) => m.content)).toEqual(["a1", "a1 again"]);
    // Switching back to the first reply shows its whole branch again.
    const back = { ...c, leaf: latestLeaf(c, replies[0].id!) };
    expect(thread(back).map((m) => m.content)).toEqual(["q1", "a1", "q2"]);
  });

  it("editing a question starts a branch beside the original", () => {
    let c = ensureTree(base());
    const q2 = c.messages[2];
    c = append(c, { role: "user", content: "q2, edited", createdAt: 10 }, q2.parentId ?? null);
    expect(thread(c).map((m) => m.content)).toEqual(["q1", "a1", "q2, edited"]);
    expect(siblings(c, q2).length).toBe(2);
  });
});
