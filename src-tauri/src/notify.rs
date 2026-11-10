//! Where alerts go: this Mac's Notification Center, and the owner's phone or team
//! through ntfy, Pushover, a webhook (or Slack) and email (src-tauri/src/delivery).
//! Also notifications for long tasks that finish in the background.
//!
//! What pages is decided by one gate (`Gate`, plan §2.1), the same algorithm the node
//! agents run (sparkbox_agent.py), both checked against agent/fixtures/gate-parity.json.
//! Its state is per alert key K = (rule, entity). A page is never dropped silently:
//! every alert event gets an outcome, recorded in the delivery log.
//! - The only dedupe: the same K raised again at the same severity under 5 minutes
//!   after it was sent, with no resolve in between (a restart re-raising it).
//! - Critical alerts and escalations always page, quiet hours included.
//! - Warnings are held during quiet hours or with critical-only on, and coalesce into
//!   their incident when it was paged in the last 30 minutes (its next message lists them).
//! - The resolve of anything paged is sent once it has stayed clear for 10 minutes (in quiet
//!   hours only a paged critical's). Raised again before then at no higher a severity, it's
//!   flapping: not paged again, and the clear is never sent (G16). A node being back online is
//!   never held: it's sent at once. Gone again within 10 minutes of that return, it's paged again;
//!   the second time as flapping ("cell3 offline · flapping"). After that its drops and returns
//!   aren't paged, unless it stays away 10 minutes ("Still flapping: …"), until a raise comes more
//!   than 10 minutes after its last return (G16b). A de-escalation isn't paged,
//!   and the climb back within the repeat interval isn't either. An unacknowledged critical
//!   repeats "Still firing" ("Still flapping" once it has flapped) every `repeat_minutes`; an
//!   acknowledgement stops that.
//! - An incident a node's agent paged counts as paged here too: the Mac's warnings in it are
//!   coalesced into its next message (G17).
//! - What waits in the group window is saved with the gate, so a quit inside it doesn't lose it.
//! - Sends due within 10 s are one message per incident, and more than two messages at
//!   once become one grouped message naming every critical first.
//! - Quiet hours follow one IANA time zone, captured from this Mac when they're saved,
//!   whatever clock the Mac or a node has now. When they end, the warnings they held that
//!   are still firing go out as one digest ("While quiet: 2 warnings still firing").
//!
//! The same gate drives Notification Center and the phone channels. Who sends a phone
//! channel is decided per alert and channel (`agent::agent_sends`): a node's agent sends
//! its node's alerts only while it's healthy and has the current settings; a channel
//! marked "this Mac only" always goes from here. A stale agent may still page with old
//! settings, so a duplicate is possible; silence isn't. In Demo nothing leaves the Mac:
//! deliveries are recorded as "Demo — not sent". A Test button always sends for real.
//!
//! Settings and channels live in notify.json, channel secrets in the Keychain, and the
//! gate's state in gate.json (gate-demo.json for Demo), so a restart doesn't page again.
//!
//! Debug builds run with SPARKBOX_DEV_MUTE_NOTIFY=1 (every Live run of the dev app) decide
//! and record every delivery as usual, with outcome `muted`: no notification is shown and no
//! channel is sent to, Test buttons included.
//!
//! A click on a notification opens what it's about (`notify://clicked`). When
//! SparkBox comes to the front some other way within 10 minutes of one,
//! `notify://pending` lets the shell offer "Last notification · Open" instead
//! of opening anything by itself.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::{NotificationExt, PermissionState};

use crate::delivery::{self, Channel, Delivery, Kind, Message, Outcome};
use crate::health::{Alert, AlertEvent, Category, EventKind, Severity};

/// Sends due within this window go out together.
pub const GROUP_WAIT_MS: u64 = 10_000;
/// The same alert raised again at the same severity this soon after it was sent is a duplicate.
pub const DUPLICATE_MS: u64 = 5 * 60_000;
/// A warning in an incident paged this recently is listed in its next message instead.
pub const COALESCE_MS: u64 = 30 * 60_000;
/// A resolve waits this long clear before it's sent; a raise inside it is flapping.
pub const FLAP_MS: u64 = crate::health::FLAP_WINDOW_MS;
/// More than this many messages at once are grouped into one.
const GROUP_OVER: usize = 2;
/// A grouped message lists at most this many alerts, then "and N more".
const GROUP_LINES: usize = 6;
/// The repeat intervals Settings offers (0: never).
pub const REPEAT_CHOICES: [u32; 5] = [15, 30, 60, 240, 0];
/// A channel's Test button sends at most once in this long.
const TEST_EVERY_MS: u64 = 10_000;
/// How long after a notification coming to the front still offers it.
const PENDING_MS: u64 = 10 * 60_000;
pub const CLICKED_EVENT: &str = "notify://clicked";
pub const PENDING_EVENT: &str = "notify://pending";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifySettings {
    /// Notification Center on this Mac.
    pub enabled: bool,
    /// Offline, not updating, memory pressure.
    pub node: bool,
    pub disk: bool,
    pub gpu: bool,
    /// Fabric links, switches and optics.
    pub fabric: bool,
    /// Containers and systemd services.
    pub containers: bool,
    /// Long operations that finish while SparkBox is in the background.
    pub tasks: bool,
    /// Only critical alerts (warnings stay in the Alerts window).
    pub critical_only: bool,
    /// Also say when an alert clears.
    pub resolved: bool,
    /// Between these times (in `quiet_tz`) only critical alerts notify.
    pub quiet_hours: bool,
    pub quiet_start: String,
    pub quiet_end: String,
    /// The IANA zone quiet hours follow, captured from this Mac when they're saved.
    pub quiet_tz: String,
    /// "Still firing" for an unacknowledged critical alert every this many minutes (0: never).
    pub repeat_minutes: u32,
    /// When the channels or these settings last changed: a test page after this proves them.
    pub changed_ms: u64,
    /// Phone and team channels (no secrets: those are in the Keychain).
    pub channels: Vec<Channel>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings {
            enabled: true,
            node: true,
            disk: true,
            gpu: true,
            fabric: true,
            containers: true,
            tasks: true,
            critical_only: false,
            resolved: true,
            quiet_hours: false,
            quiet_start: "22:00".into(),
            quiet_end: "07:00".into(),
            quiet_tz: String::new(),
            repeat_minutes: 60,
            changed_ms: 0,
            channels: Vec::new(),
        }
    }
}

impl NotifySettings {
    fn category(&self, c: Category) -> bool {
        match c {
            Category::Node => self.node,
            Category::Disk => self.disk,
            Category::Gpu => self.gpu,
            Category::Fabric => self.fabric,
            Category::Containers => self.containers,
        }
    }

    pub fn quiet(&self) -> Option<Quiet> {
        self.quiet_hours.then(|| Quiet { start: self.quiet_start.clone(), end: self.quiet_end.clone(), tz: self.quiet_tz.clone() })
    }

    /// What the gate needs.
    pub fn gate(&self) -> GateSettings {
        GateSettings { critical_only: self.critical_only, resolved: self.resolved, repeat_minutes: self.repeat_minutes, quiet: self.quiet() }
    }

    /// The phone and team channels alerts go to (not the dead-man's switch).
    pub fn paging_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(|c| c.enabled && c.kind.pages())
    }
}

/// "22:00" → minutes after midnight.
fn minutes(hhmm: &str) -> Option<u32> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Whether `now` (minutes after midnight) falls in a quiet window that may wrap midnight.
pub fn in_quiet(now: u32, start: &str, end: &str) -> bool {
    let (Some(s), Some(e)) = (minutes(start), minutes(end)) else { return false };
    if s == e {
        false
    } else if s < e {
        now >= s && now < e
    } else {
        now >= s || now < e
    }
}

/// An IANA zone, or this Mac's when it's empty or unknown.
fn zone(tz: &str) -> jiff::tz::TimeZone {
    if tz.is_empty() {
        return jiff::tz::TimeZone::system();
    }
    jiff::tz::TimeZone::get(tz).unwrap_or_else(|_| jiff::tz::TimeZone::system())
}

/// Minutes after midnight at `now_ms` in `tz`.
pub fn minutes_in(tz: &str, now_ms: u64) -> Option<u32> {
    let z = jiff::Timestamp::from_millisecond(now_ms as i64).ok()?.to_zoned(zone(tz));
    Some(z.hour() as u32 * 60 + z.minute() as u32)
}

/// "10:42" at `ms` in `tz`.
pub fn clock_in(tz: &str, ms: u64) -> String {
    jiff::Timestamp::from_millisecond(ms as i64).map(|t| t.to_zoned(zone(tz)).strftime("%H:%M").to_string()).unwrap_or_default()
}

/// This Mac's IANA zone right now ("America/New_York"), or "" if it has none.
pub fn mac_zone() -> String {
    jiff::tz::TimeZone::system().iana_name().unwrap_or("").to_string()
}

pub fn valid_zone(tz: &str) -> bool {
    jiff::tz::TimeZone::get(tz).is_ok()
}

/// Quiet hours in one declared zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quiet {
    pub start: String,
    pub end: String,
    pub tz: String,
}

impl Quiet {
    pub fn active(&self, now_ms: u64) -> bool {
        minutes_in(&self.tz, now_ms).is_some_and(|m| in_quiet(m, &self.start, &self.end))
    }

    /// "22:00 to 07:00 · America/New_York".
    pub fn label(&self) -> String {
        let tz = if self.tz.is_empty() { "this Mac's time" } else { &self.tz };
        format!("{} to {} · {tz}", self.start, self.end)
    }
}

/// What the gate's decisions depend on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GateSettings {
    pub critical_only: bool,
    pub resolved: bool,
    pub repeat_minutes: u32,
    pub quiet: Option<Quiet>,
}

/// What an alert is about, for working out who sends it (agent::agent_sends).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scope {
    pub rule: String,
    /// The entity kind: node, endpoint, port, container…
    pub kind: String,
    pub id: String,
    pub node: Option<String>,
    pub switch: Option<String>,
}

impl Scope {
    pub fn of(a: &Alert) -> Scope {
        Scope { rule: a.rule.clone(), kind: a.entity.kind.clone(), id: a.entity.id.clone(), node: a.entity.node.clone(), switch: a.entity.switch.clone() }
    }
}

/// The alert events the gate decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Raised,
    Escalated,
    Deescalated,
    Resolved,
}

/// One alert event, as the gate sees it.
#[derive(Clone, Debug)]
pub struct GateEvent {
    pub step: Step,
    pub rule: String,
    pub entity: String,
    pub severity: Severity,
    /// The incident it belongs to (plan §2.2); its own entity when it has no other root.
    pub incident: String,
    /// The message's title if it's sent on its own: "cell3 offline", "cell3 is back online".
    pub title: String,
    pub body: String,
    pub scope: Scope,
    pub alert_id: Option<String>,
    pub open: Option<(String, Value)>,
    /// Why it's silenced, when it is.
    pub silenced: Option<String>,
    /// The planned work it's part of: "cell3 restarting (SparkBox, 10:42)".
    pub planned: Option<String>,
    /// A resolve the health engine already held for the flap window (a flapping alert clears only
    /// after 10 minutes clear): it goes with the next tick.
    pub clear_held: bool,
}

/// What became of one alert event.
#[derive(Clone, Debug, PartialEq)]
pub struct GateOutcome {
    pub rule: String,
    pub entity: String,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub title: String,
    pub severity: Severity,
    pub alert_id: Option<String>,
    pub incident: String,
}

/// Why something is in a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    Raised,
    Escalated,
    Resolved,
    /// "Still firing: …"
    Repeat,
    /// Held during quiet hours and still firing when they ended.
    Digest,
}

/// One alert in a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub why: Why,
    pub rule: String,
    pub entity: String,
    pub severity: Severity,
    pub incident: String,
    pub title: String,
    pub body: String,
    pub scope: Scope,
    pub alert_id: Option<String>,
    pub open: Option<(String, Value)>,
}

/// One notification to post: one alert, one incident, or a group of them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    /// Shared by every alert in it, in the delivery log.
    pub message_id: String,
    pub title: String,
    pub body: String,
    /// The other alerts it names (and "Also: …" for alerts coalesced into its incident).
    pub lines: Vec<String>,
    /// The worst severity in it.
    pub severity: Severity,
    /// Every alert in it cleared.
    pub resolved: bool,
    pub node: Option<String>,
    pub rule: Option<String>,
    pub alert_id: Option<String>,
    pub open: Option<(String, Value)>,
    pub members: Vec<Member>,
    pub(crate) also: Vec<String>,
}

impl Notice {
    /// A notice that isn't about alerts (a task, a sample).
    pub fn plain(title: &str, body: &str, open: Option<(String, Value)>) -> Notice {
        Notice {
            message_id: message_id(),
            title: title.into(),
            body: body.into(),
            lines: Vec::new(),
            severity: Severity::Info,
            resolved: false,
            node: None,
            rule: None,
            alert_id: None,
            open,
            members: Vec::new(),
            also: Vec::new(),
        }
    }

    /// The same message with only the alerts `keep` keeps (a channel that takes critical
    /// alerts only, or the alerts this Mac sends itself); None when nothing is left.
    pub fn subset(&self, keep: impl Fn(&Member) -> bool) -> Option<Notice> {
        let kept: Vec<Member> = self.members.iter().filter(|m| keep(m)).cloned().collect();
        if kept.is_empty() {
            return None;
        }
        if kept.len() == self.members.len() {
            return Some(self.clone());
        }
        let mut n = compose_all(group_by_incident(kept), Vec::new());
        n.message_id = self.message_id.clone();
        Some(n)
    }
}

/// Debug builds with SPARKBOX_DEV_MUTE_NOTIFY=1: nothing is shown or sent; every delivery is
/// recorded with outcome `muted`.
pub fn dev_muted() -> bool {
    static MUTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MUTED.get_or_init(|| cfg!(debug_assertions) && std::env::var("SPARKBOX_DEV_MUTE_NOTIFY").is_ok_and(|v| v == "1"))
}

/// Why a muted row wasn't sent.
const MUTED_REASON: &str = "Notifications are muted in this dev run (SPARKBOX_DEV_MUTE_NOTIFY)";

fn message_id() -> String {
    format!("m-{}-{:06x}", crate::history::now_ms(), fastrand::u32(..0xff_ffff))
}

/// "30 s", "4 min", "2 h".
pub fn fmt_span(ms: u64) -> String {
    let s = ms / 1000;
    if s < 90 {
        format!("{s} s")
    } else if s < 90 * 60 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{} h", (s + 1800) / 3600)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct KeyState {
    sent_severity: Option<Severity>,
    sent_step: Option<Step>,
    sent_at_ms: Option<u64>,
    /// Paged since it was last raised, so its resolve is sent.
    paged_since_raise: bool,
    /// The worst severity paged since it was raised (quiet hours send a critical's resolve only).
    paged_severity: Option<Severity>,
    last_page_ms: Option<u64>,
    firing: bool,
    resolved_since_sent: bool,
    severity: Option<Severity>,
    acked: bool,
    silenced: bool,
    planned: bool,
    /// What a repeat says, from the last page.
    title: String,
    body: String,
    incident: String,
    scope: Scope,
    alert_id: Option<String>,
    /// A warning held for quiet hours, for the digest when they end: what it said.
    held_quiet: Option<String>,
    // ── r4-alerts ──
    /// A paged alert's resolve, waiting to stay clear for 10 minutes (sent by `tick` then).
    resolve: Option<Member>,
    resolve_due_ms: Option<u64>,
    /// Raised again while its resolve waited: repeats say "Still flapping".
    flapped: bool,
    // ── r1-h ──
    /// A node's last return (sent at once, or not sent while it flaps): raised again within 10
    /// minutes of it, the node is coming and going (G16b).
    back_ms: Option<u64>,
    /// Re-raises within 10 minutes of a return, in this run of them: the second is paged as
    /// flapping, the later ones aren't.
    flaps: u32,
    /// A flapping node's going away that wasn't paged, since then: its return isn't sent either,
    /// and still away 10 minutes later it's paged ("Still flapping").
    away_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IncidentState {
    paged_severity: Option<Severity>,
    paged_at_ms: Option<u64>,
    paged_title: String,
    /// Warnings coalesced into it, for its next message.
    pub(crate) also: Vec<String>,
}

/// Decides what to post for alert events. Pure (time comes in), so the rules are testable;
/// its state is saved between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Gate {
    keys: BTreeMap<String, KeyState>,
    incidents: BTreeMap<String, IncidentState>,
    /// Quiet hours were on at the last tick (their end sends the digest).
    quiet_was: bool,
    /// What's waiting in the group window, saved so a quit inside it still sends it.
    pending: Vec<Member>,
    window_due: Option<u64>,
    /// The morning digest, sent with the next flush.
    digest: Option<Notice>,
}

