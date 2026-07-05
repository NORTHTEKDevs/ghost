//! OmniParser-YOLO ONNX icon detector + Set-of-Marks (tier 4).
//!
//! # Feature gate
//! This module is only compiled when the `yolo` cargo feature is enabled.
//! The default workspace build does NOT enable this feature.
//!
//! # Model
//! Expects an OmniParser icon-detector ONNX model.  Point the `GHOST_YOLO_MODEL`
//! environment variable at the `.onnx` file at runtime.  Never commit a model
//! binary to git.
//!
//! # How to obtain the model
//! 1. Clone `microsoft/OmniParser` from GitHub.
//! 2. Download `icon_detect/model.pt` weights (see the repo README for Hugging Face link).
//! 3. Export to ONNX: `python export_onnx.py --weights icon_detect/model.pt --output icon_detect.onnx`
//!    (script provided in the OmniParser repo).
//! 4. Set `GHOST_YOLO_MODEL=/path/to/icon_detect.onnx`.
//!
//! # Set-of-Marks (SoM)
//! When a Description target can't be grounded by OCR/UIA, this tier:
//! 1. Runs the YOLO model to get a list of interactable regions.
//! 2. Fuses regions with WinRT OCR boxes (OmniParser asymmetric-containment
//!    overlap removal — see [`remove_overlapping`]).
//! 3. Overlays numbered bounding boxes on the screenshot and asks the VLM
//!    to pick an ID.
//! 4. Maps the ID back to the region center.

use crate::marks::Region;

// ---------------------------------------------------------------------------
// ONNX model loader (only compiled with the `yolo` feature)
// ---------------------------------------------------------------------------

/// YOLO icon detector backed by ONNX Runtime.
///
/// Load via [`YoloDetector::load`].  The model is loaded once and reused.
pub struct YoloDetector {
    // Inner session is behind a Box<dyn Any> so the ort types don't leak
    // into callers that might not have ort in scope.
    _session: Box<dyn std::any::Any + Send + Sync>,
    _input_name: String,
    /// Model input size (width, height) — depends on the OmniParser export.
    pub input_size: (u32, u32),
}

impl YoloDetector {
    /// Load a YOLO detector from `GHOST_YOLO_MODEL` env var.
    ///
    /// Returns an error if the env var is not set or the model fails to load.
    pub fn from_env() -> Result<Self, String> {
        let path = std::env::var("GHOST_YOLO_MODEL")
            .map_err(|_| "GHOST_YOLO_MODEL env var not set; point it at the OmniParser icon_detect.onnx file".to_string())?;
        Self::load(&path)
    }

    /// Load a YOLO detector from a filesystem path.
    ///
    /// Uses ort 2.0.0-rc.10 API: `ort::session::Session::builder().commit_from_file()`.
    /// `Environment` was removed in rc.9+; the global env is managed by the ort runtime.
    /// `GraphOptimizationLevel` lives in `ort::session::builder` in rc.10.
    pub fn load(path: &str) -> Result<Self, String> {
        use ort::session::Session;
        use ort::session::builder::GraphOptimizationLevel;

        let session = Session::builder()
            .map_err(|e| format!("ort Session::builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .map_err(|e| format!("ort opt level: {e}"))?
            .commit_from_file(path)
            .map_err(|e| format!("ort load model {path}: {e}"))?;

        Ok(Self {
            _session: Box::new(session),
            _input_name: "images".into(), // typical YOLO input name
            input_size: (640, 640),        // OmniParser default
        })
    }

    /// Detect interactable icon regions in a raw RGBA image.
    ///
    /// Returns regions in absolute coordinates relative to the top-left of `rgba`.
    ///
    /// This is a placeholder implementation.  A real implementation would:
    /// 1. Resize the image to `self.input_size`.
    /// 2. Run the ONNX session.
    /// 3. Apply NMS to the output bounding boxes.
    /// 4. Scale boxes back to original image dimensions.
    ///
    /// The model is not shipped in the repo.  See the module doc comment for
    /// how to obtain and export it.
    pub fn detect_icons(&self, _rgba: &[u8], _width: u32, _height: u32) -> Vec<Region> {
        // STUB: model inference not yet implemented.
        // Requires: resize, CHW transpose, run session, decode boxes, NMS.
        tracing::debug!("detect_icons: stub — model inference not yet implemented, returning empty");
        vec![]
    }
}
