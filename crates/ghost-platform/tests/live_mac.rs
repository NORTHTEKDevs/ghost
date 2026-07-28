//! Live macOS tests. Require a logged-in desktop and both TCC grants.
//!
//! These are *not* the verification protocol — `ghost doctor --mac` is, because it
//! produces a report a maintainer can read without access to the machine. These
//! exist for the narrower case of a developer working on the backend on their own
//! Mac who wants a fast `cargo test` loop over one function.
//!
//! Opt in with:
//!
//! ```text
//! GHOST_LIVE_MAC=1 cargo test -p ghost-platform --test live_mac -- --ignored
//! ```
//!
//! Both gates are deliberate. `#[ignore]` keeps them out of a plain `cargo test`,
//! including CI's. The `GHOST_LIVE_MAC` check keeps them out of a `--ignored` run
//! that someone starts without realising these move the mouse and read the screen.
//! (`env!` is not used for the second gate: it is evaluated at compile time, so it
//! would make the file fail to *build* on a Mac that has not set the variable,
//! rather than skip.)

#![cfg(target_os = "macos")]

use ghost_platform::macos::{capture, perms, window};

/// Returns true when the caller opted in and the OS has granted what is needed.
fn live(needs_capture: bool) -> bool {
    if std::env::var_os("GHOST_LIVE_MAC").is_none() {
        eprintln!("skipped: set GHOST_LIVE_MAC=1 to run live macOS tests");
        return false;
    }
    if !perms::accessibility_granted() {
        eprintln!("skipped: Accessibility is not granted for this binary");
        return false;
    }
    if needs_capture && !perms::screen_recording_granted() {
        eprintln!("skipped: Screen Recording is not granted for this binary");
        return false;
    }
    true
}

#[test]
#[ignore = "needs a desktop and TCC grants; see the module docs"]
fn the_main_display_reports_a_plausible_scale() {
    if !live(true) {
        return;
    }
    let scale = capture::main_display_scale();
    // 1.0 on a non-Retina display, 2.0 on Retina, 3.0 on nothing Apple ships as a
    // desktop display today. A value outside this range means the backing scale
    // factor was misread, which silently offsets every coordinate Ghost computes.
    assert!(
        (1.0..=3.0).contains(&scale),
        "implausible display scale {scale}"
    );
}

#[test]
#[ignore = "needs a desktop and TCC grants; see the module docs"]
fn a_full_screen_capture_is_neither_empty_nor_blank() {
    if !live(true) {
        return;
    }
    let shot = capture::capture_screen().expect("capture the screen");
    assert!(!shot.png.is_empty(), "captured zero bytes");
    // A logged-in desktop always has something on it. An all-one-colour image is
    // what CoreGraphics returns when Screen Recording is missing, and the
    // preflight above said it is not.
    assert!(!shot.blank, "capture was blank despite the grant being present");
    assert_eq!(shot.pixel_width as f64 / shot.scale(), shot.region.width() as f64);
}

#[test]
#[ignore = "needs a desktop and TCC grants; see the module docs"]
fn the_frontmost_app_appears_in_the_window_list() {
    if !live(true) {
        return;
    }
    let pid = window::frontmost_pid().expect("some app is frontmost");
    let windows = window::list_windows().expect("enumerate windows");
    assert!(
        windows.iter().any(|w| w.pid == pid),
        "the frontmost app (pid {pid}) owns none of the {} listed windows",
        windows.len()
    );
}

#[test]
#[ignore = "needs a desktop and TCC grants; see the module docs"]
fn every_listed_window_reports_a_focus_state_consistent_with_the_frontmost_app() {
    if !live(true) {
        return;
    }
    let frontmost = window::frontmost_pid();
    for w in window::list_windows().expect("enumerate windows") {
        assert_eq!(
            w.focused,
            Some(w.pid) == frontmost,
            "{w:?} disagrees with frontmost pid {frontmost:?}"
        );
    }
}

#[test]
#[ignore = "needs a desktop and TCC grants; see the module docs"]
fn the_clipboard_round_trips() {
    if !live(false) {
        return;
    }
    use ghost_platform::macos::clipboard;
    let restore = clipboard::get_text().expect("read the clipboard");

    clipboard::set_text("ghost-live-test").expect("write the clipboard");
    assert_eq!(
        clipboard::get_text().expect("read back"),
        Some("ghost-live-test".to_string())
    );

    // Put the user's clipboard back. A test that silently eats what someone had
    // copied is a test they stop running.
    if let Some(previous) = restore {
        let _ = clipboard::set_text(&previous);
    }
}