fn key(rule: &str, entity: &str) -> String {
    format!("{rule}\u{1f}{entity}")
}

impl Gate {
    /// One outcome per event, in order. What's sent waits in the group window (`flush`).
    pub fn decide(&mut self, events: &[GateEvent], s: &GateSettings, now: u64) -> Vec<GateOutcome> {
        let quiet = s.quiet.as_ref().filter(|q| q.active(now));
        events.iter().map(|e| self.decide_one(e, s, quiet, now)).collect()
    }

    fn decide_one(&mut self, e: &GateEvent, s: &GateSettings, quiet: Option<&Quiet>, now: u64) -> GateOutcome {
        let crit = e.severity == Severity::Crit;
        let k = key(&e.rule, &e.entity);
        let st = self.keys.entry(k).or_default().clone();
        let incident = self.incidents.get(&e.incident).cloned().unwrap_or_default();
        let back_online = crate::health::BACK_ONLINE_RULES.contains(&e.rule.as_str());
        // A node gone again within 10 minutes of a return that was told, at no more than what was
        // paged: coming and going (G16b).
        let node_flap = e.step == Step::Raised
            && back_online
            && !st.firing
            && st.back_ms.is_some_and(|b| now.saturating_sub(b) < FLAP_MS)
            && st.paged_severity.is_some_and(|p| e.severity <= p);
        let back_ago = fmt_span(now.saturating_sub(st.back_ms.unwrap_or(now)));
        let (outcome, reason, why): (Outcome, Option<String>, Option<Why>) = match e.step {
            Step::Raised => {
                if let Some(r) = &e.silenced {
                    (Outcome::Silenced, Some(r.clone()), None)
                } else if let Some(r) = &e.planned {
                    (Outcome::Planned, Some(r.clone()), None)
                } else if node_flap && st.flaps >= 2 {
                    // Paged as flapping already: neither its going away nor its return is, unless
                    // it stays away 10 minutes (`tick`).
                    (Outcome::Flapping, Some(format!("Away again {back_ago} after it came back; paged as flapping")), None)
                } else if st.resolve_due_ms.is_some_and(|d| now < d) && st.paged_severity.is_some_and(|p| e.severity <= p) {
                    // Back before its clear went out: the page stands, and the clear is never sent.
                    let cleared = st.resolve_due_ms.unwrap_or(now).saturating_sub(FLAP_MS);
                    (Outcome::Flapping, Some(format!("Back {} after it cleared; paged once", fmt_span(now.saturating_sub(cleared)))), None)
                } else if st.sent_step == Some(Step::Raised)
                    && st.sent_severity == Some(e.severity)
                    && !st.resolved_since_sent
                    && st.sent_at_ms.is_some_and(|t| now.saturating_sub(t) < DUPLICATE_MS)
                {
                    let ago = now.saturating_sub(st.sent_at_ms.unwrap_or(now));
                    (Outcome::Duplicate, Some(format!("Already sent {} earlier", fmt_span(ago))), None)
                } else if !crit && s.critical_only {
                    (Outcome::Held, Some("Critical alerts only".into()), None)
                } else if let (false, Some(q)) = (crit, quiet) {
                    (Outcome::Held, Some(format!("Quiet hours, {}", q.label())), None)
                } else if !crit && incident.paged_severity.is_some() && incident.paged_at_ms.is_some_and(|t| now.saturating_sub(t) < COALESCE_MS) {
                    let ago = now.saturating_sub(incident.paged_at_ms.unwrap_or(now));
                    (Outcome::Coalesced, Some(format!("Part of “{}”, paged {} earlier", incident.paged_title, fmt_span(ago))), None)
                } else {
                    (Outcome::Sent, None, Some(Why::Raised))
                }
            }
            Step::Escalated => {
                let repeat_ms = s.repeat_minutes as u64 * 60_000;
                if let Some(r) = &e.silenced {
                    (Outcome::Silenced, Some(r.clone()), None)
                } else if let Some(r) = &e.planned {
                    (Outcome::Planned, Some(r.clone()), None)
                } else if st.paged_severity.is_some_and(|p| e.severity <= p) && st.last_page_ms.is_some_and(|t| repeat_ms == 0 || now.saturating_sub(t) < repeat_ms) {
                    // It eased and climbed back: what was paged still stands.
                    let ago = now.saturating_sub(st.last_page_ms.unwrap_or(now));
                    (Outcome::Flapping, Some(format!("Back to critical; paged {} earlier", fmt_span(ago))), None)
                } else {
                    (Outcome::Sent, None, Some(Why::Escalated))
                }
            }
            Step::Deescalated => (Outcome::Eased, Some("Went from critical to warning".into()), None),
            Step::Resolved => {
                if !st.paged_since_raise {
                    (Outcome::NotPaged, Some("It was never paged".into()), None)
                } else if !s.resolved {
                    (Outcome::NotPaged, Some("“When an alert clears” is off".into()), None)
                } else if let (Some(q), false) = (quiet, st.paged_severity == Some(Severity::Crit)) {
                    (Outcome::Held, Some(format!("Quiet hours, {}", q.label())), None)
                } else if back_online && st.away_ms.is_some() {
                    // A flapping node's going away wasn't paged: nor is its return.
                    (Outcome::NotPaged, Some("Flapping: its going away wasn't paged either".into()), None)
                } else if back_online {
                    // "cell3 is back online" is never damped: held, it would say a node is down that's up.
                    (Outcome::Sent, None, Some(Why::Resolved))
                } else {
                    // Sent by `tick` once it has stayed clear for 10 minutes (flap damping).
                    (Outcome::Held, Some(format!("Sent if it stays clear for {}", fmt_span(FLAP_MS))), None)
                }
            }
        };
        let wait_resolve = e.step == Step::Resolved && outcome == Outcome::Held && reason.as_deref().is_some_and(|r| r.starts_with("Sent if"));
        // The second time a node is gone again soon after it came back, the page says it's flapping.
        let (title, body) = if node_flap && st.flaps == 1 {
            (format!("{} · flapping", e.title), format!("{}\nGone again {back_ago} after it came back: its next returns and drops aren't paged unless it stays away {}.", e.body, fmt_span(FLAP_MS)))
        } else {
            (e.title.clone(), e.body.clone())
        };

        let st = self.keys.get_mut(&key(&e.rule, &e.entity)).expect("inserted above");
        match e.step {
            Step::Raised => {
                // A raise while it's still firing (a restart) or flapping keeps what was paged; a new one starts over.
                if node_flap && !matches!(outcome, Outcome::Silenced | Outcome::Planned) {
                    st.flaps += 1;
                    st.flapped = true;
                    st.away_ms = (outcome == Outcome::Flapping).then_some(now);
                } else if outcome == Outcome::Flapping {
                    st.flapped = true;
                } else if !st.firing {
                    st.paged_since_raise = false;
                    st.paged_severity = None;
                    st.acked = false;
                    st.flapped = false;
                    (st.back_ms, st.flaps, st.away_ms) = (None, 0, None);
                }
                // A clear still waiting is never sent now: flapping, or (raised after it was due) a new page.
                st.resolve = None;
                st.resolve_due_ms = None;
                st.firing = true;
            }
            Step::Escalated => {
                st.firing = true;
                st.acked = false;
            }
            Step::Deescalated => {}
            Step::Resolved => {
                st.firing = false;
                st.resolved_since_sent = true;
                st.acked = false;
                if back_online && (why.is_some() || st.away_ms.is_some()) {
                    // Its return, told or (flapping) not: a raise within 10 minutes is coming and going.
                    (st.back_ms, st.away_ms) = (Some(now), None);
                }
                if wait_resolve {
                    // The clear quotes what was paged: "Resolved: cell1 GPU 95 °C".
                    let title = match e.title.strip_prefix("Resolved: ") {
                        Some(_) if !st.title.is_empty() => format!("Resolved: {}", st.title),
                        _ => e.title.clone(),
                    };
                    st.resolve = Some(Member {
                        why: Why::Resolved,
                        rule: e.rule.clone(),
                        entity: e.entity.clone(),
                        severity: e.severity,
                        incident: e.incident.clone(),
                        title,
                        body: e.body.clone(),
                        scope: e.scope.clone(),
                        alert_id: e.alert_id.clone(),
                        open: e.open.clone(),
                    });
                    st.resolve_due_ms = Some(if e.clear_held { now } else { now + FLAP_MS });
                }
            }
        }
        st.held_quiet = match (e.step, outcome, &reason) {
            (Step::Raised, Outcome::Held, Some(r)) if r.starts_with("Quiet hours") => Some(e.title.clone()),
            (Step::Raised | Step::Escalated | Step::Deescalated, _, _) if why.is_none() => st.held_quiet.take(),
            _ => None,
        };
        if e.step != Step::Resolved {
            st.severity = Some(e.severity);
            st.silenced = e.silenced.is_some();
            st.planned = e.planned.is_some();
            st.incident = e.incident.clone();
            st.scope = e.scope.clone();
            st.alert_id = e.alert_id.clone();
        }
        if let Some(why) = why {
            st.sent_severity = Some(e.severity);
            st.sent_step = Some(e.step);
            st.sent_at_ms = Some(now);
            if why != Why::Resolved {
                st.paged_since_raise = true;
                st.paged_severity = st.paged_severity.max(Some(e.severity));
                st.last_page_ms = Some(now);
                st.resolved_since_sent = false;
                st.title = e.title.clone();
                st.body = e.body.clone();
                let inc = self.incidents.entry(e.incident.clone()).or_default();
                inc.paged_severity = inc.paged_severity.max(Some(e.severity));
                inc.paged_at_ms = Some(now);
                inc.paged_title = e.title.clone();
            }
            self.enqueue(Member {
                why,
                rule: e.rule.clone(),
                entity: e.entity.clone(),
                severity: e.severity,
                incident: e.incident.clone(),
                title,
                body,
                scope: e.scope.clone(),
                alert_id: e.alert_id.clone(),
                open: e.open.clone(),
            }, now);
        }
        if outcome == Outcome::Coalesced {
            self.incidents.entry(e.incident.clone()).or_default().also.push(e.title.clone());
        }
        GateOutcome {
            rule: e.rule.clone(),
            entity: e.entity.clone(),
            outcome,
            reason,
            title: e.title.clone(),
            severity: e.severity,
            alert_id: e.alert_id.clone(),
            incident: e.incident.clone(),
        }
    }

    /// The resolves that have stayed clear for 10 minutes (in quiet hours only those of a paged
    /// critical; the others are dropped, as a quiet resolve always was). The ones that come due
    /// together go out as one message per incident (`flush`).
    fn due_resolves(&mut self, s: &GateSettings, now: u64) -> Vec<GateOutcome> {
        let quiet = s.quiet.as_ref().is_some_and(|q| q.active(now));
        let mut due = Vec::new();
        for st in self.keys.values_mut() {
            if st.firing || !st.resolve_due_ms.is_some_and(|d| now >= d) {
                continue;
            }
            let Some(m) = st.resolve.take() else { continue };
            st.resolve_due_ms = None;
            st.flapped = false;
            if quiet && st.paged_severity != Some(Severity::Crit) {
                continue;
            }
            st.sent_severity = Some(m.severity);
            st.sent_step = Some(Step::Resolved);
            st.sent_at_ms = Some(now);
            due.push(m);
        }
        let out = due
            .iter()
            .map(|m| GateOutcome {
                rule: m.rule.clone(),
                entity: m.entity.clone(),
                outcome: Outcome::Sent,
                reason: Some(format!("Clear for {}", fmt_span(FLAP_MS))),
                title: m.title.clone(),
                severity: m.severity,
                alert_id: m.alert_id.clone(),
                incident: m.incident.clone(),
            })
            .collect();
        for m in due {
            self.enqueue(m, now);
        }
        out
    }

    /// A flapping node whose going away wasn't paged and that is still away 10 minutes later:
    /// "Still flapping: cell3 offline", and its return will be told (G16b). A critical always; a
    /// warning not in quiet hours or with critical-only on.
    fn lasting_aways(&mut self, s: &GateSettings, now: u64) -> Vec<GateOutcome> {
        let quiet = s.quiet.as_ref().is_some_and(|q| q.active(now));
        let mut due = Vec::new();
        for (k, st) in self.keys.iter_mut() {
            if !st.firing || st.acked || st.silenced || st.planned || !st.away_ms.is_some_and(|t| now.saturating_sub(t) >= FLAP_MS) {
                continue;
            }
            let severity = st.severity.unwrap_or(Severity::Crit);
            if severity != Severity::Crit && (quiet || s.critical_only) {
                continue;
            }
            st.away_ms = None;
            st.last_page_ms = Some(now);
            let (rule, entity) = k.split_once('\u{1f}').unwrap_or((k.as_str(), ""));
            due.push(Member {
                why: Why::Repeat,
                rule: rule.to_string(),
                entity: entity.to_string(),
                severity,
                incident: st.incident.clone(),
                title: format!("Still flapping: {}", st.title),
                body: st.body.clone(),
                scope: st.scope.clone(),
                alert_id: st.alert_id.clone(),
                open: Some(("alerts".into(), json!({}))),
            });
        }
        let out = due
            .iter()
            .map(|m| GateOutcome {
                rule: m.rule.clone(),
                entity: m.entity.clone(),
                outcome: Outcome::Sent,
                reason: Some(format!("Still away after {}", fmt_span(FLAP_MS))),
                title: m.title.clone(),
                severity: m.severity,
                alert_id: m.alert_id.clone(),
                incident: m.incident.clone(),
            })
            .collect();
        for m in due {
            self.enqueue(m, now);
        }
        out
    }

    /// Another sender (a node's agent) paged `incident`: here it counts as paged, so a warning
    /// in it within 30 minutes is coalesced and listed in this side's next message about it (G17).
    pub fn paged_elsewhere(&mut self, incident: &str, severity: Severity, title: &str, now: u64) {
        let inc = self.incidents.entry(incident.to_string()).or_default();
        inc.paged_severity = inc.paged_severity.max(Some(severity));
        inc.paged_at_ms = Some(now);
        inc.paged_title = title.to_string();
    }

    fn enqueue(&mut self, m: Member, now: u64) {
        self.pending.push(m);
        self.window_due.get_or_insert(now + GROUP_WAIT_MS);
    }

    /// No more repeats for this alert.
    pub fn ack(&mut self, rule: &str, entity: &str) {
        if let Some(st) = self.keys.get_mut(&key(rule, entity)) {
            st.acked = true;
        }
    }

    /// Brings the gate in line with what's firing now (acknowledged, silenced, planned), so
    /// repeats follow it; keys that stopped firing without a resolve stop repeating.
    pub fn sync(&mut self, firing: &[(String, String, bool, bool, bool)]) {
        let now: HashMap<String, (bool, bool, bool)> = firing.iter().map(|(r, e, a, s, p)| (key(r, e), (*a, *s, *p))).collect();
        for (k, st) in self.keys.iter_mut() {
            match now.get(k) {
                Some((acked, silenced, planned)) => {
                    st.acked = *acked;
                    st.silenced = *silenced;
                    st.planned = *planned;
                }
                None => st.firing = false,
            }
        }
    }

