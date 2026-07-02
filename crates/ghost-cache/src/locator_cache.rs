//! In-session in-memory locator cache.
//!
//! Maps `(role, name)` -> `(left, top, right, bottom)` for elements found
//! during the current session. On lookup, the caller validates the cached
//! rect via `IUIAutomation::ElementFromPoint(center)` and only uses the
//! hit if the element's name/role still match.
//!
//! This is intentionally kept simple (HashMap, no SQLite) to keep the
//! hot-path latency well below 1ms. The SQLite LocatorStore is for
//! cross-session persistence; this cache is per-session ephemeral.
//!
//! Thread-safety: wrapped in a Mutex inside GhostSession; all accesses
//! happen on the COM STA thread (the MCP tokio main thread).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Key for the in-memory locator cache: (hwnd, role, name) triple.
/// `hwnd` is the foreground HWND at lookup time (as isize, 0 = any window).
/// Scoping by HWND prevents cross-window false hits when different windows
/// have elements with the same name/role.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocatorKey {
    pub hwnd: isize,
    pub role: String,
    pub name: String,
}

impl LocatorKey {
    pub fn new(role: impl Into<String>, name: impl Into<String>) -> Self {
        Self { hwnd: 0, role: role.into(), name: name.into() }
    }

    pub fn with_hwnd(hwnd: isize, role: impl Into<String>, name: impl Into<String>) -> Self {
        Self { hwnd, role: role.into(), name: name.into() }
    }
}

/// Result of a cache lookup.
#[derive(Debug, Clone)]
pub struct LocatorHitResult {
    pub rect: (i32, i32, i32, i32),
    /// Center of the cached rect, used for ElementFromPoint validation.
    pub center: (i32, i32),
}

impl LocatorHitResult {
    pub fn from_rect(rect: (i32, i32, i32, i32)) -> Self {
        let cx = (rect.0 + rect.2) / 2;
        let cy = (rect.1 + rect.3) / 2;
        Self { rect, center: (cx, cy) }
    }
}

/// Stats exposed via cache_stats() MCP tool.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LocatorCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub upserts: u64,
}

/// Entries older than this are treated as misses. Point-validation
/// (ElementFromPoint name/role match) can pass on a coincidentally-matching
/// element after a re-render; a TTL bounds how long that window stays open
/// while still serving the hot within-flow case (repeat actions in seconds).
const ENTRY_TTL: std::time::Duration = std::time::Duration::from_secs(30);

struct Inner {
    map: HashMap<LocatorKey, ((i32, i32, i32, i32), std::time::Instant)>,
    stats: LocatorCacheStats,
}

pub struct LocatorCache {
    inner: Arc<Mutex<Inner>>,
}

impl LocatorCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                map: HashMap::new(),
                stats: LocatorCacheStats::default(),
            })),
        }
    }

    /// Store (or update) a `(role, name) -> rect` mapping.
    pub fn upsert(&self, key: LocatorKey, rect: (i32, i32, i32, i32)) {
        let mut g = self.inner.lock().unwrap();
        g.map.insert(key, (rect, std::time::Instant::now()));
        g.stats.upserts += 1;
    }

    /// Look up a cached rect. Returns `Some(LocatorHitResult)` on hit, `None` on
    /// miss or expiry. Increments the appropriate stat counter.
    pub fn lookup(&self, key: &LocatorKey) -> Option<LocatorHitResult> {
        let mut g = self.inner.lock().unwrap();
        match g.map.get(key).copied() {
            Some((rect, stored_at)) => {
                if stored_at.elapsed() > ENTRY_TTL {
                    g.map.remove(key);
                    g.stats.misses += 1;
                    return None;
                }
                g.stats.hits += 1;
                Some(LocatorHitResult::from_rect(rect))
            }
            None => {
                g.stats.misses += 1;
                None
            }
        }
    }

    /// Invalidate (remove) a specific key — called when ElementFromPoint validation fails.
    pub fn invalidate(&self, key: &LocatorKey) {
        let mut g = self.inner.lock().unwrap();
        g.map.remove(key);
        g.stats.invalidations += 1;
    }

    /// Clear all entries (e.g., on foreground-window change).
    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.map.clear();
    }

    /// Return current hit/miss statistics.
    pub fn stats(&self) -> LocatorCacheStats {
        self.inner.lock().unwrap().stats.clone()
    }
}

impl Default for LocatorCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> LocatorKey {
        LocatorKey::new("button", "OK")
    }

    #[test]
    fn miss_on_empty_cache() {
        let c = LocatorCache::new();
        assert!(c.lookup(&key()).is_none());
        assert_eq!(c.stats().misses, 1);
        assert_eq!(c.stats().hits, 0);
    }

    #[test]
    fn upsert_then_lookup_returns_hit() {
        let c = LocatorCache::new();
        c.upsert(key(), (10, 20, 110, 70));
        let hit = c.lookup(&key()).unwrap();
        assert_eq!(hit.rect, (10, 20, 110, 70));
        assert_eq!(hit.center, (60, 45));
        assert_eq!(c.stats().hits, 1);
        assert_eq!(c.stats().upserts, 1);
    }

    #[test]
    fn invalidate_causes_subsequent_miss() {
        let c = LocatorCache::new();
        c.upsert(key(), (0, 0, 100, 50));
        c.invalidate(&key());
        assert!(c.lookup(&key()).is_none());
        assert_eq!(c.stats().invalidations, 1);
    }

    #[test]
    fn upsert_updates_existing_rect() {
        let c = LocatorCache::new();
        c.upsert(key(), (0, 0, 10, 10));
        c.upsert(key(), (5, 5, 50, 50));
        let hit = c.lookup(&key()).unwrap();
        assert_eq!(hit.rect, (5, 5, 50, 50));
    }

    /// Simulate the fast-path: cache hit returns without a walk.
    /// In the real session, if lookup returns Some, we validate via ElementFromPoint.
    /// This test exercises the key logic gate without COM.
    #[test]
    fn cache_hit_skips_walk_simulation() {
        let c = LocatorCache::new();
        c.upsert(key(), (100, 200, 200, 250));
        // Simulate: element_from_point would be called at center (150, 225).
        // Here we just verify the hit and center are correct.
        let hit = c.lookup(&key()).unwrap();
        assert_eq!(hit.center, (150, 225));
        // If element_from_point matched: upsert again (no-op) and return element.
        // If it didn't match: invalidate.
        // --- simulated validation pass ---
        c.upsert(key(), hit.rect); // re-upsert on valid hit
        assert_eq!(c.stats().upserts, 2);
    }

    /// Simulate the stale-rect (ElementFromPoint mismatch) invalidation path.
    #[test]
    fn stale_rect_invalidation_falls_back_to_miss() {
        let c = LocatorCache::new();
        c.upsert(key(), (999, 999, 1000, 1000)); // rect in unreachable area
        let _hit = c.lookup(&key()).unwrap();
        // --- simulated validation FAIL (element name/role mismatch) ---
        c.invalidate(&key());
        // Next lookup must miss.
        assert!(c.lookup(&key()).is_none());
        assert_eq!(c.stats().invalidations, 1);
    }
}
