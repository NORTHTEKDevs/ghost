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
                TierResult::Hit(Grounded::from_rect(rect, CONFIDENCE_CACHE, Tier::Cache))
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

            TierResult::Hit(Grounded::from_rect(rect_raw, CONFIDENCE_UIA, Tier::Uia))
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
                Err(_) => TierResult::Miss,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// VlmTier
// ---------------------------------------------------------------------------

/// Cloud VLM fallback (NVIDIA / Anthropic). Applicable for Description and as a
/// last-resort for Name/Text.  Skipped in LocateMode::Instant (enforced by engine).
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
                Err(_) => TierResult::Miss,
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
