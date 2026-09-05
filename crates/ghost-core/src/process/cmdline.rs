//! Read another process's command line, and from it the Chromium DevTools port.
//!
//! Why: agents drive the user's own browser (Comet, Chrome) far more than any
//! browser Ghost launches. When that browser was started with
//! `--remote-debugging-port=N`, CDP is the best background path there is - real
//! input events into a specific renderer, DOM names instead of a sparse UIA tree,
//! full modifier support - and the window it belongs to can be recognised by
//! reading the process's command line. The orphan sweep reads command lines for
//! the same reason: a Ghost-launched browser carries its profile root on its.
//!
//! How: `NtQueryInformationProcess(ProcessCommandLineInformation)`, the query
//! class Windows has offered since 8.1 for exactly this. It needs only
//! `PROCESS_QUERY_LIMITED_INFORMATION` and returns the string in one call. The
//! previous implementation walked the target's PEB with `ReadProcessMemory`,
//! which needed `PROCESS_VM_READ`, only worked for 64-bit targets, and put the
//! process-memory-reading API triad of `OpenProcess`, `NtQueryInformationProcess`
//! and `ReadProcessMemory` in the import table - a pattern antivirus heuristics
//! weight heavily, for good reason, and one Ghost never needed.

use std::path::Path;
use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::{CloseHandle, STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// `ProcessCommandLineInformation`: returns a `UNICODE_STRING` followed by its
/// characters, all inside the caller's buffer.
const PROCESS_COMMAND_LINE_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(60);
/// Refuse absurd lengths - a hostile or corrupt size must not become a huge alloc.
const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;

/// The full command line of `pid`, or `None` when the process is gone, is not
/// ours to query, or reports nothing.
pub fn command_line(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let result = (|| {
            // First call: how big is it? The kernel answers with
            // STATUS_INFO_LENGTH_MISMATCH and the required size.
            let mut needed = 0u32;
            let status = NtQueryInformationProcess(
                process,
                PROCESS_COMMAND_LINE_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
            if status != STATUS_INFO_LENGTH_MISMATCH || needed == 0 {
                return None;
            }
            let needed = needed as usize;
            if needed > MAX_COMMAND_LINE_BYTES {
                return None;
            }
            // 8-byte aligned storage: the UNICODE_STRING at the front holds a pointer.
            let words = needed.div_ceil(8);
            let mut buf = vec![0u64; words];
            let mut returned = 0u32;
            let status = NtQueryInformationProcess(
                process,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                needed as u32,
                &mut returned,
            );
            if status.is_err() {
                return None;
            }
            let us = &*(buf.as_ptr() as *const UNICODE_STRING);
            let chars = us.Length as usize / 2;
            if us.Buffer.is_null() || chars == 0 {
                return None;
            }
            // The string must lie inside the buffer we own; anything else is a
            // malformed answer, not something to dereference.
            let base = buf.as_ptr() as usize;
            let end = base + words * 8;
            let start = us.Buffer.0 as usize;
            if start < base || start + chars * 2 > end {
                return None;
            }
            let wide = std::slice::from_raw_parts(us.Buffer.0 as *const u16, chars);
            Some(String::from_utf16_lossy(wide))
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
    /// command line, and it must contain the binary path.
    #[test]
    fn reads_our_own_command_line() {
        let line = command_line(std::process::id()).expect("own command line");
        assert!(line.to_lowercase().contains("ghost"), "{line}");
    }

    /// The real reader against ANOTHER process, with a marker only that process
    /// carries: this is what the orphan sweep and the CDP router depend on.
    #[test]
    fn reads_another_process_command_line_exactly() {
        let marker = format!("ghost-cmdline-probe-{}", std::process::id());
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", &format!("echo {marker}>nul & ping -n 3 127.0.0.1 >nul")])
            .spawn()
            .expect("spawn cmd.exe");
        let line = command_line(child.id()).expect("child command line");
        let _ = child.kill();
        let _ = child.wait();
        assert!(line.contains(&marker), "marker missing from {line:?}");
        assert!(line.to_lowercase().contains("cmd.exe"), "{line}");
    }

    /// A pid nobody has is a clean None, never a panic or a garbage string.
    #[test]
    fn a_dead_pid_reads_as_none() {
        assert_eq!(command_line(u32::MAX - 7), None);
    }
}
