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

/// Raised together with the stop event when the HUMAN stops Ghost (Ctrl+Alt+G).
/// While it is up, an agent's `ghost_reset` cannot lift the stop: only the
/// human can (Ctrl+Alt+Shift+G, or `ghost reset` in a terminal).
const HUMAN_EVENT_NAME: &str = r"Local\ghost-emergency-stop-human";

/// The name this process uses for one of the shared events: the real one,
/// except
/// - in this crate's own unit tests, which use a per-process name
///   (`trigger_stop` there used to raise the shared event, so every
///   `cargo test` briefly stopped every Ghost process on the machine), and
/// - in a private build with the `isolated-stop-events` feature, which takes a
///   namespace from `GHOST_ISOLATED_STOP_NS` so a live test can raise a stop
///   without stopping the human's running Ghosts. Release builds never enable
///   the feature: a released binary that honoured the variable would let any
///   agent with a shell start a Ghost that ignores the human's stop.
fn event_name(base: &str) -> String {
    if cfg!(test) {
        return format!("{base}-test-{}", std::process::id());
    }
    match isolated_ns() {
        Some(ns) => format!("{base}-isolated-{ns}"),
        None => base.to_string(),
    }
}

// The private feature must never reach a release build (`--all-features`
// would otherwise switch it on): private test builds are debug builds.
#[cfg(all(feature = "isolated-stop-events", not(debug_assertions)))]
compile_error!("`isolated-stop-events` is for private debug test builds only; never build it in release");

