//! macOS backend — implemented, **not yet verified on-device**.
//!
//! This is a real native engine built on Accessibility, CoreGraphics events, and
//! CoreGraphics window capture. It is nonetheless reported as
//! `functional: false` by [`crate::capabilities_for`], and that is not an
//! oversight: no part of it has been executed on a Mac. Compiling and linking
//! against Apple's SDK in CI proves the FFI is well-formed; it proves nothing
//! about whether TextEdit actually receives a keystroke. The flag flips when
//! `ghost doctor --mac` passes on real hardware — see `docs/mac-testing.md`.
//!
//! # Layout
//!
//! | Module | Responsibility | Apple APIs |
//! | --- | --- | --- |
//! | [`ax`] | element discovery, act, read-back | `AXUIElement*` |
//! | [`input`] | keyboard and mouse synthesis | `CGEvent*` |
//! | [`capture`] | screenshots and point/pixel math | `CGWindowListCreateImage`, `CGDisplay*` |
//! | [`clipboard`] | copy/paste | `NSPasteboard` |
//! | [`window`] | enumeration and focus | `CGWindowListCopyWindowInfo`, `NSWorkspace` |
//! | [`perms`] | the two TCC grants | `AXIsProcessTrusted*`, `CGPreflightScreenCaptureAccess` |
//! | [`error`] | typed errors, exhaustive `AXError` | — |
//! | [`ffi`] | CoreFoundation conversions | `CF*` |
//!
//! # What is deliberately missing
//!
//! [`crate::Feature::BackgroundDispatch`] — driving an app without taking focus.
//! On Windows this is posted window messages (`BM_CLICK`, `WM_SETTEXT`), which
//! reach a control without activating its window. macOS has no equivalent:
//! CGEvent posts to the session-wide queue and goes to whatever is focused, and
//! while `AXUIElementPerformAction` and `AXUIElementSetAttributeValue` do not
//! *inherently* activate a window, whether they do in practice is up to each app's
//! accessibility provider. That makes it a measurement, not an implementation, and
//! it is out of scope here. Ghost does not claim capabilities it has not measured.

pub mod ax;
pub mod capture;
pub mod clipboard;
pub mod error;
pub mod ffi;
pub mod input;
pub mod perms;
pub mod window;

pub use error::{MacError, MacResult, Permission};

use crate::{capabilities_for, Backend, Capabilities, Feature, Platform, MAC_FEATURES};
use ax::AxElement;
use capture::Capture;
use input::{Modifier, MouseButton};
use window::MacWindow;
use crate::types::{ElementInfo, Locator, Point, WindowRef};

/// The macOS backend's declared feature set must be exactly Ghost's full feature
/// set minus [`Feature::BackgroundDispatch`].
///
/// Checked at compile time so that adding a tenth `Feature` to Ghost cannot
/// silently leave macOS behind: the count stops matching and the build fails.
const _: () = assert!(
    MAC_FEATURES.len() + 1 == crate::ALL_FEATURES.len(),
    "macOS must implement every Feature except BackgroundDispatch"
);

/// The macOS engine.
pub struct MacBackend;

impl Backend for MacBackend {
    fn platform(&self) -> Platform {
        Platform::MacOS
    }

    fn capabilities(&self) -> Capabilities {
        // Still `functional: false`. The native code exists and compiles; it has
        // not been run on a Mac.
        capabilities_for(Platform::MacOS)
    }
}

impl MacBackend {
    /// Both TCC grants, without prompting.
    pub fn permissions(&self) -> perms::PermissionState {
        perms::PermissionState::probe()
    }

    /// On-screen windows — `CGWindowListCopyWindowInfo`.
    pub fn list_windows(&self) -> MacResult<Vec<WindowRef>> {
        Ok(window::list_windows()?
            .iter()
            .map(MacWindow::as_window_ref)
            .collect())
    }

    /// Find a window by title substring, falling back to application name.
    pub fn find_window(&self, query: &str) -> MacResult<MacWindow> {
        window::find_window(query)
    }

    /// Bring a window's application forward and raise the window within it.
    pub fn focus_window(&self, query: &str) -> MacResult<()> {
        let target = window::find_window(query)?;
        window::focus_window(&target)
    }

    /// The localized name of the frontmost application, per
    /// `NSWorkspace.frontmostApplication`.
    pub fn frontmost_app(&self) -> Option<String> {
        window::frontmost_app_name()
    }

