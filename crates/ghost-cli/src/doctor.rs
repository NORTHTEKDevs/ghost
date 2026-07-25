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
pub fn evaluate_windows_build(build: u32) -> Status {
    if build >= 19041 { Status::Pass } else { Status::Fail }
}

/// A DPI scale other than 100% is fine, but only when the process is DPI-aware.
/// If it is not, every screen coordinate is silently scaled and clicks land
/// off-target - the classic "works on my machine" bug.
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

#[cfg(not(windows))]
pub fn run_checks() -> Vec<Check> {
    vec![Check::new("platform", Status::Fail, "Ghost's engine is Windows-only")]
}

#[cfg(test)]
mod tests {
    use super::*;

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
