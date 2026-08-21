//! Live isolated-desktop input tests.
//!
//! `#[ignore]` gated - they need a real interactive Windows session, so they run
//! with `cargo test -p ghost-core --test desktop_input -- --ignored`.
//!
//! These exist because `desktop_type` used to report `ok` while doing nothing.
//! Nothing on a never-displayed desktop has ever held keyboard focus, so
//! `focused_control` fell through to the top-level frame and every WM_CHAR was
//! posted to a window that ignores it. The call still returned `Ok(())`, which is
//! precisely the blind `ok: true` the product is sold against.
//!
//! The invariant under test is deliberately NOT "typing always works". Whether a
//! given control accepts posted characters is the app's business, and picking a
//! cooperative app would make the test a statement about that app. The invariant
//! is the honest one, and it holds for every target:
//!
//!     type_text may fail, but it must never report success without the text
//!     actually being in the control.
//!
//! That is exactly what the old code violated, so this fails against it and
//! passes against the fix regardless of which app is used.

#![cfg(windows)]

use ghost_core::desktop::DesktopSession;

fn wait_for(d: &DesktopSession, title: &str, ms: u64) -> Option<isize> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if let Ok(windows) = d.windows() {
            if let Some(w) = windows.iter().find(|w| w.title.contains(title)) {
                return Some(w.hwnd);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    None
}

/// Character Map is a classic Win32 dialog with a real `Edit` control, so it
/// exercises the targeting path rather than the windowless-control question.
#[test]
#[ignore]
fn typing_never_reports_success_without_the_text_landing() {
    const MARK: &str = "GhostTypeProof";
    let d = DesktopSession::create("typetest").expect("create desktop");
    d.launch("charmap.exe").expect("launch charmap");
    let hwnd = wait_for(&d, "Character Map", 10_000).expect("charmap on the isolated desktop");

    match d.type_text(hwnd, MARK) {
        Ok(()) => {
            // Claimed success: the text MUST be readable back. This is the
            // assertion the old implementation could not survive.
            let seen = d.read_text(hwnd).unwrap_or_default();
            assert!(
                seen.contains(MARK),
                "type_text reported success but the control reads back {seen:?} - \
                 this is the blind ok the read-back exists to prevent"
            );
        }
        Err(e) => {
            // Honest failure is a pass. What matters is that it did not lie.
            eprintln!("type_text declined, which is acceptable: {e}");
        }
    }
}

#[test]
#[ignore]
fn typing_into_a_window_with_no_text_control_errors_instead_of_claiming_success() {
    // A window with no message-postable text control must FAIL loudly. Reporting
    // Ok here is the original defect.
    let d = DesktopSession::create("notarget").expect("create desktop");
    d.launch("mspaint.exe").expect("launch paint");
    let Some(hwnd) = wait_for(&d, "Paint", 10_000) else {
        eprintln!("skipped: Paint did not appear on the isolated desktop");
        return;
    };
    let result = d.type_text(hwnd, "should not silently succeed");
    assert!(
        result.is_err(),
        "typing into a window with no text control must error, got {result:?}"
    );
}

// A third test asserting `read_text` errors on a control-less window was removed
// rather than weakened. It was a claim about one app's internals (Paint exposes
// something the class filter matches), not about this crate's contract - the same
// over-specification that makes a suite look green while proving nothing. The two
// tests above cover the actual defect: never claim success without the read-back,
// and fail loudly when there is no target.
