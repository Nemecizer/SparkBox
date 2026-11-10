// The Help window against the real manifest and articles: how a reader moves around it. The
// tests name articles by id and title only, so they hold as the words change.
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WindowManagerProvider, type Params } from "../wm/WindowManager";
import HelpWindow from "./HelpWindow";
import { articleMeta, manifest, tocSection, TYPE_LABEL } from "./meta";
import { slug } from "./render";

const openUrl = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

const opened = vi.fn();

async function show(params: Params = {}) {
  let r: ReturnType<typeof render> | undefined;
  await act(async () => {
    r = render(
      <WindowManagerProvider sizeOf={() => [1000, 700]} isKnown={() => true} openElsewhere={opened}>
        <HelpWindow params={params} />
      </WindowManagerProvider>,
    );
  });
  return r!;
}

const shown = () => document.querySelector<HTMLElement>("[data-help-article]")?.dataset.helpArticle;
const search = () => screen.getByRole("searchbox", { name: "Search Help" });
const tree = () => screen.getByRole("tree", { name: "Help contents" });
const rowNamed = (name: string) => within(tree()).getByRole("treeitem", { name });
const type = async (text: string) => {
  await act(async () => {
    fireEvent.change(search(), { target: { value: text } });
  });
};

beforeEach(() => {
  localStorage.clear();
  opened.mockClear();
  openUrl.mockClear();
});

describe("finding your way", () => {
  it("opens on its home page, with every part of the table of contents", async () => {
    await show();
    expect(screen.getByRole("heading", { level: 1, name: "SparkBox Help" })).toBeInTheDocument();
    for (const s of manifest.toc) expect(within(tree()).getByRole("treeitem", { name: s.title })).toBeInTheDocument();
    expect(search()).toHaveFocus();
  });

  it("opens at the article a link asked for, on its heading", async () => {
    await show({ article: "check-disk-space", anchor: "steps" });
    expect(shown()).toBe("check-disk-space");
    expect(screen.getByRole("heading", { level: 1, name: articleMeta("check-disk-space")!.title })).toBeInTheDocument();
    expect(document.querySelector('[data-anchor="steps"]')).toHaveClass("help-target");
  });

  it("shows where you are: the eyebrow, the breadcrumb, the row in the contents", async () => {
    await show({ article: "check-disk-space" });
    expect(screen.getByText(TYPE_LABEL.howto)).toBeInTheDocument();
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(crumbs).toHaveTextContent("How-To Guides");
    expect(crumbs).toHaveTextContent("Watch your cluster");
    expect(rowNamed(articleMeta("check-disk-space")!.title)).toHaveAttribute("aria-selected", "true");
  });

  it("lists the page's headings and related articles", async () => {
    await show({ article: "check-disk-space" });
    const otp = screen.getByRole("complementary", { name: "On this page" });
    expect(within(otp).getByRole("link", { name: "Steps" })).toHaveAttribute("href", "#steps");
    for (const id of articleMeta("check-disk-space")!.related) {
      expect(document.querySelector(`.help-related a[href="help:${id}"]`)).not.toBeNull();
    }
  });

  it("comes back to where you were", async () => {
    const first = await show({ article: "check-disk-space" });
    first.unmount();
    await show();
    expect(shown()).toBe("check-disk-space");
  });

  it("falls back to its home page for a place that's gone", async () => {
    localStorage.setItem("sparkbox.help.last", JSON.stringify({ article: "no-such-article" }));
    await show();
    expect(screen.getByRole("heading", { level: 1, name: "SparkBox Help" })).toBeInTheDocument();
  });

  it("has the one calm line, only on articles not yet proven on the cluster", async () => {
    const unproven = manifest.articles.find((a) => a.verified === false)!;
    const a = await show({ article: unproven.id });
    expect(screen.getByText(manifest.calmLine)).toBeInTheDocument();
    a.unmount();
    localStorage.clear();
    // Public-export Help has no native acceptance claim. Exercise the verified
    // presentation branch with an explicit test fixture, then restore its status.
    const verified = manifest.articles.find((a) => a.id === "check-disk-space")!;
    const original = verified.verified;
    try {
      verified.verified = true;
      await show({ article: verified.id });
      expect(screen.queryByText(manifest.calmLine)).toBeNull();
    } finally {
      verified.verified = original;
    }
  });

  it("lists a section's articles from the breadcrumb", async () => {
    await show({ article: "check-disk-space" });
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    await act(async () => {
      fireEvent.click(within(crumbs).getByRole("button", { name: "How-To Guides" }));
    });
    expect(screen.getByRole("heading", { level: 1, name: "How-To Guides" })).toBeInTheDocument();
    expect(document.querySelector('.help-card[href="help:download-a-model"]')).not.toBeNull();
  });

  it("opens a group from the breadcrumb at that group, not at the top of the section", async () => {
    await show({ article: "download-a-model" });
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    await act(async () => {
      fireEvent.click(within(crumbs).getByRole("button", { name: "Run models" }));
    });
    expect(screen.getByRole("heading", { level: 1, name: "How-To Guides" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "Run models" })).toHaveClass("help-target");
  });

  it("lists a long section's groups under its title, and each link goes to its group", async () => {
    await show({ section: "how-to" });
    const groups = tocSection("how-to")!.groups.filter((g) => g.title);
    const jump = screen.getByRole("navigation", { name: "Groups in How-To Guides" });
    expect(within(jump).getAllByRole("link").map((a) => a.textContent)).toEqual(groups.map((g) => g.title));

    fireEvent.click(within(jump).getByRole("link", { name: groups[3].title! }));
    expect(document.querySelector(`[data-anchor="${slug(groups[3].title!)}"]`)).toHaveClass("help-target");
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("heading", { level: 1, name: "How-To Guides" })).toBeInTheDocument();
  });

  it("has no list of groups on a section without groups", async () => {
    await show({ section: "glossary" });
    expect(screen.queryByRole("navigation", { name: /^Groups in/ })).toBeNull();
  });

  it("gives every group of every section a place to land", async () => {
    for (const s of manifest.toc) {
      const named = s.groups.filter((g) => g.title);
      if (!named.length) continue;
      const r = await show({ section: s.id });
      for (const g of named) expect(screen.getByRole("heading", { level: 2, name: g.title! })).toHaveAttribute("data-anchor");
      r.unmount();
      localStorage.clear();
    }
  });

  it("reads Getting Started in the order Getting Started itself walks through", () => {
    const list = manifest.toc.find((s) => s.id === "getting-started")!.groups[0].articles;
    expect(list.indexOf("tour-the-demo")).toBeLessThan(list.indexOf("your-first-model"));
    expect(list.indexOf("your-first-model")).toBeLessThan(list.indexOf("connect-your-cluster"));
    expect(list.indexOf("connect-your-cluster")).toBeLessThan(list.indexOf("run-your-first-real-model"));
  });

  it("never lists the test build's How-To among the main ones on its home page", async () => {
    await show();
    const main = screen.getByRole("heading", { level: 2, name: "Main How-Tos" }).nextElementSibling!;
    expect(main.querySelector('a[href="help:open-a-fence"]')).toBeNull();
    expect(main.querySelector('a[href="help:unlock-admin-mode"]')).not.toBeNull();
  });
});

