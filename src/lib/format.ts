export const GiB = 1024 ** 3;

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/**
 * "2.5 TiB", "119.7 GiB", "512 MiB". `{decimals: 1}` is the house style for data sizes
 * (§2.3): GiB and up always carry exactly that many decimals, so the Dashboard, Files and
 * the node window print the same "2.5 TiB free". A bare number keeps the older rule
 * (no decimals from 100 up).
 */
export function fmtBytes(bytes: number, digits: number | { decimals: number } = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
  const v = bytes / 1024 ** i;
  if (typeof digits === "object") {
    const d = i >= 3 ? digits.decimals : i === 0 || v >= 100 ? 0 : digits.decimals;
    return `${v.toFixed(d)} ${BYTE_UNITS[i]}`;
  }
  return `${v.toFixed(i === 0 ? 0 : v >= 100 ? 0 : digits)} ${BYTE_UNITS[i]}`;
}

/** "102 / 120 GiB": both values in the unit of the larger one, for tight table cells. */
export function fmtBytesPair(part: number, whole: number): string {
  const i = whole >= 1024 ? Math.min(Math.floor(Math.log(whole) / Math.log(1024)), BYTE_UNITS.length - 1) : 0;
  const scale = 1024 ** i;
  const digits = whole / scale >= 100 || i === 0 ? 0 : 1;
  return `${(part / scale).toFixed(digits)} / ${(whole / scale).toFixed(digits)} ${BYTE_UNITS[i]}`;
}

/** "19.1 / 17.2 Gb/s": receive and transmit in one shared unit. */
/** "39.6 / 75.9 Mb/s" as ["39.6 / 75.9", "Mb/s"], for a tile's big number and small unit. */
export function splitUnit(text: string): [string, string | undefined] {
  const at = text.lastIndexOf(" ");
  return at > 0 ? [text.slice(0, at), text.slice(at + 1)] : [text, undefined];
}

export function fmtBitsPair(rx: number, tx: number): string {
  const top = Math.max(rx, tx);
  if (!Number.isFinite(top) || top < 1) return "0 / 0 b/s";
  const units = ["b/s", "kb/s", "Mb/s", "Gb/s", "Tb/s"];
  const i = Math.min(Math.floor(Math.log10(top) / 3), units.length - 1);
  const scale = 1000 ** i;
  const digits = top / scale >= 100 || i === 0 ? 0 : 1;
  return `${(rx / scale).toFixed(digits)} / ${(tx / scale).toFixed(digits)} ${units[i]}`;
}

