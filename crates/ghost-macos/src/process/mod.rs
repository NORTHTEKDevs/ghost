//! Process launch.
//!
//! Unlike almost everything else in this crate, spawning a process has no
//! dependency on Accessibility/CGEvent/ScreenCaptureKit at all -- it is a
//! plain `fork`/`exec`, unprivileged on macOS exactly as on Linux, and
//! `std::process::Command` already does it without any extra crate. So this
//! is a REAL implementation, not a stub: `ghost_window op=launch` actually
//! starts a program on macOS today.
//!
//! One correctness detail a naive `Command::spawn()` misses: dropping the
//! returned `Child` without ever waiting on it leaves a zombie entry in the
//! process table until this (long-running) ghost process exits. A detached
//! background thread that calls `.wait()` and discards the result reaps it
//! without making the caller block on the child's lifetime -- the behavior
//! callers actually want (`launch` returns the PID immediately, same as the
//! Windows/Linux engines).

use crate::error::CoreError;

pub fn launch(exe: &str) -> Result<u32, CoreError> {
    let mut child = std::process::Command::new(exe).spawn().map_err(|e| {
        // No single existing variant names "process spawn failed" precisely;
        // `ProcessNotFound` is the closest shape and correct for the common
        // case (bad path/name -> ENOENT). The underlying OS error text is
        // folded into the message so a permission or resource failure is
        // still visible, not hidden behind a misleading label.
        CoreError::ProcessNotFound { name: format!("{exe}: {e}") }
    })?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_unknown_binary_fails_with_a_readable_message() {
        let err = launch("ghost-macos-definitely-not-a-real-binary-xyz").unwrap_err();
        assert!(err.to_string().contains("ghost-macos-definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn launch_a_real_process_returns_a_nonzero_pid() {
        // `true` ships on every macOS/Unix box and exits immediately.
        let pid = launch("true").expect("spawning /usr/bin/true must succeed");
        assert!(pid > 0);
    }
}
