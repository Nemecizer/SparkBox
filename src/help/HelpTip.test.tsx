// Info icons: what they show, and that nothing around them gets their click.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Table, type Column } from "../components/Table";
import { FormRow, HelpTip, InfoTip, SectionTitle } from "../components/ui";
import { paletteSections, withHelpSearch } from "../components/CommandPalette";
import { ShortcutsHelp } from "../components/ShortcutsHelp";
import { getCommands, registerCommands, type Command } from "../lib/commands";
import { closePanel, openPanel } from "../lib/session";
import { WindowManagerProvider } from "../wm/WindowManager";
import { articleMeta, headingName, resolveTopic, splitTopic, tips } from "./meta";
import { ensureHelpCommands, HELP_GROUP, loadHelpMeta, parseOpenLink } from "./nav";

const opened = vi.fn();

function shell(ui: React.ReactNode) {
  return render(<WindowManagerProvider sizeOf={() => [800, 600]} isKnown={() => true} openElsewhere={opened}>{ui}</WindowManagerProvider>);
}

beforeEach(async () => {
  opened.mockClear();
  await loadHelpMeta();
});

const tipButton = (name: RegExp | string) => screen.getByRole("button", { name });

describe("topics", () => {
  it("resolve a tip, an article and an article's heading", () => {
    const t = resolveTopic("models.load.gpu-share")!;
    expect(t.title).toBe(tips["models.load.gpu-share"].title);
    expect(`${t.article}#${t.anchor}`).toBe(tips["models.load.gpu-share"].article);
    expect(t.howtos.map((h) => h.id)).toEqual(["load-a-model-on-one-node"]);
    expect(resolveTopic("check-disk-space")!.body).toBe(articleMeta("check-disk-space")!.summary);
    expect(resolveTopic("node-window#gpu")!.anchor).toBe("gpu");
    expect(resolveTopic("no-such-thing")).toBeNull();
    expect(splitTopic("a#b")).toEqual({ id: "a", anchor: "b" });
  });

  it("list How-Tos only, three at most, and never the page itself", () => {
    for (const id of ["node-window", "models-window", "fabric"]) {
      const t = resolveTopic(id)!;
      expect(t.howtos.length).toBeLessThanOrEqual(3);
      for (const h of t.howtos) {
        expect(h.id).not.toBe(id);
        expect(["howto", "tutorial"]).toContain(articleMeta(h.id)!.type);
      }
    }
  });
});

