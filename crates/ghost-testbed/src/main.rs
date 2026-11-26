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
//! - a BUTTON "Quit" that closes the window and ends the process;
//! - a DISABLED BUTTON "Locked": a click on it must have no effect, the negative
//!   control for action verification;
//! - a BUTTON "Note" whose only effect is rewriting the distant STATIC "Status"
//!   label (not the title, not the button): a click whose effect is elsewhere in
//!   the window, which verification must still see.
//!
//! - a BUTTON "Popup" that opens an owned popup window of the same process,
//!   over the main window, without activating it; the popup's BUTTON "Ping"
//!   only appends ` [popup=N]` to the main window's title (an owned window a
//!   coordinate click must still be allowed to reach).
//!
//! `ghost-testbed [--title <text>] [--label <text>] [--pos <x>,<y>] [--topmost]`.
//! `--label` sets the Status label's starting text, so two windows can be told
//! apart through their UI tree. `--pos` places the window and `--topmost`
//! creates it topmost, so occlusion tests can cover one testbed with another
//! without any other window in between. Exits when the window closes.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod bed {
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_EDIT: i32 = 101;
    const ID_INCREMENT: i32 = 102;
    const ID_QUIT: i32 = 103;
    /// A second button with the SAME accessible name as ID_INCREMENT, so an
    /// `index` disambiguation can be proven: it records `[alt=N]`, not
    /// `[clicks=N]`, so the title says which of the two was pressed.
    const ID_INCREMENT_ALT: i32 = 104;
    const ID_LOCKED: i32 = 105;
    const ID_NOTE: i32 = 106;
    const ID_STATUS: i32 = 107;
    const ID_POPUP: i32 = 108;
    const ID_PING: i32 = 201;

    static CLICKS: AtomicU32 = AtomicU32::new(0);
    static NOTES: AtomicU32 = AtomicU32::new(0);
    static PINGS: AtomicU32 = AtomicU32::new(0);
    static BASE_TITLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    ID_INCREMENT => {
                        let n = CLICKS.fetch_add(1, Ordering::SeqCst) + 1;
                        let base = BASE_TITLE.get().cloned().unwrap_or_default();
                        let title = wide(&format!("{base} [clicks={n}]"));
                        let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
                    }
                    ID_INCREMENT_ALT => {
                        let n = CLICKS.fetch_add(1, Ordering::SeqCst) + 1;
                        let base = BASE_TITLE.get().cloned().unwrap_or_default();
                        let title = wide(&format!("{base} [alt={n}]"));
                        let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
                    }
                    ID_NOTE => {
                        let n = NOTES.fetch_add(1, Ordering::SeqCst) + 1;
                        let text = wide(&format!("Status: noted {n}"));
                        if let Ok(status) = GetDlgItem(hwnd, ID_STATUS) {
                            let _ = SetWindowTextW(status, PCWSTR(text.as_ptr()));
                        }
                    }
                    ID_POPUP => {
                        open_popup(hwnd);
                    }
                    ID_QUIT => {
                        // PostMessage, not DestroyWindow: BM_CLICK arrives as a
                        // SendMessage from the automating thread, and destroying
                        // the window inside that nested send is refused. Queue it.
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

    /// The popup's window procedure: "Ping" marks the OWNER's title.
    unsafe extern "system" fn popup_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_COMMAND && (wparam.0 & 0xffff) as i32 == ID_PING {
            if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
                let n = PINGS.fetch_add(1, Ordering::SeqCst) + 1;
                let base = BASE_TITLE.get().cloned().unwrap_or_default();
                let title = wide(&format!("{base} [popup={n}]"));
                let _ = SetWindowTextW(owner, PCWSTR(title.as_ptr()));
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// An owned popup of this process, over the owner's client area, shown
    /// without activation (a menu, dropdown or tooltip stands in the same
    /// relation to its owner).
    unsafe fn open_popup(owner: HWND) {
        let hinst: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        let class = w!("GhostTestbedPopup");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: hinst,
            lpszClassName: class,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_BTNFACE.0 + 1) as isize as *mut _,
            ),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(owner, &mut r);
        // Topmost when its owner is: a non-topmost owned window shown over a
        // topmost owner took the owner out of the topmost band (measured).
        let owner_ex = WINDOW_EX_STYLE(GetWindowLongPtrW(owner, GWL_EXSTYLE) as u32);
        let Ok(popup) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | (owner_ex & WS_EX_TOPMOST),
            class,
            w!("Ghost Testbed Popup"),
            WS_POPUP | WS_BORDER,
            r.left + 40,
            r.top + 120,
            220,
            70,
            owner,
            None,
            hinst,
            None,
        ) else {
            return;
        };
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            w!("Ping"),
            WINDOW_STYLE(BS_PUSHBUTTON as u32) | WS_CHILD | WS_VISIBLE,
            20,
            15,
            180,
            40,
            popup,
            HMENU(ID_PING as isize as *mut _),
            hinst,
            None,
        );
        let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
    }

    pub fn run() {
        let mut title = String::from("Ghost Testbed");
        let mut label = String::from("Status: idle");
        let mut pos = (120, 120);
        let mut topmost = false;
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            if a == "--title" {
                if let Some(t) = args.next() {
                    title = t;
                }
            } else if a == "--label" {
                if let Some(t) = args.next() {
                    label = t;
                }
            } else if a == "--pos" {
                if let Some((x, y)) = args.next().as_deref().and_then(|v| v.split_once(',')) {
                    if let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) {
                        pos = (x, y);
                    }
                }
            } else if a == "--topmost" {
                topmost = true;
            }
        }
        let label_w = wide(&label);
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
                if topmost { WS_EX_TOPMOST } else { WINDOW_EX_STYLE::default() },
                class,
                PCWSTR(title_w.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                pos.0,
                pos.1,
                520,
                260,
                None,
                None,
                hinst,
                None,
            )
            .expect("create window");
            let child =
                |cls: PCWSTR, text: PCWSTR, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32| {
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
            let _inc = child(
                w!("BUTTON"),
                w!("Increment"),
                BS_PUSHBUTTON as u32,
                90,
                70,
                140,
                34,
                ID_INCREMENT,
            );
            let _quit = child(
                w!("BUTTON"),
                w!("Quit"),
                BS_PUSHBUTTON as u32,
                250,
                70,
                100,
                34,
                ID_QUIT,
            );
            // Same name as the first Increment, deliberately (see ID_INCREMENT_ALT).
            let _inc_alt = child(
                w!("BUTTON"),
                w!("Increment"),
                BS_PUSHBUTTON as u32,
                90,
                114,
                140,
                34,
                ID_INCREMENT_ALT,
            );
            let _locked = child(
                w!("BUTTON"),
                w!("Locked"),
                BS_PUSHBUTTON as u32 | WS_DISABLED.0,
                250,
                114,
                100,
                34,
                ID_LOCKED,
            );
            // Top right and bottom left: as far apart as the window allows.
            let _note = child(w!("BUTTON"), w!("Note"), BS_PUSHBUTTON as u32, 370, 70, 110, 34, ID_NOTE);
            let _status = child(w!("STATIC"), PCWSTR(label_w.as_ptr()), 0, 20, 180, 200, 22, ID_STATUS);
            let _popup = child(w!("BUTTON"), w!("Popup"), BS_PUSHBUTTON as u32, 370, 114, 110, 34, ID_POPUP);
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    bed::run();
}

/// The program is Win32 throughout; off Windows it stays an empty binary so the
/// workspace still cross-checks for other targets.
#[cfg(not(windows))]
fn main() {
    eprintln!("ghost-testbed is a Windows-only test target");
}
