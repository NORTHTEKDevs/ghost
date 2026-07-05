//! Ghost grounding cascade — coordinate contract, types, VLM parser, cascade engine.
//!
//! # Crate layout
//! - `types`: `Target`, `Tier`, `Grounded`, `CoordNorm` + coordinate conversion helpers.
//! - `parser`: Typed VLM response parser (UI-TARS/Qwen/JSON/prose formats).
//! - `engine`: `GroundingEngine` — tiered cascade (Cache->UIA->OCR->[CV/YOLO]->VLM) + telemetry.
//! - `marks`: Set-of-Marks geometry — `Region`, overlap fusion, ID→region mapping (always on).
//! - `cv_detect`: CPU classical-CV element detector (GPU-free, always on).
//! - `yolo` (feature `yolo`): OmniParser-YOLO ONNX icon detector.

pub mod engine;
pub mod parser;
pub mod types;
pub mod marks;
pub mod cv_detect;

#[cfg(feature = "yolo")]
pub mod yolo;

pub use types::{CoordNorm, Grounded, Target, Tier};
pub use engine::{GroundingEngine, GroundingStats, LocateMode, TierResult, GroundingTier};
pub use parser::{ParsedVlmResponse, parse_vlm_response};
pub use marks::{Region, remove_overlapping, som_id_to_region, som_id_to_grounded};