    /// "Still firing" for every unacknowledged critical that was paged, isn't silenced or
    /// planned, and hasn't been paged for `repeat_minutes`.
    pub fn tick(&mut self, s: &GateSettings, now: u64) -> Vec<GateOutcome> {
        let mut out = self.digest_if_quiet_ended(s, now);
        out.extend(self.due_resolves(s, now));
        out.extend(self.lasting_aways(s, now));
        if s.repeat_minutes == 0 {
            return out;
        }
        let every = s.repeat_minutes as u64 * 60_000;
        let mut due = Vec::new();
        for (k, st) in self.keys.iter_mut() {
            let wants = st.firing && st.severity == Some(Severity::Crit) && st.paged_since_raise && !st.acked && !st.silenced && !st.planned;
            if wants && st.last_page_ms.is_some_and(|t| now.saturating_sub(t) >= every) {
                st.last_page_ms = Some(now);
                st.away_ms = None; // this says it's away, so its return is told
                let (rule, entity) = k.split_once('\u{1f}').unwrap_or((k.as_str(), ""));
                due.push(Member {
                    why: Why::Repeat,
                    rule: rule.to_string(),
                    entity: entity.to_string(),
                    severity: Severity::Crit,
                    incident: st.incident.clone(),
                    title: format!("{}: {}", if st.flapped { "Still flapping" } else { "Still firing" }, st.title),
                    body: st.body.clone(),
                    scope: st.scope.clone(),
                    alert_id: st.alert_id.clone(),
                    open: Some(("alerts".into(), json!({}))),
                });
            }
        }
        // Forget what stopped firing a day ago.
        self.keys.retain(|_, st| st.firing || st.resolve.is_some() || st.sent_at_ms.is_some_and(|t| now.saturating_sub(t) < 86_400_000));
        self.incidents.retain(|_, i| i.paged_at_ms.is_some_and(|t| now.saturating_sub(t) < 86_400_000));
        out.extend(due.iter().map(|m| GateOutcome {
                rule: m.rule.clone(),
                entity: m.entity.clone(),
                outcome: Outcome::Sent,
                reason: Some("Still firing".into()),
                title: m.title.clone(),
                severity: m.severity,
                alert_id: m.alert_id.clone(),
                incident: m.incident.clone(),
            }));
        for m in due {
            self.enqueue(m, now);
        }
        out
    }

    /// When quiet hours end: one message naming the warnings held during them that are still
    /// firing ("While quiet: 2 warnings still firing"). They count as paged from then on.
    fn digest_if_quiet_ended(&mut self, s: &GateSettings, now: u64) -> Vec<GateOutcome> {
        let quiet = s.quiet.as_ref().is_some_and(|q| q.active(now));
        let ended = self.quiet_was && !quiet;
        self.quiet_was = quiet;
        if !ended || s.critical_only {
            return Vec::new();
        }
        let mut members = Vec::new();
        for (k, st) in self.keys.iter_mut() {
            let Some(title) = st.held_quiet.take() else { continue };
            if !st.firing || st.silenced || st.planned || st.severity != Some(Severity::Warn) {
                continue;
            }
            st.paged_since_raise = true;
            st.paged_severity = st.paged_severity.max(Some(Severity::Warn));
            st.last_page_ms = Some(now);
            st.title = title.clone();
            let (rule, entity) = k.split_once('\u{1f}').unwrap_or((k.as_str(), ""));
            members.push(Member {
                why: Why::Digest,
                rule: rule.to_string(),
                entity: entity.to_string(),
                severity: Severity::Warn,
                incident: st.incident.clone(),
                title,
                body: st.body.clone(),
                scope: st.scope.clone(),
                alert_id: st.alert_id.clone(),
                open: Some(("alerts".into(), json!({}))),
            });
        }
        if members.is_empty() {
            return Vec::new();
        }
        let mut lines: Vec<String> = members.iter().map(|m| m.title.clone()).collect();
        if lines.len() > GROUP_LINES {
            let more = lines.len() - GROUP_LINES;
            lines.truncate(GROUP_LINES);
            lines.push(format!("and {more} more"));
        }
        let out = members
            .iter()
            .map(|m| GateOutcome {
                rule: m.rule.clone(),
                entity: m.entity.clone(),
                outcome: Outcome::Sent,
                reason: Some("Quiet hours ended: still firing".into()),
                title: m.title.clone(),
                severity: Severity::Warn,
                alert_id: m.alert_id.clone(),
                incident: m.incident.clone(),
            })
            .collect();
        self.digest = Some(Notice {
            message_id: message_id(),
            title: format!("While quiet: {} still firing", plural(members.len(), "warning", "warnings")),
            body: lines.join("\n"),
            lines,
            severity: Severity::Warn,
            resolved: false,
            node: None,
            rule: None,
            alert_id: None,
            open: Some(("alerts".into(), json!({}))),
            members,
            also: Vec::new(),
        });
        self.window_due.get_or_insert(now);
        out
    }

    /// The messages whose group window has closed by `now`.
    pub fn flush(&mut self, now: u64) -> Vec<Notice> {
        if !self.window_due.is_some_and(|d| now >= d) {
            return Vec::new();
        }
        self.window_due = None;
        let groups = group_by_incident(std::mem::take(&mut self.pending));
        let also: Vec<Vec<String>> = groups.iter().map(|g| self.incidents.get_mut(&g[0].incident).map(|i| std::mem::take(&mut i.also)).unwrap_or_default()).collect();
        let mut out = if groups.is_empty() { Vec::new() } else { compose_many(groups, also) };
        out.extend(self.digest.take());
        out
    }
}

/// Members by incident, in the order each incident first appears.
fn group_by_incident(items: Vec<Member>) -> Vec<Vec<Member>> {
    let mut groups: Vec<Vec<Member>> = Vec::new();
    for m in items {
        match groups.iter_mut().find(|g| g[0].incident == m.incident) {
            Some(g) => g.push(m),
            None => groups.push(vec![m]),
        }
    }
    groups
}

/// One message per incident, or one grouped message when there are more than two.
fn compose_many(groups: Vec<Vec<Member>>, also: Vec<Vec<String>>) -> Vec<Notice> {
    let notices: Vec<Notice> = groups.into_iter().zip(also.into_iter().chain(std::iter::repeat_with(Vec::new))).map(|(g, a)| compose(g, a)).collect();
    if notices.len() > GROUP_OVER {
        vec![grouped(notices)]
    } else {
        notices
    }
}

fn compose_all(groups: Vec<Vec<Member>>, also: Vec<Vec<String>>) -> Notice {
    let mut n = compose_many(groups, also);
    if n.len() == 1 {
        n.remove(0)
    } else {
        grouped(n)
    }
}

/// One incident's message, titled by its worst alert (the first critical one still firing).
fn compose(items: Vec<Member>, also: Vec<String>) -> Notice {
    let head = items
        .iter()
        .position(|m| m.severity == Severity::Crit && m.why != Why::Resolved)
        .or_else(|| items.iter().position(|m| m.severity == Severity::Crit))
        .unwrap_or(0);
    let h = &items[head];
    let mut lines: Vec<String> = items.iter().enumerate().filter(|(i, _)| *i != head).map(|(_, m)| m.title.clone()).collect();
    lines.extend(also.iter().map(|t| format!("Also: {t}")));
    let body = std::iter::once(h.body.clone()).chain(lines.iter().cloned()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    Notice {
        message_id: message_id(),
        title: h.title.clone(),
        body,
        severity: items.iter().map(|m| m.severity).max().unwrap_or(Severity::Warn),
        resolved: items.iter().all(|m| m.why == Why::Resolved),
        node: h.scope.node.clone(),
        rule: Some(h.rule.clone()),
        alert_id: h.alert_id.clone(),
        open: h.open.clone(),
        lines,
        also,
        members: items,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "4 alerts, 2 critical", naming every critical first, then warnings, then what cleared.
fn grouped(notices: Vec<Notice>) -> Notice {
    let members: Vec<Member> = notices.iter().flat_map(|n| n.members.clone()).collect();
    let also: Vec<String> = notices.iter().flat_map(|n| n.also.clone()).collect();
    let firing: Vec<&Member> = members.iter().filter(|m| m.why != Why::Resolved).collect();
    let crit = firing.iter().filter(|m| m.severity == Severity::Crit).count();
    let cleared = members.len() - firing.len();
    let mut title = if firing.is_empty() { format!("{} cleared", plural(cleared, "alert", "alerts")) } else { plural(firing.len(), "alert", "alerts") };
    if !firing.is_empty() && crit > 0 {
        title.push_str(&format!(", {crit} critical"));
    }
    if !firing.is_empty() && cleared > 0 {
        title.push_str(&format!(", {cleared} cleared"));
    }
    let mut lines: Vec<String> = firing.iter().filter(|m| m.severity == Severity::Crit).map(|m| m.title.clone()).collect();
    lines.extend(firing.iter().filter(|m| m.severity != Severity::Crit).map(|m| m.title.clone()));
    lines.extend(members.iter().filter(|m| m.why == Why::Resolved).map(|m| m.title.clone()));
    lines.extend(also.iter().map(|t| format!("Also: {t}")));
    if lines.len() > GROUP_LINES {
        let more = lines.len() - GROUP_LINES;
        lines.truncate(GROUP_LINES);
        lines.push(format!("and {more} more"));
    }
    Notice {
        message_id: message_id(),
        title,
        body: lines.join("\n"),
        severity: members.iter().map(|m| m.severity).max().unwrap_or(Severity::Warn),
        resolved: firing.is_empty(),
        node: None,
        rule: None,
        alert_id: None,
        open: Some(("alerts".into(), json!({}))),
        lines,
        also: Vec::new(),
        members,
    }
}

// ---- From the health engine's events to the gate's

/// A field of an alert that may not exist yet in this build (incidents, silences and
/// maintenance come from health/), read from its JSON so this module needn't change.
fn alert_str(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from)
}

enum Conv {
    Event(GateEvent),
    Ack(String, String),
    Skip(GateOutcome),
    Ignore,
}

fn category_word(c: Category) -> &'static str {
    match c {
        Category::Node => "Node status",
        Category::Disk => "Disk",
        Category::Gpu => "GPU",
        Category::Fabric => "Fabric",
        Category::Containers => "Containers",
    }
}

/// `silences` are the reasons of the silences in effect, by id.
fn convert(e: &AlertEvent, s: &NotifySettings, silences: &HashMap<String, String>, now: u64) -> Conv {
    let a = &e.alert;
    if !a.notify || a.severity == Severity::Info {
        return Conv::Ignore;
    }
    let step = match e.kind {
        EventKind::Raised => Step::Raised,
        EventKind::Escalated => Step::Escalated,
        EventKind::Eased => Step::Deescalated,
        EventKind::Resolved => Step::Resolved,
        EventKind::Acked => return Conv::Ack(a.rule.clone(), a.id.clone()),
        _ => return Conv::Ignore,
    };
    let v = serde_json::to_value(a).unwrap_or_default();
    let node_level = a.entity.kind == "node";
    let incident = alert_str(&v, "incidentId")
        .or_else(|| alert_str(&v, "rootKey"))
        .unwrap_or_else(|| match (&a.entity.node, node_level) {
            (Some(n), true) => format!("node:{n}"),
            _ => a.id.clone(),
        });
    let (title, mut body) = match (step, a.rule.as_str()) {
        // A new boot_id across the outage: "cell3 is back · it rebooted".
        (Step::Resolved, "node-offline") if a.reason.as_deref() == Some(crate::health::REBOOTED) => {
            (format!("{} is back · it rebooted", a.entity.label), "SparkBox is receiving its readings again. It came back with a new boot ID: it restarted while it was away.".to_string())
        }
        (Step::Resolved, "node-offline") => (format!("{} is back online", a.entity.label), "SparkBox is receiving its readings again.".to_string()),
        (Step::Resolved, _) => (format!("Resolved: {}", a.summary), format!("{} — {}", a.entity.label, a.message)),
        _ => (a.summary.clone(), format!("{} — {}", a.entity.label, a.message)),
    };
    // "… · Flapping · 4 times in 30 min": a page about an alert that keeps coming back says so.
    if let Some(f) = a.flapping.as_ref().filter(|_| step != Step::Resolved) {
        body = format!("{body} · {}", f.words(now));
    }
    let open = match (a.entity.kind.as_str(), &a.entity.node) {
        ("node", Some(n)) if step == Step::Resolved => Some(("node".to_string(), json!({ "id": n }))),
        _ => Some(("alerts".to_string(), json!({}))),
    };
    let silenced = alert_str(&v, "silenceId")
        .map(|id| silences.get(&id).cloned().unwrap_or_else(|| "Silenced".to_string()))
        .or_else(|| a.muted(now).then(|| format!("Muted until {}", clock_in(&s.quiet_tz, a.muted_until_ms.unwrap_or(now)))));
    let ev = GateEvent {
        step,
        rule: a.rule.clone(),
        entity: a.id.clone(),
        severity: a.severity,
        incident,
        title,
        body,
        scope: Scope::of(a),
        alert_id: Some(a.id.clone()),
        open,
        silenced,
        planned: alert_str(&v, "planned"),
        clear_held: step == Step::Resolved && a.flapping.is_some() && !crate::health::BACK_ONLINE_RULES.contains(&a.rule.as_str()),
    };
    if !s.category(a.category) {
        return Conv::Skip(GateOutcome {
            rule: ev.rule,
            entity: ev.entity,
            outcome: Outcome::NotPaged,
            reason: Some(format!("{} notifications are off", category_word(a.category))),
            title: ev.title,
            severity: ev.severity,
            alert_id: ev.alert_id,
            incident: ev.incident,
        });
    }
    Conv::Event(ev)
}

/// The health engine's events through `gate` as on_alerts runs them, then the group window
/// closed: what was decided and what would be sent (tests in health/ drive the real gate).
#[cfg(test)]
pub(crate) fn decide_events(gate: &mut Gate, events: &[AlertEvent], s: &NotifySettings, now: u64) -> (Vec<GateOutcome>, Vec<Notice>) {
    let mut outcomes = Vec::new();
    let mut gate_events = Vec::new();
    for e in events {
        match convert(e, s, &HashMap::new(), now) {
            Conv::Event(g) => gate_events.push(g),
            Conv::Ack(rule, entity) => gate.ack(&rule, &entity),
            Conv::Skip(o) => outcomes.push(o),
            Conv::Ignore => {}
        }
    }
    outcomes.extend(gate.decide(&gate_events, &s.gate(), now));
    (outcomes, gate.flush(now + GROUP_WAIT_MS))
}

/// What Notification Center's pending list keeps: title, window and its parameters.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Target {
    pub title: String,
    pub kind: String,
    pub params: Value,
}

/// Notifications shown and not yet followed: by id for clicks, and the latest one for when
/// SparkBox comes to the front without a click. Pure, so the logic is testable.
#[derive(Default)]
pub struct Pending {
    by_id: HashMap<i32, (u64, Target)>,
    latest: Option<(u64, i32)>,
}

impl Pending {
    /// Remembers a notification for a click; `offer` (SparkBox wasn't in front when it was
    /// shown) also makes it the one offered when SparkBox comes to the front.
    pub fn note(&mut self, id: i32, target: Target, now: u64, offer: bool) {
        self.by_id.insert(id, (now, target));
        self.by_id.retain(|_, (at, _)| now.saturating_sub(*at) < PENDING_MS);
        if offer {
            self.latest = Some((now, id));
        }
    }

    /// The target of a clicked notification; once followed it isn't offered again.
    pub fn clicked(&mut self, id: i32) -> Option<Target> {
        let (_, t) = self.by_id.remove(&id)?;
        if self.latest.is_some_and(|(_, l)| l == id) {
            self.latest = None;
        }
        Some(t)
    }

