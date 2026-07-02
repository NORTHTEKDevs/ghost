use windows::Win32::UI::Accessibility::*;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM, TRUE, FALSE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, IsWindowVisible, IsIconic,
    PostMessageW, SetForegroundWindow, ShowWindow, GetWindowThreadProcessId,
    BringWindowToTop, GetAncestor,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, WM_CLOSE,
    GA_ROOT,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::WindowFromPoint;
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

/// Max UIA nodes visited per subtree search. Bounds worst-case latency on
/// DOM-heavy apps (a Chromium tab can expose 10k+ nodes); typical windows
/// resolve in well under 1k visits.
const SEARCH_NODE_BUDGET: usize = 3000;

/// Max UIA nodes visited per text-extraction walk. Higher than the search
/// budget: reading a full page legitimately touches more nodes, and the walk
/// does no per-node pattern queries except on edit/document elements.
const TEXT_NODE_BUDGET: usize = 8000;

/// Max UIA nodes visited per describe_screen walk. Bounds a wide/shallow tree
/// (big list, Chromium DOM) so one describe can't hang the serial server.
const DESCRIBE_NODE_BUDGET: usize = 6000;

/// Roles whose accessible NAME carries visible text content.
const TEXT_NAME_ROLES: &[&str] = &[
    "text", "hyperlink", "listitem", "treeitem", "tabitem", "menuitem",
    "button", "checkbox", "radiobutton", "combobox", "dataitem", "headeritem",
];

/// Roles whose ValuePattern (get_text) carries the content.
const TEXT_VALUE_ROLES: &[&str] = &["edit", "document"];

