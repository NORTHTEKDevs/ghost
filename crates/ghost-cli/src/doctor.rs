//! `ghost doctor` - preflight the machine before blaming the tool.
//!
//! Every check answers one question a confused user would otherwise open an
//! issue about. Checks are pure where possible so the formatting and the
//! pass/fail policy are unit-testable without a desktop.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    /// Something optional is missing. Ghost still works.
    Warn,
    /// Ghost cannot function. Exit code 1.
    Fail,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self { name, status, detail: detail.into() }
    }
}

/// Exit code policy: only a FAIL is fatal. A WARN must never block a user who
/// simply has not configured an optional feature.
pub fn exit_code(checks: &[Check]) -> u8 {
    if checks.iter().any(|c| c.status == Status::Fail) { 1 } else { 0 }
}

/// Windows 10 build 19041 (2004) is the floor: below it the UIA and DXGI
/// behaviour Ghost relies on differs enough that we will not claim support.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn evaluate_windows_build(build: u32) -> Status {
    if build >= 19041 { Status::Pass } else { Status::Fail }
}

/// A DPI scale other than 100% is fine, but only when the process is DPI-aware.
/// If it is not, every screen coordinate is silently scaled and clicks land
/// off-target - the classic "works on my machine" bug.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn evaluate_dpi(aware: bool, scale_percent: u32) -> Status {
    match (aware, scale_percent) {
        (true, _) => Status::Pass,
        (false, 100) => Status::Warn,
        (false, _) => Status::Fail,
    }
}

pub fn render(checks: &[Check]) -> String {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = String::from("ghost doctor\n\n");
    for c in checks {
        out.push_str(&format!(
            "  {:<width$}  {:<4}  {}\n",
            c.name,
            c.status.to_string(),
            c.detail,
            width = width
        ));
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    out.push('\n');
    if fails > 0 {
        out.push_str(&format!("{fails} check(s) FAILED - Ghost will not work correctly here.\n"));
    } else if warns > 0 {
        out.push_str(&format!("All required checks passed ({warns} optional warning(s)).\n"));
    } else {
        out.push_str("All checks passed.\n");
    }
    out
}

#[cfg(windows)]
pub fn run_checks() -> Vec<Check> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
    use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
    use windows::Win32::UI::HiDpi::{GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext, DPI_AWARENESS_UNAWARE};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};

    let mut checks = Vec::new();

    // --- Windows build ---
    let build = windows_build();
    checks.push(Check::new(
        "windows build",
        evaluate_windows_build(build),
        if build >= 19041 {
            format!("build {build}")
        } else {
            format!("build {build}; Ghost requires 19041 (Windows 10 2004) or newer")
        },
    ));

    // --- interactive desktop ---
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    let interactive = session != 0xFFFF_FFFF;
    checks.push(Check::new(
        "interactive desktop",
        if interactive { Status::Pass } else { Status::Fail },
        if interactive {
            format!("console session {session}")
        } else {
            "no interactive session; UIA returns empty trees in a service context".into()
        },
    ));

    // --- UIA available ---
    // COM must be initialised on this thread first; GhostSession does that in
    // its constructor, and doctor runs before any session exists.
    let _com = ghost_core::uia::init_com();
    let uia = ghost_core::uia::UiaTree::new();
    checks.push(match &uia {
        Ok(_) => Check::new("ui automation", Status::Pass, "IUIAutomation created"),
        Err(e) => Check::new("ui automation", Status::Fail, format!("cannot create IUIAutomation: {e}")),
    });

    // --- DPI awareness ---
    let dpi = unsafe {
        let hdc = GetDC(HWND(std::ptr::null_mut()));
        let d = if hdc.is_invalid() { 96 } else { GetDeviceCaps(hdc, LOGPIXELSX) };
        if !hdc.is_invalid() { ReleaseDC(HWND(std::ptr::null_mut()), hdc); }
        d
    };
    let scale = ((dpi as f32 / 96.0) * 100.0).round() as u32;
    let aware = unsafe {
        GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) != DPI_AWARENESS_UNAWARE
    };
    checks.push(Check::new(
        "dpi awareness",
        evaluate_dpi(aware, scale),
        if aware {
            format!("aware, display scale {scale}%")
        } else if scale == 100 {
            "not DPI-aware, but display is at 100% so coordinates still line up".into()
        } else {
            format!("NOT DPI-aware at {scale}% scaling; screen coordinates will be wrong")
        },
    ));

    // --- monitors ---
    let monitors = unsafe { GetSystemMetrics(SM_CMONITORS) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    checks.push(Check::new(
        "monitors",
        Status::Pass,
        format!("{monitors} monitor(s), virtual desktop {vw}x{vh}"),
    ));

    // --- screen capture ---
    checks.push(match capture_probe() {
        Ok(n) if n > 0 => Check::new("screen capture", Status::Pass, format!("captured {n} bytes")),
        Ok(_) => Check::new("screen capture", Status::Fail, "capture returned an empty frame"),
        Err(e) => Check::new("screen capture", Status::Fail, format!("capture failed: {e}")),
    });

    // --- vision credentials (OPTIONAL) ---
    let vision = ["GHOST_VISION_API_KEY", "OPENAI_API_KEY", "NVIDIA_API_KEY", "ANTHROPIC_API_KEY"]
        .iter()
        .find(|k| std::env::var(k).map(|v| !v.trim().is_empty()).unwrap_or(false));
    checks.push(match vision {
        Some(k) => Check::new("vision (optional)", Status::Pass, format!("credential found in {k}")),
        None => Check::new(
            "vision (optional)",
            Status::Warn,
            "no vision key set; describe-by-description falls back to the accessibility tree. Everything else works.",
        ),
    });

    checks
}