describe("Back and Forward", () => {
  const follow = async (selector: string) => {
    await act(async () => {
      fireEvent.click(document.querySelector(selector)!);
    });
  };

  it("walk the pages you've seen, by button and by key", async () => {
    await show({ article: "check-disk-space" });
    const back = screen.getByRole("button", { name: "Back" });
    const forward = screen.getByRole("button", { name: "Forward" });
    expect(back).toBeDisabled();
    await follow('.help-related a[href="help:node-window"]');
    expect(shown()).toBe("node-window");
    expect(back).toBeEnabled();
    await act(async () => {
      fireEvent.click(back);
    });
    expect(shown()).toBe("check-disk-space");
    expect(forward).toBeEnabled();
    await act(async () => {
      fireEvent.keyDown(document.querySelector(".help-win")!, { key: "]", metaKey: true });
    });
    expect(shown()).toBe("node-window");
    await act(async () => {
      fireEvent.keyDown(document.querySelector(".help-win")!, { key: "[", metaKey: true });
    });
    expect(shown()).toBe("check-disk-space");
  });

  it("skip a page you asked for again", async () => {
    await show({ article: "check-disk-space" });
    const main = screen.getByLabelText("Help page");
    fireEvent.click(rowNamed(articleMeta("check-disk-space")!.title));
    fireEvent.click(rowNamed(articleMeta("check-disk-space")!.title));
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();

    main.scrollTop = 300;
    fireEvent.click(rowNamed(articleMeta("check-disk-space")!.title));
    expect(main.scrollTop).toBe(0);
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
  });

  it("put the scroll position back", async () => {
    await show({ article: "check-disk-space" });
    const main = document.querySelector<HTMLElement>(".help-main")!;
    main.scrollTop = 240;
    await follow('.help-related a[href="help:node-window"]');
    expect(main.scrollTop).toBe(0);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Back" }));
    });
    expect(main.scrollTop).toBe(240);
  });
});