    /// The latest notification, once, if SparkBox comes to the front within 10 minutes of it.
    pub fn take(&mut self, now: u64) -> Option<Target> {
        let (at, id) = self.latest.take()?;
        let (_, t) = self.by_id.remove(&id)?;
        (now.saturating_sub(at) < PENDING_MS).then_some(t)
    }
}

/// The managed notifier.
pub struct Notifier {
    path: Option<PathBuf>,
    settings: Mutex<NotifySettings>,
    /// [Live, Demo]: what Demo paged never shapes what Live pages.
    gates: [Mutex<Gate>; 2],
    gate_paths: [Option<PathBuf>; 2],
    asked: AtomicBool,
    pending: Mutex<Pending>,
    next_id: AtomicI32,
    pub deliveries: delivery::Log,
    endpoints: delivery::Endpoints,
    /// When each channel's Test last sent.
    tested: Mutex<HashMap<String, u64>>,
}

impl Notifier {
    /// `path` is notify.json; `log` is deliveries.jsonl. None keeps them in memory (tests).
    #[cfg(test)]
    pub fn new(path: Option<PathBuf>) -> Arc<Notifier> {
        Notifier::with_log(path, None)
    }

    /// The gate's state goes next to the delivery log (gate.json, gate-demo.json).
    pub fn with_log(path: Option<PathBuf>, log: Option<PathBuf>) -> Arc<Notifier> {
        let mut settings = path
            .as_ref()
            .and_then(|p| crate::util::load_json_or_quarantine::<NotifySettings>(p).ok().flatten())
            .unwrap_or_default();
        if migrate_urls(&mut settings, |name, v| crate::secrets::set(name, v)) {
            if let Some(p) = &path {
                let _ = crate::util::write_atomic(p, &serde_json::to_vec_pretty(&settings).unwrap_or_default());
            }
        }
        let dir = log.as_ref().and_then(|l| l.parent().map(|d| d.to_path_buf()));
        let gate_paths = [dir.as_ref().map(|d| d.join("gate.json")), dir.as_ref().map(|d| d.join("gate-demo.json"))];
        let load = |p: &Option<PathBuf>| p.as_deref().map(crate::util::load_json_or_default::<Gate>).unwrap_or_default();
        Arc::new(Notifier {
            path,
            settings: Mutex::new(settings),
            gates: [Mutex::new(load(&gate_paths[0])), Mutex::new(load(&gate_paths[1]))],
            gate_paths,
            asked: AtomicBool::new(false),
            pending: Mutex::new(Pending::default()),
            next_id: AtomicI32::new(1000 + fastrand::i32(0..1000)),
            deliveries: delivery::Log::open(log),
            endpoints: delivery::Endpoints::default(),
            tested: Mutex::new(HashMap::new()),
        })
    }

    pub fn settings(&self) -> NotifySettings {
        self.settings.lock().unwrap().clone()
    }

    pub fn save(&self, s: NotifySettings) -> Result<NotifySettings, String> {
        for t in [&s.quiet_start, &s.quiet_end] {
            if minutes(t).is_none() {
                return Err(format!("\"{t}\" isn't a time. Use 24-hour HH:MM, like 22:00."));
            }
        }
        if !s.quiet_tz.is_empty() && !valid_zone(&s.quiet_tz) {
            return Err(format!("\"{}\" isn't a time zone SparkBox knows.", s.quiet_tz));
        }
        if !REPEAT_CHOICES.contains(&s.repeat_minutes) {
            return Err("Repeat critical alerts every 15 min, 30 min, 1 h or 4 h, or never.".into());
        }
        if let Some(path) = &self.path {
            let bytes = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
            crate::util::write_atomic(path, &bytes)?;
        }
        *self.settings.lock().unwrap() = s.clone();
        Ok(s)
    }

    fn gate(&self, demo: bool) -> std::sync::MutexGuard<'_, Gate> {
        self.gates[demo as usize].lock().unwrap()
    }

    fn persist(&self, demo: bool, gate: &Gate) {
        if let Some(p) = &self.gate_paths[demo as usize] {
            let _ = crate::util::write_atomic(p, &serde_json::to_vec(gate).unwrap_or_default());
        }
    }

    fn quiet_now(&self) -> bool {
        self.settings().quiet().is_some_and(|q| q.active(crate::history::now_ms()))
    }

    /// Allows one Test per channel every 10 s.
    fn may_test(&self, id: &str, now: u64) -> bool {
        let mut t = self.tested.lock().unwrap();
        if t.get(id).is_some_and(|at| now.saturating_sub(*at) < TEST_EVERY_MS) {
            return false;
        }
        t.insert(id.to_string(), now);
        true
    }
}

/// A notify.json from before round 4 kept webhook and dead-man's-switch URLs in the clear: each
/// moves to the Keychain (`set`), leaving only its host. Not in look-only mode, which writes
/// nothing on this Mac. Returns whether anything moved (the settings are then saved).
pub fn migrate_urls(s: &mut NotifySettings, set: impl Fn(&str, &str) -> Result<(), String>) -> bool {
    if crate::admin::look_only() {
        return false;
    }
    let mut moved = false;
    for c in s.channels.iter_mut().filter(|c| c.kind.secret_fields().contains(&"url")) {
        let Some(u) = c.url.clone().filter(|u| !u.trim().is_empty()) else { continue };
        match set(&secret_name(&c.id, "url"), u.trim()) {
            Ok(()) => {
                c.url_host = url::Url::parse(u.trim()).ok().and_then(|x| x.host_str().map(String::from));
                c.url = None;
                if !c.saved.iter().any(|f| f == "url") {
                    c.saved.push("url".into());
                }
                moved = true;
            }
            Err(e) => eprintln!("notify: couldn't move {}'s URL to the Keychain: {e}", c.name),
        }
    }
    moved
}

/// A channel's Keychain field.
pub fn secret_name(channel: &str, field: &str) -> String {
    format!("notify.{channel}.{field}")
}

fn secret(channel: &str, field: &str) -> Option<String> {
    crate::secrets::get(&secret_name(channel, field)).ok().flatten().filter(|s| !s.is_empty())
}

/// Shows one notification, asking for permission the first time, and remembers what it
/// points at so a click (or coming back to SparkBox) can open it.
fn show(app: &AppHandle, n: &Notice) {
    if dev_muted() {
        return;
    }
    let Some(notifier) = app.try_state::<Arc<Notifier>>() else { return };
    let ux = app.notification();
    if !notifier.asked.swap(true, Ordering::SeqCst) && !matches!(ux.permission_state(), Ok(PermissionState::Granted)) {
        let _ = ux.request_permission();
    }
    let id = notifier.next_id.fetch_add(1, Ordering::SeqCst);
    if let Some((kind, params)) = &n.open {
        let target = Target { title: n.title.clone(), kind: kind.clone(), params: params.clone() };
        let focused = app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false);
        notifier.pending.lock().unwrap().note(id, target, crate::history::now_ms(), !focused);
    }
    if let Err(e) = ux.builder().id(id).title(&n.title).body(&n.body).show() {
        eprintln!("notification wasn't shown: {e}");
    }
}

/// Follows clicks on SparkBox's notifications: brings the window forward and opens what the
/// notification was about. Called once at start.
pub fn install_click_handler(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.notification().on_action(move |performed| {
        if performed.action_id() != "tap" {
            return;
        }
        let Some(id) = performed.notification().map(|n| n.id()) else { return };
        let Some(notifier) = handle.try_state::<Arc<Notifier>>() else { return };
        let Some(target) = notifier.pending.lock().unwrap().clicked(id) else { return };
        crate::tray::show_main(&handle, None);
        let _ = handle.emit(CLICKED_EVENT, &target);
    });
}

/// Posts a notification about a task or anything else outside the alert rules (the
/// Tasks switch in Settings › Notifications applies). `open` is the window to show
/// when the user comes back, e.g. `Some(("tasks", json!({})))`. Long tasks use it (lib.rs).
pub fn post(app: &AppHandle, title: &str, body: &str, open: Option<(&str, Value)>) {
    let Some(notifier) = app.try_state::<Arc<Notifier>>() else { return };
    let s = notifier.settings();
    if !s.enabled || !s.tasks || notifier.quiet_now() {
        return;
    }
    show(app, &Notice::plain(title, body, open.map(|(k, p)| (k.to_string(), p))));
}

/// The message a notice becomes on a phone channel.
pub fn message(n: &Notice, now: u64) -> Message {
    Message {
        title: n.title.clone(),
        body: n.body.clone(),
        severity: n.severity,
        state: if n.resolved { "resolved".into() } else { "firing".into() },
        node: n.node.clone(),
        rule: n.rule.clone(),
        alert_id: n.alert_id.clone(),
        ts_ms: now,
    }
}

/// What a channel's Test button sends from this Mac.
pub fn test_message(ch: &Channel, now: u64) -> Message {
    Message {
        title: "SparkBox test".into(),
        body: format!("This is how SparkBox alerts reach you on {}.", ch.name),
        severity: Severity::Warn,
        state: "test".into(),
        node: None,
        rule: None,
        alert_id: None,
        ts_ms: now,
    }
}

/// The simulated phone channel of the Demo watch state: Demo's deliveries go to it as
/// "Demo — not sent". It is never saved in notify.json.
pub fn demo_channel() -> Channel {
    Channel { topic: Some("sparkbox-demo".into()), ..Channel::empty("demo-phone", Kind::Ntfy, "Phone (ntfy · Demo)") }
}

/// A log row for one alert of a message, on one channel (or none).
fn member_row(m: &Member, n: &Notice, ch: Option<&Channel>, outcome: Outcome, via: &str, demo: bool) -> Delivery {
    // Each alert's own words; one that went out in a grouped message says which.
    let body = if n.members.len() > 1 { format!("Sent as “{}”", n.title) } else { n.body.clone() };
    Delivery {
        body,
        via: via.into(),
        demo,
        message_id: Some(n.message_id.clone()),
        rule: Some(m.rule.clone()),
        alert_id: m.alert_id.clone(),
        incident: Some(m.incident.clone()),
        reason: match m.why {
            Why::Repeat => Some("Still firing".to_string()),
            Why::Digest => Some("Quiet hours ended: still firing".to_string()),
            _ => None,
        },
        ..Delivery::row(ch, &m.title, m.severity, outcome)
    }
}

fn outcome_row(o: &GateOutcome, demo: bool) -> Delivery {
    Delivery {
        demo,
        rule: Some(o.rule.clone()),
        alert_id: o.alert_id.clone(),
        incident: Some(o.incident.clone()),
        reason: o.reason.clone(),
        ..Delivery::row(None, &o.title, o.severity, o.outcome)
    }
}

/// How one message reaches one channel: what this Mac sends, and which alerts the nodes'
/// agents send themselves (with the node whose agent does).
#[derive(Debug, Default)]
pub struct ChannelPlan {
    pub mac: Option<Notice>,
    pub agents: Vec<(Member, String)>,
}

/// Per alert and channel: this Mac sends channel `ch` for alert `m` iff the channel is
/// "this Mac only" or no healthy, current agent sends it (agent::agent_sends). A channel
/// that takes critical alerts only gets the critical ones.
pub fn plan_channel(n: &Notice, ch: &Channel, ctx: &crate::agent::PagingCtx) -> ChannelPlan {
    let wanted = |m: &Member| !ch.critical_only || m.severity == Severity::Crit;
    let by_agent = |m: &Member| if ch.mac_only { None } else { crate::agent::agent_sends(&m.scope, ctx) };
    let agents = n.members.iter().filter(|m| wanted(m)).filter_map(|m| by_agent(m).map(|node| (m.clone(), node))).collect();
    ChannelPlan { mac: n.subset(|m| wanted(m) && by_agent(m).is_none()), agents }
}

/// Sends messages whose group window closed: Notification Center (when on), then each
/// phone channel from this Mac or, for what a node's agent pages, noted as the agent's.
fn dispatch(app: &AppHandle, notices: Vec<Notice>, demo: bool) {
    if notices.is_empty() {
        return;
    }
    let Some(notifier) = app.try_state::<Arc<Notifier>>() else { return };
    let s = notifier.settings();
    let ctx = crate::agent::paging_ctx(app);
    let mut channels: Vec<Channel> = s.paging_channels().cloned().collect();
    if demo {
        channels.push(demo_channel());
    }
    let now = crate::history::now_ms();
    if dev_muted() {
        // Decided like any other, recorded once per alert, and nothing leaves the process.
        for n in notices {
            let mac = Channel::empty("mac", Kind::Mac, "This Mac");
            let rows = n.members.iter().map(|m| Delivery { reason: Some(MUTED_REASON.into()), ..member_row(m, &n, Some(&mac), Outcome::Muted, "mac", demo) });
            notifier.deliveries.record_all(rows.collect());
        }
        return;
    }
    for n in notices {
        if s.enabled {
            show(app, &n);
        }
        if channels.is_empty() {
            let mac = Channel::empty("mac", Kind::Mac, "This Mac");
            let rows = n.members.iter().map(|m| {
                if s.enabled {
                    member_row(m, &n, Some(&mac), if demo { Outcome::Demo } else { Outcome::Sent }, "mac", demo)
                } else {
                    Delivery { reason: Some("No channel is on".into()), ..member_row(m, &n, None, Outcome::NotPaged, "mac", demo) }
                }
            });
            notifier.deliveries.record_all(rows.collect());
            continue;
        }
        for ch in &channels {
            let plan = plan_channel(&n, ch, &ctx);
            let agent_rows = plan.agents.iter().map(|(m, node)| member_row(m, &n, Some(ch), if demo { Outcome::Demo } else { Outcome::Agent }, &format!("agent:{node}"), demo));
            notifier.deliveries.record_all(agent_rows.collect());
            let Some(mine) = plan.mac else { continue };
            let rows: Vec<Delivery> = mine.members.iter().map(|m| member_row(m, &mine, Some(ch), if demo { Outcome::Demo } else { Outcome::Sent }, "mac", demo)).collect();
            if demo {
                notifier.deliveries.record_all(rows);
            } else {
                deliver(app, ch.clone(), message(&mine, now), rows, false);
            }
        }
    }
}

/// Sends one message on one channel from this Mac. A failure that may pass is retried
/// after 30 s, 2 min and 10 min, and for a critical alert every 30 min for a day; the
/// rows (one per alert in the message) are updated in place each time.
pub fn deliver(app: &AppHandle, ch: Channel, msg: Message, rows: Vec<Delivery>, test: bool) -> tauri::async_runtime::JoinHandle<Delivery> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let notifier = app.state::<Arc<Notifier>>().inner().clone();
        if dev_muted() {
            let now = crate::history::now_ms();
            let done: Vec<Delivery> = rows.iter().map(|r| Delivery { ts_ms: now, outcome: Outcome::Muted, reason: Some(MUTED_REASON.into()), ..r.clone() }).collect();
            notifier.deliveries.record_all(done.clone());
            return done.into_iter().next().unwrap_or_else(|| Delivery::row(Some(&ch), &msg.title, msg.severity, Outcome::Muted));
        }
        let http = delivery::client();
        let lookup = |f: &str| if ch.saved.iter().any(|x| x == f) { secret(&ch.id, f) } else { None };
        let critical = msg.severity == Severity::Crit && msg.state != "resolved" && !test;
        let mut attempt = 1u32;
        loop {
            let started = std::time::Instant::now();
            let result = delivery::send(&ch, &lookup, &msg, &http, &notifier.endpoints).await;
            let latency = started.elapsed().as_millis() as u64;
            let wait = match &result {
                Err(f) if f.retry && !test => delivery::retry_after(attempt, critical),
                _ => None,
            };
            let now = crate::history::now_ms();
            let ok = result.is_ok();
            let error = result.err().map(|f| f.message);
            let done: Vec<Delivery> = rows
                .iter()
                .map(|r| Delivery {
                    ts_ms: now,
                    ok,
                    error: error.clone(),
                    attempts: attempt,
                    outcome: if ok { Outcome::Sent } else { Outcome::Failed },
                    next_retry_ms: wait.map(|w| now + w.as_millis() as u64),
                    max_attempts: (!ok && wait.is_some()).then(|| delivery::max_attempts(critical)),
                    latency_ms: ok.then_some(latency),
                    ..r.clone()
                })
                .collect();
            notifier.deliveries.record_all(done.clone());
            let Some(wait) = wait else {
                if !ok {
                    crate::logging::write("notify", &format!("{} to {} failed: {}", msg.title, ch.name, error.as_deref().unwrap_or("")));
                }
                return done.into_iter().next().unwrap_or_else(|| Delivery::row(Some(&ch), &msg.title, msg.severity, Outcome::Failed));
            };
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    })
}

