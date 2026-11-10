import { describe, expect, it } from "vitest";
import { parseGlossary } from "./definitions";

const SOURCE = `# Glossary

Intro words that belong to no entry.

## A

### Agent

See [always-on agent](#always-on-agent).

### Always-on agent

A small **service** on a node. See [Alerts](help:alerts-window).

## Numbers

### /dev/shm

A folder in memory. It holds [a node's](#agent) data.
`;

describe("the Glossary as definitions", () => {
  const all = parseGlossary(SOURCE);

  it("has an entry for each term, by the anchor Help links to", () => {
    expect([...all.keys()]).toEqual(["agent", "always-on-agent", "devshm"]);
    expect(all.get("devshm")!.term).toBe("/dev/shm");
  });

  it("shows the entry a See line points to, under its own name", () => {
    expect(all.get("agent")!.term).toBe("Agent");
    expect(all.get("agent")!.html).toContain("A small <strong>service</strong> on a node.");
  });

  it("keeps links working from outside the Glossary", () => {
    expect(all.get("devshm")!.html).toContain('href="help:glossary#agent"');
    expect(all.get("always-on-agent")!.html).toContain('href="help:alerts-window"');
  });
});
