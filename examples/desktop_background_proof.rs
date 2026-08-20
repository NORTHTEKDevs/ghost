//! Live proof that ghost drives a desktop app without taking the screen.
//!
//! Opens a scratch file in Notepad, drives it entirely through background paths, and
//! measures the thing a user would notice if it cheated: the foreground window
//! changing. Keep working in another window while it runs.
//!
//!     cargo run -p ghost-session --example desktop_background_proof

use ghost_core::system::DesktopSnapshot;
use ghost_session::{By, GhostSession};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;
    println!("focus policy: {}", session.focus_policy());
    assert_eq!(
        session.focus_policy(),
        "background",
        "the default policy must be background or the guarantee is off"
    );

    // A uniquely-named scratch file. Matching on "Notepad" alone would also match a
    // document the user already has open, and typing into that would edit their work.
    let stamp = std::process::id();
    let mut scratch = std::env::temp_dir();
    scratch.push(format!("ghost_scratch_{stamp}.txt"));
    std::fs::write(&scratch, "scratch\n")?;
    let file_name = scratch.file_name().unwrap().to_string_lossy().to_string();

    println!("opening scratch file {file_name}");
    session.launch(&format!("notepad.exe {}", scratch.display())).await?;
    tokio::time::sleep(Duration::from_millis(2000)).await;

    let target = session.window(&file_name)?;
    assert!(
        target.title.contains(&file_name),
        "refusing to run: resolved '{}' instead of the scratch file",
        target.title
    );
    println!("target: '{}' (pid {})", target.title, target.pid);

    println!("\nswitch to any other window now - 3 seconds");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let before = DesktopSnapshot::capture();
    println!("before: foreground='{}'", before.foreground_title);

    let mut failures: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("[{}] {:<28} {}", if ok { "ok  " } else { "FAIL" }, name, detail);
        if !ok {
            failures.push(format!("{name}: {detail}"));
        }
    };

    // ---- 1. background typing via window messages ------------------------
    let typed = "ghost typed this in the background";
    match target.type_text(typed) {
        Ok(()) => check("type_background", true, "WM_CHAR to the focused control".into()),
        Err(e) => check("type_background", false, e.to_string()),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- 2. find the editor by role and read it back ---------------------
    // Scoped to this window. Unscoped, By::role("document") matches the first
    // document-role element on the whole desktop - typically a browser page.
    match session.find_in(&target.title, By::role("document")).await {
        Ok(el) => {
            let text = el.document_text(8192);
            check(
                "find by role + read back",
                text.contains(typed),
                format!("{:?}", text.chars().take(50).collect::<String>()),
            );
            println!("     background actions: {:?}", el.supported_actions());
        }
        Err(e) => check("find by role + read back", false, e.to_string()),
    }

    // ---- 3. background window capture ------------------------------------
    match target.capture(false) {
        Ok(png) => {
            let ok = png.starts_with(&[0x89, 0x50, 0x4E, 0x47]) && png.len() > 2000;
            let mut out = std::env::temp_dir();
            out.push("ghost_bg_window.png");
            std::fs::write(&out, &png)?;
            check("capture_window", ok, format!("{} bytes, window never raised", png.len()));
        }
        Err(e) => check("capture_window", false, e.to_string()),
    }

    // ---- 4. Ctrl+Z must really undo, not type a literal "z" ---------------
    // This is the exact operation that used to corrupt documents: posted key
    // messages cannot set modifier state, so ghost sends WM_UNDO instead.
    target.hotkey(&["Ctrl".into()], "z")?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after_undo = match session.find_in(&target.title, By::role("document")).await {
        Ok(el) => el.document_text(8192),
        Err(e) => format!("<unreadable: {e}>"),
    };
    check(
        "Ctrl+Z is a real undo",
        !after_undo.contains(typed) && !after_undo.contains('z'),
        format!("document now {:?}", after_undo.chars().take(40).collect::<String>()),
    );

    // ---- 4b. a shortcut with no message equivalent must refuse, not fake it
    let hk = target.hotkey(&["Ctrl".into()], "s");
    check(
        "unmappable shortcut refused",
        hk.is_err(),
        "Ctrl+S has no control message; ghost errors rather than typing 's'".into(),
    );

    // ---- 5. the foreground primitives must refuse -------------------------
    let blocked = session.click_at(400, 400).await;
    check(
        "foreground click refused",
        matches!(
            blocked,
            Err(ghost_session::GhostError::Core(
                ghost_core::error::CoreError::NoBackgroundPath { .. }
            ))
        ),
        "SendInput is gated by the focus policy".into(),
    );

    // ---- the measurement --------------------------------------------------
    let delta = before.delta_now();
    println!("\ndesktop state: {}", delta.describe());
    if delta.foreground_changed {
        failures.push(format!("foreground was stolen: {}", delta.describe()));
    }
    if delta.cursor_moved {
        // Advisory only: a human at the keyboard moves their own mouse, and this
        // example cannot distinguish that from a rogue SendInput. The structural
        // guarantee is covered by tests/focus_enforcement.rs.
        println!("  note: cursor moved. ghost cannot move it under this policy, so this");
        println!("        is your own mouse unless focus_enforcement is also failing.");
    }

    println!("\nscratch file: {} (delete when done)", scratch.display());
    if failures.is_empty() {
        println!("PASS: app driven in the background, foreground untouched");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("  FAIL: {f}");
        }
        std::process::exit(1);
    }
}
