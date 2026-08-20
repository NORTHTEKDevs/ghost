use crate::error::{GhostError, Result};
use ghost_core::input::hotkey::is_stopped;
use ghost_core::uia::{element::UiaElement, patterns};

pub use ghost_core::uia::patterns::ActionRoute;

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

    fn guard(&self, action: &str) -> Result<()> {
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        if !self.inner.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: format!("element is disabled (cannot {action})"),
            });
        }
        Ok(())
    }

    /// Activate this element. Tries Invoke, Select, Toggle, ExpandCollapse, then the
    /// MSAA default action - all of which run inside the target app without moving
    /// the cursor. Only falls back to a real click if the focus policy allows it.
    ///
    /// Returns the route actually taken, so callers can report whether the screen
    /// was touched instead of guessing.
    pub fn click(&self) -> Result<ActionRoute> {
        self.guard("click")?;
        patterns::invoke(&self.inner).map_err(GhostError::Core)
    }

    /// Set this element's text via ValuePattern (or the MSAA bridge), without
    /// focusing the window or typing into the user's keyboard stream.
    pub fn type_text(&self, text: &str) -> Result<ActionRoute> {
        self.guard("type")?;
        patterns::set_value(&self.inner, text).map_err(GhostError::Core)
    }

    /// Toggle a checkbox or toggle button.
    pub fn toggle(&self) -> Result<ActionRoute> {
        self.guard("toggle")?;
        patterns::toggle(&self.inner).map_err(GhostError::Core)
    }

    /// Select a tab, list item, or radio button.
    pub fn select(&self) -> Result<ActionRoute> {
        self.guard("select")?;
        patterns::select(&self.inner).map_err(GhostError::Core)
    }

    /// Open or close a combo box, tree item, or split button.
    pub fn expand_collapse(&self, expand: bool) -> Result<ActionRoute> {
        self.guard("expand_collapse")?;
        patterns::expand_collapse(&self.inner, expand).map_err(GhostError::Core)
    }

    /// Scroll this container. `direction` is "up"/"down"/"left"/"right".
    pub fn scroll(&self, direction: &str, amount: i32) -> Result<ActionRoute> {
        self.guard("scroll")?;
        patterns::scroll(&self.inner, direction, amount).map_err(GhostError::Core)
    }

    /// Bring this element into view inside its scrollable parent.
    pub fn scroll_into_view(&self) -> Result<ActionRoute> {
        self.guard("scroll_into_view")?;
        patterns::scroll_into_view(&self.inner).map_err(GhostError::Core)
    }

    /// Set a slider or spinner value.
    pub fn set_range_value(&self, value: f64) -> Result<ActionRoute> {
        self.guard("set_range_value")?;
        patterns::set_range_value(&self.inner, value).map_err(GhostError::Core)
    }

    /// Get the current text value of this element.
    pub fn get_text(&self) -> String {
        self.inner.get_text()
    }

    /// Full document text via TextPattern, which reads the whole body of an editor or
    /// document rather than a single control value. Falls back to `get_text`.
    pub fn document_text(&self, max_chars: i32) -> String {
        patterns::document_text(&self.inner, max_chars).unwrap_or_else(|| self.get_text())
    }

    /// Which background actions this element actually supports. Lets an agent pick a
    /// working action instead of guessing and falling back to the screen.
    pub fn supported_actions(&self) -> Vec<&'static str> {
        patterns::supported_actions(&self.inner)
    }
}

// Note: GhostElement wraps live COM objects (IUIAutomationElement) which require a
// running Windows UIA server. Unit tests cannot meaningfully test this without
// mocking COM or spinning up a real UIA server. Integration tests are in tests/notepad.rs.
