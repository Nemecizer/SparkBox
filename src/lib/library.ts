import { useCallback, useEffect, useRef, useState } from "react";
import { modelsApi, type LibraryEntry, type LibraryReport, type LibraryNodeStatus } from "./modelsApi";

/** Every copy is a dated observation, never authority for a node mutation. */
export function libraryView(report: LibraryReport | null, nodes: readonly { id: string; online: boolean }[], error: string | null = null): {
  entries: LibraryEntry[] | null; statuses: LibraryNodeStatus[];
} {
  const statuses = nodes.map((n): LibraryNodeStatus => {
    const old = report?.nodes.find((s) => s.node === n.id);
    return { node: n.id, checkedAtMs: old?.checkedAtMs ?? null,
      state: !n.online ? "offline" : error ? "error" : old?.state ?? "unknown",
      error: error ?? old?.error ?? null };
  });
  const entries = report?.entries.map((e) => ({ ...e, copies: Object.fromEntries(Object.entries(e.copies)
    .filter(([id]) => nodes.some((n) => n.id === id))
    .map(([id, c]) => [id, { ...c, stale: statuses.find((s) => s.node === id)?.state !== "ready" }])) }))
    .filter((e) => Object.keys(e.copies).length > 0) ?? null;
  return { entries, statuses };
}

/** Discard responses from older refreshes, previous settings/worlds and closed windows. */
export function useLibraryReport(context: string, availability = "") {
  const [result, setResult] = useState<{ context: string; report: LibraryReport } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    setScanning(true);
    try {
      const report = await modelsApi.libraryReport();
      if (request !== generation.current) return;
      setResult({ context, report });
      setError(null);
    } catch (e) {
      if (request === generation.current) setError(String(e).replace(/^Error: /, ""));
    } finally {
      if (request === generation.current) setScanning(false);
    }
  }, [context]);
  useEffect(() => {
    // Reconnection alone is not a successful scan. Invalidate previous freshness
    // before dispatch so an offline → online round trip cannot revive old badges.
    setResult((old) => old && ({ ...old, report: { ...old.report,
      nodes: old.report.nodes.map((n) => ({ ...n, state: "unknown" as const })),
    } }));
    setError(null);
    void refresh();
    return () => { generation.current++; };
  }, [refresh, availability]);
  return { report: result?.context === context ? result.report : null, error, scanning, refresh };
}
