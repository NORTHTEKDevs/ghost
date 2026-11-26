//! `ghost_see mode=delta`: what changed in a window since the agent last
//! looked at it.
//!
//! Until 2026-11-26 the mode read a global UIA mirror that nothing in
//! production ever filled, and ignored the window it was given, so it answered
//! "nothing changed" whatever happened (0/10 deltas after a real change,
//! docs/benches/2026-11-26-stage-8). Each window now keeps its own last
//! snapshot here, keyed by the resolved handle, and a delta is the difference
//! between that snapshot and a fresh walk of the same window.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Windows remembered at once; the least recently read is dropped first. An
/// agent drives one or two windows, and a snapshot is a few hundred elements.
pub const MAX_WINDOWS: usize = 16;

/// An element's identity across two walks: role, name, and which occurrence of
/// that role+name it is in walk order (two "Close" buttons stay distinct).
fn keys(elements: &[Value]) -> Vec<(String, String, usize)> {
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    elements
        .iter()
        .map(|e| {
            let role = e["role"].as_str().unwrap_or_default().to_string();
            let name = e["name"].as_str().unwrap_or_default().to_string();
            let n = seen.entry((role.clone(), name.clone())).or_insert(0);
            let k = (role, name, *n);
            *n += 1;
            k
        })
        .collect()
}

/// Elements only in `new` (added), only in `old` (removed), and in both with
/// different properties - rect, enabled - (updated, as they are now).
pub fn diff(old: &[Value], new: &[Value]) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let old_by_key: HashMap<_, _> = keys(old).into_iter().zip(old.iter()).collect();
    let new_keys = keys(new);
    let new_set: std::collections::HashSet<_> = new_keys.iter().cloned().collect();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    for (k, e) in new_keys.iter().zip(new.iter()) {
        match old_by_key.get(k) {
            None => added.push(e.clone()),
            Some(o) if *o != e => updated.push(e.clone()),
            Some(_) => {}
        }
    }
    let removed = keys(old)
        .into_iter()
        .zip(old.iter())
        .filter(|(k, _)| !new_set.contains(k))
        .map(|(_, e)| e.clone())
        .collect();
    (added, removed, updated)
}

#[derive(Default)]
pub struct DeltaStore {
    /// window key -> (seq, last snapshot), most recently read last.
    entries: Vec<(String, u64, Vec<Value>)>,
}

impl DeltaStore {
    /// Record `now` for `window` and answer the delta the caller asked for.
    ///
    /// The first read of a window, or a `since_seq` that is not this window's
    /// last seq, cannot be diffed: the whole tree comes back as `added` with
    /// `baseline: true`. Otherwise the reply is the change since the window's
    /// last read. `seq` always identifies the snapshot just taken.
    pub fn observe(&mut self, window: &str, since_seq: Option<u64>, now: Vec<Value>) -> Value {
        let prev = self.entries.iter().position(|(k, _, _)| k == window).map(|i| self.entries.remove(i));
        let (seq, added, removed, updated, baseline) = match prev {
            Some((_, last_seq, old)) if since_seq.is_none() || since_seq == Some(last_seq) => {
                let (a, r, u) = diff(&old, &now);
                (last_seq + 1, a, r, u, false)
            }
            Some((_, last_seq, _)) => (last_seq + 1, now.clone(), vec![], vec![], true),
            None => (1, now.clone(), vec![], vec![], true),
        };
        self.entries.push((window.to_string(), seq, now));
        if self.entries.len() > MAX_WINDOWS {
            self.entries.remove(0);
        }
        json!({ "seq": seq, "added": added, "removed": removed, "updated": updated, "baseline": baseline })
    }
}

