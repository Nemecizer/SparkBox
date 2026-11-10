import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { Copy, Play, Square, Trash2 } from "lucide-react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import stylesCss from "../styles.css?raw";
import aiCss from "../windows/ai.css?raw";
import {
  CodeBlock, InfoTip, Meter, MiddleEllipsis, Notice, SearchField, Segmented, Sparkline, StatusDot, Switch, Tabs, Tile, tileColumns, Toolbar,
  ToolButton, trendPath,
} from "./ui";

// The unlock prompt, under the test's control: what it resolves to and what it was asked, and
// whether SparkBox is in look-only mode. The lock rule's words and lockOf are the real ones.
const admin = vi.hoisted(() => ({ grant: true, on: false, lookOnly: false, calls: [] as unknown[][] }));
vi.mock("../lib/admin", async (importOriginal) => {
  const real = await importOriginal<typeof import("../lib/admin")>();
  const state = () => ({ unlocked: admin.on, lookOnly: admin.lookOnly, expiresAtMs: null, idleMinutes: 15, requiresAuth: false });
  return {
    LOOK_ONLY_TIP: real.LOOK_ONLY_TIP,
    NEEDS_ADMIN_TIP: real.NEEDS_ADMIN_TIP,
    lockOf: (needs: boolean | undefined) => real.lockOf(needs, state()),
    useAdminState: state,
    requestAdmin: (...args: unknown[]) => {
      admin.calls.push(args);
      return Promise.resolve(admin.grant);
    },
    // A locked control asks with the in-app sheet first (§3.3): confirmUnlock(action, scope).
    confirmUnlock: (...args: unknown[]) => {
      admin.calls.push(args);
      return Promise.resolve(admin.grant);
    },
    showLookOnly: () => {
      admin.calls.push(["the look-only sheet"]);
      return Promise.resolve();
    },
    useAdmin: () => admin.on,
  };
});

describe("StatusDot", () => {
  it.each([
    ["online", "Online"],
    ["connecting", "Connecting"],
    ["offline", "Offline"],
    ["error", "Error"],
    ["stale", "Stale"],
    ["unconfigured", "Not set up"],
    ["warn", "Warning"],
    ["crit", "Critical"],
  ] as const)("names %s for screen readers and draws its own shape", (status, word) => {
    const { container } = render(<StatusDot status={status} />);
    const img = screen.getByRole("img", { name: word });
    expect(img).toHaveClass(`sdot-${status}`);
    expect(container.querySelector(".status")).toHaveAttribute("title", word);
  });

  it("shows the word, or the replacement text, when asked", () => {
    render(<><StatusDot status="online" label /><StatusDot status="offline" text="No link" /></>);
    expect(screen.getByText("Online")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "No link" })).toBeInTheDocument();
  });

  it("uses different geometry for every state", () => {
    const shapes = new Set(
      (["online", "connecting", "offline", "error", "stale", "unconfigured", "warn", "crit"] as const).map((s) => {
        const { container, unmount } = render(<StatusDot status={s} />);
        const svg = container.querySelector("svg")!;
        const sig = [...svg.children].map((c) => `${c.tagName}:${c.getAttribute("d") ?? c.getAttribute("r")}:${c.getAttribute("fill")}:${c.getAttribute("stroke-dasharray")}`).join("|");
        unmount();
        return sig;
      }),
    );
    expect(shapes.size).toBe(8);
  });
});

function SegHarness({ onChange }: { onChange?: (v: string) => void }) {
  const [v, setV] = useState("demo");
  return (
    <Segmented
      label="Data source"
      value={v}
      onChange={(x) => {
        setV(x);
        onChange?.(x);
      }}
      options={[{ value: "demo", label: "Demo" }, { value: "live", label: "Live" }, { value: "off", label: "Off", disabled: true }, { value: "auto", label: "Auto" }]}
    />
  );
}

