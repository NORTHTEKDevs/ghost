//! Ghost Testbed: a deterministic Win32 target for the live test suite.
//!
//! Why this exists: the live tests used to drive Windows 11 Notepad, which is a
//! single-instance Store app that RESTORES THE USER'S OWN TABS into whatever
//! instance starts - including one on a hidden desktop - so a test that typed
//! into "the first document" typed into the user's unsaved file. This window has
//! no session, no singleton, no auto-save and no surprises:
//!
//! - a STATIC label "Field" followed by an EDIT control (the UIA proxy names the
//!   edit after the label, so `role=edit` and `name=Field` both resolve);
//! - a BUTTON "Increment" that appends ` [clicks=N]` to the window title, so a
//!   click is verifiable from outside through the title alone;
//! - a BUTTON "Quit" that closes the window and ends the process.
//!
//! `ghost-testbed [--title <text>]`. Exits when the window closes.

#![cfg(windows)]
#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicU32, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_EDIT: i32 = 101;
const ID_INCREMENT: i32 = 102;
const ID_QUIT: i32 = 103;

static CLICKS: AtomicU32 = AtomicU32::new(0);
static BASE_TITLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            match id {
                ID_INCREMENT => {
                    let n = CLICKS.fetch_add(1, Ordering::SeqCst) + 1;
                    let base = BASE_TITLE.get().cloned().unwrap_or_default();
                    let title = wide(&format!("{base} [clicks={n}]"));
                    let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
                }
                ID_QUIT => {
                    // PostMessage, not DestroyWindow: BM_CLICK arrives as a
                    // SendMessage from the automating thread, and destroying the
                    // window inside that nested send is refused. Queue it instead.
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    let mut title = String::from("Ghost Testbed");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--title" {
            if let Some(t) = args.next() {
                title = t;
            }
        }
    }
    let _ = BASE_TITLE.set(title.clone());
    let title_w = wide(&title);
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        let class = w!("GhostTestbedWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_WINDOW.0 + 1) as isize as *mut _,
            ),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            120,
            120,
            520,
            260,
            None,
            None,
            hinst,
            None,
        )
        .expect("create window");
        let child = |cls: PCWSTR, text: PCWSTR, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32| {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                cls,
                text,
                WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
                x,
                y,
                w,
                h,
                hwnd,
                HMENU(id as isize as *mut _),
                hinst,
                None,
            )
            .expect("create child")
        };
        let _label = child(w!("STATIC"), w!("Field"), 0, 20, 24, 60, 22, 0);
        let _edit = child(
            w!("EDIT"),
            w!(""),
            WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            90,
            20,
            380,
            28,
            ID_EDIT,
        );
        let _inc = child(w!("BUTTON"), w!("Increment"), BS_PUSHBUTTON as u32, 90, 70, 140, 34, ID_INCREMENT);
        let _quit = child(w!("BUTTON"), w!("Quit"), BS_PUSHBUTTON as u32, 250, 70, 100, 34, ID_QUIT);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
