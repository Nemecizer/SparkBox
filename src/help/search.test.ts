import { describe, expect, it } from "vitest";
import { buildIndex, landing, matchedTerms, queryWords, search, searchOrNearest, segments, snippet, stem, suggestion, type SearchDoc } from "./search";

const doc = (id: string, title: string, extra: Partial<SearchDoc> = {}): SearchDoc => ({
  id, title, type: "feature", summary: "", keywords: [], headings: [], body: "", ...extra,
});

const ix = buildIndex([
  doc("download", "Download a model", { type: "howto", summary: "Fetch a model from Hugging Face to your first node.", keywords: ["hugging face", "get a model"], body: "Click Download. SparkBox checks every file." }),
  doc("load", "Load a model on one node", { type: "howto", summary: "Start serving a model.", body: "Choose the GPU memory share and press Load." }),
  doc("restart", "Restart a node", { type: "howto", summary: "Restart a node safely.", keywords: ["reboot"], body: "The node goes offline for a few minutes." }),
  doc("glossary", "Glossary", { type: "reference", summary: "Every word.", body: "A token is a small piece of text. A model is the trained AI you run." }),
  doc("errors", "Admin and safety messages", {
    type: "troubleshooting",
    summary: "What each message means.",
    body: "The fence doesn't allow this change.\nWhat it means. Only the test build uses a fence.\nDownloads\nSome other words about a download here.",
    headings: [{ level: 3, text: "Downloads", anchor: "downloads", at: 80 }],
  }),
  doc("cell1", "cell1 notes", { body: "cell1 is the first node." }),
  doc("cell3", "cell3 notes", { body: "cell3 is the third node." }),
]);

const ids = (q: string) => search(ix, q).map((h) => h.doc.id);

describe("stem", () => {
  it("folds plurals and verb endings, and leaves numbers alone", () => {
    expect(["downloads", "downloading", "downloaded"].map(stem)).toEqual(["download", "download", "download"]);
    expect(["stopped", "running", "copies", "processes", "models"].map(stem)).toEqual(["stop", "run", "copy", "process", "model"]);
    expect(stem("cell3")).toBe("cell3");
    expect(stem("still")).toBe("still");
    expect(stem("logs")).toBe("log");
  });

  it("folds a verb that ends in e", () => {
    for (const forms of [["update", "updates", "updated", "updating"], ["delete", "deleted", "deleting"], ["close", "closed", "closing"], ["save", "saves", "saving"]]) {
      expect(new Set(forms.map(stem)).size, forms[0]).toBe(1);
    }
  });

  it("keeps apart words that only look alike", () => {
    expect(stem("responding")).not.toBe(stem("respond"));
    expect(stem("theme")).not.toBe(stem("them"));
    expect(stem("mode")).not.toBe(stem("model"));
  });
});

