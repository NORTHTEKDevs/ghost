//! UIA mirror snapshot types + delta computation.

use serde::{Deserialize, Serialize};

pub type Seq = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementNode {
    pub runtime_id: String,
    pub name: String,
    pub role: String,
    pub rect: (i32, i32, i32, i32),
    pub ax_checksum: [u8; 16],
    pub parent_runtime_id: Option<String>,
}

impl ElementNode {
    pub fn compute_checksum(runtime_id: &str, name: &str, role: &str, rect: (i32, i32, i32, i32)) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(runtime_id.as_bytes());
        hasher.update(name.as_bytes());
        hasher.update(role.as_bytes());
        hasher.update(&rect.0.to_le_bytes());
        hasher.update(&rect.1.to_le_bytes());
        hasher.update(&rect.2.to_le_bytes());
        hasher.update(&rect.3.to_le_bytes());
        let full = hasher.finalize();
        let bytes = full.as_bytes();
        let mut out = [0u8; 16];
        out.copy_from_slice(&bytes[..16]);
        out
    }

    #[cfg(test)]
    pub fn dummy() -> Self {
        Self {
            runtime_id: "dummy".into(),
            name: "dummy".into(),
            role: "button".into(),
            rect: (0, 0, 1, 1),
            ax_checksum: [0; 16],
            parent_runtime_id: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: Seq,
    pub nodes: Vec<ElementNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotDelta {
    pub seq: Seq,
    pub added: Vec<ElementNode>,
    pub removed: Vec<ElementNode>,
    pub updated: Vec<ElementNode>,
}

impl Snapshot {
    pub fn diff(&self, other: &Snapshot) -> SnapshotDelta {
        use std::collections::HashMap;
        let map_a: HashMap<&str, &ElementNode> =
            self.nodes.iter().map(|n| (n.runtime_id.as_str(), n)).collect();
        let map_b: HashMap<&str, &ElementNode> =
            other.nodes.iter().map(|n| (n.runtime_id.as_str(), n)).collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut updated = Vec::new();

        for (id, node) in &map_b {
            match map_a.get(id) {
                None => added.push((*node).clone()),
                Some(prev) if prev.ax_checksum != node.ax_checksum => {
                    updated.push((*node).clone())
                }
                _ => {}
            }
        }
        for (id, node) in &map_a {
            if !map_b.contains_key(id) {
                removed.push((*node).clone());
            }
        }

        SnapshotDelta {
            seq: other.seq,
            added,
            removed,
            updated,
        }
    }
}

/// In-memory mirror state. The event-subscription machinery is wired in Task 7.
pub struct UiaCache;

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, name: &str) -> ElementNode {
        node_full(id, name, (0, 0, 10, 10))
    }

    fn node_full(id: &str, name: &str, rect: (i32, i32, i32, i32)) -> ElementNode {
        let checksum = ElementNode::compute_checksum(id, name, "button", rect);
        ElementNode {
            runtime_id: id.into(),
            name: name.into(),
            role: "button".into(),
            rect,
            ax_checksum: checksum,
            parent_runtime_id: None,
        }
    }

    #[test]
    fn diff_empty_snapshots_is_empty_delta() {
        let a = Snapshot::default();
        let b = Snapshot::default();
        let d = a.diff(&b);
        assert!(d.added.is_empty() && d.removed.is_empty() && d.updated.is_empty());
    }

    #[test]
    fn diff_detects_added_and_removed() {
        let a = Snapshot { seq: 1, nodes: vec![node("1", "A"), node("2", "B")] };
        let b = Snapshot { seq: 2, nodes: vec![node("2", "B"), node("3", "C")] };
        let d = a.diff(&b);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.added[0].runtime_id, "3");
        assert_eq!(d.removed[0].runtime_id, "1");
    }

    #[test]
    fn diff_detects_updates_by_runtime_id_when_props_change() {
        let a = Snapshot { seq: 1, nodes: vec![node_full("1", "A", (0, 0, 10, 10))] };
        let b = Snapshot { seq: 2, nodes: vec![node_full("1", "A-renamed", (0, 0, 10, 10))] };
        let d = a.diff(&b);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.updated.len(), 1);
    }
}
