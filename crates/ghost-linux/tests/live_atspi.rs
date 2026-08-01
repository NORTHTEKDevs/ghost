//! Live AT-SPI tests against a real GTK application.
//!
//! These are `#[ignore]`d so a normal `cargo test` stays fast and display-free.
//! CI runs them with `-- --ignored` inside a synthesised desktop:
//! `xvfb-run` for X11, `dbus-run-session` for a session bus, and
//! `at-spi-bus-launcher` for the accessibility bus. See
//! `.github/workflows/linux.yml`.
//!
//! They exist because a Linux automation engine cannot be trusted on unit tests
//! alone: everything interesting happens across a D-Bus boundary to another
//! process's toolkit. The one that matters most is
//! [`sets_and_reads_back_entry_text`] -- it proves Ghost's wedge on Linux, i.e.
//! that an application can be driven through AT-SPI with no synthetic input at
//! all.

#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ghost_linux::a11y::tree::{list_windows, A11yTree, WindowInfo};

/// A GTK application launched for the duration of one test, killed on drop even
/// if the test panics. Leaked GUI processes are a documented way to make a
/// later automation test pass for the wrong reason.
struct TestApp(Child);

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `zenity --forms` gives a dialog containing a labelled text entry plus OK and
/// Cancel buttons -- an entry, a button and a window in one process, with no
/// test fixture to build.
fn launch_gtk_app(title: &str) -> TestApp {
    let child = Command::new("zenity")
        .args([
            "--forms",
            &format!("--title={title}"),
            "--text=ghost live test",
            "--add-entry=Name",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("zenity must be installed for the live suite (apt-get install zenity)");
    TestApp(child)
}

/// Poll until a window whose name contains `needle` appears.
///
/// Polling rather than sleeping a fixed time: GTK startup under Xvfb varies by
/// an order of magnitude between a warm and a cold runner.
fn wait_for_window(needle: &str, timeout: Duration) -> Option<WindowInfo> {
    let deadline = Instant::now() + timeout;
    let needle = needle.to_ascii_lowercase();
    loop {
        if let Ok(windows) = list_windows() {
            if let Some(w) =
                windows.into_iter().find(|w| w.name.to_ascii_lowercase().contains(&needle))
            {
                return Some(w);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

const APP_TIMEOUT: Duration = Duration::from_secs(20);

/// Roles currently on screen, for failure messages. A test that says "no
/// role=edit found" is far less useful than one that says what was there
/// instead -- toolkits disagree about widget roles and the answer varies by
/// GTK/Qt version.
fn roles_in(tree: &A11yTree) -> String {
    match tree.describe_screen_fast() {
        Ok(els) => {
            let mut v: Vec<String> =
                els.iter().map(|e| format!("{}:{:?}", e.role, e.name)).collect();
            v.sort();
            v.dedup();
            v.join(", ")
        }
        Err(e) => format!("<describe failed: {e}>"),
    }
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn a11y_bus_is_reachable() {
    // The single most common Linux setup failure. If this fails, nothing else
    // in the engine can work and every other failure is downstream noise.
    let tree = A11yTree::new().expect(
        "cannot reach the AT-SPI bus. Is at-spi2-core installed and \
         toolkit-accessibility enabled?",
    );
    tree.list_windows().expect("listing windows must not error once the bus is up");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn lists_windows_of_a_running_gtk_app() {
    let _app = launch_gtk_app("GhostWindowProbe");
    let win = wait_for_window("GhostWindowProbe", APP_TIMEOUT)
        .expect("the launched GTK window must appear in list_windows");

    assert_ne!(win.hwnd, 0, "a real window must have a non-zero handle");
    assert!(win.pid > 0, "window must report the owning pid");
    assert_eq!(win.state, "normal");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn finds_an_entry_by_role() {
    let _app = launch_gtk_app("GhostEntryProbe");
    let win = wait_for_window("GhostEntryProbe", APP_TIMEOUT).expect("window must appear");

    let tree = A11yTree::new().unwrap();
    let el = tree
        .find_by_role_in_hwnd(win.hwnd, "edit")
        .expect("role search must not error")
        .unwrap_or_else(|| panic!("no role=edit found. Roles present: {}", roles_in(&tree)));

    assert!(el.is_enabled(), "a fresh entry is enabled");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn sets_and_reads_back_entry_text() {
    // THE wedge test. Text is set through AT-SPI EditableText -- no keystrokes,
    // no pointer, no focus change -- and read back from the live tree. If this
    // passes, Ghost drives Linux applications the same way it drives Windows
    // ones in background mode.
    let _app = launch_gtk_app("GhostTypeProbe");
    let win = wait_for_window("GhostTypeProbe", APP_TIMEOUT).expect("window must appear");

    let tree = A11yTree::new().unwrap();
    let el = tree
        .find_by_role_in_hwnd(win.hwnd, "edit")
        .unwrap()
        .unwrap_or_else(|| panic!("no role=edit found. Roles present: {}", roles_in(&tree)));

    el.set_value("ghost-was-here").expect("EditableText.SetTextContents must succeed");

    // Re-resolve rather than trusting the cached snapshot: the whole point of
    // act-then-verify is reading the application's real state back.
    let after = tree
        .find_by_role_in_hwnd(win.hwnd, "edit")
        .unwrap()
        .expect("entry must still be findable after the write");

    assert_eq!(
        after.get_text().trim(),
        "ghost-was-here",
        "text set through AT-SPI must be readable back from the application"
    );
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn finds_a_button_by_name() {
    let _app = launch_gtk_app("GhostButtonProbe");
    let win = wait_for_window("GhostButtonProbe", APP_TIMEOUT).expect("window must appear");

    let tree = A11yTree::new().unwrap();
    let el = tree
        .find_by_name_in_hwnd(win.hwnd, "Cancel")
        .expect("name search must not error")
        .expect("the dialog's Cancel button must be findable by name");

    assert_eq!(el.role_name(), "button", "a GTK Button must map to Ghost's `button` role");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn invoking_a_button_through_atspi_has_a_real_effect() {
    // The other half of the wedge. `sets_and_reads_back_entry_text` proves the
    // write path; this proves the *action* path -- Action.DoAction, with no
    // synthetic click and no pointer movement.
    //
    // The effect is observable rather than assumed: dismissing the dialog ends
    // the process, so the window must disappear from the accessibility tree. A
    // test that only asserted "do_action returned Ok" would pass even if the
    // application ignored it entirely.
    let _app = launch_gtk_app("GhostInvokeProbe");
    let win = wait_for_window("GhostInvokeProbe", APP_TIMEOUT).expect("window must appear");

    let tree = A11yTree::new().unwrap();
    let cancel = tree
        .find_by_name_in_hwnd(win.hwnd, "Cancel")
        .unwrap()
        .unwrap_or_else(|| panic!("no Cancel button. Roles present: {}", roles_in(&tree)));

    cancel.invoke().expect("Action.DoAction must succeed on a GTK button");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut still_there = true;
    while Instant::now() < deadline {
        let present = list_windows()
            .map(|ws| ws.iter().any(|w| w.name.contains("GhostInvokeProbe")))
            .unwrap_or(false);
        if !present {
            still_there = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !still_there,
        "invoking Cancel must actually dismiss the dialog, not just return Ok"
    );
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn describe_screen_returns_actionable_elements() {
    let _app = launch_gtk_app("GhostDescribeProbe");
    let win = wait_for_window("GhostDescribeProbe", APP_TIMEOUT).expect("window must appear");

    let tree = A11yTree::new().unwrap();
    let els = tree.describe_screen(Some("GhostDescribeProbe")).expect("describe must not error");

    assert!(!els.is_empty(), "a dialog with an entry and buttons must describe as non-empty");
    assert!(
        els.iter().any(|e| e.role == "button"),
        "describe_screen must surface the dialog buttons; got roles: {:?}",
        els.iter().map(|e| e.role.as_str()).collect::<Vec<_>>()
    );
    assert!(
        els.iter().all(|e| e.right > e.left && e.bottom > e.top),
        "every described element must carry a real on-screen rectangle"
    );
    let _ = win;
}

#[test]
#[ignore = "live: needs a display"]
fn captures_the_screen() {
    let (rgba, w, h) =
        ghost_linux::capture::capture_screen_full_rgba().expect("screen capture must succeed");
    assert!(w > 0 && h > 0, "capture must report real dimensions");
    assert_eq!(rgba.len(), w * h * 4, "buffer must match its dimensions");
}

#[test]
#[ignore = "live: needs a display"]
fn encodes_a_png_screenshot() {
    let png = ghost_linux::capture::capture_screen().expect("PNG capture must succeed");
    assert!(png.len() > 8, "PNG must not be empty");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "output must be a real PNG");
}

#[test]
#[ignore = "live: needs an X11 display"]
fn x11_pointer_can_be_moved_and_read_back() {
    // Only meaningful on X11: under Wayland the portal will not report pointer
    // position, which the engine reports honestly as `None`.
    if ghost_linux::session_kind() != ghost_linux::SessionKind::X11 {
        eprintln!("skipping: not an X11 session");
        return;
    }
    let backend = ghost_linux::input::backend().expect("X11 input backend must initialise");
    backend.move_to(120, 90).expect("XTEST pointer move must succeed");
    std::thread::sleep(Duration::from_millis(120));
    let pos = backend.cursor_pos().expect("X11 must report pointer position");
    assert_eq!(pos, (120, 90), "pointer must land where it was sent");
}
