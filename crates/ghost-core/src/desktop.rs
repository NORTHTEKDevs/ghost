//! Isolated desktops: full-fidelity automation of apps that expose no automation
//! surface at all.
//!
//! UIA patterns and window messages cover apps that cooperate. Some do not: games,
//! canvas-rendered UIs, custom-drawn controls, and anything that reads raw input
//! state rather than messages. Those apps only answer *real* input - and real input
//! on the user's desktop is exactly what ghost refuses to do.
//!
//! A window station can hold many desktops, each with its own window list, message
//! queues, and foreground window. Only one is displayed. Ghost creates a second
//! desktop and launches the target app onto it, so the app's windows never appear on
//! the user's screen at all - not even for the instant between launching and being
//! moved aside. This is the desktop-app equivalent of running a browser headless.
//!
//! What works on a non-displayed desktop, measured rather than assumed
//! (`examples/desktop_input_probe.rs`):
//!
//! | Mechanism | Works? |
//! |---|---|
//! | UI Automation patterns | yes, from a thread bound to the desktop |
//! | Window messages (click, type, scroll) | yes |
//! | `PrintWindow` capture | yes |
//! | `SendInput` real input | **no** - `ERROR_ACCESS_DENIED` |
//!
//! That last row is an OS boundary, not a missing feature. `SendInput` is refused from
//! any thread whose desktop is not the *input* desktop, and the input desktop is by
//! definition the one on screen. Making this desktop the input desktop needs
//! `SwitchDesktop`, which would display it - the opposite of the point. So apps that
//! read raw input state directly (`GetAsyncKeyState`, DirectInput, most games) cannot
//! be driven here, or anywhere, without taking over the real screen. For those, raise
//! the focus policy deliberately and accept the tradeoff.
//!
//! Other constraints:
//! - An app must be *launched* onto a desktop. Windows cannot move an existing window
//!   between desktops.
//! - Some GPU-accelerated apps render blank on a non-displayed desktop; the capture
//!   path reports that rather than returning a black image.

use crate::error::CoreError;
use crate::focus;
use crossbeam_channel::{unbounded, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, EnumDesktopWindows, SetThreadDesktop, DESKTOP_CONTROL_FLAGS,
    HDESK,
};
use windows::Win32::System::Threading::{
    CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};

/// `GENERIC_ALL` on the desktop object: ghost owns this desktop outright.
const DESKTOP_ALL_ACCESS: u32 = 0x1000_0000;

static DESKTOP_SEQ: AtomicU64 = AtomicU64::new(0);

type Job = Box<dyn FnOnce() + Send>;

/// A window living on an isolated desktop.
#[derive(Debug, Clone)]
pub struct DesktopWindow {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
}

/// An isolated desktop plus the worker thread bound to it.
///
/// Every operation runs on that worker, because desktop binding is a *thread*
/// property: input injected from an unbound thread would go to the user's desktop,
/// which is precisely the bug this type exists to prevent.
pub struct DesktopSession {
    name: String,
    jobs: Sender<Job>,
}

impl DesktopSession {
    /// Create a new isolated desktop and its worker thread.
    pub fn create(label: &str) -> Result<Self, CoreError> {
        let seq = DESKTOP_SEQ.fetch_add(1, Ordering::SeqCst);
        // Desktop names are per-window-station, so they must not collide with another
        // ghost process on the same login.
        let name = format!("ghost-{}-{}-{}", std::process::id(), seq, sanitize(label));
        let (tx, rx) = unbounded::<Job>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread_name = name.clone();

        std::thread::Builder::new()
            .name(format!("ghost-desktop-{name}"))
            .spawn(move || {
                // The desktop handle is created *on this thread* and never leaves it.
                // HDESK is not Send, and SetThreadDesktop only affects the caller.
                let hdesk = unsafe {
                    CreateDesktopW(
                        &HSTRING::from(thread_name.as_str()),
                        None,
                        None,
                        DESKTOP_CONTROL_FLAGS(0),
                        DESKTOP_ALL_ACCESS,
                        None,
                    )
                };
                let hdesk = match hdesk {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("CreateDesktopW: {e}")));
                        return;
                    }
                };
                // SetThreadDesktop fails if the thread already owns windows or hooks,
                // which is why this is a freshly spawned thread that has done nothing.
                if let Err(e) = unsafe { SetThreadDesktop(hdesk) } {
                    let _ = ready_tx.send(Err(format!("SetThreadDesktop: {e}")));
                    unsafe {
                        let _ = CloseDesktop(hdesk);
                    }
                    return;
                }
                // Exempt this thread from the focus policy: its input cannot reach the
                // user's desktop.
                focus::mark_isolated_desktop_thread();
                DESKTOP_HANDLE.with(|h| h.set(hdesk.0 as isize));

