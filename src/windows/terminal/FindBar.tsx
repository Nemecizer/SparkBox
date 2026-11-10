import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import type { ISearchOptions } from "@xterm/addon-search";
import { setFindOpen, type TermTab } from "../../lib/terminals";
import { cssVar } from "../../lib/theme";

/** ⌘F in a terminal tab: searches its scrollback, with "n of m", case and regex options. */
export function FindBar({ tab }: { tab: TermTab }) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCase] = useState(false);
  const [regex, setRegex] = useState(false);
  const [result, setResult] = useState<{ index: number; count: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, [tab.key]);

  useEffect(() => {
    const sub = tab.search.onDidChangeResults((r) => setResult(r ? { index: r.resultIndex, count: r.resultCount } : null));
    return () => {
      sub.dispose();
      tab.search.clearDecorations();
    };
  }, [tab]);

  const opts = (incremental = false): ISearchOptions => ({
    caseSensitive,
    regex,
    incremental,
    decorations: {
      matchBackground: cssVar("--warning") || "#efa810",
      matchOverviewRuler: cssVar("--warning") || "#efa810",
      activeMatchBackground: cssVar("--serious") || "#e67339",
      activeMatchColorOverviewRuler: cssVar("--serious") || "#e67339",
    },
  });

  const find = (dir: 1 | -1, incremental = false) => {
    if (!query) {
      tab.search.clearDecorations();
      setResult(null);
      return;
    }
    try {
      if (dir === 1) tab.search.findNext(query, opts(incremental));
      else tab.search.findPrevious(query, opts());
    } catch {
      setResult({ index: -1, count: 0 }); // an unfinished regular expression
    }
  };

  // Search as you type, and again when an option changes.
  useEffect(() => {
    find(1, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, caseSensitive, regex]);

  useEffect(() => {
    const onNext = (e: Event) => find((e as CustomEvent<1 | -1>).detail);
    window.addEventListener("sparkbox:terminal-find", onNext);
    return () => window.removeEventListener("sparkbox:terminal-find", onNext);
  });

  const close = () => {
    setFindOpen(false);
    tab.term.focus();
  };

  const status = !query ? "" : !result || result.count === 0 ? "No results" : result.index < 0 ? `${result.count}+ matches` : `${result.index + 1} of ${result.count}`;
  return (
    <div className="sb-term-find" role="search" aria-label="Find in terminal">
      <input
        ref={input}
        data-find-target
        value={query}
        placeholder="Find in scrollback"
        aria-label="Find in scrollback"
        spellCheck={false}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            find(e.shiftKey ? -1 : 1);
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            close();
          }
        }}
      />
      <span className={`sb-term-find-count${query && result?.count === 0 ? " none" : ""}`} aria-live="polite">{status}</span>
      <button type="button" className={`sb-term-find-opt${caseSensitive ? " on" : ""}`} aria-pressed={caseSensitive} title="Match case" onClick={() => setCase(!caseSensitive)}>Aa</button>
      <button type="button" className={`sb-term-find-opt mono${regex ? " on" : ""}`} aria-pressed={regex} title="Regular expression" onClick={() => setRegex(!regex)}>.*</button>
      <button type="button" className="sb-term-find-btn" title="Previous (⇧Return)" aria-label="Previous match" onClick={() => find(-1)}><ChevronUp size={14} /></button>
      <button type="button" className="sb-term-find-btn" title="Next (Return)" aria-label="Next match" onClick={() => find(1)}><ChevronDown size={14} /></button>
      <button type="button" className="sb-term-find-btn" title="Close (Esc)" aria-label="Close find" onClick={close}><X size={14} /></button>
    </div>
  );
}
