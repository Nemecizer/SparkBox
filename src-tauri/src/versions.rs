//! Exact reported-version comparisons, shared by Fleet and multi-node preflight.
//! A match is not a compatibility guarantee; unknown never counts as agreement.
use crate::model::{LinkStatus, NodeSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Facts {
    pub node: String,
    pub name: String,
    #[serde(default)]
    pub hardware: Option<String>,
    #[serde(default)]
    pub unavailable: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Matching,
    Mismatched,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Value {
    pub node: String,
    pub value: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub key: String,
    pub label: String,
    pub state: State,
    pub values: Vec<Value>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub state: State,
    pub rows: Vec<Row>,
}

const CORE: &[(&str, &str)] = &[
    ("dgxOta", "DGX OS (OTA)"),
    ("kernel", "Kernel"),
    ("driver", "NVIDIA driver"),
    ("cuda", "CUDA (driver maximum)"),
    ("toolkit", "NVIDIA container toolkit"),
    ("collector", "Collector build"),
    ("store", "Docker image store"),
    ("imageId", "Serving image ID"),
    ("nccl", "NCCL (declared)"),
];
const EXTRA: &[(&str, &str)] = &[("os", "OS"), ("bios", "BIOS"), ("cx7", "ConnectX-7 firmware"), ("docker", "Docker"), ("ntp", "Clock synced")];

fn clean(s: Option<&String>) -> Option<String> {
    s.map(|s| s.trim()).filter(|s| !s.is_empty() && !["unknown", "n/a", "none", "—", "(hidden)"].contains(&s.to_ascii_lowercase().as_str())).map(String::from)
}

fn image_id(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn verdict(values: &[Value]) -> State {
    let known: BTreeSet<_> = values.iter().filter_map(|v| v.value.as_ref()).collect();
    if known.len() > 1 {
        State::Mismatched
    } else if values.len() >= 2 && known.len() == 1 && values.iter().all(|v| v.value.is_some()) {
        State::Matching
    } else {
        State::Unknown
    }
}

fn worst(states: impl Iterator<Item = State>) -> State {
    let states: Vec<_> = states.collect();
    if states.contains(&State::Mismatched) {
        State::Mismatched
    } else if states.is_empty() || states.contains(&State::Unknown) {
        State::Unknown
    } else {
        State::Matching
    }
}

pub fn compare(facts: &[Facts], extras: bool) -> Report {
    let mut rows = Vec::new();
    for &(key, label) in CORE.iter().chain(EXTRA.iter().filter(|_| extras)) {
        let values: Vec<_> = facts
            .iter()
            .map(|f| {
                let value =
                    f.unavailable.is_none().then(|| clean(f.values.get(key).and_then(Option::as_ref))).flatten().filter(|v| key != "imageId" || image_id(v));
                let reason = value.is_none().then(|| f.unavailable.clone().unwrap_or_else(|| "Not reported".into()));
                Value { node: f.node.clone(), value, reason }
            })
            .collect();
        let mut state = verdict(&values);
        let mut note = None;
        if key == "bios" {
            let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for (f, v) in facts.iter().zip(&values) {
                groups.entry(clean(f.hardware.as_ref()).unwrap_or_else(|| format!("unknown:{}", f.node))).or_default().push(v.clone());
            }
            state = worst(groups.values().map(|v| verdict(v)));
            note = Some("Compared only within the same hardware model; a model with one node has no peer to compare.".into());
        }
        if key == "imageId" {
            let stores: Vec<_> = facts
                .iter()
                .map(|f| Value {
                    node: f.node.clone(),
                    value: f.unavailable.is_none().then(|| clean(f.values.get("store").and_then(Option::as_ref))).flatten(),
                    reason: None,
                })
                .collect();
            if verdict(&stores) != State::Matching {
                state = State::Unknown;
                note = Some("Image IDs need the same known Docker image store on at least two nodes before they can be compared.".into());
            }
        }
        rows.push(Row { key: key.into(), label: label.into(), state, values, note });
    }
    Report { state: worst(rows.iter().map(|r| r.state)), rows }
}

/// A fresh snapshot's host and currently running serving-container declarations.
pub fn snapshot_facts(n: &NodeSnapshot) -> Facts {
    let s = &n.system;
    let mut values: BTreeMap<String, Option<String>> = [
        ("os", Some(s.os.clone())),
        ("dgxOta", s.dgx_ota.clone()),
        ("kernel", Some(s.kernel.clone())),
        ("driver", Some(s.driver.clone())),
        ("cuda", Some(s.cuda.clone())),
        ("toolkit", s.container_toolkit.clone()),
        ("collector", s.collector_version.clone()),
        ("store", s.image_store.clone()),
        ("bios", s.bios.clone()),
        ("cx7", s.cx7_fw.clone()),
        ("docker", s.docker.clone()),
        ("ntp", s.ntp_synced.map(|v| if v { "yes" } else { "no" }.into())),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect();
    let serving: Vec<_> = n
        .containers
        .iter()
        .filter(|c| {
            let image = c.labels.get("sparkbox.image").unwrap_or(&c.image).to_ascii_lowercase();
            c.state == "running" && (image.contains("vllm") || image.contains("sglang"))
        })
        .collect();
    let ids: BTreeSet<_> = serving.iter().filter_map(|c| c.image_id.as_ref().filter(|id| image_id(id))).collect();
    let one_image = !serving.is_empty() && ids.len() == 1 && serving.iter().all(|c| c.image_id.as_ref().is_some_and(|id| image_id(id)));
    values.insert("imageId".into(), one_image.then(|| (*ids.first().unwrap()).clone()));
    let declarations: Vec<_> = serving.iter().map(|c| c.env_flags.get("NCCL_VERSION").filter(|s| declared_version(s)).cloned()).collect();
    let nccl: BTreeSet<_> = declarations.iter().flatten().collect();
    values.insert("nccl".into(), (one_image && nccl.len() == 1 && declarations.iter().all(Option::is_some)).then(|| (*nccl.first().unwrap()).clone()));
    Facts {
        node: n.id.clone(),
        name: n.name.clone(),
        hardware: Some(s.model.clone()),
        unavailable: if n.status != LinkStatus::Online {
            Some("Node is offline".into())
        } else if n.stale {
            Some("Stale readings".into())
        } else {
            None
        },
        values,
    }
}

fn declared_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".+_-".contains(&b))
}

pub fn fleet(nodes: &[NodeSnapshot]) -> Report {
    compare(&nodes.iter().map(snapshot_facts).collect::<Vec<_>>(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClusterSnapshot, Container};
    use crate::models::plan::{DeploySpec, Preflight, PreflightSet};

    fn fact(id: &str) -> Facts {
        let mut values: BTreeMap<_, _> = CORE.iter().chain(EXTRA).map(|(k, _)| (k.to_string(), Some("1.2.3".into()))).collect();
        values.insert("imageId".into(), Some(format!("sha256:{}", "a".repeat(64))));
        values.insert("store".into(), Some("containerd".into()));
        Facts { node: id.into(), name: format!("Name {id}"), hardware: Some("MSI".into()), values, ..Default::default() }
    }
    fn row<'a>(report: &'a Report, key: &str) -> &'a Row {
        report.rows.iter().find(|r| r.key == key).unwrap()
    }

    #[test]
    fn matches_require_two_or_more_complete_readings() {
        let a = fact("a");
        let b = fact("b");
        assert_eq!(compare(&[a.clone(), b.clone()], true).state, State::Matching);
        for facts in [vec![], vec![a.clone()]] {
            assert_eq!(compare(&facts, false).state, State::Unknown);
        }
        for missing in [None, Some("".into()), Some("Unknown".into()), Some(" N/A ".into()), Some("—".into())] {
            let mut b = b.clone();
            b.values.insert("driver".into(), missing);
            let report = compare(&[a.clone(), b], false);
            assert_eq!(row(&report, "driver").state, State::Unknown);
            assert_eq!(row(&report, "driver").values[1].value, None);
            assert_eq!(report.state, State::Unknown);
        }
    }
    #[test]
    fn disagreement_needs_no_majority_and_missing_values_remain_visible() {
        let a = fact("a");
        let mut b = fact("b");
        b.values.insert("driver".into(), Some("2.0".into()));
        for facts in [vec![a.clone(), b.clone()], vec![a.clone(), b.clone(), a.clone(), b.clone()]] {
            assert_eq!(row(&compare(&facts, false), "driver").state, State::Mismatched);
        }
        let mut c = fact("c");
        c.unavailable = Some("Stale readings".into());
        let report = compare(&[a, b, c], false);
        let driver = row(&report, "driver");
        assert_eq!(report.state, State::Mismatched);
        assert_eq!(driver.values[2].value, None);
        assert_eq!(driver.values[2].reason.as_deref(), Some("Stale readings"));
        assert_eq!(row(&report, "kernel").state, State::Unknown);
    }
    #[test]
    fn image_ids_need_valid_full_ids_and_the_same_known_store() {
        let a = fact("a");
        let mut b = fact("b");
        b.values.insert("imageId".into(), Some(format!("sha256:{}", "b".repeat(64))));
        assert_eq!(row(&compare(&[a.clone(), b.clone()], false), "imageId").state, State::Mismatched);
        for store in [None, Some("overlay2".into())] {
            b.values.insert("store".into(), store);
            let report = compare(&[a.clone(), b.clone()], false);
            assert_eq!(row(&report, "imageId").state, State::Unknown);
            assert!(row(&report, "imageId").values[1].value.is_some(), "still show the raw ID");
        }
        b = fact("b");
        b.values.insert("imageId".into(), Some("sha256:short".into()));
        assert_eq!(row(&compare(&[a, b], false), "imageId").state, State::Unknown);
    }
    #[test]
    fn bios_is_compared_only_with_peers_of_the_same_hardware() {
        let a = fact("a");
        let b = fact("b");
        let mut c = fact("c");
        let mut d = fact("d");
        for f in [&mut c, &mut d] {
            f.hardware = Some("ASUS".into());
            f.values.insert("bios".into(), Some("other".into()));
        }
        assert_eq!(row(&compare(&[a.clone(), b.clone(), c.clone(), d.clone()], true), "bios").state, State::Matching);
        assert_eq!(row(&compare(&[a.clone(), b.clone(), c.clone()], true), "bios").state, State::Unknown);
        d.values.insert("bios".into(), Some("drift".into()));
        assert_eq!(row(&compare(&[a, b, c, d], true), "bios").state, State::Mismatched);
    }
    #[test]
    fn snapshot_reads_only_unambiguous_running_serving_images_and_declarations() {
        let id = format!("sha256:{}", "a".repeat(64));
        let serving = Container {
            image: "sha256:pinned".into(),
            image_id: Some(id.clone()),
            state: "running".into(),
            labels: [("sparkbox.image".into(), "nvcr.io/nvidia/vllm:tag".into())].into(),
            env_flags: [("NCCL_VERSION".into(), "2.27.3-1".into())].into(),
            ..Default::default()
        };
        let mut n = NodeSnapshot {
            id: "stable-id".into(),
            name: "Renamed node".into(),
            status: LinkStatus::Online,
            containers: vec![serving.clone()],
            ..Default::default()
        };
        let facts = snapshot_facts(&n);
        assert_eq!(facts.node, "stable-id");
        assert_eq!(facts.values["imageId"], Some(id));
        assert_eq!(facts.values["nccl"].as_deref(), Some("2.27.3-1"));
        n.containers.push(Container { state: "exited".into(), image_id: None, ..serving.clone() });
        n.containers.push(Container { image: "nginx".into(), labels: BTreeMap::new(), image_id: None, ..serving.clone() });
        assert!(snapshot_facts(&n).values["nccl"].is_some());
        for invalid in ["(hidden)", "token=secret", "", "2.0\nsecret"] {
            n.containers[0].env_flags.insert("NCCL_VERSION".into(), invalid.into());
            assert!(snapshot_facts(&n).values["nccl"].is_none());
        }
        n.containers = vec![serving.clone(), Container { image_id: Some(format!("sha256:{}", "b".repeat(64))), ..serving.clone() }];
        assert!(snapshot_facts(&n).values["imageId"].is_none());
        assert!(snapshot_facts(&n).values["nccl"].is_none());
        n.containers[1].image_id = serving.image_id.clone();
        n.containers[1].env_flags.clear();
        assert!(snapshot_facts(&n).values["imageId"].is_some());
        assert!(snapshot_facts(&n).values["nccl"].is_none());
        n.stale = true;
        assert_eq!(fleet(&[n.clone(), n.clone()]).state, State::Unknown);
        n.stale = false;
        n.status = LinkStatus::Offline;
        let report = fleet(&[n]);
        assert!(report.rows.iter().all(|r| r.values[0].reason.as_deref() == Some("Node is offline")));
    }
    #[test]
    fn preflight_version_drift_is_advisory_but_image_identity_remains_blocking() {
        let to_preflight = |f: Facts| Preflight {
            node: f.node,
            image_id: f.values["imageId"].clone(),
            store: f.values["store"].clone(),
            versions: f.values,
            ..Default::default()
        };
        let mut a = to_preflight(fact("a"));
        let mut b = to_preflight(fact("b"));
        b.versions.insert("kernel".into(), Some("other".into()));
        let spec = DeploySpec::default();
        let judge = |a: Preflight, b: Preflight| PreflightSet::judge(&spec, vec![a, b], 100);
        let set = judge(a.clone(), b.clone());
        assert_eq!(set.verdict, "ready");
        assert_eq!(set.versions.unwrap().state, State::Mismatched);
        b.image_id = Some(format!("sha256:{}", "b".repeat(64)));
        assert_eq!(judge(a.clone(), b.clone()).verdict, "blocked");
        b.store = Some("overlay2".into());
        let set = judge(a.clone(), b.clone());
        assert_eq!(set.verdict, "blocked");
        assert_eq!(row(&set.versions.unwrap(), "imageId").state, State::Unknown);
        a.demo = true;
        a.versions.clear();
        b.versions.clear();
        let set = judge(a.clone(), b);
        assert_eq!(set.verdict, "demo");
        assert!(PreflightSet::judge(&spec, vec![a], 100).versions.is_none());
    }
    #[test]
    fn browser_report_is_the_shared_engine_result_for_the_unchanged_snapshot_fixture() {
        let snapshot: ClusterSnapshot = serde_json::from_str(include_str!("../../src/dev/fixtures/snapshot.json")).unwrap();
        let expected: Report = serde_json::from_str(include_str!("../../src/dev/fixtures/version-report.json")).unwrap();
        assert_eq!(fleet(&snapshot.nodes), expected);
    }
}
