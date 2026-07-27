//! Typed errors for the macOS backend.
//!
//! Every Accessibility call returns an `AXError` (a C `int32_t`). This module
//! turns that integer into an exhaustive Rust enum so a failure can never be
//! silently swallowed, and so the two permission failures — the ones a user can
//! actually fix — are distinguishable from genuine bugs.

use accessibility_sys::{
    kAXErrorAPIDisabled, kAXErrorActionUnsupported, kAXErrorAttributeUnsupported,
    kAXErrorCannotComplete, kAXErrorFailure, kAXErrorIllegalArgument, kAXErrorInvalidUIElement,
    kAXErrorInvalidUIElementObserver, kAXErrorNoValue, kAXErrorNotEnoughPrecision,
    kAXErrorNotImplemented, kAXErrorNotificationAlreadyRegistered,
    kAXErrorNotificationNotRegistered, kAXErrorNotificationUnsupported,
    kAXErrorParameterizedAttributeUnsupported, kAXErrorSuccess, AXError,
};

/// The two macOS privacy grants Ghost needs. Both are per-binary TCC entries: the
/// grant follows the *executable*, so a rebuilt or moved binary is a new subject
/// and must be re-approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// System Settings > Privacy & Security > Accessibility. Gates every AX call.
    Accessibility,
    /// System Settings > Privacy & Security > Screen Recording. Gates capture.
    ScreenRecording,
}

impl Permission {
    /// The exact System Settings pane a user must open, for error text.
    pub fn settings_pane(&self) -> &'static str {
        match self {
            Permission::Accessibility => "Privacy & Security > Accessibility",
            Permission::ScreenRecording => "Privacy & Security > Screen Recording",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::ScreenRecording => "screen-recording",
        }
    }
}

/// Every `AXError` the Accessibility API is documented to return, named.
///
/// `AXError` is a C `int32_t`, so an out-of-contract value is representable;
/// [`AxStatus::Unknown`] carries it rather than letting a `_` arm hide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxStatus {
    Success,
    Failure,
    IllegalArgument,
    InvalidUIElement,
    InvalidUIElementObserver,
    CannotComplete,
    AttributeUnsupported,
    ActionUnsupported,
    NotificationUnsupported,
    NotImplemented,
    NotificationAlreadyRegistered,
    NotificationNotRegistered,
    ApiDisabled,
    NoValue,
    ParameterizedAttributeUnsupported,
    NotEnoughPrecision,
    /// A value outside the documented set. Kept rather than collapsed so an
    /// unexpected OS response shows up in a bug report verbatim.
    Unknown(i32),
}

impl AxStatus {
    /// Classify a raw `AXError` returned by any `AXUIElement*` function.
    ///
    /// Matching on Apple's `kAXError*` names rather than on integer literals is
    /// deliberate: the numbers are meaningless to a reader and a transposed digit
    /// would be invisible. The lint allowance below is the cost of keeping them.
    #[allow(non_upper_case_globals)]
    pub fn from_raw(raw: AXError) -> Self {
        match raw {
            kAXErrorSuccess => AxStatus::Success,
            kAXErrorFailure => AxStatus::Failure,
            kAXErrorIllegalArgument => AxStatus::IllegalArgument,
            kAXErrorInvalidUIElement => AxStatus::InvalidUIElement,
            kAXErrorInvalidUIElementObserver => AxStatus::InvalidUIElementObserver,
            kAXErrorCannotComplete => AxStatus::CannotComplete,
            kAXErrorAttributeUnsupported => AxStatus::AttributeUnsupported,
            kAXErrorActionUnsupported => AxStatus::ActionUnsupported,
            kAXErrorNotificationUnsupported => AxStatus::NotificationUnsupported,
            kAXErrorNotImplemented => AxStatus::NotImplemented,
            kAXErrorNotificationAlreadyRegistered => AxStatus::NotificationAlreadyRegistered,
            kAXErrorNotificationNotRegistered => AxStatus::NotificationNotRegistered,
            kAXErrorAPIDisabled => AxStatus::ApiDisabled,
            kAXErrorNoValue => AxStatus::NoValue,
            kAXErrorParameterizedAttributeUnsupported => {
                AxStatus::ParameterizedAttributeUnsupported
            }
            kAXErrorNotEnoughPrecision => AxStatus::NotEnoughPrecision,
            other => AxStatus::Unknown(other),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, AxStatus::Success)
    }

