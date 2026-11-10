// Cancel says what canceling each kind of job leaves behind, and a state from the wire reads in
// American English. A job's end refreshes the Library (ModelsWindow).
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultHandlers } from "../../dev/mockBackend";
import { MOCKS } from "../../dev/mocks";
import { _resetAdminForTests } from "../../lib/admin";
import type { AdminState } from "../../lib/opsApi";
import * as session from "../../lib/session";
import { demoDiskPreflight } from "../../dev/mocks/disk";
import type { Job, JobState } from "../../lib/types";
import { mockCalls, mockTauri } from "../../test/tauri";
import * as toasts from "../../lib/toasts";
import { WindowManagerProvider } from "../../wm/WindowManager";
import { runningChanged } from "../ModelsWindow";
import { JobsTab, jobWord, CANCEL_WORDS } from "./JobsTab";

const UNLOCKED: AdminState = { unlocked: true, expiresAtMs: null, idleMinutes: 15, requiresAuth: true, lookOnly: false };
const job = (kind: Job["kind"], state: JobState = "running", message = ""): Job => ({
  id: `job-${kind}`, node: "cell1", kind, state, step: "copying", message, label: `A ${kind}`, bytesDone: 0, bytesTotal: 0, rateBps: 0, startedAt: 1, updatedAt: 1,
});

async function show(jobs: Job[], job_cancel: () => void = () => undefined, extra: Record<string, () => unknown> = {}, state = UNLOCKED) {
  mockTauri(Object.assign({}, defaultHandlers(), ...MOCKS, { admin_state: () => state, job_cancel }, extra));
  _resetAdminForTests(state);
  await act(async () => {
    render(
      <WindowManagerProvider sizeOf={() => [1200, 900]} isKnown={() => true}>
        <JobsTab jobs={jobs} refresh={() => {}} />
      </WindowManagerProvider>,
    );
  });
}

afterEach(() => vi.restoreAllMocks());

describe("Resume download", () => {
  const stopped = (): Job => ({ ...job("download", "cancelled"), resume: { repo: "org/model", revision: "a".repeat(40), dest: "/home/spark/models/org/model", planHash: "c".repeat(64) } });
  const select = async () => act(async () => { fireEvent.mouseDown(screen.getByText("A download")); });
  const resumeButton = () => screen.getByRole("button", { name: /^Resume download…/ }) as HTMLButtonElement;

  it("confirms the original node, folder and revision and sends the exact reviewed plan", async () => {
    const old = stopped();
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([old], undefined, { job_resume_download: () => ({ ...old, id: "dl-new", state: "running" }) });
    await select();
    await act(async () => { fireEvent.click(resumeButton()); });
    expect(confirm.mock.calls[0][0]).toMatchObject({ title: "Choose a download tool on cell1", confirmLabel: "Continue" });
    expect(confirm.mock.calls[1][0]).toMatchObject({ title: "Resume org/model on cell1?", confirmLabel: "Resume download" });
    expect(render(<>{confirm.mock.calls[1][0].body}</>).container.textContent).toContain(old.resume!.revision);
    expect(render(<>{confirm.mock.calls[1][0].body}</>).container.textContent).toContain(old.resume!.dest);
    expect(render(<>{confirm.mock.calls[1][0].body}</>).container.textContent).toContain("keeps the old job record");
    expect(mockCalls.find((c) => c.cmd === "job_resume_download")?.args).toEqual({ node: "cell1", id: old.id, expected: old.resume, review: { node: "cell1", tool: { kind: "host", path: "/home/demo/.local/bin/hf", fingerprint: "a".repeat(64), label: "Existing hf on this node" }, installConsent: false } });
    expect(screen.getByText("A download")).toBeTruthy();
    expect(mockCalls.some((c) => c.cmd === "model_download")).toBe(false);
  });

  it("does nothing when confirmation is declined", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(false);
    await show([stopped()]); await select();
    await act(async () => { fireEvent.click(resumeButton()); });
    expect(mockCalls.some((c) => c.cmd === "job_resume_download")).toBe(false);
  });

  it("checks the original resume destination and keeps the old row when space is short", async () => {
    const old = stopped();
    const request = { kind: "resume" as const, node: old.node, id: old.id, expected: old.resume! };
    const checks = demoDiskPreflight(request); checks[0].ok = false;
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    vi.spyOn(session, "alertInfo").mockResolvedValue(undefined);
    await show([old], undefined, { model_disk_preflight: () => checks }); await select();
    await act(async () => { fireEvent.click(resumeButton()); });
    expect(mockCalls.find((c) => c.cmd === "model_disk_preflight")?.args).toEqual({ request });
    expect(mockCalls.some((c) => c.cmd === "job_resume_download")).toBe(false);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(screen.getByText("A download")).toBeTruthy();
  });

  it.each(["running", "done", "legacy"])("does not resume a %s job", async (state) => {
    const old = stopped();
    if (state === "legacy") old.resume = null;
    else old.state = state as JobState;
    await show([old]); await select();
    expect(resumeButton().disabled).toBe(true);
  });

  it.each([{ ...UNLOCKED, unlocked: false }, { ...UNLOCKED, lookOnly: true }])("keeps the Admin/Look only lock: %j", async (state) => {
    await show([stopped()], undefined, {}, state); await select();
    await act(async () => { fireEvent.click(resumeButton()); });
    expect(mockCalls.some((c) => c.cmd === "job_resume_download")).toBe(false);
  });

  it("continues to tool review after the normal Admin unlock without another click", async () => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValueOnce(true).mockResolvedValueOnce(true).mockResolvedValueOnce(false);
    await show([stopped()], undefined, { admin_unlock: () => UNLOCKED }, { ...UNLOCKED, unlocked: false });
    await select(); await act(async () => { fireEvent.click(resumeButton()); });
    expect(confirm.mock.calls.map(([options]) => options.confirmLabel)).toEqual(["Unlock…", "Continue", "Resume download"]);
    expect(mockCalls.some((call) => call.cmd === "model_download_tools")).toBe(true);
    expect(mockCalls.some((call) => call.cmd === "job_resume_download")).toBe(false);
  });

  it("shows a stale-plan refusal and retains the original row", async () => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const toast = vi.spyOn(toasts, "toast").mockReturnValue("t");
    await show([stopped()], undefined, { job_resume_download: () => { throw new Error("The saved plan changed. Refresh Jobs."); } });
    await select(); await act(async () => { fireEvent.click(resumeButton()); });
    expect(toast).toHaveBeenCalledWith(expect.objectContaining({ title: "Couldn't resume the download", body: "The saved plan changed. Refresh Jobs." }));
    expect(screen.getByText("A download")).toBeTruthy();
  });
});

