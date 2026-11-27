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

/// The typing read-back judge, one source shared with `ghost_core::input`.
#[path = "../../../ghost-core/src/input/readback.rs"]
pub mod readback;
pub use readback::typed_ok;

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
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }

    // Serialise attempts so two callers cannot race into raising two consent
    // dialogs on Wayland.
    let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }

    // Only SUCCESS is cached. A failure here is very often transient and
    // user-recoverable: on Wayland the first synthetic input raises a consent
    // dialog, and the user may decline it, never see it (it can open on another
    // workspace), or let it time out. Caching that error would leave synthetic
    // input dead for the entire life of the MCP server, recoverable only by
    // restarting the client -- turning one mis-click into a broken session.
    // Retrying means the next call simply asks again.
    let b = select_backend()?;
    Ok(BACKEND.get_or_init(|| b))
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
    //! The emergency stop, shared by every Ghost process of this user.
    //!
    //! Windows broadcasts the stop through two named kernel events. Here the
    //! same two states are marker files in a per-user directory:
    //! `$XDG_RUNTIME_DIR/ghost/` (`stop`, `human-stop`), or, where
    //! `XDG_RUNTIME_DIR` is unset (a bare ssh or cron session),
    //! `/tmp/ghost-stop-<uid>/`. Either directory must be a real directory
    //! (not a symlink), owned by this user, with no group or other access;
    //! Ghost creates it that way. One that fails those checks, or cannot be
    //! read, cannot be trusted to show a stop another process raised, so every
    //! process counts as stopped (fail closed, as on Windows) and
    //! `stop_event_error` says why. Processes of one user that see different
    //! `XDG_RUNTIME_DIR` values do not share a stop.
    //!
    //! Known denial of service, by design: on a multi-user host where
    //! `XDG_RUNTIME_DIR` is unset, another local user can create
    //! `/tmp/ghost-stop-<uid>` first (or a symlink there). That directory
    //! fails the checks, so this user's Ghost counts as stopped until it is
    //! removed - fail closed: a stop marker that cannot be trusted is treated
    //! as a stop, never ignored. Setting `XDG_RUNTIME_DIR` (a login session
    //! does) avoids the shared `/tmp`.
    //!
    //! `STOP_FLAG` stays process-local: `ghost_stop scope=process` sets it
    //! alone.

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::error::Result;

    /// Emergency-stop flag, mirroring the Windows engine's `STOP_FLAG`:
    /// this process only.
    pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

    /// Raised by any stop; an agent's reset removes it.
    const STOP: &str = "stop";
    /// Raised with `STOP` when the human stopped Ghost (Ctrl+Alt+G); only the
    /// human's resume removes it.
    const HUMAN: &str = "human-stop";

    /// This process's effective uid. One syscall that cannot fail: it runs
    /// on every `is_stopped`, i.e. before every action.
    fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions and always succeeds.
        unsafe { libc::geteuid() }
    }

    /// The marker directory this process uses.
    ///
    /// This crate's own tests never use the real one: `GHOST_TEST_STOP_DIR`
    /// when set (a per-test directory), else a per-process one under the temp
    /// dir. The variable is read only under `cfg(test)`: a shipped binary that
    /// honoured it would let any agent with a shell start a Ghost that ignores
    /// the human's stop.
    fn marker_dir() -> std::result::Result<PathBuf, String> {
        if cfg!(test) {
            return Ok(std::env::var_os("GHOST_TEST_STOP_DIR").map(PathBuf::from).unwrap_or_else(|| {
                std::env::temp_dir().join(format!("ghost-stop-test-{}", std::process::id()))
            }));
        }
        match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            Some(d) => Ok(PathBuf::from(d).join("ghost")),
            None => {
                Ok(PathBuf::from(format!("/tmp/ghost-stop-{}", euid())))
            }
        }
    }

    /// Check that `dir` is safe to trust: a directory (not a symlink), owned
    /// by this user, with no group/other access. `Ok(false)` when it does not
    /// exist yet (then no stop was ever raised through it).
    fn check_dir(dir: &Path) -> std::result::Result<bool, String> {
        use std::os::unix::fs::MetadataExt;
        let meta = match std::fs::symlink_metadata(dir) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("cannot read the stop marker dir {}: {e}", dir.display())),
        };
        let uid = euid();
        if !meta.file_type().is_dir() {
            Err(format!("the stop marker dir {} is not a directory", dir.display()))
        } else if meta.uid() != uid {
            Err(format!("the stop marker dir {} is owned by uid {}, not {uid}", dir.display(), meta.uid()))
        } else if meta.mode() & 0o077 != 0 {
            Err(format!("the stop marker dir {} is open to other users (mode {:o})", dir.display(), meta.mode() & 0o777))
        } else {
            Ok(true)
        }
    }

    /// Whether `name` is raised. A directory that cannot be trusted counts as
    /// raised (fail closed).
    fn raised(name: &str) -> bool {
        let Ok(dir) = marker_dir() else { return true };
        match check_dir(&dir) {
            Ok(false) => false,
            Ok(true) => match std::fs::symlink_metadata(dir.join(name)) {
                Ok(_) => true,
                Err(e) => e.kind() != std::io::ErrorKind::NotFound,
            },
            Err(_) => true,
        }
    }

    /// Raise or lower `name`, creating the directory (0700) when needed.
    fn set(name: &str, up: bool) -> std::result::Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = marker_dir()?;
        if !check_dir(&dir)? {
            if !up {
                return Ok(());
            }
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("cannot create the stop marker dir {}: {e}", dir.display())),
            }
            // Re-check: another process (or user) may have won the race.
            check_dir(&dir)?;
        }
        let path = dir.join(name);
        let r = if up {
            std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path).map(drop)
        } else {
            std::fs::remove_file(&path)
        };
        match r {
            Err(e) if !(e.kind() == std::io::ErrorKind::NotFound && !up) => {
                Err(format!("cannot {} the stop marker {}: {e}", if up { "raise" } else { "lower" }, path.display()))
            }
            _ => Ok(()),
        }
    }

    /// Whether the emergency stop is active - raised here, or by any other
    /// Ghost process of this user through the shared markers.
    pub fn is_stopped() -> bool {
        STOP_FLAG.load(Ordering::SeqCst) || raised(STOP) || raised(HUMAN)
    }

    /// Whether the stop in force is the shared one, not only this process's
    /// `scope: "process"` flag.
    pub fn shared_stop_raised() -> bool {
        raised(STOP) || raised(HUMAN)
    }

    /// Whether the human raised the stop (in any Ghost process). An
    /// untrustworthy marker dir counts as raised: a human stop cannot be ruled
    /// out, so an agent reset is refused.
    pub fn human_stop_active() -> bool {
        raised(HUMAN)
    }

    /// Why this process cannot trust the shared stop markers, when it cannot.
    /// While this is `Some`, every action is refused and no reset helps.
    pub fn stop_event_error() -> Option<String> {
        marker_dir().and_then(|d| check_dir(&d)).err()
    }

    /// An agent's stop (`ghost_stop`): raised for every Ghost process. If the
    /// marker cannot be written, this process stops anyway.
    pub fn trigger_stop() {
        if let Err(e) = set(STOP, true) {
            tracing::warn!("emergency stop: {e}; stopping this process only");
            STOP_FLAG.store(true, Ordering::SeqCst);
        }
    }

    /// The human's stop (Ctrl+Alt+G): only `reset_human_stop` lifts it.
    pub fn trigger_human_stop() {
        let human = set(HUMAN, true);
        let stop = set(STOP, true);
        if let Err(e) = human.and(stop) {
            tracing::warn!("emergency stop: {e}; stopping this process only");
            STOP_FLAG.store(true, Ordering::SeqCst);
        }
    }

    /// An agent's reset (`ghost_reset`): lowers this process's flag and the
    /// shared stop, never the human's marker - a stop the human raised stays
    /// in force everywhere.
    pub fn reset_stop() {
        STOP_FLAG.store(false, Ordering::SeqCst);
        if let Err(e) = set(STOP, false) {
            tracing::warn!("emergency stop reset: {e}");
        }
    }

    /// The human's resume (Ctrl+Alt+Shift+G, or `ghost reset`): lifts any
    /// stop, theirs included, in every Ghost process.
    pub fn reset_human_stop() {
        STOP_FLAG.store(false, Ordering::SeqCst);
        for name in [HUMAN, STOP] {
            if let Err(e) = set(name, false) {
                tracing::warn!("emergency stop resume: {e}");
            }
        }
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

    /// Release every modifier, unconditionally.
    ///
    /// `ghost_key` deliberately exposes `down:ctrl` / `up:ctrl` so a caller can
    /// hold a modifier across calls. If `ghost_stop` fires between the two, the
    /// modifier stays physically latched in the user's session until they tap
    /// the real key - and on Linux `ghost_stop` is the *only* out-of-band
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

    #[cfg(test)]
    mod uid_tests {
        /// `is_stopped` runs on every action: the uid comes from the kernel
        /// (one syscall), not from parsing `/proc/self/status` each time.
        #[test]
        fn euid_is_the_syscall_not_a_proc_read() {
            let src = include_str!("mod.rs");
            let start = src.find("    fn euid()").expect("euid");
            let body = &src[start..start + src[start..].find("
    }").expect("end")];
            assert!(!body.contains("/proc/self/status"), "euid still reads /proc:
{body}");
            assert!(body.contains("geteuid"), "{body}");
        }

        #[test]
        fn euid_matches_the_kernel() {
            assert_eq!(super::euid(), unsafe { libc::geteuid() });
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
        bridge.run_action_default(move |ctx| Box::pin(async move { ops::do_best_action(ctx, &addr).await }))?;
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
        bridge.run_action_default(move |ctx| {
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
        bridge.run_action_default(move |ctx| {
            Box::pin(async move { ops::edit_command(ctx, &addr, kind).await })
        })
    }

    pub fn send_key(handle: isize, keysym: u32) -> Result<()> {
        if let Some(addr) = handles::resolve(handle) {
            if let Ok(bridge) = global_bridge() {
                let _ = bridge
                    .run_action_default(move |ctx| Box::pin(async move { ops::grab_focus(ctx, &addr).await }));
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
    /// message never touches the OS cursor. Linux has no equivalent - a
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
        hotkey::reset_human_stop();
        assert!(!hotkey::is_stopped());
        hotkey::trigger_stop();
        assert!(hotkey::is_stopped() && !hotkey::human_stop_active());
        hotkey::reset_stop();
        assert!(!hotkey::is_stopped());
        // A human stop survives an agent reset; only the human's resume lifts it.
        hotkey::trigger_human_stop();
        hotkey::reset_stop();
        assert!(hotkey::is_stopped() && hotkey::human_stop_active());
        hotkey::reset_human_stop();
        assert!(!hotkey::is_stopped());
    }

    /// A per-test marker directory (0700, ours), never the real shared one.
    fn temp_marker_dir(tag: &str) -> std::path::PathBuf {
        use std::os::unix::fs::DirBuilderExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ghost-stop-test-{}-{tag}-{nanos}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).expect("temp marker dir");
        dir
    }

    /// Run `child_stop_probe` in a fresh process against `dir`: what a second
    /// Ghost process (`ghost reset`, a new `ghost-mcp`) would see.
    fn probe(dir: &std::path::Path, step: &str) -> bool {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["input::tests::child_stop_probe", "--exact", "--test-threads=1", "--quiet"])
            .env("GHOST_TEST_STOP_DIR", dir)
            .env("GHOST_TEST_STOP_PROBE", step)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// The child half of the cross-process tests; a no-op in a normal run.
    #[test]
    fn child_stop_probe() {
        let Ok(step) = std::env::var("GHOST_TEST_STOP_PROBE") else { return };
        assert!(std::env::var_os("GHOST_TEST_STOP_DIR").is_some(), "never probe the real markers");
        match step.as_str() {
            "trigger" => hotkey::trigger_stop(),
            "trigger-human" => hotkey::trigger_human_stop(),
            "agent-reset" => hotkey::reset_stop(),
            "human-reset" => hotkey::reset_human_stop(),
            "expect-stopped" => assert!(hotkey::is_stopped()),
            "expect-human" => assert!(hotkey::is_stopped() && hotkey::human_stop_active()),
            "expect-running" => assert!(!hotkey::is_stopped() && !hotkey::human_stop_active()),
            other => panic!("unknown probe step {other}"),
        }
    }

    /// A stop raised in one process is in force in every other, an agent's
    /// reset never lifts the human's, and the human's resume lifts both.
    #[test]
    fn a_stop_reaches_every_process_and_only_the_human_lifts_theirs() {
        let dir = temp_marker_dir("reach");
        assert!(probe(&dir, "expect-running"), "a fresh dir is not stopped");
        assert!(probe(&dir, "trigger"));
        assert!(dir.join("stop").exists(), "the stop left no shared marker");
        assert!(probe(&dir, "expect-stopped"), "another process did not see the stop");
        assert!(probe(&dir, "agent-reset"));
        assert!(probe(&dir, "expect-running"), "a reset elsewhere did not lift it");

        assert!(probe(&dir, "trigger-human"));
        assert!(probe(&dir, "agent-reset"));
        assert!(dir.join("human-stop").exists(), "an agent reset removed the human's marker");
        assert!(probe(&dir, "expect-human"), "an agent reset lifted the human's stop");
        assert!(probe(&dir, "human-reset"));
        assert!(probe(&dir, "expect-running"), "the human's resume did not lift it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker directory others could write to (or that is not ours) cannot
    /// be trusted to show a stop: every process counts as stopped.
    #[test]
    fn an_insecure_marker_dir_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_marker_dir("open");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(probe(&dir, "expect-human"), "a world-writable marker dir was trusted");
        let _ = std::fs::remove_dir_all(&dir);

        // Owned by someone else (only checkable as root, which may chown).
        let root = std::fs::read_to_string("/proc/self/status")
            .map(|s| s.lines().any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(2) == Some("0")))
            .unwrap_or(false);
        if root {
            let dir = temp_marker_dir("owner");
            std::os::unix::fs::chown(&dir, Some(4_321), None).unwrap();
            assert!(probe(&dir, "expect-human"), "a marker dir owned by another user was trusted");
            let _ = std::fs::remove_dir_all(&dir);
        }

        // A symlink in place of the directory is refused too.
        let real = temp_marker_dir("target");
        let link = real.with_extension("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(probe(&link, "expect-human"), "a symlinked marker dir was trusted");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
    }
}
