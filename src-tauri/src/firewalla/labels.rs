//! What the owner says is plugged into each port of the Switch X, because nothing else can say
//! for certain: LLDP hears a whole LAN segment and the cloud has no per-port device list. The
//! labels live in firewalla_ports.json in the app's config folder. Nothing here reaches the switch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The Switch X's front panel: 8 RJ45 ports and 4 SFP+ cages, numbered 1 to 12.
pub const PORTS: u32 = 12;
/// A free-text label, in characters.
const TEXT_MAX: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LabelKind {
    /// One of the nodes: the port then shows that node's own view of its LAN link.
    Node,
    Crs804,
    Gold,
    /// This Mac.
    Mac,
    /// Whatever the owner typed.
    Text,
    /// The owner says nothing is plugged in here.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortLabel {
    pub kind: LabelKind,
    /// The node's id, for `Node`.
    #[serde(default)]
    pub node: Option<String>,
    /// The words, for `Text`.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub set_at_ms: u64,
}

/// Switch id → port → label.
pub type Labels = BTreeMap<String, BTreeMap<u32, PortLabel>>;

/// A label from the UI, checked and stamped: `nodes` are the ids it may name.
pub fn clean(input: &PortLabel, nodes: &[String], now_ms: u64) -> Result<PortLabel, String> {
    let (node, text) = match input.kind {
        LabelKind::Node => {
            let id = input.node.as_deref().filter(|id| nodes.iter().any(|n| n == id));
            (Some(id.ok_or("That node isn't in the inventory.")?.to_string()), None)
        }
        LabelKind::Text => {
            let words: String = input.text.as_deref().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
            let words = words.trim();
            if words.is_empty() {
                return Err("Type what's plugged in.".into());
            }
            if words.chars().count() > TEXT_MAX {
                return Err(format!("Keep it to {TEXT_MAX} characters."));
            }
            (None, Some(words.to_string()))
        }
        _ => (None, None),
    };
    Ok(PortLabel { kind: input.kind, node, text, set_at_ms: now_ms })
}

/// What a label says, for the activity log.
pub fn words(label: Option<&PortLabel>) -> String {
    match label {
        None => "no label".into(),
        Some(l) => match l.kind {
            LabelKind::Node => l.node.clone().unwrap_or_default(),
            LabelKind::Crs804 => "the CRS804".into(),
            LabelKind::Gold => "the Gold".into(),
            LabelKind::Mac => "this Mac".into(),
            LabelKind::Text => format!("“{}”", l.text.as_deref().unwrap_or("")),
            LabelKind::Empty => "empty".into(),
        },
    }
}