describe("links in an article", () => {
  it("open: opens that SparkBox window, https: the browser, and neither moves Help", async () => {
    await show({ article: "check-disk-space" });
    await act(async () => {
      fireEvent.click(document.querySelector('.help-prose a[href="open:dashboard"]')!);
    });
    expect(opened).toHaveBeenCalledWith("dashboard", {});
    const prose = document.querySelector(".help-prose")!;
    prose.insertAdjacentHTML("beforeend", '<a id="out" href="https://huggingface.co">out</a>');
    await act(async () => {
      fireEvent.click(document.getElementById("out")!);
    });
    expect(openUrl).toHaveBeenCalledWith("https://huggingface.co");
    expect(shown()).toBe("check-disk-space");
  });

  it("help: moves within Help", async () => {
    await show({ article: "check-disk-space" });
    await act(async () => {
      fireEvent.click(document.querySelector('.help-prose a[href^="help:"]:not(.help-term)')!);
    });
    expect(shown()).not.toBe("check-disk-space");
  });

  it("a # link can be undone: Back returns to the place left, and Forward goes on again", async () => {
    await show({ article: "troubleshooting" });
    const main = document.querySelector<HTMLElement>(".help-main")!;
    const heading = document.querySelector<HTMLElement>(".help-prose [data-anchor]")!;
    main.scrollTop = 300;
    const link = document.createElement("a");
    link.href = `#${heading.dataset.anchor}`;
    link.textContent = "to the heading";
    document.querySelector(".help-prose")!.prepend(link);
    await act(async () => {
      fireEvent.click(link);
    });
    expect(shown()).toBe("troubleshooting");
    expect(screen.getByRole("button", { name: "Back" })).toBeEnabled();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Back" }));
    });
    expect(shown()).toBe("troubleshooting");
    expect(main.scrollTop).toBe(300);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Forward" }));
    });
    expect(screen.getByRole("button", { name: "Forward" })).toBeDisabled();
  });

  it("the same # link twice adds no step to Back", async () => {
    await show({ article: "troubleshooting", anchor: "buttons-locks-and-touch-id" });
    const link = document.createElement("a");
    link.href = "#buttons-locks-and-touch-id";
    link.textContent = "again";
    document.querySelector(".help-prose")!.prepend(link);
    await act(async () => {
      fireEvent.click(link);
    });
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
  });
});

describe("a defined term", () => {
  const first = () => document.querySelector<HTMLElement>("a.help-term")!;

  it("opens its definition beside the word, and the page stays", async () => {
    await show({ article: "load-a-model-on-one-node" });
    await act(async () => {
      fireEvent.click(first());
    });
    expect(shown()).toBe("load-a-model-on-one-node");
    expect(document.querySelector(".help-term-pop")).not.toBeNull();
    expect(document.querySelector(".help-term-text")!.textContent!.length).toBeGreaterThan(10);
  });

  it("closes with Esc and gives focus back to the word", async () => {
    await show({ article: "load-a-model-on-one-node" });
    const word = first();
    await act(async () => {
      fireEvent.click(word);
    });
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(document.querySelector(".help-term-pop")).toBeNull();
    expect(word).toHaveFocus();
  });

  it("closes on a click away, and on a second click on the word", async () => {
    await show({ article: "load-a-model-on-one-node" });
    await act(async () => {
      fireEvent.click(first());
    });
    await act(async () => {
      fireEvent.pointerDown(document.body);
    });
    expect(document.querySelector(".help-term-pop")).toBeNull();
    await act(async () => {
      fireEvent.click(first());
    });
    await act(async () => {
      fireEvent.click(first());
    });
    expect(document.querySelector(".help-term-pop")).toBeNull();
  });

  it("a word inside a definition swaps the definition, and the page stays", async () => {
    await show({ article: "load-a-model-on-one-node" });
    const word = first();
    await act(async () => {
      fireEvent.click(word);
    });
    const inside = document.createElement("a");
    inside.className = "help-term";
    inside.href = "help:glossary#docker";
    document.querySelector(".help-term-text")!.append(inside);
    await act(async () => {
      fireEvent.click(inside);
    });
    expect(shown()).toBe("load-a-model-on-one-node");
    expect(document.querySelector(".help-term-pop .helptip-title")).toHaveTextContent("Docker");
  });

  it("Open in Glossary goes to the entry", async () => {
    await show({ article: "load-a-model-on-one-node" });
    await act(async () => {
      fireEvent.click(first());
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: "Open in Glossary" }));
    });
    expect(shown()).toBe("glossary");
    expect(document.querySelector(".help-term-pop")).toBeNull();
  });
});

