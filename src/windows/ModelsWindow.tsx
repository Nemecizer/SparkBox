// Models: the library on the nodes, Hugging Face, what's serving, jobs, node
// readiness, and loading with vLLM. The tabs live in ./models.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Settings2 } from "lucide-react";
import { Icon } from "../components/Icon";
import { Tabs } from "../components/ui";
import { api } from "../lib/api";
import { libraryView, useLibraryReport } from "../lib/library";
import { useCluster } from "../lib/store";
import { usePrefs } from "./settings/GeneralSection";
import { useEndpoints } from "../lib/endpoints";
import { modelsApi, type Deployment } from "../lib/modelsApi";
import type { Job } from "../lib/types";
import { useWM, useWindowEvent, type Params } from "../wm/WindowManager";
import { BenchTab } from "./models/BenchTab";
import { HubTab, type Draft } from "./models/HubTab";
import { JobsTab } from "./models/JobsTab";
import { LibraryTab } from "./models/LibraryTab";
import { HistoryTab } from "./models/HistoryTab";
import { LoadedTab } from "./models/LoadedTab";
import { LoadPage } from "./models/LoadPage";
import { PrepareTab } from "./models/PrepareTab";
import { useNodes } from "./models/shared";

type Tab = "library" | "hf" | "loaded" | "history" | "bench" | "jobs" | "prepare";
const TAB_IDS: Tab[] = ["library", "hf", "loaded", "history", "bench", "jobs", "prepare"];

/** params.tab: one of the tabs, "hub" (older name for hf), or "load" (opens the Load page). */
function tabFrom(p: string | undefined): Tab | "load" | null {
  if (p === "load") return "load";
  if (p === "hub") return "hf";
  return TAB_IDS.includes(p as Tab) ? (p as Tab) : null;
}

/** A job started or ended between two reads of the running ones. */
export function runningChanged(before: ReadonlySet<string>, now: ReadonlySet<string>): boolean {
  return before.size !== now.size || [...before].some((id) => !now.has(id));
}

