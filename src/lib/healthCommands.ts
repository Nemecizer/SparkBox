// Palette commands for alerts (App.tsx registers them once): open Alerts,
// acknowledge or silence any active incident by name, and send a test
// notification. The per-incident commands follow the health engine as
// incidents come and go.
import { Bell, BellOff, Check, Send } from "lucide-react";
import { ADMIN_REFUSED, withAdmin } from "./admin";
import { registerCommands, type Command } from "./commands";
import { alertScope, applyHealth, getHealth, healthApi, subscribeHealth, type HealthState, type Incident } from "./health";
import { notifyApi } from "./notifyApi";
import { toast } from "./toasts";
import { openWindow } from "../wm/WindowManager";


function incidentCommands(h: HealthState | null): Command[] {
  const out: Command[] = [];
  for (const i of (h?.incidents ?? []).filter((x) => x.state === "firing" || x.state === "recovering")) {
    const entity = { kind: "alert", id: i.id, name: i.title };
    // Acknowledging and a 1-hour silence are node scope (health.ts alertScope).
    const run = (what: "alert_ack" | "alert_silence", fn: () => Promise<HealthState>, done: string) => async () => {
      try {
        applyHealth(await withAdmin(fn, { scope: alertScope(what, getHealth()?.demo !== false, "1h"), action: what }));
        toast({ kind: "success", title: done, key: `alert:${i.id}` });
      } catch (e) {
        if (e === ADMIN_REFUSED) return;
        toast({ kind: "error", title: "Couldn't change the alert", body: String((e as { message?: string })?.message ?? e), key: `alert:${i.id}` });
      }
    };
    out.push({
      id: `alert:ack:${i.id}`,
      title: `Acknowledge ${i.title}`,
      group: "Alerts",
      subtitle: "A known issue: no repeats and it stops counting, until it gets worse",
      icon: Check,
      verbs: ["acknowledge", "ack"],
      entity,
      needsAdmin: true,
      run: run("alert_ack", () => healthApi.ack(i.id), `Acknowledged ${i.title}`),
    });
    out.push({
      id: `alert:silence:${i.id}`,
      title: `Silence ${i.title} for 1 hour`,
      group: "Alerts",
      subtitle: "Not counted or paged for an hour; longer silences are in Alerts",
      icon: BellOff,
      verbs: ["silence", "mute", "snooze"],
      entity,
      needsAdmin: true,
      run: run("alert_silence", () => healthApi.silence({ incident: i.id }, "1h", "", label(i)), `Silenced ${i.title} for 1 hour`),
    });
  }
  return out;
}

/** What a silence on an incident is called (in the sheet and the log): its title when that names
 * what it affects ("cell3 offline"), else what it affects ("Root disk on gx10-0000"). */
export function label(i: Incident): string {
  return !i.affects || i.title.toLowerCase().includes(i.affects.toLowerCase()) ? i.title : i.affects;
}

const STATIC: Command[] = [
  {
    id: "alerts:open",
    title: "Open Alerts",
    group: "Alerts",
    icon: Bell,
    verbs: ["open", "show"],
    keywords: ["alerts", "problems", "warnings", "critical", "events", "xid", "incidents", "silences"],
    run: () => openWindow("alerts"),
  },
  {
    id: "alerts:silenced",
    title: "Show silenced alerts",
    group: "Alerts",
    icon: BellOff,
    verbs: ["show", "open"],
    keywords: ["silenced", "muted", "known issues", "snoozed"],
    run: () => openWindow("alerts", { tab: "silenced" }),
  },
  {
    id: "alerts:test-notification",
    title: "Send test notification",
    group: "Alerts",
    subtitle: "To every enabled channel, from this Mac",
    icon: Send,
    verbs: ["send", "test"],
    keywords: ["notification", "ntfy", "pushover", "webhook", "slack", "email", "phone"],
    run: async () => {
      try {
        const channels = (await notifyApi.channels()).filter((c) => c.enabled);
        if (!channels.length) {
          toast({ kind: "info", title: "No channel is enabled", body: "Turn one on in Settings › Notifications." });
          return;
        }
        const results = await Promise.all(channels.map((c) => notifyApi.test(c.id).then((r) => ({ c, r }), (e) => ({ c, r: { ok: false, message: String(e), delivery: null } }))));
        const failed = results.filter((x) => !x.r.ok);
        if (failed.length) {
          toast({ kind: "error", title: `Test failed on ${failed.map((x) => x.c.name).join(", ")}`, body: failed[0].r.message });
        } else {
          toast({ kind: "success", title: `Test sent to ${results.length === 1 ? results[0].c.name : `${results.length} channels`}` });
        }
      } catch (e) {
        toast({ kind: "error", title: "Couldn't send the test", body: String(e) });
      }
    },
  },
];

/** Registers the alert commands; returns a function that removes them. */
export function registerHealthCommands(): () => void {
  const offStatic = registerCommands(STATIC);
  let offIncidents = registerCommands(incidentCommands(getHealth()));
  let key = "";
  const offHealth = subscribeHealth((h) => {
    const next = (h?.incidents ?? []).map((i) => `${i.id}|${i.state}|${i.title}`).join(";");
    if (next === key) return;
    key = next;
    offIncidents();
    offIncidents = registerCommands(incidentCommands(h));
  });
  return () => {
    offHealth();
    offIncidents();
    offStatic();
  };
}
