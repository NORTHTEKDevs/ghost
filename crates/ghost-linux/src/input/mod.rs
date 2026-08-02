//! Input, mirroring `ghost_core::input`.
//!
//! Backend selection, in priority order:
//!
//! | Session | Default backend | Why |
//! |---|---|---|
//! | X11 | XTEST | drives every client, absolute coordinates, no prompts |
//! | Wayland | RemoteDesktop portal | the only compositor-supported path |
//! | any, `GHOST_INPUT=uinput` | uinput | unattended; no portal lifetime |
//!
//! Remember that on Linux synthetic input is the *fallback*. The primary path is
//! [`BackgroundClicker`], which drives controls through AT-SPI: no pointer
//! movement, no focus steal, no prompts, and identical on X11 and Wayland.

pub mod hotkey_x11;
pub mod keyboard;
pub mod portal;
pub mod uinput;
pub mod x11;

use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

/// Key naming lives at the crate root so its tests run on any host; re-exported
/// here because shared code expects `input::keysym`.
pub use crate::keysym;
pub use crate::keysym::{name_to_vk, VirtualKey};

use crate::a11y::{global_bridge, handles, ops};
use crate::error::{CoreError, Result};
use crate::session::{session_kind, SessionKind};

/// Delay between a press and its release. Zero-length key events are dropped by
/// some toolkits, which reads as "typing did nothing".
const KEY_HOLD: Duration = Duration::from_millis(12);

pub enum Backend {
    // Boxed: X11Backend carries a connection and the whole keysym->keycode
    // map, which would otherwise make every Backend value that large.
    X11(Box<x11::X11Backend>),
    Portal(portal::PortalBackend),
    Uinput(uinput::UinputBackend),
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::X11(_) => "x11",
            Backend::Portal(_) => "portal",
            Backend::Uinput(_) => "uinput",
        }
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<()> {
        match self {
            Backend::X11(b) => b.move_to(x, y),
            Backend::Portal(b) => b.move_to(x, y),
            Backend::Uinput(b) => b.move_to(x, y),
        }
    }

    pub fn button(&self, btn: MouseButton, down: bool) -> Result<()> {
        match self {
            Backend::X11(b) => b.button(btn.x11_code(), down),
            Backend::Portal(b) => b.button(btn.evdev_code() as i32, down),
            Backend::Uinput(b) => b.button(btn.evdev_code(), down),
        }
    }

    pub fn key(&self, vk: VirtualKey, down: bool) -> Result<()> {
        match self {
            Backend::X11(b) => b.key(vk.0, down),
            Backend::Portal(b) => b.key(vk.0, down),
            // uinput speaks evdev codes, not keysyms, and mapping the full
            // keysym space to evdev codes needs the active keymap. Rather than
            // type the wrong characters, refuse and point at the working path.
            Backend::Uinput(_) => Err(CoreError::Unsupported(
                "the uinput backend cannot translate keysyms without the active keymap; \
                 use AT-SPI text entry (ghost_act with name/role) for typing"
                    .into(),
            )),
        }
    }

    /// Vertical scroll. Positive is up, matching the Windows engine.
    pub fn scroll(&self, clicks: i32) -> Result<()> {
        self.scroll_axis(clicks, false)
    }

    /// Scroll on either axis. Horizontal exists because the Windows engine
    /// supports `direction=left|right` via MOUSEEVENTF_HWHEEL; refusing it here
    /// would make the same `ghost_scroll` call succeed on one platform and error
    /// on the other.
    pub fn scroll_axis(&self, clicks: i32, horizontal: bool) -> Result<()> {
        match self {
            Backend::X11(b) => b.scroll_axis(clicks, horizontal),
            Backend::Portal(b) => b.scroll_axis(clicks, horizontal),
            Backend::Uinput(b) => b.scroll_axis(clicks, horizontal),
        }
    }

    pub fn cursor_pos(&self) -> Option<(i32, i32)> {
        match self {
            Backend::X11(b) => b.cursor_pos(),
            // Neither the portal nor uinput will report pointer position.
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    fn x11_code(&self) -> u8 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        }
    }
    fn evdev_code(&self) -> u16 {
        match self {
            MouseButton::Left => uinput::BTN_LEFT,
            MouseButton::Right => uinput::BTN_RIGHT,
            MouseButton::Middle => uinput::BTN_MIDDLE,
        }
    }
}