    /// The accessibility tree of a window, flattened —
    /// `AXUIElementCreateApplication` then a `kAXChildrenAttribute` walk.
    pub fn snapshot(&self, window_query: &str) -> MacResult<Vec<ElementInfo>> {
        let target = window::find_window(window_query)?;
        let app = AxElement::for_app(target.pid)?;

        // Prefer the specific AX window matching the title so a second document
        // window's elements do not leak into the snapshot.
        for ax_window in app.windows()? {
            let title = ax_window.name()?;
            if !target.title.is_empty() && ax::name_matches(&title, &target.title) {
                return ax_window.snapshot();
            }
        }
        // Titles were withheld, or the app exposes no window list: fall back to
        // the whole application tree rather than returning nothing.
        app.snapshot()
    }

    /// The AX element for a window, as the root of a search.
    pub fn window_element(&self, window_query: &str) -> MacResult<AxElement> {
        let target = window::find_window(window_query)?;
        let app = AxElement::for_app(target.pid)?;
        for ax_window in app.windows()? {
            let title = ax_window.name()?;
            if target.title.is_empty() || ax::name_matches(&title, &target.title) {
                return Ok(ax_window);
            }
        }
        Ok(app)
    }

    /// Locate one element beneath a window.
    ///
    /// [`Locator::Description`] is vision grounding, which lives in `ghost-ground`
    /// and needs a captured image plus a VLM — not something this backend resolves
    /// on its own, so it is rejected here rather than silently mis-handled.
    pub fn find(&self, window_query: &str, locator: &Locator) -> MacResult<ElementInfo> {
        // Checked before the window lookup so an unsupported locator reports itself
        // rather than being masked by whatever the window search happens to say.
        reject_vision_locator(locator)?;
        let root = self.window_element(window_query)?;
        let candidates = root.snapshot()?;

        let found = match locator {
            Locator::Name(name) => candidates
                .iter()
                .find(|e| e.actionable && ax::name_matches(&e.name, name))
                .or_else(|| candidates.iter().find(|e| ax::name_matches(&e.name, name))),
            Locator::Role(role) => candidates
                .iter()
                .find(|e| e.actionable && e.role.eq_ignore_ascii_case(role))
                .or_else(|| candidates.iter().find(|e| e.role.eq_ignore_ascii_case(role))),
            // Already rejected above; repeated so the match stays exhaustive without a
            // catch-all arm that would swallow a future variant.
            Locator::Description(_) => return Err(vision_locator_error()),
        };

        found
            .cloned()
            .ok_or_else(|| MacError::ElementNotFound(format!("{locator:?} in {window_query:?}")))
    }

    /// Click an element by locator: find it, then click its centre with a
    /// synthesized mouse event.
    ///
    /// Mouse synthesis is used rather than `kAXPressAction` because it works for
    /// every element regardless of whether its provider implements `AXPress`, and
    /// because it is what a user does. The trade-off is that it requires the window
    /// to be frontmost, so the window is focused first.
    pub fn click(&self, window_query: &str, locator: &Locator) -> MacResult<ElementInfo> {
        let element = self.find(window_query, locator)?;
        self.focus_window(window_query)?;
        input::click_at(element.rect.center(), MouseButton::Left, 1)?;
        Ok(element)
    }

    /// Click at an absolute screen point, in points.
    pub fn click_at(&self, point: Point) -> MacResult<()> {
        input::click_at(point, MouseButton::Left, 1)
    }

    /// Type text into a window's focused control, via Unicode CGEvents.
    pub fn type_text(&self, text: &str) -> MacResult<()> {
        input::type_text(text)
    }

    /// Set an element's value directly —
    /// `AXUIElementSetAttributeValue(kAXValueAttribute)`.
    ///
    /// Faster and focus-independent compared to typing, but apps that only watch
    /// for key events will not notice. Returns the value read back afterwards so
    /// the caller can verify.
    pub fn set_value(&self, window_query: &str, locator: &Locator, text: &str) -> MacResult<Option<String>> {
        let root = self.window_element(window_query)?;
        let target = self.find_ax(&root, locator)?;
        target.set_value(text)?;
        target.value_string()
    }

    /// Read an element's `kAXValueAttribute` — the verify half of act-then-verify.
    pub fn read_value(&self, window_query: &str, locator: &Locator) -> MacResult<Option<String>> {
        let root = self.window_element(window_query)?;
        let target = self.find_ax(&root, locator)?;
        target.value_string()
    }

