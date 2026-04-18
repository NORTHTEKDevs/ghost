//! Intent compiler: JSON step list -> typed Op enum + JSONLogic conditions.

use crate::error::IntentError;
use crate::jsonlogic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Click { target: String },
    Type { target: String, text: String },
    Press { key: String },
    Hotkey { modifiers: Vec<String>, key: String },
    WaitForText {
        text: String,
        #[serde(default = "default_true")]
        appears: bool,
        #[serde(default = "default_wait_ms")]
        timeout_ms: u64,
    },
    WaitUntil {
        condition: Value,
        #[serde(default = "default_wait_ms")]
        timeout_ms: u64,
    },
    WaitForIdle {
        #[serde(default = "default_stable")]
        stable_frames: u32,
        #[serde(default = "default_wait_ms")]
        timeout_ms: u64,
    },
    Navigate { url: String },
    FocusWindow { name: String },
    Screenshot,
}

fn default_true() -> bool { true }
fn default_wait_ms() -> u64 { 5000 }
fn default_stable() -> u32 { 3 }

#[derive(Debug, Clone)]
pub struct CompiledIntent {
    pub ops: Vec<Op>,
    pub abort_if: Option<Value>,
    pub retry_if: Option<Value>,
    pub max_duration_ms: u64,
}

#[derive(Debug, Deserialize)]
struct RawIntent {
    steps: Vec<Value>,
    #[serde(default)]
    abort_if: Option<Value>,
    #[serde(default)]
    retry_if: Option<Value>,
    #[serde(default = "default_max_duration")]
    max_duration_ms: u64,
}

fn default_max_duration() -> u64 { 30_000 }

pub struct IntentCompiler;

impl IntentCompiler {
    pub fn compile(json: &str) -> Result<CompiledIntent, IntentError> {
        let raw: RawIntent = serde_json::from_str(json)
            .map_err(|e| IntentError::Invalid(format!("parse: {e}")))?;
        let mut ops = Vec::with_capacity(raw.steps.len());
        for (i, step) in raw.steps.iter().enumerate() {
            let op: Op = serde_json::from_value(step.clone())
                .map_err(|e| IntentError::Invalid(format!("step {i}: {e}")))?;
            ops.push(op);
        }
        if let Some(expr) = &raw.abort_if {
            jsonlogic::validate(expr)?;
        }
        if let Some(expr) = &raw.retry_if {
            jsonlogic::validate(expr)?;
        }
        Ok(CompiledIntent {
            ops,
            abort_if: raw.abort_if,
            retry_if: raw.retry_if,
            max_duration_ms: raw.max_duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_step_list() {
        let json = r#"{"steps":[{"op":"click","target":"Submit"},{"op":"wait_for_text","text":"OK"}]}"#;
        let c = IntentCompiler::compile(json).unwrap();
        assert_eq!(c.ops.len(), 2);
    }

    #[test]
    fn rejects_unknown_op() {
        let json = r#"{"steps":[{"op":"transcendent_meditation"}]}"#;
        assert!(IntentCompiler::compile(json).is_err());
    }

    #[test]
    fn rejects_malformed_abort_if_at_compile_time() {
        let json = r#"{"steps":[],"abort_if":{"??":[1,2]}}"#;
        assert!(IntentCompiler::compile(json).is_err());
    }

    #[test]
    fn parses_hotkey_with_modifiers() {
        let json = r#"{"steps":[{"op":"hotkey","modifiers":["Ctrl"],"key":"t"}]}"#;
        let c = IntentCompiler::compile(json).unwrap();
        assert_eq!(c.ops.len(), 1);
    }

    #[test]
    fn max_duration_defaults_to_30s() {
        let c = IntentCompiler::compile(r#"{"steps":[]}"#).unwrap();
        assert_eq!(c.max_duration_ms, 30_000);
    }
}
