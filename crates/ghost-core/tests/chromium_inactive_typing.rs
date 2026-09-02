//! Physics probe for the one typing case the hidden-desktop suites do not
//! exercise on their own: posted click + posted characters into a Chromium
//! window whose page is BLURRED (deactivated), which is the state a browser on
//! the user's desktop is in while the human works in another window. That is
//! the user-desktop typing ladder's rung 2 (`GhostSession::act_background`).
//!
//! A hidden desktop has no foreground window at all (GetForegroundWindow is
//! NULL there), so the deactivation is delivered the way the system delivers
//! it - WM_NCACTIVATE(FALSE), WM_ACTIVATE(WA_INACTIVE), WM_KILLFOCUS - and the
//! page itself reports what it saw through its title (onblur / onfocus /
//! document.hasFocus at input time). Never touches the human's desktop.
//!
//!   cargo test -p ghost-core --release --test chromium_inactive_typing -- --ignored --nocapture

#![cfg(windows)]

use ghost_core::desktop::DesktopSession;
use ghost_core::input::BackgroundClicker;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn first_existing(cands: &[&str]) -> Option<PathBuf> {
    cands.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Nothing may outlive the probe on a desktop nobody can see.
struct Kill(Vec<u32>);
impl Drop for Kill {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = ghost_core::process::kill(*pid);
        }
    }
}

fn title_of(desktop: &DesktopSession, hwnd: isize) -> String {
    desktop
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.hwnd == hwnd)
        .map(|w| w.title)
        .unwrap_or_default()
}

fn wait_title(desktop: &DesktopSession, hwnd: isize, needle: &str, ms: u64) -> String {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        let t = title_of(desktop, hwnd);
        if t.contains(needle) || Instant::now() >= deadline {
            return t;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn posted_typing_lands_in_a_blurred_chromium_window() {
    let Some(chrome) = first_existing(&[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ]) else {
        eprintln!("skipped: no Chromium browser installed");
        return;
    };

    let tmp = std::env::temp_dir().join("ghost-inactive-typing");
    std::fs::create_dir_all(&tmp).unwrap();
    let page = tmp.join("page.html");
    std::fs::write(
        &page,
        "<!doctype html><title>GBT READY</title><input id=\"i\" aria-label=\"Field\" \
         oninput=\"document.title='GBT TYPED '+this.value+(document.hasFocus()?' F':' B')\" \
         style=\"position:fixed;left:20px;top:20px;width:400px;height:60px;font-size:30px\">\
         <script>window.onblur=()=>document.title='GBT BLURRED';\
         window.onfocus=()=>document.title='GBT FOCUSED';</script>",
    )
    .unwrap();
    let profile = tmp.join("profile");
    let url = format!("file:///{}", page.display().to_string().replace(std::path::MAIN_SEPARATOR, "/"));

    let desktop = DesktopSession::create("inactive-chromium").expect("hidden desktop");
    let mut kill = Kill(Vec::new());
    let cmd = format!(
        "\"{}\" --user-data-dir=\"{}\" --no-first-run --no-default-browser-check \
         --disable-gpu-vsync --disable-frame-rate-limit --window-size=800,600 \"{url}\"",
        chrome.display(),
        profile.display()
    );
    kill.0.push(desktop.launch(&cmd).expect("launch chromium"));
    let w = desktop.wait_for_window("GBT", 20_000).expect("chromium window with the page");
    let ch = w.hwnd;
    kill.0.push(w.pid);
    std::thread::sleep(Duration::from_millis(1500));
    let at_launch = title_of(&desktop, ch);

    // The input's centre through UIA on that desktop (Chromium builds its
    // accessibility tree lazily: the first walk switches it on).
    let mut centre = None;
    for _ in 0..4 {
        centre = desktop
            .with_uia(move |tree| {
                tree.find_by_name_in_hwnd(ch, "Field")
                    .ok()
                    .flatten()
                    .and_then(|e| e.bounding_rect())
                    .map(|r| r.center())
            })
            .expect("uia");
        if centre.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(450));
    }
    let (cx, cy) = centre.expect("the page input through UIA");

    // Deactivate the window the way the system does when another window is
    // activated, then let the page say whether it went blurred.
    desktop
        .exec(move || unsafe {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                SendMessageTimeoutW, SMTO_ABORTIFHUNG, WA_INACTIVE, WM_ACTIVATE, WM_KILLFOCUS, WM_NCACTIVATE,
            };
            let h = HWND(ch as *mut core::ffi::c_void);
            let focused = BackgroundClicker::focused_control(ch);
            let f = HWND(focused as *mut core::ffi::c_void);
            SendMessageTimeoutW(h, WM_NCACTIVATE, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None);
            SendMessageTimeoutW(h, WM_ACTIVATE, WPARAM(WA_INACTIVE as usize), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None);
            SendMessageTimeoutW(f, WM_KILLFOCUS, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None);
        })
        .expect("worker");
    let blurred = wait_title(&desktop, ch, "GBT BLURRED", 4_000);

    // Rung 2 exactly as the product does it: a posted click at the element,
    // then posted characters to the control that took the focus.
    let typed = "inactive ok";
    desktop
        .exec(move || -> Result<(), String> {
            BackgroundClicker::click_screen(ch, cx, cy).map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(200));
            let target = BackgroundClicker::focused_control(ch);
            for c in typed.chars() {
                BackgroundClicker::send_char(target, c).map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .expect("worker")
        .expect("posted input");
    let after = wait_title(&desktop, ch, "GBT TYPED inactive ok", 8_000);
    eprintln!(
        "INACTIVE TYPING: at_launch={at_launch:?} after_deactivate={blurred:?} after_typing={after:?} \
         (F = page believed it had focus while typing, B = it did not)"
    );
    assert!(
        blurred.contains("GBT BLURRED"),
        "could not put the page into the blurred state, nothing to prove: {blurred:?}"
    );
    assert!(
        after.contains("GBT TYPED inactive ok"),
        "the text did not land in the blurred Chromium window: {after:?}"
    );
}
