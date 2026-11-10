import { afterEach, expect, it } from "vitest";
import { demoDiskPreflight } from "./disk";

afterEach(() => history.replaceState({}, "", "/"));
import { baseSnapshot } from "../mockBackend";
import { cancelDemoDownload, demoDownloadTools, downloadRecoveryJobs, resumeDemoDownload, startDemoDownload } from "./jobs";

it("browser Demo routes new attempts and resumes to the chosen stable node", () => {
  const node = baseSnapshot.nodes[3].id;
  const review = { node, tool: demoDownloadTools(node).options[0], installConsent: false };
  const old = startDemoDownload({ repo: "fixture/target", review });
  expect(old.node).toBe(node);
  cancelDemoDownload(old.id);
  const resumed = resumeDemoDownload({ node, id: old.id, expected: old.resume!, review });
  expect(resumed.node).toBe(node);
  expect(resumed.id).not.toBe(old.id);
  expect(downloadRecoveryJobs().find((j) => j.id === old.id)?.state).toBe("cancelled");
  cancelDemoDownload(resumed.id);
  const other = { node: "cell1", tool: demoDownloadTools("cell1").options[0], installConsent: false };
  expect(() => resumeDemoDownload({ node: "cell1", id: resumed.id, expected: resumed.resume!, review: other })).toThrow();
});

it("browser Demo refuses unknown targets without creating a job", () => {
  const before = downloadRecoveryJobs().map((j) => j.id);
  const node = "not-in-inventory";
  expect(() => startDemoDownload({ repo: "fixture/target", review: { node, tool: demoDownloadTools(node).options[0], installConsent: false } })).toThrow("connected inventory node");
  expect(downloadRecoveryJobs().map((j) => j.id)).toEqual(before);
});


it.each(["low", "unknown", "changed"])("browser Demo refuses %s disk space without changing saved jobs", (mode) => {
  const node = "cell2";
  const request = { kind: "download" as const, node, repo: "fixture/disk" };
  const review = { node, tool: demoDownloadTools(node).options[0], installConsent: false };
  const old = startDemoDownload({ repo: request.repo, review }); cancelDemoDownload(old.id);
  const before = structuredClone(downloadRecoveryJobs());
  history.replaceState({}, "", `/?disk=${mode}`);
  if (mode === "changed") expect(demoDiskPreflight(request)[0].ok).toBe(true);
  expect(() => startDemoDownload({ repo: request.repo, review })).toThrow();
  expect(() => resumeDemoDownload({ node, id: old.id, expected: old.resume!, review })).toThrow();
  expect(downloadRecoveryJobs()).toEqual(before);
});
