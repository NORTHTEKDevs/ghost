//! FSM executor: runs a CompiledIntent via an OpsDispatcher trait.

use crate::compiler::{CompiledIntent, Op};
use crate::error::IntentError;
use crate::jsonlogic;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntentState {
    pub last_error: Option<String>,
    pub last_op_index: usize,
    pub extras: Value,
}

impl IntentState {
    pub fn to_json(&self) -> Value {
        json!({
            "last_error": self.last_error,
            "last_op_index": self.last_op_index,
            "extras": self.extras,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IntentStatus {
    Success,
    Aborted { reason: String },
    Failed { at: usize, reason: String },
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpOutcome {
    pub index: usize,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentResult {
    pub status: IntentStatus,
    pub executed: usize,
    pub ops_results: Vec<OpOutcome>,
    pub duration_ms: u64,
}

#[async_trait(?Send)]
pub trait OpsDispatcher {
    async fn dispatch(&self, op: &Op, state: &mut IntentState) -> Result<(), IntentError>;
}

pub struct FsmExecutor<'a> {
    dispatcher: &'a dyn OpsDispatcher,
}

impl<'a> FsmExecutor<'a> {
    pub fn new(dispatcher: &'a dyn OpsDispatcher) -> Self {
        Self { dispatcher }
    }

    pub async fn run(&self, intent: &CompiledIntent) -> IntentResult {
        let start = Instant::now();
        let deadline = Duration::from_millis(intent.max_duration_ms);
        let mut state = IntentState::default();
        let mut ops_results = Vec::with_capacity(intent.ops.len());

        for (i, op) in intent.ops.iter().enumerate() {
            if start.elapsed() > deadline {
                return IntentResult {
                    status: IntentStatus::Timeout,
                    executed: i,
                    ops_results,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
            state.last_op_index = i;
            let mut attempt = 0u32;
            let mut last_err: Option<String> = None;
            loop {
                if start.elapsed() > deadline {
                    return IntentResult {
                        status: IntentStatus::Timeout,
                        executed: i,
                        ops_results,
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                match self.dispatcher.dispatch(op, &mut state).await {
                    Ok(()) => {
                        ops_results.push(OpOutcome { index: i, ok: true, error: None });
                        state.last_error = None;
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                        state.last_error = last_err.clone();
                    }
                }
                if let Some(abort) = &intent.abort_if {
                    if let Ok(v) = jsonlogic::eval(abort, &state.to_json()) {
                        if v.as_bool() == Some(true) {
                            ops_results.push(OpOutcome { index: i, ok: false, error: last_err.clone() });
                            return IntentResult {
                                status: IntentStatus::Aborted { reason: last_err.unwrap_or("abort_if true".into()) },
                                executed: i,
                                ops_results,
                                duration_ms: start.elapsed().as_millis() as u64,
                            };
                        }
                    }
                }
                let should_retry = match &intent.retry_if {
                    Some(expr) => jsonlogic::eval(expr, &state.to_json())
                        .map(|v| v.as_bool() == Some(true))
                        .unwrap_or(false),
                    None => false,
                };
                if should_retry && attempt < 3 {
                    let backoff_ms = 50u64 * (1 << attempt).min(32);
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    attempt += 1;
                    continue;
                }
                ops_results.push(OpOutcome { index: i, ok: false, error: last_err.clone() });
                return IntentResult {
                    status: IntentStatus::Failed { at: i, reason: last_err.unwrap_or_default() },
                    executed: i,
                    ops_results,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        }
        IntentResult {
            status: IntentStatus::Success,
            executed: intent.ops.len(),
            ops_results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::IntentCompiler;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct OkDispatcher;
    #[async_trait(?Send)]
    impl OpsDispatcher for OkDispatcher {
        async fn dispatch(&self, _op: &Op, _state: &mut IntentState) -> Result<(), IntentError> {
            Ok(())
        }
    }

    struct ErrDispatcher { msg: String }
    #[async_trait(?Send)]
    impl OpsDispatcher for ErrDispatcher {
        async fn dispatch(&self, _op: &Op, _state: &mut IntentState) -> Result<(), IntentError> {
            Err(IntentError::OpFailed(self.msg.clone()))
        }
    }

    struct FlakyDispatcher {
        fails: Arc<AtomicUsize>,
        until: usize,
    }
    #[async_trait(?Send)]
    impl OpsDispatcher for FlakyDispatcher {
        async fn dispatch(&self, _op: &Op, _state: &mut IntentState) -> Result<(), IntentError> {
            let n = self.fails.fetch_add(1, Ordering::SeqCst);
            if n < self.until {
                Err(IntentError::OpFailed("flake".into()))
            } else {
                Ok(())
            }
        }
    }

    fn intent(json: &str) -> CompiledIntent {
        IntentCompiler::compile(json).unwrap()
    }

    #[tokio::test]
    async fn runs_all_ops_in_order_happy_path() {
        let i = intent(r#"{"steps":[{"op":"click","target":"a"},{"op":"click","target":"b"}]}"#);
        let ex = FsmExecutor::new(&OkDispatcher);
        let r = ex.run(&i).await;
        assert!(matches!(r.status, IntentStatus::Success));
        assert_eq!(r.executed, 2);
    }

    #[tokio::test]
    async fn aborts_when_abort_if_becomes_true() {
        let i = intent(r#"{"steps":[{"op":"click","target":"a"}],"abort_if":{"contains":[{"var":"last_error"},"boom"]}}"#);
        let d = ErrDispatcher { msg: "boom".into() };
        let ex = FsmExecutor::new(&d);
        let r = ex.run(&i).await;
        assert!(matches!(r.status, IntentStatus::Aborted { .. }));
    }

    #[tokio::test]
    async fn retries_on_retry_if_up_to_cap() {
        let fails = Arc::new(AtomicUsize::new(0));
        let d = FlakyDispatcher { fails: fails.clone(), until: 2 };
        let i = intent(r#"{"steps":[{"op":"click","target":"a"}],"retry_if":{"==":[1,1]}}"#);
        let ex = FsmExecutor::new(&d);
        let r = ex.run(&i).await;
        assert!(matches!(r.status, IntentStatus::Success));
        assert_eq!(fails.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn enforces_max_duration_ms() {
        struct Slow;
        #[async_trait(?Send)]
        impl OpsDispatcher for Slow {
            async fn dispatch(&self, _: &Op, _: &mut IntentState) -> Result<(), IntentError> {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(())
            }
        }
        let i = intent(r#"{"steps":[{"op":"click","target":"a"},{"op":"click","target":"b"},{"op":"click","target":"c"}],"max_duration_ms":100}"#);
        let ex = FsmExecutor::new(&Slow);
        let r = ex.run(&i).await;
        assert!(matches!(r.status, IntentStatus::Timeout));
    }

    #[tokio::test]
    async fn returns_failed_on_op_error_without_retry() {
        let i = intent(r#"{"steps":[{"op":"click","target":"a"}]}"#);
        let d = ErrDispatcher { msg: "x".into() };
        let ex = FsmExecutor::new(&d);
        let r = ex.run(&i).await;
        assert!(matches!(r.status, IntentStatus::Failed { .. }));
    }
}
