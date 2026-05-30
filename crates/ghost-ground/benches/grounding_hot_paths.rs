//! Criterion latency benchmarks for ghost-ground pure hot paths.
//!
//! These benches cover only COM-free, I/O-free code:
//!   - coord math (norm_to_px, px_to_norm, CoordNorm)
//!   - VLM response parser (all formats)
//!   - GroundingEngine tier-ordering with stub tiers
//!
//! Budget assertions (documented; enforced manually via the bench output):
//!   - coord math:   <2 ms   (actual: sub-microsecond)
//!   - parser:       <1 ms   (actual: sub-microsecond; plain strings, no alloc-heavy path)
//!   - engine (stub tiers): <2 ms per locate call (actual: bounded by number of tiers, no I/O)
//!
//! Run: `cargo bench -p ghost-ground`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, black_box};
use ghost_ground::{
    CoordNorm, Grounded, Target, Tier,
    engine::{GroundingEngine, GroundingStats, LocateMode, TierResult, GroundingTier},
    parser::parse_vlm_response,
    types::{norm_to_px, px_to_norm},
};

// ---------------------------------------------------------------------------
// 1. Coordinate math — budget: <2 ms per 1000 calls (<2 µs per call)
// ---------------------------------------------------------------------------

fn bench_coord_math(c: &mut Criterion) {
    let mut g = c.benchmark_group("coord_math");
    // Budget: entire group should complete <2 ms per iteration for 1000 ops.

    g.bench_function("norm_to_px_1920", |b| {
        b.iter(|| {
            for pct in [0u16, 100, 250, 500, 750, 900, 1000] {
                black_box(norm_to_px(black_box(pct), 1920));
            }
        });
    });

    g.bench_function("px_to_norm_1920", |b| {
        b.iter(|| {
            for px in [0i32, 192, 480, 960, 1440, 1728, 1919] {
                black_box(px_to_norm(black_box(px), 1920));
            }
        });
    });

    g.bench_function("coord_norm_clamped", |b| {
        b.iter(|| {
            black_box(CoordNorm::clamped(black_box(500), black_box(300)));
            black_box(CoordNorm::clamped(black_box(-50), black_box(9999)));
        });
    });

    g.bench_function("grounded_from_rect", |b| {
        b.iter(|| {
            black_box(Grounded::from_rect(
                black_box((100, 200, 300, 400)),
                0.9,
                Tier::Uia,
            ))
        });
    });

    g.bench_function("grounded_from_point", |b| {
        b.iter(|| {
            black_box(Grounded::from_point(
                black_box((640, 480)),
                0.6,
                Tier::Vlm,
            ))
        });
    });

    g.finish();
}

// ---------------------------------------------------------------------------
// 2. VLM response parser — budget: <1 ms per parse call
// ---------------------------------------------------------------------------

fn bench_parser(c: &mut Criterion) {
    let mut g = c.benchmark_group("vlm_parser");

    let inputs: &[(&str, &str)] = &[
        ("bare_json", r#"{"x": 500, "y": 375}"#),
        ("fenced_json", "```json\n{\"x\": 250, \"y\": 750}\n```"),
        ("prose_json", r#"The element is located at {"x": 123, "y": 456} on screen."#),
        ("uitars_click", "click(start_box='(250, 375)')"),
        ("uitars_type", "type(start_box='(100,200)', content='hello world')"),
        ("uitars_scroll", "scroll(start_box='(500,500)', direction='down', amount=3)"),
        ("bare_tuple", "(320, 240)"),
        ("not_found", "The element is not found in the current view."),
        ("not_visible", "Element not visible in the screenshot."),
    ];

    for (name, input) in inputs {
        g.bench_with_input(BenchmarkId::new("parse", name), input, |b, input| {
            b.iter(|| black_box(parse_vlm_response(black_box(input))));
        });
    }

    g.finish();
}

// ---------------------------------------------------------------------------
// 3. GroundingEngine tier-ordering with stub tiers — budget: <2 ms
// ---------------------------------------------------------------------------

/// Stub tier that always returns Miss. Zero I/O, COM-free, pure logic.
struct AlwaysMissTier(Tier);

impl GroundingTier for AlwaysMissTier {
    fn tier(&self) -> Tier { self.0 }
    fn locate<'a>(
        &'a self,
        _target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async { TierResult::Miss })
    }
}