    /// True when the status means "this app never had this attribute/action",
    /// which is normal tree-walking noise rather than a failure worth reporting.
    pub fn is_absent(&self) -> bool {
        matches!(
            self,
            AxStatus::NoValue
                | AxStatus::AttributeUnsupported
                | AxStatus::ParameterizedAttributeUnsupported
        )
    }

    /// `kAXErrorAPIDisabled` is the OS's way of saying the caller is not a
    /// trusted Accessibility client — i.e. the grant is missing, not a bug.
    pub fn is_permission_problem(&self) -> bool {
        matches!(self, AxStatus::ApiDisabled)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AxStatus::Success => "kAXErrorSuccess",
            AxStatus::Failure => "kAXErrorFailure",
            AxStatus::IllegalArgument => "kAXErrorIllegalArgument",
            AxStatus::InvalidUIElement => "kAXErrorInvalidUIElement",
            AxStatus::InvalidUIElementObserver => "kAXErrorInvalidUIElementObserver",
            AxStatus::CannotComplete => "kAXErrorCannotComplete",
            AxStatus::AttributeUnsupported => "kAXErrorAttributeUnsupported",
            AxStatus::ActionUnsupported => "kAXErrorActionUnsupported",
            AxStatus::NotificationUnsupported => "kAXErrorNotificationUnsupported",
            AxStatus::NotImplemented => "kAXErrorNotImplemented",
            AxStatus::NotificationAlreadyRegistered => "kAXErrorNotificationAlreadyRegistered",
            AxStatus::NotificationNotRegistered => "kAXErrorNotificationNotRegistered",
            AxStatus::ApiDisabled => "kAXErrorAPIDisabled",
            AxStatus::NoValue => "kAXErrorNoValue",
            AxStatus::ParameterizedAttributeUnsupported => {
                "kAXErrorParameterizedAttributeUnsupported"
            }
            AxStatus::NotEnoughPrecision => "kAXErrorNotEnoughPrecision",
            AxStatus::Unknown(_) => "unknown AXError",
        }
    }
}

/// Anything the macOS backend can fail with. No backend function panics; a
/// `Result` carrying one of these is always returned instead.
#[derive(Debug, thiserror::Error)]
pub enum MacError {
    /// A privacy grant is missing. This is the only error variant a user can fix
    /// themselves, so it renders as an instruction rather than a diagnosis.
    #[error("Ghost needs {permission:?} permission: open System Settings > {pane} and enable Ghost, then run this again")]
    PermissionDenied {
        permission: Permission,
        pane: &'static str,
    },

    /// An Accessibility call failed. `op` names the API so a report is actionable.
    #[error("{op} failed: {} ({raw})", status.as_str())]
    Ax {
        op: &'static str,
        status: AxStatus,
        raw: i32,
    },

    #[error("no element matched {0}")]
    ElementNotFound(String),

