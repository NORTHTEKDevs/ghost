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
}

// Note: GhostElement wraps live COM objects (IUIAutomationElement) which require a
// running Windows UIA server. Unit tests cannot meaningfully test this without
// mocking COM or spinning up a real UIA server. Integration tests are in tests/notepad.rs.
