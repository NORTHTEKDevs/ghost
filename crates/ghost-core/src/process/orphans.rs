//! Find and end processes that a Ghost server launched and then abandoned:
//! the server died (taskkill, a crash, a killed terminal) before it could close
//! them. Every browser Ghost launches carries its profile directory under
//! `ghost-browser-profiles` on its command line, and its parent is the server
//! that launched it. A marker match whose parent is gone - or whose parent pid
//! now belongs to a younger process (pid reuse) - is an orphan nobody can ever
//! talk to again.
//!
//! Why: on 2026-09-01 two headless Chromes from servers that had exited days
//! earlier were still running (32 processes between them), invisible, holding
//! memory on a machine that is already slow. The job object in ghost-browser
//! stops new ones from outliving their server; this sweep clears the ones
//! already there, and anything a failed job assignment lets through.

use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// One row of the process table.
#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

/// The current process table (pid, parent pid, image name).
pub fn snapshot() -> Vec<ProcessEntry> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                out.push(ProcessEntry {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    name: String::from_utf16_lossy(&entry.szExeFile[..end]),
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// When `pid` started, as FILETIME ticks (100 ns since 1601). `None` when the
/// process is gone or is not ours to query.
pub fn creation_time(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let r = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
    }
}

/// The rule, kept pure so it is tested without processes: a child is orphaned
/// when its parent is gone, or when the process now holding the parent's pid
/// started AFTER the child did (the pid was reused by something unrelated).
pub fn is_orphan(child_created: Option<u64>, parent_created: Option<u64>) -> bool {
    match (child_created, parent_created) {
        (_, None) => true,
        (Some(child), Some(parent)) => parent > child,
        (None, Some(_)) => false,
    }
}

/// A process the sweep decided nobody owns any more.
#[derive(Debug, Clone)]
pub struct Orphan {
    pub pid: u32,
    pub name: String,
    pub parent_pid: u32,
    pub command_line: String,
}

/// Top-level processes (no `--type=`, so not a renderer or helper) named in
/// `names` whose command line contains `marker` (case-insensitive) and whose
/// parent is gone or reused. Anything this process launched is never an
/// orphan, and a live parent we cannot query is treated as an owner.
pub fn find_orphans(marker: &str, names: &[&str]) -> Vec<Orphan> {
    let procs = snapshot();
    let me = std::process::id();
    let marker = marker.to_lowercase();
    let mut out = Vec::new();
    for p in &procs {
        if p.pid == me || p.parent_pid == me || !names.iter().any(|n| p.name.eq_ignore_ascii_case(n)) {
            continue;
        }
        let Some(cmd) = super::cmdline::command_line(p.pid) else { continue };
        let lc = cmd.to_lowercase();
        if !lc.contains(&marker) || lc.contains("--type=") {
            continue;
        }
        let parent_alive = procs.iter().any(|q| q.pid == p.parent_pid);
        let parent_created = if parent_alive { creation_time(p.parent_pid) } else { None };
        if parent_alive && parent_created.is_none() {
            continue;
        }
        if is_orphan(creation_time(p.pid), parent_created) {
            out.push(Orphan { pid: p.pid, name: p.name.clone(), parent_pid: p.parent_pid, command_line: cmd });
        }
    }
    out
}

/// End every orphan and each descendant still listed under it. Returns the
/// orphans that were targeted (a kill that fails is not retried).
pub fn kill_orphans(marker: &str, names: &[&str]) -> Vec<Orphan> {
    let orphans = find_orphans(marker, names);
    if orphans.is_empty() {
        return orphans;
    }
    let procs = snapshot();
    let mut doomed: Vec<u32> = orphans.iter().map(|o| o.pid).collect();
    let mut i = 0;
    while i < doomed.len() {
        let parent = doomed[i];
        for q in &procs {
            if q.parent_pid == parent && !doomed.contains(&q.pid) {
                doomed.push(q.pid);
            }
        }
        i += 1;
    }
    // Children first, so a browser process does not get to notice its
    // renderers dying and respawn them while the sweep is running.
    for pid in doomed.iter().rev() {
        let _ = super::kill(*pid);
    }
    orphans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    #[test]
    fn orphan_rule() {
        assert!(is_orphan(Some(10), None), "parent gone");
        assert!(is_orphan(None, None), "both unknown: gone");
        assert!(is_orphan(Some(10), Some(20)), "parent pid reused by a younger process");
        assert!(!is_orphan(Some(20), Some(10)), "parent older than child: owned");
        assert!(!is_orphan(None, Some(10)), "unreadable child under a live parent: owned");
    }

    #[test]
    fn the_snapshot_lists_this_process_under_its_parent() {
        let me = std::process::id();
        let procs = snapshot();
        let mine = procs.iter().find(|p| p.pid == me).expect("this process is in the table");
        assert!(mine.parent_pid != 0);
        assert!(creation_time(me).is_some());
    }

    /// A marked process whose launcher has exited is found, ended with its
    /// descendants, and an identically marked process under a LIVE launcher
    /// (this test) is left alone.
    #[test]
    fn sweep_ends_an_abandoned_marked_process_and_spares_an_owned_one() {
        let marker = format!("ghost-orphan-probe-{}", std::process::id());
        // The marker rides in an `echo`, never a `rem`: cmd's `rem` swallows the
        // rest of the line, `&` included, and the shell would exit at once.
        // Owned: our own child carrying the marker.
        let mut owned = std::process::Command::new("cmd.exe")
            .raw_arg(format!("/c \"echo {marker}-owned>nul & ping -n 30 127.0.0.1 >nul\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("owned child");
        // Abandoned: a launcher that starts a marked cmd.exe with `start /b`
        // (a plain CreateProcess, so the launcher is its parent) and exits.
        let status = std::process::Command::new("cmd.exe")
            .raw_arg(format!(
                "/c start \"\" /b cmd.exe /c \"echo {marker}-abandoned>nul & ping -n 30 127.0.0.1 >nul\""
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .expect("launcher");
        assert!(status.success(), "launcher failed: {status}");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = Vec::new();
        while Instant::now() < deadline {
            found = find_orphans(&marker, &["cmd.exe"]);
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(found.len(), 1, "exactly the abandoned one: {found:?}");
        assert!(found[0].command_line.contains("-abandoned"), "{found:?}");
        let abandoned = found[0].pid;

        let killed = kill_orphans(&marker, &["cmd.exe"]);
        assert_eq!(killed.len(), 1);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && snapshot().iter().any(|p| p.pid == abandoned) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!snapshot().iter().any(|p| p.pid == abandoned), "orphan still running");
        assert!(
            owned.try_wait().expect("try_wait").is_none(),
            "the sweep ended a process that a live launcher still owns"
        );
        let _ = owned.kill();
        // The ping under the owned cmd is not ours to leave behind either.
        for p in snapshot().iter().filter(|p| p.parent_pid == owned.id()) {
            let _ = super::super::kill(p.pid);
        }
    }
}
