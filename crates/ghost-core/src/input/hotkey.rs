//! Emergency stop: Ctrl+Alt+G halts every ghost process on the machine.
//!
//! Two subtleties make this harder than "register a hotkey":
//!
//! - `RegisterHotKey` binds the hotkey to the thread that calls it, and `WM_HOTKEY`
//!   is posted to *that thread's* message queue. Registering on one thread and
//!   pumping messages on another (the original design) means the message arrives at
//!   a queue nobody reads, and the hotkey silently never fires. Registration and the
//!   message pump must live on the same thread.
//! - Only one process can own a hotkey. With several ghost processes running at once
//!   (multiple Claude sessions, each with its own MCP server), the others would have
//!   no working stop. So the stop signal itself is a *named event* shared across the
//!   session: whichever process owns the hotkey broadcasts by setting the event, and
//!   every ghost process runs a watcher that trips its local stop flag when the
//!   event goes up. One keypress stops everything.
//!
//! If the hotkey owner exits, another process's hotkey thread acquires the
//! registration on its next retry, so ownership migrates instead of dying.

use crate::error::CoreError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

/// Session-scoped, like the input desktop it protects. Manual-reset so every
/// waiting process wakes, not just one.
const STOP_EVENT_NAME: &str = r"Local\ghost-emergency-stop-event";

static REGISTERED: AtomicBool = AtomicBool::new(false);

pub fn is_stopped() -> bool {
    STOP_FLAG.load(Ordering::Acquire)
}

/// Trip the stop locally and broadcast it to every other ghost process.
pub fn trigger_stop() {
    STOP_FLAG.store(true, Ordering::Release);
    if let Some(ev) = open_stop_event() {
        unsafe {
            let _ = SetEvent(ev.0);
        }
    }
}

/// Clear the stop so automation can resume. Also lowers the shared event -
/// otherwise every new ghost process would arrive pre-stopped, and the watchers
/// would re-trip immediately.
pub fn reset_stop() {
    STOP_FLAG.store(false, Ordering::Release);
    if let Some(ev) = open_stop_event() {
        unsafe {
            let _ = ResetEvent(ev.0);
        }
    }
}

/// Owned event handle. `CreateEventW` opens the existing event when one exists, so
/// every process converges on the same kernel object.
struct EventHandle(HANDLE);
impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
// Safety: kernel event handles are process-global and thread-agnostic.
unsafe impl Send for EventHandle {}
unsafe impl Sync for EventHandle {}

/// Process-lifetime handle to the shared stop event.
///
/// Held open for the life of the process, not per call: a named kernel object is
/// destroyed when its last handle closes, so an open-set-close sequence would raise
/// the event and then immediately destroy it, and no other process would ever see
/// the signal.
static STOP_EVENT: std::sync::OnceLock<Option<EventHandle>> = std::sync::OnceLock::new();

fn open_stop_event() -> Option<&'static EventHandle> {
    STOP_EVENT
        .get_or_init(|| unsafe {
            CreateEventW(None, true, false, &HSTRING::from(STOP_EVENT_NAME))
                .ok()
                .map(EventHandle)
        })
        .as_ref()
}

/// Install the emergency stop for this process. Idempotent.
///
/// Spawns two threads: a hotkey thread that owns (or waits to own) the Ctrl+Alt+G
/// registration and broadcasts on the shared event, and a watcher thread that trips
/// the local stop flag whenever any process broadcasts.
pub fn register_emergency_stop() -> Result<(), CoreError> {
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    // Watcher: reacts to a stop raised by ANY ghost process.
    std::thread::Builder::new()
        .name("ghost-stop-watcher".into())
        .spawn(|| {
            let Some(ev) = open_stop_event() else { return };
            loop {
                unsafe {
                    if WaitForSingleObject(ev.0, 1_000) == WAIT_OBJECT_0 {
                        if !is_stopped() {
                            tracing::warn!("emergency stop received from another ghost process");
                            STOP_FLAG.store(true, Ordering::Release);
                            release_all_modifiers();
                        }
                        // Manual-reset event: wait for reset_stop to lower it before
                        // listening again, or this loop would spin hot.
                        while WaitForSingleObject(ev.0, 0) == WAIT_OBJECT_0 {
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        STOP_FLAG.store(false, Ordering::Release);
                    }
                }
            }
        })
        .map_err(|e| CoreError::ComInit(format!("spawn stop watcher: {e}")))?;

    // Hotkey owner: registration and message pump on the SAME thread, or WM_HOTKEY
    // lands in a queue nobody reads.
    std::thread::Builder::new()
        .name("ghost-stop-hotkey".into())
        .spawn(|| unsafe {
            loop {
                if RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT, b'G' as u32).is_ok() {
                    break;
                }
                // Another ghost process owns it and will broadcast for us. Keep
                // retrying so ownership migrates here if that process exits.
                std::thread::sleep(Duration::from_secs(3));
            }
            let mut msg = MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 || ret.0 == -1 {
                    break;
                }
                if msg.message == WM_HOTKEY && msg.wParam.0 == 1 {
                    tracing::warn!("emergency stop triggered (Ctrl+Alt+G)");
                    trigger_stop();
                    release_all_modifiers();
                }
            }
        })
        .map_err(|e| CoreError::ComInit(format!("spawn hotkey thread: {e}")))?;

    Ok(())
}

/// Send key-up events for all modifier keys so no key stays stuck.
pub fn release_all_modifiers() {
    let modifiers = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN];
    let inputs: Vec<INPUT> = modifiers
        .iter()
        .map(|&vk| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        })
        .collect();

    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            tracing::warn!("release_all_modifiers: sent {}/{} inputs", sent, inputs.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These mutate process-global and session-global state (the named event), so
    // they run as one test to avoid interference from parallel test threads.
    #[test]
    fn stop_flag_round_trips_and_broadcasts() {
        reset_stop();
        assert!(!is_stopped());

        trigger_stop();
        assert!(is_stopped());
        // The broadcast side: the named event must now be signaled, because that is
        // what other ghost processes actually observe.
        let ev = open_stop_event().expect("event");
        unsafe {
            assert_eq!(
                WaitForSingleObject(ev.0, 0),
                WAIT_OBJECT_0,
                "trigger_stop must raise the shared event"
            );
        }

        reset_stop();
        assert!(!is_stopped());
        unsafe {
            assert_ne!(
                WaitForSingleObject(ev.0, 0),
                WAIT_OBJECT_0,
                "reset_stop must lower the shared event"
            );
        }
    }

    #[test]
    fn registration_is_idempotent() {
        register_emergency_stop().expect("first");
        register_emergency_stop().expect("second call must be a no-op, not an error");
    }
}
