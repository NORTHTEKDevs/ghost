//! Focus policy: the gate that decides whether an action is allowed to touch the
//! user's real cursor, keyboard focus, or foreground window.
//!
//! Ghost's headline claim is that it automates *without* taking over the screen.
//! That claim is only true if focus-stealing primitives (`SendInput`,
//! `SetForegroundWindow`) are opt-in rather than the default. This module makes
//! them opt-in.
//!
//! Three policies:
//! - `Background` (default) - never touch the real cursor/foreground. If an action
//!   has no background path, it fails with `NoBackgroundPath` naming the action.
//! - `PreferBackground` - try the background path first, fall back to foreground
//!   input (serialized behind the cross-process lease).
//! - `Foreground` - legacy behavior: drive the real cursor and keyboard directly.
//!
//! Set via `set_policy()`, the `GHOST_FOCUS_POLICY` env var, or the
//! `ghost_set_focus_policy` MCP tool.

use crate::error::CoreError;
use std::sync::atomic::{AtomicU8, Ordering};

const BACKGROUND: u8 = 0;
const PREFER_BACKGROUND: u8 = 1;
const FOREGROUND: u8 = 2;
const UNSET: u8 = 255;

static POLICY: AtomicU8 = AtomicU8::new(UNSET);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPolicy {
    /// Never steal the cursor, keyboard focus, or foreground window.
    Background,
    /// Try background first; fall back to real input if no background path exists.
    PreferBackground,
    /// Always use real input (SendInput / SetForegroundWindow).
    Foreground,
}

impl std::str::FromStr for FocusPolicy {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "background" | "bg" | "strict" => Ok(FocusPolicy::Background),
            "prefer_background" | "prefer" | "auto" => Ok(FocusPolicy::PreferBackground),
            "foreground" | "fg" | "legacy" => Ok(FocusPolicy::Foreground),
            _ => Err(()),
        }
    }
}

impl FocusPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            FocusPolicy::Background => "background",
            FocusPolicy::PreferBackground => "prefer_background",
            FocusPolicy::Foreground => "foreground",
        }
    }

    fn code(self) -> u8 {
        match self {
            FocusPolicy::Background => BACKGROUND,
            FocusPolicy::PreferBackground => PREFER_BACKGROUND,
            FocusPolicy::Foreground => FOREGROUND,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            PREFER_BACKGROUND => FocusPolicy::PreferBackground,
            FOREGROUND => FocusPolicy::Foreground,
            _ => FocusPolicy::Background,
        }
    }
}

/// Read the policy from the `GHOST_FOCUS_POLICY` env var, defaulting to `Background`.
/// Called once lazily; an explicit `set_policy` always wins afterwards.
fn resolve_initial() -> u8 {
    let code = std::env::var("GHOST_FOCUS_POLICY")
        .ok()
        .and_then(|v| v.parse::<FocusPolicy>().ok())
        .unwrap_or(FocusPolicy::Background)
        .code();
    // Only store if still unset; a concurrent set_policy must not be clobbered.
    let _ = POLICY.compare_exchange(UNSET, code, Ordering::SeqCst, Ordering::SeqCst);
    POLICY.load(Ordering::SeqCst)
}

/// The active focus policy for this process.
pub fn policy() -> FocusPolicy {
    let raw = POLICY.load(Ordering::SeqCst);
    let raw = if raw == UNSET { resolve_initial() } else { raw };
    FocusPolicy::from_code(raw)
}

/// Override the focus policy for this process.
pub fn set_policy(p: FocusPolicy) {
    POLICY.store(p.code(), Ordering::SeqCst);
}

/// True when the current policy forbids touching the real cursor / foreground.
pub fn is_background_only() -> bool {
    policy() == FocusPolicy::Background
}

/// True when the current policy permits real input injection.
pub fn foreground_allowed() -> bool {
    policy() != FocusPolicy::Background
}