describe("search", () => {
  it("puts a title match first", () => {
    expect(ids("download a model")[0]).toBe("download");
    expect(ids("restart")[0]).toBe("restart");
  });

  it("drops common words, so a whole question works", () => {
    expect(queryWords("how do I load a model")).toEqual(["load", "model"]);
    expect(ids("how do I load a model")[0]).toBe("load");
  });

  it("finds word forms", () => {
    expect(ids("downloading models")[0]).toBe("download");
  });

  it("forgives a typo, without changing a number", () => {
    expect(ids("downlaod")[0]).toBe("download");
    expect(ids("restrat")[0]).toBe("restart");
    expect(ids("cell3")).toContain("cell3");
    expect(ids("cell3")).not.toContain("cell1");
    expect(ids("cel3")).not.toContain("cell1");
  });

  it("matches as you type", () => {
    expect(ids("downl")[0]).toBe("download");
    expect(ids("glos")[0]).toBe("glossary");
  });

  it("knows the words a reader may use for SparkBox's", () => {
    expect(ids("reboot")[0]).toBe("restart");
    expect(ids("deploy model")[0]).toBe("load");
    expect(ids("ram")).toContain("load");
  });

  it("finds a pasted message, punctuation and all", () => {
    expect(ids("The fence doesn't allow this change.")[0]).toBe("errors");
  });

  it("says so when no article has every word", () => {
    const hits = search(ix, "restart glossary");
    expect(hits.length).toBeGreaterThan(0);
    expect(hits.every((h) => h.partial)).toBe(true);
    expect(search(ix, "download model")[0].partial).toBe(false);
  });

  it("keeps an article that has most of the words, even when another has them all", () => {
    const six = buildIndex([
      doc("a", "Download a model", { type: "howto", summary: "Fetch a model." }),
      doc("b", "Long notes", { body: "Words about a model, a download and a transfer, among many others." }),
    ]);
    const hits = search(six, "download model transfer");
    expect(hits.map((h) => h.doc.id)).toEqual(expect.arrayContaining(["a", "b"]));
    expect(hits.find((h) => h.doc.id === "a")!.partial).toBe(true);
    expect(hits.find((h) => h.doc.id === "b")!.partial).toBe(false);
  });

  it("treats a word with an apostrophe as one word", () => {
    expect(stem("doesn't")).toBe("doesnt");
    expect(queryWords("the model won't load")).toEqual(["model", "load"]);
    expect(queryWords("what's a node")).toEqual(["node"]);
  });

  it("expands the last word as a prefix only when it isn't a word itself, and barely then", () => {
    const six = buildIndex([doc("a", "Admin mode"), doc("b", "Model list", { body: "model model model model" })]);
    expect(search(six, "admin mode").map((h) => h.doc.id)).toEqual(["a"]);
    expect(matchedTerms(six, "mode")).not.toContain("model");
    expect(matchedTerms(six, "mod")).toContain("model");
  });

  it("never expands two letters", () => {
    expect(search(buildIndex([doc("a", "Identity pin")]), "id")).toEqual([]);
    const six = buildIndex([doc("a", "Identity pin"), doc("b", "Touch id")]);
    expect(search(six, "id").map((h) => h.doc.id)).toEqual(["b"]);
  });

  it("gives a long page no credit for its length", () => {
    const six = buildIndex([
      doc("short", "Keep notes", { type: "howto", body: "Pin the driver here." }),
      doc("long", "A long page", { body: `pin ${"other words ".repeat(1500)} pin pin pin` }),
      doc("c", "Filler", { body: "other words" }),
    ]);
    expect(search(six, "pin")[0].doc.id).toBe("short");
  });

  it("lowers a page of messages unless a heading is what was asked", () => {
    const body = (word: string) => `${word} and other words about it.`;
    const six = buildIndex([
      doc("guide", "Notes", { type: "feature", body: body("widget") }),
      doc("errors", "Messages", {
        type: "troubleshooting",
        body: `${body("widget")} ${body("offline")}`,
        headings: [{ level: 3, text: "A widget shows Offline", anchor: "a-widget-shows-offline", at: 0 }],
      }),
    ]);
    expect(search(six, "widget")[0].doc.id).toBe("guide");
    expect(search(six, "widget offline")[0].doc.id).toBe("errors");
  });

  it("gives a phrase the writer listed the credit of a title", () => {
    const six = buildIndex([doc("a", "Install updates", { keywords: ["update the cluster"] }), doc("b", "Update history", { body: "cluster update cluster update" })]);
    expect(search(six, "how do I update the cluster")[0].doc.id).toBe("a");
  });

  it("reads a typo as the word it stands for, in phrases too", () => {
    const six = buildIndex([doc("a", "Install updates", { keywords: ["update the cluster"] }), doc("b", "Notes", { body: "cluster update cluster update" })]);
    expect(search(six, "updte the cluster")[0].doc.id).toBe("a");
  });

  it("parts words that run together, but not a word still being typed", () => {
    const six = buildIndex([doc("a", "Admin mode"), doc("b", "Administrator notes")]);
    expect(search(six, "adminmode")[0].doc.id).toBe("a");
    expect(search(six, "administ")[0].doc.id).toBe("b");
  });

  it("gives every way of saying can't the same phrase", () => {
    const six = buildIndex([doc("a", "Fix a load", { keywords: ["model wont load"] }), doc("b", "Load a model", { body: "load model load model" })]);
    for (const q of ["model wont load", "the model cannot load", "model failed to load", "model did not load", "model couldn't load"]) {
      expect(search(six, q)[0].doc.id, q).toBe("a");
    }
  });

  it("matches a listed phrase in any order, or with more said around it", () => {
    const six = buildIndex([doc("a", "Pin", { keywords: ["driver version"] }), doc("b", "Notes", { body: "version driver version driver" })]);
    expect(search(six, "version of the driver")[0].doc.id).toBe("a");
    expect(search(six, "which driver version do i have now")[0].doc.id).toBe("a");
  });

  it("doesn't let a word for doing something outrank what it's done to", () => {
    const six = buildIndex([doc("a", "Tell if a model works"), doc("b", "Demo and Live", { body: "Demo data is simulated." })]);
    expect(search(six, "tell if i am in demo")[0].doc.id).toBe("b");
  });

  it("drops the app's own name when something else was asked", () => {
    expect(queryWords("which ports does sparkbox use")).toEqual(["ports", "use"]);
    expect(queryWords("sparkbox")).toEqual(["sparkbox"]);
  });

  it("reads a node's name as a node, unless it's all that was typed", () => {
    const six = buildIndex([doc("a", "Open a terminal on a node"), doc("b", "cell2 notes", { body: "cell2 cell2" })]);
    expect(search(six, "terminal on cell2")[0].doc.id).toBe("a");
    expect(search(six, "cell2")[0].doc.id).toBe("b");
  });

  it("keeps a test-build page back until the query asks for it", () => {
    const six = buildIndex([
      doc("fence", "Let the test build make changes", { testBuild: true }),
      doc("admin", "Admin mode", { summary: "Unlock it to make changes." }),
      ...["a", "b", "c", "d", "e", "f"].map((id) => doc(id, `Other page ${id}`)),
    ]);
    expect(search(six, "make changes")[0].doc.id).toBe("admin");
    expect(search(six, "test build make changes")[0].doc.id).toBe("fence");
  });

  it("puts the Glossary's entry first for a definition, after a page all about the word", () => {
    const six = buildIndex([
      doc("howto", "Pin the widget stack", { type: "howto", body: "widget widget widget" }),
      doc("guide", "Notes", { body: "A widget here." }),
      doc("glossary", "Glossary", { type: "reference", body: "Widget\nA thing.\nGizmo\nAnother thing.", headings: [{ level: 3, text: "Widget", anchor: "widget", at: 0 }, { level: 3, text: "Gizmo", anchor: "gizmo", at: 16 }] }),
      doc("concept", "Gizmos", { type: "concept", body: "gizmo" }),
    ]);
    expect(search(six, "what is a widget")[0].doc.id).toBe("glossary");
    expect(search(six, "what does gizmo mean").map((h) => h.doc.id).slice(0, 2)).toEqual(["concept", "glossary"]);
    expect(search(six, "widget")[0].doc.id).toBe("howto");
  });

  it("doesn't take a misspelled phrase for a word", () => {
    const six = buildIndex([
      doc("right", "Load a model", { type: "howto" }),
      doc("listed", "Fix a load", { phrases: ["laod failed"] }),
    ]);
    expect(search(six, "laod a model")[0].doc.id).toBe("right");
  });

  it("still finds a page by a real word only its phrases have", () => {
    const six = buildIndex([doc("a", "Quiet hours", { phrases: ["gibberish everywhere"] }), doc("b", "Other page")]);
    expect(search(six, "gibberish").map((h) => h.doc.id)).toEqual(["a"]);
  });

  it("reads two letters swapped as the word they make, before a commoner near one", () => {
    const six = buildIndex([
      ...["a", "b", "c", "d"].map((id) => doc(id, `Admin mode ${id}`)),
      doc("right", "Model list"),
    ]);
    expect(search(six, "modle list")[0].doc.id).toBe("right");
  });

  it("matches a phrase listed for an article, however it was asked", () => {
    const six = buildIndex([doc("a", "Fetch one", { keywords: ["get a model"] }), doc("b", "Model list", { body: "model model" })]);
    expect(search(six, "how do i get a model")[0].doc.id).toBe("a");
  });

  it("finds nothing for nothing", () => {
    expect(search(ix, "   ")).toEqual([]);
    expect(search(ix, "zzzzzzzz")).toEqual([]);
  });
});

