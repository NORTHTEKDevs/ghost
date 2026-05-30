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

use crate::types::{Grounded, Tier};

/// A detected region (bounding box in absolute screen pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// Bounding box: (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    /// Confidence from the YOLO model, 0.0..=1.0.
    pub confidence: f32,
}

impl Region {
    pub fn center(&self) -> (i32, i32) {
        let cx = (self.rect.0 + self.rect.2) / 2;
        let cy = (self.rect.1 + self.rect.3) / 2;
        (cx, cy)
    }

    pub fn area(&self) -> i64 {
        let w = (self.rect.2 - self.rect.0).max(0) as i64;
        let h = (self.rect.3 - self.rect.1).max(0) as i64;
        w * h
    }

    /// Intersection area with another region.
    pub fn intersection_area(&self, other: &Region) -> i64 {
        let left = self.rect.0.max(other.rect.0);
        let top = self.rect.1.max(other.rect.1);
        let right = self.rect.2.min(other.rect.2);
        let bottom = self.rect.3.min(other.rect.3);
        let w = (right - left).max(0) as i64;
        let h = (bottom - top).max(0) as i64;
        w * h
    }

    /// Returns the fraction of `self`'s area that overlaps with `other`.
    pub fn overlap_fraction(&self, other: &Region) -> f32 {
        let self_area = self.area();
        if self_area == 0 {
            return 0.0;
        }
        self.intersection_area(other) as f32 / self_area as f32
    }
}

// ---------------------------------------------------------------------------
// Set-of-Marks ID mapping
// ---------------------------------------------------------------------------

/// Map a 1-based Set-of-Marks ID to the corresponding region.
///
/// Returns `None` if `id` is out of range.
pub fn som_id_to_region(regions: &[Region], id: usize) -> Option<&Region> {
    if id == 0 || id > regions.len() {
        return None;
    }
    Some(&regions[id - 1])
}

/// Convert a Set-of-Marks ID to a [`Grounded`] result using the region center.
pub fn som_id_to_grounded(regions: &[Region], id: usize) -> Option<Grounded> {
    let r = som_id_to_region(regions, id)?;
    Some(Grounded::from_rect(r.rect, r.confidence, Tier::Yolo))
}

// ---------------------------------------------------------------------------
// Overlap removal — OmniParser asymmetric containment
// ---------------------------------------------------------------------------

/// Overlap-removal threshold.  A region is removed if this fraction of its
/// area is covered by a larger region (asymmetric — the larger region is kept).
const CONTAINMENT_THRESHOLD: f32 = 0.90;