describe("a picture", () => {
  const enlarge = async () => {
    await show({ article: "check-disk-space" });
    const img = document.querySelector<HTMLImageElement>("img.help-shot")!;
    img.src = "data:image/gif;base64,R0lGODlhAQABAAAAACw=";
    await act(async () => {
      fireEvent.click(img);
    });
    return img;
  };

  it("opens at full size on a click, with its caption, and Esc closes it", async () => {
    const img = await enlarge();
    const view = screen.getByRole("dialog", { name: img.alt });
    expect(view.querySelector("img")).toHaveAttribute("src", img.src);
    expect(view).toHaveTextContent(img.alt);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("dialog", { name: img.alt })).toBeNull();
    expect(img).toHaveFocus();
  });

  it("keeps the keyboard inside until it closes", async () => {
    const img = await enlarge();
    const close = screen.getByRole("button", { name: "Close" });
    expect(close).toHaveFocus();
    expect(document.getElementById("root") ?? document.body).toBeTruthy();
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);
    await act(async () => {
      fireEvent.click(document.querySelector(".help-zoom")!);
    });
    // Open again now there's a root, as there is in the app.
    await act(async () => {
      fireEvent.click(img);
    });
    expect(root).toHaveAttribute("inert");
    const button = screen.getByRole("button", { name: "Close" });
    expect(fireEvent.keyDown(button, { key: "Tab" })).toBe(false);
    expect(button).toHaveFocus();
    expect(fireEvent.keyDown(button, { key: "Tab", shiftKey: true })).toBe(false);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(root).not.toHaveAttribute("inert");
    expect(img).toHaveFocus();
    root.remove();
  });

  it("opens from the keyboard", async () => {
    await show({ article: "check-disk-space" });
    const img = document.querySelector<HTMLImageElement>("img.help-shot")!;
    expect(img).toHaveAttribute("tabindex", "0");
    await act(async () => {
      fireEvent.keyDown(img, { key: "Enter" });
    });
    expect(document.querySelector(".help-zoom")).not.toBeNull();
  });
});

describe("the Contents panel in a narrow window", () => {
  // CSS shows it only when the window is narrow, and tests have no layout.
  const contents = () => screen.getByRole("button", { name: "Contents", hidden: true });
  const open = async () => {
    await show({ article: "check-disk-space" });
    // What the narrow-window rule does: tests have no container queries.
    document.querySelector<HTMLElement>(".help-toc-toggle")!.style.display = "inline-flex";
    await act(async () => {
      fireEvent.click(contents());
    });
  };

  it("takes focus when it opens", async () => {
    await open();
    expect(document.activeElement?.getAttribute("role")).toBe("treeitem");
  });

  it("Esc closes it and returns to Contents", async () => {
    await open();
    await act(async () => {
      fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    });
    expect(document.querySelector(".help-win.toc-open")).toBeNull();
    expect(contents()).toHaveFocus();
  });

  it("a press outside it closes it", async () => {
    await open();
    await act(async () => {
      fireEvent.pointerDown(document.querySelector(".help-main")!);
    });
    expect(document.querySelector(".help-win.toc-open")).toBeNull();
  });

  it("choosing an article closes it and moves to the page", async () => {
    await open();
    await act(async () => {
      fireEvent.click(rowNamed(articleMeta("getting-started")!.title));
    });
    expect(shown()).toBe("getting-started");
    expect(document.querySelector(".help-win.toc-open")).toBeNull();
    expect(document.querySelector(".help-main")).toHaveFocus();
  });
});

describe("On this page", () => {
  const outline = () => [...document.querySelectorAll(".help-otp li")];

  it("lists sections and their parts on a normal article", async () => {
    await show({ article: "models-load-page" });
    expect(outline().length).toBeGreaterThan(3);
    expect(outline().length).toBeLessThanOrEqual(40);
  });

  it("stays a map on a page with hundreds of headings", async () => {
    await show({ article: "glossary" });
    const list = outline();
    expect(list.length).toBeGreaterThan(5);
    expect(list.length).toBeLessThan(40);
    expect(list.every((li) => li.classList.contains("level-2"))).toBe(true);
  });
});