    /// Walk the tree for the live `AxElement` behind a locator.
    ///
    /// [`find`] returns the inert [`ElementInfo`] an agent plans over; acting via
    /// AX needs the handle itself, which cannot be stored in that struct.
    fn find_ax(&self, root: &AxElement, locator: &Locator) -> MacResult<AxElement> {
        // Without this the walk below would match nothing and report "element not
        // found", which reads as "your description was wrong" rather than "this
        // backend cannot resolve descriptions at all".
        reject_vision_locator(locator)?;
        let mut queue = vec![root.clone()];
        let mut depth = 0;
        while let Some(current) = queue.pop() {
            depth += 1;
            if depth > 10_000 {
                break;
            }
            let matches = match locator {
                Locator::Name(name) => ax::name_matches(&current.name()?, name),
                Locator::Role(role) => ax::ghost_role(&current.role()?).eq_ignore_ascii_case(role),
                Locator::Description(_) => return Err(vision_locator_error()),
            };
            if matches {
                return Ok(current);
            }
            queue.extend(current.children()?);
        }
        Err(MacError::ElementNotFound(format!("{locator:?}")))
    }

    /// Press a key with optional modifiers.
    pub fn press_key(&self, key: &str, modifiers: &[Modifier]) -> MacResult<()> {
        input::press_key(key, modifiers)
    }

    /// A keyboard shortcut from modifier names, e.g. `["cmd"], "c"`.
    ///
    /// Modifiers are applied as `CGEventFlags` on the key event, never as separate
    /// synthetic keystrokes — see [`input`].
    pub fn hotkey(&self, modifiers: &[String], key: &str) -> MacResult<()> {
        input::hotkey(modifiers, key)
    }

    /// Capture the whole main display.
    pub fn screenshot(&self) -> MacResult<Capture> {
        capture::capture_screen()
    }

    /// Capture one window by title.
    pub fn screenshot_window(&self, window_query: &str) -> MacResult<Capture> {
        let target = window::find_window(window_query)?;
        capture::capture_window(target.window_id, target.bounds)
    }

    /// Read the clipboard as text.
    pub fn get_clipboard(&self) -> MacResult<Option<String>> {
        clipboard::get_text()
    }

    /// Replace the clipboard's text.
    pub fn set_clipboard(&self, text: &str) -> MacResult<()> {
        clipboard::set_text(text)
    }

    /// Whether a feature is claimed on macOS. `BackgroundDispatch` is always
    /// `false` here.
    pub fn supports(&self, feature: Feature) -> bool {
        MAC_FEATURES.contains(&feature)
    }
}

/// The error a [`Locator::Description`] earns from this backend.
fn vision_locator_error() -> MacError {
    MacError::Unsupported(
        "description locators are resolved by ghost-ground vision grounding, not by the macOS backend directly".into(),
    )
}

/// Reject a locator this backend cannot resolve, before any work is done.
///
/// Ordering matters: if the window lookup ran first, an unsupported locator would
/// surface as `WindowNotFound` whenever the window also happened to be missing,
/// which points the caller at the wrong problem.
fn reject_vision_locator(locator: &Locator) -> MacResult<()> {
    match locator {
        Locator::Name(_) | Locator::Role(_) => Ok(()),
        Locator::Description(_) => Err(vision_locator_error()),
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;
    use crate::all_features;

    #[test]
    fn the_mac_feature_set_is_everything_except_background_dispatch() {
        let backend = MacBackend;
        for feature in all_features() {
            let expected = feature != Feature::BackgroundDispatch;
            assert_eq!(
                backend.supports(feature),
                expected,
                "{feature:?} should be {}claimed on macOS",
                if expected { "" } else { "un" }
            );
        }
        assert!(!backend.supports(Feature::BackgroundDispatch));
        assert_eq!(MAC_FEATURES.len(), all_features().len() - 1);
    }

    #[test]
    fn macos_still_reports_not_functional_because_nothing_has_run_on_a_mac() {
        // The honesty gate for this whole drop. Compiling is not verifying.
        let backend = MacBackend;
        assert_eq!(backend.platform(), Platform::MacOS);
        assert!(!backend.is_functional());
        assert!(!backend.capabilities().functional);
    }

    #[test]
    fn description_locators_are_refused_rather_than_mishandled() {
        // Vision grounding lives in ghost-ground and needs a VLM round trip.
        // Silently failing to match would look like "element not found".
        let backend = MacBackend;
        let err = backend
            .find("anything", &Locator::Description("the blue button".into()))
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ghost-ground"), "{msg}");
    }
}
