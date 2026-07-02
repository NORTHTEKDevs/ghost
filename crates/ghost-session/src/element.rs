use ghost_core::uia::{element::UiaElement, patterns};
use ghost_core::input::hotkey::is_stopped;
use crate::error::{GhostError, Result};

pub struct GhostElement {
    inner: UiaElement,
}

impl GhostElement {
    pub(crate) fn new(inner: UiaElement) -> Self {
        Self { inner }
    }

    /// The element's accessible name.
    pub fn name(&self) -> String {
        self.inner.name()
    }

    /// The element's bounding rectangle as (left, top, right, bottom).
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)> {
        self.inner.bounding_rect().map(|r| (r.left, r.top, r.right, r.bottom))
    }

    /// True if the element is enabled (interactable).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    /// True if the element is scrolled/collapsed out of view (stale rect).
    pub fn is_offscreen(&self) -> bool {
        self.inner.is_offscreen()
    }

    /// Best-effort scroll-into-view via ScrollItemPattern (no-op if unsupported).
    pub fn scroll_into_view(&self) -> Result<()> {
        self.inner.scroll_into_view().map_err(GhostError::Core)
    }

    /// Click this element using InvokePattern or coordinate fallback.
    ///
    /// Note: This is synchronous because `UiaElement` holds COM pointers which are `!Send`.
    /// COM objects require thread affinity and cannot be safely passed across `spawn_blocking`.
    pub fn click(&self) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if !self.inner.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::invoke(&self.inner).map_err(GhostError::Core)
    }

    /// Type text into this element using ValuePattern or keyboard fallback.
    ///
    /// Note: This is synchronous because `UiaElement` holds COM pointers which are `!Send`.
    /// COM objects require thread affinity and cannot be safely passed across `spawn_blocking`.
    pub fn type_text(&self, text: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if !self.inner.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::set_value(&self.inner, text).map_err(GhostError::Core)
    }

    /// Get the current text value of this element.
    pub fn get_text(&self) -> String {
        self.inner.get_text()
    }

    /// Set UIA focus to this element. Used before keyboard actions to ensure
    /// input lands in the right control.
    pub fn set_focus(&self) -> Result<()> {
        self.inner.set_focus().map_err(GhostError::Core)
    }
}

// Note: GhostElement wraps live COM objects (IUIAutomationElement) which require a
// running Windows UIA server. Unit tests cannot meaningfully test this without
// mocking COM or spinning up a real UIA server. Integration tests are in tests/notepad.rs.