/// Stub tier that always returns a Hit at the given confidence.
struct AlwaysHitTier(Tier, f32);

impl GroundingTier for AlwaysHitTier {
    fn tier(&self) -> Tier { self.0 }
    fn locate<'a>(
        &'a self,
        _target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        let confidence = self.1;
        let tier = self.0;
        Box::pin(async move {
            TierResult::Hit(Grounded::from_rect((100, 100, 200, 200), confidence, tier))
        })
    }
}

/// Stub tier that returns NotApplicable.
struct NotApplicableTier(Tier);

impl GroundingTier for NotApplicableTier {
    fn tier(&self) -> Tier { self.0 }
    fn locate<'a>(
        &'a self,
        _target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async { TierResult::NotApplicable })
    }
}

fn bench_engine(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");

    let mut g = c.benchmark_group("engine");

    // Scenario A: cache hit on first tier (best case, should be <0.1 ms)
    g.bench_function("cache_hit_first_tier", |b| {
        let tiers: Vec<Box<dyn GroundingTier>> = vec![
            Box::new(AlwaysHitTier(Tier::Cache, 0.95)),
            Box::new(AlwaysMissTier(Tier::Uia)),
            Box::new(AlwaysMissTier(Tier::Ocr)),
        ];
        let mut engine = GroundingEngine::new(tiers);
        let target = Target::Name("Submit".into());
        b.iter(|| {
            rt.block_on(async {
                black_box(engine.locate(black_box(&target), LocateMode::InstantOnly).await)
            })
        });
    });

    // Scenario B: all instant tiers miss (falls to VLM escalation path, but no VLM tier
    // registered → returns None; exercises full tier iteration)
    g.bench_function("all_instant_miss_no_vlm", |b| {
        let tiers: Vec<Box<dyn GroundingTier>> = vec![
            Box::new(AlwaysMissTier(Tier::Cache)),
            Box::new(AlwaysMissTier(Tier::Uia)),
            Box::new(AlwaysMissTier(Tier::Ocr)),
        ];
        let mut engine = GroundingEngine::new(tiers);
        let target = Target::Name("Submit".into());
        b.iter(|| {
            rt.block_on(async {
                black_box(engine.locate(black_box(&target), LocateMode::Instant).await)
            })
        });
    });

    // Scenario C: tier ordering — NotApplicable skipped, then Miss, then Hit
    g.bench_function("tier_ordering_skip_na_then_hit", |b| {
        let tiers: Vec<Box<dyn GroundingTier>> = vec![
            Box::new(NotApplicableTier(Tier::Cache)),
            Box::new(AlwaysMissTier(Tier::Uia)),
            Box::new(AlwaysHitTier(Tier::Ocr, 0.70)),
        ];
        let mut engine = GroundingEngine::new(tiers);
        let target = Target::Text("Submit".into());
        b.iter(|| {
            rt.block_on(async {
                black_box(engine.locate(black_box(&target), LocateMode::InstantOnly).await)
            })
        });
    });

    // Scenario D: coords bypass — no tier traversal at all
    g.bench_function("coords_bypass", |b| {
        let tiers: Vec<Box<dyn GroundingTier>> = vec![
            Box::new(AlwaysMissTier(Tier::Cache)),
        ];
        let mut engine = GroundingEngine::new(tiers);
        let target = Target::Coords(640, 480);
        b.iter(|| {
            rt.block_on(async {
                black_box(engine.locate(black_box(&target), LocateMode::Instant).await)
            })
        });
    });

    g.finish();
}

criterion_group!(benches, bench_coord_math, bench_parser, bench_engine);
criterion_main!(benches);
