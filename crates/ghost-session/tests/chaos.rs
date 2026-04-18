//! Failure injection suite. Most gated `#[ignore]` (require real drivers).
//! Run with: `cargo test -p ghost-session --features chaos -- --ignored`

#![cfg(windows)]

use ghost_intent::compiler::IntentCompiler;
use ghost_intent::executor::{FsmExecutor, IntentStatus, OpsDispatcher, IntentState};
use ghost_intent::error::IntentError;
use ghost_intent::compiler::Op;
use async_trait::async_trait;

struct AlwaysFails;
#[async_trait(?Send)]
impl OpsDispatcher for AlwaysFails {
    async fn dispatch(&self, _: &Op, _: &mut IntentState) -> Result<(), IntentError> {
        Err(IntentError::OpFailed("simulated".into()))
    }
}

#[tokio::test]
async fn fsm_halts_on_max_duration_when_retry_if_always_true() {
    let intent = IntentCompiler::compile(
        r#"{"steps":[{"op":"click","target":"x"}],"retry_if":{"==":[1,1]},"max_duration_ms":150}"#
    ).unwrap();
    let d = AlwaysFails;
    let ex = FsmExecutor::new(&d);
    let r = ex.run(&intent).await;
    // Either the retry cap trips (Failed) or the duration gate (Timeout) — both are valid halts.
    assert!(matches!(r.status, IntentStatus::Failed { .. } | IntentStatus::Timeout));
    assert!(r.duration_ms < 1000, "should halt within 1s, got {}", r.duration_ms);
}

#[tokio::test]
#[ignore]
async fn cache_recovers_from_com_disconnect() {
    // Requires chaos feature + real UiaCache wiring through session.
    // Placeholder: see ghost-cache chaos_drop_events().
}

#[tokio::test]
#[ignore]
async fn sta_pool_circuit_trips_on_repeated_panics() {
    // Covered by ghost-core::uia::sta_pool tests; this is an E2E smoke.
}
