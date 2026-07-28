//! Integration test: automate Notepad
//! Run with: cargo test -p ghost-session --test notepad -- --ignored --nocapture

// Compiles on Windows only; see the note in calculator.rs.
#![cfg(windows)]

use ghost_session::{GhostSession, By, session::Region};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires Windows display; run manually"]
async fn test_notepad_type_text() {
    let session = GhostSession::new().expect("failed to create session");

    let pid = session.launch("notepad.exe").await
        .expect("failed to launch Notepad");

    // Wait for Notepad to fully initialize
    tokio::time::sleep(Duration::from_millis(800)).await;

    let edit = session.find(By::role("edit")).await
        .expect("failed to find edit area");

    edit.type_text("Hello from Ghost!").expect("failed to type text");

    // Screenshot as proof
    let png = session.screenshot(Region::full()).await
        .expect("failed to take screenshot");
    assert!(!png.is_empty(), "screenshot should not be empty");
    std::fs::write("notepad_test_screenshot.png", &png).ok();

    // Cleanup
    ghost_core::process::kill(pid).ok();
}

#[tokio::test]
#[ignore = "requires Windows display; run manually"]
async fn test_notepad_find_by_name() {
    let session = GhostSession::new().expect("failed to create session");

    let pid = session.launch("notepad.exe").await
        .expect("failed to launch Notepad");
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Notepad's window title contains "Notepad"
    let el = session.find(By::name("Notepad")).await
        .expect("failed to find element with name Notepad");
    assert!(!el.name().is_empty());

    ghost_core::process::kill(pid).ok();
}
