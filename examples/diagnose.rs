//! Diagnostic: verify all 24 Ghost MCP tools. Run: cargo run --example diagnose

use ghost_session::{GhostSession, By, session::Region};
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("=== Ghost v0.2.0 Diagnostic (24 tools) ===");
    println!("Emergency stop: Ctrl+Alt+G\n");

    let session = match GhostSession::new() {
        Ok(s) => s,
        Err(e) => { eprintln!("FATAL: {}", e); return; }
    };

    // --- Perception ---
    check("screenshot", {
        match session.screenshot(Region::full()).await {
            Ok(png) => { std::fs::write("diag_screenshot.png", &png).ok(); format!("OK {} bytes", png.len()) }
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("describe_screen (full)", {
        match session.describe_screen(None).await {
            Ok(els) => format!("OK {} elements", els.len()),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("list_windows", {
        match session.list_windows().await {
            Ok(wins) => format!("OK {} windows", wins.len()),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    // --- Clipboard ---
    let test_text = "ghost-v0.2.0-clipboard-test";
    check("set_clipboard", {
        match session.set_clipboard(test_text).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });
    check("get_clipboard", {
        match session.get_clipboard().await {
            Ok(text) if text == test_text => "OK (roundtrip verified)".into(),
            Ok(text) => format!("FAIL: got {:?}", text),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    // --- Launch a known app ---
    check("launch notepad", {
        match session.launch("notepad.exe").await {
            Ok(pid) => format!("OK pid={}", pid),
            Err(e) => format!("FAIL: {}", e),
        }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    save_screenshot(&session, "diag_notepad.png").await;

    // --- Window management ---
    check("focus_window (notepad)", {
        match session.focus_window("Notepad").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("window_state maximize", {
        match session.window_state("Notepad", "maximize").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    check("window_state restore", {
        match session.window_state("Notepad", "restore").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    // --- Keyboard ---
    check("find edit (notepad)", {
        match session.find(By::role("edit")).await {
            Ok(el) => format!("OK rect={:?}", el.bounding_rect()),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("press Tab", {
        match session.press("Tab").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("hotkey Ctrl+A", {
        match session.hotkey(&["Ctrl"], "a").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("key_down Shift", {
        match session.key_down("Shift").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("key_up Shift", {
        match session.key_up("Shift").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    // --- Mouse ---
    check("hover (center)", {
        match session.hover(960, 540).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("right_click_at (center)", {
        match session.right_click_at(960, 540).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.press("Escape").await.ok();

    check("double_click_at (center)", {
        match session.double_click_at(960, 540).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("scroll down (center)", {
        match session.scroll(960, 540, "down", 3).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    check("drag (short move)", {
        match session.drag(400, 400, 450, 450).await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    // --- Wait ---
    check("wait 200ms", {
        let before = std::time::Instant::now();
        session.wait(200).await;
        format!("OK ({}ms)", before.elapsed().as_millis())
    });

    // --- Close notepad ---
    check("window_state close (notepad)", {
        match session.window_state("Notepad", "close").await {
            Ok(_) => "OK".into(),
            Err(e) => format!("FAIL: {}", e),
        }
    });

    save_screenshot(&session, "diag_final.png").await;
    println!("\n=== Diagnostic complete. Review diag_*.png ===");
}

fn check(label: &str, result: String) {
    println!("[{}] {}", label, result);
}

async fn save_screenshot(session: &GhostSession, path: &str) {
    if let Ok(png) = session.screenshot(Region::full()).await {
        std::fs::write(path, &png).ok();
        println!("  -> {}", path);
    }
}