describe("Segmented", () => {
  it("is a radio group with one tab stop", () => {
    render(<SegHarness />);
    expect(screen.getByRole("radiogroup", { name: "Data source" })).toBeInTheDocument();
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false", "false", "false"]);
    expect(radios.map((r) => r.tabIndex)).toEqual([0, -1, -1, -1]);
  });

  it("moves and chooses with arrow keys, skipping disabled options and wrapping", async () => {
    const user = userEvent.setup();
    const seen: string[] = [];
    render(<SegHarness onChange={(v) => seen.push(v)} />);
    await user.click(screen.getByRole("radio", { name: "Demo" }));
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("radio", { name: "Live" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Live" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("radio", { name: "Auto" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("radio", { name: "Demo" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{End}");
    await user.keyboard("{Home}");
    expect(seen).toEqual(["demo", "live", "auto", "demo", "auto", "demo"]);
  });
});

function TabsHarness() {
  const [v, setV] = useState("library");
  return (
    <Tabs
      label="Models"
      value={v}
      onChange={setV}
      tabs={[{ id: "library", label: "Library" }, { id: "hf", label: "Hugging Face" }, { id: "jobs", label: "Jobs", badge: 2 }]}
    >
      <p>Panel {v}</p>
    </Tabs>
  );
}

describe("Tabs", () => {
  it("wires tabs to their panel", () => {
    render(<TabsHarness />);
    const tab = screen.getByRole("tab", { name: "Library" });
    const panel = screen.getByRole("tabpanel");
    expect(tab).toHaveAttribute("aria-selected", "true");
    expect(tab.getAttribute("aria-controls")).toBe(panel.id);
    expect(panel.getAttribute("aria-labelledby")).toBe(tab.id);
    expect(screen.getAllByRole("tab").map((t) => t.tabIndex)).toEqual([0, -1, -1]);
  });

  it("follows ←/→, Home and End with roving focus", async () => {
    const user = userEvent.setup();
    render(<TabsHarness />);
    await user.click(screen.getByRole("tab", { name: "Library" }));
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Hugging Face" })).toHaveFocus();
    expect(screen.getByRole("tabpanel")).toHaveTextContent("Panel hf");
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: /Jobs/ })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Library" })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: /Jobs/ })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tabpanel")).toHaveTextContent("Panel library");
  });
});

