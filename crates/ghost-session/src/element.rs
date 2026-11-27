use crate::engine::uia::{element::UiaElement, patterns};
use crate::engine::input::hotkey::is_stopped;
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

    /// True if the element is enabled (interactable). False also when the
    /// provider did not answer; actions use [`Self::is_definitely_disabled`].
    pub fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    /// True only when the element reports itself disabled. An action refuses
    /// on this, not on `!is_enabled()`, so a provider that errors on IsEnabled
    /// is still driven.
    pub fn is_definitely_disabled(&self) -> bool {
        refuse_as_disabled(self.inner.is_enabled_checked())
    }

    /// True if the element is a text-editable control (edit/document/combobox) - 
    /// safe to clear (Ctrl+A+Delete) and paste into.
    pub fn is_editable(&self) -> bool {
        crate::engine::uia::patterns::is_editable_role(self.inner.control_type())
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
        if self.is_definitely_disabled() {
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
        if self.is_definitely_disabled() {
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

    /// The element's value for a read or a typing read-back: the ValuePattern
    /// value, else the control's WM_GETTEXT; the name only for a non-editable
    /// role with no ValuePattern (see [`pick_value`]).
    pub fn typed_value(&self) -> String {
        read_value(&self.inner)
    }

    /// Get the element's current text SELECTION via TextPattern (empty if none /
    /// unsupported). Reads a selection without clobbering the clipboard.
    pub fn get_selection(&self) -> String {
        patterns::get_selection(&self.inner).unwrap_or_default()
    }

    /// Set UIA focus to this element. Used before keyboard actions to ensure
    /// input lands in the right control.
    pub fn set_focus(&self) -> Result<()> {
        self.inner.set_focus().map_err(GhostError::Core)
    }

    /// The element's native Win32 window handle (0 if windowless). Nonzero means
    /// the control can be driven with window messages (true background dispatch).
    pub fn native_window_handle(&self) -> isize {
        self.inner.native_window_handle()
    }

    /// UIA control-type id (e.g. 50000 = button).
    pub fn control_type(&self) -> u32 {
        self.inner.control_type()
    }

    /// Background click: InvokePattern ONLY. Never foregrounds the window, never
    /// moves the real cursor, never falls back to a coordinate click. Errors with
    /// `NotActionableInBackground` if the element exposes no InvokePattern - the
    /// caller must NOT silently steal focus in that case.
    pub fn click_background(&self) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if self.is_definitely_disabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::invoke_ex(&self.inner, false).map_err(GhostError::Core)
    }

    /// Background type: ValuePattern.SetValue ONLY. No foreground, no cursor, no
    /// keyboard fallback. Errors with `NotActionableInBackground` if there is no
    /// ValuePattern.
    pub fn type_text_background(&self, text: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if self.is_definitely_disabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::set_value_ex(&self.inner, text, false).map_err(GhostError::Core)
    }
}

// Note: GhostElement wraps live COM objects (IUIAutomationElement) which require a
// running Windows UIA server. Unit tests cannot meaningfully test this without
// mocking COM or spinning up a real UIA server. Integration tests are in tests/notepad.rs.

/// Whether an action must refuse an element as disabled, given its enabled
/// state (`None` = the accessibility provider did not answer). Only a definite
/// "disabled" refuses: a provider that errors on IsEnabled used to read as
/// disabled, so every act path refused an element it could have driven.
pub(crate) fn refuse_as_disabled(enabled: Option<bool>) -> bool {
    enabled == Some(false)
}

/// The value a read reports for a bare element: see [`pick_value`].
pub(crate) fn read_value(el: &crate::engine::uia::element::UiaElement) -> String {
    let role = effective_role_name(el);
    pick_value(role, el.value_text(), || window_text(el), || el.name())
}

/// The role Ghost reports for `el`. On Linux the raw AT-SPI role of an
/// editable text field is "text"/"paragraph"; the normalised role says "edit",
/// which is what decides whether a value read may fall back to the name.
pub(crate) fn effective_role_name(el: &crate::engine::uia::element::UiaElement) -> &'static str {
    #[cfg(target_os = "linux")]
    {
        el.role_name()
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::engine::uia::element::role_id_to_name(el.control_type())
    }
}