#[cfg(feature = "isolated-stop-events")]
fn isolated_ns() -> Option<String> {
    std::env::var("GHOST_ISOLATED_STOP_NS")
        .ok()
        .filter(|ns| !ns.is_empty() && ns.len() <= 32 && ns.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

#[cfg(not(feature = "isolated-stop-events"))]
fn isolated_ns() -> Option<String> {
    None
}

fn stop_event_name() -> String {
    event_name(STOP_EVENT_NAME)
}

fn human_event_name() -> String {
    event_name(HUMAN_EVENT_NAME)
}

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Whether the emergency stop is active - raised here, or by any other Ghost
/// process through the shared event.
///
/// The event is read directly, not only through the watcher thread's copy:
/// the watcher polls once a second, so a process started while a stop was in
/// force (every CLI command, `ghost-http`, a fresh `ghost-mcp`) acted for up
/// to a second before it knew (docs/plans/2026-11-26-emergency-stop-audit.md).
pub fn is_stopped() -> bool {
    // The human event too: an agent reset lowers only the stop event, so a
    // human stop must hold on its own even if that reset races the hotkey.
    STOP_FLAG.load(Ordering::Acquire) || raised(&STOP_EVENT, stop_event_name) || raised(&HUMAN_EVENT, human_event_name)
}

/// Whether the stop in force is the shared one (raised by `ghost_stop`, the
/// hotkey, or any other Ghost process) rather than only this process's
/// `scope: "process"` flag. An unreadable event counts as raised.
pub fn shared_stop_raised() -> bool {
    raised(&STOP_EVENT, stop_event_name) || raised(&HUMAN_EVENT, human_event_name)
}

/// Whether the human raised the stop (Ctrl+Alt+G, in any Ghost process). An
/// event that cannot be read counts as raised: a human stop cannot be ruled
/// out, so an agent reset is refused (fail closed).
pub fn human_stop_active() -> bool {
    raised(&HUMAN_EVENT, human_event_name)
}

/// The shared event is signalled - or cannot be read at all. A process that
/// cannot open the event cannot see a stop raised by another Ghost process
/// (the Ctrl+Alt+G owner, a `ghost_stop` elsewhere), so it fails closed: it
/// counts as stopped, and `stop_event_error` says why. A zero-timeout wait
/// only reads the state.
fn raised(cell: &'static SharedEvent, name: fn() -> String) -> bool {
    match open_event(cell, name).as_ref() {
        Ok(ev) => unsafe { WaitForSingleObject(ev.0, 0) == WAIT_OBJECT_0 },
        Err(_) => true,
    }
}

/// Why this process cannot read the shared stop events, when it cannot. While
/// this is `Some`, every action is refused (fail closed) and a reset cannot
/// help: the fix is to end the Ghost process that created the event with a
/// DACL this user is not in, then restart this one.
pub fn stop_event_error() -> Option<String> {
    open_event(&STOP_EVENT, stop_event_name)
        .as_ref()
        .err()
        .or_else(|| open_event(&HUMAN_EVENT, human_event_name).as_ref().err())
        .cloned()
}

fn set(cell: &'static SharedEvent, name: fn() -> String, up: bool) {
    if let Ok(ev) = open_event(cell, name) {
        unsafe {
            let _ = if up { SetEvent(ev.0) } else { ResetEvent(ev.0) };
        }
    }
}

/// An agent's stop (`ghost_stop`): trip it locally and broadcast it to every
/// other ghost process. Any reset lifts it.
pub fn trigger_stop() {
    STOP_FLAG.store(true, Ordering::Release);
    set(&STOP_EVENT, stop_event_name, true);
}

/// The human's stop (Ctrl+Alt+G): the same broadcast, marked as the human's,
/// so only the human can lift it (`reset_human_stop`).
pub fn trigger_human_stop() {
    STOP_FLAG.store(true, Ordering::Release);
    set(&HUMAN_EVENT, human_event_name, true);
    set(&STOP_EVENT, stop_event_name, true);
}

/// An agent's reset (`ghost_reset`): lowers this process's flag and the stop
/// event, never the human's mark - so a stop the human raised stays in force
/// (`is_stopped` reads the human event). Callers check `human_stop_active`
/// first to tell the agent why. Lowering the event matters: otherwise every new
/// ghost process would arrive pre-stopped, and the watchers would re-trip.
pub fn reset_stop() {
    STOP_FLAG.store(false, Ordering::Release);
    set(&STOP_EVENT, stop_event_name, false);
}

/// The human's resume (Ctrl+Alt+Shift+G, or `ghost reset` in a terminal):
/// lifts any stop, theirs included.
pub fn reset_human_stop() {
    STOP_FLAG.store(false, Ordering::Release);
    set(&HUMAN_EVENT, human_event_name, false);
    set(&STOP_EVENT, stop_event_name, false);
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
type SharedEvent = std::sync::OnceLock<std::result::Result<EventHandle, String>>;
static STOP_EVENT: SharedEvent = std::sync::OnceLock::new();
static HUMAN_EVENT: SharedEvent = std::sync::OnceLock::new();

fn open_stop_event() -> Option<&'static EventHandle> {
    open_event(&STOP_EVENT, stop_event_name).as_ref().ok()
}

fn open_event(cell: &'static SharedEvent, name: fn() -> String) -> &'static std::result::Result<EventHandle, String> {
    cell.get_or_init(|| {
        let name = name();
        // An explicit DACL that grants this user: with the default one, an
        // elevated Ghost that creates the event first grants only
        // Administrators and SYSTEM, and every non-elevated Ghost after it
        // could not open the event. (When the event already exists, the
        // descriptor is ignored and the open is checked against its DACL.)
        let sec = event_security(true);
        if sec.is_none() {
            tracing::warn!("stop event: could not build its security descriptor; using the default DACL");
        }
        unsafe { CreateEventW(sec.as_ref().map(|s| &s.attributes as *const _), true, false, &HSTRING::from(name.as_str())) }
            .map(EventHandle)
            .map_err(|e| {
                let why = format!(
                    "cannot open the shared emergency-stop event {name} ({e}); a stop raised by another Ghost \
                     process would go unseen here, so this process refuses every action. End the Ghost process \
                     that created the event (one running elevated, or any other program using that name) and \
                     restart this one"
                );
                tracing::error!("{why}");
                why
            })
    })
}

/// A security descriptor for the stop event: full access for SYSTEM and, when
/// `grant_user`, for the current user, INTERACTIVE and Administrators - so
/// every Ghost in this logon session can open it, elevated or not, including
/// one elevated as a different admin account (over-the-shoulder UAC). `Local\`
/// already confines the name to this session. Tests build one with SYSTEM only
/// to stand in for an event this user cannot open.
struct EventSecurity {
    attributes: windows::Win32::Security::SECURITY_ATTRIBUTES,
    // Referenced by `attributes`; kept alive with it.
    _descriptor: Box<windows::Win32::Security::SECURITY_DESCRIPTOR>,
    _acl: Vec<u64>,
    _sids: Vec<Vec<u64>>,
}

fn event_security(grant_user: bool) -> Option<EventSecurity> {
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    const EVENT_ALL_ACCESS: u32 = 0x001F_0003;
    unsafe {
        let mut sids: Vec<Vec<u64>> = Vec::new();
        let mut well_known = vec![WinLocalSystemSid];
        if grant_user {
            well_known.extend([WinInteractiveSid, WinBuiltinAdministratorsSid]);
        }
        for kind in well_known {
            let mut len = SECURITY_MAX_SID_SIZE;
            let mut sid = vec![0u64; (len as usize).div_ceil(8)];
            CreateWellKnownSid(kind, None, Some(PSID(sid.as_mut_ptr().cast())), &mut len).ok()?;
            sids.push(sid);
        }
        // The current user, from the process token.
        if grant_user {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
            let mut need = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut need);
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), need, &mut need);
            let _ = CloseHandle(token);
            got.ok()?;
            let user = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
            let n = GetLengthSid(user);
            let mut copy = vec![0u64; (n as usize).div_ceil(8)];
            CopySid(n, PSID(copy.as_mut_ptr().cast()), user).ok()?;
            sids.push(copy);
        }
        let mut acl = vec![0u64; 128]; // 1 KiB: room for a few ACEs
        let pacl = acl.as_mut_ptr() as *mut ACL;
        InitializeAcl(pacl, (acl.len() * 8) as u32, ACL_REVISION).ok()?;
        for sid in &mut sids {
            AddAccessAllowedAce(pacl, ACL_REVISION, EVENT_ALL_ACCESS, PSID(sid.as_mut_ptr().cast())).ok()?;
        }
        let mut descriptor = Box::new(SECURITY_DESCRIPTOR::default());
        let psd = PSECURITY_DESCRIPTOR(&mut *descriptor as *mut SECURITY_DESCRIPTOR as *mut _);
        InitializeSecurityDescriptor(psd, 1 /* SECURITY_DESCRIPTOR_REVISION */).ok()?;
        SetSecurityDescriptorDacl(psd, true, Some(pacl), false).ok()?;
        Some(EventSecurity {
            attributes: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd.0,
                bInheritHandle: false.into(),
            },
            _descriptor: descriptor,
            _acl: acl,
            _sids: sids,
        })
    }
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
                        // The local flag, not is_stopped(): that reads this
                        // same event, so it is always true here and the
                        // modifier release below never ran.
                        if !STOP_FLAG.load(Ordering::Acquire) {
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
            // Ctrl+Alt+G (the human's stop) is a RegisterHotKey hotkey: only
            // one process can own it, and another Ghost process may and will
            // broadcast for us; a thread timer retries every 3 s so ownership
            // migrates here if that process exits. Anything may RAISE a stop.
            //
            // Ctrl+Alt+Shift+G (the human's resume) is read by a low-level
            // keyboard hook instead, which sees whether a key was injected
            // (SendInput, SendKeys, keybd_event) and ignores it: RegisterHotKey
            // fires for injected keys too, so any program could have resumed.
            let mut owned = false;
            let register = |owned: &mut bool| {
                if !*owned {
                    *owned = RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT, b'G' as u32).is_ok();
                }
            };
            register(&mut owned);
            let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            if SetWindowsHookExW(WH_KEYBOARD_LL, Some(resume_hook), Some(module.into()), 0).is_err() {
                tracing::warn!(
                    "the resume key is unavailable here (keyboard hook refused)"
                );
            }
            let _ = SetTimer(None, 0, 3_000, None);
            let mut msg = MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 || ret.0 == -1 {
                    break;
                }
                match (msg.message, msg.wParam.0) {
                    (WM_TIMER, _) if !owned => register(&mut owned),
                    (WM_HOTKEY, 1) => {
                        tracing::warn!("emergency stop raised by the human (Ctrl+Alt+G)");
                        trigger_human_stop();
                        release_all_modifiers();
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| CoreError::ComInit(format!("spawn hotkey thread: {e}")))?;

    Ok(())
}