export function ModelsWindow({ params }: { params: Params }) {
  const wm = useWM();
  const nodes = useNodes();
  const endpoints = useEndpoints();
  const asked = tabFrom(params.tab);
  const [tab, setTab] = useState<Tab>(asked && asked !== "load" ? asked : "library");
  const cluster = useCluster();
  const { prefs } = usePrefs();
  const context = JSON.stringify([cluster?.demo, prefs?.models.modelRoot, cluster?.nodes.map((n) => [n.id, n.host])]);
  const availability = nodes.map((n) => `${n.id}:${n.online}`).join(",");
  const { report, error: libraryError, scanning, refresh: refreshLibrary } = useLibraryReport(context, availability);
  const { entries: library, statuses: libraryNodes } = useMemo(() => libraryView(report, nodes, libraryError), [report, availability, libraryError]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const settings = prefs?.models ?? null;
  const [draft, setDraft] = useState<Draft | null>(asked === "load" ? { key: "", engine: params.engine === "sglang" ? "sglang" : undefined } : null);
  const running = useRef<Set<string>>(new Set());

  // Opening the window again with another tab (the sidebar's vLLM item asks for "load").
  useEffect(() => {
    const t = tabFrom(params.tab);
    if (t === "load" && !params.edit) setDraft((d) => d ?? { key: "", engine: params.engine === "sglang" ? "sglang" : undefined });
    else if (t && t !== "load") setTab(t);
  }, [params.tab, params.nav]);

  // params.edit: Edit & reload a SparkBox load (from its endpoint card).
  useEffect(() => {
    if (!params.edit) return;
    modelsApi.deploymentsList().then((list) => {
      const d = list.find((x) => x.spec.id === params.edit);
      if (d) setDraft({ key: d.spec.modelPath, spec: d.spec, replaces: d.spec.id });
    }, () => {});
  }, [params.edit, params.nav]);

  const refreshJobs = useCallback(async () => {
    try {
      const list = await api.jobsList();
      setJobs(list);
      // A download or copy that starts or ends (done, failed or canceled) changes the library.
      const now = new Set(list.filter((j) => j.state === "running").map((j) => j.id));
      const changed = runningChanged(running.current, now);
      running.current = now;
      if (changed) refreshLibrary();
    } catch {
      /* nodes offline: keep the last list */
    }
  }, [refreshLibrary]);

  const refreshDeployments = useCallback(async () => {
    setDeployments(await modelsApi.deploymentsList().catch(() => []));
  }, []);

  useEffect(() => {
    refreshJobs();
    refreshDeployments();
  }, [refreshLibrary, refreshJobs, refreshDeployments]);

  // Loads change every few seconds while starting; jobs while running.
  useEffect(() => {
    const t = window.setInterval(() => {
      refreshDeployments();
      if (tab === "jobs" || running.current.size > 0) refreshJobs();
    }, 2500);
    return () => window.clearInterval(t);
  }, [tab, refreshDeployments, refreshJobs]);

  // Keep observed transfer phases current while the Library is visible.
  useEffect(() => {
    if (tab !== "library" || scanning) return;
    const t = window.setInterval(() => { if (running.current.size) void refreshLibrary(); }, 10_000);
    return () => window.clearInterval(t);
  }, [tab, scanning, refreshLibrary]);

  // The "in use" badges follow what's serving.
  const serving = endpoints.filter((e) => e.health !== "down").map((e) => e.id).join(",");
  useEffect(() => {
    if (library) refreshLibrary();
    // Only when the set of serving endpoints changes.
  }, [serving]);

  useWindowEvent("refresh", () => {
    refreshLibrary();
    refreshJobs();
    refreshDeployments();
  });

  if (draft) {
    return (
      <LoadPage
        // No folder asked for: the page picks one that fits, or none (it never opens on a red plan).
        draft={draft}
        key={draft.key || (library ? "library" : "empty")}
        library={library ?? []}
        nodes={nodes}
        deployments={deployments}
        settings={settings}
        onClose={() => setDraft(null)}
        onStarted={() => {
          setDraft(null);
          setTab("loaded");
          refreshDeployments();
        }}
        onShared={() => {
          setDraft(null);
          setTab("jobs");
          refreshJobs();
        }}
      />
    );
  }

  const activeJobs = jobs.filter((j) => j.state === "running").length;
  const servingCount = endpoints.filter((e) => e.health === "ready" || e.health === "loading").length;

  return (
    <div className="col fill ai-models">
      <div className="ai-tabs-row">
        <Tabs
          label="Models"
          value={tab}
          onChange={(id) => setTab(id as Tab)}
          tabs={[
            { id: "library", label: "Library", badge: library?.length || undefined },
            { id: "hf", label: "Hugging Face" },
            { id: "loaded", label: "Loaded", badge: servingCount || undefined },
            { id: "history", label: "History" },
            { id: "bench", label: "Benchmarks" },
            { id: "jobs", label: "Jobs", badge: activeJobs || undefined },
            { id: "prepare", label: "Prepare cluster" },
          ]}
        />
        <button type="button" className="ai-settings-link" onClick={() => wm.open("settings", { section: "models" })} title="Model folder, vLLM image, Hugging Face token and the AI advisor">
          <Icon icon={Settings2} size={12} />Settings
        </button>
      </div>
      <div className="tab-body">
        {tab === "library" && (
          <LibraryTab
            library={library}
            error={libraryError}
            statuses={libraryNodes}
            scanning={scanning}
            nodes={nodes}
            onRefresh={refreshLibrary}
            onLoad={(key) => setDraft({ key })}
            onJobs={() => {
              setTab("jobs");
              refreshJobs();
            }}
            onShowLoaded={() => setTab("loaded")}
          />
        )}
        {tab === "hf" && (
          <HubTab
            library={library ?? []}
            onLoad={setDraft}
            onDownloading={() => {
              setTab("jobs");
              refreshJobs();
            }}
          />
        )}
        {tab === "loaded" && (
          <LoadedTab
            deployments={deployments}
            refresh={refreshDeployments}
            onLoad={() => setDraft({ key: "" })}
            onBenchmark={() => setTab("bench")}
          />
        )}
        {tab === "history" && <HistoryTab />}
        {tab === "bench" && <BenchTab />}
        {tab === "jobs" && <JobsTab jobs={jobs} refresh={refreshJobs} />}
        {tab === "prepare" && <PrepareTab />}
      </div>
    </div>
  );
}
