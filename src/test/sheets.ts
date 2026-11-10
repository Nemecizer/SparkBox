// Every stylesheet under src/, as text, for the colour-policy and type-floor checks.
// (vite.config.ts lets Vitest keep CSS contents: by default it blanks them.)
const raw = import.meta.glob("../**/*.css", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** "src/windows/net.css" → its text. */
export const sheets: Record<string, string> = Object.fromEntries(Object.entries(raw).map(([k, v]) => [k.replace(/^\.\.\//, "src/"), v]));
