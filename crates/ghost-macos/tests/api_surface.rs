//! Compile-time proof that ghost-macos's public surface matches every shape
//! `ghost-session` actually calls through `crate::engine::*`.
//!
//! `ghost-session` itself cannot be type-checked from Windows (it pulls in
//! `ring`/`blake3` via `reqwest`+`rustls`/`ghost-cache`, whose build scripts
//! need a real macOS-targeting `cc` -- see docs/macos-build.md). This file is
//! the proxy that closes that gap: it `use`s and calls every symbol
//! ghost-session references from `crate::engine::*`, with the identical
//! argument types, return types, `?`-propagation, and field access (notably
//! `vk.0` on `name_to_vk`'s result, exactly as `session.rs` does) that the
//! real call sites use. If this type-checks for `aarch64-apple-darwin`, every
//! one of those call sites in ghost-session would too -- the engine
//! indirection means ghost-session's logic never depends on anything beyond
//! these signatures. Gated to macOS only, mirroring `lib.rs`, so it compiles
//! to nothing (and therefore cannot break) on Windows/Linux CI.
#![cfg(target_os = "macos")]

use ghost_macos::capture::{self, CaptureFormat, Mark, Verification};
use ghost_macos::error::CoreError;
use ghost_macos::input::{self, hotkey, keyboard, mouse, BackgroundClicker, EditCommand};
use ghost_macos::ocr;
use ghost_macos::system;
use ghost_macos::uia::{self, patterns, tree, ElementDescriptor};

// ---------------------------------------------------------------------------
// error
// ---------------------------------------------------------------------------

