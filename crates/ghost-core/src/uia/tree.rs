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
use crate::focus;

pub struct UiaTree {
    automation: IUIAutomation,
    /// Batches the properties every search reads into one server-side fetch, so
    /// matching happens against cached values instead of a COM call per property per
    /// element.
    cache_request: IUIAutomationCacheRequest,
    /// Created once. `ControlViewWalker()` is itself a cross-process call, and the
    /// original code invoked it at every node of every recursive walk.
    walker: IUIAutomationTreeWalker,
}

// Safety: ghost initializes COM with COINIT_MULTITHREADED (`init_com`), and the UIA
// client documentation states that in the multithreaded apartment its objects may be
// called from any thread. AddRef/Release are thread-safe, and cross-thread calls do
// not require marshalling in the MTA. The same justification already covers the
// cached D3D11 device in `capture::screen`. This is what lets one `GhostSession` be
// shared across concurrently executing MCP requests.
unsafe impl Send for UiaTree {}
unsafe impl Sync for UiaTree {}

/// Properties every search and description needs. Fetching them in one batch is the
/// difference between one IPC and four per element.
const CACHED_PROPERTIES: &[UIA_PROPERTY_ID] = &[
    UIA_NamePropertyId,
    UIA_ControlTypePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_AcceleratorKeyPropertyId,
    UIA_AutomationIdPropertyId,
];

impl UiaTree {
    pub fn new() -> Result<Self, CoreError> {
        unsafe {
            let automation: IUIAutomation = CoCreateInstance(
                &CUIAutomation8,
                None,
                CLSCTX_INPROC_SERVER,
            ).map_err(|e| CoreError::ComInit(e.to_string()))?;
            let cache_request = automation
                .CreateCacheRequest()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            for pid in CACHED_PROPERTIES {
                cache_request
                    .AddProperty(*pid)
                    .map_err(|e| CoreError::ComInit(e.to_string()))?;
            }
            // Full mode keeps the returned elements live, so a cached search result
            // can still be acted on with control patterns rather than only read.
            cache_request
                .SetAutomationElementMode(AutomationElementMode_Full)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let walker = automation
                .ControlViewWalker()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            Ok(Self { automation, cache_request, walker })
        }
    }

