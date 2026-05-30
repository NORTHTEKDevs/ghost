//! Reflection ring buffer for VLM failure feedback.
//!
//! Keeps the last N `(obs_hash, action, outcome)` entries per session.
//! On a grounding/action failure, [`ReflectionBuffer::failure_hint`] builds
//! a compact negative hint string that is prepended to the next VLM prompt,
//! steering the model away from repeating the same mistake.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Outcome of an action attempt recorded in the buffer.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionOutcome {
    /// The action succeeded (or appeared to succeed).
    Ok,
    /// The action failed with a reason.
    Failed(String),
}

/// A single recorded entry: what the screen looked like, what was done, what happened.
#[derive(Debug, Clone)]
pub struct ReflectionEntry {
    /// Hash of the screen observation (e.g. downsampled BGRA or element snapshot).
    pub obs_hash: u64,
    /// Human-readable description of the attempted action, e.g. "click (320,240) for 'Submit'".
    pub action: String,
    /// Outcome of the attempt.
    pub outcome: ActionOutcome,
}

/// Bounded ring buffer of [`ReflectionEntry`] records.
///
/// `CAPACITY` entries are kept; oldest entries are evicted when full.
pub struct ReflectionBuffer {
    entries: VecDeque<ReflectionEntry>,
    capacity: usize,
}

impl ReflectionBuffer {
    /// Default capacity (5 entries, matching the plan spec).
    pub const DEFAULT_CAPACITY: usize = 5;

    /// Create a new buffer with `capacity` slots.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Create a buffer with [`Self::DEFAULT_CAPACITY`].
    pub fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }

    /// Push a new entry, evicting the oldest if at capacity.
    pub fn push(&mut self, entry: ReflectionEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Convenience: record a failure.
    pub fn record_failure(&mut self, obs_hash: u64, action: impl Into<String>, reason: impl Into<String>) {
        self.push(ReflectionEntry {
            obs_hash,
            action: action.into(),
            outcome: ActionOutcome::Failed(reason.into()),
        });
    }

    /// Convenience: record a success.
    pub fn record_success(&mut self, obs_hash: u64, action: impl Into<String>) {
        self.push(ReflectionEntry {
            obs_hash,
            action: action.into(),
            outcome: ActionOutcome::Ok,
        });
    }

    /// Returns the number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the buffer contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Build a negative hint string summarising recent failures.
    ///
    /// Returns `None` if there are no failures in the buffer (empty or all successes).
    /// The string is prepended to the VLM prompt to steer it away from repeating mistakes.
    ///
    /// Format example:
    /// ```text
    /// [Reflection - previous failed attempts, do not repeat]:
    /// - clicked (320, 240) for 'Submit' → not found
    /// - clicked (400, 300) for 'Submit' → element not visible
    /// ```
    pub fn failure_hint(&self) -> Option<String> {
        let failures: Vec<&ReflectionEntry> = self
            .entries
            .iter()
            .filter(|e| matches!(e.outcome, ActionOutcome::Failed(_)))
            .collect();

        if failures.is_empty() {
            return None;
        }

        let mut lines = vec!["[Reflection - previous failed attempts, do not repeat]:".to_string()];
        for entry in &failures {
            let reason = match &entry.outcome {
                ActionOutcome::Failed(r) => r.as_str(),
                ActionOutcome::Ok => unreachable!(),
            };
            lines.push(format!("- {} → {}", entry.action, reason));
        }
        Some(lines.join("\n"))
    }

    /// Snapshot the current entries (newest-first) for diagnostics.
    pub fn entries(&self) -> impl Iterator<Item = &ReflectionEntry> {
        self.entries.iter().rev()
    }
}