/// The process-wide input backend, created on first use.
pub fn backend() -> Result<&'static Backend> {
    static BACKEND: OnceLock<std::result::Result<Backend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| select_backend().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| CoreError::platform(e.clone()))
}

fn select_backend() -> Result<Backend> {
    if let Ok(forced) = std::env::var("GHOST_INPUT") {
        return match forced.trim().to_ascii_lowercase().as_str() {
            "uinput" => Ok(Backend::Uinput(uinput::UinputBackend::new()?)),
            "x11" => Ok(Backend::X11(Box::new(x11::X11Backend::new()?))),
            "portal" => Ok(Backend::Portal(portal::PortalBackend::new()?)),
            other => Err(CoreError::platform(format!(
                "GHOST_INPUT={other:?} is not recognised (expected x11, portal or uinput)"
            ))),
        };
    }

    match session_kind() {
        SessionKind::X11 => Ok(Backend::X11(Box::new(x11::X11Backend::new()?))),
        SessionKind::Wayland => Ok(Backend::Portal(portal::PortalBackend::new()?)),
        SessionKind::Headless => Err(CoreError::Unsupported(
            "no display session detected, so synthetic input is unavailable. AT-SPI actions still \
             work if an accessibility bus is reachable; set GHOST_INPUT=uinput for a headless box"
                .into(),
        )),
    }
}

// ----------------------------------------------------------------- keyboard

/// Mirrors `ghost_core::input::keyboard::type_text`.
///
/// This is the synthetic fallback. Prefer AT-SPI text entry, which handles
/// arbitrary Unicode without touching the keymap.
pub fn type_text(text: &str) -> Result<()> {
    let b = backend()?;
    for ch in text.chars() {
        let Some(sym) = keysym::char_to_keysym(ch) else { continue };
        let vk = VirtualKey(sym);
        b.key(vk, true)?;
        sleep(KEY_HOLD);
        b.key(vk, false)?;
    }
    Ok(())
}

pub fn press_key(vk: VirtualKey) -> Result<()> {
    let b = backend()?;
    b.key(vk, true)?;
    sleep(KEY_HOLD);
    b.key(vk, false)
}

pub fn key_down(vk: VirtualKey) -> Result<()> {
    backend()?.key(vk, true)
}

pub fn key_up(vk: VirtualKey) -> Result<()> {
    backend()?.key(vk, false)
}

/// Mirrors `ghost_core::input::keyboard::clear_focused_field`: select all, delete.
pub fn clear_focused_field() -> Result<()> {
    let b = backend()?;
    let ctrl = VirtualKey(0xFFE3);
    let a = VirtualKey(0x61);
    b.key(ctrl, true)?;
    b.key(a, true)?;
    sleep(KEY_HOLD);
    b.key(a, false)?;
    b.key(ctrl, false)?;
    press_key(VirtualKey(0xFFFF)) // Delete
}

// -------------------------------------------------------------------- mouse

pub mod mouse {
    use super::*;

    pub fn move_to(x: i32, y: i32) -> Result<()> {
        backend()?.move_to(x, y)
    }

    pub fn click(x: i32, y: i32) -> Result<()> {
        press_at(x, y, MouseButton::Left)
    }

    pub fn right_click(x: i32, y: i32) -> Result<()> {
        press_at(x, y, MouseButton::Right)
    }

    pub fn double_click(x: i32, y: i32) -> Result<()> {
        press_at(x, y, MouseButton::Left)?;
        sleep(Duration::from_millis(40));
        press_at(x, y, MouseButton::Left)
    }

    pub fn hover(x: i32, y: i32) -> Result<()> {
        backend()?.move_to(x, y)
    }