describe("search", () => {
  it("shows ranked results as you type, with the words marked", async () => {
    await show();
    await type("check disk");
    const links = document.querySelectorAll<HTMLElement>(".help-result");
    expect(links.length).toBeGreaterThan(0);
    expect(links[0]).toHaveAttribute("href", expect.stringContaining("help:check-disk-space"));
    expect(links[0].querySelector("mark")).not.toBeNull();
  });

  it("forgives a typo", async () => {
    await show();
    await type("chekc disk");
    expect(document.querySelector(".help-result")).toHaveAttribute("href", expect.stringContaining("check-disk-space"));
  });

  it("arrows walk the results, Return opens one, and the words stay marked in the article", async () => {
    await show();
    await type("check disk");
    fireEvent.keyDown(search(), { key: "ArrowDown" });
    expect(document.activeElement).toBe(document.querySelector('[data-result="0"]'));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    expect(search()).toHaveFocus();
    await act(async () => {
      fireEvent.keyDown(search(), { key: "Enter" });
    });
    expect(shown()).toBe("check-disk-space");
    expect(document.querySelector("mark.help-hit")).not.toBeNull();
  });

  it("Back returns to the results, and clearing the box returns to where you were", async () => {
    await show({ article: "check-disk-space" });
    await type("disk");
    await act(async () => {
      fireEvent.click(document.querySelector(".help-result")!);
    });
    expect(document.querySelector(".help-result")).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Back" }));
    });
    expect(document.querySelector(".help-result")).not.toBeNull();
    expect(search()).toHaveValue("disk");
    await type("");
    expect(shown()).toBe("check-disk-space");
  });

  it("opens at the top when the query names the article, past nothing", async () => {
    await show();
    await type("check disk space");
    expect(document.querySelector(".help-result")).toHaveAttribute("href", "help:check-disk-space");
  });

  it("tells the true count, and can show all of it", async () => {
    await show();
    await type("node");
    expect(document.querySelector(".help-results-count")).toHaveTextContent(/^Showing the top 40 of \d{3} articles$/);
    expect(document.querySelectorAll(".help-result")).toHaveLength(40);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Show all \d{3} articles$/ }));
    });
    expect(document.querySelectorAll(".help-result").length).toBeGreaterThan(40);
    expect(document.querySelector(".help-results-count")).toHaveTextContent(/^\d{3} articles$/);
  });

  it("says so when nothing matches", async () => {
    await show();
    await type("zzzzzzzzzz");
    expect(screen.getByText(/Nothing in Help matches/)).toBeInTheDocument();
  });

  it("shows the nearest spelling's results when a word finds nothing, and says so", async () => {
    await show();
    await type("vlm");
    expect(document.querySelector(".help-result")).not.toBeNull();
    expect(document.querySelector(".help-results-count")).toHaveTextContent("Nothing matches “vlm”");
    expect(screen.getByRole("heading", { level: 1, name: "Results for “vllm”" })).toBeInTheDocument();
  });

  it("opens on a search a link asked for", async () => {
    await show({ q: "disk" });
    expect(document.querySelector(".help-result")).not.toBeNull();
    expect(search()).toHaveValue("disk");
  });
});

describe("the table of contents from the keyboard", () => {
  it("arrows move, right opens a group, left closes it, Return reads", async () => {
    await show();
    const howto = rowNamed("How-To Guides");
    howto.focus();
    expect(howto).toHaveAttribute("aria-expanded", "false");
    await act(async () => {
      fireEvent.keyDown(howto, { key: "ArrowRight" });
    });
    expect(rowNamed("How-To Guides")).toHaveAttribute("aria-expanded", "true");
    const first = rowNamed("Connect and log in");
    await act(async () => {
      fireEvent.keyDown(rowNamed("How-To Guides"), { key: "ArrowDown" });
      await new Promise((res) => requestAnimationFrame(res));
    });
    expect(first).toHaveFocus();
    await act(async () => {
      fireEvent.keyDown(rowNamed("How-To Guides"), { key: "ArrowLeft" });
    });
    expect(rowNamed("How-To Guides")).toHaveAttribute("aria-expanded", "false");
    await act(async () => {
      fireEvent.keyDown(rowNamed(articleMeta("what-is-sparkbox")!.title), { key: "Enter" });
    });
    expect(shown()).toBe("what-is-sparkbox");
  });

  it("has one tab stop", async () => {
    await show();
    expect(within(tree()).getAllByRole("treeitem").filter((r) => r.tabIndex === 0)).toHaveLength(1);
  });

  it("remembers what you opened", async () => {
    const r = await show();
    await act(async () => {
      fireEvent.click(document.querySelector('[data-key="s:concepts"] .help-twist')!);
    });
    r.unmount();
    await show();
    expect(rowNamed("Concepts")).toHaveAttribute("aria-expanded", "true");
  });
});
