use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use async_trait::async_trait;
use ghost_cache::uia_mirror::{UiaCache, SnapshotDelta, Snapshot, CacheStats};
use ghost_intent::compiler::{CompiledIntent, IntentCompiler, Op};
use ghost_intent::error::IntentError;
use ghost_intent::executor::{FsmExecutor, IntentResult, IntentState, OpsDispatcher};
use ghost_core::capture::idle::IdleDetector;
use ghost_core::{
    capture::capture_screen,
    input::hotkey::{register_emergency_stop, is_stopped, reset_stop},
    input::keyboard::{key_down as core_key_down, key_up as core_key_up, name_to_vk, press_key},
    input::mouse::{
        hover as core_hover, right_click as core_right_click,
        double_click as core_double_click, drag as core_drag, scroll as core_scroll,
    },
    process::launch as proc_launch,
    system::{get_clipboard as core_get_clipboard, set_clipboard as core_set_clipboard},
    uia::{
        init_com,
        tree::{UiaTree, WindowInfo, WindowState, list_windows as core_list_windows,
               focus_window as core_focus_window, set_window_state},
    },
};
use crate::{
    locator::By,
    error::{GhostError, Result},
};

pub struct Region;

impl Region {
    pub fn full() -> Self {
        Region
    }
}

pub struct GhostSession {
    timeout_ms: u64,
    tree: UiaTree,
    cache: Arc<UiaCache>,
}

impl GhostSession {
    /// Create a new automation session.
    /// Initializes COM, registers the Ctrl+Alt+G emergency stop hotkey, and creates the UIA tree.
    pub fn new() -> Result<Self> {
        init_com().map_err(GhostError::Core)?;
        register_emergency_stop().map_err(GhostError::Core)?;
        let tree = UiaTree::new().map_err(GhostError::Core)?;
        Ok(Self {
            timeout_ms: 5000,
            tree,
            cache: Arc::new(UiaCache::new()),
        })
    }

    /// Return a structural delta between the current screen snapshot and `since_seq`.
    /// Pass `since_seq = None` to get the full current snapshot as a delta.
    pub async fn describe_screen_delta(
        &self,
        window: Option<&str>,
        since_seq: Option<u64>,
    ) -> Result<SnapshotDelta> {
        self.cache.snapshot_delta(window, since_seq).map_err(Into::into)
    }

    /// Return cache statistics (snapshots served, history hit rate, etc).
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Invalidate the UIA cache. Next describe_screen_delta returns a full snapshot.
    pub fn cache_invalidate(&self) {
        self.cache.invalidate();
    }