/// Hash a string observation (e.g. element name + rect) into a u64 obs_hash.
pub fn hash_obs(obs: &str) -> u64 {
    let mut h = DefaultHasher::new();
    obs.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Capacity and eviction ---

    #[test]
    fn buffer_starts_empty() {
        let buf = ReflectionBuffer::new(3);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn push_fills_to_capacity() {
        let mut buf = ReflectionBuffer::new(3);
        for i in 0..3 {
            buf.record_failure(i, format!("action {i}"), "reason");
        }
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn push_beyond_capacity_evicts_oldest() {
        let mut buf = ReflectionBuffer::new(3);
        buf.record_failure(0, "action 0", "fail0");
        buf.record_failure(1, "action 1", "fail1");
        buf.record_failure(2, "action 2", "fail2");
        buf.record_failure(3, "action 3", "fail3"); // evicts "action 0"
        assert_eq!(buf.len(), 3);
        // Newest-first iteration: action 3, 2, 1 — action 0 gone.
        let actions: Vec<&str> = buf.entries().map(|e| e.action.as_str()).collect();
        assert_eq!(actions, vec!["action 3", "action 2", "action 1"]);
    }

    #[test]
    fn eviction_happens_exactly_at_capacity() {
        let mut buf = ReflectionBuffer::new(2);
        buf.record_failure(0, "a", "f");
        buf.record_failure(1, "b", "f");
        assert_eq!(buf.len(), 2);
        buf.record_failure(2, "c", "f"); // evicts "a"
        assert_eq!(buf.len(), 2);
        let actions: Vec<&str> = buf.entries().map(|e| e.action.as_str()).collect();
        assert_eq!(actions, vec!["c", "b"]);
    }

    #[test]
    fn capacity_one_always_keeps_only_last() {
        let mut buf = ReflectionBuffer::new(1);
        buf.record_failure(0, "first", "f");
        buf.record_failure(1, "second", "f");
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.entries().next().unwrap().action, "second");
    }

    // --- Hint string formatting ---

    #[test]
    fn hint_is_none_when_empty() {
        let buf = ReflectionBuffer::new(5);
        assert!(buf.failure_hint().is_none());
    }

    #[test]
    fn hint_is_none_when_all_successes() {
        let mut buf = ReflectionBuffer::new(5);
        buf.record_success(0, "click (100,200) for 'OK'");
        buf.record_success(1, "click (300,400) for 'Cancel'");
        assert!(buf.failure_hint().is_none());
    }

    #[test]
    fn hint_contains_failure_action_and_reason() {
        let mut buf = ReflectionBuffer::new(5);
        buf.record_failure(42, "click (320, 240) for 'Submit'", "not found");
        let hint = buf.failure_hint().unwrap();
        assert!(hint.contains("click (320, 240) for 'Submit'"));
        assert!(hint.contains("not found"));
        assert!(hint.contains("[Reflection"));
    }

    #[test]
    fn hint_excludes_successes_includes_only_failures() {
        let mut buf = ReflectionBuffer::new(5);
        buf.record_success(0, "click (100,200) for 'OK'");
        buf.record_failure(1, "click (400, 300) for 'Submit'", "element not visible");
        buf.record_success(2, "scroll down");
        let hint = buf.failure_hint().unwrap();
        assert!(!hint.contains("'OK'"), "successes should not appear in hint");
        assert!(hint.contains("'Submit'"), "failures should appear in hint");
        assert!(!hint.contains("scroll down"), "successes should not appear in hint");
    }

    #[test]
    fn hint_format_is_multiline_with_header() {
        let mut buf = ReflectionBuffer::new(5);
        buf.record_failure(1, "click (100, 200) for 'A'", "not found");
        buf.record_failure(2, "click (300, 400) for 'B'", "timed out");
        let hint = buf.failure_hint().unwrap();
        let lines: Vec<&str> = hint.lines().collect();
        assert!(lines[0].contains("[Reflection"));
        assert!(lines.iter().any(|l| l.contains("'A'") && l.contains("not found")));
        assert!(lines.iter().any(|l| l.contains("'B'") && l.contains("timed out")));
    }

    #[test]
    fn hint_after_eviction_only_shows_recent_failures() {
        let mut buf = ReflectionBuffer::new(2);
        buf.record_failure(0, "old action", "old error");
        buf.record_failure(1, "recent 1", "err1");
        buf.record_failure(2, "recent 2", "err2"); // evicts "old action"
        let hint = buf.failure_hint().unwrap();
        assert!(!hint.contains("old action"), "evicted entry should not appear in hint");
        assert!(hint.contains("recent 1"));
        assert!(hint.contains("recent 2"));
    }

    // --- obs_hash helper ---

    #[test]
    fn same_string_gives_same_hash() {
        assert_eq!(hash_obs("Submit button at (0,0,100,50)"), hash_obs("Submit button at (0,0,100,50)"));
    }

    #[test]
    fn different_strings_give_different_hashes() {
        assert_ne!(hash_obs("button A"), hash_obs("button B"));
    }
}
