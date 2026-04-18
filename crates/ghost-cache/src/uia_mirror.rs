//! UIA mirror: snapshot types, in-memory mirror, delta API.
//!
//! Event subscription (Structure/PropertyChanged handlers) is stubbed - the
//! `apply_mutation` entry-point lets the walker-driven snapshot path drive
//! updates until the COM handlers are wired in a follow-up.

use crate::error::CacheError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub type Seq = u64;
const SNAPSHOT_HISTORY: usize = 8;

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

    #[cfg(any(test, feature = "test-hooks"))]
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

struct CacheInner {
    current: Snapshot,
    history: VecDeque<Snapshot>,
    stats: CacheStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub mutations_applied: u64,
    pub snapshots_served: u64,
    pub deltas_served: u64,
    pub history_hits: u64,
    pub history_misses: u64,
}

pub struct UiaCache {
    inner: Arc<Mutex<CacheInner>>,
}

impl UiaCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheInner {
                current: Snapshot::default(),
                history: VecDeque::with_capacity(SNAPSHOT_HISTORY),
                stats: CacheStats::default(),
            })),
        }
    }

    pub fn seq(&self) -> Seq {
        self.inner.lock().unwrap().current.seq
    }

    pub fn snapshot(&self, _window: Option<&str>, _since_seq: Option<Seq>) -> Result<Snapshot, CacheError> {
        let mut inner = self.inner.lock().unwrap();
        inner.stats.snapshots_served += 1;
        Ok(inner.current.clone())
    }

    pub fn snapshot_delta(&self, _window: Option<&str>, since_seq: Option<Seq>) -> Result<SnapshotDelta, CacheError> {
        let mut inner = self.inner.lock().unwrap();
        inner.stats.deltas_served += 1;
        let current = inner.current.clone();
        let Some(since) = since_seq else {
            return Ok(SnapshotDelta {
                seq: current.seq,
                added: current.nodes.clone(),
                removed: vec![],
                updated: vec![],
            });
        };
        if since == current.seq {
            inner.stats.history_hits += 1;
            return Ok(SnapshotDelta { seq: current.seq, added: vec![], removed: vec![], updated: vec![] });
        }
        if let Some(prev) = inner.history.iter().find(|s| s.seq == since) {
            inner.stats.history_hits += 1;
            return Ok(prev.diff(&current));
        }
        inner.stats.history_misses += 1;
        Ok(SnapshotDelta { seq: current.seq, added: current.nodes.clone(), removed: vec![], updated: vec![] })
    }

    /// Replace the current snapshot wholesale; archive the prior into history.
    /// Wired by the walker-driven refresh path and, in the future, by COM event handlers.
    pub fn apply_snapshot(&self, mut new_snap: Snapshot) {
        let mut inner = self.inner.lock().unwrap();
        let prior = std::mem::take(&mut inner.current);
        new_snap.seq = prior.seq + 1;
        if inner.history.len() >= SNAPSHOT_HISTORY {
            inner.history.pop_front();
        }
        inner.history.push_back(prior);
        inner.current = new_snap;
        inner.stats.mutations_applied += 1;
    }

    /// Test-only single-node append. Bumps seq by 1.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn apply_mutation_for_test(&self, node: ElementNode) {
        let mut inner = self.inner.lock().unwrap();
        let mut next = inner.current.clone();
        next.seq += 1;
        next.nodes.push(node);
        let prior = std::mem::replace(&mut inner.current, next);
        if inner.history.len() >= SNAPSHOT_HISTORY {
            inner.history.pop_front();
        }
        inner.history.push_back(prior);
        inner.stats.mutations_applied += 1;
    }

    pub fn stats(&self) -> CacheStats {
        self.inner.lock().unwrap().stats.clone()
    }

    pub fn invalidate(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.current = Snapshot::default();
        inner.history.clear();
    }
}

impl Default for UiaCache {
    fn default() -> Self {
        Self::new()
    }
}

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
    }

    #[test]
    fn diff_detects_updates_by_runtime_id_when_props_change() {
        let a = Snapshot { seq: 1, nodes: vec![node_full("1", "A", (0, 0, 10, 10))] };
        let b = Snapshot { seq: 2, nodes: vec![node_full("1", "A-renamed", (0, 0, 10, 10))] };
        let d = a.diff(&b);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.updated.len(), 1);
    }

    #[test]
    fn cache_applies_mutation_and_bumps_seq() {
        let cache = UiaCache::new();
        let before = cache.seq();
        cache.apply_mutation_for_test(ElementNode::dummy());
        let after = cache.seq();
        assert!(after > before);
    }

    #[test]
    fn snapshot_returns_noop_delta_when_seq_matches() {
        let cache = UiaCache::new();
        let s1 = cache.snapshot(None, None).unwrap();
        let delta = cache.snapshot_delta(None, Some(s1.seq)).unwrap();
        assert!(delta.added.is_empty() && delta.removed.is_empty() && delta.updated.is_empty());
        assert_eq!(delta.seq, s1.seq);
    }

    #[test]
    fn snapshot_delta_uses_history_after_mutation() {
        let cache = UiaCache::new();
        let s1 = cache.snapshot(None, None).unwrap();
        cache.apply_mutation_for_test(ElementNode::dummy());
        let delta = cache.snapshot_delta(None, Some(s1.seq)).unwrap();
        assert_eq!(delta.added.len(), 1);
    }
}