describe("a Help tip", () => {
  it("opens on a click, never on hover, with its title, text, How-Tos and the full guide", async () => {
    shell(<HelpTip topic="models.load.gpu-share" />);
    const b = tipButton("About GPU memory share");
    fireEvent.mouseEnter(b);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(b);
    const pop = await screen.findByRole("dialog", { name: "About GPU memory share" });
    expect(pop).toHaveTextContent("GPU memory share");
    expect(pop).toHaveTextContent(tips["models.load.gpu-share"].body);
    expect(within(pop).getByRole("button", { name: articleMeta("load-a-model-on-one-node")!.title })).toBeInTheDocument();
    fireEvent.click(within(pop).getByRole("button", { name: "Read the full guide" }));
    const [article, anchor] = tips["models.load.gpu-share"].article.split("#");
    expect(opened).toHaveBeenCalledWith("help", { article, anchor });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a How-To link opens that How-To", async () => {
    shell(<HelpTip topic="models.load.gpu-share" />);
    fireEvent.click(tipButton("About GPU memory share"));
    const pop = await screen.findByRole("dialog");
    fireEvent.click(within(pop).getByRole("button", { name: articleMeta("load-a-model-on-one-node")!.title }));
    expect(opened).toHaveBeenCalledWith("help", { article: "load-a-model-on-one-node" });
  });

  it("an article's tip shows its summary", async () => {
    shell(<HelpTip topic="check-disk-space" label="About disks" />);
    fireEvent.click(tipButton("About disks"));
    expect(await screen.findByRole("dialog")).toHaveTextContent(articleMeta("check-disk-space")!.summary);
  });

  it("works from the keyboard: Tab reaches it, Return opens it, focus moves in, Esc comes back", async () => {
    const user = userEvent.setup();
    shell(<><button>before</button><HelpTip topic="models.load.gpu-share" /></>);
    await user.tab();
    await user.tab();
    const b = tipButton("About GPU memory share");
    expect(b).toHaveFocus();
    await user.keyboard("{Enter}");
    const pop = await screen.findByRole("dialog");
    await waitFor(() => expect(pop).toHaveFocus());
    await user.tab();
    expect(within(pop).getAllByRole("button")[0]).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(b).toHaveFocus();
  });

  it("closes when you click away", async () => {
    shell(<><button>elsewhere</button><HelpTip topic="models.load.gpu-share" /></>);
    fireEvent.click(tipButton("About GPU memory share"));
    await screen.findByRole("dialog");
    fireEvent.pointerDown(screen.getByRole("button", { name: "elsewhere" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("still works in a read-only form", async () => {
    shell(<fieldset disabled><button>Save</button><HelpTip topic="check-disk-space" label="About disks" /></fieldset>);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(tipButton("About disks"));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("says so if a topic names nothing, rather than opening empty", async () => {
    shell(<HelpTip topic="no-such-thing" label="About this" />);
    fireEvent.click(tipButton("About this"));
    expect(await screen.findByRole("dialog")).toHaveTextContent("This guide isn't available.");
  });
});

describe("an InfoTip with live text and a topic", () => {
  it("keeps its own words and adds the guide", async () => {
    shell(<InfoTip label="About disks" topic="check-disk-space"><p>Warns at 85%.</p></InfoTip>);
    fireEvent.click(tipButton("About disks"));
    const pop = await screen.findByRole("dialog");
    expect(pop).toHaveTextContent("Warns at 85%.");
    expect(pop).not.toHaveTextContent(articleMeta("check-disk-space")!.summary);
    expect(within(pop).getByRole("button", { name: "Read the full guide" })).toBeInTheDocument();
  });

  it("SectionTitle takes a topic, with or without text", async () => {
    shell(<SectionTitle title="Disks" infoTopic="check-disk-space" />);
    fireEvent.click(tipButton(`About ${articleMeta("check-disk-space")!.title}`));
    expect(await screen.findByRole("dialog")).toHaveTextContent(articleMeta("check-disk-space")!.summary);
  });

  it("is named for what it explains, not for where it sits", () => {
    shell(<SectionTitle title="cell1" infoTopic="models.load.gpu-share" />);
    expect(tipButton("About GPU memory share")).toBeInTheDocument();
    shell(<SectionTitle title="Alerts" infoTopic="alerts-window#history" />);
    expect(tipButton("About History")).toBeInTheDocument();
  });

  it("a hand-written name stays", () => {
    shell(<HelpTip topic="check-disk-space" label="About disks" />);
    expect(tipButton("About disks")).toBeInTheDocument();
  });

  it("a heading's name comes from its anchor, capitals included", () => {
    expect(headingName("read-the-fit-table")).toBe("Read the fit table");
    expect(headingName("gpu")).toBe("GPU");
    expect(headingName("cpu")).toBe("CPU");
  });
});

describe("in the Keyboard shortcuts sheet", () => {
  const sheet = async () => {
    registerCommands([{ id: "help:open", title: "SparkBox Help", group: "App", run: () => {} }, { id: "app:shortcuts", title: "Keyboard Shortcuts", group: "App", run: () => {} }]);
    shell(<ShortcutsHelp />);
    act(() => openPanel("shortcuts"));
    return screen.findByRole("dialog", { name: "Keyboard shortcuts" });
  };

  it("the tip sits above the sheet, and Read the full guide closes the sheet first", async () => {
    await sheet();
    const name = `About ${articleMeta("keyboard-shortcuts")!.title}`;
    fireEvent.click(tipButton(name));
    const pop = await screen.findByRole("dialog", { name });
    expect(pop).toHaveClass("over-panel");
    fireEvent.click(within(pop).getByRole("button", { name: "Read the full guide" }));
    expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).toBeNull();
    expect(opened).toHaveBeenCalledWith("help", { article: "keyboard-shortcuts" });
  });

  it("has one group of each name, with every Help key in it", async () => {
    const dialog = await sheet();
    const names = within(dialog).getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(new Set(names).size).toBe(names.length);
    const help = within(dialog).getByRole("heading", { name: "Help" }).closest("section")!;
    expect(help).toHaveTextContent("SparkBox Help");
    expect(help).toHaveTextContent("Search Help");
    expect(help).toHaveTextContent("Back, Forward");
    act(() => closePanel());
  });

  it("outside the sheet a tip stays an ordinary popover", async () => {
    shell(<HelpTip topic="models.load.gpu-share" />);
    fireEvent.click(tipButton("About GPU memory share"));
    expect(await screen.findByRole("dialog", { name: "About GPU memory share" })).not.toHaveClass("over-panel");
  });
});

describe("what's around an icon doesn't get its click", () => {
  interface Row { name: string }
  const columns: Column<Row>[] = [{ key: "name", label: "Name", help: "check-disk-space", sort: (r) => r.name, render: (r) => r.name }];

  it("a table header doesn't sort", async () => {
    shell(<Table tableId="help-tip-test" ariaLabel="Rows" columns={columns} rows={[{ name: "b" }, { name: "a" }]} rowKey={(r) => r.name} />);
    const before = screen.getAllByRole("row").map((r) => r.textContent);
    fireEvent.click(tipButton(`About ${articleMeta("check-disk-space")!.title}`));
    await screen.findByRole("dialog");
    expect(screen.getAllByRole("columnheader")[0]).toHaveAttribute("aria-sort", "none");
    expect(screen.getAllByRole("row").map((r) => r.textContent)).toEqual(before);
  });

  it("a form label doesn't focus its field", async () => {
    shell(<FormRow label={<>Max context <HelpTip topic="check-disk-space" label="About context" /></>} htmlFor="f"><input id="f" /></FormRow>);
    await userEvent.setup().click(tipButton("About context"));
    expect(screen.getByRole("textbox")).not.toHaveFocus();
  });
});

describe("open links", () => {
  it("name a window and its view", () => {
    expect(parseOpenLink("open:models")).toEqual({ kind: "models", params: {} });
    expect(parseOpenLink("open:models?tab=load")).toEqual({ kind: "models", params: { tab: "load" } });
    expect(parseOpenLink("open:settings?section=fence")).toEqual({ kind: "settings", params: { section: "fence" } });
    expect(parseOpenLink("help:x")).toBeNull();
    expect(parseOpenLink("open:../x")).toBeNull();
  });
});

describe("the command palette", () => {
  const cmd = (id: string, title: string, group: string): Command => ({ id, title, group, run: () => {} });

  it("lists the articles about Help for the word help, not all of them", async () => {
    ensureHelpCommands();
    await loadHelpMeta();
    await waitFor(() => expect(getCommands().some((c) => c.id.startsWith("help-article:"))).toBe(true));
    const rows = paletteSections(getCommands(), "help", []).flatMap((s) => s.rows);
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(20);
  });
  const cmds = [cmd("a", "Open Terminal", "Windows"), cmd("help-article:b", "Download a model", HELP_GROUP)];

  it("keeps articles out of an empty palette and finds them in a search", () => {
    expect(paletteSections(cmds, "", []).map((s) => s.name)).toEqual(["Windows"]);
    expect(paletteSections(cmds, "downl", []).map((s) => s.name)).toEqual([HELP_GROUP]);
  });

  it("still offers one you opened lately", () => {
    expect(paletteSections(cmds, "", ["help-article:b"]).map((s) => s.name)).toEqual(["Recent", "Windows"]);
    const rows = [{ name: "Windows", rows: [] }];
    expect(withHelpSearch(rows, "topology").map((s) => s.name)).toEqual(["Windows", "Search Help"]);
    for (const q of ["what is a gpu", "why is it locked", "how do i load a model", "Where is the fence?"]) {
      expect(withHelpSearch(rows, q).map((s) => s.name), q).toEqual(["Search Help", "Windows"]);
    }
  });
});

describe("loading", () => {
  it("the index comes once, however many ask", async () => {
    await act(async () => {
      const [a, b] = await Promise.all([loadHelpMeta(), loadHelpMeta()]);
      expect(a).toBe(b);
    });
  });
});
