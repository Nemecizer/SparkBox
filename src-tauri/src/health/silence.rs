//! Silences: a known issue the owner doesn't want counted or paged, for 1 hour, 1 day,
//! 1 week, until it clears (at most 7 days), or indefinitely (Alertmanager's matchers, with
//! fixed choices). A silence matches an alert by rule and entity, or a whole incident by its
//! root.
//!
//! Node scope covers one alert (its rule and entity) or one incident, for an hour or a day.
//! Anything wider (a rule or an entity everywhere), "until it clears" and a week or longer need
//! Mac scope with Live data and a reason (`needs_mac`, checked in alert_silence).
//!
//! Silencing the same thing again (the same incident, else the same rule and entity) changes
//! the silence it has instead of adding one, and keeps its id. Ending a silence ends every
//! silence on the same alerts, so "Silence ended" is true.
//!
//! Silences are kept per data source (alert-silences.live.json, alert-silences.demo.json):
//! a Demo silence never hides a Live alert. Live silences are pushed to the node agents.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Alert;

/// The only durations there are (the UI offers exactly these).
pub const DURATIONS: [&str; 5] = ["1h", "1d", "1w", "resolved", "never"];
/// "Until it clears" ends after this long at most, cleared or not.
pub const UNTIL_CLEARS_MAX_MS: u64 = 7 * 86_400_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Matcher {
    #[serde(default)]
    pub rule: Option<String>,
    /// An entity key, as `entity_key` writes it ("disk:gx10-0000:/").
    #[serde(default)]
    pub entity: Option<String>,
    /// An incident's root key ("link:cell2/enP2p1s0f1np1").
    #[serde(default)]
    pub incident: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Until {
    At { ms: u64 },
    /// Until nothing it matches is active any more.
    Resolved,
    Never,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Silence {
    pub id: String,
    pub matcher: Matcher,
    pub until: Until,
    pub reason: String,
    pub created_ms: u64,
    /// "Admin on this Mac" (Live) or "Demo".
    pub created_by: String,
    /// live | demo
    pub source: String,
    /// What it silences, as the Alerts window named it: "Root disk on gx10-0000".
    #[serde(default)]
    pub label: String,
    /// The duration chosen: one of DURATIONS.
    #[serde(default)]
    pub duration: String,
    // ── r1-h ──
    /// "Until it clears": when it ends even if it hasn't cleared (7 days on). Older silences
    /// without it end 7 days after they were made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

/// "disk:gx10-0000:/", "port:crs804:qsfp56-dd-1-2", "node:cell3": what a silence names.
pub fn entity_key(a: &Alert) -> String {
    let e = &a.entity;
    let place = e.switch.as_deref().or(e.node.as_deref()).unwrap_or("");
    format!("{}:{place}:{}", e.kind, e.id)
}

/// "for 1 hour", "until it clears", "indefinitely".
pub fn duration_words(d: &str) -> &'static str {
    match d {
        "1h" => "for 1 hour",
        "1d" => "for 1 day",
        "1w" => "for 1 week",
        "resolved" => "until it clears (at most 7 days)",
        _ => "indefinitely",
    }
}

/// One alert (its rule and entity) or one incident: what node scope may silence. A blank field
/// names nothing.
pub fn narrow(m: &Matcher) -> bool {
    let named = |f: &Option<String>| f.as_deref().is_some_and(|v| !v.trim().is_empty());
    named(&m.incident) || (named(&m.rule) && named(&m.entity))
}

/// A silence that needs Mac scope with Live data (Touch ID) and a reason: a rule or an entity
/// everywhere, "until it clears", or a week or longer. One alert or one incident for an hour or
/// a day is node scope, like an acknowledgement.
pub fn needs_mac(m: &Matcher, duration: &str) -> bool {
    !narrow(m) || matches!(duration, "1w" | "never" | "resolved")
}

/// Why a silence with no reason is refused.
pub fn reason_needed(m: &Matcher, duration: &str) -> String {
    match duration {
        "1w" | "never" => "Say why: a silence for a week or longer needs a reason.".into(),
        "resolved" => "Say why: a silence until it clears needs a reason.".into(),
        _ if !narrow(m) => "Say why: a silence of more than one alert needs a reason.".into(),
        _ => "Say why.".into(),
    }
}

/// A new silence, or why not: only the listed durations, a matcher that names something,
/// and a reason for the wide and the long ones (`needs_mac`).
pub fn make(matcher: Matcher, duration: &str, reason: &str, label: &str, now: u64, demo: bool) -> Result<Silence, String> {
    let matcher = matcher.normalized();
    if !DURATIONS.contains(&duration) {
        return Err(format!("A silence lasts 1 hour, 1 day, 1 week, until it clears or indefinitely, not \"{duration}\"."));
    }
    let until = match duration {
        "1h" => Until::At { ms: now + 3_600_000 },
        "1d" => Until::At { ms: now + 86_400_000 },
        "1w" => Until::At { ms: now + 7 * 86_400_000 },
        "resolved" => Until::Resolved,
        _ => Until::Never,
    };
    if matcher.rule.is_none() && matcher.entity.is_none() && matcher.incident.is_none() {
        return Err("A silence has to name an alert or an incident.".into());
    }
    let reason = reason.trim();
    if needs_mac(&matcher, duration) && reason.is_empty() {
        return Err(reason_needed(&matcher, duration));
    }
    if reason.chars().count() > 200 {
        return Err("Keep the reason under 200 characters.".into());
    }
    let max_ms = (until == Until::Resolved).then_some(now + UNTIL_CLEARS_MAX_MS);
    Ok(Silence {
        id: format!("sil-{now:x}-{:04x}", fastrand::u16(..)),
        matcher,
        until,
        reason: reason.to_string(),
        created_ms: now,
        created_by: if demo { "Demo".into() } else { "Admin on this Mac".into() },
        source: if demo { "demo" } else { "live" }.into(),
        label: label.to_string(),
        duration: duration.to_string(),
        max_ms,
    })
}

impl Matcher {
    /// Each field trimmed, and a blank one as none: `{rule, entity: ""}` names no entity, so it's
    /// a rule-wide silence (Mac scope and a reason), not one alert. (A node's agent reads a blank
    /// field as "any".)
    pub fn normalized(self) -> Matcher {
        let field = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Matcher { rule: field(self.rule), entity: field(self.entity), incident: field(self.incident) }
    }

    /// The same thing silenced: the same incident, else the same rule and entity. A second
    /// silence of it replaces the first (a double click, or a longer silence).
    pub fn same_as(&self, other: &Matcher) -> bool {
        match (&self.incident, &other.incident) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.rule == other.rule && self.entity == other.entity,
            _ => false,
        }
    }
}

impl Silence {
    /// Whether it covers `alert`, whose incident is `incident`.
    pub fn covers(&self, alert: &Alert, incident: &str) -> bool {
        let m = &self.matcher;
        m.incident.as_deref().is_none_or(|i| i == incident)
            && m.rule.as_deref().is_none_or(|r| r == alert.rule)
            && m.entity.as_deref().is_none_or(|e| e == entity_key(alert))
    }

    /// A timed silence whose time is up, or "until it clears" after its 7 days.
    pub fn expired(&self, now: u64) -> bool {
        self.ends_ms().is_some_and(|t| t <= now)
    }

    /// When it ends at the latest: its time, "until it clears"'s 7 days; None when indefinite.
    /// (The agents get it as `untilMs`, so they end it too.)
    pub fn ends_ms(&self) -> Option<u64> {
        match self.until {
            Until::At { ms } => Some(ms),
            Until::Resolved => Some(self.max_ms.unwrap_or(self.created_ms + UNTIL_CLEARS_MAX_MS)),
            Until::Never => None,
        }
    }

    /// When it ends, for the old `mutedUntilMs` field (the end of time for open-ended ones).
    pub fn until_ms(&self) -> u64 {
        self.ends_ms().unwrap_or(FOREVER_MS)
    }
}

/// 9999-12-31: "no end" for readers of `mutedUntilMs` that compare it with now.
pub const FOREVER_MS: u64 = 253_402_300_799_000;

/// The two silence files, one per data source.
pub struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    /// Opens the store. The round-2 alert-mutes.json is discarded: it mixed Demo and Live mutes.
    pub fn new(dir: Option<PathBuf>) -> Store {
        if let Some(old) = dir.as_ref().map(|d| d.join("alert-mutes.json")).filter(|p| p.exists()) {
            match std::fs::remove_file(&old) {
                Ok(()) => eprintln!("health: discarded {} (it couldn't tell Demo mutes from Live ones; silence again in Alerts)", old.display()),
                Err(e) => eprintln!("health: couldn't discard {}: {e}", old.display()),
            }
        }
        Store { dir }
    }

    fn path(&self, demo: bool) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(if demo { "alert-silences.demo.json" } else { "alert-silences.live.json" }))
    }

    pub fn load(&self, demo: bool) -> Vec<Silence> {
        let Some(p) = self.path(demo) else { return Vec::new() };
        let list: Vec<Silence> = match crate::util::load_json_or_quarantine::<Vec<Silence>>(&p) {
            Ok(Some(l)) => l,
            _ => Vec::new(),
        };
        // A file only ever holds its own source's silences, and none with a blank field (one
        // couldn't be made now: the agents would read it as every alert of its rule).
        list.into_iter()
            .filter(|s| s.source == if demo { "demo" } else { "live" })
            .filter(|s| {
                let ok = s.matcher == s.matcher.clone().normalized();
                if !ok {
                    eprintln!("health: dropped silence {} ({}): its matcher has a blank field", s.id, s.label);
                }
                ok
            })
            .collect()
    }

    pub fn save(&self, demo: bool, list: &[Silence]) {
        if let Some(p) = self.path(demo) {
            if let Err(e) = write(&p, list) {
                eprintln!("health: couldn't save {}: {e}", p.display());
            }
        }
    }
}

fn write(p: &Path, list: &[Silence]) -> Result<(), String> {
    crate::util::write_atomic(p, &serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?)
}
