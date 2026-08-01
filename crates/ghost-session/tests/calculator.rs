//! Live Windows-app test: drives Notepad/Calculator/Explorer via the
//! Windows engine, so it is gated to Windows.
#![cfg(windows)]

//! Integration test: automate Calculator
//! Run with: cargo test -p ghost-session --test calculator -- --ignored --nocapture

use ghost_session::{GhostSession, By};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires Windows display; run manually"]
async fn test_calculator_click_button() {
    let session = GhostSession::new().expect("failed to create session");

    let pid = session.launch("calc.exe").await
        .expect("failed to launch Calculator");

    // Calculator takes longer to initialize than Notepad
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Click the "One" button (UIA name for the "1" key)
    let btn = session.find(By::name("One")).await
        .expect("failed to find '1' button");
    btn.click().expect("failed to click '1' button");

    ghost_core::process::kill(pid).ok();
}