/// OmniParser-style asymmetric containment overlap removal.
///
/// Removes a region from `candidates` if ≥ [`CONTAINMENT_THRESHOLD`] (90%) of its
/// area is covered by a region in `dominators`.  Also removes from `candidates`
/// any pair where one candidate is largely contained in another.
///
/// This is used to fuse YOLO icon boxes with WinRT OCR text boxes:
/// - `icon_regions`: YOLO detections.
/// - `ocr_regions`: WinRT OCR bounding boxes.
///
/// A YOLO box that is mostly inside an OCR text box (or vice-versa) is
/// redundant and is removed.
pub fn remove_overlapping(icon_regions: Vec<Region>, ocr_regions: &[Region]) -> Vec<Region> {
    // Step 1: remove any icon region that is largely contained by an OCR region.
    let filtered: Vec<Region> = icon_regions
        .into_iter()
        .filter(|icon| {
            for ocr in ocr_regions {
                if icon.overlap_fraction(ocr) >= CONTAINMENT_THRESHOLD {
                    return false; // icon is inside an OCR box — discard
                }
            }
            true
        })
        .collect();

    // Step 2: within the remaining icon regions, remove smaller ones contained
    // by larger ones.
    let n = filtered.len();
    let mut keep = vec![true; n];
    for i in 0..n {
        for j in 0..n {
            if i == j || !keep[i] {
                continue;
            }
            // If region i is largely inside region j, remove i.
            if filtered[i].overlap_fraction(&filtered[j]) >= CONTAINMENT_THRESHOLD
                && filtered[j].area() > filtered[i].area()
            {
                keep[i] = false;
            }
        }
    }

    filtered.into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, r)| r).collect()
}

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
    #[allow(unused_variables)]
    pub fn load(path: &str) -> Result<Self, String> {
        use ort::{Environment, Session, SessionBuilder, GraphOptimizationLevel};

        let session = SessionBuilder::new()
            .map_err(|e| format!("ort SessionBuilder: {e}"))?
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
        // TODO: implement when model integration is complete.
        // Requires: resize, CHW transpose, run session, decode boxes, NMS.
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Tests — pure math, no model required
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn region(l: i32, t: i32, r: i32, b: i32) -> Region {
        Region { rect: (l, t, r, b), confidence: 0.8 }
    }

    // --- Intersection / overlap ---

    #[test]
    fn no_overlap_gives_zero_fraction() {
        let a = region(0, 0, 10, 10);
        let b = region(20, 20, 30, 30);
        assert_eq!(a.overlap_fraction(&b), 0.0);
    }

    #[test]
    fn full_containment_gives_one() {
        let small = region(2, 2, 8, 8);  // 6×6 = 36
        let big = region(0, 0, 10, 10);   // 10×10 = 100
        let frac = small.overlap_fraction(&big);
        // small is completely inside big → fraction = 1.0
        assert!((frac - 1.0).abs() < 1e-5, "frac={frac}");
    }

    #[test]
    fn partial_overlap() {
        // a = (0,0,10,10) area=100; b = (5,0,15,10) area=100; intersection=5×10=50
        let a = region(0, 0, 10, 10);
        let b = region(5, 0, 15, 10);
        let frac = a.overlap_fraction(&b);
        assert!((frac - 0.5).abs() < 1e-5, "frac={frac}");
    }

    #[test]
    fn zero_area_region_gives_zero_fraction() {
        let zero = region(5, 5, 5, 5); // degenerate
        let big = region(0, 0, 10, 10);
        assert_eq!(zero.overlap_fraction(&big), 0.0);
    }

    // --- remove_overlapping ---

    #[test]
    fn icon_inside_ocr_is_removed() {
        let icon = region(1, 1, 9, 9);   // 8×8 = 64, entirely inside the OCR box
        let ocr = region(0, 0, 10, 10);  // 10×10 = 100
        // icon.overlap_fraction(ocr) = 1.0 >= 0.9 → removed
        let result = remove_overlapping(vec![icon], &[ocr]);
        assert!(result.is_empty(), "icon inside OCR box should be removed");
    }

    #[test]
    fn icon_outside_ocr_is_kept() {
        let icon = region(50, 50, 80, 80);
        let ocr = region(0, 0, 10, 10);
        let result = remove_overlapping(vec![icon.clone()], &[ocr]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rect, icon.rect);
    }

    #[test]
    fn smaller_icon_inside_larger_icon_is_removed() {
        let small = region(2, 2, 8, 8);   // 6×6 = 36
        let large = region(0, 0, 10, 10); // 10×10 = 100
        // small.overlap_fraction(large) ≈ 1.0 >= 0.9 → removed
        let result = remove_overlapping(vec![small, large.clone()], &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rect, large.rect);
    }

    #[test]
    fn non_overlapping_icons_both_kept() {
        let a = region(0, 0, 10, 10);
        let b = region(20, 20, 30, 30);
        let result = remove_overlapping(vec![a.clone(), b.clone()], &[]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn partially_overlapping_icon_kept_if_below_threshold() {
        // icon overlaps 40% with ocr → kept (threshold is 90%)
        let icon = region(0, 0, 10, 10); // area = 100
        let ocr = region(6, 0, 16, 10);  // intersection = 4×10 = 40 → 40% of icon
        let result = remove_overlapping(vec![icon], &[ocr]);
        assert_eq!(result.len(), 1, "40% overlap should not trigger removal");
    }

    // --- Set-of-Marks ID mapping ---

    #[test]
    fn som_id_1_maps_to_first_region() {
        let regions = vec![region(0, 0, 10, 10), region(20, 20, 30, 30)];
        let r = som_id_to_region(&regions, 1).unwrap();
        assert_eq!(r.rect, (0, 0, 10, 10));
    }

    #[test]
    fn som_id_2_maps_to_second_region() {
        let regions = vec![region(0, 0, 10, 10), region(20, 20, 30, 30)];
        let r = som_id_to_region(&regions, 2).unwrap();
        assert_eq!(r.rect, (20, 20, 30, 30));
    }

    #[test]
    fn som_id_0_returns_none() {
        let regions = vec![region(0, 0, 10, 10)];
        assert!(som_id_to_region(&regions, 0).is_none());
    }

    #[test]
    fn som_id_out_of_range_returns_none() {
        let regions = vec![region(0, 0, 10, 10)];
        assert!(som_id_to_region(&regions, 5).is_none());
    }

    #[test]
    fn som_grounded_has_correct_center() {
        let regions = vec![region(0, 0, 20, 10)];
        let g = som_id_to_grounded(&regions, 1).unwrap();
        assert_eq!(g.center, (10, 5));
        assert_eq!(g.source, Tier::Yolo);
    }

    // --- Region helpers ---

    #[test]
    fn region_center_correct() {
        let r = region(0, 0, 100, 50);
        assert_eq!(r.center(), (50, 25));
    }

    #[test]
    fn region_area_correct() {
        let r = region(0, 0, 10, 5);
        assert_eq!(r.area(), 50);
    }

    #[test]
    fn negative_rect_gives_zero_area() {
        // right < left → area = 0
        let r = region(10, 0, 5, 5);
        assert_eq!(r.area(), 0);
    }
}
