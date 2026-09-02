//! Live Windows-app test: drives Calculator through the BACKGROUND path.
//! Run with: cargo test -p ghost-session --test calculator -- --ignored --nocapture
//!
//! This test used to call `GhostElement::click()`, which is the foreground path
//! (UIA Invoke with a coordinate fallback that moves the real cursor). Since
//! 0.19 the default focus policy refuses that, so the test failed with
//! `NoBackgroundPath` on every run - it was asserting pre-0.19 behaviour. The
//! product's own path is `click_background`: UIA Invoke only, no cursor, no
//! foreground, and an honest error when the control has no Invoke pattern.

#![cfg(windows)]

use ghost_session::{By, GhostSession};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires Windows display; run manually"]
async fn calculator_button_clicks_in_the_background() {
    let session = GhostSession::new().expect("failed to create session");
    let pid = match session.launch("calc.exe").await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipped: could not launch Calculator ({e})");
            return;
        }
    };
    // Calculator is a Store app: it hands off to a broker, takes a while to show
    // a window, and may not come up at all on a desktop that is never displayed.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // "One" is the UIA name of the 1 key.
    let Ok(btn) = session.find(By::name("One")).await else {
        eprintln!("skipped: Calculator's keypad did not appear");
        ghost_core::process::kill(pid).ok();
        return;
    };
    let result = btn.click_background();
    ghost_core::process::kill(pid).ok();
    result.expect("a XAML button exposes InvokePattern, so the background click must succeed");
}
