//! Cascade grounding engine.
//!
//! Tries tiers in order: Cache (T1) → UIA (T2) → OCR (T3) → [YOLO (T4)] → VLM (T5).
//! First tier whose confidence ≥ threshold wins.
//!
//! # Design note — COM boundary
//! The tier interfaces are `async` trait objects.  In production (ghost-session),
//! the UIA and Cache tiers call COM; they live on the STA thread inside
//! `ghost-session`'s single-threaded tokio runtime (the MCP event loop).
//! The ONNX/YOLO and VLM tiers run their blocking work via `spawn_blocking`.
//!
//! This crate holds only the pure tier-ordering, threshold, and telemetry
//! logic.  It is COM-free and fully unit-testable with stub tiers.

use std::collections::HashMap;
use std::time::Instant;

use crate::types::{Grounded, Target, Tier};

// ---------------------------------------------------------------------------
// Confidence thresholds per tier (defaults)
// ---------------------------------------------------------------------------

/// Default confidence threshold: a tier result with confidence ≥ this wins immediately.
pub const DEFAULT_THRESHOLD: f32 = 0.50;

/// Per-tier default confidence scores.
pub const CONFIDENCE_CACHE: f32 = 0.95;
pub const CONFIDENCE_UIA: f32 = 0.90;
pub const CONFIDENCE_OCR: f32 = 0.70;
pub const CONFIDENCE_YOLO: f32 = 0.75;
pub const CONFIDENCE_VLM: f32 = 0.60;

// ---------------------------------------------------------------------------
// LocateMode — Instant vs Deliberate dispatch
// ---------------------------------------------------------------------------

/// Dispatch mode controlling which tiers are attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocateMode {
    /// Try only fast local tiers (Cache, UIA, OCR, YOLO). No cloud VLM.
    /// The engine escalates to VLM automatically on a full local miss.
    #[default]
    Instant,
    /// Try all tiers including cloud VLM and reflection from the first attempt.
    Deliberate,
    /// Try only local tiers (Cache, UIA, OCR, YOLO). Never escalates to VLM.
    /// Use when API cost must be zero (offline, cost-sensitive automation).
    InstantOnly,
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

/// Per-tier telemetry counters, collected over the engine lifetime.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TierStats {
    pub attempts: u64,
    pub hits: u64,
    /// Total wall-clock time spent in this tier (milliseconds).
    pub total_ms: u64,
}

impl TierStats {
    fn record_attempt(&mut self, hit: bool, elapsed_ms: u64) {
        self.attempts += 1;
        if hit {
            self.hits += 1;
        }
        self.total_ms += elapsed_ms;
    }
}

/// Aggregate telemetry across all tiers, exposed via `GroundingEngine::stats()`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GroundingStats {
    pub by_tier: HashMap<String, TierStats>,
    /// How many locate calls resolved in Instant mode.
    pub instant_hits: u64,
    /// How many locate calls required Deliberate mode (escalation).
    pub deliberate_hits: u64,
    /// How many locate calls failed all tiers.
    pub total_misses: u64,
}

impl GroundingStats {
    fn record_tier(&mut self, tier: Tier, hit: bool, elapsed_ms: u64) {
        self.by_tier
            .entry(tier.to_string())
            .or_default()
            .record_attempt(hit, elapsed_ms);
    }
}

// ---------------------------------------------------------------------------
// TierResult — what a tier returns
// ---------------------------------------------------------------------------

/// Result from a single tier attempt.
#[derive(Debug)]
pub enum TierResult {
    /// The tier successfully grounded the target.
    Hit(Grounded),
    /// The tier could not locate the target.
    Miss,
    /// The tier is not applicable for this target kind.
    NotApplicable,
}

// ---------------------------------------------------------------------------
// Tier trait — implemented by ghost-session for real, by stubs in tests
// ---------------------------------------------------------------------------

/// A single grounding tier.  Implementations are in `ghost-session` (which
/// has access to COM, ONNX, and async HTTP).
///
/// All methods are `async` so that VLM calls can await without blocking.
/// Sync tiers (Cache, UIA, OCR) just return immediately.
///
/// # COM safety
/// The future returned by `locate` is NOT `Send`.  COM UIA objects are thread-affine
/// (STA) and cannot be moved to another thread.  The grounding engine always runs on
/// the session's block_on STA thread, so `Send` is not required.
pub trait GroundingTier {
    /// The tier identifier.
    fn tier(&self) -> Tier;