    pub fn drag(from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        let b = backend()?;
        b.move_to(from_x, from_y)?;
        b.button(MouseButton::Left, true)?;
        // Intermediate motion: a single jump is ignored by many drag handlers,
        // which only begin a drag after they see movement while held.
        for i in 1..=8 {
            let x = from_x + (to_x - from_x) * i / 8;
            let y = from_y + (to_y - from_y) * i / 8;
            b.move_to(x, y)?;
            sleep(Duration::from_millis(12));
        }
        b.button(MouseButton::Left, false)
    }

    pub fn scroll(x: i32, y: i32, direction: &str, amount: i32) -> Result<()> {
        let b = backend()?;
        b.move_to(x, y)?;
        let (clicks, horizontal) = match direction.trim().to_ascii_lowercase().as_str() {
            "up" => (amount, false),
            "down" => (-amount, false),
            "right" => (amount, true),
            "left" => (-amount, true),
            other => {
                return Err(CoreError::platform(format!(
                    "scroll direction {other:?} is not supported (expected up, down, left or right)"
                )))
            }
        };
        b.scroll_axis(clicks, horizontal)
    }

    fn press_at(x: i32, y: i32, btn: MouseButton) -> Result<()> {
        let b = backend()?;
        b.move_to(x, y)?;
        sleep(Duration::from_millis(8));
        b.button(btn, true)?;
        sleep(Duration::from_millis(20));
        b.button(btn, false)
    }
}

// ------------------------------------------------------------------ hotkey

pub mod hotkey {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::error::Result;

    /// Emergency-stop flag, mirroring the Windows engine's `STOP_FLAG`.
    pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

    pub fn is_stopped() -> bool {
        STOP_FLAG.load(Ordering::SeqCst)
    }

    pub fn trigger_stop() {
        STOP_FLAG.store(true, Ordering::SeqCst);
    }

    pub fn reset_stop() {
        STOP_FLAG.store(false, Ordering::SeqCst);
    }

    /// Register the global Ctrl+Alt+G emergency stop.
    ///
    /// Works on X11 via `GrabKey`, matching the Windows engine's
    /// `RegisterHotKey`. Wayland does not permit global key grabs, so there this
    /// reports unavailable and `ghost_stop` over MCP remains the supported
    /// route.
    ///
    /// A failure here is deliberately **not** fatal to session startup: losing
    /// the convenience hotkey must not stop Ghost from running.
    pub fn register_emergency_stop() -> Result<()> {
        match super::hotkey_x11::register() {
            Ok(()) => Ok(()),
            // Reported, not fatal: ghost_stop still works.
            Err(_) => Ok(()),
        }
    }

    /// Whether the global hotkey is actually active, for `ghost doctor`.
    pub fn global_hotkey_status() -> std::result::Result<(), String> {
        super::hotkey_x11::register().map_err(|e| e.to_string())
    }

    /// Release every modifier, unconditionally.
    ///
    /// `ghost_key` deliberately exposes `down:ctrl` / `up:ctrl` so a caller can
    /// hold a modifier across calls. If `ghost_stop` fires between the two, the
    /// modifier stays physically latched in the user's session until they tap
    /// the real key — and on Linux `ghost_stop` is the *only* out-of-band
    /// control, since there is no global emergency hotkey. So the stop path has
    /// to actively clear them, exactly as the Windows engine does.
    ///
    /// Errors are ignored on purpose: this runs on the panic/stop path, where
    /// making a best effort on every key matters more than reporting which one
    /// failed.
    pub fn release_all_modifiers() {
        const MODIFIERS: [u32; 8] = [
            0xFFE1, 0xFFE2, // Shift_L, Shift_R
            0xFFE3, 0xFFE4, // Control_L, Control_R
            0xFFE9, 0xFFEA, // Alt_L, Alt_R
            0xFFEB, 0xFFEC, // Super_L, Super_R
        ];
        let Ok(b) = super::backend() else { return };
        for sym in MODIFIERS {
            let _ = b.key(super::VirtualKey(sym), false);
        }
    }
}

// -------------------------------------------------- background dispatch

/// Edit commands, mirroring `ghost_core::input::EditCommand`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Cut,
    Paste,
    Undo,
    SelectAll,
}

