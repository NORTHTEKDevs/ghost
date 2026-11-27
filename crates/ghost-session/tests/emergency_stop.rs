//! The emergency stop at the session layer, for the paths the live test
//! (`emergency_stop_live.rs`) does not reach: launch, focus, window state,
//! the registries (desktops, browsers, tabs), the focus policy, intents.
//!
//! Only the process-local `STOP_FLAG` is raised (never `trigger_stop`, whose
//! shared event would stop every Ghost process here), and only by the one test
//! in this binary that reads it. Every call
//! made after a reset names something that does not exist, so nothing real is
//! touched whether the gate holds or not.

#![cfg(windows)]

use ghost_core::input::STOP_FLAG;
use ghost_session::{GhostError, GhostSession};
use std::sync::atomic::Ordering;

fn set_stop(on: bool) {
    STOP_FLAG.store(on, Ordering::Release);
}

fn refused<T: std::fmt::Debug>(what: &str, r: ghost_session::error::Result<T>) {
    assert!(matches!(r, Err(GhostError::Stopped)), "{what} ran while stopped: {r:?}");
}

fn not_refused<T: std::fmt::Debug>(what: &str, r: ghost_session::error::Result<T>) {
    assert!(!matches!(r, Err(GhostError::Stopped)), "{what} still refused after reset: {r:?}");
}

const NO_SUCH: &str = "ghost-emergency-stop-test: nothing has this name";

#[tokio::test]
async fn session_actions_refuse_while_stopped_and_run_after_reset() {
    let s = GhostSession::new().expect("session");
    set_stop(true);
    refused("launch", s.launch("ghost-emergency-stop-test-no-such-program.exe").await);
    refused("launch_hidden", s.launch_hidden("ghost-emergency-stop-test-no-such-program.exe").await);
    refused("focus_window", s.focus_window(NO_SUCH).await);
    refused("ensure_window_foreground", s.ensure_window_foreground(NO_SUCH).await);
    refused("window_state", s.window_state(NO_SUCH, "minimize").await);
    refused("set_clipboard", s.set_clipboard("must not land").await);
    refused("execute_intent", s.execute_intent("{}").await);
    refused("desktop_create", s.desktop_create("stoptest").await);
    refused("desktop_launch", s.desktop_launch("stoptest", "x.exe").await);
    refused("desktop_click", s.desktop_click("stoptest", 0, 1, 1, "left").await);
    refused("desktop_type", s.desktop_type("stoptest", 0, "a").await);
    refused("desktop_close", s.desktop_close("stoptest").await);
    refused("browser_launch", s.browser_launch_with("stoptest", "headless", None).await);
    refused("tab_open", s.tab_open("stoptest", "about:blank").await);
    refused("set_focus_policy foreground", s.set_focus_policy("foreground"));
    assert!(s.set_focus_policy("background").is_ok(), "dropping to background must stay allowed");
    assert!(ghost_session::error::is_stopped());
    set_stop(false);

    not_refused("launch", s.launch("ghost-emergency-stop-test-no-such-program.exe").await);
    not_refused("focus_window", s.focus_window(NO_SUCH).await);
    not_refused("window_state", s.window_state(NO_SUCH, "minimize").await);
    not_refused("desktop_launch", s.desktop_launch("stoptest", "x.exe").await);
    not_refused("tab_open", s.tab_open("stoptest", "about:blank").await);
}

#[test]
fn the_stop_error_says_who_lifts_it_without_coaching_a_reset() {
    let msg = GhostError::Stopped.to_string();
    assert!(msg.contains("stopped") && msg.contains("Ctrl+Alt+G"), "{msg}");
    // An agent reads this text: it must not be told to call ghost_reset.
    assert!(!msg.contains("ghost_reset"), "{msg}");
}
