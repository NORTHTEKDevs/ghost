//! Core types for the grounding cascade.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Target — what the caller wants to ground
// ---------------------------------------------------------------------------

/// Describes what the caller wants the engine to locate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// Locate by UIA Name property (element label / accessible name).
    Name(String),
    /// Locate by UIA control-type role string ("button", "edit", ...).
    Role(String),
    /// Locate by natural-language description — routed to VLM.
    Description(String),
    /// Locate by visible on-screen text — routed to OCR.
    Text(String),
    /// Already-known absolute screen coordinates; bypass grounding.
    Coords(i32, i32),
}

impl Target {
    /// Returns the inner string for Name/Role/Description/Text, or None for Coords.
    pub fn inner_str(&self) -> Option<&str> {
        match self {
            Target::Name(s) | Target::Role(s) | Target::Description(s) | Target::Text(s) => {
                Some(s.as_str())
            }
            Target::Coords(_, _) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tier — which grounding tier resolved the target
// ---------------------------------------------------------------------------

/// Which grounding tier resolved the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    /// Validated in-session locator cache.
    Cache,
    /// UI Automation (Win32 UIA accessibility tree).
    Uia,
    /// WinRT OCR (on-device text recognition).
    Ocr,
    /// OmniParser YOLO icon detector + Set-of-Marks (feature `yolo`).
    Yolo,
    /// Cloud VLM (NVIDIA / Anthropic).
    Vlm,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tier::Cache => write!(f, "cache"),
            Tier::Uia => write!(f, "uia"),
            Tier::Ocr => write!(f, "ocr"),
            Tier::Yolo => write!(f, "yolo"),
            Tier::Vlm => write!(f, "vlm"),
        }
    }
}

// ---------------------------------------------------------------------------
// Grounded — the result of successful grounding
// ---------------------------------------------------------------------------

/// Result of successfully grounding a [`Target`] to screen coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grounded {
    /// Bounding rect of the element: (left, top, right, bottom) in absolute screen pixels.
    pub rect: (i32, i32, i32, i32),
    /// Center of the bounding rect in absolute screen pixels.
    /// Always valid regardless of source tier.
    pub center: (i32, i32),
    /// Confidence score in 0.0..=1.0. Tier defaults: cache=0.95, uia=0.90, ocr=0.70, yolo=0.75, vlm=0.60.
    pub confidence: f32,
    /// Which tier produced this result.
    pub source: Tier,
    /// Accessible name of the element, if known at grounding time.
    /// Populated by Cache and UIA tiers; `None` for OCR, VLM, and YOLO tiers.
    pub name: Option<String>,
}

impl Grounded {
    /// Construct from a rect, computing center automatically.
    pub fn from_rect(rect: (i32, i32, i32, i32), confidence: f32, source: Tier) -> Self {
        let cx = (rect.0 + rect.2) / 2;
        let cy = (rect.1 + rect.3) / 2;
        Self { rect, center: (cx, cy), confidence, source, name: None }
    }

    /// Construct from a center point (e.g. VLM or coord target), using a 1x1 rect.
    pub fn from_point(center: (i32, i32), confidence: f32, source: Tier) -> Self {
        Self {
            rect: (center.0, center.1, center.0, center.1),
            center,
            confidence,
            source,
            name: None,
        }
    }

    /// Returns true when the rect is meaningful (Cache/UIA tiers produce real bounding rects).
    /// Returns false for point-only tiers (OCR, VLM, YOLO) where only center is valid.
    pub fn has_rect(&self) -> bool {
        !matches!(self.source, Tier::Ocr | Tier::Vlm | Tier::Yolo)
    }
}

// ---------------------------------------------------------------------------
// CoordNorm — 0-1000 normalised coordinate space
// ---------------------------------------------------------------------------

/// A normalised coordinate in the 0..=1000 space used by UI-TARS / Qwen models.
///
/// (0,0) = top-left corner, (1000,1000) = bottom-right corner of the reference
/// image (usually the downscaled screenshot region).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordNorm(pub u16, pub u16);