#[cfg(windows)]
fn windows_build() -> u32 {
    // GetVersionEx lies for unmanifested apps; read the real build from the registry.
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    use windows::core::PCWSTR;
    use std::os::windows::ffi::OsStrExt;
    use std::ffi::OsStr;

    let key: Vec<u16> = OsStr::new(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .encode_wide().chain(std::iter::once(0)).collect();
    let val: Vec<u16> = OsStr::new("CurrentBuildNumber")
        .encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let mut size: u32 = 0;
        if RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(val.as_ptr()),
            RRF_RT_REG_SZ, None, None, Some(&mut size)).is_err() { return 0; }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        if RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(val.as_ptr()),
            RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), Some(&mut size)).is_err() { return 0; }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n]).trim().parse().unwrap_or(0)
    }
}

#[cfg(windows)]
fn capture_probe() -> Result<usize, String> {
    ghost_core::capture::capture_screen()
        .map(|b| b.len())
        .map_err(|e| e.to_string())
}

/// Interpreting how many accessible windows the desktop exposes.
///
/// Zero windows is **not** a failure. It is ambiguous, and the two readings are
/// indistinguishable from here: accessibility may be off so applications publish
/// nothing, or nothing may be open - entirely normal on a fresh session.
///
/// Reporting FAIL would tell a user with an empty desktop that their machine is
/// broken and send them to change a setting that is already correct. So zero
/// warns and explains both readings; only an unreachable bus is fatal.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn evaluate_a11y(window_count: usize) -> Status {
    if window_count > 0 {
        Status::Pass
    } else {
        Status::Warn
    }
}

