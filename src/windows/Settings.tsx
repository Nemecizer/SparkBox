// Settings as a Mac preferences window: a searchable list of sections on the
// left, the chosen section on the right. Sections come from settings/index.ts,
// where each package registers its own. `params.section` opens a given one;
// otherwise the window reopens on the last section used. In look-only mode every section's
// form is read-only (a disabled fieldset), except the ones that stay usable then.
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { SearchX } from "lucide-react";
import { Icon } from "../components/Icon";
import { EmptyState, HelpTip, SearchField } from "../components/ui";
import { useAdminState } from "../lib/admin";
import type { Params } from "../wm/WindowManager";
import { sectionMatches, sortedSections } from "./settings/index";

export { KeychainField } from "./settings/LoginSection";

const LAST_KEY = "sparkbox.settings.section";

/**
 * Sections that stay usable in look-only mode: Security & privacy (its look-only switch; the
 * section makes the rest of itself read-only), Diagnostics (export, evidence) and the fence
 * (closing one stays allowed). Every other section's form is read-only there, instead of
 * taking edits that Save then refuses.
 */
export const OPEN_IN_LOOK_ONLY = ["security", "diagnostics", "fence"];

function remembered(): string | null {
  try {
    return localStorage.getItem(LAST_KEY);
  } catch {
    return null;
  }
}

export function Settings({ params }: { params: Params }) {
  const all = sortedSections();
  const valid = (id: string | null | undefined) => (id && all.some((s) => s.id === id) ? id : null);
  const [current, setCurrent] = useState(() => valid(params.section) ?? valid(remembered()) ?? all[0].id);
  const [query, setQuery] = useState("");
  const lookOnly = !!useAdminState().lookOnly;
  const listRef = useRef<HTMLUListElement>(null);
  const titleId = useId();

  useEffect(() => {
    const id = valid(params.section);
    if (id) setCurrent(id);
    // Only when a deep link asks (nav counts repeat links to the same section).
  }, [params.section, params.nav]);

  const choose = (id: string) => {
    setCurrent(id);
    try {
      localStorage.setItem(LAST_KEY, id);
    } catch {
      // Not remembered; the window still works.
    }
  };

  const visible = all.filter((s) => sectionMatches(s, query));
  const section = all.find((s) => s.id === current) ?? all[0];
  const Body = section.component;

  const onListKey = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const i = visible.findIndex((s) => s.id === current);
    const to = e.key === "Home" ? 0 : e.key === "End" ? visible.length - 1 : Math.max(0, Math.min(visible.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
    const next = visible[to];
    if (!next) return;
    choose(next.id);
    listRef.current?.querySelector<HTMLButtonElement>(`[data-section="${next.id}"]`)?.focus();
  };

  return (
    <div className="settings-win">
      <nav className="settings-nav" aria-label="Settings sections">
        <div
          className="settings-search"
          onKeyDown={(e) => {
            if (e.key === "Enter" && visible.length > 0) choose(visible[0].id);
            if (e.key === "ArrowDown" && visible.length > 0) {
              e.preventDefault();
              listRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
            }
          }}
        >
          <SearchField value={query} onChange={setQuery} placeholder="Search" label="Search settings" />
        </div>
        <ul className="settings-list" ref={listRef} onKeyDown={onListKey}>
          {visible.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                data-section={s.id}
                className={`settings-item${s.id === section.id ? " on" : ""}`}
                aria-current={s.id === section.id ? "page" : undefined}
                tabIndex={s.id === section.id || (!visible.some((v) => v.id === section.id) && s === visible[0]) ? 0 : -1}
                onClick={() => choose(s.id)}
              >
                <span className="settings-item-icon" style={s.tint ? { background: s.tint } : undefined}><Icon icon={s.icon} size={14} /></span>
                <span className="settings-item-label">{s.label}</span>
              </button>
            </li>
          ))}
        </ul>
        {visible.length === 0 && <p className="settings-nomatch muted">No settings match “{query}”.</p>}
      </nav>
      <section className="settings-pane" aria-labelledby={titleId}>
        <header className="settings-pane-head">
          <Icon icon={section.icon} size={16} />
          <h2 id={titleId}>{section.label}</h2>
          <HelpTip topic={section.help} label={`About ${section.label}`} />
        </header>
        <div className="settings-pane-body" key={section.id}>
          <fieldset className="settings-fieldset" disabled={lookOnly && !OPEN_IN_LOOK_ONLY.includes(section.id)}>
            {Body ? <Body /> : <EmptyState icon={SearchX} title="Nothing here yet" />}
          </fieldset>
        </div>
      </section>
    </div>
  );
}
