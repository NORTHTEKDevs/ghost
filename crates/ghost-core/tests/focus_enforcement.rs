//! The guarantee test: under the default policy, no primitive in ghost-core can
//! touch the user's cursor, keyboard, or foreground window.
//!
//! This is the test that keeps the product claim honest. If someone adds a new
//! `SendInput` or `SetForegroundWindow` call path and forgets the policy gate, the
//! matching assertion here fails.
//!
//! All assertions live in one test function on purpose: the focus policy is
//! process-global state, and cargo runs test functions on parallel threads.

use ghost_core::error::CoreError;
use ghost_core::focus::{self, FocusPolicy};
use ghost_core::input::{keyboard, mouse};
use ghost_core::uia::tree;

fn is_blocked<T>(r: Result<T, CoreError>, what: &str) {
    match r {
        Err(CoreError::NoBackgroundPath { .. }) => {}
        Err(other) => panic!("{what} failed for the wrong reason: {other}"),
        Ok(_) => panic!("{what} was allowed to run under the Background policy"),
    }
}

#[test]
fn background_policy_blocks_every_screen_stealing_primitive() {
    focus::set_policy(FocusPolicy::Background);
    assert!(focus::is_background_only());

    // Mouse: every one of these moves the user's real cursor.
    is_blocked(mouse::click(10, 10), "mouse::click");
    is_blocked(mouse::move_to(10, 10), "mouse::move_to");
    is_blocked(mouse::hover(10, 10), "mouse::hover");
    is_blocked(mouse::right_click(10, 10), "mouse::right_click");
    is_blocked(mouse::double_click(10, 10), "mouse::double_click");
    is_blocked(mouse::drag(10, 10, 20, 20), "mouse::drag");
    is_blocked(mouse::scroll(10, 10, "down", 1), "mouse::scroll");

    // Keyboard: these land in whatever window the user is currently typing into.
    let vk = keyboard::name_to_vk("Enter").expect("Enter is a known key");
    is_blocked(keyboard::type_text("hello"), "keyboard::type_text");
    is_blocked(keyboard::press_key(vk), "keyboard::press_key");
    is_blocked(keyboard::key_down(vk), "keyboard::key_down");
    is_blocked(keyboard::key_up(vk), "keyboard::key_up");

    // Window activation: raises a window over the user's work.
    is_blocked(tree::focus_window("Notepad"), "tree::focus_window");

    // Restore the default for anything else in this binary.
    focus::set_policy(FocusPolicy::Background);
}

#[test]
fn error_message_tells_the_caller_how_to_opt_in() {
    // An agent hitting this wall needs to learn the escape hatch from the message
    // alone, without reading the source.
    let err = focus::require_foreground_allowed("click").unwrap_err().to_string();
    assert!(err.contains("click"), "{err}");
    assert!(err.contains("background"), "{err}");
    assert!(
        err.contains("prefer_background") || err.contains("foreground"),
        "message must name the policy that unblocks it: {err}"
    );
}
