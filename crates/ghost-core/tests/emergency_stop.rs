//! The emergency stop at the lowest layer: each state-changing primitive
//! refuses with `CoreError::Stopped` before it dispatches anything, and the
//! hidden-desktop worker holds back a job that was already queued when the
//! stop arrived.
//!
//! The stop is raised with the process-local `STOP_FLAG` only, never
//! `trigger_stop`: the shared event would stop every Ghost process on this
//! machine. Tests share that flag, so they take a lock.
//!
//! What is called after a reset targets nothing real: hwnd 0, a window name and
//! a program that do not exist. Primitives whose un-gated call would touch
//! the real desktop (SendInput, the clipboard, raising the window under a
//! point) are only called while stopped.

#![cfg(windows)]

use ghost_core::desktop::DesktopSession;
use ghost_core::error::CoreError;
use ghost_core::input::{BackgroundClicker, EditCommand, STOP_FLAG};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

static LOCK: Mutex<()> = Mutex::new(());

/// Raises the local stop for the life of the guard.
struct Stopped<'a>(#[allow(dead_code)] MutexGuard<'a, ()>);

impl Drop for Stopped<'_> {
    fn drop(&mut self) {
        STOP_FLAG.store(false, Ordering::Release);
    }
}

fn stopped() -> Stopped<'static> {
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    STOP_FLAG.store(true, Ordering::Release);
    Stopped(g)
}

fn is_stopped_err<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    matches!(r, Err(CoreError::Stopped))
}

/// A named call to one primitive.
type Call = (&'static str, fn() -> Result<(), CoreError>);

const NO_SUCH_WINDOW: &str = "ghost-emergency-stop-test: no window has this title";

#[test]
fn postmessage_primitives_refuse_while_stopped_and_run_after_reset() {
    let calls: [Call; 10] = [
        ("click", || BackgroundClicker::click(0, (1, 1))),
        ("click_screen", || BackgroundClicker::click_screen(0, 1, 1)),
        ("double_click_screen", || BackgroundClicker::double_click_screen(0, 1, 1)),
        ("right_click_screen", || BackgroundClicker::right_click_screen(0, 1, 1)),
        ("hover_screen", || BackgroundClicker::hover_screen(0, 1, 1)),
        ("button_click", || BackgroundClicker::button_click(0)),
        ("edit_command", || BackgroundClicker::edit_command(0, EditCommand::SelectAll)),
        ("send_key", || BackgroundClicker::send_key(0, 0x41)),
        ("send_char", || BackgroundClicker::send_char(0, 'a')),
        ("set_text", || BackgroundClicker::set_text(0, "a")),
    ];
    {
        let _s = stopped();
        for (name, f) in calls {
            assert!(is_stopped_err(f()), "{name} dispatched while stopped");
        }
    }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (name, f) in calls {
        assert!(!is_stopped_err(f()), "{name} still refused after reset");
    }
}

#[test]
fn window_process_and_clipboard_primitives_refuse_while_stopped() {
    {
        let _s = stopped();
        assert!(is_stopped_err(ghost_core::uia::tree::focus_window(NO_SUCH_WINDOW)));
        assert!(is_stopped_err(ghost_core::uia::tree::set_window_state(
            NO_SUCH_WINDOW,
            ghost_core::uia::tree::WindowState::Minimize
        )));
        assert!(is_stopped_err(ghost_core::uia::tree::focus_window_under_point(1, 1)));
        assert!(is_stopped_err(ghost_core::process::launch("ghost-emergency-stop-test-no-such-program.exe")));
        assert!(is_stopped_err(ghost_core::system::clipboard::set_clipboard("must not land")));
        assert!(is_stopped_err(ghost_core::desktop::scroll_window(0, 1, false)));
        assert!(is_stopped_err(ghost_core::input::mouse::click(1, 1)));
        assert!(is_stopped_err(ghost_core::input::keyboard::press_key(
            windows::Win32::UI::Input::KeyboardAndMouse::VK_F24
        )));
        assert!(ghost_core::stop::is_active());
        assert!(is_stopped_err(ghost_core::stop::ensure_not_stopped()));
    }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    assert!(!is_stopped_err(ghost_core::uia::tree::focus_window(NO_SUCH_WINDOW)));
    assert!(!is_stopped_err(ghost_core::uia::tree::set_window_state(
        NO_SUCH_WINDOW,
        ghost_core::uia::tree::WindowState::Minimize
    )));
    assert!(!is_stopped_err(ghost_core::process::launch("ghost-emergency-stop-test-no-such-program.exe")));
}

#[test]
fn the_hidden_desktop_worker_refuses_actions_but_still_reads() {
    let d = DesktopSession::create("ghost-stop-test-worker").expect("hidden desktop");
    let ran = Arc::new(AtomicBool::new(false));
    {
        let _s = stopped();
        let r = ran.clone();
        assert!(is_stopped_err(d.exec_action(move || r.store(true, Ordering::SeqCst))));
        let r = ran.clone();
        assert!(is_stopped_err(d.with_uia_action(move |_| r.store(true, Ordering::SeqCst))));
        assert!(is_stopped_err(d.launch("ghost-emergency-stop-test-no-such-program.exe")));
        assert!(is_stopped_err(d.click(0, 1, 1)));
        assert!(is_stopped_err(d.type_text(0, "a")));
        assert!(is_stopped_err(d.press(0, "Enter")));
        assert!(is_stopped_err(d.scroll(0, 0, 0, 1, false)));
        assert!(!ran.load(Ordering::SeqCst), "an action job ran while stopped");
        assert!(d.windows().is_ok(), "reads must keep working while stopped");
    }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let r = ran.clone();
    d.exec_action(move || r.store(true, Ordering::SeqCst)).expect("runs after reset");
    assert!(ran.load(Ordering::SeqCst));
}

/// A job queued before the stop, still waiting behind another job when the
/// stop arrives, must not run: the worker checks again right before it runs.
#[test]
fn a_job_queued_before_the_stop_does_not_run_after_it() {
    let d = Arc::new(DesktopSession::create("ghost-stop-test-queue").expect("hidden desktop"));
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let busy = {
        let d = d.clone();
        std::thread::spawn(move || d.exec(|| std::thread::sleep(Duration::from_millis(400))))
    };
    std::thread::sleep(Duration::from_millis(100));
    let ran = Arc::new(AtomicBool::new(false));
    let queued = {
        let (d, r) = (d.clone(), ran.clone());
        std::thread::spawn(move || d.exec_action(move || r.store(true, Ordering::SeqCst)))
    };
    std::thread::sleep(Duration::from_millis(100));
    STOP_FLAG.store(true, Ordering::Release);
    let result = queued.join().unwrap();
    STOP_FLAG.store(false, Ordering::Release);
    drop(g);
    busy.join().unwrap().unwrap();
    assert!(is_stopped_err(result), "the queued job was not refused");
    assert!(!ran.load(Ordering::SeqCst), "the queued job ran after the stop");
}
