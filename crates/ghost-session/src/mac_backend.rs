//! macOS implementation of [`SessionBackend`], delegating to
//! [`ghost_platform::macos::MacBackend`].
//!
//! This module is a thin adapter and nothing more. All of the Accessibility,
//! CGEvent and CGWindowList work lives in `ghost-platform`; the only jobs here are
//! translating [`ghost_platform::macos::MacError`] into [`GhostError`] and satisfying
//! the async signature over what is really synchronous C FFI.
//!
//! # Why the permission error is translated specially
//!
//! A missing TCC grant is the single most likely reason this backend fails on a
//! machine that has never run it, and it is not a bug — it is a dialog the user has
//! not clicked yet. Collapsing it into a generic error string would leave the user
//! reading "operation failed" with no idea that the fix is two clicks away in System
//! Settings, so [`GhostError::Config`] carries the pane name through.

use async_trait::async_trait;
use ghost_platform::macos::{MacBackend, MacError};
use ghost_platform::{Capabilities, ElementInfo, Locator, Platform, Point, WindowRef};

use crate::backend::SessionBackend;
use crate::error::{GhostError, Result};

/// The macOS session engine.
///
/// Note that [`Capabilities::functional`] is false for this backend: the native code
/// exists and compiles, but has not been verified on a Mac. `ghost doctor --mac` is
/// what changes that.
pub struct MacSessionBackend {
    inner: MacBackend,
}

impl MacSessionBackend {
    pub fn new() -> Result<Self> {
        Ok(Self { inner: MacBackend })
    }

    /// The two TCC grants Ghost needs, probed without prompting.
    pub fn permissions(&self) -> ghost_platform::macos::perms::PermissionState {
        self.inner.permissions()
    }

    /// The underlying platform backend, for `ghost doctor --mac`, which needs
    /// finer-grained access than the [`SessionBackend`] trait exposes.
    pub fn platform_backend(&self) -> &MacBackend {
        &self.inner
    }
}

/// Map a macOS backend error onto Ghost's error type.
///
/// The permission case is preserved as an actionable sentence rather than being
/// flattened, because it is the failure a first-run user will actually hit.
fn map_err(e: MacError) -> GhostError {
    match &e {
        MacError::PermissionDenied { pane, .. } => GhostError::Config(format!(
            "{e} — grant it in System Settings > Privacy & Security > {pane}, then run Ghost again"
        )),
        MacError::ElementNotFound(query) => GhostError::ElementNotFound {
            query: query.clone(),
            screenshot: None,
        },
        MacError::WindowNotFound(query) => GhostError::ProcessNotFound { name: query.clone() },
        MacError::Unsupported(what) => GhostError::ElementNotInteractable {
            element: what.clone(),
            reason: "not supported by the macOS backend".to_string(),
        },
        _ => GhostError::Platform(e.to_string()),
    }
}

#[async_trait(?Send)]
impl SessionBackend for MacSessionBackend {
    fn platform(&self) -> Platform {
        Platform::MacOS
    }

    fn capabilities(&self) -> Capabilities {
        ghost_platform::capabilities_for(Platform::MacOS)
    }

    async fn list_windows(&self) -> Result<Vec<WindowRef>> {
        self.inner.list_windows().map_err(map_err)
    }

    async fn focus_window(&self, query: &str) -> Result<()> {
        self.inner.focus_window(query).map_err(map_err)
    }

    async fn snapshot(&self, window: &str) -> Result<Vec<ElementInfo>> {
        self.inner.snapshot(window).map_err(map_err)
    }

    async fn find(&self, window: &str, locator: &Locator) -> Result<ElementInfo> {
        self.inner.find(window, locator).map_err(map_err)
    }

    async fn click(&self, window: &str, locator: &Locator) -> Result<ElementInfo> {
        self.inner.click(window, locator).map_err(map_err)
    }

    async fn click_at(&self, point: Point) -> Result<()> {
        self.inner.click_at(point).map_err(map_err)
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.inner.type_text(text).map_err(map_err)
    }

    async fn press_key(&self, modifiers: &[String], key: &str) -> Result<()> {
        // `hotkey` handles the empty-modifier case, so there is no need to branch
        // between it and `press_key` here.
        self.inner.hotkey(modifiers, key).map_err(map_err)
    }

    async fn read_value(&self, window: &str, locator: &Locator) -> Result<Option<String>> {
        self.inner.read_value(window, locator).map_err(map_err)
    }

    async fn screenshot_window(&self, window: &str) -> Result<Vec<u8>> {
        self.inner
            .screenshot_window(window)
            .map(|c| c.png)
            .map_err(map_err)
    }

    async fn screenshot(&self) -> Result<Vec<u8>> {
        self.inner.screenshot().map(|c| c.png).map_err(map_err)
    }

    async fn get_clipboard(&self) -> Result<Option<String>> {
        self.inner.get_clipboard().map_err(map_err)
    }

    async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.inner.set_clipboard(text).map_err(map_err)
    }

    async fn frontmost_app(&self) -> Option<String> {
        self.inner.frontmost_app()
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn the_mac_backend_does_not_claim_to_be_verified() {
        let backend = MacSessionBackend::new().expect("construction touches no OS state");
        assert_eq!(backend.platform(), Platform::MacOS);
        assert!(
            !backend.capabilities().functional,
            "macOS must not report functional until ghost doctor --mac passes on hardware"
        );
    }

    #[test]
    fn a_missing_grant_becomes_an_error_that_says_where_to_click() {
        let err = map_err(MacError::permission(
            ghost_platform::macos::Permission::Accessibility,
        ));
        let msg = err.to_string();
        assert!(msg.contains("System Settings"), "{msg}");
        assert!(msg.contains("Accessibility"), "{msg}");
    }

    #[test]
    fn a_missing_element_keeps_its_query_for_the_error_message() {
        let err = map_err(MacError::ElementNotFound("Name(\"Submit\")".into()));
        assert!(err.to_string().contains("Submit"), "{err}");
    }
}
