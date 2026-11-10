import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronRight, Lock } from "lucide-react";
import { confirmUnlock, lockOf, LOOK_ONLY_TIP, NEEDS_ADMIN_TIP, useAdminState } from "../lib/admin";
import {
  closeContextMenu, disabledReason, hasSubmenu, isDisabled, isSeparator, useContextMenu, type MenuItem,
} from "../lib/contextMenu";
import "./shell.css";

/**
 * The one lock rule (admin.ts lockOf) on a menu's items, submenus included: an item that needs
 * Admin (`locked`, or the older `hint: "Admin"` tag) gets a trailing lock instead of the tag,
 * and in look-only mode it's disabled, so arrows, Enter and type-ahead pass it by.
 */
export function ruleItems(items: MenuItem[], lookOnly: boolean): MenuItem[] {
  return items.map((item) => {
    if (isSeparator(item)) return item;
    if (hasSubmenu(item)) return { ...item, submenu: ruleItems(item.submenu, lookOnly) };
    const needs = item.locked || item.hint === "Admin";
    if (!needs) return item;
    return { ...item, locked: true, hint: item.hint === "Admin" ? undefined : item.hint, disabled: item.disabled || (lookOnly ? true : undefined) };
  });
}

/** Draws the open context menu (see lib/contextMenu.ts): arrows, Enter, Esc, type-ahead and submenus. */
export function ContextMenuHost() {
  const menu = useContextMenu();
  const admin = useAdminState();
  if (!menu) return null;
  return createPortal(
    <div
      className="ctx-scrim"
      data-shell-overlay
      onPointerDown={(e) => e.target === e.currentTarget && closeContextMenu(false)}
      onContextMenu={(e) => {
        e.preventDefault();
        closeContextMenu(false);
      }}
    >
      <MenuList items={ruleItems(menu.items, !!admin.lookOnly)} x={menu.x} y={menu.y} anchor={menu.anchor} autoFocusFirst={menu.fromKeyboard} onClose={() => closeContextMenu()} depth={0} />
    </div>,
    document.body,
  );
}