impl EditCommand {
    /// Mirrors `EditCommand::from_ctrl_key`.
    pub fn from_ctrl_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "c" => Some(EditCommand::Copy),
            "x" => Some(EditCommand::Cut),
            "v" => Some(EditCommand::Paste),
            "z" => Some(EditCommand::Undo),
            "a" => Some(EditCommand::SelectAll),
            _ => None,
        }
    }

    /// Map to the engine-level command the AT-SPI path performs.
    fn kind(&self) -> ops::EditKind {
        match self {
            EditCommand::Copy => ops::EditKind::Copy,
            EditCommand::Cut => ops::EditKind::Cut,
            EditCommand::Paste => ops::EditKind::Paste,
            EditCommand::Undo => ops::EditKind::Undo,
            EditCommand::SelectAll => ops::EditKind::SelectAll,
        }
    }
}

/// Drive a control without taking foreground or moving the pointer.
///
/// On Windows this posts window messages. On Linux it calls the control's own
/// AT-SPI action, which is a stronger guarantee: the toolkit performs the
/// operation itself, so there is no pointer to move and no window to raise.
pub struct BackgroundClicker;

impl BackgroundClicker {
    /// Invoke the element behind `handle`.
    pub fn button_click(handle: isize) -> Result<()> {
        let addr = handles::resolve(handle).ok_or(CoreError::WindowGone)?;
        let bridge = global_bridge()?;
        bridge.run_default(move |ctx| Box::pin(async move { ops::do_best_action(ctx, &addr).await }))?;
        Ok(())
    }

    pub fn click(handle: isize, _client_xy: (i32, i32)) -> Result<()> {
        Self::button_click(handle)
    }

    /// Set a control's text directly -- no keystrokes, no focus change.
    pub fn set_text(handle: isize, text: &str) -> Result<()> {
        let addr = handles::resolve(handle).ok_or(CoreError::WindowGone)?;
        let text = text.to_string();
        let bridge = global_bridge()?;
        bridge.run_default(move |ctx| {
            Box::pin(async move { ops::set_text_value(ctx, &addr, &text).await })
        })?;
        Ok(())
    }

    /// The focused control inside a window, or 0.
    pub fn focused_control(window_handle: isize) -> isize {
        let Some(addr) = handles::resolve(window_handle) else { return 0 };
        let Ok(bridge) = global_bridge() else { return 0 };
        bridge
            .run_default(move |ctx| Box::pin(async move { ops::focused_descendant(ctx, &addr).await }))
            .ok()
            .flatten()
            .map(|a| handles::intern(&a))
            .unwrap_or(0)
    }

    /// Clipboard/edit commands, performed by the application itself.
    ///
    /// These go through AT-SPI's `EditableText`/`Text` interfaces, which is the
    /// direct analogue of the Windows engine's `WM_COPY`/`WM_CUT`/`WM_PASTE`/
    /// `EM_SETSEL`: the application runs its own copy or paste, so no keystroke
    /// is synthesised, the pointer does not move, and **focus does not change**.
    ///
    /// The earlier implementation grabbed focus and fired Ctrl+C. That both
    /// disturbed the desktop -- breaking the guarantee this call exists to
    /// provide -- and could act on the wrong control entirely if the focus grab
    /// quietly failed.
    pub fn edit_command(handle: isize, cmd: EditCommand) -> Result<()> {
        let addr = handles::resolve(handle).ok_or(CoreError::WindowGone)?;
        let bridge = global_bridge()?;
        let kind = cmd.kind();
        bridge.run_default(move |ctx| {
            Box::pin(async move { ops::edit_command(ctx, &addr, kind).await })
        })
    }

    pub fn send_key(handle: isize, keysym: u32) -> Result<()> {
        if let Some(addr) = handles::resolve(handle) {
            if let Ok(bridge) = global_bridge() {
                let _ = bridge
                    .run_default(move |ctx| Box::pin(async move { ops::grab_focus(ctx, &addr).await }));
            }
        }
        press_key(VirtualKey(keysym))
    }

    pub fn send_char(handle: isize, ch: char) -> Result<()> {
        match keysym::char_to_keysym(ch) {
            Some(sym) => Self::send_key(handle, sym),
            None => Ok(()),
        }
    }

