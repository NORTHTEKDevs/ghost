//! Real `GroundingTier` implementations for the four tiers used in ghost-session.
//!
//! These are built per-`ground()` call so they can borrow `&GhostSession` fields
//! without lifetime headaches from storing trait objects that self-reference.
//!
//! All COM-touching tiers (Cache, UIA) return immediately (no .await) so they
//! never need to cross a thread boundary.  The OCR and VLM tiers do their
//! blocking/async work via spawn_blocking / reqwest as they already do in the
//! existing session methods.

use ghost_ground::engine::{
    CONFIDENCE_CACHE, CONFIDENCE_OCR, CONFIDENCE_UIA, CONFIDENCE_VLM,
    GroundingTier, TierResult,
};
use ghost_ground::types::{Grounded, Target, Tier};

use ghost_cache::{LocatorCache, locator_cache::LocatorKey};
use ghost_core::uia::tree::UiaTree;

// ---------------------------------------------------------------------------
// CacheTier
// ---------------------------------------------------------------------------

/// Checks the in-session validated locator cache.
/// NotApplicable for `Target::Coords` (already grounded) and `Target::Description`/`Target::Text`
/// (which are not keyed by locator-cache entries).
pub struct CacheTier<'s> {
    pub locator_cache: &'s LocatorCache,
    pub tree: &'s UiaTree,
    pub hwnd: isize,
}

impl<'s> GroundingTier for CacheTier<'s> {
    fn tier(&self) -> Tier {
        Tier::Cache
    }

    fn locate<'a>(
        &'a self,
        target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async move {
            let key = match target {
                Target::Name(n) => {
                    LocatorKey::with_hwnd(self.hwnd, "*", n.as_str())
                }
                Target::Role(r) => {
                    LocatorKey::with_hwnd(self.hwnd, r.as_str(), "*")
                }
                // Description and Text are not stored in the locator cache.
                Target::Description(_) | Target::Text(_) | Target::Coords(_, _) => {
                    return TierResult::NotApplicable;
                }
            };

            let hit = match self.locator_cache.lookup(&key) {
                Some(h) => h,
                None => return TierResult::Miss,
            };

            let (cx, cy) = hit.center;

            // Validate via ElementFromPoint.
            let el = match self.tree.element_from_point(cx, cy) {
                Ok(Some(el)) => el,
                _ => {
                    self.locator_cache.invalidate(&key);
                    return TierResult::Miss;
                }
            };

            let matches = match target {
                Target::Name(n) => el.name().to_lowercase() == n.to_lowercase(),
                Target::Role(r) => {
                    let role = ghost_core::uia::element::role_id_to_name(el.control_type());
                    role == r.as_str()
                }
                _ => false,
            };

            if matches {
                let rect = (hit.rect.0, hit.rect.1, hit.rect.2, hit.rect.3);
                // HIGH-2: populate name from the element retrieved from the cache.
                let element_name = match target {
                    Target::Name(n) => Some(n.clone()),
                    Target::Role(_) => Some(el.name()),
                    _ => None,
                };
                let mut grounded = Grounded::from_rect(rect, CONFIDENCE_CACHE, Tier::Cache);
                grounded.name = element_name;
                TierResult::Hit(grounded)
            } else {
                self.locator_cache.invalidate(&key);
                TierResult::Miss
            }
        })
    }
}

// ---------------------------------------------------------------------------
// UiaTier
// ---------------------------------------------------------------------------

/// Walks the UIA accessibility tree to find the element.
/// Applicable for Name and Role targets; not for Description/Text/Coords.
pub struct UiaTier<'s> {
    pub tree: &'s UiaTree,
    pub locator_cache: &'s LocatorCache,
    pub hwnd: isize,
    /// If a UIA element was found, store its rect so act() can decide dispatch path.
    /// We surface it via the Grounded rect; act() then invokes focus-independent
    /// InvokePattern/SetValue via find() when the tier wins.
    _phantom: std::marker::PhantomData<&'s ()>,
}

impl<'s> UiaTier<'s> {
    pub fn new(tree: &'s UiaTree, locator_cache: &'s LocatorCache, hwnd: isize) -> Self {
        Self { tree, locator_cache, hwnd, _phantom: std::marker::PhantomData }
    }
}

