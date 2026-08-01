//! Display-session detection.
//!
//! The single most consequential fact about automating a Linux desktop is
//! whether the session is X11 or Wayland, because synthetic input and screen
//! capture differ completely between them. AT-SPI, by contrast, is identical on
//! both -- which is why Ghost prefers it.

use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A real X11 session. XTEST input and `GetImage` capture both work against
    /// the whole desktop.
    X11,
    /// A Wayland session. Synthetic input goes through the RemoteDesktop portal
    /// (or uinput); capture goes through the Screenshot portal. XTEST reaches
    /// XWayland clients only, and root-window capture does not work.
    Wayland,
    /// No display server detected -- headless, or a service without session
    /// environment. AT-SPI may still work if an accessibility bus is reachable.
    Headless,
}

impl SessionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionKind::X11 => "x11",
            SessionKind::Wayland => "wayland",
            SessionKind::Headless => "headless",
        }
    }

    /// Whether XTEST can be expected to drive native clients.
    pub fn xtest_is_authoritative(&self) -> bool {
        matches!(self, SessionKind::X11)
    }
}

/// Detect the session type.
///
/// `XDG_SESSION_TYPE` is authoritative when present; otherwise fall back to the
/// display socket variables. Note that `DISPLAY` is *also* set in a Wayland
/// session when XWayland is running, so Wayland is checked first.
pub fn session_kind() -> SessionKind {
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => return SessionKind::Wayland,
        Ok("x11") => return SessionKind::X11,
        _ => {}
    }
    if env::var_os("WAYLAND_DISPLAY").is_some() {
        return SessionKind::Wayland;
    }
    if env::var_os("DISPLAY").is_some() {
        return SessionKind::X11;
    }
    SessionKind::Headless
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_kind_strings_are_stable() {
        assert_eq!(SessionKind::X11.as_str(), "x11");
        assert_eq!(SessionKind::Wayland.as_str(), "wayland");
        assert_eq!(SessionKind::Headless.as_str(), "headless");
    }

    #[test]
    fn only_x11_makes_xtest_authoritative() {
        assert!(SessionKind::X11.xtest_is_authoritative());
        assert!(!SessionKind::Wayland.xtest_is_authoritative());
        assert!(!SessionKind::Headless.xtest_is_authoritative());
    }
}