/// Physical (not injected) state of the resume chord's modifiers, kept by the
/// keyboard hook from real key events only.
static PHYS_CTRL: AtomicBool = AtomicBool::new(false);
static PHYS_ALT: AtomicBool = AtomicBool::new(false);
static PHYS_SHIFT: AtomicBool = AtomicBool::new(false);

/// The resume chord: G pressed, not injected, while Ctrl, Alt and Shift are
/// physically down (each tracked from non-injected events).
fn is_human_resume(vk: u32, injected: bool, ctrl: bool, alt: bool, shift: bool) -> bool {
    vk == u32::from(b'G') && !injected && ctrl && alt && shift
}

/// Low-level keyboard hook (installed on the hotkey thread, whose message
/// loop services it): tracks the physical modifiers and lifts the human's stop
/// on a real Ctrl+Alt+Shift+G. Never swallows a key.
unsafe extern "system" fn resume_hook(code: i32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = k.flags.0 & LLKHF_INJECTED.0 != 0;
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if !injected && (down || up) {
            let flag = match VIRTUAL_KEY(k.vkCode as u16) {
                VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Some(&PHYS_CTRL),
                VK_MENU | VK_LMENU | VK_RMENU => Some(&PHYS_ALT),
                VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Some(&PHYS_SHIFT),
                _ => None,
            };
            if let Some(f) = flag {
                f.store(down, Ordering::Relaxed);
            }
        }
        if down
            && is_human_resume(
                k.vkCode,
                injected,
                PHYS_CTRL.load(Ordering::Relaxed),
                PHYS_ALT.load(Ordering::Relaxed),
                PHYS_SHIFT.load(Ordering::Relaxed),
            )
            && human_stop_active()
        {
            tracing::warn!("emergency stop lifted by the human (Ctrl+Alt+Shift+G)");
            reset_human_stop();
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
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

    /// Set in the child copy of this test binary that runs the round trip.
    const CHILD_ENV: &str = "GHOST_STOP_ROUND_TRIP_CHILD";

    /// Raising the stop in this process would refuse every gated primitive
    /// that another unit test is calling on a parallel thread (13 of them
    /// failed with `Stopped` in 2 of 7 runs). So the round trip runs in a child
    /// copy of this binary, which has its own flag and its own per-process
    /// event, and this process never raises either.
    #[test]
    fn stop_flag_round_trips_and_broadcasts() {
        if std::env::var_os(CHILD_ENV).is_some() {
            return round_trip();
        }
        run_in_child("input::hotkey::tests::stop_flag_round_trips_and_broadcasts");
    }

    /// Run one test of this binary in a child process (CHILD_ENV set), so its
    /// stop flag and event are not this process's.
    fn run_in_child(test: &str) {
        let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, "1")
            .output()
            .expect("run the test in a child process");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{test} failed in the child:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// An event this process cannot open (another process created it with a
    /// DACL that leaves this user out - an elevated Ghost with the default
    /// admin-only DACL, or a squatter) must not read as "not stopped": a stop
    /// raised there would go unseen here. Fail closed, and say why.
    #[test]
    fn an_event_this_process_cannot_open_fails_closed() {
        if std::env::var_os(CHILD_ENV).is_none() {
            return run_in_child("input::hotkey::tests::an_event_this_process_cannot_open_fails_closed");
        }
        // Created before this process first touches the event: SYSTEM only.
        let sec = event_security(false).expect("security descriptor");
        let squatter = unsafe { CreateEventW(Some(&sec.attributes), true, false, &HSTRING::from(stop_event_name())) }
            .expect("create the event");
        assert!(!STOP_FLAG.load(Ordering::Acquire));
        assert!(open_stop_event().is_none(), "this user should not be able to open it");
        assert!(is_stopped(), "an event that cannot be read must fail closed");
        let why = stop_event_error().expect("the reason is reported");
        assert!(why.contains(STOP_EVENT_NAME), "{why}");
        unsafe {
            let _ = CloseHandle(squatter);
        }
    }

    /// The event Ghost creates grants this user, so a second process of the
    /// same user - elevated or not - can open it.
    #[test]
    fn the_event_ghost_creates_is_open_to_this_user() {
        if std::env::var_os(CHILD_ENV).is_none() {
            return run_in_child("input::hotkey::tests::the_event_ghost_creates_is_open_to_this_user");
        }
        assert!(open_stop_event().is_some(), "{:?}", stop_event_error());
        assert!(stop_event_error().is_none());
        assert!(!is_stopped());
    }

    /// H1: a stop the human raised (Ctrl+Alt+G) survives an agent's reset and
    /// is lifted only by the human's resume; an agent's own stop is not marked
    /// as the human's and any reset lifts it.
    #[test]
    fn only_the_human_lifts_a_human_stop() {
        if std::env::var_os(CHILD_ENV).is_none() {
            return run_in_child("input::hotkey::tests::only_the_human_lifts_a_human_stop");
        }
        reset_human_stop();
        assert!(!is_stopped() && !human_stop_active());

        // The agent's stop: any reset lifts it.
        trigger_stop();
        assert!(is_stopped() && !human_stop_active(), "an agent stop is not the human's");
        reset_stop();
        assert!(!is_stopped());

        // The human's stop: the agent reset lowers its own flag and the stop
        // event, and the stop still holds through the human event.
        trigger_human_stop();
        assert!(is_stopped() && human_stop_active());
        reset_stop();
        assert!(human_stop_active(), "an agent reset lowered the human's mark");
        assert!(is_stopped(), "an agent reset lifted a human stop");

        // The human's resume lifts it.
        reset_human_stop();
        assert!(!is_stopped() && !human_stop_active());
    }

    /// The resume chord counts only real key presses: an injected G (or a
    /// chord whose modifiers were injected, so never marked down) does not.
    #[test]
    fn only_a_physical_ctrl_alt_shift_g_resumes() {
        let g = u32::from(b'G');
        assert!(is_human_resume(g, false, true, true, true));
        assert!(!is_human_resume(g, true, true, true, true), "an injected G resumed");
        assert!(!is_human_resume(g, false, true, true, false), "Ctrl+Alt+G (the stop key) resumed");
        assert!(!is_human_resume(g, false, false, true, true));
        assert!(!is_human_resume(u32::from(b'H'), false, true, true, true));
    }

    /// A release build never takes the event names from the environment: a
    /// binary that did would let any agent start a Ghost that ignores the
    /// human's stop. (Run in a child with the variable set.)
    #[test]
    fn a_default_build_ignores_the_isolated_namespace() {
        if std::env::var_os(CHILD_ENV).is_none() {
            let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args(["input::hotkey::tests::a_default_build_ignores_the_isolated_namespace", "--exact", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .env("GHOST_ISOLATED_STOP_NS", "should-be-ignored")
                .output()
                .expect("child");
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
            return;
        }
        if cfg!(feature = "isolated-stop-events") {
            assert_eq!(isolated_ns().as_deref(), Some("should-be-ignored"));
        } else {
            assert_eq!(isolated_ns(), None, "a default build read GHOST_ISOLATED_STOP_NS");
        }
    }

    fn round_trip() {
        // The shared event every running Ghost watches must be left alone: the
        // tests use their own per-process event.
        assert_ne!(stop_event_name(), STOP_EVENT_NAME);
        let shared = unsafe { CreateEventW(None, true, false, &HSTRING::from(STOP_EVENT_NAME)) }.expect("shared");
        let shared_raised = || unsafe { WaitForSingleObject(shared, 0) == WAIT_OBJECT_0 };
        let shared_before = shared_raised();

        reset_stop();
        assert!(!is_stopped());

        trigger_stop();
        assert!(is_stopped());
        assert_eq!(shared_raised(), shared_before, "trigger_stop in a test raised the shared event");
        // The broadcast side: the named event must now be signaled, because that is
        // what other ghost processes actually observe.
        let ev = open_stop_event().expect("event");
        unsafe {
            assert_eq!(
                WaitForSingleObject(ev.0, 0),
                WAIT_OBJECT_0,
                "trigger_stop must raise this process's stop event"
            );
        }

        reset_stop();
        assert!(!is_stopped());
        unsafe {
            assert_ne!(
                WaitForSingleObject(ev.0, 0),
                WAIT_OBJECT_0,
                "reset_stop must lower this process's stop event"
            );
            let _ = CloseHandle(shared);
        }
    }

    #[test]
    fn registration_is_idempotent() {
        register_emergency_stop().expect("first");
        register_emergency_stop().expect("second call must be a no-op, not an error");
    }
}