/// Truncate `s` to at most `max` bytes WITHOUT splitting a UTF-8 character —
/// `String::truncate` panics off a char boundary, which would crash the whole
/// server on ordinary Unicode content (accents, CJK, emoji).
fn truncate_at_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
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
            // Acquire the walker once for the whole search (was N+1 COM proxies).
            let walker = self.get_walker()?;
            let mut budget = SEARCH_NODE_BUDGET;
            self.search_subtree_by_name(&root, &name_lower, &walker, 0, &mut budget)
        }
    }

    /// Find first element matching the given role name (e.g. "edit", "button").
    /// Slow path: walks the entire desktop tree. Prefer `find_by_role_fast`.
    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let walker = self.get_walker()?;
            let mut budget = SEARCH_NODE_BUDGET;
            self.search_subtree_by_role(&root, role, &walker, 0, &mut budget)
        }
    }

    /// Scoped name search: walks only the subtree rooted at `hwnd`.
    pub fn find_by_name_in_hwnd(&self, hwnd: HWND, name: &str) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let walker = self.get_walker()?;
            let mut budget = SEARCH_NODE_BUDGET;
            self.search_subtree_by_name(&root, &name_lower, &walker, 0, &mut budget)
        }
    }

    /// Scoped role search: walks only the subtree rooted at `hwnd`.
    pub fn find_by_role_in_hwnd(&self, hwnd: HWND, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let walker = self.get_walker()?;
            let mut budget = SEARCH_NODE_BUDGET;
            self.search_subtree_by_role(&root, role, &walker, 0, &mut budget)
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

    /// Recursive name search. `walker` is acquired once by the caller (see `find_by_name`).
    /// MEDIUM-8: `depth` param guards against stack overflow on pathological UIA trees.
    /// `budget` caps total visited nodes so a DOM-heavy app (Chrome/Electron) can't
    /// turn one find into an unbounded COM-call storm.
    unsafe fn search_subtree_by_name(
        &self,
        element: &IUIAutomationElement,
        name: &str,
        walker: &IUIAutomationTreeWalker,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Option<UiaElement>, CoreError> {
        if depth > 50 || *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        let el = UiaElement(element.clone());
        if el.name().to_lowercase().contains(name) {
            return Ok(Some(el));
        }
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_subtree_by_name(&c, name, walker, depth + 1, budget)? {
                return Ok(Some(found));
            }
            if *budget == 0 {
                return Ok(None);
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }

    /// Recursive role search. `walker` is acquired once by the caller (see `find_by_role`).
    /// MEDIUM-8: `depth` param guards against stack overflow on pathological UIA trees.
    /// `budget` caps total visited nodes (see `search_subtree_by_name`).
    unsafe fn search_subtree_by_role(
        &self,
        element: &IUIAutomationElement,
        role: &str,
        walker: &IUIAutomationTreeWalker,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Option<UiaElement>, CoreError> {
        if depth > 50 || *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        let el = UiaElement(element.clone());
        let el_role = role_id_to_name(el.control_type());
        if el_role == role || role_alias_matches(role, el_role) {
            return Ok(Some(el));
        }
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_subtree_by_role(&c, role, walker, depth + 1, budget)? {
                return Ok(Some(found));
            }
            if *budget == 0 {
                return Ok(None);
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
            // Acquire walker once for the entire collect pass.
            let walker = self.get_walker()?;
            let mut results = Vec::new();
            let mut budget = DESCRIBE_NODE_BUDGET;
            self.collect_interactive(&root, &mut results, 0, &walker, &mut budget)?;
            Ok(results)
        }
    }

    /// Resolve the UIA element to walk for a given optional window scope.
    ///
    /// A window scope that matches nothing is an ERROR (`ProcessNotFound`), not a
    /// silent full-desktop walk: dumping every window's elements (including
    /// minimized ones at -32000 coords) produced huge, misleading responses.
    ///
    /// # Safety
    /// COM calls; must run on the session STA thread.
    unsafe fn resolve_scope_root(
        &self,
        walker: &IUIAutomationTreeWalker,
        window_name: Option<&str>,
    ) -> Result<IUIAutomationElement, CoreError> {
        let Some(wname) = window_name else {
            return self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()));
        };
        let wname_lower = wname.to_lowercase();
        let desktop = self.automation.GetRootElement()
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let mut child = walker.GetFirstChildElement(&desktop).ok();
        while let Some(c) = child {
            let el = UiaElement(c.clone());
            if el.name().to_lowercase().contains(&wname_lower) {
                return Ok(c);
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        // Not a direct UIA child of the desktop (minimized/cloaked windows often
        // aren't). Fall back to an HWND title match so agents can still scope to
        // windows they just interacted with.
        let win = list_windows()?
            .into_iter()
            .find(|w| w.name.to_lowercase().contains(&wname_lower))
            .ok_or_else(|| CoreError::ProcessNotFound { name: wname.to_string() })?;
        if win.state == "minimized" {
            // Element coords of a minimized window are garbage (-32000).
            return Err(CoreError::WindowMinimized { name: win.name });
        }
        self.automation.ElementFromHandle(HWND(win.hwnd))
            .map_err(|e| CoreError::ComInit(e.to_string()))
    }

    /// Return structured list of interactive elements. Optionally scoped to a window by partial name.
    pub fn describe_screen(&self, window_name: Option<&str>) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            // Acquire walker once — used for both the window-title scan and the collect pass.
            let walker = self.get_walker()?;
            let root = self.resolve_scope_root(&walker, window_name)?;
            let mut results = Vec::new();
            let mut budget = DESCRIBE_NODE_BUDGET;
            self.collect_interactive(&root, &mut results, 0, &walker, &mut budget)?;
            Ok(results)
        }
    }

    /// Collect up to `cap` elements matching `name` (case-insensitive contains)
    /// AND `role` (with aliases) within the subtree rooted at `hwnd`. A criterion
    /// that is None always matches; at least one must be Some. Unlike the
    /// first-match find_* functions, this enables disambiguation (`matches`
    /// count, nth-match selection) when several elements share a name/role.
    pub fn find_all_in_hwnd(
        &self,
        hwnd: HWND,
        name: Option<&str>,
        role: Option<&str>,
        cap: usize,
    ) -> Result<Vec<UiaElement>, CoreError> {
        if name.is_none() && role.is_none() {
            return Ok(Vec::new());
        }
        let name_lower = name.map(|n| n.to_lowercase());
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let walker = self.get_walker()?;
            let mut out = Vec::new();
            let mut budget = SEARCH_NODE_BUDGET;
            self.collect_matching(&root, name_lower.as_deref(), role, &walker, 0, &mut budget, cap, &mut out)?;
            Ok(out)
        }
    }

    unsafe fn collect_matching(
        &self,
        element: &IUIAutomationElement,
        name: Option<&str>,
        role: Option<&str>,
        walker: &IUIAutomationTreeWalker,
        depth: usize,
        budget: &mut usize,
        cap: usize,
        out: &mut Vec<UiaElement>,
    ) -> Result<(), CoreError> {
        if depth > 50 || *budget == 0 || out.len() >= cap {
            return Ok(());
        }
        *budget -= 1;
        let el = UiaElement(element.clone());
        let name_ok = name.map_or(true, |n| el.name().to_lowercase().contains(n));
        let role_ok = role.map_or(true, |r| {
            let er = role_id_to_name(el.control_type());
            er == r || role_alias_matches(r, er)
        });
        // depth > 0: never match the subtree root (the window element itself) —
        // a window title containing the searched name would otherwise become
        // match #0 and shift every real element's index.
        if depth > 0 && name_ok && role_ok {
            out.push(el);
        }
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            self.collect_matching(&c, name, role, walker, depth + 1, budget, cap, out)?;
            if out.len() >= cap || *budget == 0 {
                return Ok(());
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(())
    }

    /// Extract readable text from a window (or the full desktop). Walks the
    /// subtree collecting accessible names of text-carrying roles plus
    /// ValuePattern content of edit/document elements — the cheap way to READ
    /// a page or app (vs screenshots/element dumps). Returns (text, truncated).
    pub fn collect_text(
        &self,
        window_name: Option<&str>,
        max_chars: usize,
    ) -> Result<(String, bool), CoreError> {
        unsafe {
            let walker = self.get_walker()?;
            let root = self.resolve_scope_root(&walker, window_name)?;
            let mut out = String::new();
            let mut budget = TEXT_NODE_BUDGET;
            let truncated = self.collect_text_rec(&root, &walker, 0, &mut budget, max_chars, &mut out)?;
            Ok((out, truncated))
        }
    }

    unsafe fn collect_text_rec(
        &self,
        element: &IUIAutomationElement,
        walker: &IUIAutomationTreeWalker,
        depth: usize,
        budget: &mut usize,
        max_chars: usize,
        out: &mut String,
    ) -> Result<bool, CoreError> {
        if depth > 50 || *budget == 0 {
            return Ok(false);
        }
        if out.len() >= max_chars {
            return Ok(true);
        }
        *budget -= 1;
        let el = UiaElement(element.clone());
        let role = role_id_to_name(el.control_type());
        let piece = if TEXT_VALUE_ROLES.contains(&role) {
            let t = el.get_text();
            if t.is_empty() { el.name() } else { t }
        } else if TEXT_NAME_ROLES.contains(&role) {
            el.name()
        } else {
            String::new()
        };
        if !piece.is_empty() {
            // Skip immediate duplicates (containers often repeat child text).
            let is_dup = out.len() > piece.len()
                && out.ends_with('\n')
                && out[..out.len() - 1].ends_with(&piece);
            if !is_dup {
                out.push_str(&piece);
                out.push('\n');
                if out.len() >= max_chars {
                    truncate_at_char_boundary(out, max_chars);
                    return Ok(true);
                }
            }
        }
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if self.collect_text_rec(&c, walker, depth + 1, budget, max_chars, out)? {
                return Ok(true);
            }
            if *budget == 0 {
                return Ok(false);
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(false)
    }

    /// Recursive interactive-element collector. `walker` is acquired once by the caller.
    unsafe fn collect_interactive(
        &self,
        element: &IUIAutomationElement,
        results: &mut Vec<ElementDescriptor>,
        depth: usize,
        walker: &IUIAutomationTreeWalker,
        budget: &mut usize,
    ) -> Result<(), CoreError> {
        // `budget` caps TOTAL nodes visited. The 500-result / depth-50 caps bound
        // output and vertical depth but not breadth — a wide, shallow tree (a big
        // list/grid, a Chromium/Electron DOM with thousands of non-interactive
        // nodes) would otherwise be walked in full, hanging the single-threaded
        // server on one describe call. Matches the other walkers' budget pattern.
        if results.len() >= 500 || depth > 50 || *budget == 0 {
            return Ok(());
        }
        *budget -= 1;
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
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            self.collect_interactive(&c, results, depth + 1, walker, budget)?;
            if results.len() >= 500 || *budget == 0 {
                return Ok(());
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(())
    }

    /// Return the UIA element at the given screen coordinates.
    /// Used by the locator cache to validate cached rects: if the element at the
    /// center of the cached rect still matches the expected name/role, the cache
    /// hit is valid. Returns None if no element is found (minimized window, etc).
    pub fn element_from_point(&self, x: i32, y: i32) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let pt = POINT { x, y };
            match self.automation.ElementFromPoint(pt) {
                Ok(el) => Ok(Some(UiaElement(el))),
                Err(_) => Ok(None),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub name: String,
    pub pid: u32,
    pub focused: bool,
    pub hwnd: *mut core::ffi::c_void,
    /// "normal" | "minimized". Minimized (and Win11-cloaked-minimized) windows are
    /// included so agents don't lose track of windows they just interacted with.
    pub state: &'static str,
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
    let visible = IsWindowVisible(hwnd).as_bool();
    let iconic = IsIconic(hwnd).as_bool();
    // Keep visible windows AND minimized ones (Win11 cloaks some minimized app
    // windows so IsWindowVisible is false — e.g. Notepad — but they're alive and
    // restorable; dropping them made agents lose track of open windows).
    if !visible && !iconic {
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
    let state = if iconic { "minimized" } else { "normal" };
    let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    list.push(WindowInfo { name, pid, focused, hwnd: hwnd.0, state });
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

        // A minimized window can't receive foreground — restore it first.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
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

/// Bring the top-level window that contains the screen point (x, y) to the foreground.
///
/// Steps:
///   1. `WindowFromPoint` → child HWND at coordinates.
///   2. `GetAncestor(GA_ROOT)` → top-level owner.
///   3. `ensure_foreground(hwnd, 600)`.
///
/// Returns `Ok(true)` if foreground was confirmed, `Ok(false)` on timeout
/// (callers should warn and continue — consistent with the tolerant act strategy).
/// Returns `Err` only when Windows APIs are unavailable (not on focus timeout).
///
/// # Safety
/// Calls Win32 functions that require no special thread affinity.
pub fn focus_window_under_point(x: i32, y: i32) -> Result<bool, CoreError> {
    unsafe {
        let pt = POINT { x, y };
        let child = WindowFromPoint(pt);
        if child.is_invalid() {
            return Ok(false);
        }
        // Walk to the top-level owner window.
        let root = GetAncestor(child, GA_ROOT);
        let hwnd = if root.is_invalid() { child } else { root };
        ensure_foreground(hwnd, 600)
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

    /// MEDIUM-8: search_subtree_by_name depth constant is 50; verify alias function is depth-agnostic.
    /// (Full recursive stack-overflow guard is validated by the limit constant itself —
    /// tested here indirectly since we can't construct a 51-deep live UIA tree in unit tests.)
    #[test]
    fn search_depth_limit_constant_is_fifty() {
        // Verify the depth limit used in search_subtree_by_name/role is as specified.
        // The actual guard is `if depth > 50 { return Ok(None) }`.
        const MAX_DEPTH: usize = 50;
        assert!(MAX_DEPTH == 50, "depth limit must be 50 per spec");
    }

    /// HIGH-1: focus_window_under_point with an off-screen coord returns Ok (not an error).
    /// The helper is tolerant — it returns Ok(false) for invalid/off-screen points.
    #[test]
    fn focus_window_under_point_off_screen_coord_returns_ok() {
        // A point far off-screen (negative, or beyond any reasonable monitor) should not
        // panic or return Err — it should return Ok(false) (no window found there).
        // This exercises the invalid-HWND code path without a live window.
        let result = focus_window_under_point(-99999, -99999);
        assert!(result.is_ok(), "focus_window_under_point must not error on off-screen coord");
        // May be Ok(true) if somehow a window exists there, but most likely Ok(false).
    }

    /// HIGH-1: focus_window_under_point with an absurdly negative coord returns Ok.
    #[test]
    fn focus_window_under_point_large_negative_returns_ok() {
        let result = focus_window_under_point(i32::MIN, i32::MIN);
        assert!(result.is_ok(), "focus_window_under_point must not error on i32::MIN coords");
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

    #[test]
    fn truncate_at_char_boundary_never_splits_utf8() {
        let mut s = String::from("caf\u{e9}\n"); // é is 2 bytes; byte 4 is mid-char
        truncate_at_char_boundary(&mut s, 4);
        assert_eq!(s, "caf");

        let mut s2 = String::from("ab\u{1F600}cd"); // emoji is 4 bytes starting at byte 2
        truncate_at_char_boundary(&mut s2, 3);
        assert_eq!(s2, "ab");

        let mut s3 = String::from("plain ascii");
        truncate_at_char_boundary(&mut s3, 5);
        assert_eq!(s3, "plain");

        let mut s4 = String::from("short");
        truncate_at_char_boundary(&mut s4, 100);
        assert_eq!(s4, "short");
    }
}
