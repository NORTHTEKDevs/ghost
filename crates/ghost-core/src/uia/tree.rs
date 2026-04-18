use windows::Win32::UI::Accessibility::*;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, IsWindowVisible,
    PostMessageW, SetForegroundWindow, ShowWindow, GetWindowThreadProcessId,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
};
use super::element::{UiaElement, role_id_to_name, ElementDescriptor, INTERACTIVE_ROLES};
use crate::error::CoreError;

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
    pub fn find_by_name(&self, name: &str) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_name(&root, &name_lower)
        }
    }

    /// Find first element matching the given role name (e.g. "edit", "button").
    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree_by_role(&root, role)
        }
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
        if role_id_to_name(el.control_type()) == role {
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

pub fn focus_window(name: &str) -> Result<(), CoreError> {
    let name_lower = name.to_lowercase();
    let windows = list_windows()?;
    let win = windows.iter()
        .find(|w| w.name.to_lowercase().contains(&name_lower))
        .ok_or_else(|| CoreError::ProcessNotFound { name: name.to_string() })?;
    unsafe {
        let _ = SetForegroundWindow(HWND(win.hwnd));
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
}