                let _ = ready_tx.send(Ok(()));
                while let Ok(job) = rx.recv() {
                    job();
                }
                unsafe {
                    let _ = CloseDesktop(hdesk);
                }
            })
            .map_err(|e| CoreError::Desktop(format!("spawn worker: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { name, jobs: tx }),
            Ok(Err(e)) => Err(CoreError::Desktop(e)),
            Err(_) => Err(CoreError::Desktop(
                "worker thread died during startup".into(),
            )),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Run `f` on the desktop-bound worker and return its result.
    pub fn exec<T, F>(&self, f: F) -> Result<T, CoreError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        self.jobs
            .send(Box::new(move || {
                let _ = tx.send(f());
            }))
            .map_err(|_| CoreError::Desktop("desktop worker is gone".into()))?;
        rx.recv()
            .map_err(|_| CoreError::Desktop("desktop worker dropped the job".into()))
    }

    /// Launch a process onto this desktop. Returns its PID.
    ///
    /// The desktop is chosen at creation time via `STARTUPINFO.lpDesktop`; there is no
    /// way to move a process or window onto a desktop afterwards, so anything to be
    /// automated here must be started here.
    pub fn launch(&self, command: &str) -> Result<u32, CoreError> {
        let cmd = command.to_string();
        let name = self.name.clone();
        self.exec(move || launch_on_desktop(&cmd, &name))?
    }

    /// Every visible top-level window on this desktop.
    pub fn windows(&self) -> Result<Vec<DesktopWindow>, CoreError> {
        self.exec(visible_windows)?
    }

    /// Every window on this desktop, including hidden and helper windows.
    pub fn all_windows(&self) -> Result<Vec<DesktopWindow>, CoreError> {
        self.exec(enum_desktop_windows)?
    }

    /// Wait for a window whose title contains `needle` to appear on this desktop.
    pub fn wait_for_window(
        &self,
        needle: &str,
        timeout_ms: u64,
    ) -> Result<DesktopWindow, CoreError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let want = needle.to_lowercase();
        loop {
            if let Ok(list) = self.windows() {
                if let Some(w) = list
                    .into_iter()
                    .find(|w| w.title.to_lowercase().contains(&want))
                {
                    return Ok(w);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(CoreError::Desktop(format!(
                    "no window matching '{needle}' appeared on desktop '{}' in {timeout_ms}ms",
                    self.name
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    /// Whether real hardware-level input (`SendInput`) can be injected here.
    ///
    /// Always false, and worth stating plainly rather than discovering at runtime:
    /// Windows returns `ERROR_ACCESS_DENIED` for `SendInput` from a thread bound to a
    /// desktop that is not the *input* desktop, and the input desktop is the one being
    /// displayed. Making this desktop the input desktop would need `SwitchDesktop`,
    /// which puts it on the user's screen - the opposite of the point.
    ///
    /// Measured, not assumed: see `examples/desktop_input_probe.rs`.
    pub fn real_input_supported(&self) -> bool {
        false
    }

    /// Left click at a point in a window's client area, via that window's message
    /// queue. This is the input path that works on a non-displayed desktop.
    pub fn click(&self, hwnd: isize, x: i32, y: i32) -> Result<(), CoreError> {
        self.exec(move || crate::input::BackgroundClicker::click(hwnd, (x, y)))?
    }

    pub fn right_click(&self, hwnd: isize, x: i32, y: i32) -> Result<(), CoreError> {
        self.exec(move || crate::input::BackgroundClicker::right_click_screen(hwnd, x, y))?
    }

    pub fn double_click(&self, hwnd: isize, x: i32, y: i32) -> Result<(), CoreError> {
        self.exec(move || crate::input::BackgroundClicker::double_click_screen(hwnd, x, y))?
    }

    pub fn hover(&self, hwnd: isize, x: i32, y: i32) -> Result<(), CoreError> {
        self.exec(move || crate::input::BackgroundClicker::hover_screen(hwnd, x, y))?
    }

    pub fn scroll(
        &self,
        hwnd: isize,
        _x: i32,
        _y: i32,
        notches: i32,
        horizontal: bool,
    ) -> Result<(), CoreError> {
        self.exec(move || scroll_window(hwnd, notches, horizontal))?
    }

    /// Read the text of the input target inside `hwnd`, via WM_GETTEXT.
    ///
    /// This is the read-back that makes typing on an invisible desktop provable:
    /// there is no screen to look at, so the control's own value is the evidence.
    pub fn read_text(&self, hwnd: isize) -> Result<String, CoreError> {
        self.exec(move || {
            let target = crate::input::BackgroundClicker::text_target(hwnd)
                .ok_or(CoreError::NoTextControl)?;
            crate::input::BackgroundClicker::read_text(target).ok_or(CoreError::NoTextControl)
        })?
    }

    /// Type into a window on this desktop, and prove it landed.
    ///
    /// Two things had to be fixed here, and both produced a silent `ok`:
    ///
    /// 1. Nothing on a desktop that is never displayed has ever held keyboard
    ///    focus, so resolving by focus alone fell through to the top-level frame,
    ///    which discards every WM_CHAR. Resolve a real text control instead.
    /// 2. `WM_CHAR` is *posted* (asynchronous) while `WM_GETTEXT` is *sent*, and a
    ///    sent message is serviced ahead of the queued ones - so an immediate
    ///    read-back saw only the characters that happened to be processed already.
    ///    Poll until the value stops changing rather than reading once.
    ///
    /// The read-back is the proof. If the value never changes, this errors instead
    /// of claiming success.
    pub fn type_text(&self, hwnd: isize, text: &str) -> Result<(), CoreError> {
        let t = text.to_string();
        self.exec(move || {
            let target = match crate::input::BackgroundClicker::text_target(hwnd) {
                Some(t) => t,
                // No message-postable control (WinUI/UWP apps like Windows 11
                // Notepad): fall back to the UIA ValuePattern, which needs no
                // keyboard input and so works on a non-displayed desktop.
                None => return type_text_via_uia(hwnd, &t),
            };
            let read = || crate::input::BackgroundClicker::read_text(target).unwrap_or_default();
            let before = read();
            for ch in t.chars() {
                crate::input::BackgroundClicker::send_char(target, ch)?;
            }
            if t.is_empty() {
                return Ok(());
            }
            // Let the posted characters drain: stop as soon as the expected text is
            // present, or when the value has held still across two reads.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
            let mut last = read();
            let mut stable = 0;
            while std::time::Instant::now() < deadline {
                if last.contains(&t) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
                let now = read();
                if now == last {
                    stable += 1;
                    if stable >= 3 {
                        break;
                    }
                } else {
                    stable = 0;
                    last = now;
                }
            }
            if last == before {
                // The control accepted no posted characters at all. Some apps
                // expose a message-postable-looking control whose thread never
                // pumps them on a non-displayed desktop - before erroring, try
                // the UIA ValuePattern, which bypasses the message queue.
                if type_text_via_uia(hwnd, &t).is_ok() {
                    return Ok(());
                }
                return Err(CoreError::TypeNotVerified { text: t });
            }
            if !last.contains(&t) {
                // Partial landing: same rescue, so a target that dropped the
                // tail gets a second chance rather than a false-negative.
                if type_text_via_uia(hwnd, &t).is_ok() {
                    return Ok(());
                }
                return Err(CoreError::TypePartial {
                    wanted: t,
                    got: last,
                });
            }
            Ok::<(), CoreError>(())
        })?
    }

    pub fn press(&self, hwnd: isize, key: &str) -> Result<(), CoreError> {
        let k = key.to_string();
        self.exec(move || match crate::input::keyboard::name_to_vk(&k) {
            Some(vk) => {
                let target = crate::input::BackgroundClicker::text_target(hwnd).unwrap_or(hwnd);
                crate::input::BackgroundClicker::send_key(target, vk.0)
            }
            None => Err(CoreError::Win32 {
                code: 0,
                context: "unknown key name",
            }),
        })?
    }

    /// Editing shortcut against a window on this desktop, using the standard control
    /// messages (WM_UNDO/WM_COPY/...) rather than faked modifier keys.
    pub fn shortcut(&self, hwnd: isize, name: &str) -> Result<(), CoreError> {
        let key = name.trim().to_lowercase();
        // Accept both "undo"/"copy" names and "ctrl+z" combos.
        let ctrl_key = key.strip_prefix("ctrl+").unwrap_or(&key);
        let cmd = crate::input::EditCommand::from_ctrl_key(ctrl_key)
            .or_else(|| match key.as_str() {
                "undo" => crate::input::EditCommand::from_ctrl_key("z"),
                "copy" => crate::input::EditCommand::from_ctrl_key("c"),
                "cut" => crate::input::EditCommand::from_ctrl_key("x"),
                "paste" => crate::input::EditCommand::from_ctrl_key("v"),
                "select_all" | "selectall" => crate::input::EditCommand::from_ctrl_key("a"),
                _ => None,
            })
            .ok_or(CoreError::Win32 {
                code: 0,
                context: "unsupported shortcut",
            })?;
        self.exec(move || {
            let target = crate::input::BackgroundClicker::text_target(hwnd)
                .ok_or(CoreError::NoTextControl)?;
            crate::input::BackgroundClicker::edit_command(target, cmd)
        })?
    }

    /// Run a closure with COM and a UIA tree bound to this desktop.
    ///
    /// UIA works here, which is what makes an isolated desktop genuinely useful: an
    /// app can be launched somewhere the user never sees and still be driven by
    /// control patterns rather than pixels. A UIA client only sees providers on its
    /// own thread's desktop, so this must run on the worker.
    pub fn with_uia<T, F>(&self, f: F) -> Result<T, CoreError>
    where
        T: Send + 'static,
        F: FnOnce(&crate::uia::tree::UiaTree) -> T + Send + 'static,
    {
        self.exec(move || {
            crate::uia::init_com()?;
            let tree = crate::uia::tree::UiaTree::new()?;
            Ok::<T, CoreError>(f(&tree))
        })?
    }

    /// PNG of one window on this desktop. `client_only` is accepted for API parity;
    /// PrintWindow captures the whole window.
    pub fn capture(&self, hwnd: isize, _client_only: bool) -> Result<Vec<u8>, CoreError> {
        self.exec(move || {
            let (rgba, w, h) = crate::capture::capture_window_printwindow(hwnd)?;
            crate::capture::screen::encode_png_rgba(&rgba, w as u32, h as u32)
        })?
    }

    /// Give a window on this desktop keyboard focus, so typing lands in it.
    ///
    /// Safe under any focus policy: "foreground" here means foreground *of the
    /// isolated desktop*, which is not displayed and has its own input queue.
    pub fn focus_window(&self, hwnd: isize) -> Result<(), CoreError> {
        self.exec(move || unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
            use windows::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_SHOW,
            };
            let h = HWND(hwnd as *mut core::ffi::c_void);
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
            let _ = SetActiveWindow(h);
            Ok::<(), CoreError>(())
        })?
    }
}

/// Type into a window that has no message-postable text control, via UIA.
///
/// Modern app frameworks (WinUI 3, UWP) expose their text fields only through
/// UI Automation - there is no classic EDIT control to post WM_CHAR to. The
/// ValuePattern sets the value directly, no keyboard involved, so it works on
/// a non-displayed desktop where SendInput is refused. The read-back is the
/// proof: SetValue reported success means nothing if the value did not change.
fn type_text_via_uia(hwnd: isize, text: &str) -> Result<(), CoreError> {
    crate::uia::init_com()?;
    let tree = crate::uia::tree::UiaTree::new()?;
    for role in ["edit", "document", "combobox"] {
        let candidates = tree.find_all_in_hwnd(hwnd, None, Some(role), 10)?;
        for el in candidates {
            if !el.is_enabled() || el.is_offscreen() {
                continue;
            }
            // Providers may normalise newlines on SetValue (WinUI stores \r\n);
            // compare folded so a successful set is never misread as a failure.
            let fold = |s: &str| s.replace("\r\n", "\n");
            if crate::uia::patterns::set_value_ex(&el, text, false).is_ok()
                && fold(&el.get_text()).contains(&fold(text))
            {
                return Ok(());
            }
        }
    }
    Err(CoreError::NoTextControl)
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        // Dropping the sender ends the worker's recv loop, which closes the desktop.
        // Processes still running on it are orphaned onto a desktop nobody is bound
        // to, so callers should terminate them first.
    }
}

thread_local! {
    static DESKTOP_HANDLE: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

fn sanitize(label: &str) -> String {
    let s: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if s.is_empty() {
        "session".into()
    } else {
        s
    }
}

fn launch_on_desktop(command: &str, desktop: &str) -> Result<u32, CoreError> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    unsafe {
        let mut desktop_w: Vec<u16> = OsStr::new(desktop)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop_w.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let mut cmd: Vec<u16> = OsStr::new(command)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        CreateProcessW(
            None,
            PWSTR(cmd.as_mut_ptr()),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        )
        .map_err(|e| CoreError::Desktop(format!("CreateProcessW on desktop '{desktop}': {e}")))?;
        let pid = pi.dwProcessId;
        let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
        Ok(pid)
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut title = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut title);
    if len > 0 {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let list = &mut *(lparam.0 as *mut Vec<DesktopWindow>);
        list.push(DesktopWindow {
            hwnd: hwnd.0 as isize,
            title: String::from_utf16_lossy(&title[..len as usize]),
            pid,
        });
    }
    TRUE
}

/// Visible top-level windows on the desktop the calling thread is bound to.
///
/// `EnumDesktopWindows` returns every window including message-only and helper
/// windows (an app typically has several with names like "GDI+ Window"), so matching
/// a title against the raw list picks the wrong one.
pub fn visible_windows() -> Result<Vec<DesktopWindow>, CoreError> {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    Ok(enum_desktop_windows()?
        .into_iter()
        .filter(|w| unsafe { IsWindowVisible(HWND(w.hwnd as *mut core::ffi::c_void)).as_bool() })
        .collect())
}

fn enum_desktop_windows() -> Result<Vec<DesktopWindow>, CoreError> {
    let mut list: Vec<DesktopWindow> = Vec::new();
    let hdesk = HDESK(DESKTOP_HANDLE.with(|h| h.get()) as *mut core::ffi::c_void);
    unsafe {
        // Errors here are expected and benign: EnumDesktopWindows reports failure if
        // any window vanishes mid-enumeration, but the windows collected so far are
        // still valid.
        let _ = EnumDesktopWindows(
            hdesk,
            Some(enum_proc),
            LPARAM(&mut list as *mut Vec<DesktopWindow> as isize),
        );
    }
    Ok(list)
}

fn as_hwnd(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

/// Background wheel scroll via a posted WM_MOUSEWHEEL to the window.
fn scroll_window(hwnd: isize, notches: i32, horizontal: bool) -> Result<(), CoreError> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW};
    const WM_MOUSEWHEEL: u32 = 0x020A;
    const WM_MOUSEHWHEEL: u32 = 0x020E;
    const WHEEL_DELTA: i32 = 120;
    let h = as_hwnd(hwnd);
    unsafe {
        if !IsWindow(h).as_bool() {
            return Err(CoreError::WindowGone);
        }
        let delta = WHEEL_DELTA * notches;
        let wparam = WPARAM(((delta as u32 as usize) << 16) & 0xFFFF_0000);
        let msg = if horizontal {
            WM_MOUSEHWHEEL
        } else {
            WM_MOUSEWHEEL
        };
        PostMessageW(h, msg, wparam, LPARAM(0)).map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: "PostMessage wheel",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_names_are_sanitized_and_bounded() {
        assert_eq!(sanitize("my app"), "myapp");
        assert_eq!(sanitize(r"a\b/c:d"), "abcd");
        assert_eq!(sanitize(""), "session");
        assert_eq!(sanitize("!!!"), "session");
        assert!(sanitize(&"x".repeat(200)).len() <= 24);
    }

    #[test]
    fn creating_a_desktop_binds_a_worker_and_exempts_it_from_the_focus_policy() {
        crate::focus::set_policy(crate::focus::FocusPolicy::Background);
        let d = DesktopSession::create("unit").expect("create desktop");
        assert!(d.name().starts_with("ghost-"));
        assert!(d.name().ends_with("unit"), "{}", d.name());

        // The calling thread is still bound by the policy...
        assert!(!crate::focus::on_isolated_desktop());
        assert!(crate::input::mouse::click(5, 5).is_err());

        // ...but the desktop worker is not, because its input cannot reach the user.
        let exempt = d.exec(|| crate::focus::on_isolated_desktop()).unwrap();
        assert!(exempt, "worker thread must be marked isolated");
    }

    #[test]
    fn a_fresh_desktop_has_no_application_windows() {
        let d = DesktopSession::create("empty").expect("create desktop");
        let windows = d.windows().expect("enumerate");
        // A brand new desktop has no apps on it. Anything here would mean we
        // enumerated the user's desktop instead, which would be a serious bug.
        assert!(
            windows.is_empty(),
            "isolated desktop should start empty, saw: {:?}",
            windows.iter().map(|w| &w.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_desktops_get_distinct_names() {
        let a = DesktopSession::create("x").unwrap();
        let b = DesktopSession::create("x").unwrap();
        assert_ne!(a.name(), b.name());
    }

    #[test]
    fn exec_propagates_values_and_runs_off_the_calling_thread() {
        let d = DesktopSession::create("exec").unwrap();
        assert_eq!(d.exec(|| 6 * 7).unwrap(), 42);
        let worker = d.exec(|| std::thread::current().id()).unwrap();
        assert_ne!(worker, std::thread::current().id());
    }
}