    /// Background click at a screen point.
    ///
    /// On Windows this posts `WM_LBUTTONDOWN/UP` to the target window: the
    /// message never touches the OS cursor. Linux has no equivalent — a
    /// coordinate click here would be a *real* synthetic pointer warp, which
    /// moves the user's cursor and can steal focus under click-to-focus window
    /// managers.
    ///
    /// So this drives the element behind `handle` through AT-SPI instead, which
    /// is the actual background primitive on Linux. If that element exposes no
    /// usable action, the call **fails** rather than silently falling back to a
    /// pointer warp. That failure is the honest answer: callers use this path
    /// precisely because they were promised nothing would be disturbed, and
    /// `cursor_preserved` cannot even detect the violation on Wayland (pointer
    /// position is unreadable there, so an unchanged reading is assumed).
    pub fn click_screen(handle: isize, _x: i32, _y: i32) -> Result<()> {
        Self::button_click(handle).map_err(|_| CoreError::NotActionableInBackground {
            what: "click (element exposes no AT-SPI action; a coordinate click would move the real cursor)",
        })
    }

    pub fn double_click_screen(handle: isize, _x: i32, _y: i32) -> Result<()> {
        // AT-SPI has no distinct double-click action. Invoking twice is not
        // equivalent and can double-submit, so refuse rather than approximate.
        let _ = handle;
        Err(CoreError::NotActionableInBackground {
            what: "double_click (AT-SPI exposes no double-click action; retry without background=true)",
        })
    }

    pub fn right_click_screen(handle: isize, _x: i32, _y: i32) -> Result<()> {
        let _ = handle;
        Err(CoreError::NotActionableInBackground {
            what: "right_click (AT-SPI exposes no context-menu action; retry without background=true)",
        })
    }

    pub fn hover_screen(handle: isize, _x: i32, _y: i32) -> Result<()> {
        let _ = handle;
        Err(CoreError::NotActionableInBackground {
            what: "hover (hover has no accessibility equivalent; retry without background=true)",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_commands_parse_the_same_letters_as_the_windows_engine() {
        assert_eq!(EditCommand::from_ctrl_key("c"), Some(EditCommand::Copy));
        assert_eq!(EditCommand::from_ctrl_key("X"), Some(EditCommand::Cut));
        assert_eq!(EditCommand::from_ctrl_key("v"), Some(EditCommand::Paste));
        assert_eq!(EditCommand::from_ctrl_key("z"), Some(EditCommand::Undo));
        assert_eq!(EditCommand::from_ctrl_key("a"), Some(EditCommand::SelectAll));
        assert_eq!(EditCommand::from_ctrl_key("q"), None);
    }

    #[test]
    fn every_edit_command_maps_to_an_engine_kind() {
        // A missing arm would silently perform the wrong operation.
        assert_eq!(EditCommand::Copy.kind(), ops::EditKind::Copy);
        assert_eq!(EditCommand::Cut.kind(), ops::EditKind::Cut);
        assert_eq!(EditCommand::Paste.kind(), ops::EditKind::Paste);
        assert_eq!(EditCommand::Undo.kind(), ops::EditKind::Undo);
        assert_eq!(EditCommand::SelectAll.kind(), ops::EditKind::SelectAll);
    }

    #[test]
    fn mouse_button_codes_differ_between_x11_and_evdev() {
        // X11 numbers buttons 1/2/3; evdev uses 0x110+. Mixing them up would
        // right-click when asked to left-click.
        assert_eq!(MouseButton::Left.x11_code(), 1);
        assert_eq!(MouseButton::Right.x11_code(), 3);
        assert_eq!(MouseButton::Left.evdev_code(), 0x110);
        assert_eq!(MouseButton::Right.evdev_code(), 0x111);
    }

    #[test]
    fn stop_flag_round_trips() {
        hotkey::reset_stop();
        assert!(!hotkey::is_stopped());
        hotkey::trigger_stop();
        assert!(hotkey::is_stopped());
        hotkey::reset_stop();
        assert!(!hotkey::is_stopped());
    }
}
