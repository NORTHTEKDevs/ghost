use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS, PROCESSENTRY32W,
};
use windows::Win32::System::Threading::{
    CreateProcessW, TerminateProcess, OpenProcess, PROCESS_CREATION_FLAGS, PROCESS_TERMINATE,
    STARTUPINFOW, PROCESS_INFORMATION,
};
use windows::Win32::Foundation::CloseHandle;
use crate::error::CoreError;

/// Find the PID of a running process by executable name (case-insensitive).
/// Returns None if not found.
pub fn find_pid_by_name(name: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let null_pos = entry.szExeFile.iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let proc_name = String::from_utf16_lossy(&entry.szExeFile[..null_pos]);
                if proc_name.eq_ignore_ascii_case(name) {
                    let _ = CloseHandle(snapshot);
                    return Some(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        None
    }
}

/// Look `exe` up in the Windows "App Paths" registry, the mechanism the Run
/// dialog and ShellExecute use to find programs that are NOT on PATH.
///
/// This matters because `CreateProcessW` searches only PATH and the working
/// directory. Every major browser installs outside PATH - on a stock Windows 11
/// box `where msedge.exe` finds nothing, while
/// `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msedge.exe` holds
/// `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`. Without this
/// lookup, `launch("msedge.exe")` fails with FILE_NOT_FOUND (0x80070002) even
/// though Edge is installed, so browser automation is unusable by app name.
///
/// HKCU is checked before HKLM to honour a per-user install.
fn resolve_via_app_paths(exe: &str) -> Option<String> {
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    use windows::core::PCWSTR;
    use std::os::windows::ffi::OsStrExt;
    use std::ffi::OsStr;

    // A path (rather than a bare name) is never an App Paths key.
    if exe.contains('\\') || exe.contains('/') {
        return None;
    }
    // App Paths keys always carry the .exe suffix.
    let key_name = if exe.to_ascii_lowercase().ends_with(".exe") {
        exe.to_string()
    } else {
        format!("{exe}.exe")
    };

    let subkey: Vec<u16> = OsStr::new(&format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{key_name}"
    ))
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();

    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Some(v) = query_default_sz(root, &subkey) {
            return Some(v);
        }
    }
    return None;

    fn query_default_sz(root: HKEY, subkey: &[u16]) -> Option<String> {
        unsafe {
            let mut size: u32 = 0;
            // Size probe first: the value length is not known up front.
            RegGetValueW(
                root,
                PCWSTR(subkey.as_ptr()),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
            .ok()
            .ok()?;
            if size == 0 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            RegGetValueW(
                root,
                PCWSTR(subkey.as_ptr()),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..n]).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
    }
}

/// Wrap a path in quotes when it contains spaces, so `CreateProcessW` does not
/// split `C:\Program Files\...` into an executable plus arguments.
pub(crate) fn quote_if_needed(path: &str) -> String {
    if path.starts_with('"') || !path.contains(' ') {
        path.to_string()
    } else {
        format!("\"{path}\"")
    }
}

/// Launch a process by executable name or full path.
/// Returns the PID of the new process.
///
/// Falls back to an App Paths lookup when the direct call cannot find the
/// executable, so bare names like "msedge.exe" work as a user would expect.
pub fn launch(exe: &str) -> Result<u32, CoreError> {
    match launch_raw(exe) {
        Ok(pid) => Ok(pid),
        Err(direct_err) => match resolve_via_app_paths(exe) {
            Some(resolved) => {
                tracing::debug!(exe, resolved, "launch: resolved via App Paths");
                launch_raw(&quote_if_needed(&resolved))
            }
            // Report the original error: it describes what the caller asked for.
            None => Err(direct_err),
        },
    }
}

fn launch_raw(exe: &str) -> Result<u32, CoreError> {
    use std::os::windows::ffi::OsStrExt;
    use std::ffi::OsStr;

    unsafe {
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let mut cmd: Vec<u16> = OsStr::new(exe).encode_wide().chain(std::iter::once(0)).collect();

        CreateProcessW(
            None,
            windows::core::PWSTR(cmd.as_mut_ptr()),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        ).map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateProcessW" })?;
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        Ok(pi.dwProcessId)
    }
}

/// Kill a process by PID immediately.
pub fn kill(pid: u32) -> Result<(), CoreError> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OpenProcess" })?;
        TerminateProcess(handle, 1)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "TerminateProcess" })?;
        let _ = CloseHandle(handle);
        Ok(())
    }
}

/// Turn what an agent typed (`msedge.exe`, `notepad`, a full path with
/// arguments) into a command line `CreateProcessW` can start. A bare program
/// name that is not on PATH is resolved through the App Paths registry, which
/// is where every major browser registers itself. Anything with whitespace is
/// treated as an already-formed command line and passed through untouched.
pub fn resolve_command_line(command: &str) -> String {
    let c = command.trim();
    if c.is_empty() || c.chars().any(char::is_whitespace) || std::path::Path::new(c).is_absolute() {
        return c.to_string();
    }
    match resolve_via_app_paths(c) {
        Some(resolved) => quote_if_needed(&resolved),
        None => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_if_needed_wraps_paths_containing_spaces() {
        assert_eq!(
            quote_if_needed(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
            "\"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\""
        );
    }

    #[test]
    fn quote_if_needed_leaves_clean_paths_alone() {
        assert_eq!(quote_if_needed(r"C:\Windows\notepad.exe"), r"C:\Windows\notepad.exe");
        assert_eq!(quote_if_needed("notepad.exe"), "notepad.exe");
    }

    #[test]
    fn quote_if_needed_does_not_double_quote() {
        let already = "\"C:\\Program Files\\app.exe\"";
        assert_eq!(quote_if_needed(already), already);
    }

    #[test]
    fn app_paths_lookup_ignores_explicit_paths() {
        // A caller passing a real path must not be rerouted through the registry.
        assert!(resolve_via_app_paths(r"C:\Windows\System32\notepad.exe").is_none());
        assert!(resolve_via_app_paths("sub/dir/app.exe").is_none());
    }

    #[test]
    fn app_paths_lookup_returns_none_for_unknown_program() {
        assert!(resolve_via_app_paths("ghost-no-such-program-xyzzy.exe").is_none());
    }

    #[test]
    fn app_paths_lookup_finds_a_browser_when_one_is_installed() {
        // Edge ships with Windows 11 and is registered under App Paths but is
        // NOT on PATH - the exact case that made launch("msedge.exe") fail.
        match resolve_via_app_paths("msedge.exe") {
            Some(p) => {
                assert!(
                    p.to_ascii_lowercase().contains("msedge.exe"),
                    "resolved to an unexpected target: {p}"
                );
                assert!(
                    std::path::Path::new(p.trim_matches('"')).exists(),
                    "App Paths pointed at a file that does not exist: {p}"
                );
            }
            None => eprintln!("SKIP: Edge is not registered under App Paths on this machine"),
        }
    }
}
