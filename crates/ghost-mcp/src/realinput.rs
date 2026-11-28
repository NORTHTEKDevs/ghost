//! Did a HUMAN do that, or did a program?
//!
//! `GetLastInputInfo` cannot tell: it counts `SendInput` exactly like a real
//! key. That is fatal for the foreground sentinel, which has to answer "did the
//! human choose this window?" while automation - Ghost's own, or any other
//! program's - is injecting input. Measured 2026-09-06: with a program typing,
//! every foreground change looked like the human's, so a browser that activated
//! itself was recorded as a window the human had chosen and kept the screen.
//!
//! The low-level hooks do tell: `WH_KEYBOARD_LL` and `WH_MOUSE_LL` receive a
//! flag (`LLKHF_INJECTED` / `LLMHF_INJECTED`) on anything a program synthesized.
//! This module keeps three timestamps from real events only:
//!
//! - **key** - any real keystroke. "The human is at the machine."
//! - **click** - a real mouse button going down. Together with alt, this is how
//!   a person actually chooses a window; typing into one window is not consent
//!   to another window taking the screen.
//! - **alt** - a real Alt going down, which is the start of Alt+Tab.
//!
//! The callbacks do nothing but store a millisecond and hand the event on, so
//! they cannot add input latency. Windows only; elsewhere every reader reports
//! "no real input seen", which leaves the sentinel disabled rather than wrong.

// Off Windows there are no low-level hooks and the sentinel that reads these is
// compiled out, so the whole module is inert rather than absent: the shared code
// keeps one shape on both platforms.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};

static LAST_KEY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_CLICK_MS: AtomicU64 = AtomicU64::new(0);
static LAST_ALT_MS: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ago(stamp: &AtomicU64) -> Option<u64> {
    match stamp.load(Ordering::Relaxed) {
        0 => None,
        t => Some(now_ms().saturating_sub(t)),
    }
}

/// Milliseconds since the human last pressed a real key or mouse button.
/// `None` when neither has been seen (or the hooks are not installed).
pub fn since_real_input() -> Option<u64> {
    match (ago(&LAST_KEY_MS), ago(&LAST_CLICK_MS)) {
        (Some(k), Some(c)) => Some(k.min(c)),
        (k, c) => k.or(c),
    }
}

/// Milliseconds since a real mouse button went down.
pub fn since_real_click() -> Option<u64> {
    ago(&LAST_CLICK_MS)
}

/// Milliseconds since a real Alt key went down (the start of Alt+Tab).
pub fn since_real_alt() -> Option<u64> {
    ago(&LAST_ALT_MS)
}

/// Whether the hooks are live. When they are not, callers must not treat
/// "no real input" as fact.
pub fn watching() -> bool {
    INSTALLED.load(Ordering::Relaxed) == 1
}

/// The rule the sentinel uses: did the human CHOOSE this window?
///
/// A person selects a window by clicking it or by alt-tabbing to it. Typing
/// does not select anything - a human typing into their editor has not agreed
/// to a browser taking the screen, which is the whole complaint this exists to
/// answer. Both arguments are "milliseconds ago", `None` meaning never.
pub fn human_chose_a_window(since_click_ms: Option<u64>, since_alt_ms: Option<u64>) -> bool {
    const CLICK_WINDOW_MS: u64 = 900;
    const ALT_WINDOW_MS: u64 = 2_000;
    since_click_ms.map(|ms| ms <= CLICK_WINDOW_MS).unwrap_or(false)
        || since_alt_ms.map(|ms| ms <= ALT_WINDOW_MS).unwrap_or(false)
}

/// Install the hooks on a dedicated thread with its own message loop.
/// Idempotent; a no-op off Windows.
pub fn start() {
    #[cfg(windows)]
    {
        if INSTALLED.swap(2, Ordering::SeqCst) != 0 {
            return; // already starting or started
        }
        let _ = std::thread::Builder::new()
            .name("ghost-real-input".into())
            .spawn(pump);
    }
}

#[cfg(windows)]
unsafe extern "system" fn keyboard(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_SYSKEYDOWN};
    if code >= 0 {
        let msg = w.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
            if k.flags & LLKHF_INJECTED == windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT_FLAGS(0) {
                let t = now_ms();
                LAST_KEY_MS.store(t, Ordering::Relaxed);
                // 0x12 = VK_MENU, either Alt key: the start of Alt+Tab.
                if k.vkCode == 0x12 || k.vkCode == 0xA4 || k.vkCode == 0xA5 {
                    LAST_ALT_MS.store(t, Ordering::Relaxed);
                }
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

#[cfg(windows)]
unsafe extern "system" fn mouse(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, LLMHF_INJECTED, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
        WM_NCLBUTTONDOWN, WM_RBUTTONDOWN,
    };
    if code >= 0 {
        let msg = w.0 as u32;
        if matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_NCLBUTTONDOWN) {
            let m = &*(l.0 as *const MSLLHOOKSTRUCT);
            if m.flags & LLMHF_INJECTED == 0 {
                LAST_CLICK_MS.store(now_ms(), Ordering::Relaxed);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

#[cfg(windows)]
fn pump() {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, MSG, WH_KEYBOARD_LL,
        WH_MOUSE_LL,
    };
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), Some(module.into()), 0);
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), Some(module.into()), 0);
        if kb.is_err() || ms.is_err() {
            tracing::warn!("real-input hooks could not be installed; the foreground sentinel will not act");
            INSTALLED.store(0, Ordering::SeqCst);
            return;
        }
        INSTALLED.store(1, Ordering::SeqCst);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_click_or_alt_counts_as_choosing_a_window() {
        // Typing is not choosing: this is the case the sentinel exists for.
        assert!(!human_chose_a_window(None, None));
        assert!(!human_chose_a_window(Some(5_000), Some(30_000)));
        // A click just now, or an alt-tab a moment ago.
        assert!(human_chose_a_window(Some(50), None));
        assert!(human_chose_a_window(Some(900), None));
        assert!(!human_chose_a_window(Some(901), None));
        assert!(human_chose_a_window(None, Some(1_500)));
        assert!(!human_chose_a_window(None, Some(2_001)));
    }

    #[test]
    fn readers_report_nothing_before_any_real_input() {
        // The statics start at zero, which must read as "never seen", not "now".
        assert_eq!(ago(&AtomicU64::new(0)), None);
        assert!(ago(&AtomicU64::new(now_ms() - 10)).unwrap() < 5_000);
    }
}