function MenuList({ items, x, y, flipX, anchor, autoFocusFirst, onClose, onBack, depth }: {
  items: MenuItem[];
  x: number;
  y: number;
  /** Where the right edge goes when there's no room on the right (submenus open to the left). */
  flipX?: number;
  /** The button a menuAt menu hangs from: it flips above it, or lines up with its right edge. */
  anchor?: { left: number; right: number; top: number; bottom: number };
  autoFocusFirst: boolean;
  onClose: () => void;
  onBack?: () => void;
  depth: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });
  const [hi, setHi] = useState(autoFocusFirst ? firstEnabled(items, 0, 1) : -1);
  const [open, setOpen] = useState<number | null>(null);
  const typed = useRef({ text: "", at: 0 });

  // Keep the menu on screen: flip left or up when it would overflow.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let nx = x;
    let ny = y;
    if (nx + r.width > vw - 4) nx = flipX !== undefined ? flipX - r.width : anchor ? Math.min(anchor.right, vw - 4) - r.width : vw - r.width - 4;
    if (ny + r.height > vh - 4) ny = anchor && anchor.top - 2 - r.height >= 4 ? anchor.top - 2 - r.height : Math.max(4, vh - r.height - 4);
    setPos({ x: Math.max(4, nx), y: Math.max(4, ny) });
  }, [x, y, flipX, anchor?.left, anchor?.top]);

  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
  }, []);

  const activate = (i: number) => {
    const item = items[i];
    if (!item || isSeparator(item) || isDisabled(item)) return;
    if (hasSubmenu(item)) {
      setOpen(i);
      return;
    }
    onClose();
    // Locked: the in-app unlock sheet first (then Touch ID with Live data), and only then the item.
    if (lockOf("locked" in item && item.locked) === "locked") {
      void confirmUnlock(item.label.replace(/…$/, "").replace(/ \(.*\)$/, "").toLowerCase()).then((ok) => ok && item.run?.());
      return;
    }
    void item.run?.();
  };

  const onKey = (e: ReactKeyboardEvent) => {
    if (open !== null) return;
    const k = e.key;
    if (k === "ArrowDown" || k === "ArrowUp") {
      e.preventDefault();
      setHi((h) => firstEnabled(items, h + (k === "ArrowDown" ? 1 : -1), k === "ArrowDown" ? 1 : -1));
    } else if (k === "Home" || k === "End") {
      e.preventDefault();
      setHi(firstEnabled(items, k === "Home" ? 0 : items.length - 1, k === "Home" ? 1 : -1));
    } else if (k === "Enter" || k === " ") {
      e.preventDefault();
      if (hi >= 0) activate(hi);
    } else if (k === "ArrowRight") {
      e.preventDefault();
      if (hi >= 0 && hasSubmenu(items[hi]) && !isDisabled(items[hi])) setOpen(hi);
    } else if (k === "ArrowLeft" || k === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (onBack) onBack();
      else if (k === "Escape") onClose();
    } else if (k === "Tab") {
      e.preventDefault();
    } else if (k.length === 1 && !e.metaKey && !e.ctrlKey) {
      // Type-ahead: jump to the next item starting with what's been typed.
      const now = Date.now();
      typed.current = { text: (now - typed.current.at < 700 ? typed.current.text : "") + k.toLowerCase(), at: now };
      const t = typed.current.text;
      const n = items.length;
      // A first letter moves on from the current row; a longer prefix keeps matching it.
      const from = t.length > 1 ? Math.max(hi, 0) : hi + 1;
      for (let k = 0; k < n; k++) {
        const i = (((from + k) % n) + n) % n;
        const it = items[i];
        if (!isSeparator(it) && !isDisabled(it) && it.label.toLowerCase().startsWith(t)) {
          setHi(i);
          break;
        }
      }
    }
  };

  // Columns only when some row uses them, so a plain menu has no empty gutter.
  const hasChecks = items.some((i) => !isSeparator(i) && "checked" in i && i.checked !== undefined);
  const hasIcons = items.some((i) => !isSeparator(i) && !!i.icon);

  return (
    <div
      ref={ref}
      className="ctx-menu"
      role="menu"
      tabIndex={-1}
      style={{ left: pos.x, top: pos.y }}
      onKeyDown={onKey}
      aria-activedescendant={hi >= 0 ? `ctx-${depth}-${hi}` : undefined}
    >
      {items.map((item, i) => {
        if (isSeparator(item)) return <div key={`sep-${i}`} className="ctx-sep" role="separator" />;
        const disabled = isDisabled(item);
        const reason = disabledReason(item);
        const Icon = item.icon;
        const sub = hasSubmenu(item);
        const checked = !sub && "checked" in item ? item.checked : undefined;
        const lock = !sub && "locked" in item ? lockOf(item.locked) : null;
        return (
          <div
            key={`${item.label}-${i}`}
            id={`ctx-${depth}-${i}`}
            role={checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={checked}
            aria-disabled={disabled || undefined}
            aria-haspopup={sub ? "menu" : undefined}
            aria-expanded={sub ? open === i : undefined}
            className={`ctx-item${i === hi ? " hi" : ""}${disabled ? " disabled" : ""}${!sub && "danger" in item && item.danger ? " danger" : ""}`}
            title={lock === "lookOnly" ? LOOK_ONLY_TIP : lock === "locked" ? NEEDS_ADMIN_TIP : undefined}
            data-lock={lock ?? undefined}
            onPointerEnter={() => {
              setHi(i);
              setOpen(sub && !disabled ? i : null);
            }}
            onClick={() => activate(i)}
          >
            {hasChecks && <span className="ctx-check">{checked && <Check size={12} aria-hidden />}</span>}
            {hasIcons && (Icon ? <Icon className="ctx-icon" size={14} aria-hidden /> : <span className="ctx-icon" />)}
            <span className="ctx-label">
              {item.label}
              {reason && <span className="ctx-reason">{reason}</span>}
            </span>
            {!sub && "hint" in item && item.hint && <span className="ctx-hint">{item.hint}</span>}
            {lock && <Lock className="ctx-lock" size={12} aria-label={lock === "lookOnly" ? "Look-only" : "Needs Admin"} />}
            {!sub && "shortcut" in item && item.shortcut && <span className="ctx-shortcut">{item.shortcut}</span>}
            {sub && <ChevronRight size={12} className="ctx-more" aria-hidden />}
            {sub && open === i && (
              <SubMenu
                items={item.submenu}
                depth={depth + 1}
                onClose={onClose}
                onBack={() => {
                  setOpen(null);
                  ref.current?.focus({ preventScroll: true });
                }}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

/** A submenu opens beside its row. */
function SubMenu({ items, depth, onClose, onBack }: { items: MenuItem[]; depth: number; onClose: () => void; onBack: () => void }) {
  const anchor = useRef<HTMLSpanElement>(null);
  const [at, setAt] = useState<{ x: number; y: number; flipX: number } | null>(null);
  useLayoutEffect(() => {
    const row = anchor.current?.parentElement?.getBoundingClientRect();
    if (row) setAt({ x: row.right - 2, y: row.top - 4, flipX: row.left + 2 });
  }, []);
  return (
    <span ref={anchor} className="ctx-sub-anchor" onClick={(e) => e.stopPropagation()}>
      {at && <MenuList items={items} x={at.x} y={at.y} flipX={at.flipX} autoFocusFirst depth={depth} onClose={onClose} onBack={onBack} />}
    </span>
  );
}

function firstEnabled(items: MenuItem[], from: number, dir: 1 | -1): number {
  const n = items.length;
  for (let k = 0; k < n; k++) {
    const i = (((from + k * dir) % n) + n) % n;
    if (!isSeparator(items[i]) && !isDisabled(items[i])) return i;
  }
  return -1;
}