/** Network throughput, in bits per second (decimal units). */
export function fmtBits(bps: number): string {
  // A trickle (an LLDP frame now and then) is never shown as nothing at all (C8).
  if (Number.isFinite(bps) && bps > 0 && bps < 1) return "<1 b/s";
  if (!Number.isFinite(bps) || bps < 1) return "0 b/s";
  const units = ["b/s", "kb/s", "Mb/s", "Gb/s", "Tb/s"];
  const i = Math.min(Math.floor(Math.log10(bps) / 3), units.length - 1);
  const v = bps / 1000 ** i;
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

/** Disk throughput, in bytes per second (decimal units, like iostat). */
export function fmtByteRate(Bps: number): string {
  if (!Number.isFinite(Bps) || Bps < 1) return "0 B/s";
  const units = ["B/s", "kB/s", "MB/s", "GB/s"];
  const i = Math.min(Math.floor(Math.log10(Bps) / 3), units.length - 1);
  const v = Bps / 1000 ** i;
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

export function fmtPct(p: number, digits = 0): string {
  return `${p.toFixed(digits)}%`;
}

export function fmtDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

/** Countdown as m:ss below an hour, then h:mm:ss, including durations of a full day. */
export function fmtCountdown(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  const seconds = String(s % 60).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${seconds}`;
  return `${Math.floor(s / 3600)}:${String(Math.floor(s / 60) % 60).padStart(2, "0")}:${seconds}`;
}

export function fmtClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export type Level = "ok" | "warn" | "crit";

export function level(pct: number, warn: number, crit: number): Level {
  return pct >= crit ? "crit" : pct >= warn ? "warn" : "ok";
}

/** Thresholds used everywhere a value turns amber/red. Alert rules reuse these in Phase 5. */
export const THRESHOLDS = {
  memPct: [85, 95],
  diskPct: [80, 92],
  gpuTempC: [80, 90],
} as const;

/**
 * A date and time the way Finder shows it: "Today at 10:42", "Yesterday at 18:03",
 * "Oct 24 at 06:12" this year, "Oct 24, 2025 at 06:12" before. `ms` is Unix milliseconds.
 */
export function fmtDateTime(ms: number, now: number = Date.now()): string {
  if (!Number.isFinite(ms) || ms <= 0) return "—";
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(new Date(now)) - day(d)) / 86_400_000);
  if (days === 0) return `Today at ${time}`;
  if (days === 1) return `Yesterday at ${time}`;
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  const date = d.toLocaleDateString([], sameYear ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
  return `${date} at ${time}`;
}

// ── r2-design ──

/** "3 min ago", "in 2 h", "just now": how far `ms` is from `now`, in the largest whole unit. */
export function fmtRelative(ms: number, now: number = Date.now()): string {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const d = Math.round((now - ms) / 1000);
  const a = Math.abs(d);
  if (a < 5) return "just now";
  const [n, unit] = a < 60 ? [a, "s"] : a < 3600 ? [Math.floor(a / 60), "min"] : a < 86400 ? [Math.floor(a / 3600), "h"] : [Math.floor(a / 86400), a < 172800 ? "day" : "days"];
  return d >= 0 ? `${n} ${unit} ago` : `in ${n} ${unit}`;
}

/** UTC offsets (hours) for the zone abbreviations systemd prints. */
const TZ_ABBR: Record<string, number> = {
  UTC: 0, GMT: 0, Z: 0, WET: 0, BST: 1, CET: 1, WEST: 1, CEST: 2, EET: 2, EEST: 3, MSK: 3, IST: 5.5, JST: 9, KST: 9,
  AEST: 10, AEDT: 11, NZST: 12, NZDT: 13, HST: -10, AKST: -9, AKDT: -8, PST: -8, PDT: -7, MST: -7, MDT: -6,
  CST: -6, CDT: -5, EST: -5, EDT: -4, AST: -4, ADT: -3,
};

/**
 * A systemd or journal time ("Tue 2026-10-27 14:03:11 UTC", "2026-10-27T14:03:11+0000",
 * or the journal's microseconds since the epoch) as the app shows times everywhere:
 * `local` through fmtDateTime ("Today at 2:03 PM"), `utc` for the tooltip and `relative`
 * ("3 min ago"). "n/a", empty and zero give "—" with empty extras.
 */
export function fmtSystemdTime(raw: string | number | null | undefined, now: number = Date.now()): { local: string; utc: string; relative: string; ms: number | null } {
  const none = { local: "—", utc: "", relative: "", ms: null };
  const ms = parseSystemdTime(raw);
  if (ms == null) return none;
  const iso = new Date(ms).toISOString();
  return { local: fmtDateTime(ms, now), utc: `${iso.slice(0, 10)} ${iso.slice(11, 19)} UTC`, relative: fmtRelative(ms, now), ms };
}

/** Unix milliseconds from a systemd timestamp, or null when there's no time. */
export function parseSystemdTime(raw: string | number | null | undefined): number | null {
  if (raw == null) return null;
  const s = String(raw).trim();
  if (!s || s === "n/a" || s === "0") return null;
  if (/^\d+$/.test(s)) {
    const n = Number(s);
    // Microseconds (journal __REALTIME_TIMESTAMP), milliseconds or seconds.
    const ms = n > 1e14 ? n / 1000 : n > 1e11 ? n : n * 1000;
    return ms > 0 ? Math.round(ms) : null;
  }
  const m = /^(?:[A-Za-z]{3},?\s+)?(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?\s*([A-Za-z]{1,5}|[+-]\d{2}:?\d{2})?$/.exec(s);
  if (!m) {
    const t = Date.parse(s);
    return Number.isFinite(t) ? t : null;
  }
  const [, y, mo, d, h, mi, sec, frac, zone] = m;
  const fracMs = frac ? Math.round(Number(`0.${frac}`) * 1000) : 0;
  const utc = Date.UTC(+y, +mo - 1, +d, +h, +mi, +sec, fracMs);
  if (!zone) return new Date(+y, +mo - 1, +d, +h, +mi, +sec, fracMs).getTime();
  if (/^[+-]/.test(zone)) {
    const sign = zone[0] === "-" ? -1 : 1;
    const digits = zone.slice(1).replace(":", "");
    const off = sign * (Number(digits.slice(0, 2)) * 60 + Number(digits.slice(2, 4)));
    return utc - off * 60_000;
  }
  const hours = TZ_ABBR[zone.toUpperCase()];
  // An abbreviation we don't know is read as this Mac's local time rather than guessed.
  return hours === undefined ? new Date(+y, +mo - 1, +d, +h, +mi, +sec, fracMs).getTime() : utc - hours * 3_600_000;
}

// ── other packages: append formatters below, each under its own "// ── r2-<pkg> ──" marker ──

// ── r3-charts ──

const RATE_NAMES = ["b/s", "kb/s", "Mb/s", "Gb/s", "Tb/s"];
const RATE_PREFIX: Record<string, number> = { "": 0, k: 1, M: 2, G: 3, T: 4 };

/** Digits for a rate in its unit: none from 100 up, one from 1, two below (but "0" for nothing). */
const rateDigits = (v: number) => (v === 0 ? 0 : v >= 100 ? 0 : v >= 1 ? 1 : 2);

/**
 * A bit rate that always says its unit: "1.4 Mb/s", "105 Mb/s", "6.1 Gb/s", "0 b/s" (§2.5).
 * `unit` ("k", "M", "G") fixes the unit, so a column of rates lines up in one.
 */
export function fmtRate(bps: number, unit?: "k" | "M" | "G"): string {
  if (!Number.isFinite(bps) || bps < 1) return unit ? `0 ${RATE_NAMES[RATE_PREFIX[unit]]}` : "0 b/s";
  const i = unit ? RATE_PREFIX[unit] : Math.min(Math.floor(Math.log10(bps) / 3), RATE_NAMES.length - 1);
  const v = bps / 1000 ** i;
  return `${v.toFixed(unit ? rateDigits(v) : v >= 100 || i === 0 ? 0 : 1)} ${RATE_NAMES[i]}`;
}

/**
 * Receive and transmit in one unit, for a table cell: "1.0/49.9 Mb/s", "0.12/0.05 Mb/s".
 * The unit is the larger one's, or `unit` when given; a value too small to show reads "<0.01".
 * With `arrows`, each side carries its own unit: "↓ 1.0 Mb/s ↑ 30.9 Mb/s".
 */
export function fmtRatePair(rx: number, tx: number, opts: { unit?: "k" | "M" | "G"; arrows?: boolean } = {}): string {
  if (opts.arrows) return `↓ ${fmtRate(rx, opts.unit)} ↑ ${fmtRate(tx, opts.unit)}`;
  const top = Math.max(Number.isFinite(rx) ? rx : 0, Number.isFinite(tx) ? tx : 0);
  const i = opts.unit ? RATE_PREFIX[opts.unit] : top < 1 ? 0 : Math.min(Math.floor(Math.log10(top) / 3), RATE_NAMES.length - 1);
  const scale = 1000 ** i;
  const one = (x: number) => {
    const v = Number.isFinite(x) && x > 0 ? x / scale : 0;
    const d = opts.unit ? rateDigits(v) : top / scale >= 100 || i === 0 ? 0 : 1;
    if (v > 0 && Number(v.toFixed(d)) === 0) return d === 0 ? "<1" : `<${(10 ** -d).toFixed(d)}`;
    return v.toFixed(d);
  };
  return `${one(rx)}/${one(tx)} ${RATE_NAMES[i]}`;
}

// ── r3-paging ──

// ── r3-agent ──

// ── r3-incidents ──

// ── r3-serving ──

// ── r3-desk ──

// ── r3-safety ──

// ── r4-nodes ──

/** "5.6 ms", "22 ms", "1.2 s": a latency, printed the same everywhere (probe.rs ms_text). */
export function fmtMs(ms: number): string {
  if (ms < 10) return `${ms.toFixed(1)} ms`;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

// ── r4-serving ──

// ── r4-alerts ──

// ── r4-agent ──

// ── r4-safety ──

// ── r4-demo ──

// ── r4-craft ──