/// The process-wide store (one MCP server drives one session).
pub fn store() -> &'static Mutex<DeltaStore> {
    static STORE: OnceLock<Mutex<DeltaStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(DeltaStore::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str, left: i32) -> Value {
        json!({ "role": role, "name": name, "left": left, "top": 0, "right": left + 10, "bottom": 10, "enabled": true })
    }

    #[test]
    fn identical_walks_have_no_change() {
        let a = vec![el("button", "OK", 0), el("edit", "Field", 20)];
        assert_eq!(diff(&a, &a.clone()), (vec![], vec![], vec![]));
    }

    #[test]
    fn a_renamed_button_is_one_removed_and_one_added() {
        let old = vec![el("button", "Count 0", 0)];
        let new = vec![el("button", "Count 1", 0)];
        let (a, r, u) = diff(&old, &new);
        assert_eq!((a, r, u), (vec![el("button", "Count 1", 0)], vec![el("button", "Count 0", 0)], vec![]));
    }

    #[test]
    fn a_moved_or_disabled_element_is_updated_as_it_is_now() {
        let old = vec![el("button", "OK", 0)];
        let mut disabled = el("button", "OK", 0);
        disabled["enabled"] = json!(false);
        assert_eq!(diff(&old, &[el("button", "OK", 5)]).2, vec![el("button", "OK", 5)]);
        assert_eq!(diff(&old, &[disabled.clone()]).2, vec![disabled]);
    }

    #[test]
    fn duplicates_are_told_apart_by_occurrence() {
        let old = vec![el("button", "Close", 0), el("button", "Close", 50)];
        let new = vec![el("button", "Close", 0)];
        let (a, r, u) = diff(&old, &new);
        assert_eq!((a, r, u), (vec![], vec![el("button", "Close", 50)], vec![]));
    }

    #[test]
    fn first_read_is_a_baseline_with_everything_added() {
        let mut s = DeltaStore::default();
        let reply = s.observe("hwnd:1", None, vec![el("button", "OK", 0)]);
        assert_eq!(reply["seq"], 1);
        assert_eq!(reply["baseline"], true);
        assert_eq!(reply["added"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn later_reads_report_only_the_change_and_advance_seq() {
        let mut s = DeltaStore::default();
        s.observe("hwnd:1", None, vec![el("button", "Count 0", 0)]);
        let same = s.observe("hwnd:1", Some(1), vec![el("button", "Count 0", 0)]);
        assert_eq!((same["seq"].as_u64(), same["baseline"].as_bool()), (Some(2), Some(false)));
        assert!(same["added"].as_array().unwrap().is_empty() && same["removed"].as_array().unwrap().is_empty());
        let changed = s.observe("hwnd:1", None, vec![el("button", "Count 1", 0)]);
        assert_eq!(changed["added"][0]["name"], "Count 1");
        assert_eq!(changed["removed"][0]["name"], "Count 0");
    }

    #[test]
    fn windows_do_not_see_each_others_changes() {
        let mut s = DeltaStore::default();
        s.observe("hwnd:1", None, vec![el("button", "Count 0", 0)]);
        s.observe("hwnd:2", None, vec![el("button", "Count 0", 0)]);
        s.observe("hwnd:2", None, vec![el("button", "Count 5", 0)]);
        let target = s.observe("hwnd:1", None, vec![el("button", "Count 0", 0)]);
        assert!(target["added"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_stale_since_seq_gets_a_baseline_not_a_wrong_diff() {
        let mut s = DeltaStore::default();
        s.observe("hwnd:1", None, vec![el("button", "A", 0)]);
        s.observe("hwnd:1", None, vec![el("button", "A", 0)]);
        let reply = s.observe("hwnd:1", Some(1), vec![el("button", "A", 0)]);
        assert_eq!(reply["baseline"], true);
        assert_eq!(reply["seq"], 3);
    }

    #[test]
    fn memory_is_bounded() {
        let mut s = DeltaStore::default();
        for i in 0..(MAX_WINDOWS + 5) {
            s.observe(&format!("hwnd:{i}"), None, vec![el("button", "A", 0)]);
        }
        assert_eq!(s.entries.len(), MAX_WINDOWS);
        assert!(s.observe("hwnd:0", None, vec![])["baseline"].as_bool().unwrap(), "oldest was dropped");
    }
}