#[cfg(target_os = "linux")]
pub fn run_checks() -> Vec<Check> {
    use crate::engine::session::{session_kind, SessionKind};

    let mut out = Vec::new();

    let kind = session_kind();
    out.push(Check::new(
        "session",
        match kind {
            SessionKind::X11 | SessionKind::Wayland => Status::Pass,
            SessionKind::Headless => Status::Warn,
        },
        match kind {
            SessionKind::X11 => "X11 - XTEST input and GetImage capture available".to_string(),
            SessionKind::Wayland => {
                "Wayland - input via RemoteDesktop portal, capture via Screenshot portal".to_string()
            }
            SessionKind::Headless => {
                "no display session detected (XDG_SESSION_TYPE unset). AT-SPI may still work; \
                 synthetic input needs GHOST_INPUT=uinput"
                    .to_string()
            }
        },
    ));

    // The accessibility bus. Failing here means every element operation fails.
    match crate::engine::a11y::tree::list_windows() {
        Ok(windows) => {
            let n = windows.len();
            out.push(Check::new(
                "at-spi bus",
                Status::Pass,
                format!("connected; {n} window(s) visible"),
            ));
            out.push(Check::new(
                "accessibility",
                evaluate_a11y(n),
                if n > 0 {
                    let sample: Vec<&str> =
                        windows.iter().take(3).map(|w| w.name.as_str()).collect();
                    format!("applications are exposing trees (e.g. {})", sample.join(", "))
                } else {
                    "no application is currently exposing a tree. That is expected if nothing \
                     is open. If applications ARE open, run: gsettings set \
                     org.gnome.desktop.interface toolkit-accessibility true -- then RESTART them"
                        .to_string()
                },
            ));
        }
        Err(e) => out.push(Check::new(
            "at-spi bus",
            Status::Fail,
            format!(
                "{e}. Install at-spi2-core and enable accessibility: \
                 gsettings set org.gnome.desktop.interface toolkit-accessibility true"
            ),
        )),
    }

    // Synthetic input is a fallback, not the primary path, so a failure here is
    // a warning: AT-SPI actions still drive most applications.
    //
    // On Wayland this deliberately does NOT construct the backend. Doing so
    // opens the RemoteDesktop portal, which raises a screen-share consent
    // dialog -- running diagnostics must never prompt the user for permissions,
    // and on a fresh machine that dialog is alarming and can block for minutes.
    // Report what would be selected instead; the real request happens on the
    // first synthetic input, where the user expects it.
    match kind {
        SessionKind::Wayland => out.push(Check::new(
            "input backend",
            Status::Pass,
            "portal (RemoteDesktop). Consent is requested once, on the first synthetic input,              and remembered afterwards. AT-SPI actions never prompt"
                .to_string(),
        )),
        _ => match crate::engine::input::backend() {
            Ok(b) => out.push(Check::new(
                "input backend",
                Status::Pass,
                format!("{} (override with GHOST_INPUT=x11|portal|uinput)", b.name()),
            )),
            Err(e) => out.push(Check::new(
                "input backend",
                Status::Warn,
                format!("{e}. AT-SPI actions still work; only coordinate input is affected"),
            )),
        },
    }

    // What this session can and cannot do, stated up front. The X11-only
    // features degrade honestly on Wayland, but a user should learn that here
    // rather than from a failed call mid-task.
    if kind == SessionKind::Wayland {
        out.push(Check::new(
            "wayland limits",
            Status::Warn,
            "window minimize/maximize/restore (EWMH), occluded-window capture (XComposite) and              the global Ctrl+Alt+G hotkey need X11 and report Unsupported here. Everything              else -- element discovery, actions, typing, clipboard, screenshots -- works.              XWayland apps get the X11 features too"
                .to_string(),
        ));
    }

    // The emergency stop is the one thing a user needs to know the state of
    // before letting an agent drive their desktop.
    out.push(match kind {
        SessionKind::X11 => {
            if crate::engine::input::hotkey_x11::is_registered() {
                Check::new(
                    "emergency stop",
                    Status::Pass,
                    "Ctrl+Alt+G is armed globally, and ghost_stop over MCP also works".to_string(),
                )
            } else {
                Check::new(
                    "emergency stop",
                    Status::Warn,
                    "Ctrl+Alt+G is not armed in this process (it is registered when a Ghost              session starts, and another application may hold the combination).              ghost_stop over MCP still works"
                        .to_string(),
                )
            }
        }
        _ => Check::new(
            "emergency stop",
            Status::Warn,
            "ghost_stop over MCP. Wayland does not let a client grab keys globally, so there              is no Ctrl+Alt+G here -- that is a security property of the display server,              not a Ghost limitation"
                .to_string(),
        ),
    });

    // Same reasoning as the input backend: on Wayland a capture goes through
    // the Screenshot portal, which prompts. Diagnostics must not ask the user
    // for permissions, so report the path instead of exercising it.
    match kind {
        SessionKind::Wayland => out.push(Check::new(
            "capture",
            Status::Pass,
            "Screenshot portal. Needs xdg-desktop-portal and xdg-desktop-portal-gnome;              consent is requested on first use"
                .to_string(),
        )),
        _ => match capture_probe() {
            Ok(bytes) => out.push(Check::new("capture", Status::Pass, format!("{bytes} bytes"))),
            Err(e) => out.push(Check::new("capture", Status::Warn, e)),
        },
    }

    out
}