    /// Every element in `root`'s subtree, with `CACHED_PROPERTIES` already fetched.
    ///
    /// One `FindAllBuildCache` call does the whole traversal inside UI Automation and
    /// returns the batch. The previous approach - recursing in this process, calling
    /// COM for each child and each property - made the same work cost thousands of
    /// cross-process round trips.
    unsafe fn cached_subtree(
        &self,
        root: &IUIAutomationElement,
    ) -> Result<Vec<UiaElement>, CoreError> {
        let condition = self
            .automation
            .CreateTrueCondition()
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let found = root
            .FindAllBuildCache(TreeScope_Subtree, &condition, &self.cache_request)
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let len = found.Length().unwrap_or(0);
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            if let Ok(el) = found.GetElement(i) {
                out.push(UiaElement(el));
            }
        }
        Ok(out)
    }

    /// Find first element whose name contains `name` (case-insensitive), anywhere on
    /// the desktop.
    ///
    /// Prefer `find_by_name_in` for background work: an unscoped search walks every
    /// window on the machine and will happily return an element out of whatever the
    /// user has open, or out of another ghost process's target.
    pub fn find_by_name(&self, name: &str) -> Result<Option<UiaElement>, CoreError> {
        self.find_by_name_in(None, name)
    }

    /// Find first element matching the given role name (e.g. "edit", "button").
    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        self.find_by_role_in(None, role)
    }

    /// Name search scoped to one top-level window.
    pub fn find_by_name_in(
        &self,
        window_name: Option<&str>,
        name: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.scope_root_strict(window_name)?;
            // Substring matching cannot be expressed as a UIA property condition, so
            // the subtree comes back in one batch and the filter runs here - still a
            // single round trip rather than one per element.
            Ok(self
                .cached_subtree(&root)?
                .into_iter()
                .find(|el| el.cached_name().to_lowercase().contains(&name_lower)))
        }
    }

    /// Role search scoped to one top-level window.
    pub fn find_by_role_in(
        &self,
        window_name: Option<&str>,
        role: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.scope_root_strict(window_name)?;
            // A control type is an exact property match, so UI Automation can do the
            // search itself and return only the first hit.
            let Some(id) = super::element::role_name_to_id(role) else {
                return Ok(None);
            };
            let value = windows::core::VARIANT::from(id as i32);
            let condition = self
                .automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &value)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            match root.FindFirstBuildCache(TreeScope_Subtree, &condition, &self.cache_request) {
                Ok(el) => Ok(Some(UiaElement(el))),
                // FindFirst reports "no match" as an error rather than an empty
                // result, so a miss must not propagate as a failure.
                Err(_) => Ok(None),
            }
        }
    }

    /// Like `scope_root`, but a named window that does not exist is an error rather
    /// than a silent widening to the whole desktop.
    ///
    /// Falling back to the desktop is the wrong default for a scoped search: the
    /// caller asked for "the edit box in *this* window" and would instead get some
    /// unrelated app's edit box, then type into it.
    unsafe fn scope_root_strict(
        &self,
        window_name: Option<&str>,
    ) -> Result<IUIAutomationElement, CoreError> {
        let Some(wname) = window_name else {
            return self
                .automation
                .GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()));
        };
        self.top_level_window(wname)?
            .ok_or_else(|| CoreError::ProcessNotFound { name: wname.to_string() })
    }

    /// The first top-level window whose name contains `needle`, case-insensitively.
    ///
    /// Walks the desktop's children and stops at the first match. A batched
    /// `FindAllBuildCache` was tried here and is markedly slower (49ms vs 4ms): it has
    /// to contact every top-level window's provider before returning anything, and on
    /// a real desktop several of those are browsers and Electron apps that answer
    /// slowly. Enumerating lazily and short-circuiting wins, so the batching stays
    /// where it pays - fetching a matched window's whole subtree.
    unsafe fn top_level_window(
        &self,
        needle: &str,
    ) -> Result<Option<IUIAutomationElement>, CoreError> {
        let desktop = self
            .automation
            .GetRootElement()
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let needle = needle.to_lowercase();
        let mut child = self.walker.GetFirstChildElement(&desktop).ok();
        while let Some(c) = child {
            if UiaElement(c.clone()).name().to_lowercase().contains(&needle) {
                return Ok(Some(c));
            }
            child = self.walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }

    /// Find the command that owns a keyboard shortcut, e.g. "Ctrl+Z" -> Edit > Undo.
    ///
    /// Window messages cannot express a modifier combo: posting WM_KEYDOWN for
    /// VK_CONTROL does not change the target thread's key state, so the app's own
    /// message loop translates the following keystroke into a plain character - a
    /// background "Ctrl+Z" arrives as the letter "z" and corrupts the document.
    /// Invoking the accelerator's command element performs the real action instead.
    pub fn find_by_accelerator(
        &self,
        window_name: Option<&str>,
        accelerator: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let wanted = normalize_accelerator(accelerator);
        if wanted.is_empty() {
            return Ok(None);
        }
        unsafe {
            let root = self.scope_root(window_name)?;
            Ok(self.cached_subtree(&root)?.into_iter().find(|el| {
                normalize_accelerator(&el.cached_accelerator_key()) == wanted
            }))
        }
    }

    /// Root element for a search: a named top-level window, or the desktop.
    unsafe fn scope_root(&self, window_name: Option<&str>) -> Result<IUIAutomationElement, CoreError> {
        if let Some(wname) = window_name {
            if let Some(found) = self.top_level_window(wname)? {
                return Ok(found);
            }
        }
        self.automation
            .GetRootElement()
            .map_err(|e| CoreError::ComInit(e.to_string()))
    }

    /// Return structured list of interactive elements. Optionally scoped to a window by partial name.
    pub fn describe_screen(&self, window_name: Option<&str>) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            let root = self.scope_root(window_name)?;
            let mut results = Vec::new();
            for el in self.cached_subtree(&root)? {
                if results.len() >= 500 {
                    break;
                }
                let role = role_id_to_name(el.cached_control_type());
                if !INTERACTIVE_ROLES.contains(&role) {
                    continue;
                }
                let name = el.cached_name();
                if name.is_empty() {
                    continue;
                }
                if let Some(rect) = el.cached_bounding_rect() {
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
            Ok(results)
        }
    }

}

