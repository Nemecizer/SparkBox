//! The fabric rules, merged into the shipped table by `health::rule::extra_defaults()`.
//!
//! "RoCE on a fabric interface" (`roce-iface`) is one alert per interface, fed by the
//! fabric monitor's rate table (fabric::rates): its severity is the worst counter's, and
//! its message says what the rising counters mean (fixtures/roce-iface.json has the
//! counters, their default lines and the causes; the node agent reads the same file).
//! Cabling and the NCCL transport come from the fabric monitor's assessment
//! (`fabric::coded_readings`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::rule::{CounterTh, EntityKind, Op, Rule, RuleKind, RuleSet};
use super::Severity;
use crate::fabric::rates::{fmt_per_s, IfaceRates};

pub const RULE: &str = "roce-iface";

const TABLE_JSON: &str = include_str!("fixtures/roce-iface.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterDef {
    pub key: String,
    pub label: String,
    /// loss | phy | drop | pause
    pub group: String,
    /// Raises only when the 60 s average is past the line too.
    pub sustained: bool,
    pub warn: Option<f64>,
    pub crit: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GroupDef {
    pub headline: String,
    pub cause: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub counters: Vec<CounterDef>,
    pub groups: BTreeMap<String, GroupDef>,
    /// Which group's cause wins when several rise (PHY errors explain retransmits).
    pub cause_order: Vec<String>,
    /// How long pause frames must run past their line before they're a storm.
    pub pause_storm_s: u64,
}

pub fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(TABLE_JSON).expect("roce-iface.json"))
}

/// The counters of one group ("loss", "phy", "drop", "pause"): what the Fabric verdict
/// and the alert both count.
pub fn group_keys(group: &str) -> Vec<&'static str> {
    table().counters.iter().filter(|c| c.group == group).map(|c| c.key.as_str()).collect()
}

/// The round-2 per-counter rules and the counter each became.
const OLD_RULES: [(&str, &str, Option<f64>, Option<f64>); 9] = [
    ("roce-out-of-sequence", "outOfSequence", Some(0.0), Some(100.0)),
    ("roce-seq-errors", "packetSeqErr", Some(0.0), Some(100.0)),
    ("roce-ack-timeouts", "localAckTimeoutErr", Some(0.0), Some(100.0)),
    ("roce-crc", "rxCrcErrorsPhy", Some(0.0), None),
    ("roce-symbol", "rxSymbolErrPhy", Some(0.0), None),
    ("roce-discards", "rxDiscardsPhy", Some(0.0), None),
    ("roce-out-of-buffer", "rxOutOfBuffer", Some(0.0), None),
    ("roce-pause-tx", "txPause", Some(10_000.0), None),
    ("roce-pause-rx", "rxPause", Some(10_000.0), None),
];

/// A rules.json from before roce-iface: the owner's own lines and switches on the old
/// per-counter rules carry over; the old shipped lines (a warning at 0/s) don't.
pub fn migrate(saved: &RuleSet, counters: &mut [CounterTh]) {
    for (old, key, warn, crit) in OLD_RULES {
        let (Some(r), Some(c)) = (saved.get(old), counters.iter_mut().find(|c| c.key == key)) else { continue };
        if r.warn != warn {
            c.warn = r.warn;
        }
        if r.crit != crit {
            c.crit = r.crit;
        }
        c.enabled = r.enabled;
    }
}

fn counter_lines() -> Vec<CounterTh> {
    table().counters.iter().map(|c| CounterTh { key: c.key.clone(), label: c.label.clone(), warn: c.warn, crit: c.crit, enabled: true }).collect()
}

#[allow(clippy::too_many_arguments)]
fn r(id: &str, title: &str, entity: EntityKind, kind: RuleKind, metric: &str, warn: Option<f64>, for_s: u64, clear_s: u64, message: &str, help: &str) -> Rule {
    Rule {
        id: id.into(),
        area: "fabric".into(),
        title: title.into(),
        short: String::new(),
        entity,
        kind,
        metric: metric.into(),
        op: Op::Gt,
        warn,
        crit: None,
        clear: None,
        for_s,
        crit_for_s: None,
        clear_s,
        enabled: true,
        overrides: BTreeMap::new(),
        message: message.into(),
        unit: String::new(),
        window_s: None,
        notify: true,
        help: help.into(),
        counters: Vec::new(),
        mac_only: false,
    }
}

