import { describe, expect, it } from "vitest";
import { renderHelp } from "./render";
import terms from "../../docs/help/terms.json";
import { compileTerms, type TermSpec } from "./terms";

const SPECS: TermSpec[] = [
  { anchor: "port", forms: ["port", "ports"], not: "switch ports?" },
  { anchor: "api", forms: ["API"], exact: true },
  { anchor: "api-key", forms: ["API key"] },
  { anchor: "docker", forms: ["Docker"], exact: true, skip: ["about-docker"] },
];
const TERMS = compileTerms(SPECS, new Map([["port", "A numbered door."]]));

const links = (src: string, id?: string) => {
  const d = document.createElement("div");
  d.innerHTML = renderHelp(src, undefined, { terms: TERMS, id }).html;
  return [...d.querySelectorAll("a.help-term")].map((a) => a.textContent);
};

describe("code spans", () => {
  const CODE = compileTerms([{ anchor: "tilde", forms: [], code: "^~/" }, { anchor: "api", forms: ["API"], exact: true }]);
  const html = (src: string) => renderHelp(src, undefined, { terms: CODE }).html;

  it("link once, when the span is the term", () => {
    const d = document.createElement("div");
    d.innerHTML = html("Files go to `~/models` and `~/.sparkbox`.\n");
    const a = d.querySelectorAll("a.help-term");
    expect(a).toHaveLength(1);
    expect(a[0].getAttribute("href")).toBe("help:glossary#tilde");
    expect(a[0].querySelector("code")?.textContent).toBe("~/models");
    expect(d.querySelectorAll("code")).toHaveLength(2);
  });

  it("leave a term alone inside other code, or in a quoted message", () => {
    expect(html("Run `ls ~/models` here.\n")).not.toContain("help-term");
    expect(html('It says "see `~/models`" here.\n')).not.toContain("help-term");
  });

  it("a term with no forms matches no plain word", () => {
    expect(html("The tilde and ~/models in text.\n")).not.toContain("help-term");
  });
});

describe("words that look like a term", () => {
  const real = compileTerms(terms.terms as TermSpec[]);
  const linked = (src: string) => {
    const d = document.createElement("div");
    d.innerHTML = renderHelp(src, undefined, { terms: real, id: "some-article" }).html;
    return [...d.querySelectorAll("a.help-term")].map((a) => a.getAttribute("href"));
  };

  it("an engine that isn't a model's stays plain", () => {
    expect(linked("The Mac's web engine draws it. Its health engine rates it.\n")).toEqual([]);
    expect(linked("The tunnel engine holds it.\n")).not.toContain("help:glossary#engine");
    expect(linked("The engine runs it.\n")).toEqual(["help:glossary#engine"]);
  });

  it("a token in a list of secrets stays plain", () => {
    expect(linked("Never type a password, token or key.\n")).toEqual([]);
    expect(linked("Count each token.\n")).toEqual(["help:glossary#token"]);
  });
});

describe("first uses", () => {
  it("links each term once, the first time, with its definition", () => {
    const d = document.createElement("div");
    d.innerHTML = renderHelp("A port here. Another port there.\n\nA third port.\n", undefined, { terms: TERMS }).html;
    const a = d.querySelectorAll("a.help-term");
    expect(a).toHaveLength(1);
    expect(a[0].getAttribute("href")).toBe("help:glossary#port");
    expect(a[0].getAttribute("title")).toBe("A numbered door.");
    expect(d.textContent).toContain("A port here. Another port there.");
  });

  it("prefers the longer term", () => {
    expect(links("Paste your API key.\n")).toEqual(["API key"]);
    expect(links("The API, then the API key.\n")).toEqual(["API", "API key"]);
  });

  it("leaves the app's words, code, links and headings alone", () => {
    expect(links("## The port\n\n**Port**\n")).toEqual([]);
    expect(links("`port` and [the port](help:x) and <kbd>port</kbd>\n")).toEqual([]);
    expect(links('The app says "Port 8000 is taken" here.\n')).toEqual([]);
    expect(links("| Port |\n|---|\n| A cell |\n")).toEqual([]);
  });

  it("links in tables, lists and callouts", () => {
    expect(links("| A |\n|---|\n| The port |\n")).toEqual(["port"]);
    expect(links("- The port\n")).toEqual(["port"]);
    expect(links("> [!NOTE]\n> The port here.\n")).toEqual(["port"]);
  });

  it("keeps a callout a callout after linking inside it", () => {
    const d = document.createElement("div");
    d.innerHTML = renderHelp("> [!WARNING]\n> **Careful.** The port here.\n", undefined, { terms: TERMS }).html;
    expect(d.querySelector(".help-callout.warning a.help-term")?.textContent).toBe("port");
    expect(d.textContent).not.toContain("[!");
  });

  it("doesn't link what the article defines on the spot", () => {
    expect(links("A **port** is a numbered door. The port.\n")).toEqual([]);
    expect(links("The **port**, a numbered door. The port.\n")).toEqual([]);
    expect(links("SSH and a port, a numbered door.\n")).toEqual([]);
    expect(links("[The port](help:glossary#port) and a port.\n")).toEqual(["The port"]);
  });

  it("skips other senses of a word, and articles about the term", () => {
    expect(links("One switch port, then a port.\n")).toEqual(["port"]);
    expect(links("The switch ports.\n")).toEqual([]);
    expect(links("Docker runs here.\n", "about-docker")).toEqual([]);
    expect(links("Docker runs here.\n", "other")).toEqual(["Docker"]);
  });

  it("is case-sensitive for names, not for words", () => {
    expect(links("Ports are numbered.\n")).toEqual(["Ports"]);
    expect(links("docker group\n")).toEqual([]);
  });

  it("doesn't touch a word inside a longer one or a path", () => {
    expect(links("portable, airport, ~/port, port-forward\n")).toEqual([]);
  });
});