    #[error("element has no {0} attribute")]
    AttributeMissing(&'static str),

    #[error("no window matched {0}")]
    WindowNotFound(String),

    #[error("could not create a CGEvent source — the window server rejected the request")]
    EventSource,

    #[error("could not synthesize a {0} CGEvent")]
    EventCreation(&'static str),

    #[error("screen capture returned no image for {0}")]
    CaptureFailed(String),

    #[error("captured image was {0}")]
    CaptureUnusable(String),

    #[error("PNG encode failed: {0}")]
    Encode(String),

    #[error("clipboard: {0}")]
    Clipboard(&'static str),

    #[error("{0}")]
    Unsupported(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl MacError {
    /// Construct the permission error for a grant, filling in the settings pane.
    pub fn permission(permission: Permission) -> Self {
        MacError::PermissionDenied {
            permission,
            pane: permission.settings_pane(),
        }
    }

    /// True when the fix is "grant a permission", not "file a bug". `ghost doctor`
    /// uses this to decide whether to re-prompt or to report a hard failure.
    pub fn is_permission_denied(&self) -> bool {
        match self {
            MacError::PermissionDenied { .. } => true,
            MacError::Ax { status, .. } => status.is_permission_problem(),
            _ => false,
        }
    }
}

pub type MacResult<T> = Result<T, MacError>;

/// Turn a raw `AXError` from `op` into a `Result`, mapping the permission case to
/// [`MacError::PermissionDenied`] so callers do not have to special-case it.
pub fn check_ax(op: &'static str, raw: AXError) -> MacResult<()> {
    let status = AxStatus::from_raw(raw);
    if status.is_success() {
        return Ok(());
    }
    if status.is_permission_problem() {
        return Err(MacError::permission(Permission::Accessibility));
    }
    Err(MacError::Ax { op, status, raw })
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn every_documented_axerror_maps_to_a_named_variant() {
        let documented = [
            kAXErrorSuccess,
            kAXErrorFailure,
            kAXErrorIllegalArgument,
            kAXErrorInvalidUIElement,
            kAXErrorInvalidUIElementObserver,
            kAXErrorCannotComplete,
            kAXErrorAttributeUnsupported,
            kAXErrorActionUnsupported,
            kAXErrorNotificationUnsupported,
            kAXErrorNotImplemented,
            kAXErrorNotificationAlreadyRegistered,
            kAXErrorNotificationNotRegistered,
            kAXErrorAPIDisabled,
            kAXErrorNoValue,
            kAXErrorParameterizedAttributeUnsupported,
            kAXErrorNotEnoughPrecision,
        ];
        for raw in documented {
            let status = AxStatus::from_raw(raw);
            assert!(
                !matches!(status, AxStatus::Unknown(_)),
                "AXError {raw} fell through to Unknown"
            );
            assert_ne!(status.as_str(), "unknown AXError");
        }
    }

    #[test]
    fn undocumented_axerror_is_preserved_not_collapsed() {
        assert_eq!(AxStatus::from_raw(-1), AxStatus::Unknown(-1));
        assert_eq!(AxStatus::from_raw(12345), AxStatus::Unknown(12345));
    }

    #[test]
    fn success_is_ok_and_api_disabled_becomes_a_permission_error() {
        assert!(check_ax("probe", kAXErrorSuccess).is_ok());

        let err = check_ax("probe", kAXErrorAPIDisabled).unwrap_err();
        assert!(err.is_permission_denied());
        match err {
            MacError::PermissionDenied { permission, .. } => {
                assert_eq!(permission, Permission::Accessibility)
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_ax_failure_is_not_a_permission_problem() {
        let err = check_ax("probe", kAXErrorCannotComplete).unwrap_err();
        assert!(!err.is_permission_denied());
        // The op name and the symbolic status both survive into the message,
        // because that string is what lands in a bug report.
        let msg = err.to_string();
        assert!(msg.contains("probe"), "{msg}");
        assert!(msg.contains("kAXErrorCannotComplete"), "{msg}");
    }

    #[test]
    fn absent_attributes_are_distinguished_from_real_failures() {
        assert!(AxStatus::from_raw(kAXErrorNoValue).is_absent());
        assert!(AxStatus::from_raw(kAXErrorAttributeUnsupported).is_absent());
        assert!(!AxStatus::from_raw(kAXErrorCannotComplete).is_absent());
        assert!(!AxStatus::from_raw(kAXErrorSuccess).is_absent());
    }

    #[test]
    fn permission_error_names_the_settings_pane_a_user_must_open() {
        let err = MacError::permission(Permission::ScreenRecording);
        let msg = err.to_string();
        assert!(msg.contains("System Settings"), "{msg}");
        assert!(msg.contains("Screen Recording"), "{msg}");
    }
}