    /// Attempt to ground `target`.  Must not panic.
    /// Called from an async context on the session's STA thread.
    fn locate<'a>(
        &'a self,
        target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>>;
}

// ---------------------------------------------------------------------------
// GroundingEngine
// ---------------------------------------------------------------------------

/// The cascade engine.  Holds an ordered list of tier implementations and
/// runs them in order until one succeeds with sufficient confidence.
///
/// The lifetime `'t` is the lifetime of the tier implementations.  For tiers
/// that borrow session state (COM, cache, etc.) `'t` is tied to the session
/// reference.  For stub tiers in tests `'t` can be `'static`.
///
/// # COM / thread safety
/// `GroundingEngine` is intentionally NOT `Send`.  The UIA tiers hold COM pointers
/// that are thread-affine (STA).  The engine always runs on the session's STA
/// block_on thread; never move it to a different thread.
pub struct GroundingEngine<'t> {
    /// Ordered list of tiers to try (first = highest priority).
    tiers: Vec<Box<dyn GroundingTier + 't>>,
    /// Minimum confidence a tier result must have to be accepted.
    threshold: f32,
    /// Cumulative telemetry.
    stats: GroundingStats,
}

impl<'t> GroundingEngine<'t> {
    /// Create a new engine with the given tiers (in priority order) and a
    /// default confidence threshold of [`DEFAULT_THRESHOLD`].
    pub fn new(tiers: Vec<Box<dyn GroundingTier + 't>>) -> Self {
        Self {
            tiers,
            threshold: DEFAULT_THRESHOLD,
            stats: GroundingStats::default(),
        }
    }

    /// Override the confidence threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Locate `target` using the cascade, beginning at `mode`.
    ///
    /// If `mode` is [`LocateMode::Instant`] and all local tiers miss,
    /// the engine automatically escalates to [`LocateMode::Deliberate`]
    /// (adds VLM) and records the escalation in telemetry.
    ///
    /// Returns `None` when all tiers (at the effective mode) fail.
    pub async fn locate(
        &mut self,
        target: &Target,
        mode: LocateMode,
    ) -> Option<Grounded> {
        // Coords: bypass grounding entirely.
        if let Target::Coords(x, y) = target {
            return Some(Grounded::from_point((*x, *y), 1.0, Tier::Cache));
        }

        // First pass: run tiers appropriate for the requested mode.
        let result = self.run_tiers(target, mode).await;

        if let Some(grounded) = result {
            match mode {
                LocateMode::Instant | LocateMode::InstantOnly => self.stats.instant_hits += 1,
                LocateMode::Deliberate => self.stats.deliberate_hits += 1,
            }
            return Some(grounded);
        }

        // MEDIUM-3/6/8: Escalate from Instant → VLM-ONLY (not a full re-run of all tiers).
        // InstantOnly never escalates to avoid any VLM API cost.
        if mode == LocateMode::Instant {
            tracing::warn!(?target, "instant local miss; escalating to VLM");
            let escalated = self.run_vlm_only(target).await;
            if escalated.is_some() {
                self.stats.deliberate_hits += 1;
                return escalated;
            }
        }

        self.stats.total_misses += 1;
        None
    }

    /// Run only the VLM tier (Instant-miss escalation path).
    /// This ensures escalation never re-runs Cache/UIA/OCR that already missed.
    async fn run_vlm_only(&mut self, target: &Target) -> Option<Grounded> {
        for i in 0..self.tiers.len() {
            if self.tiers[i].tier() != Tier::Vlm {
                continue;
            }
            let start = Instant::now();
            let result = self.tiers[i].locate(target).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            match result {
                TierResult::Hit(grounded) => {
                    let hit = grounded.confidence >= self.threshold;
                    self.stats.record_tier(Tier::Vlm, hit, elapsed_ms);
                    if hit {
                        return Some(grounded);
                    }
                }
                TierResult::Miss => {
                    self.stats.record_tier(Tier::Vlm, false, elapsed_ms);
                }
                TierResult::NotApplicable => {}
            }
        }
        None
    }

