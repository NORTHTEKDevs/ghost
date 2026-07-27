//! The two macOS privacy grants Ghost cannot work without.
//!
//! macOS gates automation behind TCC (Transparency, Consent, and Control). Unlike
//! Windows, where UI Automation works for any process on an interactive desktop,
//! on macOS *nothing* works until the user approves the binary by hand:
//!
//! | Grant | Apple API | What it gates |
//! | --- | --- | --- |
//! | Accessibility | `AXIsProcessTrusted` / `AXIsProcessTrustedWithOptions` | every `AXUIElement*` call |
//! | Screen Recording | `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess` | `CGWindowListCreateImage` |
//!
//! The grant is keyed to the **executable**, not the user or the app name. A
//! rebuilt binary at the same path is a different subject as far as TCC is
//! concerned, so a fresh `cargo build` generally means re-approving. That is why
//! `ghost doctor --mac` prompts rather than assuming.
//!
//! Every AX entry point in this backend calls [`require_accessibility`] first and
//! every capture entry point calls [`require_screen_recording`] first, so a
//! missing grant surfaces as one actionable sentence instead of a wall of
//! `kAXErrorCannotComplete`.

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::access::ScreenCaptureAccess;

use super::error::{MacError, MacResult, Permission};

/// Whether this binary is a trusted Accessibility client — `AXIsProcessTrusted`.
///
/// Never prompts. Safe to call in a loop while waiting for the user to flip the
/// switch in System Settings.
pub fn accessibility_granted() -> bool {
    unsafe { accessibility_sys::AXIsProcessTrusted() }
}

/// Ask the OS to show the "…would like to control this computer" dialog and add
/// this binary to the Accessibility list — `AXIsProcessTrustedWithOptions` with
/// `kAXTrustedCheckOptionPrompt: true`.
///
/// Returns the trust state *at the moment of the call*, which is almost always
/// `false` on a first run: the dialog is asynchronous and the user has not acted
/// yet. Callers must poll [`accessibility_granted`] afterwards.
pub fn prompt_accessibility() -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(accessibility_sys::kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        accessibility_sys::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

/// Whether this binary may capture the screen — `CGPreflightScreenCaptureAccess`.
///
/// Never prompts.
pub fn screen_recording_granted() -> bool {
    ScreenCaptureAccess.preflight()
}

/// Ask the OS for Screen Recording access — `CGRequestScreenCaptureAccess`.
///
/// As with Accessibility, a `false` return on first run means "the user has not
/// answered yet", not "denied forever". Note that macOS only re-reads this grant
/// for some clients on relaunch, so `ghost doctor --mac` tells the user to restart
/// Ghost if capture still fails after granting.
pub fn request_screen_recording() -> bool {
    ScreenCaptureAccess.request()
}

/// Gate every Accessibility call. Returns the actionable permission error rather
/// than letting the AX API fail opaquely.
pub fn require_accessibility() -> MacResult<()> {
    if accessibility_granted() {
        Ok(())
    } else {
        Err(MacError::permission(Permission::Accessibility))
    }
}

/// Gate every screen-capture call.
pub fn require_screen_recording() -> MacResult<()> {
    if screen_recording_granted() {
        Ok(())
    } else {
        Err(MacError::permission(Permission::ScreenRecording))
    }
}

/// A snapshot of both grants, for `ghost doctor --mac` to report in one row each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionState {
    pub accessibility: bool,
    pub screen_recording: bool,
}

impl PermissionState {
    /// Read both grants without prompting.
    pub fn probe() -> Self {
        PermissionState {
            accessibility: accessibility_granted(),
            screen_recording: screen_recording_granted(),
        }
    }

    pub fn all_granted(&self) -> bool {
        self.accessibility && self.screen_recording
    }

    /// The grants still missing, so the caller can name them without repeating
    /// the field-by-field check.
    pub fn missing(&self) -> Vec<Permission> {
        let mut out = Vec::new();
        if !self.accessibility {
            out.push(Permission::Accessibility);
        }
        if !self.screen_recording {
            out.push(Permission::ScreenRecording);
        }
        out
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn probing_permissions_never_panics_on_a_headless_runner() {
        // A CI runner has neither grant. The point of this test is that the
        // preflight calls are safe to make anyway — Ghost must be able to *ask*
        // without crashing, since that is how doctor reports a missing grant.
        let state = PermissionState::probe();
        assert_eq!(state.accessibility, accessibility_granted());
        assert_eq!(state.screen_recording, screen_recording_granted());
    }

    #[test]
    fn require_helpers_agree_with_the_probe_in_both_directions() {
        let state = PermissionState::probe();

        assert_eq!(require_accessibility().is_ok(), state.accessibility);
        assert_eq!(require_screen_recording().is_ok(), state.screen_recording);

        // Whichever way the runner is configured, a failure must be the
        // actionable permission error and never a generic one.
        if let Err(e) = require_accessibility() {
            assert!(e.is_permission_denied());
            assert!(e.to_string().contains("Accessibility"), "{e}");
        }
        if let Err(e) = require_screen_recording() {
            assert!(e.is_permission_denied());
            assert!(e.to_string().contains("Screen Recording"), "{e}");
        }
    }

    #[test]
    fn missing_lists_exactly_the_ungranted_permissions() {
        let none = PermissionState {
            accessibility: false,
            screen_recording: false,
        };
        assert_eq!(
            none.missing(),
            vec![Permission::Accessibility, Permission::ScreenRecording]
        );
        assert!(!none.all_granted());

        let partial = PermissionState {
            accessibility: true,
            screen_recording: false,
        };
        assert_eq!(partial.missing(), vec![Permission::ScreenRecording]);
        assert!(!partial.all_granted());

        let all = PermissionState {
            accessibility: true,
            screen_recording: true,
        };
        assert!(all.missing().is_empty());
        assert!(all.all_granted());
    }
}