describe("what a result shows", () => {
  it("marks the matching words", () => {
    const hit = search(ix, "download model")[0];
    expect(segments(hit.doc.title, hit.terms).filter((s) => s.hit).map((s) => s.text)).toEqual(["Download", "model"]);
  });

  it("shows the words around the match and names the heading above it", () => {
    const hit = search(ix, "downloads").find((h) => h.doc.id === "errors")!;
    const s = snippet(hit);
    expect(s.segs.some((x) => x.hit)).toBe(true);
    expect(s.anchor).toBe("downloads");
    expect(s.heading).toBe("Downloads");
  });

  it("falls back to the summary when the body doesn't have the word", () => {
    const hit = search(ix, "hugging").find((h) => h.doc.id === "download")!;
    expect(snippet(hit).segs.map((x) => x.text).join("")).toContain("Hugging Face");
  });

  describe("around a heading", () => {
    const body = "Load it, then wait for Ready.\nBefore you start\nIf this is your first load, read this page.\nNext steps\nChat with the model.\n";
    const headings = [
      { level: 2, text: "Before you start", anchor: "before-you-start", at: 30 },
      { level: 2, text: "Next steps", anchor: "next-steps", at: 91 },
    ];
    const six = buildIndex([doc("a", "Load", { body, headings })]);
    const text = (q: string) => snippet(search(six, q)[0]).segs.map((s) => s.text).join("");

    it("stays in one section", () => {
      expect(text("ready")).toBe("Load it, then wait for Ready.");
      expect(text("chat")).toBe("Chat with the model.");
    });

    it("starts below the heading above it, and stops at the next", () => {
      expect(text("first")).toBe("If this is your first load, read this page.");
    });

    it("keeps a heading the query matched, set apart from its text", () => {
      expect(text("before")).toBe("Before you start · If this is your first load, read this page.");
    });
  });

  it("gives the terms to mark in an article", () => {
    expect(matchedTerms(ix, "downloading")).toContain("download");
  });
});