    /// Run all tiers that are active for `mode`, in order.
    async fn run_tiers(&mut self, target: &Target, mode: LocateMode) -> Option<Grounded> {
        for i in 0..self.tiers.len() {
            let tier_id = self.tiers[i].tier();

            // Skip VLM in Instant and InstantOnly modes during the primary pass.
            // (Instant escalates via run_vlm_only separately; InstantOnly never calls VLM.)
            if matches!(mode, LocateMode::Instant | LocateMode::InstantOnly) && tier_id == Tier::Vlm {
                continue;
            }

            let start = Instant::now();
            let result = self.tiers[i].locate(target).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            match result {
                TierResult::Hit(grounded) => {
                    let hit = grounded.confidence >= self.threshold;
                    self.stats.record_tier(tier_id, hit, elapsed_ms);
                    if hit {
                        return Some(grounded);
                    }
                    // Below threshold: keep trying.
                }
                TierResult::Miss => {
                    self.stats.record_tier(tier_id, false, elapsed_ms);
                }
                TierResult::NotApplicable => {
                    // Don't count as attempt.
                }
            }
        }
        None
    }

    /// Snapshot current telemetry.
    pub fn stats(&self) -> &GroundingStats {
        &self.stats
    }
}

// ---------------------------------------------------------------------------
// Tests — pure logic, no COM
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ---------------------------------------------------------------------------
    // Stub tier helpers
    // ---------------------------------------------------------------------------

    struct StubTier {
        tier: Tier,
        result: TierResult,
        /// Tracks how many times locate was called.
        calls: Arc<AtomicUsize>,
    }

    impl StubTier {
        fn hit(tier: Tier, confidence: f32) -> (Box<dyn GroundingTier>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls2 = calls.clone();
            let rect = (0, 0, 100, 50);
            let center = (50, 25);
            let g = Grounded { rect, center, confidence, source: tier, name: None };
            (
                Box::new(StubTier { tier, result: TierResult::Hit(g), calls: calls2 }),
                calls,
            )
        }

        fn miss(tier: Tier) -> (Box<dyn GroundingTier>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls2 = calls.clone();
            (Box::new(StubTier { tier, result: TierResult::Miss, calls: calls2 }), calls)
        }

        fn na(tier: Tier) -> Box<dyn GroundingTier> {
            Box::new(StubTier {
                tier,
                result: TierResult::NotApplicable,
                calls: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    impl GroundingTier for StubTier {
        fn tier(&self) -> Tier {
            self.tier
        }

        fn locate<'a>(
            &'a self,
            _target: &'a Target,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let r = match &self.result {
                TierResult::Hit(g) => TierResult::Hit(g.clone()),
                TierResult::Miss => TierResult::Miss,
                TierResult::NotApplicable => TierResult::NotApplicable,
            };
            Box::pin(async move { r })
        }
    }

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn run<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn name(s: &str) -> Target {
        Target::Name(s.into())
    }

    // ---------------------------------------------------------------------------
    // Ordering tests
    // ---------------------------------------------------------------------------

    #[test]
    fn first_hit_tier_wins() {
        let (cache_tier, cache_calls) = StubTier::hit(Tier::Cache, CONFIDENCE_CACHE);
        let (uia_tier, uia_calls) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![cache_tier, uia_tier]);
        let result = run(engine.locate(&name("btn"), LocateMode::Deliberate));

        assert!(result.is_some());
        assert_eq!(result.unwrap().source, Tier::Cache);
        assert_eq!(cache_calls.load(Ordering::SeqCst), 1);
        // UIA should not have been called because Cache already won.
        assert_eq!(uia_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn miss_falls_through_to_next_tier() {
        let (cache_miss, cache_calls) = StubTier::miss(Tier::Cache);
        let (uia_hit, uia_calls) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![cache_miss, uia_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::Deliberate));

        assert!(result.is_some());
        assert_eq!(result.unwrap().source, Tier::Uia);
        assert_eq!(cache_calls.load(Ordering::SeqCst), 1);
        assert_eq!(uia_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn all_miss_returns_none() {
        let (t1, _) = StubTier::miss(Tier::Cache);
        let (t2, _) = StubTier::miss(Tier::Uia);
        let (t3, _) = StubTier::miss(Tier::Ocr);
        let (t4, _) = StubTier::miss(Tier::Vlm);
        let mut engine = GroundingEngine::new(vec![t1, t2, t3, t4]);
        let result = run(engine.locate(&name("btn"), LocateMode::Deliberate));
        assert!(result.is_none());
    }

    // ---------------------------------------------------------------------------
    // Threshold tests
    // ---------------------------------------------------------------------------

    #[test]
    fn low_confidence_hit_skipped_falls_to_next() {
        // Cache returns 0.3 confidence, below threshold 0.5.
        let (cache_low, _) = StubTier::hit(Tier::Cache, 0.3);
        let (uia_high, _) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![cache_low, uia_high]);
        let result = run(engine.locate(&name("btn"), LocateMode::Deliberate)).unwrap();
        assert_eq!(result.source, Tier::Uia);
    }

    #[test]
    fn custom_threshold_accepted() {
        // With threshold 0.95 only the cache (0.95) wins.
        let (cache_tier, _) = StubTier::hit(Tier::Cache, CONFIDENCE_CACHE);
        let (uia_tier, _) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA); // 0.90 < 0.95
        let mut engine = GroundingEngine::new(vec![uia_tier, cache_tier]).with_threshold(0.95);
        let result = run(engine.locate(&name("btn"), LocateMode::Deliberate)).unwrap();
        assert_eq!(result.source, Tier::Cache);
    }

    // ---------------------------------------------------------------------------
    // Instant mode — VLM excluded
    // ---------------------------------------------------------------------------

    #[test]
    fn instant_mode_skips_vlm() {
        // Proper test: VLM is the only real tier; Instant first pass should skip it.
        // Escalation via run_vlm_only will then call it exactly once.
        let (vlm_only, vlm_only_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let (cache_miss, _) = StubTier::miss(Tier::Cache);
        let mut eng = GroundingEngine::new(vec![cache_miss, vlm_only]);
        let _result = run(eng.locate(&name("btn"), LocateMode::Instant));
        // MEDIUM-3: escalation must call VLM exactly once (not twice from a full Deliberate re-run).
        let vlm_total = vlm_only_calls.load(Ordering::SeqCst);
        assert_eq!(vlm_total, 1, "VLM called {vlm_total} times; must be exactly 1 (escalation only, not re-run)");
    }

    // MEDIUM-3: Instant miss → exactly one extra VLM attempt, not a full tier re-run.
    #[test]
    fn instant_miss_escalates_vlm_only_not_full_rerun() {
        let (cache_miss, cache_calls) = StubTier::miss(Tier::Cache);
        let (uia_miss, uia_calls) = StubTier::miss(Tier::Uia);
        let (vlm_hit, vlm_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let mut engine = GroundingEngine::new(vec![cache_miss, uia_miss, vlm_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::Instant));
        assert!(result.is_some(), "should escalate and find via VLM");
        // Cache and UIA each called once (in the Instant pass), not twice.
        assert_eq!(cache_calls.load(Ordering::SeqCst), 1, "Cache must be called exactly once");
        assert_eq!(uia_calls.load(Ordering::SeqCst), 1, "UIA must be called exactly once");
        // VLM called exactly once (escalation only).
        assert_eq!(vlm_calls.load(Ordering::SeqCst), 1, "VLM must be called exactly once during escalation");
    }

    // MEDIUM-7: InstantOnly never escalates to VLM, even on full miss.
    #[test]
    fn instant_only_never_calls_vlm() {
        let (cache_miss, _) = StubTier::miss(Tier::Cache);
        let (uia_miss, _) = StubTier::miss(Tier::Uia);
        let (vlm_hit, vlm_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let mut engine = GroundingEngine::new(vec![cache_miss, uia_miss, vlm_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::InstantOnly));
        assert!(result.is_none(), "InstantOnly should return None on local miss — no VLM escalation");
        assert_eq!(vlm_calls.load(Ordering::SeqCst), 0, "VLM must never be called in InstantOnly mode");
    }

    // MEDIUM-7: InstantOnly returns hit when a local tier succeeds.
    #[test]
    fn instant_only_local_hit_returns_result() {
        let (uia_hit, uia_calls) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let (vlm_hit, vlm_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let mut engine = GroundingEngine::new(vec![uia_hit, vlm_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::InstantOnly)).unwrap();
        assert_eq!(result.source, Tier::Uia);
        assert_eq!(uia_calls.load(Ordering::SeqCst), 1);
        assert_eq!(vlm_calls.load(Ordering::SeqCst), 0, "VLM must not be called when UIA hit in InstantOnly");
    }

    #[test]
    fn instant_mode_local_hit_no_escalation() {
        // When UIA hits in Instant mode, VLM should never be called.
        let (uia_hit, uia_calls) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let (vlm_hit, vlm_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let mut engine = GroundingEngine::new(vec![uia_hit, vlm_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::Instant)).unwrap();
        assert_eq!(result.source, Tier::Uia);
        assert_eq!(uia_calls.load(Ordering::SeqCst), 1);
        assert_eq!(vlm_calls.load(Ordering::SeqCst), 0, "VLM must not be called when UIA hit in Instant mode");
    }

    #[test]
    fn instant_escalates_to_deliberate_on_full_miss() {
        let (miss_cache, _) = StubTier::miss(Tier::Cache);
        let (vlm_hit, vlm_calls) = StubTier::hit(Tier::Vlm, CONFIDENCE_VLM);
        let mut engine = GroundingEngine::new(vec![miss_cache, vlm_hit]);
        let result = run(engine.locate(&name("btn"), LocateMode::Instant));
        assert!(result.is_some(), "should escalate and find via VLM");
        assert_eq!(vlm_calls.load(Ordering::SeqCst), 1, "VLM should be called once during escalation");
        assert_eq!(engine.stats().deliberate_hits, 1);
    }

    // ---------------------------------------------------------------------------
    // Coords bypass
    // ---------------------------------------------------------------------------

    #[test]
    fn coords_target_bypasses_all_tiers() {
        let (miss, calls) = StubTier::miss(Tier::Cache);
        let mut engine = GroundingEngine::new(vec![miss]);
        let result = run(engine.locate(&Target::Coords(100, 200), LocateMode::Instant)).unwrap();
        assert_eq!(result.center, (100, 200));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no tier should be called for Coords target");
    }

    // ---------------------------------------------------------------------------
    // Telemetry
    // ---------------------------------------------------------------------------

    #[test]
    fn telemetry_records_attempts_and_hits() {
        let (cache_miss, _) = StubTier::miss(Tier::Cache);
        let (uia_hit, _) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![cache_miss, uia_hit]);
        run(engine.locate(&name("btn"), LocateMode::Deliberate));

        let stats = engine.stats();
        let cache_stats = stats.by_tier.get("cache").unwrap();
        assert_eq!(cache_stats.attempts, 1);
        assert_eq!(cache_stats.hits, 0);

        let uia_stats = stats.by_tier.get("uia").unwrap();
        assert_eq!(uia_stats.attempts, 1);
        assert_eq!(uia_stats.hits, 1);
    }

    #[test]
    fn telemetry_instant_vs_deliberate_counts() {
        let (uia_hit, _) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![uia_hit]);
        run(engine.locate(&name("btn"), LocateMode::Instant));
        assert_eq!(engine.stats().instant_hits, 1);
        assert_eq!(engine.stats().deliberate_hits, 0);
    }

    #[test]
    fn telemetry_miss_counted() {
        let (cache_miss, _) = StubTier::miss(Tier::Cache);
        let mut engine = GroundingEngine::new(vec![cache_miss]);
        run(engine.locate(&name("btn"), LocateMode::Deliberate));
        assert_eq!(engine.stats().total_misses, 1);
    }

    // ---------------------------------------------------------------------------
    // NotApplicable tier does not count as attempt
    // ---------------------------------------------------------------------------

    #[test]
    fn not_applicable_tier_not_counted() {
        let na = StubTier::na(Tier::Ocr);
        let (uia_hit, _) = StubTier::hit(Tier::Uia, CONFIDENCE_UIA);
        let mut engine = GroundingEngine::new(vec![na, uia_hit]);
        run(engine.locate(&name("btn"), LocateMode::Deliberate));

        let stats = engine.stats();
        // OCR was NotApplicable — should NOT appear in stats.
        assert!(!stats.by_tier.contains_key("ocr"));
    }
}