/// Canonical form of an accelerator string so "Ctrl+Z", "CTRL + Z", and
/// "Control+Z" all compare equal. Modifier order is normalized too, because apps are
/// inconsistent about whether they write "Ctrl+Shift+S" or "Shift+Ctrl+S".
pub fn normalize_accelerator(s: &str) -> String {
    let mut modifiers: Vec<&str> = Vec::new();
    let mut key = String::new();
    for part in s.split('+') {
        let p = part.trim().to_lowercase();
        if p.is_empty() {
            continue;
        }
        match p.as_str() {
            "ctrl" | "control" => modifiers.push("ctrl"),
            "shift" => modifiers.push("shift"),
            "alt" | "menu" => modifiers.push("alt"),
            "win" | "meta" | "cmd" => modifiers.push("win"),
            _ => key = p,
        }
    }
    if key.is_empty() {
        return String::new();
    }
    // Fixed modifier order, deduplicated.
    let mut ordered: Vec<&str> = ["ctrl", "alt", "shift", "win"]
        .into_iter()
        .filter(|m| modifiers.contains(m))
        .collect();
    ordered.push(&key);
    ordered.join("+")
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

/// Bring a window to the foreground. This is by definition a screen-stealing action,
/// so it is gated by the focus policy: background automation must target windows by
/// handle instead of activating them.
pub fn focus_window(name: &str) -> Result<(), CoreError> {
    focus::require_foreground_allowed("focus_window")?;
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
    // Maximize and restore both raise the window in front of whatever the user is
    // doing; minimize and close do not.
    if matches!(state, WindowState::Maximize | WindowState::Restore) {
        focus::require_foreground_allowed("window_state")?;
    }
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
    fn accelerator_normalization_ignores_case_spacing_and_synonyms() {
        assert_eq!(normalize_accelerator("Ctrl+Z"), "ctrl+z");
        assert_eq!(normalize_accelerator("CTRL + Z"), "ctrl+z");
        assert_eq!(normalize_accelerator("Control+Z"), "ctrl+z");
    }

    #[test]
    fn accelerator_normalization_is_order_independent() {
        // Apps write the same shortcut both ways; a lookup must match either.
        assert_eq!(
            normalize_accelerator("Ctrl+Shift+S"),
            normalize_accelerator("Shift+Ctrl+S")
        );
        assert_eq!(normalize_accelerator("Ctrl+Shift+S"), "ctrl+shift+s");
        assert_eq!(normalize_accelerator("Alt+Ctrl+Del"), "ctrl+alt+del");
    }

    #[test]
    fn a_bare_key_normalizes_to_itself() {
        assert_eq!(normalize_accelerator("F5"), "f5");
        assert_eq!(normalize_accelerator("Delete"), "delete");
    }

    #[test]
    fn modifier_only_or_empty_accelerators_are_rejected() {
        // An element with an empty AcceleratorKey must not match a real lookup, or
        // every search would return the first element it walked.
        assert_eq!(normalize_accelerator(""), "");
        assert_eq!(normalize_accelerator("   "), "");
        assert_eq!(normalize_accelerator("Ctrl"), "");
        assert_eq!(normalize_accelerator("Ctrl+"), "");
    }

    #[test]
    fn window_state_from_str_parses_all_variants() {
        assert!(matches!(WindowState::from_str("maximize"), Some(WindowState::Maximize)));
        assert!(matches!(WindowState::from_str("minimize"), Some(WindowState::Minimize)));
        assert!(matches!(WindowState::from_str("restore"), Some(WindowState::Restore)));
        assert!(matches!(WindowState::from_str("close"), Some(WindowState::Close)));
        assert!(WindowState::from_str("invalid").is_none());
    }
}