describe("controls", () => {
  it("Switch is a switch with its state", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Switch label="Keep running in the menu bar" checked={false} onChange={onChange} />);
    const sw = screen.getByRole("switch", { name: "Keep running in the menu bar" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    await user.click(sw);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("Meter reports its value and level", () => {
    render(<Meter pct={91} label="109 / 120 GiB" warn={85} crit={95} ariaLabel="Memory on cell1" />);
    const bar = screen.getByRole("progressbar", { name: "Memory on cell1" });
    expect(bar).toHaveAttribute("aria-valuenow", "91");
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    expect(bar).toHaveAttribute("aria-valuetext", "109 / 120 GiB, warning");
  });

  it("Tile becomes a button when clickable and carries its tone in words", async () => {
    const onClick = vi.fn();
    render(<Tile label="Fabric" value="38.4" unit="Gb/s" tone="crit" onClick={onClick} trend={[1, 2, 3]} />);
    const tile = screen.getByRole("button", { name: /^Fabric, Critical/ });
    fireEvent.click(tile);
    expect(onClick).toHaveBeenCalled();
    expect(tile.querySelector(".tile-trend")).not.toBeNull();
  });

  it("SearchField marks itself for ⌘F and clears on Escape", async () => {
    const user = userEvent.setup();
    function H() {
      const [v, setV] = useState("vllm");
      return <SearchField value={v} onChange={setV} placeholder="Filter" count={3} />;
    }
    render(<H />);
    const box = screen.getByRole("searchbox", { name: "Filter" });
    expect(box).toHaveAttribute("data-find-target");
    expect(screen.getByText("3")).toBeInTheDocument();
    await user.click(box);
    await user.keyboard("{Escape}");
    expect(box).toHaveValue("");
  });

  it("Notice offers dismiss and retry", async () => {
    const user = userEvent.setup();
    const onDismiss = vi.fn();
    const onRetry = vi.fn();
    render(<Notice tone="error" onDismiss={onDismiss} onRetry={onRetry}>Couldn't reach cell2.</Notice>);
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't reach cell2.");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onRetry).toHaveBeenCalled();
    expect(onDismiss).toHaveBeenCalled();
  });

  it("CodeBlock copies its text and toggles wrapping", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = render(<CodeBlock text="docker ps --all" label="Command" />);
    await user.click(screen.getByRole("button", { name: /Copy/ }));
    expect(writeText).toHaveBeenCalledWith("docker ps --all");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
    const pre = container.querySelector("pre")!;
    expect(pre).toHaveClass("wrap");
    await user.click(screen.getByRole("button", { name: /Wrap/ }));
    expect(pre).not.toHaveClass("wrap");
  });
});

describe("Toolbar", () => {
  it("collects lower-priority buttons (through fragments) into More", async () => {
    const user = userEvent.setup();
    const onDelete = vi.fn();
    render(
      <Toolbar label="Containers">
        <ToolButton icon={Play} label="Start" />
        <>
          <ToolButton icon={Square} label="Stop" priority={2} locked />
          <ToolButton icon={Trash2} label="Remove" priority={3} danger onClick={onDelete} />
        </>
        <ToolButton icon={Copy} label="Copy ID" priority={3} />
      </Toolbar>,
    );
    expect(screen.getByRole("toolbar", { name: "Containers" })).toBeInTheDocument();
    // Locked stays enabled (a click unlocks, then runs); the toolbar says Read-only once.
    expect(screen.getByRole("button", { name: "Stop — needs Admin" })).toBeEnabled();
    const more = screen.getByRole("button", { name: "More" });
    expect(more).toHaveAttribute("data-p3", "true");
    expect(more).toHaveAttribute("data-danger", "true");
    await user.click(more);
    const items = screen.getAllByRole("menuitem").map((m) => m.textContent);
    expect(items).toEqual(["Stop", "Remove", "Copy ID"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }));
    });
    expect(onDelete).toHaveBeenCalled();
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("tileColumns", () => {
  it("keeps tiles on one row when they fit, else balances the rows", () => {
    expect(tileColumns(6, 1200)).toBe(6);
    expect(tileColumns(6, 960)).toBe(3); // not 5 + 1
    expect(tileColumns(6, 700)).toBe(3);
    expect(tileColumns(5, 700)).toBe(3); // 3 + 2
    expect(tileColumns(6, 360)).toBe(2);
    expect(tileColumns(4, 2000)).toBe(4);
    expect(tileColumns(3, 100)).toBe(1);
  });
});

// ---- §2.2 trends

/** Every drawn segment of a path, as [x0, y0, x1, y1] (control points dropped). */
function segments(d: string): [number, number, number, number][] {
  const out: [number, number, number, number][] = [];
  let at: [number, number] | null = null;
  for (const m of d.matchAll(/([MLC])([^MLC]+)/g)) {
    const nums = m[2].split(/[ ,]+/).filter(Boolean).map(Number);
    const end: [number, number] = [nums[nums.length - 2], nums[nums.length - 1]];
    if (m[1] !== "M" && at) out.push([at[0], at[1], end[0], end[1]]);
    at = end;
  }
  return out;
}

describe("Sparkline", () => {
  const steps = [0, 0, 0, 0, 10, 10, 10, 10, 30, 30, 30, 30, 0, 0, 0, 0];

  it.each(["linear", "monotone"] as const)("draws a stepwise input with no vertical-only segment (%s)", (curve) => {
    const { container } = render(<Sparkline values={steps} max={30} width={60} height={18} curve={curve} />);
    const d = container.querySelector("path")!.getAttribute("d")!;
    const segs = segments(d);
    expect(segs.length).toBeGreaterThan(3);
    for (const [x0, , x1] of segs) expect(x1 - x0).toBeGreaterThan(0.5);
  });

  it("averages into at most width/2 buckets and fills only when asked: the theme's --spark-fill, or a number capped at 8%", () => {
    const many = Array.from({ length: 300 }, (_, i) => (i % 7) * 10);
    const { container, rerender } = render(<Sparkline values={many} width={52} height={18} />);
    const paths = container.querySelectorAll("path");
    expect(paths).toHaveLength(1);
    expect(segments(paths[0].getAttribute("d")!).length).toBeLessThanOrEqual(26 - 1);
    // The line's weight is the theme's (2px in Black), with 1.5 as the attribute fallback.
    expect(paths[0].getAttribute("stroke-width")).toBe("1.5");
    expect((paths[0] as SVGPathElement).style.strokeWidth).toBe("var(--spark-width, 1.5)");
    rerender(<Sparkline values={many} width={52} height={18} fill />);
    const area = container.querySelectorAll("path")[0] as SVGPathElement;
    expect(area.classList.contains("spark-fill")).toBe(true);
    expect(area.style.fillOpacity).toBe("var(--spark-fill, 0.08)");
    rerender(<Sparkline values={many} width={52} height={18} fill={0.3} />);
    expect(Number((container.querySelectorAll("path")[0] as SVGPathElement).style.fillOpacity)).toBe(0.08);
  });

  it("leaves gaps open", () => {
    const d = trendPath([{ x: 0, y: 1 }, { x: 1, y: 2 }, null, { x: 3, y: 1 }, { x: 4, y: 2 }]);
    expect(d.match(/M/g)).toHaveLength(2);
  });
});

describe("stacked fills", () => {
  it("are parted by a 2 px surface gap, so neighbours of one lightness (page cache beside other use, host runtime beside the in-use hatch) still read apart", () => {
    // The dataviz spacer: the boundary is the surface's contrast, never the two fills' (gray-9 beside the
    // cyan is 1.1:1 in Black). The memory split bar is a flex row with a gap; the memory plan's segments
    // are sized in percent, so each ends in a 2 px inset of the surface instead.
    const rule = (css: string, selector: string) => css.split("\n").find((line) => line.startsWith(`${selector} {`)) ?? "";
    expect(rule(stylesCss, ".stackbar")).toMatch(/gap: 2px/);
    expect(rule(aiCss, ".ai-plan-bar > .seg")).toMatch(/box-shadow: inset -2px 0 0 var\(--surface/);
  });
});

describe("Tile", () => {
  it("draws a line trend with no fill, on its domain, at the theme's --spark-width (1.5px fallback)", () => {
    const { container } = render(<Tile label="Memory" value="336" trend={[10, 30, 50, 70]} trendDomain={[0, 100]} />);
    const svg = container.querySelector("svg.tile-trend")!;
    expect(svg.querySelectorAll("path")).toHaveLength(1);
    const path = svg.querySelector("path") as SVGPathElement;
    expect(path.getAttribute("fill")).toBe("none");
    expect(path.getAttribute("stroke-width")).toBe("1.5");
    expect(path.style.strokeWidth).toBe("var(--spark-width, 1.5)");
    expect(svg.querySelector("polygon")).toBeNull();
  });

  it("meter variant carries its colour as --meter-color and its level as a class", () => {
    const { container, rerender } = render(<Tile label="Root disk" value="41" unit="%" meter={{ pct: 41, warn: 80, crit: 90, color: "var(--m-read)" }} />);
    const meter = container.querySelector(".tile-meter") as HTMLElement;
    expect(meter.classList.contains("ok")).toBe(true);
    expect(meter.style.getPropertyValue("--meter-color")).toBe("var(--m-read)");
    expect((meter.firstElementChild as HTMLElement).style.width).toBe("41%");
    rerender(<Tile label="Root disk" value="92" unit="%" meter={{ pct: 92, warn: 80, crit: 90 }} />);
    const crit = container.querySelector(".tile-meter") as HTMLElement;
    expect(crit.classList.contains("crit")).toBe(true);
    expect(crit.style.getPropertyValue("--meter-color")).toBe("");
  });

  it("hides the trend when it doesn't move or is a count", () => {
    const flat = render(<Tile label="Memory" value="336" trend={[50, 50.2, 50.1, 50]} trendDomain={[0, 100]} />);
    expect(flat.container.querySelector(".tile-trend")).toBeNull();
    const count = render(<Tile label="GPUs serving" value="4/4" trend={[1, 2, 3, 4]} trendKind="none" />);
    expect(count.container.querySelector(".tile-trend")).toBeNull();
  });

  it("is quiet when good: no accent, no tone class", () => {
    const { container } = render(<Tile label="Power" value="201" tone="good" />);
    expect(container.querySelector(".tile")!.className).toBe("tile");
  });
});

// ---- §D6 locked and busy actions

describe("locked actions", () => {
  beforeEach(() => {
    admin.grant = true;
    admin.calls = [];
  });

  // P1.43's one rule (it replaces §D6's "no padlock per button"): the action's own label and
  // icon, a trailing lock and a tooltip.
  it("stay enabled, keep their label and icon, and say they need Admin with a trailing lock", () => {
    render(<Toolbar label="cell1"><ToolButton icon={Play} label="Restart…" title="Restart cell1" locked onClick={() => {}} /></Toolbar>);
    const b = screen.getByRole("button", { name: "Restart… — needs Admin" });
    expect(b).toBeEnabled();
    expect(b).toHaveTextContent("Restart…");
    expect(b).toHaveAttribute("title", "Restart cell1 · needs Admin (click to unlock)");
    expect(b).toHaveAttribute("data-lock", "locked");
    expect(b.querySelectorAll("svg")).toHaveLength(2);
    expect(b.lastElementChild).toHaveClass("tool-lock");
    expect(screen.getAllByRole("button", { name: /Read-only/ })).toHaveLength(1);
  });

  it("in look-only mode are disabled, say how to turn it off, and nothing promises an unlock", async () => {
    admin.lookOnly = true;
    try {
      const run = vi.fn();
      render(
        <Toolbar label="cell1">
          <ToolButton icon={Play} label="Restart…" title="Restart cell1" locked onClick={run} />
          <ToolButton icon={Square} label="Stop" priority={2} locked onClick={run} />
          <ToolButton icon={Copy} label="Copy ID" onClick={() => {}} />
        </Toolbar>,
      );
      const b = screen.getByRole("button", { name: "Restart… — look-only" });
      expect(b).toBeDisabled();
      expect(b).toHaveAttribute("title", "Look-only · turn off in Settings › Security & privacy");
      expect(b.lastElementChild).toHaveClass("tool-lock");
      expect(screen.getByRole("button", { name: "Copy ID" })).toBeEnabled();
      const chip = screen.getByRole("button", { name: /^Look only/ });
      expect(chip).toHaveTextContent("Look only");
      expect(chip).toHaveAttribute("title", "Look-only · turn off in Settings › Security & privacy");
      fireEvent.click(screen.getByRole("button", { name: "More" }));
      expect(screen.getByRole("menuitem", { name: "Stop — look-only" })).toBeDisabled();
      const words = [...document.querySelectorAll("[title], [aria-label]")].flatMap((el) => [el.getAttribute("title") ?? "", el.getAttribute("aria-label") ?? ""]);
      expect(words.filter((w) => /click to unlock|unlock/i.test(w))).toEqual([]);
      expect(document.body.textContent).not.toMatch(/unlock/i);
      await act(async () => fireEvent.click(chip));
      expect(admin.calls, "the chip only says why: no unlock").toEqual([["the look-only sheet"]]);
      expect(run).not.toHaveBeenCalled();
    } finally {
      admin.lookOnly = false;
    }
  });

  it("unlock, then carry on with the action; a cancelled unlock does nothing", async () => {
    const user = userEvent.setup();
    const run = vi.fn();
    render(<ToolButton icon={Play} label="Restart…" locked scope="node" onClick={run} />);
    await user.click(screen.getByRole("button"));
    expect(admin.calls).toEqual([["restart", "node"]]);
    expect(run).toHaveBeenCalledTimes(1);
    admin.grant = false;
    await user.click(screen.getByRole("button"));
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("the Read-only chip unlocks with the locked tools' scope", async () => {
    const user = userEvent.setup();
    render(<Toolbar label="VPN"><ToolButton icon={Play} label="Connect" locked scope="mac" /></Toolbar>);
    await user.click(screen.getByRole("button", { name: /Read-only/ }));
    expect(admin.calls).toEqual([["make changes", "mac"]]);
  });

  it("says Touch ID, not Read-only, when Admin is on but the tools change this Mac", () => {
    admin.on = true;
    try {
      render(<Toolbar label="VPN"><ToolButton icon={Play} label="Connect" locked scope="mac" /></Toolbar>);
      expect(screen.getByRole("button", { name: /Changes on this Mac need Touch ID/ })).toHaveTextContent("Needs Touch ID");
      expect(screen.queryByText("Read-only")).toBeNull();
    } finally {
      admin.on = false;
    }
  });

  it("is busy while its Promise runs and ignores more clicks (no triple-click duplicates)", async () => {
    let finish = () => {};
    const run = vi.fn(() => new Promise<void>((r) => (finish = r)));
    render(<ToolButton icon={Play} label="Start" onClick={run} />);
    const b = screen.getByRole("button", { name: "Start" });
    await act(async () => {
      fireEvent.click(b);
      fireEvent.click(b);
      fireEvent.click(b);
    });
    expect(run).toHaveBeenCalledTimes(1);
    expect(b).toHaveAttribute("aria-busy", "true");
    await act(async () => finish());
    expect(b).not.toHaveAttribute("aria-busy");
    await act(async () => {
      fireEvent.click(b);
    });
    expect(run).toHaveBeenCalledTimes(2);
  });

  it("a destructive button is offered in More with its label, ready for when labels give way", () => {
    render(<Toolbar label="Node"><ToolButton icon={Play} label="Restart…" /><ToolButton icon={Trash2} label="Shut down…" danger /></Toolbar>);
    const more = screen.getByRole("button", { name: "More" });
    expect(more).toHaveAttribute("data-danger", "true");
    fireEvent.click(more);
    expect(screen.getByRole("menuitem", { name: "Shut down…" })).toHaveClass("danger");
  });
});

// ---- §D7 prose in popovers, clipping

describe("an info icon before the Help index has loaded", () => {
  it("reads About this", () => {
    // The tests' HelpTip topics resolve once ../help/meta has loaded; nothing here loads it.
    render(<InfoTip topic="mon.fabric.verdict"><p>Why.</p></InfoTip>);
    expect(screen.getByRole("button", { name: "About this" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More information" })).toBeNull();
  });
});

describe("InfoTip and clipping", () => {
  it("opens a popover from the keyboard and Esc returns focus", async () => {
    const user = userEvent.setup();
    render(<InfoTip label="About capacity"><p>Free memory per node.</p></InfoTip>);
    const b = screen.getByRole("button", { name: "About capacity" });
    b.focus();
    await user.keyboard("{Enter}");
    expect(b).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("dialog", { name: "About capacity" })).toHaveTextContent("Free memory per node.");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(b).toHaveFocus();
  });

  it("MiddleEllipsis keeps the end and puts the whole text in the tooltip", () => {
    const { container } = render(<MiddleEllipsis text="/home/sparkuser/models/meta-llama/config.json" />);
    expect(container.querySelector(".mid-ellipsis-tail")!.textContent).toBe("/config.json");
    expect(container.firstElementChild).toHaveAttribute("title", "/home/sparkuser/models/meta-llama/config.json");
  });
});
