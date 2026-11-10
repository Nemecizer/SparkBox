import { describe, expect, it } from "vitest";
import { headingsOf, plainDoc, renderHelp, slug } from "./render";

const dom = (html: string) => {
  const d = document.createElement("div");
  d.innerHTML = html;
  return d;
};

describe("anchors", () => {
  it("follow STYLE.md: lower case, hyphens, no punctuation, & as and", () => {
    expect(slug("What's on the screen")).toBe("whats-on-the-screen");
    expect(slug("Containers & services")).toBe("containers-and-services");
    expect(slug('"Port 8000 is taken"')).toBe("port-8000-is-taken");
    expect(slug("Step 1: Open Models")).toBe("step-1-open-models");
  });

  it("number a repeated heading, the same way for pages and search", () => {
    const src = "# T\n\n## Steps\n\n### Stop\n\n## Steps\n\n### `/dev/shm` & **more**\n";
    expect(headingsOf(src).map((h) => h.anchor)).toEqual(["steps", "stop", "steps-2", "devshm-and-more"]);
    expect(renderHelp(src).headings.map((h) => h.anchor)).toEqual(["steps", "stop", "steps-2", "devshm-and-more"]);
  });
});

describe("renderHelp", () => {
  it("leaves the title to the window and marks headings for On this page", () => {
    const d = dom(renderHelp("# Title\n\nGoal.\n\n## Steps\n").html);
    expect(d.querySelector("h1")).toBeNull();
    expect(d.querySelector("h2")?.getAttribute("data-anchor")).toBe("steps");
  });

  it("turns the four callouts into notes, with the words kept and the marker gone", () => {
    for (const [kind, label] of [["NOTE", "Note"], ["TIP", "Tip"], ["WARNING", "Warning"], ["DANGER", "Danger"]]) {
      const d = dom(renderHelp(`> [!${kind}]\n> **What could go wrong.** How to undo it.\n`).html);
      const box = d.querySelector(`.help-callout.${kind.toLowerCase()}`)!;
      expect(box.getAttribute("role")).toBe("note");
      expect(box.querySelector(".help-callout-title")?.textContent).toBe(label);
      expect(box.querySelector(".help-callout-title svg")).not.toBeNull();
      expect(box.querySelector("strong")?.textContent).toBe("What could go wrong.");
      expect(box.textContent).not.toContain("[!");
    }
  });

  it("keeps a plain quote a quote", () => {
    const d = dom(renderHelp("> A quote.\n").html);
    expect(d.querySelector("blockquote")?.textContent).toContain("A quote.");
  });

  it("numbers steps so they can stand out, from the list's own start", () => {
    const d = dom(renderHelp("3. Third\n4. Fourth\n").html);
    const items = [...d.querySelectorAll("ol.help-steps > li")];
    expect(items.map((i) => i.getAttribute("data-n"))).toEqual(["3", "4"]);
  });

  it("keeps sub-lists, tables and keys", () => {
    const d = dom(renderHelp("1. One\n   - sub\n\nPress <kbd>⌘K</kbd>.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n").html);
    expect(d.querySelector("ol.help-steps ul li")?.textContent).toBe("sub");
    expect(d.querySelector("kbd")?.textContent).toBe("⌘K");
    expect(d.querySelector(".help-table table td")?.textContent).toBe("1");
  });

  it("gives each kind of link its look and keeps its address", () => {
    const d = dom(renderHelp("[a](help:x#y) [b](open:models?tab=load) [c](https://huggingface.co)\n").html);
    const [a, b, c] = [...d.querySelectorAll("a")];
    expect(a.getAttribute("href")).toBe("help:x#y");
    expect(b.className).toBe("help-open");
    expect(b.getAttribute("href")).toBe("open:models?tab=load");
    expect(c.className).toBe("help-ext");
  });

  it("makes a bundled picture a figure, and shows the caption when it's missing", () => {
    const image = (src: string) => (src === "img/a.jpg" ? "/assets/a-123.jpg" : null);
    const one = dom(renderHelp("![The caption](img/a.jpg)\n", image).html);
    expect(one.querySelector("p")).toBeNull();
    expect(one.querySelector("figure img")?.getAttribute("src")).toBe("/assets/a-123.jpg");
    expect(one.querySelector("figcaption")?.textContent).toBe("The caption");
    const gone = dom(renderHelp("![Later](img/b.jpg)\n", image).html);
    expect(gone.querySelector("img")).toBeNull();
    expect(gone.querySelector("figure.missing figcaption")?.textContent).toBe("Later");
  });

  it("still strips anything active, though the content is bundled", () => {
    const d = dom(renderHelp('<script>alert(1)</script><img src=x onerror=alert(1)><iframe src="https://x"></iframe>\n\n[x](javascript:alert(1))\n').html);
    expect(d.querySelector("script, iframe")).toBeNull();
    expect(d.innerHTML).not.toContain("onerror");
    expect(d.innerHTML).not.toContain("javascript:");
  });

  it("gives code a Copy button only SparkBox can add", () => {
    const d = dom(renderHelp('```sh\ndf -h\n```\n\n<button class="code-copy">Mine</button>\n').html);
    expect([...d.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Copy"]);
  });
});

describe("plainDoc", () => {
  it("reads the words of every kind of block, and knows where headings fall", () => {
    const { text, headings } = plainDoc("# T\n\nIntro **bold** [link](help:x).\n\n## Steps\n\n1. Click `Load…`.\n\n> [!NOTE]\n> A note.\n\n| A |\n|---|\n| cell |\n");
    expect(text).toContain("Intro bold link.");
    expect(text).toContain("Click Load…");
    expect(text).toContain("A note.");
    expect(text).not.toContain("[!NOTE]");
    expect(text).toContain("cell");
    expect(text.slice(headings[0].at)).toMatch(/^Steps/);
  });
});

describe("a picture's size", () => {
  it("is marked 2x, so a page shows it at half its pixels and enlarging uses them all", () => {
    const only = (src: string) => (src === "img/a.jpg" ? "/bundled/img/a.jpg" : null);
    const img = dom(renderHelp("![c](img/a.jpg)\n", only).html).querySelector("img")!;
    expect(img.getAttribute("srcset")).toBe("/bundled/img/a.jpg 2x");
    expect(img.getAttribute("src")).toBe("/bundled/img/a.jpg");
    expect(img.hasAttribute("data-retina")).toBe(false);
  });
});

describe("diagrams", () => {
  it("are a picture with a light and a dark file, marked 2x like the rest", () => {
    const html = renderHelp("![How it splits](img/split.svg)\n", (src) => `/bundled/${src}`).html;
    const [light, dark] = [...dom(html).querySelectorAll("img")];
    expect(light.getAttribute("src")).toBe("/bundled/img/split.svg");
    expect(light).toHaveClass("for-light");
    expect(light.getAttribute("srcset")).toBe("/bundled/img/split.svg 2x");
    expect(dark.getAttribute("src")).toBe("/bundled/img/split-dark.svg");
    expect(dark).toHaveClass("for-dark");
  });
});

describe("pictures", () => {
  const image = (src: string) => `/bundled/${src}`;
  const only = (src: string) => (src.endsWith("-dark.jpg") ? null : `/bundled/${src}`);

  it("are reachable from the keyboard, with their caption as text", () => {
    const img = dom(renderHelp("![The Disks tab](img/a.jpg)\n", only).html).querySelector("img")!;
    expect(img).toHaveClass("help-shot");
    expect(img.getAttribute("tabindex")).toBe("0");
    expect(img.getAttribute("role")).toBe("button");
    expect(img.alt).toBe("The Disks tab");
  });

  it("come in one version until a dark one exists", () => {
    expect(dom(renderHelp("![c](img/a.jpg)\n", only).html).querySelectorAll("img")).toHaveLength(1);
  });

  it("come in a version for each theme when both exist", () => {
    const imgs = dom(renderHelp("![c](img/a.jpg)\n", image).html).querySelectorAll("img");
    expect(imgs).toHaveLength(2);
    expect(imgs[0]).toHaveClass("for-light");
    expect(imgs[0].getAttribute("src")).toBe("/bundled/img/a.jpg");
    expect(imgs[1]).toHaveClass("for-dark");
    expect(imgs[1].getAttribute("src")).toBe("/bundled/img/a-dark.jpg");
  });
});
