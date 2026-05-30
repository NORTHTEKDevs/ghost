use windows::Win32::UI::Accessibility::*;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM, TRUE, FALSE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, IsWindowVisible,
    PostMessageW, SetForegroundWindow, ShowWindow, GetWindowThreadProcessId,
    BringWindowToTop,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, WM_CLOSE,
};
use windows::Win32::System::Threading::{GetCurrentThreadId, AttachThreadInput};
use super::element::{UiaElement, role_id_to_name, ElementDescriptor, INTERACTIVE_ROLES};
use crate::error::CoreError;

/// Check if a searched role matches an element role through defined aliases.
/// By::Role("tab") matches both "tab" and "tabitem".
/// By::Role("list") matches both "list" and "listitem".
pub(crate) fn role_alias_matches(searched: &str, el_role: &str) -> bool {
    match searched {
        "tab" => el_role == "tabitem",
        "list" => el_role == "listitem",
        _ => false,
    }
}

pub struct UiaTree {
    automation: IUIAutomation,
}

impl UiaTree {
    pub fn new() -> Result<Self, CoreError> {
        unsafe {
            let automation: IUIAutomation = CoCreateInstance(
                &CUIAutomation8,
                None,
                CLSCTX_INPROC_SERVER,
            ).map_err(|e| CoreError::ComInit(e.to_string()))?;
            Ok(Self { automation })
        }
    }


    /// Find first element whose name contains `name` (case-insensitive).
    /// Slow path: walks the entire desktop tree. Prefer `find_by_name_fast`.
    pub fn find_by_name(&self, name: &str) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_name(&root, &name_lower)
        }
    }

    /// Find first element matching the given role name (e.g. "edit", "button").
    /// Slow path: walks the entire desktop tree. Prefer `find_by_role_fast`.
    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_role(&root, role)
        }
    }

    /// Scoped name search: walks only the subtree rooted at `hwnd`.
    pub fn find_by_name_in_hwnd(&self, hwnd: HWND, name: &str) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_name(&root, &name_lower)
        }
    }

    /// Scoped role search: walks only the subtree rooted at `hwnd`.
    pub fn find_by_role_in_hwnd(&self, hwnd: HWND, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_role(&root, role)
        }
    }

    /// Fast name search: tries foreground window subtree first, falls back to full desktop walk.
    /// Typical case (target is in the focused window) is 10-100x faster than `find_by_name`.
    pub fn find_by_name_fast(&self, name: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let fg = GetForegroundWindow();
            if !fg.is_invalid() {
                if let Ok(Some(el)) = self.find_by_name_in_hwnd(fg, name) {
                    return Ok(Some(el));
                }
            }
        }
        self.find_by_name(name)
    }

    /// Fast role search: tries foreground window subtree first, falls back to full desktop walk.
    pub fn find_by_role_fast(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let fg = GetForegroundWindow();
            if !fg.is_invalid() {
                if let Ok(Some(el)) = self.find_by_role_in_hwnd(fg, role) {
                    return Ok(Some(el));
                }
            }
        }
        self.find_by_role(role)
    }

    unsafe fn get_walker(&self) -> Result<IUIAutomationTreeWalker, CoreError> {
        self.automation.ControlViewWalker()
            .map_err(|e| CoreError::ComInit(e.to_string()))
    }

    unsafe fn search_subtree_by_name(
        &self,
        element: &IUIAutomationElement,
        name: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let el = UiaElement(element.clone());
        if el.name().to_lowercase().contains(name) {
            return Ok(Some(el));
        }
        let walker = self.get_walker()?;
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_subtree_by_name(&c, name)? {
                return Ok(Some(found));
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }

    unsafe fn search_subtree_by_role(
        &self,
        element: &IUIAutomationElement,
        role: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let el = UiaElement(element.clone());
        let el_role = role_id_to_name(el.control_type());
        if el_role == role || role_alias_matches(role, el_role) {
            return Ok(Some(el));
        }
        let walker = self.get_walker()?;
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_subtree_by_role(&c, role)? {
                return Ok(Some(found));
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }

    /// Fast describe: scoped to the foreground window subtree only.
    /// Typically 5-50x faster than `describe_screen(None)` since it skips the desktop walk.
    pub fn describe_screen_fast(&self) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_invalid() {
                return self.describe_screen(None);
            }
            let root = self.automation.ElementFromHandle(fg)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let mut results = Vec::new();
            self.collect_interactive(&root, &mut results, 0)?;
            Ok(results)
        }
    }

    /// Return structured list of interactive elements. Optionally scoped to a window by partial name.
    pub fn describe_screen(&self, window_name: Option<&str>) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            let root = if let Some(wname) = window_name {
                let wname_lower = wname.to_lowercase();
                let desktop = self.automation.GetRootElement()
                    .map_err(|e| CoreError::ComInit(e.to_string()))?;
                let walker = self.get_walker()?;
                let mut child = walker.GetFirstChildElement(&desktop).ok();
                let mut found = None;
                while let Some(c) = child {
                    let el = UiaElement(c.clone());
                    if el.name().to_lowercase().contains(&wname_lower) {
                        found = Some(c);
                        break;
                    }
                    child = walker.GetNextSiblingElement(&c).ok();
                }
                found.unwrap_or_else(|| self.automation.GetRootElement().unwrap())
            } else {
                self.automation.GetRootElement()
                    .map_err(|e| CoreError::ComInit(e.to_string()))?
            };
            let mut results = Vec::new();
            self.collect_interactive(&root, &mut results, 0)?;
            Ok(results)
        }
    }

    unsafe fn collect_interactive(
        &self,
        element: &IUIAutomationElement,
        results: &mut Vec<ElementDescriptor>,
        depth: usize,
    ) -> Result<(), CoreError> {
        if results.len() >= 500 || depth > 50 {
            return Ok(());
        }
        let el = UiaElement(element.clone());
        let role = role_id_to_name(el.control_type());
        if INTERACTIVE_ROLES.contains(&role) {
            let name = el.name();
            if !name.is_empty() {
                if let Some(rect) = el.bounding_rect() {
                    results.push(ElementDescriptor {
                        name,
                        role: role.to_string(),
                        left: rect.left,
                        top: rect.top,
                        right: rect.right,
                        bottom: rect.bottom,
                    });
                }
            }
        }
        let walker = self.get_walker()?;
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            self.collect_interactive(&c, results, depth + 1)?;
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub name: String,
    pub pid: u32,
    pub focused: bool,
    pub hwnd: *mut core::ffi::c_void,
}

