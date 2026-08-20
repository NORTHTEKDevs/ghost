//! Diagnostic: which automation mechanisms actually work on a non-displayed desktop?
use ghost_core::desktop::DesktopSession;
use windows::Win32::Foundation::HWND;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let d = DesktopSession::create("probe")?;
    println!("desktop: {}", d.name());

    // 1. SendInput
    let (sent, err) = d.exec(|| unsafe {
        use windows::Win32::Foundation::{GetLastError, SetLastError, WIN32_ERROR};
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let inputs = [INPUT { r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_A, ..Default::default() } } }];
        SetLastError(WIN32_ERROR(0));
        let s = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        (s, GetLastError().0)
    })?;
    println!("[SendInput]     sent={sent} lasterror={err}  -> {}",
             if sent > 0 { "WORKS" } else { "BLOCKED (ERROR_ACCESS_DENIED=5)" });

    let scratch = std::env::temp_dir().join(format!("ghost_probe_{}.txt", std::process::id()));
    std::fs::write(&scratch, "")?;
    let pid = d.launch(&format!("notepad.exe {}", scratch.display()))?;
    std::thread::sleep(std::time::Duration::from_millis(3000));

    // 2. Window enumeration, visible windows only
    let visible = d.exec(|| {
        ghost_core::desktop::visible_windows()
    })??;
    println!("[EnumDesktop]   {} visible windows: {:?}", visible.len(),
             visible.iter().map(|w| w.title.clone()).collect::<Vec<_>>());
    let Some(w) = visible.iter().find(|w| w.title.to_lowercase().contains("notepad")) else {
        println!("no notepad window found");
        let _ = ghost_core::process::kill(pid);
        return Ok(());
    };
    println!("target: '{}' hwnd={:#x}", w.title, w.hwnd);

    // 3. PostMessage typing
    let hw = w.hwnd;
    let posted = d.exec(move || {
        ghost_core::input::postmessage::type_text(HWND(hw as *mut core::ffi::c_void),
                                                  "posted into a hidden desktop")
    })?;
    println!("[PostMessage]   {:?}", posted.map(|_| "WORKS"));

    // 4. UIA from a thread bound to this desktop
    let uia = d.exec(move || -> Result<String, String> {
        ghost_core::uia::init_com().map_err(|e| e.to_string())?;
        let tree = ghost_core::uia::tree::UiaTree::new().map_err(|e| e.to_string())?;
        let el = tree.find_by_role_in(None, "document").map_err(|e| e.to_string())?;
        match el {
            Some(e) => Ok(format!("found document, text={:?}",
                ghost_core::uia::patterns::document_text(&e, 200).unwrap_or_default())),
            None => Err("no document element visible".into()),
        }
    })?;
    println!("[UIA]           {uia:?}");

    // 5. Capture
    let cap = d.capture(w.hwnd, false);
    match &cap {
        Ok(png) => {
            std::fs::write(std::env::temp_dir().join("ghost_probe_desktop.png"), png)?;
            println!("[PrintWindow]   WORKS, {} bytes", png.len());
        }
        Err(e) => println!("[PrintWindow]   FAILED: {e}"),
    }

    let _ = ghost_core::process::kill(pid);
    let _ = std::fs::remove_file(&scratch);
    Ok(())
}
