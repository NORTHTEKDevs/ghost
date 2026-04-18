//! v0.3.0 hot-path benchmarks. Run with `cargo bench -p ghost-intent`.
//!
//! Budgets (enforced manually via docs/benches/v030-baseline.md):
//! - jsonlogic eval (single op): < 1µs
//! - intent compile (3 ops): < 50µs

use criterion::{criterion_group, criterion_main, Criterion, black_box};
use ghost_intent::compiler::IntentCompiler;
use ghost_intent::jsonlogic;
use serde_json::{json, Value};

fn bench_jsonlogic_eval(c: &mut Criterion) {
    let expr: Value = json!({"==":[{"var":"cache_seq"},0]});
    let state = json!({"cache_seq": 0, "last_error": null});
    c.bench_function("jsonlogic_eq_var", |b| {
        b.iter(|| {
            let _ = black_box(jsonlogic::eval(&expr, &state).unwrap());
        })
    });
}

fn bench_intent_compile(c: &mut Criterion) {
    let json = r#"{"steps":[
        {"op":"click","target":"Submit"},
        {"op":"wait_for_text","text":"OK"},
        {"op":"press","key":"Enter"}
    ],"max_duration_ms":5000}"#;
    c.bench_function("intent_compile_3_ops", |b| {
        b.iter(|| {
            let _ = black_box(IntentCompiler::compile(json).unwrap());
        })
    });
}

criterion_group!(benches, bench_jsonlogic_eval, bench_intent_compile);
criterion_main!(benches);
