/// <reference types="vitest/config" />
import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and leaves src-tauri to cargo's watcher.
// `npm run dev:mock` serves the same tree on another port; mock.html runs the UI
// against the simulated backend in src/dev, so it works in any browser.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: {
    rollupOptions: { input: { main: resolve(import.meta.dirname, "index.html") } },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // Keep stylesheet contents, so contrast.test.ts and colour-policy.test.ts can read every
    // one of them with ?raw (Vitest blanks CSS by default).
    css: { include: [/\.css(\?|$)/] },
    restoreMocks: true,
  },
});