impl<'s> GroundingTier for UiaTier<'s> {
    fn tier(&self) -> Tier {
        Tier::Uia
    }

    fn locate<'a>(
        &'a self,
        target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async move {
            let found = match target {
                Target::Name(n) => self.tree.find_by_name_fast(n),
                Target::Role(r) => self.tree.find_by_role_fast(r),
                Target::Description(_) | Target::Text(_) | Target::Coords(_, _) => {
                    return TierResult::NotApplicable;
                }
            };

            let el = match found {
                Ok(Some(e)) => e,
                Ok(None) => return TierResult::Miss,
                Err(_) => return TierResult::Miss,
            };

            let rect_raw = match el.bounding_rect() {
                Some(r) => (r.left, r.top, r.right, r.bottom),
                None => return TierResult::Miss,
            };

            // Upsert into locator cache so the next call can use CacheTier.
            let key = match target {
                Target::Name(n) => Some(LocatorKey::with_hwnd(self.hwnd, "*", n.as_str())),
                Target::Role(r) => Some(LocatorKey::with_hwnd(self.hwnd, r.as_str(), "*")),
                _ => None,
            };
            if let Some(k) = key {
                self.locator_cache.upsert(k, rect_raw);
            }

            // HIGH-2: populate name from the element name at grounding time.
            let element_name = Some(el.name());
            let mut grounded = Grounded::from_rect(rect_raw, CONFIDENCE_UIA, Tier::Uia);
            grounded.name = element_name;
            TierResult::Hit(grounded)
        })
    }
}

// ---------------------------------------------------------------------------
// OcrTier
// ---------------------------------------------------------------------------

/// WinRT OCR fallback. Applicable for Text targets and as a fallback for Name.
pub struct OcrTier<'s> {
    pub session: &'s crate::session::GhostSession,
}

impl<'s> GroundingTier for OcrTier<'s> {
    fn tier(&self) -> Tier {
        Tier::Ocr
    }