#[cfg(windows)]
fn window_text(el: &crate::engine::uia::element::UiaElement) -> Option<String> {
    let ctrl = el.native_window_handle();
    if ctrl == 0 {
        return None;
    }
    crate::engine::input::BackgroundClicker::read_text(ctrl)
}

#[cfg(not(windows))]
fn window_text(_el: &crate::engine::uia::element::UiaElement) -> Option<String> {
    None
}

/// An element's value: the ValuePattern value when it has text, else the
/// control's own window text. An editable role (edit/document/combobox) never
/// reads its name: a wait that fell back to the name compared against a label
/// ("Field"), not against what the field contains. Any other role with no
/// ValuePattern at all (`value` is `None`) reads its name, because for a
/// windowless label or status text (WPF/UWP/Chromium text, a status bar) the
/// name IS the visible text.
pub(crate) fn pick_value(
    role: &str,
    value: Option<String>,
    window_text: impl FnOnce() -> Option<String>,
    name: impl FnOnce() -> String,
) -> String {
    let has_value_pattern = value.is_some();
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        return v;
    }
    if let Some(t) = window_text().filter(|t| !t.is_empty()) {
        return t;
    }
    if has_value_pattern || is_editable_role_name(role) {
        return String::new();
    }
    name()
}

/// [`crate::engine::uia::patterns::is_editable_role`] by role name.
pub(crate) fn is_editable_role_name(role: &str) -> bool {
    matches!(role, "edit" | "document" | "combobox")
}

#[cfg(test)]
mod value_read_tests {
    //! A value read takes the ValuePattern value, else the control's own
    //! window text. Only a non-editable role with no ValuePattern reads its name.
    use super::pick_value;

    fn no_name() -> String {
        panic!("an editable role must never read its name")
    }

    #[test]
    fn the_value_pattern_wins_when_it_has_text() {
        assert_eq!(pick_value("edit", Some("hello".into()), || panic!("not needed"), no_name), "hello");
    }

    #[test]
    fn an_empty_value_falls_back_to_the_window_text() {
        // A classic Win32 EDIT: the proxy's value is empty, WM_GETTEXT has it.
        assert_eq!(pick_value("edit", Some(String::new()), || Some("hello".into()), no_name), "hello");
        assert_eq!(pick_value("edit", None, || Some("hello".into()), no_name), "hello");
    }

    #[test]
    fn an_editable_role_with_nothing_readable_is_empty_never_the_name() {
        for role in ["edit", "document", "combobox"] {
            assert_eq!(pick_value(role, None, || None, no_name), "", "{role}");
            assert_eq!(pick_value(role, Some(String::new()), || None, no_name), "", "{role}");
        }
    }

    #[test]
    fn a_windowless_label_or_status_text_reads_its_name() {
        for role in ["text", "statusbar"] {
            assert_eq!(pick_value(role, None, || None, || "Done".into()), "Done", "{role}");
        }
    }

    #[test]
    fn a_non_editable_role_with_a_value_pattern_does_not_read_its_name() {
        // An empty ValuePattern is an answer: the control has a value and it is empty.
        assert_eq!(pick_value("text", Some(String::new()), || None, || "Done".into()), "");
    }
}

#[cfg(test)]
mod refuse_tests {
    use super::refuse_as_disabled;

    #[test]
    fn a_definitely_disabled_element_is_refused() {
        assert!(refuse_as_disabled(Some(false)));
    }

    #[test]
    fn an_enabled_element_is_not_refused() {
        assert!(!refuse_as_disabled(Some(true)));
    }

    #[test]
    fn an_element_whose_provider_did_not_answer_is_not_refused() {
        assert!(!refuse_as_disabled(None));
    }
}
