// A pop-out has its own page, so nothing the desk registers reaches it.
import { act, fireEvent, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { POPOUT_OPEN } from "../lib/popout";
import { PopoutRoot } from "./PopoutRoot";

const emit = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("@tauri-apps/api/event", () => ({ emit, listen: vi.fn(async () => () => {}) }));

const pressHelp = () => fireEvent.keyDown(window, { code: "Slash", key: "?", metaKey: true, shiftKey: true });

beforeEach(() => emit.mockClear());

describe("a pop-out", () => {
  it("opens Help on the desk with ⌘?", async () => {
    let view: ReturnType<typeof render> | undefined;
    await act(async () => {
      view = render(<PopoutRoot route={{ kind: "no-such-window", params: {} }} />);
    });
    pressHelp();
    expect(emit).toHaveBeenCalledWith(POPOUT_OPEN, { kind: "help", params: {} });
    view!.unmount();
    emit.mockClear();
    pressHelp();
    expect(emit).not.toHaveBeenCalled();
  });
});