pub fn default_rules() -> Vec<Rule> {
    use EntityKind::{Cluster, Net, Port};
    use RuleKind::Coded;
    vec![
        Rule {
            unit: "/s".into(),
            short: "RoCE errors".into(),
            counters: counter_lines(),
            ..r(
                RULE,
                "RoCE on a fabric interface",
                Net,
                Coded,
                "roce",
                None,
                0,
                30,
                "RoCE retransmitting · CRC and symbol errors rising · check the optic or cable",
                "One alert per fabric interface, from its RoCE and PHY counters. Retransmits and drops must average past their line for a minute; any CRC or symbol error counts at once. It clears once nothing has grown for its clear time.",
            )
        },
        Rule {
            short: "NCCL on TCP".into(),
            ..r(
            "nccl-tcp",
            "NCCL on TCP sockets",
            Cluster,
            Coded,
            "ncclTcp",
            Some(0.0),
            60,
            120,
            "NCCL is using TCP sockets instead of RDMA",
            "A multi-node model moves under half of its fabric bytes over RDMA while generating, or its NCCL settings disable or misdirect RDMA.",
        )
        },
        Rule {
            short: "Cabling changed".into(),
            ..r(
            "fabric-cabling",
            "Fabric cabling changed",
            Port,
            Coded,
            "cablingMismatch",
            Some(0.0),
            30,
            120,
            "A node is on a different switch port than the learned cabling",
            "SparkBox learns which port each node's fabric cable is in. A swap still links, but breaks port comments and per-port history.",
        )
        },
    ]
}

/// What the RoCE rule makes of one interface's rates.
#[derive(Clone, Debug, PartialEq)]
pub struct Judged {
    pub severity: Severity,
    /// "RoCE retransmitting · CRC and symbol errors rising · check the optic or cable"
    pub message: String,
    /// "check the optic or cable"
    pub cause: String,
    /// The evidence: "Out of sequence 162/s · ACK timeouts 3.4/s · CRC errors 18/s".
    pub detail: String,
    /// The worst counter's rate.
    pub value: f64,
}

/// One interface against the rule's counter lines. `active`: the alert is already raised, so
/// it holds while any loss, PHY or drop counter still grows (and pause frames stay past their
/// line); otherwise a counter has to pass its line (and, if sustained, average past it for a
/// minute, or for pause frames, storm for 30 s). None: nothing to raise or hold, including
/// when there are no counters at all.
pub fn judge(lines: &[CounterTh], rates: &IfaceRates, active: bool) -> Option<Judged> {
    let t = table();
    let mut hits: Vec<(&CounterDef, &CounterTh, f64, Severity)> = Vec::new();
    for line in lines.iter().filter(|l| l.enabled) {
        let Some(def) = t.counters.iter().find(|c| c.key == line.key) else { continue };
        let Some(now) = rates.r10(&def.key) else { continue };
        let warn = line.warn.unwrap_or(f64::INFINITY);
        let lasted = if def.group == "pause" { rates.pause_storm_s.is_some_and(|s| s >= t.pause_storm_s) } else { !def.sustained || rates.r60(&def.key).is_some_and(|m| m > warn) };
        let raise = now > warn && lasted;
        // Held: still growing at all (pause frames are normal at low rates, so only past the line).
        let hold = active && if def.group == "pause" { now > warn } else { now > 0.0 };
        if !(raise || hold) {
            continue;
        }
        let sev = if line.crit.is_some_and(|c| now > c) { Severity::Crit } else { Severity::Warn };
        hits.push((def, line, now, sev));
    }
    if hits.is_empty() {
        return None;
    }
    let severity = hits.iter().map(|h| h.3).max().unwrap_or(Severity::Warn);
    let rising = |g: &str| hits.iter().any(|h| h.0.group == g);
    let cause_group = t.cause_order.iter().find(|g| rising(g)).cloned().unwrap_or_else(|| hits[0].0.group.clone());
    let cause = t.groups.get(&cause_group).map(|g| g.cause.clone()).unwrap_or_default();
    // The headline is what hurts the traffic most; PHY errors, when they're the cause, say why.
    let lead = ["loss", "pause", "drop", "phy"].into_iter().find(|g| rising(g)).unwrap_or("phy");
    let mut parts = Vec::new();
    if lead == "phy" {
        parts.push(phy_words(&hits));
    } else {
        parts.push(t.groups.get(lead).map(|g| g.headline.clone()).unwrap_or_default());
        if rising("phy") {
            parts.push(phy_words(&hits));
        }
    }
    if !parts.iter().any(|p| p.eq_ignore_ascii_case(&cause)) {
        parts.push(cause.clone());
    }
    let mut sorted = hits.clone();
    sorted.sort_by(|a, b| b.2.total_cmp(&a.2));
    let detail = sorted.iter().map(|h| format!("{} {}", h.1.label, fmt_per_s(h.2))).collect::<Vec<_>>().join(" · ");
    Some(Judged { severity, message: parts.join(" · "), cause, detail, value: sorted[0].2 })
}