    fn locate<'a>(
        &'a self,
        target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async move {
            let needle = match target {
                Target::Text(t) => t.as_str(),
                Target::Name(n) => n.as_str(), // OCR fallback for Name when UIA misses
                Target::Description(_) | Target::Role(_) | Target::Coords(_, _) => {
                    return TierResult::NotApplicable;
                }
            };

            match self.session.find_text_local(needle, true).await {
                Ok(Some((x, y))) => {
                    // OCR returns center pixel; produce a 1px Grounded.
                    TierResult::Hit(Grounded::from_point((x, y), CONFIDENCE_OCR, Tier::Ocr))
                }
                Ok(None) => TierResult::Miss,
                Err(e) => {
                    tracing::warn!(error=%e, "OcrTier locate failed (treated as Miss)");
                    TierResult::Miss
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// VlmTier
// ---------------------------------------------------------------------------

/// Cloud VLM fallback (NVIDIA / Anthropic). Applicable for Description and as a
/// last-resort for Name/Text.  Called in LocateMode::Deliberate AND on automatic
/// Instant-miss escalation (enforced by the engine; VlmTier itself is always willing).
pub struct VlmTier<'s> {
    pub session: &'s crate::session::GhostSession,
}

impl<'s> GroundingTier for VlmTier<'s> {
    fn tier(&self) -> Tier {
        Tier::Vlm
    }

    fn locate<'a>(
        &'a self,
        target: &'a Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + 'a>> {
        Box::pin(async move {
            let description = match target {
                Target::Description(d) => d.as_str(),
                Target::Name(n) => n.as_str(),   // last-resort natural language hint
                Target::Text(t) => t.as_str(),    // last-resort text hint
                Target::Role(_) | Target::Coords(_, _) => {
                    return TierResult::NotApplicable;
                }
            };

            match self.session.locate_by_description(description).await {
                Ok((x, y)) => {
                    TierResult::Hit(Grounded::from_point((x, y), CONFIDENCE_VLM, Tier::Vlm))
                }
                // LOW (VlmTier error signal): log failures instead of silently swallowing
                // misconfigured-API-key and network errors.
                Err(e) => {
                    tracing::warn!(error=%e, "VlmTier locate failed (treated as Miss)");
                    TierResult::Miss
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// YoloTier (optional, feature = "yolo")
// ---------------------------------------------------------------------------

/// OmniParser-YOLO icon detector tier (T4).
///
/// Placed between OcrTier and VlmTier in the cascade.
/// Only instantiated when the `yolo` feature is enabled AND `YoloDetector::from_env()`
/// succeeds (i.e. `GHOST_YOLO_MODEL` points at a valid `.onnx` file).
///
/// Returns Miss today because `detect_icons` is a documented stub returning empty.
/// Becomes active once model inference is implemented.
#[cfg(feature = "yolo")]
pub struct YoloTier<'s> {
    pub session: &'s crate::session::GhostSession,
    pub detector: ghost_ground::yolo::YoloDetector,
}

#[cfg(feature = "yolo")]
impl<'s> ghost_ground::engine::GroundingTier for YoloTier<'s> {
    fn tier(&self) -> ghost_ground::types::Tier {
        ghost_ground::types::Tier::Yolo
    }

    fn locate<'a>(
        &'a self,
        target: &'a ghost_ground::types::Target,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ghost_ground::engine::TierResult> + 'a>> {
        Box::pin(async move {
            use ghost_ground::engine::TierResult;
            use ghost_ground::types::{Target, Grounded, Tier};

            // Coords are pre-grounded; Role has no visual anchor for YOLO.
            match target {
                Target::Coords(_, _) | Target::Role(_) => return TierResult::NotApplicable,
                Target::Name(_) | Target::Text(_) | Target::Description(_) => {}
            }

            // Capture foreground window region.
            let rect = self.session.foreground_window_rect();
            let rgba_result = tokio::task::spawn_blocking(move || {
                ghost_core::capture::capture_region_raw(rect)
            })
            .await;

            let (raw, w, h) = match rgba_result {
                Ok(Ok(r)) => r,
                _ => return TierResult::Miss,
            };

            // Run YOLO detection (stub: returns empty — will Miss today).
            let regions = self.detector.detect_icons(&raw, w as u32, h as u32);
            if regions.is_empty() {
                return TierResult::Miss;
            }

            // With regions present, use Set-of-Marks matching (future: ask VLM to pick ID).
            // For now, return the highest-confidence region center.
            let best = regions.iter().max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal));
            match best {
                Some(r) => {
                    let (cx, cy) = r.center();
                    let mut grounded = Grounded::from_point((cx, cy), ghost_ground::engine::CONFIDENCE_YOLO, Tier::Yolo);
                    grounded.name = None; // YOLO does not recover accessible name
                    TierResult::Hit(grounded)
                }
                None => TierResult::Miss,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helper: get current foreground HWND as isize (0 = none)
// ---------------------------------------------------------------------------

pub fn foreground_hwnd() -> isize {
    unsafe {
        let h = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        if h.is_invalid() { 0 } else { h.0 as isize }
    }
}

// ---------------------------------------------------------------------------
// Tests — pure logic, no COM
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ghost_ground::types::{Target, Tier};

    /// Verify that Target variants produce the expected Tier affinity.
    /// This is a pure-data test: no COM, no session.
    #[test]
    fn target_to_tier_affinity_name() {
        // Name → CacheTier / UiaTier / OcrTier / VlmTier all applicable
        let t = Target::Name("OK".into());
        assert_eq!(t.inner_str(), Some("OK"));
    }

    #[test]
    fn target_to_tier_affinity_description() {
        // Description → only VlmTier applicable
        let t = Target::Description("blue submit button".into());
        assert_eq!(t.inner_str(), Some("blue submit button"));
    }

    #[test]
    fn target_to_tier_affinity_coords_bypasses_all() {
        let t = Target::Coords(100, 200);
        assert_eq!(t.inner_str(), None);
    }

    #[test]
    fn tier_display_names_match_expected() {
        assert_eq!(Tier::Cache.to_string(), "cache");
        assert_eq!(Tier::Uia.to_string(), "uia");
        assert_eq!(Tier::Ocr.to_string(), "ocr");
        assert_eq!(Tier::Vlm.to_string(), "vlm");
    }
}
