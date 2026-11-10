import { createLucideIcon, type LucideIcon, type LucideProps } from "lucide-react";

/** The three sizes the UI uses: 12 in dense rows and badges, 14 in toolbars, 16 in the sidebar. Larger only for empty states. */
export type IconSize = 12 | 14 | 16 | 20 | 32;

/**
 * One stroke weight for every icon (1.75 on lucide's 24-unit grid), so a 14px
 * toolbar glyph and a 16px sidebar glyph look like the same family. Icons are
 * decorative unless given an aria-label.
 */
export function Icon({ icon: Glyph, size = 14, ...rest }: { icon: LucideIcon; size?: IconSize } & Omit<LucideProps, "size" | "ref">) {
  const labelled = rest["aria-label"] !== undefined;
  return <Glyph size={size} strokeWidth={1.75} aria-hidden={labelled ? undefined : true} role={labelled ? "img" : undefined} focusable={false} {...rest} />;
}

/**
 * Stop: a filled square inside a circle, so it reads as an action (like a media stop
 * button) and never as an empty checkbox. Drop-in for any lucide icon (toolbars, menus).
 */
export const StopIcon: LucideIcon = createLucideIcon("sparkbox-stop", [
  ["circle", { cx: "12", cy: "12", r: "9.5", key: "ring" }],
  ["rect", { x: "8.25", y: "8.25", width: "7.5", height: "7.5", rx: "1.25", fill: "currentColor", key: "square" }],
]);
