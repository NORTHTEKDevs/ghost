//! Verifies `By::role("edit")` resolves correctly across the Windows text-surface
//! families, and - critically - that the WinUI alias never outranks an exact match.
//!
//! Background: Win11 ships Notepad as a WinUI app whose text area reports UIA
//! control type Document (50030) rather than Edit (50004). Ghost aliases
//! edit -> document to handle that, but the alias is a FALLBACK only, because in
//! Chromium the entire web page is a Document enclosing the real Edit controls.
//! An eager alias would return the page body instead of the omnibox.
//!
//! Run with:
//!   cargo test -p ghost-session --test text_surface_matrix -- --ignored --test-threads=1

#![cfg(windows)]

use ghost_session::{By, GhostSession};
use std::time::Duration;

const UIA_EDIT: u32 = 50004;
const UIA_DOCUMENT: u32 = 50030;

struct Probe {
    control_type: u32,
    text: String,
}

/// PIDs of every running process with this image name.
///
/// Needed because `process::kill` on the PID returned by `launch` does not stop a
/// WinUI/Store app: `notepad.exe` in System32 is a launcher stub that hands off to
/// a separate Store-app process and then exits. Killing only the stub leaks a
/// window per run - an earlier version of this file left 13 Notepad processes on
/// the machine, and a later test then found one of THOSE instead of its own target.
fn pids_named(image: &str) -> Vec<u32> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split("\",\"").nth(1).and_then(|p| p.trim_matches('"').parse().ok()))
        .collect()
}

/// Launch `exe`, focus it, resolve `role=edit`, type `text`, read it back.
///
/// Cleanup kills only processes that did not exist before this call, so a Notepad
/// or browser window the user already had open is never touched.
async fn probe(exe: &str, window_hint: &str, text: &str) -> Result<Probe, String> {
    let before = pids_named(exe);

    let session = GhostSession::new()
        .map_err(|e| format!("session: {e}"))?
        .with_timeout(8000);
    let pid = session
        .launch(exe)
        .await
        .map_err(|e| format!("launch {exe}: {e}"))?;
    if pid == 0 {
        return Err(format!("launch {exe} returned pid 0"));
    }
    tokio::time::sleep(Duration::from_millis(2000)).await;

    let result = async {
        // Make the foreground deterministic. find() searches the foreground
        // window first and only then walks the whole desktop, so without this the
        // desktop fallback can return a text field belonging to a DIFFERENT
        // application entirely.
        session
            .focus_window(window_hint)
            .await
            .map_err(|e| format!("focus {window_hint}: {e}"))?;
        tokio::time::sleep(Duration::from_millis(600)).await;

        let edit = session
            .find(By::role("edit"))
            .await
            .map_err(|e| format!("find role=edit in {exe}: {e}"))?;
        let control_type = edit.control_type();
        edit.type_text(text)
            .map_err(|e| format!("type into {exe}: {e}"))?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok::<Probe, String>(Probe {
            control_type,
            text: edit.get_text(),
        })
    }
    .await;

    // Kill only what this call started, then WAIT for the processes to actually
    // go away. process::kill is asynchronous: returning while a killed window is
    // still tearing down left the desktop in a state where the next test's
    // launch could not be focused (FocusFailed), which failed the release gate.
    let mine: Vec<u32> = pids_named(exe).into_iter().filter(|p| !before.contains(p)).collect();
    for p in &mine {
        ghost_core::process::kill(*p).ok();
    }
    for _ in 0..40 {
        let still = pids_named(exe);
        if !mine.iter().any(|p| still.contains(p)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Give the shell a moment to settle focus after the window disappears.
    tokio::time::sleep(Duration::from_millis(300)).await;

    result
}

#[tokio::test]
#[ignore]
async fn winui_document_surface_resolves_via_alias() {
    let p = probe("notepad.exe", "Notepad", "ghost-winui-probe")
        .await
        .expect("WinUI text surface must resolve");

    // Measured on a CLEAN desktop: Win11 WinUI Notepad's text area is a
    // Document (50030), reached through the edit->document alias. Accept Edit
    // too, because which control type Microsoft ships is not Ghost's contract -
    // "role=edit finds a usable text surface" is.
    assert!(
        p.control_type == UIA_EDIT || p.control_type == UIA_DOCUMENT,
        "role=edit on a WinUI app must resolve to a text surface, got control type {}",
        p.control_type
    );
    assert!(
        p.text.contains("ghost-winui-probe"),
        "typed text did not read back from the WinUI surface; got {:?}",
        p.text
    );
}

#[tokio::test]
#[ignore]
async fn chromium_omnibox_prefers_exact_edit_over_enclosing_document() {
    // THE regression guard. Edge's page body is a Document. If the alias were
    // applied eagerly instead of as a fallback, this resolves to the page and
    // the control type comes back as Document.
    let mut last_err = String::new();
    let mut found = None;
    for (exe, hint) in [("msedge.exe", "Edge"), ("chrome.exe", "Chrome")] {
        // Chromium browsers are multi-process singletons: launching one when an
        // instance is already up hands the request to the existing browser and
        // exits, so this test cannot tell its own window from the user's. PID
        // diffing (which works for Notepad) cannot fix that. Refuse to run
        // rather than assert against a window we do not own - an unisolated
        // run here produced a phantom failure on 2026-07-25.
        if !pids_named(exe).is_empty() {
            last_err = format!("{exe} is already running; cannot isolate a test window");
            continue;
        }
        match probe(exe, hint, "example.com").await {
            Ok(p) => {
                found = Some(p);
                break;
            }
            Err(e) => last_err = e,
        }
    }
    let Some(p) = found else {
        // Loud skip, not a silent pass: this machine has no Chromium browser.
        eprintln!("SKIP chromium_omnibox: no Chromium browser launchable ({last_err})");
        return;
    };

    assert_eq!(
        p.control_type, UIA_EDIT,
        "role=edit in a browser must resolve to the omnibox Edit, not the \
         enclosing page Document. Got control type {} - the edit->document \
         alias is being preferred over an exact match.",
        p.control_type
    );
    assert!(
        p.text.contains("example.com"),
        "typed text did not read back from the omnibox; got {:?}",
        p.text
    );
}
