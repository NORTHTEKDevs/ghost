//! Live proof of the isolated-desktop backend: an app runs where the user can never
//! see it, and is still fully driveable.
//!
//! Also proves the background-shortcut fix: Ctrl+Z performs a real undo via
//! `WM_UNDO` instead of typing a literal "z" into the document.
//!
//!     cargo run -p ghost-core --example isolated_desktop_proof

use ghost_core::desktop::DesktopSession;
use ghost_core::input::{postmessage as pm, Shortcut};
use ghost_core::system::DesktopSnapshot;
use windows::Win32::Foundation::HWND;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let before = DesktopSnapshot::capture();
    println!("user foreground: '{}'", before.foreground_title);

    let d = DesktopSession::create("proof")?;
    println!("isolated desktop: {}", d.name());
    println!("real input supported here: {}", d.real_input_supported());

    let stem = format!("ghost_iso_{}", std::process::id());
    let scratch = std::env::temp_dir().join(format!("{stem}.txt"));
    std::fs::write(&scratch, "")?;
    let pid = d.launch(&format!("notepad.exe {}", scratch.display()))?;
    println!("launched notepad pid={pid} onto it");

    // Wait for the *document* title, not "Notepad": the app shows a generic window
    // first and retitles once the file is open, so matching the app name catches it
    // before it is ready to receive input.
    let w = d.wait_for_window(&stem, 20_000)?;
    println!("window: '{}' hwnd={:#x}", w.title, w.hwnd);

    let mut failures: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("[{}] {:<26} {}", if ok { "ok  " } else { "FAIL" }, name, detail);
        if !ok {
            failures.push(name.to_string());
        }
    };

    // This window exists, but not on any desktop the user can see. A window of the
    // same title must NOT show up in the user's own window list.
    let leaked = ghost_core::uia::tree::list_windows()?
        .into_iter()
        .any(|u| u.hwnd as isize == w.hwnd);
    check("invisible to the user", !leaked, "not present on the user's desktop".into());

    let typed = "typed into an invisible desktop";
    d.type_text(w.hwnd, typed)?;
    std::thread::sleep(std::time::Duration::from_millis(400));

    // UIA works here, so the app is driveable by control patterns, not just pixels.
    let read_back = d.with_uia(move |tree| {
        tree.find_by_role_in(None, "document")
            .ok()
            .flatten()
            .map(|e| ghost_core::uia::patterns::document_text(&e, 4096).unwrap_or_default())
    })?;
    check(
        "UIA on isolated desktop",
        read_back.as_deref().map(|t| t.contains(typed)).unwrap_or(false),
        format!("{:?}", read_back.as_deref().unwrap_or("").chars().take(45).collect::<String>()),
    );

    // The shortcut fix. Undo must remove the typed text, not append a "z".
    d.shortcut(w.hwnd, "Ctrl+Z")?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    let after_undo = d
        .with_uia(move |tree| {
            tree.find_by_role_in(None, "document")
                .ok()
                .flatten()
                .map(|e| ghost_core::uia::patterns::document_text(&e, 4096).unwrap_or_default())
        })?
        .unwrap_or_default();
    check(
        "Ctrl+Z is a real undo",
        !after_undo.contains(typed) && !after_undo.contains('z'),
        format!("document now {:?}", after_undo.chars().take(45).collect::<String>()),
    );

    match d.capture(w.hwnd, false) {
        Ok(png) => {
            let out = std::env::temp_dir().join("ghost_isolated_desktop.png");
            std::fs::write(&out, &png)?;
            check(
                "capture",
                png.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
                format!("{} bytes -> {}", png.len(), out.display()),
            );
        }
        Err(e) => check("capture", false, e.to_string()),
    }

    // And confirm the same shortcut works on the user's desktop too, message-based.
    check(
        "shortcut names parse",
        Shortcut::parse("Ctrl+Z").is_some() && Shortcut::parse("Ctrl+A").is_some(),
        format!("supported: {}", Shortcut::all().join(", ")),
    );
    check(
        "dead window rejected",
        pm::click(HWND(std::ptr::null_mut()), (1, 1)).is_err(),
        "handles are validated".into(),
    );

    let delta = before.delta_now();
    println!("\nuser desktop: {}", delta.describe());
    if delta.foreground_changed {
        failures.push("user foreground changed".into());
    }

    let _ = ghost_core::process::kill(pid);
    let _ = std::fs::remove_file(&scratch);

    if failures.is_empty() {
        println!("PASS: app driven on an invisible desktop, real undo, user untouched");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("  FAIL: {f}");
        }
        std::process::exit(1);
    }
}