    /// Poll `condition` (a JSONLogic expression) against session state every `poll_ms`
    /// until it evaluates true or `timeout_ms` elapses.
    ///
    /// State exposed to the condition: `{ "cache_seq": u64, "last_error": Option<String> }`.
    pub async fn wait_until(
        &self,
        condition: serde_json::Value,
        timeout_ms: u64,
        poll_ms: u64,
    ) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let poll = Duration::from_millis(poll_ms.max(10));
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            let state = serde_json::json!({
                "cache_seq": self.cache.seq(),
                "last_error": serde_json::Value::Null,
            });
            let v = ghost_intent::jsonlogic::eval(&condition, &state)
                .map_err(GhostError::from)?;
            if v.as_bool() == Some(true) {
                return Ok(());
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout { action: "wait_until".into(), ms: timeout_ms });
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// Wait for the screen to settle: `stable_frames` consecutive identical captures.
    /// `window` is currently informational; DXGI duplication is full-desktop.
    pub async fn wait_for_idle(
        &self,
        _window: Option<&str>,
        stable_frames: u32,
        timeout_ms: u64,
    ) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let detector = IdleDetector::new().map_err(GhostError::Core)?;
        detector.wait_stable(stable_frames, timeout_ms).await.map_err(GhostError::Core)
    }

    /// Apply a freshly walked snapshot into the cache. Used by walker-driven refresh paths.
    pub fn apply_snapshot(&self, snap: Snapshot) {
        self.cache.apply_snapshot(snap);
    }

    /// Override the per-action timeout (default: 5000ms).
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Find the first element matching the locator, retrying until timeout.
    pub async fn find(&self, by: By) -> Result<crate::GhostElement> {
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        let action = by.to_string();
        let ms = self.timeout_ms;

        let result = timeout(Duration::from_millis(ms), async {
            loop {
                if is_stopped() {
                    return Err(GhostError::Stopped);
                }
                let found = match &by {
                    By::Name(n) => self.tree.find_by_name(n).map_err(GhostError::Core)?,
                    By::Role(r) => self.tree.find_by_role(r).map_err(GhostError::Core)?,
                };
                if let Some(el) = found {
                    return Ok(crate::GhostElement::new(el));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        match result {
            Ok(r) => r,
            Err(_elapsed) => {
                let screenshot = capture_screen().ok();
                Err(GhostError::ElementNotFound {
                    query: action,
                    screenshot,
                })
            }
        }
    }

    /// Click at absolute pixel coordinates without finding an element.
    pub async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        ghost_core::input::mouse::click(x, y).map_err(GhostError::Core)
    }

    /// Capture the primary monitor as PNG bytes.
    pub async fn screenshot(&self, _region: Region) -> Result<Vec<u8>> {
        capture_screen().map_err(GhostError::Core)
    }

    /// Launch a process by name or path. Returns PID.
    pub async fn launch(&self, exe: &str) -> Result<u32> {
        proc_launch(exe).map_err(GhostError::Core)
    }

    /// Trigger emergency stop: halts all automation, releases modifier keys.
    pub fn stop(&self) {
        ghost_core::input::hotkey::trigger_stop();
        ghost_core::input::hotkey::release_all_modifiers();
    }

    /// Reset the stop flag (allows automation to resume after a stop).
    pub fn reset(&self) {
        reset_stop();
    }

    /// Press and release a named key: Enter, Tab, Escape, F5, ArrowUp, Ctrl, etc.
    pub async fn press(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let vk = name_to_vk(key).ok_or_else(|| GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        press_key(vk).map_err(GhostError::Core)
    }

    /// Press a modifier+key combo: modifiers=["Ctrl"], key="c" for Ctrl+C.
    pub async fn hotkey(&self, modifiers: &[&str], key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let mut mod_vks = Vec::new();
        for m in modifiers {
            let vk = name_to_vk(m).ok_or_else(|| GhostError::Core(
                ghost_core::error::CoreError::Win32 { code: 0, context: "unknown modifier name" }
            ))?;
            mod_vks.push(vk);
        }
        let key_vk = name_to_vk(key).ok_or_else(|| GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        for vk in &mod_vks {
            core_key_down(*vk).map_err(GhostError::Core)?;
        }
        press_key(key_vk).map_err(GhostError::Core)?;
        for vk in mod_vks.iter().rev() {
            core_key_up(*vk).map_err(GhostError::Core)?;
        }
        Ok(())
    }

    /// Hold a key down without releasing.
    pub async fn key_down(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let vk = name_to_vk(key).ok_or_else(|| GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        core_key_down(vk).map_err(GhostError::Core)
    }

    /// Release a key held by key_down.
    pub async fn key_up(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let vk = name_to_vk(key).ok_or_else(|| GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        core_key_up(vk).map_err(GhostError::Core)
    }

    /// Move mouse without clicking. Triggers hover states, dropdown menus, tooltips.
    pub async fn hover(&self, x: i32, y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        core_hover(x, y).map_err(GhostError::Core)
    }

    /// Right-click at pixel coordinates.
    pub async fn right_click_at(&self, x: i32, y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        core_right_click(x, y).map_err(GhostError::Core)
    }

    /// Double-click at pixel coordinates.
    pub async fn double_click_at(&self, x: i32, y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        core_double_click(x, y).map_err(GhostError::Core)
    }

    /// Drag from one position to another.
    pub async fn drag(&self, from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        core_drag(from_x, from_y, to_x, to_y).map_err(GhostError::Core)
    }

    /// Scroll at coordinates. direction: "up"/"down"/"left"/"right". amount = wheel notches.
    pub async fn scroll(&self, x: i32, y: i32, direction: &str, amount: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        core_scroll(x, y, direction, amount).map_err(GhostError::Core)
    }

    /// Read the current clipboard text. Returns empty string if clipboard is empty.
    pub async fn get_clipboard(&self) -> Result<String> {
        core_get_clipboard().map_err(GhostError::Core)
    }

    /// Write text to the clipboard.
    pub async fn set_clipboard(&self, text: &str) -> Result<()> {
        core_set_clipboard(text).map_err(GhostError::Core)
    }

    /// List all visible top-level windows.
    pub async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        core_list_windows().map_err(GhostError::Core)
    }

    /// Bring a window to the foreground by partial name match.
    pub async fn focus_window(&self, name: &str) -> Result<()> {
        core_focus_window(name).map_err(GhostError::Core)
    }

    /// Change window state: "maximize", "minimize", "restore", or "close".
    pub async fn window_state(&self, name: &str, state: &str) -> Result<()> {
        let ws = WindowState::from_str(state).ok_or_else(|| GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "invalid window state" }
        ))?;
        set_window_state(name, ws).map_err(GhostError::Core)
    }

    /// Wait N milliseconds.
    pub async fn wait(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// Return structured list of interactive elements. window: optional partial window title to scope.
    pub async fn describe_screen(&self, window: Option<&str>) -> Result<Vec<ghost_core::uia::ElementDescriptor>> {
        self.tree.describe_screen(window).map_err(GhostError::Core)
    }

    /// Get the text value of a found element.
    pub async fn get_text(&self, by: By) -> Result<String> {
        let el = self.find(by).await?;
        Ok(el.get_text())
    }

    /// Focus a browser window, type a URL into the address bar, press Enter, then wait for idle.
    pub async fn navigate_and_wait(
        &self,
        window_name: &str,
        url: &str,
        idle_timeout_ms: u64,
    ) -> Result<()> {
        self.focus_window(window_name).await?;
        // Ctrl+L focuses the address bar in Edge/Chrome/Firefox.
        self.hotkey(&["Ctrl"], "l").await?;
        ghost_core::input::keyboard::type_text(url).map_err(GhostError::Core)?;
        self.press("Enter").await?;
        self.wait_for_idle(Some(window_name), 3, idle_timeout_ms).await
    }

    /// Click an element, then wait for `expected_text` to appear (or disappear) on screen.
    pub async fn click_and_wait_for_text(
        &self,
        target: By,
        expected_text: &str,
        appears: bool,
        timeout_ms: u64,
    ) -> Result<()> {
        let el = self.find(target).await?;
        el.click()?;
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            let descriptors = self.describe_screen(None).await.unwrap_or_default();
            let found = descriptors.iter().any(|d| d.name.contains(expected_text));
            if found == appears {
                return Ok(());
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout {
                    action: format!("wait_for_text:{expected_text}"),
                    ms: timeout_ms,
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Fill each `(locator, text)` pair, optionally click submit, then wait for idle.
    pub async fn fill_form(
        &self,
        fields: &[(By, String)],
        submit: Option<By>,
        idle_timeout_ms: u64,
    ) -> Result<()> {
        for (by, text) in fields {
            let el = self.find(by.clone()).await?;
            el.click()?;
            ghost_core::input::keyboard::type_text(text).map_err(GhostError::Core)?;
        }
        if let Some(sub) = submit {
            let el = self.find(sub).await?;
            el.click()?;
            self.wait_for_idle(None, 3, idle_timeout_ms).await?;
        }
        Ok(())
    }

    /// Compile a JSON intent and run it through the FsmExecutor, dispatching ops against
    /// this session. See `ghost-intent::compiler` for intent schema.
    pub async fn execute_intent(&self, json: &str) -> Result<IntentResult> {
        let intent: CompiledIntent = IntentCompiler::compile(json).map_err(GhostError::from)?;
        let dispatcher = SessionOpsDispatcher { session: self };
        let executor = FsmExecutor::new(&dispatcher);
        Ok(executor.run(&intent).await)
    }
}

/// Bridges `OpsDispatcher` to session primitives. Each `Op` maps to a session method.
struct SessionOpsDispatcher<'a> {
    session: &'a GhostSession,
}

#[async_trait(?Send)]
impl<'a> OpsDispatcher for SessionOpsDispatcher<'a> {
    async fn dispatch(&self, op: &Op, _state: &mut IntentState) -> std::result::Result<(), IntentError> {
        match op {
            Op::Click { target } => {
                let el = self.session.find(By::Name(target.clone())).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
                el.click().map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::Type { target, text } => {
                let el = self.session.find(By::Name(target.clone())).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
                el.type_text(text).map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::Press { key } => {
                self.session.press(key).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::Hotkey { modifiers, key } => {
                let mods: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
                self.session.hotkey(&mods, key).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::WaitForText { text, appears, timeout_ms } => {
                let start = std::time::Instant::now();
                let deadline = Duration::from_millis(*timeout_ms);
                loop {
                    let descriptors = self.session.describe_screen(None).await
                        .map_err(|e| IntentError::OpFailed(e.to_string()))?;
                    let found = descriptors.iter().any(|d| d.name.contains(text));
                    if found == *appears { break; }
                    if start.elapsed() >= deadline {
                        return Err(IntentError::OpFailed(format!("wait_for_text:{text}")));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            Op::WaitUntil { condition, timeout_ms } => {
                self.session.wait_until(condition.clone(), *timeout_ms, 50).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::WaitForIdle { stable_frames, timeout_ms } => {
                self.session.wait_for_idle(None, *stable_frames, *timeout_ms).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::Navigate { url } => {
                let target_name = {
                    let windows = self.session.list_windows().await
                        .map_err(|e| IntentError::OpFailed(e.to_string()))?;
                    windows.iter()
                        .find(|w| w.name.contains("Edge") || w.name.contains("Chrome") || w.name.contains("Firefox"))
                        .map(|w| w.name.clone())
                        .ok_or_else(|| IntentError::OpFailed("no browser window".into()))?
                };
                self.session.navigate_and_wait(&target_name, url, 10_000).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::FocusWindow { name } => {
                self.session.focus_window(name).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
            Op::Screenshot => {
                self.session.screenshot(Region::full()).await
                    .map_err(|e| IntentError::OpFailed(e.to_string()))?;
            }
        }
        Ok(())
    }
}