/// "CRC and symbol errors rising", "CRC errors rising", "PHY errors rising".
fn phy_words(hits: &[(&CounterDef, &CounterTh, f64, Severity)]) -> String {
    let crc = hits.iter().any(|h| h.0.key == "rxCrcErrorsPhy");
    let sym = hits.iter().any(|h| h.0.key.to_ascii_lowercase().contains("symbol"));
    match (crc, sym) {
        (true, true) => "CRC and symbol errors rising".into(),
        (true, false) => "CRC errors rising".into(),
        (false, true) => "Symbol errors rising".into(),
        _ => "PHY errors rising".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::rates::CounterRate;

    #[test]
    fn the_fabric_rules_are_well_formed_and_unique() {
        let rules = default_rules();
        let mut ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), rules.len());
        for r in &rules {
            assert_eq!(r.area, "fabric");
            assert!(!r.message.is_empty() && !r.help.is_empty(), "{}", r.id);
        }
        let roce = rules.iter().find(|r| r.id == RULE).unwrap();
        assert_eq!((roce.for_s, roce.clear_s), (0, 30));
        let line = |k: &str| roce.counters.iter().find(|c| c.key == k).map(|c| (c.warn, c.crit)).unwrap();
        assert_eq!(line("outOfSequence"), (Some(1.0), Some(100.0)), "out of sequence warns past 1/s, sustained");
        assert_eq!(line("rxCrcErrorsPhy"), (Some(0.0), Some(10.0)), "any CRC error");
        assert_eq!(line("txPause"), (Some(10_000.0), None));
        // ECN and CNPs are DCQCN at work: never an alert.
        assert!(roce.counters.iter().all(|c| !c.key.starts_with("ecn") && !c.key.starts_with("cnp")));
        let set = RuleSet { version: 1, rules };
        set.validate().unwrap();
    }

    #[test]
    fn the_roce_iface_cases_run_the_same_in_the_app() {
        // agent_test.py runs the same cases through the node agent.
        let fixture: serde_json::Value = serde_json::from_str(TABLE_JSON).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let mut rates = IfaceRates { node: "cell2".into(), node_name: "cell2".into(), iface: "enP2p1s0f1np1".into(), half: 1, up: true, ..Default::default() };
            for (k, v) in case["rates"].as_object().unwrap() {
                let (r10, r60) = (v[0].as_f64(), v[1].as_f64());
                rates.counters.insert(k.clone(), CounterRate { total: Some(1), r10, r60 });
            }
            rates.pause_storm_s = case["pauseStormS"].as_u64();
            let got = judge(&counter_lines(), &rates, case["active"].as_bool().unwrap());
            let want = &case["expect"];
            match got {
                None => assert!(want.is_null(), "{}: nothing raised, wanted {want}", case["name"]),
                Some(j) => {
                    assert!(!want.is_null(), "{}: raised {j:?}", case["name"]);
                    assert_eq!(serde_json::to_value(j.severity).unwrap(), want["severity"], "{}", case["name"]);
                    assert_eq!(j.message, want["message"].as_str().unwrap(), "{}", case["name"]);
                }
            }
        }
    }

    #[test]
    fn saved_round_2_rules_migrate_to_counter_lines() {
        let mut saved = RuleSet::defaults();
        saved.rules.retain(|r| r.id != RULE);
        let old = |id: &str, warn: Option<f64>, enabled: bool| Rule { id: id.into(), warn, enabled, ..default_rules()[1].clone() };
        saved.rules.push(old("roce-out-of-sequence", Some(0.0), true)); // the old shipped line: not carried over
        saved.rules.push(old("roce-crc", Some(2.0), true)); // the owner's own line
        saved.rules.push(old("roce-pause-rx", Some(10_000.0), false)); // switched off
        saved.rules.push(old("roce-something-new", Some(5.0), true)); // unknown: ignored
        let merged = RuleSet::merged_onto_defaults(&saved);
        let roce = merged.get(RULE).unwrap();
        let c = |k: &str| roce.counters.iter().find(|c| c.key == k).unwrap().clone();
        assert_eq!(c("outOfSequence").warn, Some(1.0));
        assert_eq!(c("rxCrcErrorsPhy").warn, Some(2.0));
        assert!(!c("rxPause").enabled);
        assert!(merged.get("roce-crc").is_none() && merged.get("roce-something-new").is_none(), "the old rules leave the list");
    }
}
