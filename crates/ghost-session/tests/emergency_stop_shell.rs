//! The emergency stop ends everything a shell command started, not just the
//! shell: a command that launched a program (a script driving the desktop, a
//! `ping` here) must not keep running after the stop.
//!
//! Its own test binary: it raises the process-local `STOP_FLAG` (never the
//! shared event, which would stop every Ghost on the machine), and nothing
//! else in this process may run while it is up. The only program it starts is
//! `cmd /C ping`, on no window, and it ends that program itself if the stop
//! did not.

#![cfg(windows)]

use ghost_core::input::STOP_FLAG;
use ghost_session::{GhostError, GhostSession};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

/// (pid, parent pid, exe name) of every process.
fn processes() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// A `ping` whose parent's parent is this test process: the grandchild that
/// `cmd /C ping ...` started.
fn our_grandchild_ping() -> Option<u32> {
    let me = std::process::id();
    let all = processes();
    let parent_of = |pid: u32| all.iter().find(|p| p.0 == pid).map(|p| p.1);
    all.iter()
        .find(|p| p.2.eq_ignore_ascii_case("PING.EXE") && parent_of(p.1) == Some(me))
        .map(|p| p.0)
}

fn alive(pid: u32) -> bool {
    processes().iter().any(|p| p.0 == pid)
}

#[tokio::test]
async fn a_stop_ends_the_programs_a_shell_command_started() {
    if !ghost_session::shell::shell_enabled() {
        eprintln!("SKIP: GHOST_SHELL is off in this environment");
        return;
    }
    let s = std::sync::Arc::new(GhostSession::new().expect("session"));
    let run = {
        let s = s.clone();
        tokio::spawn(async move {
            s.shell(&json!({"op": "run", "shell": "cmd", "cmd": "ping -n 37 127.0.0.1 >nul", "timeout_ms": 60000}))
                .await
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let ping = loop {
        if let Some(pid) = our_grandchild_ping() {
            break pid;
        }
        assert!(Instant::now() < deadline, "the shell never started ping");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    STOP_FLAG.store(true, Ordering::Release);
    let r = tokio::time::timeout(Duration::from_secs(10), run).await.expect("the shell command ended").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(ping) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let survived = alive(ping);
    STOP_FLAG.store(false, Ordering::Release);
    if survived {
        let _ = ghost_core::process::kill(ping);
    }

    assert!(matches!(r, Err(GhostError::Stopped)), "the command was not stopped: {r:?}");
    assert!(!survived, "ping (pid {ping}), started by the stopped shell command, kept running");
}