pub enum WindowState {
    Maximize,
    Minimize,
    Restore,
    Close,
}

impl WindowState {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "maximize" => Some(Self::Maximize),
            "minimize" => Some(Self::Minimize),
            "restore" => Some(Self::Restore),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }
    let mut title = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut title);
    if len == 0 {
        return TRUE;
    }
    let name = String::from_utf16_lossy(&title[..len as usize]).to_string();
    let focused = GetForegroundWindow() == hwnd;
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    list.push(WindowInfo { name, pid, focused, hwnd: hwnd.0 });
    TRUE
}

pub fn list_windows() -> Result<Vec<WindowInfo>, CoreError> {
    let mut list: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_proc),
            LPARAM(&mut list as *mut Vec<WindowInfo> as isize),
        );
    }
    Ok(list)
}

/// Attempt to bring `hwnd` to the foreground using the AttachThreadInput workaround.
/// Returns Ok(true) if foreground confirmed within `timeout_ms`, Ok(false) if timed out.
///
/// MEDIUM-5 fixes:
/// (a) GetForegroundWindow() called once, value reused — eliminates the double-call TOCTOU.
/// (b) AttachThreadInput only called when thread IDs differ, and only detached for threads
///     that were actually attached — prevents the self-attach failure that ERROR_INVALID_PARAMETER.
pub fn ensure_foreground(hwnd: HWND, timeout_ms: u64) -> Result<bool, CoreError> {
    unsafe {
        // Single call to GetForegroundWindow — reuse throughout to avoid TOCTOU.
        let fg = GetForegroundWindow();
        if fg == hwnd {
            return Ok(true);
        }
        let cur_tid = GetCurrentThreadId();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let tgt_tid = GetWindowThreadProcessId(hwnd, None);

        // Only attach when IDs differ — AttachThreadInput fails (E_INVALIDARG) on self-attach.
        let attached_fg = fg_tid != cur_tid && fg_tid != 0;
        let attached_tgt = tgt_tid != cur_tid && tgt_tid != 0 && tgt_tid != fg_tid;

        if attached_fg {
            let _ = AttachThreadInput(cur_tid, fg_tid, TRUE);
        }
        if attached_tgt {
            let _ = AttachThreadInput(cur_tid, tgt_tid, TRUE);
        }

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);

        // Detach only threads we attached.
        if attached_fg {
            let _ = AttachThreadInput(cur_tid, fg_tid, FALSE);
        }
        if attached_tgt {
            let _ = AttachThreadInput(cur_tid, tgt_tid, FALSE);
        }

        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < timeout_ms as u128 {
            if GetForegroundWindow() == hwnd {
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        Ok(GetForegroundWindow() == hwnd)
    }
}

pub fn focus_window(name: &str) -> Result<(), CoreError> {
    let name_lower = name.to_lowercase();
    let windows = list_windows()?;
    let win = windows.iter()
        .find(|w| w.name.to_lowercase().contains(&name_lower))
        .ok_or_else(|| CoreError::ProcessNotFound { name: name.to_string() })?;
    let hwnd = HWND(win.hwnd);
    let confirmed = ensure_foreground(hwnd, 600)?;
    if !confirmed {
        return Err(CoreError::FocusFailed { window: name.to_string() });
    }
    Ok(())
}

pub fn set_window_state(name: &str, state: WindowState) -> Result<(), CoreError> {
    let name_lower = name.to_lowercase();
    let windows = list_windows()?;
    let win = windows.iter()
        .find(|w| w.name.to_lowercase().contains(&name_lower))
        .ok_or_else(|| CoreError::ProcessNotFound { name: name.to_string() })?;
    let hwnd = HWND(win.hwnd);
    unsafe {
        match state {
            WindowState::Maximize => { let _ = ShowWindow(hwnd, SW_MAXIMIZE); }
            WindowState::Minimize => { let _ = ShowWindow(hwnd, SW_MINIMIZE); }
            WindowState::Restore => { let _ = ShowWindow(hwnd, SW_RESTORE); }
            WindowState::Close => {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_from_str_parses_all_variants() {
        assert!(matches!(WindowState::from_str("maximize"), Some(WindowState::Maximize)));
        assert!(matches!(WindowState::from_str("minimize"), Some(WindowState::Minimize)));
        assert!(matches!(WindowState::from_str("restore"), Some(WindowState::Restore)));
        assert!(matches!(WindowState::from_str("close"), Some(WindowState::Close)));
        assert!(WindowState::from_str("invalid").is_none());
    }

    #[test]
    fn focus_window_name_not_found_returns_process_not_found() {
        let result = focus_window("__ghost_nonexistent_window_xyzzy__");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CoreError::ProcessNotFound { .. }),
            "expected ProcessNotFound, got: {:?}", err);
    }

    #[test]
    fn role_alias_tab_matches_tabitem() {
        assert!(role_alias_matches("tab", "tabitem"));
        assert!(!role_alias_matches("tab", "button"));
    }

    #[test]
    fn role_alias_list_matches_listitem() {
        assert!(role_alias_matches("list", "listitem"));
        assert!(!role_alias_matches("list", "menu"));
    }

    // LOW-9: ensure_foreground must not panic on the current foreground window.
    // Marked ignore because it requires a live desktop session.
    #[test]
    #[ignore]
    fn ensure_foreground_current_fg_returns_ok_true() {
        // Fetch the current foreground window; ensure_foreground should return Ok(true) immediately.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            // No foreground window (headless CI) — skip rather than fail.
            return;
        }
        let result = ensure_foreground(hwnd, 0);
        assert!(result.is_ok(), "ensure_foreground must not panic or error: {:?}", result);
        assert_eq!(result.unwrap(), true, "already-foreground window should return Ok(true)");
    }
}
