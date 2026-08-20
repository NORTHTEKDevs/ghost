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

/// Browsers and tabs this session has opened, keyed by caller-chosen id.
///
/// Keeping tab handles here rather than re-attaching per call matters: every
/// `Target.attachToTarget` opens a new CDP session in the browser, so re-attaching on
/// each tool call would leak sessions for the lifetime of the browser.
#[derive(Default)]
struct BrowserRegistry {
    browsers: std::collections::HashMap<String, Arc<ghost_browser::Browser>>,
    tabs: std::collections::HashMap<String, Arc<ghost_browser::Tab>>,
}

pub struct GhostSession {
    timeout_ms: u64,
    tree: UiaTree,
    cache: Arc<UiaCache>,
    browsers: Arc<tokio::sync::Mutex<BrowserRegistry>>,
    /// Isolated desktops keyed by caller-chosen id. `Arc` because a desktop's worker
    /// thread outlives any single tool call.
    desktops:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<ghost_core::DesktopSession>>>>,
}

impl GhostSession {
    /// Create a new automation session.
    /// Initializes COM, registers the Ctrl+Alt+G emergency stop hotkey, and creates the UIA tree.
    pub fn new() -> Result<Self> {
        // Before anything else: without per-monitor DPI awareness, Windows hands this
        // process virtualized coordinates while UIA reports physical ones, and every
        // click computed from an element rectangle lands in the wrong place on a
        // scaled display.
        ghost_core::system::ensure_per_monitor_aware();
        init_com().map_err(GhostError::Core)?;
        register_emergency_stop().map_err(GhostError::Core)?;
        let tree = UiaTree::new().map_err(GhostError::Core)?;
        Ok(Self {
            timeout_ms: 5000,
            tree,
            cache: Arc::new(UiaCache::new()),
            browsers: Arc::new(tokio::sync::Mutex::new(BrowserRegistry::default())),
            desktops: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
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

    /// Find the first element matching the locator anywhere on the desktop.
    ///
    /// Prefer `find_in` when you know the window. An unscoped search walks every
    /// open window, so `By::role("document")` will cheerfully return a browser's page
    /// when you meant your app's editor - and two ghost processes searching at once
    /// will find each other's elements.
    pub async fn find(&self, by: By) -> Result<crate::GhostElement> {
        self.find_scoped(None, by).await
    }

    /// Find an element inside one top-level window, matched by partial title.
    ///
    /// This is the locator background automation should use: it is unambiguous, and
    /// it is what makes concurrent ghost processes safe to run side by side.
    pub async fn find_in(&self, window: &str, by: By) -> Result<crate::GhostElement> {
        self.find_scoped(Some(window), by).await
    }

    async fn find_scoped(&self, window: Option<&str>, by: By) -> Result<crate::GhostElement> {
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        let action = match window {
            Some(w) => format!("{by} in window '{w}'"),
            None => by.to_string(),
        };
        let ms = self.timeout_ms;

        let result = timeout(Duration::from_millis(ms), async {
            loop {
                if is_stopped() {
                    return Err(GhostError::Stopped);
                }
                let found = match &by {
                    By::Name(n) => {
                        self.tree.find_by_name_in(window, n).map_err(GhostError::Core)?
                    }
                    By::Role(r) => {
                        self.tree.find_by_role_in(window, r).map_err(GhostError::Core)?
                    }
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
                // Diagnostic screenshot. A scoped search attaches only the window it
                // was searching; an unscoped one would have to capture the whole
                // desktop, which under the background policy means handing the agent
                // a picture of the user's private screen for a routine miss.
                let screenshot = match window {
                    Some(w) => crate::background::WindowTarget::resolve(w)
                        .ok()
                        .and_then(|t| t.capture(false).ok()),
                    None if ghost_core::focus::foreground_allowed() => capture_screen().ok(),
                    None => None,
                };
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

    /// Navigate a browser to a URL.
    ///
    /// Routes through CDP whenever a browser is registered, because that path is both
    /// background and reliable. The original implementation - raise the window, press
    /// Ctrl+L, type the URL with the real keyboard - takes over the screen and races
    /// the user's own typing, so it is now only reachable by explicitly allowing
    /// foreground input.
    ///
    /// Returns the route taken so callers can tell which happened.
    pub async fn navigate_and_wait(
        &self,
        window_name: &str,
        url: &str,
        idle_timeout_ms: u64,
    ) -> Result<&'static str> {
        // Prefer a registered browser. `window_name` selects the browser id if it
        // matches one, otherwise any registered browser will do.
        let browser_id = {
            let reg = self.browsers.lock().await;
            reg.browsers
                .keys()
                .find(|k| k.eq_ignore_ascii_case(window_name))
                .cloned()
                .or_else(|| reg.browsers.keys().next().cloned())
        };
        if let Some(id) = browser_id {
            // Reuse a tab already showing this site rather than piling up duplicates.
            let target = match self.tab_find(&id, window_name).await {
                Ok(info) => info.target_id,
                Err(_) => self.tab_open(&id, "about:blank").await?,
            };
            let tab = self.tab(&id, &target).await?;
            tab.navigate(url, idle_timeout_ms).await?;
            return Ok("cdp");
        }

        ghost_core::focus::require_foreground_allowed("navigate_and_wait")
            .map_err(GhostError::Core)?;
        self.focus_window(window_name).await?;
        // Ctrl+L focuses the address bar in Edge/Chrome/Firefox.
        self.hotkey(&["Ctrl"], "l").await?;
        ghost_core::input::keyboard::type_text(url).map_err(GhostError::Core)?;
        self.press("Enter").await?;
        self.wait_for_idle(Some(window_name), 3, idle_timeout_ms).await?;
        Ok("foreground")
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

    // =======================================================================
    // Focus policy
    // =======================================================================

    /// Current focus policy: "background", "prefer_background", or "foreground".
    pub fn focus_policy(&self) -> &'static str {
        ghost_core::focus::policy().as_str()
    }

    /// Change the focus policy for this process.
    ///
    /// `background` (the default) makes every screen-stealing primitive fail rather
    /// than take over the user's cursor. Raise it only for a target that genuinely
    /// has no background path.
    pub fn set_focus_policy(&self, policy: &str) -> Result<&'static str> {
        let p = ghost_core::focus::FocusPolicy::from_str(policy)
            .ok_or_else(|| GhostError::BadFocusPolicy(policy.to_string()))?;
        ghost_core::focus::set_policy(p);
        Ok(p.as_str())
    }

    // =======================================================================
    // Window-scoped background input
    // =======================================================================

    /// Resolve a window by partial title for background operations.
    pub fn window(&self, name: &str) -> Result<crate::background::WindowTarget> {
        crate::background::WindowTarget::resolve(name)
    }

    /// Background left click at a point in the window's client area.
    pub async fn click_background(&self, window_name: &str, x: i32, y: i32) -> Result<()> {
        self.window(window_name)?.click(x, y)
    }

    pub async fn right_click_background(&self, window_name: &str, x: i32, y: i32) -> Result<()> {
        self.window(window_name)?.right_click(x, y)
    }

    pub async fn double_click_background(&self, window_name: &str, x: i32, y: i32) -> Result<()> {
        self.window(window_name)?.double_click(x, y)
    }

    pub async fn hover_background(&self, window_name: &str, x: i32, y: i32) -> Result<()> {
        self.window(window_name)?.hover(x, y)
    }

    pub async fn scroll_background(
        &self,
        window_name: &str,
        x: i32,
        y: i32,
        direction: &str,
        amount: i32,
    ) -> Result<()> {
        self.window(window_name)?.scroll(x, y, direction, amount)
    }

    /// Type into a background window's focused control via WM_CHAR.
    pub async fn type_background(&self, window_name: &str, text: &str) -> Result<()> {
        self.window(window_name)?.type_text(text)
    }

    pub async fn press_background(&self, window_name: &str, key: &str) -> Result<()> {
        self.window(window_name)?.press(key)
    }

    pub async fn hotkey_background(
        &self,
        window_name: &str,
        modifiers: &[String],
        key: &str,
    ) -> Result<()> {
        self.window(window_name)?.hotkey(modifiers, key)
    }

    /// Run an editing shortcut against a background window via its standard control
    /// message. The correct background Ctrl+Z / Ctrl+A / Ctrl+V.
    pub async fn shortcut_background(&self, window_name: &str, shortcut: &str) -> Result<()> {
        let target = self.window(window_name)?;
        ghost_core::input::shortcut::apply(target.hwnd, shortcut).map_err(GhostError::Core)
    }

    /// Replace a background window's text in one message.
    pub async fn set_text_background(&self, window_name: &str, text: &str) -> Result<()> {
        self.window(window_name)?.set_text(text, 5_000)
    }

    /// PNG of one window, captured without raising it or un-occluding it.
    ///
    /// Unlike `screenshot`, this sees a window the user has covered with their own
    /// work, and it does not hand the agent a picture of the user's whole screen.
    pub async fn capture_window(&self, window_name: &str, client_only: bool) -> Result<Vec<u8>> {
        self.window(window_name)?.capture(client_only)
    }

    /// Find an element by locator, then click it through its window's message queue.
    ///
    /// The bridge for controls that expose no usable UIA pattern: UIA still knows
    /// *where* the element is, and window messages can act on that position without
    /// the cursor. Returns the window title and client point actually used.
    pub async fn click_element_background(
        &self,
        by: By,
        window_name: &str,
    ) -> Result<(String, i32, i32)> {
        let el = self.find(by).await?;
        let (l, t, r, b) = el.bounding_rect().ok_or_else(|| GhostError::ElementNotInteractable {
            element: el.name(),
            reason: "element has no bounding rectangle".into(),
        })?;
        let target = self.window(window_name)?;
        // UIA reports screen coordinates; window messages want client coordinates.
        let (cx, cy) = target.screen_to_client((l + r) / 2, (t + b) / 2)?;
        target.click(cx, cy)?;
        Ok((target.title.clone(), cx, cy))
    }

    // =======================================================================
    // Browser / tab automation
    // =======================================================================

    /// Launch an isolated browser under `id`. `mode` is "headless" or "windowed".
    ///
    /// Each id gets its own browser process and its own profile directory, so
    /// concurrent ghost processes never share cookies, ports, or a profile lock.
    pub async fn browser_launch(&self, id: &str, mode: &str) -> Result<serde_json::Value> {
        self.browser_launch_with(id, mode, None).await
    }

    /// Launch a specific installed browser ("chrome", "comet", "edge", "brave") or
    /// the default when `which` is None. All are Chromium-family and driven
    /// identically over CDP.
    pub async fn browser_launch_with(
        &self,
        id: &str,
        mode: &str,
        which: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mode = ghost_browser::LaunchMode::from_str(mode)
            .unwrap_or(ghost_browser::LaunchMode::Headless);
        let executable = match which {
            Some(name) => Some(ghost_browser::find_named_browser(name)?),
            None => None,
        };
        let mut dir = std::env::temp_dir();
        dir.push("ghost-browser-profiles");
        dir.push(format!("p{}-{}", std::process::id(), sanitize_id(id)));
        let opts = ghost_browser::LaunchOptions {
            mode,
            user_data_dir: dir,
            executable,
            ..Default::default()
        };
        let browser = ghost_browser::Browser::launch(&opts).await?;
        let info = serde_json::json!({
            "id": id,
            "port": browser.port(),
            "pid": browser.pid(),
            "browser": which.unwrap_or("default"),
            "mode": format!("{mode:?}").to_lowercase(),
        });
        self.browsers.lock().await.browsers.insert(id.to_string(), Arc::new(browser));
        Ok(info)
    }

    /// Attach to a browser already running with `--remote-debugging-port=<port>`.
    pub async fn browser_attach(&self, id: &str, port: u16) -> Result<serde_json::Value> {
        let browser = ghost_browser::Browser::attach(port).await?;
        let info = serde_json::json!({ "id": id, "port": browser.port(), "attached": true });
        self.browsers.lock().await.browsers.insert(id.to_string(), Arc::new(browser));
        Ok(info)
    }

    async fn browser_handle(&self, id: &str) -> Result<Arc<ghost_browser::Browser>> {
        self.browsers
            .lock()
            .await
            .browsers
            .get(id)
            .cloned()
            .ok_or_else(|| GhostError::BrowserNotRegistered { id: id.to_string() })
    }

    /// Close a browser ghost launched and forget its tabs. Attached browsers are
    /// disconnected but left running - they belong to the user.
    pub async fn browser_close(&self, id: &str) -> Result<()> {
        let browser = self.browser_handle(id).await?;
        browser.close().await?;
        let mut reg = self.browsers.lock().await;
        reg.browsers.remove(id);
        let prefix = format!("{id}/");
        reg.tabs.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    pub async fn browser_tabs(&self, id: &str) -> Result<Vec<ghost_browser::TabInfo>> {
        Ok(self.browser_handle(id).await?.tabs().await?)
    }

    /// Open a background tab and return its target id.
    pub async fn tab_open(&self, browser_id: &str, url: &str) -> Result<String> {
        let browser = self.browser_handle(browser_id).await?;
        let tab = browser.new_tab(url).await?;
        let target_id = tab.target_id().to_string();
        self.browsers
            .lock()
            .await
            .tabs
            .insert(format!("{browser_id}/{target_id}"), Arc::new(tab));
        Ok(target_id)
    }

    /// Get a cached tab handle, attaching on first use.
    pub async fn tab(&self, browser_id: &str, target_id: &str) -> Result<Arc<ghost_browser::Tab>> {
        let key = format!("{browser_id}/{target_id}");
        if let Some(t) = self.browsers.lock().await.tabs.get(&key) {
            return Ok(t.clone());
        }
        let browser = self.browser_handle(browser_id).await?;
        let tab = Arc::new(browser.tab(target_id).await?);
        self.browsers.lock().await.tabs.insert(key, tab.clone());
        Ok(tab)
    }

    pub async fn tab_close(&self, browser_id: &str, target_id: &str) -> Result<()> {
        let browser = self.browser_handle(browser_id).await?;
        browser.close_tab(target_id).await?;
        self.browsers.lock().await.tabs.remove(&format!("{browser_id}/{target_id}"));
        Ok(())
    }

    /// First tab in `browser_id` whose URL or title contains `needle`.
    pub async fn tab_find(&self, browser_id: &str, needle: &str) -> Result<ghost_browser::TabInfo> {
        Ok(self.browser_handle(browser_id).await?.find_tab(needle).await?)
    }
}

impl GhostSession {
    // =======================================================================
    // Isolated desktops - for apps with no usable automation surface, and for
    // running an app somewhere the user can never see it at all.
    // =======================================================================

    async fn desktop_handle(&self, id: &str) -> Result<Arc<ghost_core::DesktopSession>> {
        self.desktops
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| GhostError::DesktopNotRegistered { id: id.to_string() })
    }

    /// Create an isolated desktop under `id`. Apps launched onto it never appear on
    /// the user's screen.
    pub async fn desktop_create(&self, id: &str) -> Result<serde_json::Value> {
        let d = ghost_core::DesktopSession::create(id).map_err(GhostError::Core)?;
        let info = serde_json::json!({
            "id": id,
            "desktop": d.name(),
            "real_input_supported": d.real_input_supported(),
        });
        self.desktops.lock().await.insert(id.to_string(), Arc::new(d));
        Ok(info)
    }

    /// Destroy an isolated desktop. Processes still running on it are terminated
    /// first: otherwise they would be stranded on a desktop nobody is bound to, with
    /// no window anyone can see or close.
    pub async fn desktop_close(&self, id: &str) -> Result<serde_json::Value> {
        let d = self.desktop_handle(id).await?;
        let mut killed = Vec::new();
        if let Ok(windows) = d.all_windows() {
            let mut pids: Vec<u32> = windows.into_iter().map(|w| w.pid).collect();
            pids.sort_unstable();
            pids.dedup();
            for pid in pids {
                if ghost_core::process::kill(pid).is_ok() {
                    killed.push(pid);
                }
            }
        }
        self.desktops.lock().await.remove(id);
        Ok(serde_json::json!({ "ok": true, "terminated_pids": killed }))
    }

    pub async fn desktop_launch(&self, id: &str, command: &str) -> Result<u32> {
        self.desktop_handle(id).await?.launch(command).map_err(GhostError::Core)
    }

    pub async fn desktop_windows(&self, id: &str) -> Result<Vec<serde_json::Value>> {
        let windows = self.desktop_handle(id).await?.windows().map_err(GhostError::Core)?;
        Ok(windows
            .into_iter()
            .map(|w| serde_json::json!({ "hwnd": w.hwnd, "title": w.title, "pid": w.pid }))
            .collect())
    }

    pub async fn desktop_wait_for_window(
        &self,
        id: &str,
        needle: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
        let w = self
            .desktop_handle(id)
            .await?
            .wait_for_window(needle, timeout_ms)
            .map_err(GhostError::Core)?;
        Ok(serde_json::json!({ "hwnd": w.hwnd, "title": w.title, "pid": w.pid }))
    }

    pub async fn desktop_click(&self, id: &str, hwnd: isize, x: i32, y: i32) -> Result<()> {
        self.desktop_handle(id).await?.click(hwnd, x, y).map_err(GhostError::Core)
    }

    pub async fn desktop_right_click(&self, id: &str, hwnd: isize, x: i32, y: i32) -> Result<()> {
        self.desktop_handle(id).await?.right_click(hwnd, x, y).map_err(GhostError::Core)
    }

    pub async fn desktop_double_click(&self, id: &str, hwnd: isize, x: i32, y: i32) -> Result<()> {
        self.desktop_handle(id).await?.double_click(hwnd, x, y).map_err(GhostError::Core)
    }

    pub async fn desktop_scroll(
        &self,
        id: &str,
        hwnd: isize,
        x: i32,
        y: i32,
        direction: &str,
        amount: i32,
    ) -> Result<()> {
        let (notches, horizontal) = match direction {
            "up" => (amount, false),
            "down" => (-amount, false),
            "right" => (amount, true),
            "left" => (-amount, true),
            _ => {
                return Err(GhostError::Core(ghost_core::error::CoreError::Win32 {
                    code: 0,
                    context: "invalid scroll direction",
                }))
            }
        };
        self.desktop_handle(id)
            .await?
            .scroll(hwnd, x, y, notches, horizontal)
            .map_err(GhostError::Core)
    }

    pub async fn desktop_type(&self, id: &str, hwnd: isize, text: &str) -> Result<()> {
        self.desktop_handle(id).await?.type_text(hwnd, text).map_err(GhostError::Core)
    }

    pub async fn desktop_press(&self, id: &str, hwnd: isize, key: &str) -> Result<()> {
        self.desktop_handle(id).await?.press(hwnd, key).map_err(GhostError::Core)
    }

    pub async fn desktop_shortcut(&self, id: &str, hwnd: isize, name: &str) -> Result<()> {
        self.desktop_handle(id).await?.shortcut(hwnd, name).map_err(GhostError::Core)
    }

    pub async fn desktop_capture(&self, id: &str, hwnd: isize, client_only: bool) -> Result<Vec<u8>> {
        self.desktop_handle(id)
            .await?
            .capture(hwnd, client_only)
            .map_err(GhostError::Core)
    }

    /// Interactive elements of a window on an isolated desktop, via UIA.
    ///
    /// UIA works on a non-displayed desktop, so an app launched there is driveable by
    /// control patterns rather than pixel coordinates.
    pub async fn desktop_describe(
        &self,
        id: &str,
        window: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        let w = window.map(|s| s.to_string());
        let d = self.desktop_handle(id).await?;
        let out = d
            .with_uia(move |tree| {
                tree.describe_screen(w.as_deref())
                    .map(|els| {
                        els.into_iter()
                            .map(|e| {
                                serde_json::json!({
                                    "name": e.name,
                                    "role": e.role,
                                    "rect": [e.left, e.top, e.right, e.bottom],
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| e.to_string())
            })
            .map_err(GhostError::Core)?;
        out.map_err(|e| GhostError::UiaUnavailable { app: e })
    }

    /// Find an element by name or role on an isolated desktop and activate it.
    pub async fn desktop_click_element(
        &self,
        id: &str,
        window: Option<&str>,
        by_name: Option<&str>,
        by_role: Option<&str>,
    ) -> Result<String> {
        let w = window.map(|s| s.to_string());
        let name = by_name.map(|s| s.to_string());
        let role = by_role.map(|s| s.to_string());
        let d = self.desktop_handle(id).await?;
        let out = d
            .with_uia(move |tree| {
                let found = match (&name, &role) {
                    (Some(n), _) => tree.find_by_name_in(w.as_deref(), n),
                    (None, Some(r)) => tree.find_by_role_in(w.as_deref(), r),
                    _ => return Err("provide name or role".to_string()),
                }
                .map_err(|e| e.to_string())?;
                let el = found.ok_or_else(|| "element not found".to_string())?;
                let label = el.name();
                ghost_core::uia::patterns::invoke(&el)
                    .map(|route| format!("{label}|{}", route.as_str()))
                    .map_err(|e| e.to_string())
            })
            .map_err(GhostError::Core)?;
        out.map_err(|e| GhostError::ElementNotFound { query: e, screenshot: None })
    }
}

/// Keep a caller-supplied browser id safe to use as a directory name.
fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned.chars().take(48).collect()
    }
}

/// Bridges `OpsDispatcher` to session primitives. Each `Op` maps to a session method.
struct SessionOpsDispatcher<'a> {
    session: &'a GhostSession,
}

#[async_trait]
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