#[test]
fn error_core_error_from_conversion_shape() {
    // Mirrors ghost-session/src/error.rs: `Core(#[from] crate::engine::error::CoreError)`.
    #[derive(Debug, thiserror::Error)]
    enum GhostErrorLike {
        #[error("Core error: {0}")]
        Core(#[from] CoreError),
    }
    let e: GhostErrorLike = CoreError::WindowGone.into();
    assert!(matches!(e, GhostErrorLike::Core(CoreError::WindowGone)));
}

// ---------------------------------------------------------------------------
// input::hotkey -- the surface that unblocks ghost_shell (shell.rs:515,542)
// ---------------------------------------------------------------------------

#[test]
fn hotkey_surface_matches_shell_rs_call_sites() {
    hotkey::reset_stop();
    assert!(!hotkey::is_stopped());
    hotkey::trigger_stop();
    assert!(hotkey::is_stopped());
    hotkey::reset_stop();

    let _: Result<(), CoreError> = hotkey::register_emergency_stop();
    hotkey::release_all_modifiers();
    let _: &std::sync::atomic::AtomicBool = &hotkey::STOP_FLAG;
}

// ---------------------------------------------------------------------------
// input::keyboard -- name_to_vk().0 field access, exactly as session.rs:1688
// ---------------------------------------------------------------------------

#[test]
fn keyboard_surface_matches_session_rs_call_sites() {
    let vk = keyboard::name_to_vk("enter").expect("enter must resolve to a VIRTUAL_KEY");
    let _: u16 = vk.0; // session.rs: `BackgroundClicker::send_key(target, vk.0)`

    let _: Result<(), CoreError> = keyboard::type_text("hello");
    let _: Result<(), CoreError> = input::keyboard::clear_focused_field();
    let _: Result<(), CoreError> = keyboard::press_key(vk);
    let _: Result<(), CoreError> = keyboard::key_down(vk);
    let _: Result<(), CoreError> = keyboard::key_up(vk);
}

// ---------------------------------------------------------------------------
// input::mouse
// ---------------------------------------------------------------------------

#[test]
fn mouse_surface_matches_session_rs_call_sites() {
    let _: Result<(), CoreError> = mouse::click(0, 0);
    let _: Result<(), CoreError> = mouse::hover(0, 0);
    let _: Result<(), CoreError> = mouse::right_click(0, 0);
    let _: Result<(), CoreError> = mouse::double_click(0, 0);
    let _: Result<(), CoreError> = mouse::drag(0, 0, 1, 1);
    let _: Result<(), CoreError> = mouse::scroll(0, 0, "up", 1);
}

// ---------------------------------------------------------------------------
// input::{BackgroundClicker, EditCommand}
// ---------------------------------------------------------------------------

#[test]
fn background_clicker_and_edit_command_match_session_rs_call_sites() {
    let _: Result<(), CoreError> = BackgroundClicker::click(0, (0, 0));
    let _: Result<(), CoreError> = BackgroundClicker::click_screen(0, 0, 0);
    let _: Result<(), CoreError> = BackgroundClicker::double_click_screen(0, 0, 0);
    let _: Result<(), CoreError> = BackgroundClicker::right_click_screen(0, 0, 0);
    let _: Result<(), CoreError> = BackgroundClicker::hover_screen(0, 0, 0);
    let _: Result<(), CoreError> = BackgroundClicker::button_click(0);
    let _: Result<(), CoreError> = BackgroundClicker::set_text(0, "x");
    let target: isize = BackgroundClicker::focused_control(0);
    let _: Result<(), CoreError> = BackgroundClicker::send_char(target, 'a');
    let vk = keyboard::name_to_vk("a").unwrap();
    let _: Result<(), CoreError> = BackgroundClicker::send_key(target, vk.0);
    let cmd = EditCommand::Copy;
    let _: Result<(), CoreError> = BackgroundClicker::edit_command(target, cmd);
}

// ---------------------------------------------------------------------------
// capture -- Mark/CaptureFormat construction exactly as session.rs builds them
// ---------------------------------------------------------------------------

#[test]
fn capture_surface_matches_session_rs_call_sites() {
    let _mark = Mark { label: 1u32, x: (5i32 - 1), y: (5i32 - 1) }; // session.rs:435
    let _: Result<(Vec<u8>, usize, usize), CoreError> = capture::capture_region_raw(Some((0, 0, 10, 10)));
    let marks: Vec<Mark> = vec![];
    let _: Result<Vec<u8>, CoreError> = capture::capture_region_marked_jpeg(Some((0, 0, 10, 10)), &marks, 1400, 82);
    let _: Result<Vec<u8>, CoreError> = capture::capture_screen_region(Some((0, 0, 10, 10)), Some(768), CaptureFormat::Jpeg(80));
    let _: Result<Vec<u8>, CoreError> = capture::capture_screen_region(None, None, CaptureFormat::Png);
    let _: Result<Vec<u8>, CoreError> = capture::capture_screen();
    let _: Result<(Vec<u8>, usize, usize), CoreError> = capture::capture_window_printwindow(0);
    let _: Vec<u8> = capture::screen::crop_rgba(&[0u8; 16], 2, 0, 0, 1, 1);

    let v: Verification = capture::compute_verification(&[0u8; 16], &[0u8; 16], 2, 2, true);
    let _serialized = serde_json::to_value(v).unwrap_or_default();

    let detector = capture::idle::IdleDetector::new().expect("IdleDetector::new must succeed");
    let _fut = detector.wait_stable(3, 100); // not awaited: type-check only, no tokio runtime needed here
}

// ---------------------------------------------------------------------------
// ocr
// ---------------------------------------------------------------------------

#[test]
fn ocr_surface_matches_session_rs_call_sites() {
    let _: Result<Option<(i32, i32)>, CoreError> = ocr::find_text_local("needle", Some((0, 0, 10, 10)));
}

// ---------------------------------------------------------------------------
// system
// ---------------------------------------------------------------------------

#[test]
fn system_surface_matches_session_rs_call_sites() {
    let _: isize = system::foreground_window();
    let _: Option<(i32, i32)> = system::cursor_pos();
    let _: Option<(i32, i32, i32, i32)> = system::foreground_window_rect();
    let _: Option<(i32, i32, i32, i32)> = system::window_rect(0);
    let _: Result<String, CoreError> = system::get_clipboard();
    let _: Result<(), CoreError> = system::set_clipboard("x");
}

// ---------------------------------------------------------------------------
// uia -- element/patterns/tree, plus the ElementDescriptor return shape used
// by session.rs's describe_screen/describe_screen_fast (`Vec<ElementDescriptor>`)
// ---------------------------------------------------------------------------

#[test]
fn uia_surface_matches_session_rs_call_sites() {
    let _guard = uia::init_com().expect("init_com must succeed so GhostSession can construct");
    let _bus = uia::EventBus::global();
    let _seq: u64 = _bus.seq();

    let tree = uia::UiaTree::new().expect("UiaTree::new must succeed so GhostSession can construct");
    let _: Result<Option<uia::UiaElement>, CoreError> = tree.find_by_name_fast("x");
    let _: Result<Option<uia::UiaElement>, CoreError> = tree.find_by_role_fast("button");
    let _: Result<Option<uia::UiaElement>, CoreError> = tree.find_by_name_in_hwnd(0, "x");
    let _: Result<Option<uia::UiaElement>, CoreError> = tree.find_by_role_in_hwnd(0, "button");
    let _: Result<Vec<uia::UiaElement>, CoreError> = tree.find_all_in_hwnd(0, Some("x"), None, 10);
    let _: Result<Vec<ElementDescriptor>, CoreError> = tree.describe_screen(Some("Notepad"));
    let _: Result<Vec<ElementDescriptor>, CoreError> = tree.describe_screen_fast();
    let _: Result<(String, bool), CoreError> = tree.collect_text(None, 1000);
    let _: Result<Option<uia::UiaElement>, CoreError> = tree.element_from_point(0, 0);

    let _: Result<Vec<uia::WindowInfo>, CoreError> = uia::list_windows();
    let _: Result<(), CoreError> = uia::focus_window("Notepad");
    let ws = uia::WindowState::from_str("maximize").unwrap();
    let _: Result<(), CoreError> = uia::set_window_state("Notepad", ws);
    let _: Result<bool, CoreError> = tree::focus_window_under_point(0, 0);
    let _: bool = tree::role_alias_matches("edit", "document"); // tiers.rs:80, session.rs:294

    let role = uia::element::role_id_to_name(50000); // tiers.rs:75, session.rs:287
    assert_eq!(role, "button");
    let _: bool = patterns::is_editable_role(50004); // session.rs:1780

    let _desc = ElementDescriptor {
        name: "OK".into(),
        role: "button".into(),
        left: 0,
        top: 0,
        right: 10,
        bottom: 10,
        enabled: true,
    };

    // element.rs (ghost-session) calls these directly on `UiaElement`:
    // patterns::invoke(&self.inner), patterns::set_value(&self.inner, value),
    // patterns::get_selection(&self.inner), plus the _ex variants.
    let el = uia::UiaElement;
    let _: Result<(), CoreError> = patterns::invoke(&el);
    let _: Result<(), CoreError> = patterns::invoke_ex(&el, false);
    let _: Result<String, CoreError> = patterns::get_selection(&el);
    let _: Result<(), CoreError> = patterns::set_value(&el, "text");
    let _: Result<(), CoreError> = patterns::set_value_ex(&el, "text", false);
}
