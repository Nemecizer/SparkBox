import { isValidElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { describeError, showError } from "./errors";
import { dismissToast, getToasts } from "./toasts";
import type { VpnState, VpnStatus } from "./types";
import { setVpnStatus } from "./vpn";

const searchHelp = vi.hoisted(() => vi.fn());
vi.mock("../help/nav", () => ({ searchHelp }));

const alerts = vi.hoisted(() => [] as { title: string; body?: unknown }[]);
vi.mock("./session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./session")>()),
  alertInfo: vi.fn(async (o: { title: string; body?: unknown }) => void alerts.push(o)),
}));

const vpn = (state: VpnState) => setVpnStatus({ state } as VpnStatus);

beforeEach(() => {
  searchHelp.mockClear();
  alerts.length = 0;
  getToasts().forEach((t) => dismissToast(t.id));
});

describe("the way from an error to Help", () => {
  it("comes after the error's own fix, and searches for the words on screen", () => {
    const d = describeError({ kind: "sudoRejected", message: "x", node: "cell2" });
    expect(d.actions.map((a) => a.label)).toEqual(["Fix password", "What does this mean?"]);
    void d.actions[1].run();
    expect(searchHelp).toHaveBeenCalledWith("sudo on cell2 didn't accept the saved cluster password.");
  });

  it("is offered on every error but a cancelled one, which shows nothing", () => {
    for (const kind of ["lookOnly", "locked", "needs_auth", "authRejected", "sudoRejected", "noPassword", "timeout", "unreachable", "demoOnly", "fenced", "otherController", "wrongNode", "gone", "invalid", "failed"] as const) {
      expect(describeError({ kind, message: "x" }).actions.at(-1)?.label, kind).toBe("What does this mean?");
    }
    expect(describeError({ kind: "cancelled", message: "x" }).actions).toEqual([]);
  });
});

describe("Reconnect VPN on an unreachable node", () => {
  const labels = () => describeError("cell3 isn't connected. Use Live data").actions.map((a) => a.label);

  it("isn't offered at home: the VPN isn't in the way", () => {
    for (const state of ["off", "direct"] as const) {
      vpn(state);
      expect(labels(), state).toEqual(["What does this mean?"]);
    }
  });

  it("is offered when the VPN is in use or failing", () => {
    for (const state of ["connecting", "connected", "error"] as const) {
      vpn(state);
      expect(labels(), state).toEqual(["Reconnect VPN", "What does this mean?"]);
    }
  });

  it("a timeout follows the same rule", () => {
    vpn("direct");
    expect(describeError("SSH timeout: no reply from 192.168.50.98:22 in 15 s").actions.map((a) => a.label)).toEqual(["What does this mean?"]);
    vpn("error");
    expect(describeError("SSH timeout: no reply from 192.168.50.98:22 in 15 s").actions[0].label).toBe("Reconnect VPN");
  });
});

describe("Details on an error toast", () => {
  const shown = () => getToasts()[0].actions?.map((a) => a.label);

  it("is there when the error carries more than its sentence, and opens it in a code block", async () => {
    showError({ kind: "failed", message: "docker stop failed on cell2", node: "cell2", detail: "Error response from daemon: exit 137\nsecond line" }, { action: "Stop vllm-head" });
    expect(shown()).toEqual(["What does this mean?", "Details"]);
    await getToasts()[0].actions![1].run();
    expect(alerts.map((a) => a.title)).toEqual(["Couldn't stop vllm-head"]);
    const body = alerts[0].body as ReactElement;
    expect(isValidElement(body)).toBe(true);
    expect(renderToStaticMarkup(body)).toBe('<div class="code-block"><pre><code>Error response from daemon: exit 137\nsecond line</code></pre></div>');
  });

  it("isn't there when there's nothing more to show", () => {
    showError({ kind: "failed", message: "It failed." });
    expect(shown()).toEqual(["What does this mean?"]);
  });

  it("a Hugging Face token in stderr stays hidden", async () => {
    showError({ kind: "failed", message: "It failed.", detail: "Authorization: Bearer hf_abcdefghijklmnopqrstuvwxyz012345" });
    await getToasts()[0].actions!.find((a) => a.label === "Details")!.run();
    expect(renderToStaticMarkup(alerts[0].body as ReactElement)).not.toContain("hf_abcdefghijklmnopqrstuvwxyz012345");
  });
});