#[cfg(target_os = "linux")]
fn capture_probe() -> Result<usize, String> {
    crate::engine::capture::capture_screen().map(|b| b.len()).map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn run_checks() -> Vec<Check> {
    vec![Check::new(
        "platform",
        Status::Fail,
        "Ghost's engine supports Windows and Linux; macOS is not implemented",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_open_windows_warns_rather_than_fails() {
        // A fresh desktop with nothing open is not a broken installation, and
        // telling the user it is would send them to "fix" a correct setting.
        assert_eq!(evaluate_a11y(0), Status::Warn);
        assert_eq!(evaluate_a11y(1), Status::Pass);
    }

    #[test]
    fn only_a_fail_is_fatal() {
        // Guards the policy the check above depends on.
        let warn = vec![Check::new("x", Status::Warn, "")];
        let fail = vec![Check::new("x", Status::Fail, "")];
        assert_eq!(exit_code(&warn), 0);
        assert_eq!(exit_code(&fail), 1);
    }

    #[test]
    fn windows_build_floor_is_19041() {
        assert_eq!(evaluate_windows_build(19041), Status::Pass);
        assert_eq!(evaluate_windows_build(26100), Status::Pass);
        assert_eq!(evaluate_windows_build(19040), Status::Fail);
        assert_eq!(evaluate_windows_build(0), Status::Fail);
    }

    #[test]
    fn dpi_unaware_at_scaling_is_fatal_but_harmless_at_100_percent() {
        assert_eq!(evaluate_dpi(true, 150), Status::Pass);
        assert_eq!(evaluate_dpi(true, 100), Status::Pass);
        // The dangerous combination: not aware AND scaled -> every click is off.
        assert_eq!(evaluate_dpi(false, 150), Status::Fail);
        assert_eq!(evaluate_dpi(false, 100), Status::Warn);
    }

    #[test]
    fn only_fail_is_fatal() {
        let warn = vec![Check::new("x", Status::Warn, "optional thing missing")];
        assert_eq!(exit_code(&warn), 0, "a WARN must not block the user");

        let pass = vec![Check::new("x", Status::Pass, "")];
        assert_eq!(exit_code(&pass), 0);

        let fail = vec![Check::new("x", Status::Pass, ""), Check::new("y", Status::Fail, "")];
        assert_eq!(exit_code(&fail), 1);
    }

    #[test]
    fn render_lists_every_check_and_summarises() {
        let checks = vec![
            Check::new("alpha", Status::Pass, "fine"),
            Check::new("beta", Status::Warn, "optional"),
        ];
        let out = render(&checks);
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("PASS"));
        assert!(out.contains("WARN"));
        assert!(out.contains("1 optional warning"));
    }

    #[test]
    fn render_reports_failure_count() {
        let checks = vec![Check::new("alpha", Status::Fail, "broken")];
        assert!(render(&checks).contains("1 check(s) FAILED"));
    }
}