/// Decides what a batch of alert events deserves and records every outcome; what's sent
/// goes out when its group window closes (`start`). Called from health::on_publish (the
/// set of healthy agents it passes is no longer used: who sends is decided per channel).
pub(crate) fn on_alerts(app: &AppHandle, events: &[AlertEvent], _agents: &HashSet<String>, demo: bool) {
    let Some(notifier) = app.try_state::<Arc<Notifier>>() else { return };
    let s = notifier.settings();
    let now = crate::history::now_ms();
    let mut outcomes = Vec::new();
    let mut gate_events = Vec::new();
    let silences = silence_reasons(app);
    let mut gate = notifier.gate(demo);
    for e in events {
        match convert(e, &s, &silences, now) {
            Conv::Event(g) => gate_events.push(g),
            Conv::Ack(rule, entity) => gate.ack(&rule, &entity),
            Conv::Skip(o) => outcomes.push(o),
            Conv::Ignore => {}
        }
    }
    outcomes.extend(gate.decide(&gate_events, &s.gate(), now));
    let notices = gate.flush(now);
    notifier.persist(demo, &gate);
    drop(gate);
    let rows: Vec<Delivery> = outcomes.iter().filter(|o| o.outcome != Outcome::Sent).map(|o| outcome_row(o, demo)).collect();
    notifier.deliveries.record_all(rows);
    dispatch(app, notices, demo);
}

/// Closes group windows as they come due and sends repeats for critical alerts still
/// firing. Started once (agent::start).
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(1));
        let mut n: u64 = 0;
        loop {
            every.tick().await;
            n += 1;
            let Some(notifier) = app.try_state::<Arc<Notifier>>() else { continue };
            let demo = app.try_state::<crate::AppState>().map(|s| !crate::is_live(&s)).unwrap_or(true);
            let now = crate::history::now_ms();
            let repeat = n % 30 == 0;
            let firing = if repeat { firing_now(&app, now) } else { Vec::new() };
            let agent_pages = app.try_state::<Arc<crate::health::Health>>().map(|h| h.take_agent_pages(demo)).unwrap_or_default();
            let mut gate = notifier.gate(demo);
            for p in &agent_pages {
                gate.paged_elsewhere(&p.incident, p.severity, &p.title, p.ts_ms);
            }
            if repeat {
                gate.sync(&firing);
                gate.tick(&notifier.settings().gate(), now);
            }
            let notices = gate.flush(now);
            if repeat || !notices.is_empty() || !agent_pages.is_empty() {
                notifier.persist(demo, &gate);
            }
            drop(gate);
            dispatch(&app, notices, demo);
        }
    });
}

/// "Silenced until 10:42 AM · expanding the disk on Friday", by silence id.
fn silence_reasons(app: &AppHandle) -> HashMap<String, String> {
    let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return HashMap::new() };
    let now = crate::history::now_ms();
    health.view().silences.iter().map(|x| (x.id.clone(), silenced_until(x, now))).collect()
}

/// "Silenced until 10:42 AM", "Silenced until Tue 10:42 AM", "Silenced until it clears",
/// "Silenced indefinitely", then the reason: what a delivery that wasn't paged says.
pub fn silenced_until(x: &crate::health::silence::Silence, now: u64) -> String {
    use crate::health::silence::Until;
    let when = match x.until {
        Until::At { ms } => {
            let z = jiff::tz::TimeZone::system();
            let at = jiff::Timestamp::from_millisecond(ms as i64).map(|t| t.to_zoned(z.clone()));
            let today = jiff::Timestamp::from_millisecond(now as i64).map(|t| t.to_zoned(z).date());
            match (at, today) {
                (Ok(at), Ok(today)) if at.date() == today => format!("until {}", at.strftime("%-I:%M %p")),
                (Ok(at), _) => format!("until {}", at.strftime("%a %-I:%M %p")),
                _ => "for a while".to_string(),
            }
        }
        Until::Resolved => "until it clears (at most 7 days)".into(),
        Until::Never => "indefinitely".into(),
    };
    if x.reason.is_empty() {
        format!("Silenced {when}")
    } else {
        format!("Silenced {when} · {}", x.reason)
    }
}

/// (rule, alert id, acknowledged, silenced, planned) for what's firing now.
fn firing_now(app: &AppHandle, now: u64) -> Vec<(String, String, bool, bool, bool)> {
    let Some(health) = app.try_state::<Arc<crate::health::Health>>() else { return Vec::new() };
    firing_in(&health.view(), now)
}

/// The same, from a health view (what `sync` takes before each repeat tick).
pub(crate) fn firing_in(view: &crate::health::HealthView, now: u64) -> Vec<(String, String, bool, bool, bool)> {
    view.alerts
        .iter()
        .filter(|a| a.state == crate::health::AlertState::Firing)
        .map(|a| {
            let v = serde_json::to_value(a).unwrap_or_default();
            let silenced = a.muted(now) || alert_str(&v, "silenceId").is_some();
            (a.rule.clone(), a.id.clone(), a.acked_ms.is_some(), silenced, alert_str(&v, "planned").is_some())
        })
        .collect()
}

pub mod commands {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use serde::Serialize;
    use tauri::{AppHandle, Emitter, State};

    use super::{secret_name, show, Notice, Notifier, NotifySettings, PENDING_EVENT};
    use crate::delivery::{Channel, Delivery, Kind, Outcome};

    #[tauri::command]
    pub fn notify_settings_get(notifier: State<'_, Arc<Notifier>>) -> NotifySettings {
        notifier.settings()
    }

