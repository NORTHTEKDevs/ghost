//! Ghost grounding cascade — coordinate contract, types, VLM parser, cascade engine.
//!
//! # Crate layout
//! - `types`: `Target`, `Tier`, `Grounded`, `CoordNorm` + coordinate conversion helpers.
//! - `parser`: Typed VLM response parser (UI-TARS/Qwen/JSON/prose formats).
//! - `engine`: `GroundingEngine` — tiered cascade (Cache->UIA->OCR->[YOLO]->VLM) + telemetry.
//! - `yolo` (feature `yolo`): OmniParser-YOLO ONNX icon detector + Set-of-Marks.

pub mod engine;
pub mod parser;
pub mod types;

#[cfg(feature = "yolo")]
pub mod yolo;

pub use types::{CoordNorm, Grounded, Target, Tier};
pub use engine::{GroundingEngine, GroundingStats, LocateMode, TierResult, GroundingTier};
pub use parser::{ParsedVlmResponse, parse_vlm_response};