describe("Cancel", () => {
  const says = async (kind: Job["kind"]) => {
    const confirm = vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    await show([job(kind)]);
    await act(async () => {
      fireEvent.mouseDown(screen.getByText(`A ${kind}`));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    expect(mockCalls.find((c) => c.cmd === "job_cancel")?.args).toMatchObject({ node: "cell1", id: `job-${kind}` });
    return confirm.mock.calls[0][0].body;
  };

  it("of a download says the Library marks it Incomplete, and how to pick it up", async () => {
    expect(await says("download")).toBe(CANCEL_WORDS.download);
    expect(CANCEL_WORDS.download).toContain("saved revision and files");
  });

  it("of a copy says Copy to nodes… picks it up", async () => {
    expect(await says("pull")).toBe("The partial files stay on disk and the Library marks this copy Incomplete. Copy to nodes… picks up where it stopped.");
  });

  it("of a file server says what it stops", async () => {
    expect(await says("serve")).toBe("This stops the temporary file server. A copy still reading from it fails.");
  });

  const refused = async (words: string) => {
    vi.spyOn(session, "confirmAction").mockResolvedValue(true);
    const toast = vi.spyOn(toasts, "toast").mockReturnValue("t");
    await show([job("download")], () => {
      throw words;
    });
    await act(async () => {
      fireEvent.mouseDown(screen.getByText("A download"));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    return toast.mock.calls[0][0];
  };

  it("says so when the job ended but its container may not have", async () => {
    const words = "Canceled, but its container may still be running: Resume download stops it first.";
    expect(await refused(words)).toMatchObject({ kind: "error", title: "Its container may still be running", body: words });
  });

  it("that couldn't reach the job says it couldn't", async () => {
    expect(await refused("cell1 isn't connected")).toMatchObject({ kind: "error", title: "Couldn't cancel it", body: "cell1 isn't connected" });
  });
});

describe("a job's state", () => {
  it("reads canceled, whatever the wire says", async () => {
    expect(jobWord("cancelled")).toBe("canceled");
    expect(jobWord("done")).toBe("done");
    await show([job("pull", "cancelled", "Cancelled")]);
    expect(screen.getByText("canceled")).toBeTruthy();
    expect(screen.getByText("Canceled")).toBeTruthy();
    expect(screen.queryByText("cancelled")).toBeNull();
    expect(screen.queryByText("Cancelled")).toBeNull();
  });
});

describe("the Library follows the jobs", () => {
  it("refreshes when a job starts or ends, in any state", () => {
    const set = (...ids: string[]) => new Set(ids);
    expect(runningChanged(set("a"), set("a"))).toBe(false);
    expect(runningChanged(set(), set())).toBe(false);
    expect(runningChanged(set("a", "b"), set("a")), "done, failed or canceled: it's no longer running").toBe(true);
    expect(runningChanged(set("a"), set("b"))).toBe(true);
    expect(runningChanged(set(), set("a")), "a new job's folder reads downloading or copying").toBe(true);
  });
});