    fn on_off(b: bool) -> &'static str {
        if b {
            "on"
        } else {
            "off"
        }
    }

    fn repeat_word(m: u32) -> String {
        match m {
            0 => "never".into(),
            m if m % 60 == 0 => format!("{} h", m / 60),
            m => format!("{m} min"),
        }
    }

    /// "Critical alerts only: off → on · Quiet hours: off → 22:00 to 07:00 · America/New_York".
    pub fn settings_diff(a: &NotifySettings, b: &NotifySettings) -> Vec<String> {
        let mut out = Vec::new();
        let mut flag = |label: &str, x: bool, y: bool| {
            if x != y {
                out.push(format!("{label}: {} → {}", on_off(x), on_off(y)));
            }
        };
        flag("Notification Center", a.enabled, b.enabled);
        flag("Critical alerts only", a.critical_only, b.critical_only);
        flag("When an alert clears", a.resolved, b.resolved);
        flag("Node status", a.node, b.node);
        flag("Disk", a.disk, b.disk);
        flag("GPU", a.gpu, b.gpu);
        flag("Fabric & switches", a.fabric, b.fabric);
        flag("Containers, services & models", a.containers, b.containers);
        flag("Tasks", a.tasks, b.tasks);
        let quiet = |s: &NotifySettings| s.quiet().map(|q| q.label()).unwrap_or_else(|| "off".into());
        if quiet(a) != quiet(b) {
            out.push(format!("Quiet hours: {} → {}", quiet(a), quiet(b)));
        }
        if a.repeat_minutes != b.repeat_minutes {
            out.push(format!("Repeat critical alerts: {} → {}", repeat_word(a.repeat_minutes), repeat_word(b.repeat_minutes)));
        }
        out
    }

    /// Saves the switches, quiet hours (in the zone given, or this Mac's) and the repeat; the
    /// channels are kept as they are (notify_channels_set). Audited with old → new.
    #[tauri::command]
    pub fn notify_settings_set(app: AppHandle, notifier: State<'_, Arc<Notifier>>, settings: NotifySettings) -> Result<NotifySettings, String> {
        crate::admin::require_mac(&app)?;
        let before = notifier.settings();
        let mut next = NotifySettings { channels: before.channels.clone(), changed_ms: before.changed_ms, ..settings };
        if next.quiet_tz.is_empty() {
            next.quiet_tz = if before.quiet_tz.is_empty() { super::mac_zone() } else { before.quiet_tz.clone() };
        }
        let diff = settings_diff(&before, &next);
        if diff.is_empty() {
            return Ok(before);
        }
        next.changed_ms = crate::history::now_ms();
        let saved = notifier.save(next)?;
        if let Some(state) = tauri::Manager::try_state::<crate::AppState>(&app) {
            state.live.audit.record_app("Notification settings changed", &diff.join(" · "));
        }
        crate::agent::config_changed(&app);
        Ok(saved)
    }

    /// A sample notification, so the user can see where and how they appear (This Mac's Test).
    fn show_sample(app: &AppHandle) {
        show(app, &Notice::plain("SparkBox notifications are on", "Alerts and finished tasks will appear like this.", None));
    }

    /// The latest notification, if SparkBox came to the front within 10 minutes of it without
    /// a click: sent as `notify://pending` for the shell's "Last notification · Open", once.
    #[tauri::command]
    pub fn notify_take_pending(app: AppHandle, notifier: State<'_, Arc<Notifier>>) -> Option<super::Target> {
        let t = notifier.pending.lock().unwrap().take(crate::history::now_ms())?;
        let _ = app.emit(PENDING_EVENT, &t);
        Some(t)
    }

    /// A channel as Settings › Notifications shows it: which secrets are saved and its latest delivery.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ChannelView {
        #[serde(flatten)]
        pub channel: Channel,
        /// "ntfy.sh · sparkbox-alerts", "Notification Center".
        pub target: String,
        pub secrets: BTreeMap<String, bool>,
        pub last: Option<Delivery>,
    }

    fn mac_channel(s: &NotifySettings) -> Channel {
        Channel { enabled: s.enabled, critical_only: s.critical_only, ..Channel::empty("mac", Kind::Mac, "This Mac") }
    }

    /// Whether the data source in use is Demo (its deliveries are listed apart from Live's).
    fn demo_source(app: &AppHandle) -> bool {
        tauri::Manager::try_state::<crate::AppState>(app).map(|s| !crate::is_live(&s)).unwrap_or(true)
    }

    fn views(app: &AppHandle, notifier: &Notifier) -> Vec<ChannelView> {
        let s = notifier.settings();
        let last = notifier.deliveries.last_by_channel_in(demo_source(app));
        std::iter::once(mac_channel(&s))
            .chain(s.channels.iter().cloned())
            .map(|c| {
                let secrets = c.kind.secret_fields().iter().map(|f| (f.to_string(), c.saved.iter().any(|x| x == f))).collect();
                ChannelView { last: last.get(&c.id).cloned(), secrets, target: c.target(), channel: c }
            })
            .collect()
    }

    #[tauri::command]
    pub fn notify_channels_get(app: AppHandle, notifier: State<'_, Arc<Notifier>>) -> Vec<ChannelView> {
        views(&app, &notifier)
    }

    /// The channel list after an edit, and what the activity log says about it. A change of
    /// kind, server, URL, host, port, security or username points a channel somewhere else, so
    /// its saved secrets are cleared unless a new one comes with the change. `secrets` maps
    /// "<channel id>.<field>" to a new value ("" removes it). Returns the list to save, the
    /// Keychain fields to set or clear, and one audit line per changed channel.
    #[allow(clippy::type_complexity)]
    pub fn apply_channels(before: &[Channel], channels: Vec<Channel>, secrets: &HashMap<String, String>, lan: &[String]) -> Result<(Vec<Channel>, Option<Channel>, Vec<(String, Option<String>)>, Vec<String>), String> {
        let mut list: Vec<Channel> = Vec::new();
        let mut mac: Option<Channel> = None;
        let mut writes: Vec<(String, Option<String>)> = Vec::new();
        let mut audit: Vec<String> = Vec::new();
        let mut secrets = secrets.clone();
        for mut c in channels {
            let fields = c.kind.secret_fields();
            let old = before.iter().find(|x| x.id == c.id);
            // A webhook or dead-man's-switch URL is a credential: one that comes in the channel goes
            // to the Keychain like any other secret, and only its host stays in notify.json.
            if fields.contains(&"url") {
                let key = format!("{}.url", c.id);
                if let Some(u) = c.url.take().filter(|u| !u.trim().is_empty()) {
                    secrets.entry(key.clone()).or_insert(u.trim().to_string());
                }
                c.url_host = match secrets.get(&key) {
                    Some(u) if !u.is_empty() => Some(crate::delivery::url_check(c.kind, u, lan).map_err(|e| format!("{}: {e}", c.name))?),
                    Some(_) => None,
                    None => old.and_then(|o| o.url_host.clone()),
                };
            }
            for (key, _) in secrets.iter().filter(|(k, _)| k.starts_with(&format!("{}.", c.id))) {
                let field = &key[c.id.len() + 1..];
                if !fields.contains(&field) {
                    return Err(format!("{} has no secret called \"{field}\".", c.name));
                }
            }
            // What's in the Keychain comes from the saved list, never from the UI.
            let moved = old.is_some_and(|o| o.destination() != c.destination());
            let had: Vec<String> = old.filter(|_| !moved).map(|x| x.saved.clone()).unwrap_or_default();
            let mut notes: Vec<String> = Vec::new();
            c.saved = Vec::new();
            for f in fields {
                match secrets.get(&format!("{}.{f}", c.id)) {
                    Some(v) if !v.is_empty() => {
                        c.saved.push(f.to_string());
                        writes.push((secret_name(&c.id, f), Some(v.clone())));
                        let replaced = old.is_some_and(|o| o.saved.iter().any(|x| x == f));
                        match (*f, replaced) {
                            // The log never holds the URL: its host when that changed (field_changes), else this.
                            ("url", true) if old.and_then(|o| o.url_host.as_ref()) == c.url_host.as_ref() => notes.push("URL changed (kept secret)".into()),
                            ("url", true) => {}
                            ("url", false) => notes.push("URL saved (kept secret)".into()),
                            _ => notes.push(format!("{} {}", secret_word(f), if replaced { "replaced" } else { "saved" })),
                        }
                    }
                    Some(_) => {
                        writes.push((secret_name(&c.id, f), None));
                        if old.is_some_and(|o| o.saved.iter().any(|x| x == f)) {
                            notes.push(format!("{} removed", secret_word(f)));
                        }
                    }
                    None if had.iter().any(|x| x == f) => c.saved.push(f.to_string()),
                    None => {
                        if old.is_some_and(|o| o.saved.iter().any(|x| x == f)) {
                            writes.push((secret_name(&c.id, f), None));
                            notes.push(format!("{} cleared", secret_word(f)));
                        }
                    }
                }
            }
            let has = |f: &str| c.saved.iter().any(|x| x == f);
            c.validate(has, lan)?;
            if c.kind == Kind::Mac {
                mac = Some(c);
                continue;
            }
            if list.iter().any(|x| x.id == c.id) {
                return Err(format!("Two channels have the id \"{}\".", c.id));
            }
            let line = match old {
                None => Some(format!("Added {}{}", c.name, if c.target().is_empty() { String::new() } else { format!(": {}", c.target()) })),
                Some(o) => {
                    let mut changes = field_changes(o, &c);
                    changes.extend(notes.drain(..));
                    (!changes.is_empty()).then(|| format!("{}: {}", c.name, changes.join(" · ")))
                }
            };
            if let Some(mut line) = line {
                if old.is_none() && !notes.is_empty() {
                    line = format!("{line} · {}", notes.join(" · "));
                }
                audit.push(line);
            }
            list.push(c);
        }
        for gone in before.iter().filter(|c| !list.iter().any(|x| x.id == c.id)) {
            audit.push(format!("Removed {}", gone.name));
            for f in gone.kind.secret_fields() {
                writes.push((secret_name(&gone.id, f), None));
            }
        }
        Ok((list, mac, writes, audit))
    }

    fn secret_word(f: &str) -> &'static str {
        match f {
            "token" => "token",
            "appToken" => "app token",
            "userKey" => "user key",
            "secret" => "signing secret",
            "password" => "password",
            "url" => "URL",
            _ => "secret",
        }
    }

    /// Old → new for every setting of a channel that isn't a secret.
    fn field_changes(a: &Channel, b: &Channel) -> Vec<String> {
        let mut out = Vec::new();
        let o = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("none").to_string();
        let short = |s: String| s.trim_start_matches("https://").trim_start_matches("http://").to_string();
        let mut pair = |label: &str, x: String, y: String| {
            if x != y {
                out.push(format!("{label} {x} → {y}"));
            }
        };
        pair("name", a.name.clone(), b.name.clone());
        pair("kind", a.kind.label().into(), b.kind.label().into());
        pair("server", short(o(&a.server)), short(o(&b.server)));
        pair("topic", o(&a.topic), o(&b.topic));
        pair("device", o(&a.device), o(&b.device));
        pair("URL host", o(&a.url_host), o(&b.url_host));
        pair("host", o(&a.host), o(&b.host));
        pair("port", a.port.map(|p| p.to_string()).unwrap_or("none".into()), b.port.map(|p| p.to_string()).unwrap_or("none".into()));
        pair("security", o(&a.security), o(&b.security));
        pair("username", o(&a.user), o(&b.user));
        pair("from", o(&a.from), o(&b.from));
        pair("to", o(&a.to), o(&b.to));
        let word = |b: bool, yes: &str, no: &str| if b { yes.to_string() } else { no.to_string() };
        pair("alerts", word(a.critical_only, "critical only", "all"), word(b.critical_only, "critical only", "all"));
        pair("sent from", word(a.mac_only, "this Mac only", "this Mac and the nodes"), word(b.mac_only, "this Mac only", "this Mac and the nodes"));
        pair("Slack-compatible", word(a.slack.unwrap_or(false), "yes", "no"), word(b.slack.unwrap_or(false), "yes", "no"));
        if a.enabled != b.enabled {
            out.push(if b.enabled { "turned on".into() } else { "turned off".into() });
        }
        out
    }

    /// The cluster's own addresses (where plain http is allowed, like any private address).
    fn lan_hosts(app: &AppHandle) -> Vec<String> {
        tauri::Manager::try_state::<crate::AppState>(app)
            .map(|s| s.inventory.lock().unwrap().nodes.iter().flat_map(|n| [n.ip.clone().unwrap_or_default(), n.host.clone()]).filter(|h| !h.is_empty()).collect())
            .unwrap_or_default()
    }

    /// Replaces the channel list. `secrets` maps "<channel id>.<field>" to a new value for the
    /// Keychain ("" removes it); a channel pointed somewhere else loses its saved secrets
    /// unless new ones come with it; secrets of channels that are gone are removed too.
    /// Every change is in the activity log with old → new (never a secret). Mac scope.
    #[tauri::command]
    pub fn notify_channels_set(app: AppHandle, notifier: State<'_, Arc<Notifier>>, channels: Vec<Channel>, secrets: HashMap<String, String>) -> Result<Vec<ChannelView>, String> {
        crate::admin::require_mac(&app)?;
        let before = notifier.settings();
        let (list, mac, writes, mut audit) = apply_channels(&before.channels, channels, &secrets, &lan_hosts(&app))?;
        // Saving or clearing a channel's secret is a Keychain write: it confirms on its own.
        if !writes.is_empty() {
            crate::admin::require_mac_confirmed(&app, "notify_channels_set")?;
        }
        for (name, value) in &writes {
            match value {
                Some(v) => crate::secrets::set(name, v)?,
                None => {
                    let _ = crate::secrets::clear(name);
                }
            }
        }
        let mut next = NotifySettings { channels: list, ..before.clone() };
        if let Some(m) = mac {
            if m.enabled != next.enabled {
                audit.push(format!("This Mac: Notification Center {} → {}", on_off(next.enabled), on_off(m.enabled)));
            }
            if m.critical_only != next.critical_only {
                audit.push(format!("This Mac: critical alerts only {} → {}", on_off(next.critical_only), on_off(m.critical_only)));
            }
            next.enabled = m.enabled;
            next.critical_only = m.critical_only;
        }
        if audit.is_empty() {
            return Ok(views(&app, &notifier));
        }
        next.changed_ms = crate::history::now_ms();
        notifier.save(next)?;
        if let Some(state) = tauri::Manager::try_state::<crate::AppState>(&app) {
            state.live.audit.record_app("Notification channels changed", &audit.join("; "));
        }
        crate::agent::config_changed(&app);
        Ok(views(&app, &notifier))
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TestResult {
        pub ok: bool,
        pub message: String,
        pub delivery: Option<Delivery>,
    }

    /// Sends a test on one channel: from this Mac (always for real, Demo included), or from
    /// the agent on `via` over SSH (`sparkbox_agent.py test-notify`, node scope). At most one
    /// per channel every 10 s. Changes no setting.
    #[tauri::command]
    pub async fn notify_channel_test(app: AppHandle, id: String, via: Option<String>) -> Result<TestResult, String> {
        if via.is_some() {
            crate::admin::require(&app)?;
        }
        let notifier = tauri::Manager::state::<Arc<Notifier>>(&app).inner().clone();
        let s = notifier.settings();
        let now = crate::history::now_ms();
        if !notifier.may_test(&format!("{id}@{}", via.as_deref().unwrap_or("mac")), now) {
            return Err("Wait a few seconds before testing that channel again.".into());
        }
        if super::dev_muted() {
            return Ok(TestResult { ok: false, message: "Not sent: notifications are muted in this dev run.".into(), delivery: None });
        }
        if id == "mac" {
            show_sample(&app);
            return Ok(TestResult { ok: true, message: "Shown in Notification Center.".into(), delivery: None });
        }
        let ch = s.channels.iter().find(|c| c.id == id).cloned().ok_or("That channel isn't saved yet.")?;
        if let Some(node) = via {
            return crate::agent::test_notify(&app, &node, &ch).await;
        }
        let msg = super::test_message(&ch, now);
        let row = Delivery { body: msg.body.clone(), test: true, ..Delivery::row(Some(&ch), &msg.title, msg.severity, Outcome::Sent) };
        let d = super::deliver(&app, ch.clone(), msg, vec![row], true).await.map_err(|e| e.to_string())?;
        let message = match (d.ok, ch.kind) {
            (true, Kind::Heartbeat) => format!("The check answered in {}.", crate::agent::fmt_secs(d.latency_ms.unwrap_or(0))),
            (true, _) => "Sent from this Mac.".to_string(),
            (false, _) => format!("Failed: {}", d.error.clone().unwrap_or_default()),
        };
        Ok(TestResult { ok: d.ok, message, delivery: Some(d) })
    }

    /// The delivery log, newest first: one row per alert event and channel, with its outcome.
    #[tauri::command]
    pub fn notify_deliveries(app: AppHandle, notifier: State<'_, Arc<Notifier>>, limit: usize) -> Vec<Delivery> {
        notifier.deliveries.list_in(limit.clamp(1, crate::delivery::LOG_KEEP), demo_source(&app))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{AlertState, Entity};

    fn event(kind: EventKind, rule: &str, sev: Severity, cat: Category, node: &str, id: &str) -> AlertEvent {
        let alert: Alert = serde_json::from_value(json!({
            "id": format!("{rule}:{id}"), "rule": rule, "category": cat, "severity": sev, "state": AlertState::Firing,
            "entity": Entity { kind: "node".into(), id: id.into(), node: Some(node.into()), switch: None, label: node.into() },
            "message": "Offline", "summary": format!("{node} offline"), "detail": null, "value": null, "sinceMs": 0,
            "firingSinceMs": 0, "resolvedMs": null, "ackedMs": null, "mutedUntilMs": null
        }))
        .unwrap();
        AlertEvent::new(0, kind, alert)
    }

    fn gate_events(events: &[AlertEvent], s: &NotifySettings) -> Vec<GateEvent> {
        events.iter().filter_map(|e| match convert(e, s, &HashMap::new(), 0) {
            Conv::Event(g) => Some(g),
            _ => None,
        }).collect()
    }

    /// decide, then close the group window.
    fn run(g: &mut Gate, events: &[AlertEvent], s: &NotifySettings, now: u64) -> (Vec<GateOutcome>, Vec<Notice>) {
        let out = g.decide(&gate_events(events, s), &s.gate(), now);
        (out, g.flush(now + GROUP_WAIT_MS))
    }

    // ---- The shared fixture (plan §2.1), the same file agent_test.py runs.

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        settings: FixSettings,
        events: Vec<FixEvent>,
        expect: Vec<FixExpect>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixSettings {
        critical_only: bool,
        resolved: bool,
        repeat_minutes: u32,
        quiet: Option<Quiet>,
    }

    #[derive(Deserialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct FixEvent {
        t: u64,
        kind: String,
        rule: Option<String>,
        entity: Option<String>,
        node: Option<String>,
        severity: Option<Severity>,
        incident: Option<String>,
        title: Option<String>,
        reason: Option<String>,
        until: Option<u64>,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct FixExpect {
        t: u64,
        outcomes: Vec<FixOutcome>,
        messages: Vec<FixMessage>,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct FixOutcome {
        rule: String,
        entity: String,
        outcome: Outcome,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct FixMessage {
        title: String,
        severity: Severity,
        lines: Option<Vec<String>>,
    }

    /// Runs one fixture case the way its comment says; returns what happened at each t.
    fn run_case(c: &Case) -> Vec<FixExpect> {
        let s = GateSettings { critical_only: c.settings.critical_only, resolved: c.settings.resolved, repeat_minutes: c.settings.repeat_minutes, quiet: c.settings.quiet.clone() };
        let mut g = Gate::default();
        let mut silences: Vec<(Option<String>, String, String)> = Vec::new();
        let mut maint: Vec<(String, u64, String)> = Vec::new();
        let mut ts: Vec<u64> = c.events.iter().map(|e| e.t).collect();
        ts.dedup();
        let mut got = Vec::new();
        for t in ts {
            let now = t * 1000;
            let at: Vec<&FixEvent> = c.events.iter().filter(|e| e.t == t).collect();
            if at.iter().any(|e| e.kind == "tick") {
                g.tick(&s, now);
            }
            let mut evs = Vec::new();
            for e in at {
                let rule = e.rule.clone().unwrap_or_default();
                let entity = e.entity.clone().unwrap_or_default();
                match e.kind.as_str() {
                    "tick" => {}
                    "ack" => g.ack(&rule, &entity),
                    "paged" => g.paged_elsewhere(e.incident.as_deref().unwrap_or_default(), e.severity.unwrap_or(Severity::Warn), e.title.as_deref().unwrap_or_default(), now),
                    "silence" => silences.push((e.rule.clone(), entity.clone(), e.reason.clone().unwrap_or_default())),
                    "maintenance" => maint.push((e.node.clone().unwrap_or_default(), e.until.unwrap_or(0) * 1000, e.reason.clone().unwrap_or_default())),
                    kind => {
                        let step = match kind {
                            "raised" => Step::Raised,
                            "escalated" => Step::Escalated,
                            "deescalated" => Step::Deescalated,
                            "resolved" => Step::Resolved,
                            k => panic!("{}: unknown event kind {k}", c.name),
                        };
                        let silenced = silences.iter().find(|(r, en, _)| *en == entity && r.as_ref().is_none_or(|r| *r == rule)).map(|(_, _, why)| why.clone());
                        let planned = maint.iter().find(|(n, until, _)| e.node.as_deref() == Some(n.as_str()) && now < *until).map(|(_, _, why)| why.clone());
                        evs.push(GateEvent {
                            step,
                            rule: rule.clone(),
                            entity: entity.clone(),
                            severity: e.severity.unwrap_or(Severity::Warn),
                            incident: e.incident.clone().unwrap_or_else(|| entity.clone()),
                            title: e.title.clone().unwrap_or_default(),
                            body: String::new(),
                            scope: Scope { rule, kind: entity.split(':').next().unwrap_or("").into(), id: entity.clone(), node: e.node.clone(), switch: None },
                            alert_id: None,
                            open: None,
                            silenced,
                            planned,
                            clear_held: false,
                        });
                    }
                }
            }
            // A silence also stops repeats of what's already firing under it.
            for (r, en, _) in &silences {
                for (k, st) in g.keys.iter_mut() {
                    let (kr, ke) = k.split_once('\u{1f}').unwrap();
                    if ke == en && r.as_deref().is_none_or(|r| r == kr) {
                        st.silenced = true;
                    }
                }
            }
            let outcomes = g.decide(&evs, &s, now).into_iter().map(|o| FixOutcome { rule: o.rule, entity: o.entity, outcome: o.outcome }).collect();
            let messages = g.flush(now + GROUP_WAIT_MS).into_iter().map(|n| FixMessage { title: n.title, severity: n.severity, lines: Some(n.lines) }).collect();
            got.push(FixExpect { t, outcomes, messages });
        }
        got
    }

    #[test]
    fn gate_parity_fixture() {
        let text = include_str!("agent/fixtures/gate-parity.json");
        let f: Fixture = serde_json::from_str(text).unwrap();
        let names: Vec<&str> = f.cases.iter().map(|c| c.name.split(' ').next().unwrap()).collect();
        for g in ["G1", "G2", "G3", "G4", "G5", "G6", "G7", "G8", "G9", "G10a", "G10b", "G11", "G12", "G13", "G14", "G15", "G16", "G17"] {
            assert!(names.contains(&g), "{g} is in the fixture");
        }
        let mut failures = Vec::new();
        for c in &f.cases {
            let got = run_case(c);
            assert_eq!(got.len(), c.expect.len(), "{}: one expectation per t", c.name);
            for (want, got) in c.expect.iter().zip(got) {
                assert_eq!(want.t, got.t, "{}", c.name);
                let messages_match = want.messages.len() == got.messages.len()
                    && want.messages.iter().zip(&got.messages).all(|(w, g)| w.title == g.title && w.severity == g.severity && w.lines.as_ref().is_none_or(|l| Some(l) == g.lines.as_ref()));
                if want.outcomes != got.outcomes || !messages_match {
                    failures.push(format!("{} at t={}:\n  want {:?} {:?}\n  got  {:?} {:?}", c.name, want.t, want.outcomes, want.messages, got.outcomes, got.messages));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// agent/fixtures/docker-states.json: crash loops as Docker shows them, through the health
    /// engine's crash-loop rule and this gate, the way its comment says (agent_test.py runs it too).
    #[test]
    fn crash_loops_from_the_docker_states_fixture_page_once() {
        use crate::model::{ClusterSnapshot, Container, LinkStatus, NodeSnapshot};
        let text = include_str!("agent/fixtures/docker-states.json");
        let f: Value = serde_json::from_str(text).unwrap();
        let (every, start_s, recent) = (f["sampleS"].as_u64().unwrap(), f["startS"].as_u64().unwrap(), f["crashRecentS"].as_u64().unwrap());
        assert_eq!(recent, crate::health::rules::CRASH_RECENT_S);
        assert_eq!(f["crashWindowS"].as_u64().unwrap(), crate::health::rules::CRASH_WINDOW_S);
        fn flatten(phases: &Value, out: &mut Vec<(String, u64, bool)>) {
            for p in phases.as_array().unwrap() {
                if let Some(n) = p["repeat"].as_u64() {
                    for _ in 0..n {
                        flatten(&p["phases"], out);
                    }
                } else {
                    out.push((p["state"].as_str().unwrap().to_string(), p["forS"].as_u64().unwrap(), p["restarted"].as_bool().unwrap_or(false)));
                }
            }
        }
        let settings = NotifySettings::default();
        for case in f["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let container = case["container"].as_str().unwrap();
            let answering = case["endpoint"]["answering"].as_bool().unwrap_or(false);
            let mut phases = Vec::new();
            flatten(&case["phases"], &mut phases);
            let total: u64 = phases.iter().map(|p| p.1).sum();
            let mut engine = crate::health::Engine::default();
            let mut gate = Gate::default();
            let (mut raised, mut resolved, mut others, mut messages) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
            let (mut restarts, mut last_restart) = (0u32, None::<u64>);
            let mut t = 0u64;
            let mut seen_phase = usize::MAX;
            while t < total {
                // Which phase this sample falls in, and when it began.
                let (mut at, mut idx) = (0u64, 0usize);
                while at + phases[idx].1 <= t {
                    at += phases[idx].1;
                    idx += 1;
                }
                let (state, _, restarted) = &phases[idx];
                if idx != seen_phase && (state == "restarting" || *restarted) {
                    restarts += 1;
                }
                if state == "restarting" || (idx != seen_phase && *restarted) {
                    last_restart = Some(t);
                }
                seen_phase = idx;
                let c = Container {
                    name: container.into(),
                    state: state.clone(),
                    restart_count: restarts,
                    started_at: if state == "running" { start_s + at } else { 0 },
                    ..Default::default()
                };
                let node = NodeSnapshot { id: "cell2".into(), name: "cell2".into(), status: LinkStatus::Online, containers: vec![c], ..Default::default() };
                let snap = ClusterSnapshot { ts_ms: (start_s + t) * 1000, demo: true, nodes: vec![node], ..Default::default() };
                // The model it serves, answering on every sample (endpoint.answering).
                let served = crate::health::EndpointInfo {
                    id: "cell1-vllm-8000".into(),
                    head_node: "cell1".into(),
                    health: "ready".into(),
                    nodes: vec!["cell1".into(), "cell2".into()],
                    containers: vec![container.into()],
                    ..Default::default()
                };
                let endpoints = if answering { vec![served] } else { Vec::new() };
                let now = snap.ts_ms;
                let events = engine.step_with(&snap, &endpoints, now);
                for e in events.iter().filter(|e| e.alert.rule == "crash-loop") {
                    match e.kind {
                        EventKind::Raised => raised.push((t, e.alert.severity)),
                        EventKind::Resolved => resolved.push(t),
                        k => others.push((t, k)),
                    }
                }
                gate.tick(&settings.gate(), now);
                gate.decide(&gate_events(&events, &settings), &settings.gate(), now);
                messages += gate.flush(now + GROUP_WAIT_MS).len();
                t += every;
            }
            let want: Vec<(u64, Severity)> = case["expect"]["raised"].as_array().unwrap().iter().map(|r| (r["t"].as_u64().unwrap(), serde_json::from_value(r["severity"].clone()).unwrap())).collect();
            assert_eq!(raised, want, "{name}: the raises");
            assert!(others.is_empty(), "{name}: no escalation or easing: {others:?}");
            match case["expect"]["resolvedBetween"].as_array() {
                Some(b) => {
                    let (lo, hi) = (b[0].as_u64().unwrap(), b[1].as_u64().unwrap());
                    assert_eq!(resolved.len(), 1, "{name}: one resolve: {resolved:?}");
                    assert!((lo..=hi).contains(&resolved[0]), "{name}: resolved at {} (last restart {last_restart:?}), not in {lo}..{hi}", resolved[0]);
                }
                None => assert!(resolved.is_empty(), "{name}: {resolved:?}"),
            }
            assert!(messages <= case["expect"]["messagesAtMost"].as_u64().unwrap() as usize, "{name}: {messages} messages");
        }
    }

    #[test]
    fn a_page_waiting_in_the_group_window_survives_a_quit() {
        let dir = std::env::temp_dir().join(format!("sb-gate-quit-{}-{}", std::process::id(), fastrand::u32(..)));
        let log = Some(dir.join("deliveries.jsonl"));
        let s = NotifySettings::default();
        let crit = event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        {
            // Decided, saved as on_alerts saves it, then SparkBox quits 3 s into the 10 s window.
            let n = Notifier::with_log(None, log.clone());
            let mut g = n.gate(false);
            assert_eq!(g.decide(&gate_events(std::slice::from_ref(&crit), &s), &s.gate(), 1_000)[0].outcome, Outcome::Sent);
            assert!(g.flush(4_000).is_empty(), "still inside the window");
            n.persist(false, &g);
        }
        let again = Notifier::with_log(None, log);
        let mut g = again.gate(false);
        let sent = g.flush(60_000);
        assert_eq!(sent.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["cell3 offline"], "sent after the restart");
        assert!(g.flush(61_000).is_empty(), "once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn coalesced_warnings_ride_on_the_next_message_of_their_incident() {
        let s = NotifySettings::default();
        let mut g = Gate::default();
        let crit = event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        let (_, n) = run(&mut g, &[crit.clone()], &s, 0);
        assert_eq!(n.len(), 1);
        // A warning on the same node a minute later: listed, not paged.
        let mut warn = event(EventKind::Raised, "mem-high", Severity::Warn, Category::Node, "cell3", "cell3");
        warn.alert.summary = "cell3 memory 91%".into();
        let (o, n) = run(&mut g, &[warn], &s, 60_000);
        assert_eq!((o[0].outcome, n.len()), (Outcome::Coalesced, 0));
        assert!(o[0].reason.as_deref().unwrap().starts_with("Part of “cell3 offline”, paged 60 s earlier"), "{:?}", o[0].reason);
        // Its next message (the node back online, sent at once) carries it.
        let (o, n) = run(&mut g, &[event(EventKind::Resolved, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3")], &s, 120_000);
        assert_eq!((o[0].outcome, n.len()), (Outcome::Sent, 1));
        assert_eq!(n[0].title, "cell3 is back online");
        assert_eq!(n[0].lines, ["Also: cell3 memory 91%"]);
        assert!(n[0].body.ends_with("Also: cell3 memory 91%"), "{}", n[0].body);
        // Grouping keeps every alert's own row: one message id, several members.
        let events: Vec<_> = ["cell1", "cell2", "gx10-0000"].iter().map(|n| event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, n, n)).collect();
        let (_, n) = run(&mut g, &events, &s, 600_000);
        assert_eq!((n.len(), n[0].members.len(), n[0].title.as_str()), (1, 3, "3 alerts, 3 critical"));
        // Nothing goes out before the window closes.
        g.decide(&gate_events(&[event(EventKind::Raised, "disk-full", Severity::Crit, Category::Disk, "cell1", "/")], &s), &s.gate(), 700_000);
        assert!(g.flush(700_000 + GROUP_WAIT_MS - 1).is_empty());
        assert_eq!(g.flush(700_000 + GROUP_WAIT_MS).len(), 1);
    }

    #[test]
    fn quiet_hours_follow_their_zone_not_the_clock_of_this_machine() {
        // 21:30 UTC on 15 Jan 2026 is 22:30 in Paris and 16:30 in New York.
        let t = 1_768_512_600_000;
        assert_eq!(minutes_in("Europe/Paris", t), Some(22 * 60 + 30));
        assert_eq!(minutes_in("America/New_York", t), Some(16 * 60 + 30));
        assert_eq!(minutes_in("UTC", t), Some(21 * 60 + 30));
        let q = |tz: &str| Quiet { start: "22:00".into(), end: "07:00".into(), tz: tz.into() };
        assert!(q("Europe/Paris").active(t));
        assert!(!q("America/New_York").active(t) && !q("UTC").active(t));
        // Summer time moves the hour: 20:30 UTC in July is 22:30 in Paris (CEST).
        assert!(q("Europe/Paris").active(1_784_838_600_000));
        assert_eq!(clock_in("America/New_York", t), "16:30");
        assert_eq!(q("America/New_York").label(), "22:00 to 07:00 · America/New_York");
        assert!(valid_zone("Europe/Paris") && !valid_zone("Mars/Olympus"));
        // The settings say which zone; the gate uses it.
        let s = NotifySettings { quiet_hours: true, quiet_tz: "Europe/Paris".into(), ..Default::default() };
        let warn = event(EventKind::Raised, "mem-high", Severity::Warn, Category::Node, "cell1", "cell1");
        let (o, _) = run(&mut Gate::default(), std::slice::from_ref(&warn), &s, t);
        assert_eq!((o[0].outcome, o[0].reason.as_deref()), (Outcome::Held, Some("Quiet hours, 22:00 to 07:00 · Europe/Paris")));
        let s = NotifySettings { quiet_tz: "America/New_York".into(), ..s };
        assert_eq!(run(&mut Gate::default(), &[warn], &s, t).0[0].outcome, Outcome::Sent);
    }

    #[test]
    fn the_gate_survives_a_restart_so_a_re_raise_is_a_duplicate() {
        let dir = std::env::temp_dir().join(format!("sb-gate-{}-{}", std::process::id(), fastrand::u32(..)));
        let log = Some(dir.join("deliveries.jsonl"));
        let s = NotifySettings::default();
        let n = Notifier::with_log(None, log.clone());
        let warn = event(EventKind::Raised, "disk-full", Severity::Warn, Category::Disk, "gx10-0000", "/");
        {
            let mut g = n.gate(false);
            g.decide(&gate_events(std::slice::from_ref(&warn), &s), &s.gate(), 1_000);
            n.persist(false, &g);
        }
        let again = Notifier::with_log(None, log);
        let o = again.gate(false).decide(&gate_events(std::slice::from_ref(&warn), &s), &s.gate(), 31_000);
        assert_eq!(o[0].outcome, Outcome::Duplicate);
        // Demo has its own gate: the same alert there is new.
        assert_eq!(again.gate(true).decide(&gate_events(&[warn], &s), &s.gate(), 31_000)[0].outcome, Outcome::Sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeats_follow_acks_silences_and_whats_still_firing() {
        let s = NotifySettings { repeat_minutes: 15, ..Default::default() };
        let mut g = Gate::default();
        let crit = event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        run(&mut g, &[crit], &s, 0);
        let firing = |acked: bool, silenced: bool| vec![("node-offline".to_string(), "node-offline:cell3".to_string(), acked, silenced, false)];
        g.sync(&firing(false, false));
        assert_eq!(g.tick(&s.gate(), 15 * 60_000).len(), 1);
        assert_eq!(g.flush(15 * 60_000 + GROUP_WAIT_MS)[0].title, "Still firing: cell3 offline");
        g.sync(&firing(false, true));
        assert!(g.tick(&s.gate(), 31 * 60_000).is_empty(), "silenced since");
        // Gone from what's firing without a resolve (a rule removed): no ghost repeats.
        g.sync(&[]);
        assert!(g.tick(&s.gate(), 60 * 60_000).is_empty());
    }

    #[test]
    fn warnings_held_overnight_and_still_firing_make_one_morning_digest() {
        let s = NotifySettings { quiet_hours: true, quiet_tz: "America/New_York".into(), ..Default::default() };
        let night = 1_768_536_000_000; // 23:00 in New York
        let morning = 1_768_565_100_000; // 07:05
        let mut g = Gate::default();
        let mut disk = event(EventKind::Raised, "disk-full", Severity::Warn, Category::Disk, "gx10-0000", "/");
        disk.alert.summary = "gx10-0000 disk 86%".into();
        let mut gpu = event(EventKind::Raised, "gpu-hot", Severity::Warn, Category::Gpu, "cell2", "gpu0");
        gpu.alert.summary = "cell2 GPU 88 °C".into();
        let (o, _) = run(&mut g, &[disk.clone(), gpu.clone()], &s, night);
        assert!(o.iter().all(|o| o.outcome == Outcome::Held));
        g.tick(&s.gate(), night + 60_000);
        // cell2's GPU cooled down overnight; the disk didn't.
        let mut cooled = gpu.clone();
        cooled.kind = EventKind::Resolved;
        assert_eq!(run(&mut g, &[cooled], &s, night + 3_600_000).0[0].outcome, Outcome::NotPaged);
        assert!(g.tick(&s.gate(), morning - 600_000).is_empty(), "still quiet at 06:55");
        let o = g.tick(&s.gate(), morning);
        assert_eq!(o.iter().map(|o| (o.rule.as_str(), o.outcome)).collect::<Vec<_>>(), [("disk-full", Outcome::Sent)]);
        let n = g.flush(morning);
        assert_eq!((n.len(), n[0].title.as_str(), n[0].lines.clone()), (1, "While quiet: 1 warning still firing", vec!["gx10-0000 disk 86%".to_string()]));
        assert!(g.tick(&s.gate(), morning + 30_000).is_empty(), "once");
        // Digested counts as paged: its resolve is sent, once it has stayed clear for 10 minutes.
        let mut fixed = disk;
        fixed.kind = EventKind::Resolved;
        assert_eq!(run(&mut g, &[fixed], &s, morning + 60_000).0[0].outcome, Outcome::Held);
        let o = g.tick(&s.gate(), morning + 60_000 + FLAP_MS);
        assert_eq!(o.iter().map(|o| (o.rule.as_str(), o.outcome)).collect::<Vec<_>>(), [("disk-full", Outcome::Sent)]);
    }

    #[test]
    fn coming_back_online_says_so() {
        let mut g = Gate::default();
        let s = NotifySettings::default();
        run(&mut g, &[event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3")], &s, 0);
        let (o, n) = run(&mut g, &[event(EventKind::Resolved, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3")], &s, 60_000);
        assert_eq!(o[0].outcome, Outcome::Sent, "never held");
        assert_eq!(n[0].title, "cell3 is back online");
        assert_eq!(n[0].open, Some(("node".to_string(), json!({ "id": "cell3" }))));
        assert!(n[0].resolved);
        // It came back with a new boot_id: it says so.
        let mut back = event(EventKind::Resolved, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        back.alert.reason = Some(crate::health::REBOOTED.into());
        run(&mut g, &[event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3")], &s, 700_000);
        let (_, n) = run(&mut g, &[back], &s, 760_000);
        assert_eq!(n[0].title, "cell3 is back · it rebooted");
    }

    /// One "back online" was held 14 minutes in round 4. It's sent the moment the node is back,
    /// flapping, after a restart of the app, and in quiet hours (a node going away is critical).
    #[test]
    fn back_online_is_never_held() {
        let s = NotifySettings::default();
        let off = || event(EventKind::Raised, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        let back = || event(EventKind::Resolved, "node-offline", Severity::Crit, Category::Node, "cell3", "cell3");
        let mut g = Gate::default();
        let mut sent = Vec::new();
        for (i, ev) in [off(), back(), off(), back(), off(), back()].into_iter().enumerate() {
            let (o, n) = run(&mut g, &[ev], &s, i as u64 * 60_000);
            assert_eq!(o[0].outcome, Outcome::Sent, "step {i}");
            sent.extend(n.into_iter().map(|n| n.title));
        }
        // Each return at once; gone again soon after one it's paged again, the second time as flapping.
        assert_eq!(sent, ["cell3 offline", "cell3 is back online", "cell3 offline", "cell3 is back online", "cell3 offline · flapping", "cell3 is back online"]);
        // A flapping node-offline's resolve isn't one the engine held either.
        let mut flapping = back();
        flapping.alert.flapping = Some(crate::health::FlapInfo { count: 3, since_ms: 0 });
        assert!(gate_events(&[flapping], &s).iter().all(|e| !e.clear_held));
        // Through a restart of the app.
        let dir = std::env::temp_dir().join(format!("sb-gate-back-{}-{}", std::process::id(), fastrand::u32(..)));
        let n = Notifier::with_log(None, Some(dir.join("deliveries.jsonl")));
        {
            let mut g = n.gate(false);
            g.decide(&gate_events(&[off()], &s), &s.gate(), 0);
            n.persist(false, &g);
        }
        let again = Notifier::with_log(None, Some(dir.join("deliveries.jsonl")));
        let mut g = again.gate(false);
        g.flush(GROUP_WAIT_MS);
        assert_eq!(g.decide(&gate_events(&[back()], &s), &s.gate(), 60_000)[0].outcome, Outcome::Sent);
        assert_eq!(g.flush(60_000 + GROUP_WAIT_MS)[0].title, "cell3 is back online");
        let _ = std::fs::remove_dir_all(&dir);
        // In quiet hours: the node was paged as critical, so its return is sent too.
        let quiet = NotifySettings { quiet_hours: true, quiet_tz: "America/New_York".into(), ..Default::default() };
        let night = 1_768_536_000_000; // 23:00 in New York
        let mut g = Gate::default();
        run(&mut g, &[off()], &quiet, night);
        let (o, n) = run(&mut g, &[back()], &quiet, night + 60_000);
        assert_eq!((o[0].outcome, n[0].title.as_str()), (Outcome::Sent, "cell3 is back online"));
        // Any other resolve still waits out its 10 minutes clear.
        let mut g = Gate::default();
        let disk = || event(EventKind::Raised, "disk-full", Severity::Warn, Category::Disk, "gx10-0000", "/");
        run(&mut g, &[disk()], &s, 0);
        let mut fixed = disk();
        fixed.kind = EventKind::Resolved;
        assert_eq!(run(&mut g, &[fixed], &s, 60_000).0[0].outcome, Outcome::Held);
    }

    #[test]
    fn settings_filter_by_category_and_say_so() {
        let warn = event(EventKind::Raised, "disk-full", Severity::Warn, Category::Disk, "gx10-0000", "/");
        let s = NotifySettings { disk: false, ..Default::default() };
        match convert(&warn, &s, &HashMap::new(), 0) {
            Conv::Skip(o) => assert_eq!((o.outcome, o.reason.as_deref()), (Outcome::NotPaged, Some("Disk notifications are off"))),
            _ => panic!("a switched-off category is recorded as not paged"),
        }
    }

    #[test]
    fn information_is_never_notified() {
        let mut idle = event(EventKind::Raised, "endpoint-idle", Severity::Info, Category::Containers, "cell1", "ep");
        assert!(matches!(convert(&idle, &NotifySettings::default(), &HashMap::new(), 0), Conv::Ignore));
        idle.alert.severity = Severity::Warn;
        idle.alert.notify = false;
        assert!(matches!(convert(&idle, &NotifySettings::default(), &HashMap::new(), 0), Conv::Ignore));
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        assert!(in_quiet(23 * 60, "22:00", "07:00"));
        assert!(in_quiet(6 * 60 + 59, "22:00", "07:00"));
        assert!(!in_quiet(7 * 60, "22:00", "07:00"));
        assert!(in_quiet(13 * 60, "12:00", "14:00"));
        assert!(!in_quiet(13 * 60, "12:00", "12:00"));
        assert!(!in_quiet(13 * 60, "noon", "14:00"));
    }

    #[test]
    fn settings_reject_bad_times_zones_and_repeats_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("sb-notify-{}-{}", std::process::id(), fastrand::u32(..)));
        let n = Notifier::new(Some(dir.join("notify.json")));
        assert!(n.save(NotifySettings { quiet_start: "25:00".into(), ..Default::default() }).is_err());
        assert!(n.save(NotifySettings { quiet_tz: "Mars/Olympus".into(), ..Default::default() }).unwrap_err().contains("time zone"));
        assert!(n.save(NotifySettings { repeat_minutes: 7, ..Default::default() }).is_err());
        let s = NotifySettings { gpu: false, quiet_hours: true, quiet_tz: "America/New_York".into(), repeat_minutes: 240, ..Default::default() };
        n.save(s.clone()).unwrap();
        assert_eq!(Notifier::new(Some(dir.join("notify.json"))).settings(), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_round_one_settings_file_still_loads() {
        let dir = std::env::temp_dir().join(format!("sb-notify-old-{}-{}", std::process::id(), fastrand::u32(..)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notify.json"), r#"{"enabled":true,"node":true,"disk":false,"gpu":true,"fabric":true,"containers":true,"tasks":true,"criticalOnly":false,"resolved":true,"quietHours":false,"quietStart":"22:00","quietEnd":"07:00"}"#).unwrap();
        let s = Notifier::new(Some(dir.join("notify.json"))).settings();
        assert!(!s.disk && s.channels.is_empty());
        assert_eq!((s.repeat_minutes, s.quiet_tz.as_str()), (60, ""), "new settings take their defaults");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ntfy(server: &str, saved: &[&str]) -> Channel {
        Channel { server: Some(server.into()), topic: Some("sparkbox-alerts".into()), saved: saved.iter().map(|s| s.to_string()).collect(), ..Channel::empty("phone", Kind::Ntfy, "Phone (ntfy)") }
    }

    #[test]
    fn editing_server_clears_token() {
        let before = vec![ntfy("https://ntfy.sh", &["token"])];
        let none = HashMap::new();
        // Pointed at another server without a new token: the saved one is cleared.
        let (list, _, writes, audit) = commands::apply_channels(&before, vec![ntfy("https://ntfy.example.com", &["token"])], &none, &[]).unwrap();
        assert!(list[0].saved.is_empty(), "the old server's token doesn't follow the channel");
        assert_eq!(writes, [(secret_name("phone", "token"), None)]);
        assert_eq!(audit, ["Phone (ntfy): server ntfy.sh → ntfy.example.com · token cleared"]);
        // With a new token it's saved instead.
        let new: HashMap<String, String> = [("phone.token".to_string(), "tk_new".to_string())].into();
        let (list, _, writes, audit) = commands::apply_channels(&before, vec![ntfy("https://ntfy.example.com", &[])], &new, &[]).unwrap();
        assert_eq!(list[0].saved, ["token"]);
        assert_eq!(writes, [(secret_name("phone", "token"), Some("tk_new".to_string()))]);
        assert_eq!(audit, ["Phone (ntfy): server ntfy.sh → ntfy.example.com · token replaced"]);
        // A rename or a topic change keeps it; what the UI claims is saved never counts.
        let (list, _, writes, audit) = commands::apply_channels(&before, vec![Channel { name: "Phone".into(), topic: Some("other".into()), saved: vec![], ..before[0].clone() }], &none, &[]).unwrap();
        assert_eq!((list[0].saved.clone(), writes.len()), (vec!["token".to_string()], 0));
        assert_eq!(audit, ["Phone: name Phone (ntfy) → Phone · topic sparkbox-alerts → other"]);
        let (list, _, _, _) = commands::apply_channels(&[], vec![ntfy("https://ntfy.sh", &["token"])], &none, &[]).unwrap();
        assert!(list[0].saved.is_empty());
        // Plain http off the LAN is refused before anything is written.
        assert!(commands::apply_channels(&before, vec![ntfy("http://ntfy.example.com", &[])], &new, &[]).unwrap_err().contains("Use https for ntfy.example.com"));
        // Mac only, critical only and removal are logged too.
        let edited = Channel { mac_only: true, critical_only: true, ..before[0].clone() };
        let (_, _, _, audit) = commands::apply_channels(&before, vec![edited], &none, &[]).unwrap();
        assert_eq!(audit, ["Phone (ntfy): alerts all → critical only · sent from this Mac and the nodes → this Mac only"]);
        let (_, _, writes, audit) = commands::apply_channels(&before, vec![], &none, &[]).unwrap();
        assert_eq!((audit, writes), (vec!["Removed Phone (ntfy)".to_string()], vec![(secret_name("phone", "token"), None)]));
        let (_, _, _, audit) = commands::apply_channels(&[], vec![ntfy("https://ntfy.sh", &[])], &new, &[]).unwrap();
        assert_eq!(audit, ["Added Phone (ntfy): ntfy.sh · sparkbox-alerts · token saved"]);
    }

    #[test]
    fn webhook_and_heartbeat_urls_are_kept_secret() {
        let none = HashMap::new();
        let hook = |url: Option<&str>| Channel { url: url.map(String::from), ..Channel::empty("team", Kind::Webhook, "Team chat") };
        // Saved: the URL goes to the Keychain, notify.json keeps its host, the log never sees it.
        let (list, _, writes, audit) = commands::apply_channels(&[], vec![hook(Some("https://discord.com/api/webhooks/1234/s3cr3t-token/slack"))], &none, &[]).unwrap();
        assert_eq!((list[0].url.as_deref(), list[0].url_host.as_deref(), list[0].saved.clone()), (None, Some("discord.com"), vec!["url".to_string()]));
        assert_eq!(writes, [(secret_name("team", "url"), Some("https://discord.com/api/webhooks/1234/s3cr3t-token/slack".to_string()))]);
        assert_eq!(audit, ["Added Team chat: discord.com · URL saved (kept secret)"]);
        let json = serde_json::to_string(&NotifySettings { channels: list.clone(), ..Default::default() }).unwrap();
        assert!(!json.contains("s3cr3t") && json.contains("\"urlHost\":\"discord.com\""), "{json}");
        // Another URL on the same host: "URL changed (kept secret)". Another host: old → new host.
        let before = list;
        let (_, _, _, audit) = commands::apply_channels(&before, vec![Channel { saved: vec![], ..hook(Some("https://discord.com/api/webhooks/9/other")) }], &none, &[]).unwrap();
        assert_eq!(audit, ["Team chat: URL changed (kept secret)"]);
        let (list, _, _, audit) = commands::apply_channels(&before, vec![hook(Some("https://hooks.slack.com/services/T/B/x"))], &none, &[]).unwrap();
        assert_eq!(audit, ["Team chat: URL host discord.com → hooks.slack.com"]);
        assert!(audit.iter().all(|l| !l.contains("services/T")));
        assert_eq!(list[0].url_host.as_deref(), Some("hooks.slack.com"));
        // Unchanged (the UI sends no URL back): the saved one stays, nothing is written or logged.
        let (list, _, writes, audit) = commands::apply_channels(&before, vec![Channel { url_host: Some("discord.com".into()), ..hook(None) }], &none, &[]).unwrap();
        assert!(writes.is_empty() && audit.is_empty());
        assert_eq!((list[0].saved.clone(), list[0].url_host.as_deref()), (vec!["url".to_string()], Some("discord.com")));
        // A dead-man's switch is the same, and needs a URL.
        let hb = Channel::empty("dms", Kind::Heartbeat, "Dead-man's switch");
        assert!(commands::apply_channels(&[], vec![hb.clone()], &none, &[]).unwrap_err().contains("enter the check's URL"));
        let (list, _, _, _) = commands::apply_channels(&[], vec![Channel { url: Some("https://hc-ping.com/5c1e-uuid".into()), ..hb }], &none, &[]).unwrap();
        assert_eq!((list[0].url_host.as_deref(), list[0].target().as_str()), (Some("hc-ping.com"), "healthchecks.io"));
        assert!(commands::apply_channels(&[], vec![hook(Some("http://hooks.example.com/x"))], &none, &[]).unwrap_err().contains("Use https for hooks.example.com"));
        // Settings types the URL into its Keychain field ("team.url"), like a token: the same checks.
        let typed = HashMap::from([("team.url".to_string(), "https://hooks.slack.com/services/T/B/x".to_string())]);
        let (list, _, writes, audit) = commands::apply_channels(&[], vec![hook(None)], &typed, &[]).unwrap();
        assert_eq!((list[0].url_host.as_deref(), list[0].saved.clone()), (Some("hooks.slack.com"), vec!["url".to_string()]));
        assert_eq!(writes, [(secret_name("team", "url"), Some("https://hooks.slack.com/services/T/B/x".to_string()))]);
        assert_eq!(audit, ["Added Team chat: hooks.slack.com · URL saved (kept secret)"]);
        let plain = HashMap::from([("team.url".to_string(), "http://hooks.example.com/x".to_string())]);
        assert!(commands::apply_channels(&[], vec![hook(None)], &plain, &[]).unwrap_err().contains("Use https for hooks.example.com"));
    }

    #[test]
    fn an_old_settings_file_moves_its_urls_to_the_keychain() {
        let mut s = NotifySettings {
            channels: vec![Channel { url: Some("https://hc-ping.com/5c1e".into()), ..Channel::empty("dms", Kind::Heartbeat, "Dead-man's switch") }],
            ..Default::default()
        };
        let stored = std::sync::Mutex::new(Vec::new());
        assert!(migrate_urls(&mut s, |name, v| {
            stored.lock().unwrap().push((name.to_string(), v.to_string()));
            Ok(())
        }));
        assert_eq!(stored.into_inner().unwrap(), [(secret_name("dms", "url"), "https://hc-ping.com/5c1e".to_string())]);
        assert_eq!((s.channels[0].url.as_deref(), s.channels[0].url_host.as_deref(), s.channels[0].saved.clone()), (None, Some("hc-ping.com"), vec!["url".to_string()]));
        assert!(!migrate_urls(&mut s, |_, _| Ok(())), "once");
    }

    #[test]
    fn settings_change_audited() {
        let a = NotifySettings::default();
        let b = NotifySettings { critical_only: true, quiet_hours: true, quiet_tz: "America/New_York".into(), repeat_minutes: 30, disk: false, ..Default::default() };
        assert_eq!(
            commands::settings_diff(&a, &b),
            ["Critical alerts only: off → on", "Disk: on → off", "Quiet hours: off → 22:00 to 07:00 · America/New_York", "Repeat critical alerts: 1 h → 30 min"]
        );
        assert!(commands::settings_diff(&a, &a).is_empty(), "nothing changed, nothing logged");
        let c = NotifySettings { quiet_tz: "Europe/Paris".into(), ..b.clone() };
        assert_eq!(commands::settings_diff(&b, &c), ["Quiet hours: 22:00 to 07:00 · America/New_York → 22:00 to 07:00 · Europe/Paris"]);
    }

    #[test]
    fn a_channel_test_is_allowed_once_every_ten_seconds() {
        let n = Notifier::new(None);
        assert!(n.may_test("phone@mac", 1_000));
        assert!(!n.may_test("phone@mac", 9_000));
        assert!(n.may_test("phone@cell1", 9_000), "per channel and sender");
        assert!(n.may_test("phone@mac", 11_000));
    }

    fn target(title: &str) -> Target {
        Target { title: title.into(), kind: "alerts".into(), params: json!({}) }
    }

    #[test]
    fn a_click_opens_its_own_notification_once() {
        let mut p = Pending::default();
        p.note(1, target("cell3 offline"), 1000, true);
        p.note(2, target("gx10-0000 disk 85%"), 2000, true);
        assert_eq!(p.clicked(1).unwrap().title, "cell3 offline", "the older one, clicked in Notification Center");
        assert_eq!(p.clicked(1), None, "followed once");
        // Coming to the front later offers the latest one that wasn't clicked.
        assert_eq!(p.take(3000).unwrap().title, "gx10-0000 disk 85%");
        assert_eq!(p.take(3001), None, "offered once");
    }

    #[test]
    fn coming_back_offers_the_latest_notification_within_ten_minutes_only() {
        let mut p = Pending::default();
        p.note(7, target("cell3 offline"), 0, true);
        assert_eq!(p.take(PENDING_MS + 1), None, "too long ago: nothing is offered");
        p.note(8, target("cell3 offline"), 0, true);
        assert!(p.clicked(8).is_some());
        assert_eq!(p.take(10), None, "a clicked notification isn't offered again on focus");
        // Shown while SparkBox was in front: clickable, but not offered when coming back.
        p.note(9, target("gx10-0000 disk 85%"), 0, false);
        assert_eq!(p.take(10), None);
        assert!(p.clicked(9).is_some());
    }
}