impl CoordNorm {
    /// Clamp both axes to [0, 1000].
    pub fn clamped(x: i32, y: i32) -> Self {
        let clamp = |v: i32| v.clamp(0, 1000) as u16;
        Self(clamp(x), clamp(y))
    }
}

/// Convert a normalised coordinate to an absolute pixel value.
///
/// `norm_to_px(500, 1920)` → 960  (centre of a 1920-wide image)
///
/// Result is clamped to `[0, dim-1]` if `dim > 0`.
pub fn norm_to_px(c: u16, dim: u32) -> i32 {
    if dim == 0 {
        return 0;
    }
    let px = (c as f32 * dim as f32 / 1000.0).round() as i32;
    px.clamp(0, dim as i32 - 1)
}

/// Convert an absolute pixel value to a normalised coordinate.
///
/// `px_to_norm(960, 1920)` → 500
///
/// Result is clamped to [0, 1000].
pub fn px_to_norm(px: i32, dim: u32) -> u16 {
    if dim == 0 {
        return 0;
    }
    let norm = (px as f32 * 1000.0 / dim as f32).round() as i32;
    norm.clamp(0, 1000) as u16
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- norm_to_px ---

    #[test]
    fn norm_to_px_zero_is_zero() {
        assert_eq!(norm_to_px(0, 1920), 0);
    }

    #[test]
    fn norm_to_px_max_is_dim_minus_one() {
        // 1000 should map to the last pixel (1919 for 1920-wide).
        assert_eq!(norm_to_px(1000, 1920), 1919);
    }

    #[test]
    fn norm_to_px_centre() {
        // 500 in a 1000-wide image => 500; round(500*1000/1000)=500.
        assert_eq!(norm_to_px(500, 1000), 500);
    }

    #[test]
    fn norm_to_px_round_trip_centre_1920() {
        // round(500 * 1920 / 1000) = round(960) = 960
        assert_eq!(norm_to_px(500, 1920), 960);
    }

    #[test]
    fn norm_to_px_zero_dim_returns_zero() {
        assert_eq!(norm_to_px(500, 0), 0);
    }

    // --- px_to_norm ---

    #[test]
    fn px_to_norm_zero_is_zero() {
        assert_eq!(px_to_norm(0, 1920), 0);
    }

    #[test]
    fn px_to_norm_full_dim_gives_1000() {
        // last pixel at 1919 in 1920: round(1919*1000/1920) = round(999.48) = 999
        assert_eq!(px_to_norm(1919, 1920), 999);
    }

    #[test]
    fn px_to_norm_centre() {
        assert_eq!(px_to_norm(960, 1920), 500);
    }

    #[test]
    fn px_to_norm_zero_dim_returns_zero() {
        assert_eq!(px_to_norm(500, 0), 0);
    }

    // --- round-trip ---

    #[test]
    fn round_trip_centre_1920() {
        let norm = px_to_norm(960, 1920);
        assert_eq!(norm, 500);
        let px = norm_to_px(norm, 1920);
        assert_eq!(px, 960);
    }

    #[test]
    fn round_trip_small_dim() {
        // For a 10-pixel-wide image, px 3 => norm 300 => back to px 3.
        let norm = px_to_norm(3, 10);
        assert_eq!(norm, 300);
        let px = norm_to_px(norm, 10);
        assert_eq!(px, 3);
    }

    #[test]
    fn round_trip_arbitrary() {
        // Round-trip: norm_to_px then px_to_norm should return a value close to
        // the original.  The maximum acceptable error depends on the dimension:
        // for a dim=100 image, 1 pixel = 10 norm units, so we allow ceil(1000/dim).
        for dim in [100u32, 768, 1080, 1920, 2560] {
            // Maximum rounding error in norm units for this dimension.
            // norm_to_px clamps to [0, dim-1], so the norm=1000 → px=dim-1 → norm≈(dim-1)*1000/dim
            // which loses up to 1000/dim units; also one pixel of rounding adds up to 1000/dim units.
            let max_err = ((2000.0 / dim as f32).ceil() as i32).max(1);
            for pct in [0u16, 100, 250, 500, 750, 900, 1000] {
                let px = norm_to_px(pct, dim);
                let norm = px_to_norm(px, dim);
                let diff = (norm as i32 - pct as i32).abs();
                assert!(
                    diff <= max_err,
                    "dim={dim} pct={pct}: norm_to_px={px}, px_to_norm={norm}, diff={diff} (max_err={max_err})"
                );
            }
        }
    }

    // --- CoordNorm clamping ---

    #[test]
    fn coord_norm_clamped_within_range() {
        let c = CoordNorm::clamped(500, 300);
        assert_eq!(c, CoordNorm(500, 300));
    }

    #[test]
    fn coord_norm_clamped_negative_becomes_zero() {
        let c = CoordNorm::clamped(-50, -1);
        assert_eq!(c, CoordNorm(0, 0));
    }

    #[test]
    fn coord_norm_clamped_above_1000_becomes_1000() {
        let c = CoordNorm::clamped(9999, 1500);
        assert_eq!(c, CoordNorm(1000, 1000));
    }

    // --- Grounded helpers ---

    #[test]
    fn grounded_from_rect_computes_center() {
        let g = Grounded::from_rect((100, 200, 300, 400), 0.9, Tier::Uia);
        assert_eq!(g.center, (200, 300));
        assert!(g.name.is_none());
    }

    #[test]
    fn grounded_from_point_has_unit_rect() {
        let g = Grounded::from_point((640, 480), 0.6, Tier::Vlm);
        assert_eq!(g.center, (640, 480));
        assert_eq!(g.rect, (640, 480, 640, 480));
        assert!(g.name.is_none());
    }

    // LOW-9: has_rect returns true for Cache/UIA (real rect), false for OCR/VLM/YOLO.
    #[test]
    fn grounded_has_rect_for_uia_and_cache() {
        let uia = Grounded::from_rect((0, 0, 10, 10), 0.9, Tier::Uia);
        assert!(uia.has_rect(), "UIA tier must have a real rect");
        let cache = Grounded::from_rect((0, 0, 10, 10), 0.95, Tier::Cache);
        assert!(cache.has_rect(), "Cache tier must have a real rect");
    }

    #[test]
    fn grounded_no_rect_for_point_only_tiers() {
        let vlm = Grounded::from_point((50, 50), 0.6, Tier::Vlm);
        assert!(!vlm.has_rect(), "VLM tier must not claim a real rect");
        let ocr = Grounded::from_point((50, 50), 0.7, Tier::Ocr);
        assert!(!ocr.has_rect(), "OCR tier must not claim a real rect");
        let yolo = Grounded::from_point((50, 50), 0.75, Tier::Yolo);
        assert!(!yolo.has_rect(), "YOLO tier must not claim a real rect");
    }

    // HIGH-2: name field can be set on Grounded for UIA/Cache tiers.
    #[test]
    fn grounded_name_field_settable() {
        let mut g = Grounded::from_rect((0, 0, 100, 50), 0.9, Tier::Uia);
        g.name = Some("Submit".to_string());
        assert_eq!(g.name.as_deref(), Some("Submit"));
    }

    // --- Target helpers ---

    #[test]
    fn target_inner_str_returns_string_for_variants() {
        assert_eq!(Target::Name("ok".into()).inner_str(), Some("ok"));
        assert_eq!(Target::Role("button".into()).inner_str(), Some("button"));
        assert_eq!(Target::Description("d".into()).inner_str(), Some("d"));
        assert_eq!(Target::Text("t".into()).inner_str(), Some("t"));
    }

    #[test]
    fn target_coords_inner_str_is_none() {
        assert_eq!(Target::Coords(0, 0).inner_str(), None);
    }
}
