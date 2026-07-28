//! The seam between Ghost's public session API and a per-OS engine.
//!
//! Until this module existed there was exactly one engine — [`crate::session::GhostSession`]
//! over Win32 UIA — and its inherent methods *were* the API. That is fine with one
//! OS and untenable with two, so the operations that are genuinely common to every
//! platform are named here as a trait.
//!
//! # What belongs in this trait
//!
//! Only operations Ghost can express in [`ghost_platform`]'s neutral vocabulary —
//! [`Locator`], [`ElementInfo`], [`WindowRef`], [`Point`]. That is a deliberately
//! smaller surface than `GhostSession` offers on Windows. Vision grounding, intent
//! execution, OCR text search, scroll-until, form filling and background dispatch
//! all stay as Windows-only inherent methods, because they either depend on
//! Windows-only crates or describe a primitive macOS does not have. Widening the
//! trait to cover them would mean writing macOS stubs that return "unsupported",
//! which is a worse lie than not offering the method: a stub looks callable.
//!
//! # Why the trait is async
//!
//! The Windows engine is async throughout (it blocks on UIA round trips in a
//! `spawn_blocking`), and the public API is already `.await`-ed by every caller. The
//! macOS backend is synchronous C FFI underneath, so its implementations simply do
//! not await anything — a cost of nothing, in exchange for one shared signature.
//!
//! # Why the futures are not `Send`
//!
//! `#[async_trait(?Send)]` is not a shortcut taken to avoid fighting the borrow
//! checker; it is forced by COM. [`crate::session::GhostSession`] holds UIA
//! interface pointers, which have thread affinity and are `!Send` by construction,
//! and the binaries already honour that by pinning the session to one dedicated OS
//! thread and driving it with `block_on`. Requiring `Send` here would make the
//! Windows engine unable to implement its own trait. The macOS backend *is* `Send`,
//! so it loses nothing by being described in the weaker terms.

use async_trait::async_trait;
use ghost_platform::{Capabilities, ElementInfo, Locator, Platform, Point, WindowRef};

use crate::error::Result;

/// One OS's automation engine.
///
/// Implemented by [`crate::win_backend::WinBackend`] on Windows and
/// [`crate::mac_backend::MacSessionBackend`] on macOS; see [`Session`] for the enum
/// that dispatches between them.
#[async_trait(?Send)]
pub trait SessionBackend {
    /// Which OS this backend drives.
    fn platform(&self) -> Platform;

    /// What this backend can do, and — via [`Capabilities::functional`] — whether it
    /// has been verified on real hardware.
    fn capabilities(&self) -> Capabilities;

    /// On-screen windows.
    async fn list_windows(&self) -> Result<Vec<WindowRef>>;

    /// Bring a window to the foreground, matched by title substring.
    async fn focus_window(&self, query: &str) -> Result<()>;

    /// A flattened accessibility snapshot of one window, for an agent to plan over.
    async fn snapshot(&self, window: &str) -> Result<Vec<ElementInfo>>;

    /// Resolve a locator to a single element without acting on it.
    async fn find(&self, window: &str, locator: &Locator) -> Result<ElementInfo>;

    /// Click the element a locator resolves to, returning what was clicked.
    async fn click(&self, window: &str, locator: &Locator) -> Result<ElementInfo>;

    /// Click an absolute screen coordinate.
    async fn click_at(&self, point: Point) -> Result<()>;

    /// Type text into whatever currently has keyboard focus.
    async fn type_text(&self, text: &str) -> Result<()>;

    /// Press one key with optional modifier names (`["ctrl"], "c"`).
    async fn press_key(&self, modifiers: &[String], key: &str) -> Result<()>;

    /// Read an element's value — the verify half of act-then-verify.
    ///
    /// `Ok(None)` means the element has no value attribute, which is different from
    /// having an empty one.
    async fn read_value(&self, window: &str, locator: &Locator) -> Result<Option<String>>;

    /// PNG bytes of one window.
    async fn screenshot_window(&self, window: &str) -> Result<Vec<u8>>;

    /// PNG bytes of the whole screen.
    async fn screenshot(&self) -> Result<Vec<u8>>;

    /// Clipboard text, or `None` when the clipboard holds no text representation.
    async fn get_clipboard(&self) -> Result<Option<String>>;

    /// Replace the clipboard's text.
    async fn set_clipboard(&self, text: &str) -> Result<()>;

    /// The name of the frontmost application, when the OS will say.
    ///
    /// Used to verify that a focus change actually took effect, rather than trusting
    /// that the request was accepted.
    async fn frontmost_app(&self) -> Option<String>;
}

/// The host's automation engine, chosen at compile time.
///
/// This is the portable entry point: code that only needs the operations in
/// [`SessionBackend`] can be written once against `Session` and run on either OS.
///
/// # Why `GhostSession` is not an alias for this type
///
/// `GhostSession` exposes roughly sixty Windows-only methods — intent execution,
/// vision grounding, OCR text search, background dispatch, form filling. Aliasing it
/// to `Session` would require this enum to forward every one of them, and each
/// forwarded method would need a macOS arm that could only return "unsupported". A
/// method that exists and always fails is worse than one that does not exist, because
/// only the second is caught by the compiler. `GhostSession` therefore stays exactly
/// what it was, and [`Session::windows`] hands it back when a caller needs it.
pub enum Session {
    #[cfg(windows)]
    Windows(crate::win_backend::WinBackend),
    #[cfg(target_os = "macos")]
    MacOS(crate::mac_backend::MacSessionBackend),
}

impl Session {
    /// Build the engine for the host OS.
    ///
    /// `return` rather than trailing blocks: a cfg'd-out block still has to typecheck
    /// as a statement on the other host, and a statement block must evaluate to `()`.
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        return Ok(Session::Windows(crate::win_backend::WinBackend::new()?));

        #[cfg(target_os = "macos")]
        return Ok(Session::MacOS(
            crate::mac_backend::MacSessionBackend::new()?,
        ));
    }

    /// The full Windows engine, or `None` off Windows.
    ///
    /// The escape hatch for the Windows-only surface described on this type. Callers
    /// that need it are Windows-only themselves and can `expect` on the result.
    #[cfg(windows)]
    pub fn windows(&self) -> Option<&crate::session::GhostSession> {
        match self {
            Session::Windows(w) => Some(w.session()),
        }
    }

    /// The full Windows engine, or `None` off Windows.
    #[cfg(not(windows))]
    pub fn windows(&self) -> Option<&()> {
        None
    }

    /// The engine as a trait object, for code that is generic over the OS.
    pub fn backend(&self) -> &dyn SessionBackend {
        match self {
            #[cfg(windows)]
            Session::Windows(w) => w,
            #[cfg(target_os = "macos")]
            Session::MacOS(m) => m,
        }
    }
}