thread_local! {
    /// Set on threads bound to an isolated desktop (see `ghost_core::desktop`).
    static ISOLATED_DESKTOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the calling thread as bound to an isolated desktop.
///
/// Only `DesktopSession`'s worker calls this, immediately after `SetThreadDesktop`.
pub fn mark_isolated_desktop_thread() {
    ISOLATED_DESKTOP.with(|c| c.set(true));
}

/// True when the calling thread's input goes to an isolated desktop rather than the
/// user's.
pub fn on_isolated_desktop() -> bool {
    ISOLATED_DESKTOP.with(|c| c.get())
}

/// Guard for a primitive that can only work by stealing the screen.
///
/// Returns `NoBackgroundPath` under `Background`, `Ok(())` otherwise. Call this at
/// the top of every `SendInput` / `SetForegroundWindow` wrapper so no code path can
/// quietly grab the user's screen.
///
/// Threads bound to an isolated desktop are exempt, and that is not a loophole: the
/// policy exists to protect *the user's* cursor, focus, and screen. A separate
/// desktop has its own input queue and is not displayed, so input issued there
/// cannot reach the user's session at all.
///
/// The exemption is about *policy*, not capability. `SendInput` still fails on a
/// non-displayed desktop - Windows returns ERROR_ACCESS_DENIED off the input
/// desktop, which is why `DesktopSession::real_input_supported()` is `false`. What
/// the exemption buys is the message-queue input path (`BackgroundClicker`,
/// `EditCommand`) running unguarded there. An app that answers *only* real
/// hardware input - a game, some canvas UIs - cannot be driven on an isolated
/// desktop; that case needs the `foreground` policy on the user's own desktop.
pub fn require_foreground_allowed(action: &'static str) -> Result<(), CoreError> {
    if on_isolated_desktop() || foreground_allowed() {
        Ok(())
    } else {
        Err(CoreError::NoBackgroundPath { action })
    }
}

/// Serialises the tests in this crate that change the process-global policy.
///
/// Without it two tests race: one flips the policy to `Foreground` for a moment
/// while another asserts that a real click is refused - and under `Foreground`
/// that click is REAL, at whatever coordinates the test used, on the human's
/// desktop. Observed once as a flaky failure of the desktop test; the click it
/// implies is the worse outcome. Every test that calls `set_policy` holds this.
#[cfg(test)]
pub(crate) fn policy_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `f` while temporarily forcing `p`, restoring the previous policy afterwards.
/// Used by callers that explicitly opt a single action into foreground input.
pub fn with_policy<T>(p: FocusPolicy, f: impl FnOnce() -> T) -> T {
    let prev = policy();
    set_policy(p);
    let out = f();
    set_policy(prev);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate process-global state, so they run under one #[test] to
    // avoid cross-test interference from cargo's parallel test threads.
    #[test]
    fn policy_parsing_and_gating() {
        let _serial = policy_test_lock();
        assert_eq!(
            "background".parse::<FocusPolicy>(),
            Ok(FocusPolicy::Background)
        );
        assert_eq!("BG".parse::<FocusPolicy>(), Ok(FocusPolicy::Background));
        assert_eq!(
            "prefer-background".parse::<FocusPolicy>(),
            Ok(FocusPolicy::PreferBackground)
        );
        assert_eq!(
            "foreground".parse::<FocusPolicy>(),
            Ok(FocusPolicy::Foreground)
        );
        assert!("nonsense".parse::<FocusPolicy>().is_err());

        set_policy(FocusPolicy::Background);
        assert!(is_background_only());
        assert!(!foreground_allowed());
        assert!(matches!(
            require_foreground_allowed("click"),
            Err(CoreError::NoBackgroundPath { action: "click" })
        ));

        set_policy(FocusPolicy::PreferBackground);
        assert!(!is_background_only());
        assert!(foreground_allowed());
        assert!(require_foreground_allowed("click").is_ok());

        set_policy(FocusPolicy::Foreground);
        assert!(foreground_allowed());

        let seen = with_policy(FocusPolicy::Background, policy);
        assert_eq!(seen, FocusPolicy::Background);
        assert_eq!(policy(), FocusPolicy::Foreground, "policy must be restored");

        // Leave the process in the safe default for any later test in this binary.
        set_policy(FocusPolicy::Background);
    }
}