describe("where a result opens", () => {
  const six = buildIndex([
    doc("howto", "Check disk space", {
      type: "howto",
      body: "Before you start\nHave a node.\nSteps\nOpen the disks tab and check.\nCancel a scan\nPress Stop.",
      headings: [
        { level: 2, text: "Before you start", anchor: "before-you-start", at: 0 },
        { level: 2, text: "Steps", anchor: "steps", at: 30 },
        { level: 2, text: "Cancel a scan", anchor: "cancel-a-scan", at: 60 },
      ],
    }),
    doc("errors", "Messages", {
      type: "troubleshooting",
      body: "Disk is full\nWhat it means. Your disk is full.",
      headings: [{ level: 3, text: "Disk is full", anchor: "disk-is-full", at: 0 }],
    }),
  ]);
  const hit = (q: string, id: string) => search(six, q).find((h) => h.doc.id === id)!;

  it("at the top when the query names the article", () => {
    expect(landing(hit("check disk space", "howto"), "check disk space")).toBeUndefined();
  });

  it("never at a heading every article has", () => {
    expect(landing(hit("steps", "howto"), "steps")).toBeUndefined();
  });

  it("at a section the query names", () => {
    expect(landing(hit("cancel a scan", "howto"), "cancel a scan")).toBe("cancel-a-scan");
    expect(landing(hit("disk full", "errors"), "disk full")).toBe("disk-is-full");
  });

  it("at the top of a page of entries when the query names the page", () => {
    expect(landing(hit("messages", "errors"), "messages")).toBeUndefined();
  });
});

describe("searchOrNearest", () => {
  it("says so when it searched for the nearest spelling", () => {
    const six = buildIndex([doc("a", "Engines", { body: "vllm vllm" }), doc("b", "Editing", { body: "vim" })]);
    const { hits, instead } = searchOrNearest(six, "vlm");
    expect(instead).toBe("vllm");
    expect(hits[0].doc.id).toBe("a");
    expect(searchOrNearest(six, "engines").instead).toBeUndefined();
    expect(searchOrNearest(six, "zzzzzzzzzz")).toEqual({ hits: [] });
  });
});

describe("suggestion", () => {
  it("offers the nearest words when nothing is found", () => {
    expect(suggestion(ix, "glosary tokne")).toBe("glossary token");
    expect(suggestion(ix, "model")).toBeNull();
    expect(suggestion(ix, "zzzzzzzzzz")).toBeNull();
  });
});
