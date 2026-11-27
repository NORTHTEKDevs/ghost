//! `ghost reset` lifts even a stop the human raised, so only a human at an
//! interactive terminal may run it: an agent's own shell drives commands over
//! pipes. Run here with stdin piped (with the "right" answer in it) - it must
//! refuse before it touches any stop event, so this test never lowers or raises
//! the real shared event.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn ghost_reset_refuses_when_stdin_is_not_a_terminal() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ghost"))
        .arg("reset")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run ghost reset");
    // A script would answer whatever is asked; it must not matter.
    let _ = child.stdin.take().unwrap().write_all(b"yes\nabcde\n");
    let out = child.wait_with_output().expect("ghost reset ended");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.contains("interactive terminal"), "{stderr}");
    assert!(!stdout.contains("lifted"), "it lifted the stop from a pipe: {stdout}");
}
