// Mock backends for the browser preview, one file per package. Append one
// import below your marker and add the handlers to MOCKS (later entries win).
import type { MockHandlers } from "../../test/tauri";
// ── P1 design ──
// ── r2-design ──
// ── P2 shell ──
// ── r2-shell ──
import { handlers as shell } from "./shell";
// ── P3 ops ──
// ── r2-safety ──
import { handlers as ops } from "./ops";
// ── P4 world ──
// ── r2-fabric ──
import { handlers as net } from "./net";
// ── P5 monitoring ──
// ── r2-monitor ──
import { handlers as monitoring } from "./monitoring";
// ── P6 term-files ──
// ── r2-live ──
import { handlers as termfiles } from "./termfiles";
// ── P7 ai ──
// ── r2-ai ──
import { handlers as ai } from "./ai";
// ── r3-charts ──
// ── r3-paging ──
import { handlers as paging } from "./paging";
// ── r3-agent ──
// ── r3-incidents ──
// ── r3-serving ──
// ── r3-desk ──
// ── r3-safety ──
// ── r4-nodes ──
import { handlers as nodes } from "./r4-nodes";
// ── r4-serving ──
// ── r4-alerts ──
// ── r4-agent ──
// ── r4-safety ──
// ── r4-demo ──
import { handlers as demoTwin } from "./r4-demo";
// ── r4-craft ──

import { handlers as discovery } from "./discovery";
import { handlers as howto } from "./howto";

export const MOCKS: MockHandlers[] = [
  // ── P1 design ──
  // ── r2-design ──
  // ── P2 shell ──
  // ── r2-shell ──
  shell,
  // ── P3 ops ──
  // ── r2-safety ──
  ops,
  // ── P4 world ──
  // ── r2-fabric ──
  net,
  // ── P5 monitoring ──
  // ── r2-monitor ──
  monitoring,
  // ── P6 term-files ──
  // ── r2-live ──
  termfiles,
  // ── P7 ai ──
  // ── r2-ai ──
  ai,
  // ── r3-charts ──
  // ── r3-paging ──
  paging,
  // ── r3-agent ──
  // ── r3-incidents ──
  // ── r3-serving ──
  // ── r3-desk ──
  // ── r3-safety ──
  // ── r4-nodes ──
  nodes,
  // ── r4-serving ──
  // ── r4-alerts ──
  // ── r4-agent ──
  // ── r4-safety ──
  // ── r4-demo ──
  demoTwin,
  discovery,
  howto,
  // ── r4-craft ──
];
