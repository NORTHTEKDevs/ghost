use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use crate::error::CoreError;

pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

pub fn is_stopped() -> bool {
    STOP_FLAG.load(Ordering::Acquire)
}

pub fn trigger_stop() {
    STOP_FLAG.store(true, Ordering::Release);
}

pub fn reset_stop() {
    STOP_FLAG.store(false, Ordering::Release);
}

/// Register Ctrl+Alt+G as a global hotkey (ID=1).
/// Spawns a background thread that listens for WM_HOTKEY messages.
/// On trigger: sets STOP_FLAG, releases all modifier keys.
pub fn register_emergency_stop() -> Result<(), CoreError> {
    unsafe {
        // ID=1 is reserved for emergency stop. Idempotent: already registered = OK.
        if let Err(e) = RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT, b'G' as u32) {
            const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;
            if e.code().0 as u32 != ERROR_HOTKEY_ALREADY_REGISTERED {
                return Err(CoreError::Win32 { code: e.code().0 as u32, context: "RegisterHotKey" });
            }
        }
    }

    std::thread::spawn(|| {
        let mut msg = MSG::default();
        unsafe {
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 { break; } // WM_QUIT
                if ret.0 == -1 {
                    tracing::error!("GetMessageW failed in hotkey thread");
                    break;
                }
                if msg.message == WM_HOTKEY && msg.wParam.0 == 1 {
                    tracing::warn!("Emergency stop triggered (Ctrl+Alt+G)");
                    trigger_stop();
                    release_all_modifiers();
                }
            }
        }
    });

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

    #[test]
    fn stop_flag_starts_false() {
        STOP_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!is_stopped());
    }

    #[test]
    fn stop_flag_set_and_reset() {
        STOP_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
        trigger_stop();
        assert!(is_stopped());
        reset_stop();
        assert!(!is_stopped());
    }
}
