//! Read another process's command line, and from it the Chromium DevTools port.
//!
//! Why: agents drive the user's own browser (Comet, Chrome) far more than any
//! browser Ghost launches. When that browser was started with
//! `--remote-debugging-port=N`, CDP is the best background path there is - real
//! input events into a specific renderer, DOM names instead of a sparse UIA tree,
//! full modifier support - and the window it belongs to can be recognised by
//! reading the process's command line. Windows only; the command line lives in
//! the PEB of the target process and is read with `NtQueryInformationProcess` +
//! `ReadProcessMemory`, which works for any process of the same user.

use std::path::Path;
use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

/// `ProcessBasicInformation` - the class that returns the PEB address.
const PROCESS_BASIC_INFORMATION_CLASS: PROCESSINFOCLASS = PROCESSINFOCLASS(0);
/// x64 PEB: `ProcessParameters` pointer offset.
const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x20;
/// x64 RTL_USER_PROCESS_PARAMETERS: `CommandLine` (UNICODE_STRING) offset.
const PARAMS_COMMAND_LINE_OFFSET: usize = 0x70;
/// Refuse to read absurd lengths - a garbage pointer must not become a huge alloc.
const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ProcessBasicInformation {
    exit_status: i32,
    _pad: i32,
    peb_base_address: usize,
    affinity_mask: usize,
    base_priority: i32,
    _pad2: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

unsafe fn read_at<T: Copy + Default>(process: windows::Win32::Foundation::HANDLE, addr: usize) -> Option<T> {
    let mut out = T::default();
    ReadProcessMemory(
        process,
        addr as *const core::ffi::c_void,
        &mut out as *mut T as *mut core::ffi::c_void,
        std::mem::size_of::<T>(),
        None,
    )
    .ok()?;
    Some(out)
}

/// The full command line of `pid`, or `None` when the process is gone, is not
/// ours to read, or has no readable PEB (32-bit processes seen from a 64-bit
/// reader are not attempted).
pub fn command_line(pid: u32) -> Option<String> {
    if !cfg!(target_pointer_width = "64") {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let result = (|| {
            let mut info = ProcessBasicInformation::default();
            let mut len = 0u32;
            let status = NtQueryInformationProcess(
                process,
                PROCESS_BASIC_INFORMATION_CLASS,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut len,
            );
            if status.is_err() || info.peb_base_address == 0 {
                return None;
            }
            let params: usize = read_at(process, info.peb_base_address + PEB_PROCESS_PARAMETERS_OFFSET)?;
            if params == 0 {
                return None;
            }
            // UNICODE_STRING { Length: u16, MaximumLength: u16, Buffer: *u16 }
            let length: u16 = read_at(process, params + PARAMS_COMMAND_LINE_OFFSET)?;
            let buffer: usize = read_at(process, params + PARAMS_COMMAND_LINE_OFFSET + 8)?;
            let bytes = length as usize;
            if buffer == 0 || bytes == 0 || bytes > MAX_COMMAND_LINE_BYTES {
                return None;
            }
            let mut wide = vec![0u16; bytes / 2];
            ReadProcessMemory(
                process,
                buffer as *const core::ffi::c_void,
                wide.as_mut_ptr() as *mut core::ffi::c_void,
                bytes,
                None,
            )
            .ok()?;
            Some(String::from_utf16_lossy(&wide))
        })();
        let _ = CloseHandle(process);
        result
    }
}

/// What a Chromium command line says about remote debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugHint {
    /// The explicit `--remote-debugging-port` value (0 = "pick one and write it
    /// to DevToolsActivePort").
    pub port: Option<u16>,
    /// `--user-data-dir`, where `DevToolsActivePort` would be written.
    pub user_data_dir: Option<String>,
}

/// Parse the two switches that matter out of a command line. Handles quoted and
/// unquoted values; pure, so it is unit-tested without a process.
pub fn debug_hint_from_command_line(cmdline: &str) -> DebugHint {
    fn value_of(cmdline: &str, switch: &str) -> Option<String> {
        let start = cmdline.find(switch)? + switch.len();
        let rest = &cmdline[start..];
        let rest = rest.strip_prefix('=')?;
        if let Some(inner) = rest.strip_prefix('"') {
            return Some(inner.split('"').next().unwrap_or("").to_string());
        }
        Some(rest.split_whitespace().next().unwrap_or("").trim_end_matches('"').to_string())
    }
    DebugHint {
        port: value_of(cmdline, "--remote-debugging-port").and_then(|v| v.parse::<u16>().ok()),
        user_data_dir: value_of(cmdline, "--user-data-dir").filter(|s| !s.is_empty()),
    }
}

/// The port a hint resolves to: the explicit one, or the one Chromium wrote to
/// `DevToolsActivePort` when the switch was `=0` (or absent but remote
/// debugging is enabled some other way).
pub fn resolve_debug_port(hint: &DebugHint) -> Option<u16> {
    match hint.port {
        Some(p) if p != 0 => return Some(p),
        _ => {}
    }
    let dir = hint.user_data_dir.as_deref()?;
    let contents = std::fs::read_to_string(Path::new(dir).join("DevToolsActivePort")).ok()?;
    contents.lines().next()?.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

/// The DevTools port of `pid`, if its command line says it has one.
pub fn debug_port(pid: u32) -> Option<u16> {
    let cmdline = command_line(pid)?;
    let hint = debug_hint_from_command_line(&cmdline);
    if hint.port.is_none() && hint.user_data_dir.is_none() {
        return None;
    }
    resolve_debug_port(&hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_port_and_quoted_user_data_dir() {
        let h = debug_hint_from_command_line(
            r#""C:\Program Files\Perplexity\Comet\Application\comet.exe" --remote-debugging-port=9335 --user-data-dir="C:\Users\k\AppData\Local\Comet Profile" --flag"#,
        );
        assert_eq!(h.port, Some(9335));
        assert_eq!(h.user_data_dir.as_deref(), Some(r"C:\Users\k\AppData\Local\Comet Profile"));
        assert_eq!(resolve_debug_port(&h), Some(9335));
    }

    #[test]
    fn port_zero_needs_the_active_port_file() {
        let dir = std::env::temp_dir().join(format!("ghost-cmdline-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let h = debug_hint_from_command_line(&format!(
            "chrome.exe --remote-debugging-port=0 --user-data-dir={}",
            dir.display()
        ));
        assert_eq!(h.port, Some(0));
        assert_eq!(resolve_debug_port(&h), None, "no file yet");
        std::fs::write(dir.join("DevToolsActivePort"), "41241\n/devtools/browser/abc\n").unwrap();
        assert_eq!(resolve_debug_port(&h), Some(41241));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_switches_means_no_port() {
        let h = debug_hint_from_command_line(r#""C:\x\chrome.exe" --no-first-run https://example.com"#);
        assert_eq!(h, DebugHint { port: None, user_data_dir: None });
        assert_eq!(resolve_debug_port(&h), None);
    }

    /// The real reader against this very process: cargo's test binary has a
    /// command line, and it must contain the test filter we are running under
    /// or at least the binary path.
    #[test]
    fn reads_our_own_command_line() {
        let line = command_line(std::process::id()).expect("own command line");
        assert!(line.to_lowercase().contains("ghost"), "{line}");
    }
}
