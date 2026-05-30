use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use async_trait::async_trait;
use ghost_cache::uia_mirror::{UiaCache, SnapshotDelta, Snapshot, CacheStats};
use ghost_cache::{LocatorCache, LocatorCacheStats};
use ghost_cache::locator_cache::LocatorKey;
use ghost_intent::compiler::{CompiledIntent, IntentCompiler, Op};
use ghost_intent::error::IntentError;
use ghost_intent::executor::{FsmExecutor, IntentResult, IntentState, OpsDispatcher};
use ghost_core::capture::idle::IdleDetector;
use ghost_core::{
    capture::{capture_screen, capture_region_raw, compute_verification},
    input::hotkey::{register_emergency_stop, is_stopped, reset_stop},
    input::keyboard::{key_down as core_key_down, key_up as core_key_up, name_to_vk, press_key},
    input::mouse::{
        hover as core_hover, right_click as core_right_click,
        double_click as core_double_click, drag as core_drag, scroll as core_scroll,
    },
    process::launch as proc_launch,
    system::{get_clipboard as core_get_clipboard, set_clipboard as core_set_clipboard},
    uia::{
        init_com, ComGuard, EventBus,
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
    /// In-session in-memory locator cache. Validated via ElementFromPoint on hit.
    locator_cache: LocatorCache,
    /// Reflection ring buffer: records recent grounding/action failures so the
    /// next VLM prompt can be prefixed with a negative hint.
    pub reflection: crate::reflection::ReflectionBuffer,
    /// Keeps COM initialized for the session lifetime — calls CoUninitialize on drop.
    _com_guard: ComGuard,
}

impl GhostSession {
    /// Create a new automation session.
    /// Initializes COM, registers the Ctrl+Alt+G emergency stop hotkey, and creates the UIA tree.
    pub fn new() -> Result<Self> {
        let com_guard = init_com().map_err(GhostError::Core)?;
        register_emergency_stop().map_err(GhostError::Core)?;
        let tree = UiaTree::new().map_err(GhostError::Core)?;

        // Log which vision providers are configured (presence only, never values).
        let nvidia_ok = std::env::var("NVIDIA_API_KEY").is_ok();
        let anthropic_ok = std::env::var("ANTHROPIC_API_KEY").is_ok();
        let provider_override = std::env::var("GHOST_VISION_PROVIDER").ok();
        if nvidia_ok || anthropic_ok {
            eprintln!(
                "[ghost-session] vision providers configured: NVIDIA_API_KEY={} ANTHROPIC_API_KEY={} GHOST_VISION_PROVIDER={}",
                if nvidia_ok { "SET" } else { "unset" },
                if anthropic_ok { "SET" } else { "unset" },
                provider_override.as_deref().unwrap_or("unset (auto-detect)"),
            );
        } else {
            eprintln!("[ghost-session] WARNING: no vision API key configured; ghost_locate_by_description / ghost_click_by_description / ghost_type_by_description will fail. Set NVIDIA_API_KEY or ANTHROPIC_API_KEY.");
        }

        Ok(Self {
            timeout_ms: 5000,
            tree,
            cache: Arc::new(UiaCache::new()),
            locator_cache: LocatorCache::new(),
            reflection: crate::reflection::ReflectionBuffer::default(),
            _com_guard: com_guard,
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

    /// Return UIA mirror cache statistics (snapshots served, history hit rate, etc).
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Return locator cache hit/miss/invalidation statistics.
    pub fn locator_cache_stats(&self) -> LocatorCacheStats {
        self.locator_cache.stats()
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

    /// Build the locator cache key for a `By` variant, scoped to the current foreground HWND.
    /// MEDIUM-6: include hwnd so entries from different windows can't collide.
    fn by_to_cache_key(by: &By) -> Option<LocatorKey> {
        let hwnd = unsafe {
            let h = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            if h.is_invalid() { 0isize } else { h.0 as isize }
        };
        match by {
            By::Name(n) => Some(LocatorKey::with_hwnd(hwnd, "*", n.as_str())),
            By::Role(r) => Some(LocatorKey::with_hwnd(hwnd, r.as_str(), "*")),
            By::Description(_) => None,
        }
    }

    /// Find the first element matching the locator, retrying until timeout.
    ///
    /// Fast path (T1.5): on each attempt, checks the in-session locator cache
    /// first. If the cache has a rect for this (role, name) key, validates it
    /// via `IUIAutomation::ElementFromPoint(center_of_rect)`. If the returned
    /// element's name/role still matches, returns it immediately without walking
    /// the UIA tree. On mismatch, invalidates the cache entry and falls through
    /// to the normal walk.
    ///
    /// After a successful walk, upserts the result into the locator cache and
    /// advances `cache_seq` (via `apply_snapshot`) so `wait_until` conditions
    /// on `cache_seq` actually progress.
    ///
    /// Tries the foreground window subtree first (fast path); only falls back
    /// to a full desktop walk if the target isn't in the focused window.
    /// Polling backs off: 25ms while warm, 75ms after 1s, 150ms after 3s.
    /// `By::Description` is handled by `find_by_description` (vision fallback).
    #[tracing::instrument(skip(self), fields(timeout_ms = self.timeout_ms))]
    pub async fn find(&self, by: By) -> Result<crate::GhostElement> {
        if let By::Description(desc) = &by {
            return Err(GhostError::Vision(format!(
                "By::Description not supported by find(); use find_by_description() or click_by_description() to get coordinates. desc={desc}"
            )));
        }
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        let action = by.to_string();
        let ms = self.timeout_ms;
        let started = std::time::Instant::now();
        let cache_key = Self::by_to_cache_key(&by);

        let bus = EventBus::global();
        let result = timeout(Duration::from_millis(ms), async {
            let mut attempt: u32 = 0;
            let mut last_seq = bus.seq();
            loop {
                if is_stopped() {
                    return Err(GhostError::Stopped);
                }
                let lookup_started = std::time::Instant::now();

                // --- Locator cache fast path ---
                if let Some(ref key) = cache_key {
                    if let Some(hit) = self.locator_cache.lookup(key) {
                        let (cx, cy) = hit.center;
                        // Validate: element at that point must still match.
                        let validated = match self.tree.element_from_point(cx, cy).map_err(GhostError::Core)? {
                            Some(el) => {
                                let matches = match &by {
                                    // MEDIUM-7: equality for cache validation — contains can match wrong element
                                    By::Name(n) => el.name().to_lowercase() == n.to_lowercase(),
                                    By::Role(r) => {
                                        let role = ghost_core::uia::element::role_id_to_name(el.control_type());
                                        role == r.as_str()
                                    }
                                    By::Description(_) => false,
                                };
                                if matches { Some(el) } else { None }
                            }
                            None => None,
                        };
                        match validated {
                            Some(el) => {
                                tracing::debug!(attempt, "locator cache HIT");
                                return Ok(crate::GhostElement::new(el));
                            }
                            None => {
                                // Stale cache entry — invalidate and fall through to walk.
                                tracing::debug!("locator cache STALE, invalidating");
                                self.locator_cache.invalidate(key);
                            }
                        }
                    }
                }

                // --- Normal UIA tree walk ---
                let found = match &by {
                    By::Name(n) => self.tree.find_by_name_fast(n).map_err(GhostError::Core)?,
                    By::Role(r) => self.tree.find_by_role_fast(r).map_err(GhostError::Core)?,
                    By::Description(_) => unreachable!("filtered above"),
                };
                let lookup_us = lookup_started.elapsed().as_micros();
                if let Some(el) = found {
                    tracing::debug!(attempt, lookup_us, "find hit");
                    // Upsert rect into locator cache and advance cache_seq.
                    if let Some(ref key) = cache_key {
                        if let Some(rect) = el.bounding_rect() {
                            self.locator_cache.upsert(key.clone(), (rect.left, rect.top, rect.right, rect.bottom));
                            // HIGH-3: bump_seq instead of apply_snapshot(empty) — advancing seq
                            // without corrupting nodes or archiving the snapshot.
                            self.cache.bump_seq();
                        }
                    }
                    return Ok(crate::GhostElement::new(el));
                }
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let backoff = if elapsed_ms < 1000 { 25 }
                              else if elapsed_ms < 3000 { 75 }
                              else { 150 };
                attempt += 1;
                match bus.wait_for_change(last_seq, backoff).await {
                    Ok(s) => { last_seq = s; }
                    Err(_) => {}
                }
            }
        })
        .await;

        match result {
            Ok(r) => r,
            Err(_elapsed) => {
                tracing::warn!(action = %action, "find timeout, capturing screenshot");
                let screenshot = capture_screen().ok();
                Err(GhostError::ElementNotFound {
                    query: action,
                    screenshot,
                })
            }
        }
    }

    /// Vision fallback: locate a UI element by natural-language description.
    /// Captures a tight ROI of the foreground window, downscales + JPEG-encodes,
    /// asks Claude for the center pixel, and returns absolute screen coords.
    /// Requires `ANTHROPIC_API_KEY` env var. Override model via `GHOST_VISION_MODEL`
    /// (default: claude-haiku-4-5; ~5x faster than Opus for "where is X" queries).
    ///
    /// If the reflection buffer contains previous failures, a negative hint is
    /// prepended to the description before sending to the VLM so the model
    /// avoids repeating the same mistake.
    #[tracing::instrument(skip(self), fields(desc = %description))]
    pub async fn locate_by_description(&self, description: &str) -> Result<(i32, i32)> {
        let rect = self.foreground_window_rect()
            .ok_or_else(|| GhostError::Vision("no foreground window for vision crop".into()))?;
        let original = (
            (rect.2 - rect.0).max(1) as u32,
            (rect.3 - rect.1).max(1) as u32,
        );
        let max_dim = 1024u32;
        let final_size = if original.0.max(original.1) > max_dim {
            let scale = max_dim as f32 / original.0.max(original.1) as f32;
            (
                ((original.0 as f32) * scale).round().max(1.0) as u32,
                ((original.1 as f32) * scale).round().max(1.0) as u32,
            )
        } else {
            original
        };
        let jpeg = ghost_core::capture::capture_screen_region(
            Some(rect),
            Some(max_dim),
            ghost_core::capture::CaptureFormat::Jpeg(80),
        ).map_err(GhostError::Core)?;

        let crop = crate::vision::Crop {
            origin: (rect.0, rect.1),
            original,
            final_size,
        };

        // Prefix the description with a reflection hint if there are recent failures.
        // This steers the VLM away from repeating the same coordinate mistake.
        let augmented_description: String;
        let effective_description = if let Some(hint) = self.reflection.failure_hint() {
            augmented_description = format!("{hint}\n\nTarget: {description}");
            &augmented_description
        } else {
            description
        };

        let coords = crate::vision::vision_locate(effective_description, &jpeg, final_size).await?;
        match coords {
            Some((vx, vy)) => Ok(crop.to_screen(vx, vy)),
            None => Err(GhostError::ElementNotFound {
                query: format!("description={description}"),
                screenshot: None,
            }),
        }
    }

    /// Vision fallback + click. One round-trip for "click the blue Submit button".
    pub async fn click_by_description(&self, description: &str) -> Result<()> {
        let (x, y) = self.locate_by_description(description).await?;
        self.click_at(x, y).await
    }

    /// Vision fallback + click + type. For form fills where UIA name is unstable.
    pub async fn type_by_description(&self, description: &str, text: &str) -> Result<()> {
        let (x, y) = self.locate_by_description(description).await?;
        self.click_at(x, y).await?;
        ghost_core::input::keyboard::type_text(text).map_err(GhostError::Core)
    }

    /// Local OCR text search via Windows.Media.Ocr (free, on-device, no API).
    /// Searches for `needle` (case-insensitive contains) in the foreground window
    /// (or full screen if `foreground=false`). Returns center pixel of first match.
    /// Faster + cheaper than locate_by_description for plain-text cases; ~50-200ms typical.
    ///
    /// The blocking WinRT spin-wait (IAsyncOperation::get) runs on a spawn_blocking
    /// thread so it does not block the async runtime.
    #[tracing::instrument(skip(self), fields(needle = %needle, foreground))]
    pub async fn find_text_local(&self, needle: &str, foreground: bool) -> Result<Option<(i32, i32)>> {
        let region = if foreground { self.foreground_window_rect() } else { None };
        let needle = needle.to_string();
        tokio::task::spawn_blocking(move || {
            ghost_core::ocr::find_text_local(&needle, region).map_err(GhostError::Core)
        })
        .await
        .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?
    }

    /// Local OCR + click. Polls until needle appears or timeout. Event-driven backoff.
    pub async fn click_text_local(&self, needle: &str, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let bus = EventBus::global();
        let mut last_seq = bus.seq();
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            if let Some((x, y)) = self.find_text_local(needle, true).await? {
                return self.click_at(x, y).await;
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout {
                    action: format!("click_text_local:{needle}"),
                    ms: timeout_ms,
                });
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let backoff = if elapsed_ms < 1000 { 100 }
                          else if elapsed_ms < 3000 { 200 }
                          else { 400 };
            match bus.wait_for_change(last_seq, backoff).await {
                Ok(s) => { last_seq = s; }
                Err(_) => {}
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
    /// HIGH-2: runs on a spawn_blocking thread so the DXGI wait (up to 50ms) never
    /// blocks a tokio worker. D3D11/DXGI objects are MTA-safe behind the global Mutex.
    pub async fn screenshot(&self, _region: Region) -> Result<Vec<u8>> {
        tokio::task::spawn_blocking(|| capture_screen().map_err(GhostError::Core))
            .await
            .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?
    }

    /// Capture a screen region with optional downscale and JPEG/PNG encoding.
    /// rect: (left, top, right, bottom) in pixels; None = full screen.
    /// max_dim: longest-edge size after downscale; None = no downscale.
    /// jpeg_quality: 0-100; None = PNG output.
    /// For vision payloads, prefer rect=focused_window_bbox(), max_dim=Some(768), jpeg_quality=Some(75).
    /// HIGH-2: runs on a spawn_blocking thread for the same reason as screenshot().
    #[tracing::instrument(skip(self), fields(rect = ?rect, max_dim, jpeg_quality))]
    pub async fn screenshot_region(
        &self,
        rect: Option<(i32, i32, i32, i32)>,
        max_dim: Option<u32>,
        jpeg_quality: Option<u8>,
    ) -> Result<Vec<u8>> {
        let format = match jpeg_quality {
            Some(q) => ghost_core::capture::CaptureFormat::Jpeg(q),
            None => ghost_core::capture::CaptureFormat::Png,
        };
        tokio::task::spawn_blocking(move || {
            ghost_core::capture::capture_screen_region(rect, max_dim, format).map_err(GhostError::Core)
        })
        .await
        .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?
    }

    /// Bounding rect of the foreground window: (left, top, right, bottom) or None if no window focused.
    /// Useful as the rect arg to screenshot_region for tight vision crops.
    pub fn foreground_window_rect(&self) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
            use windows::Win32::Foundation::RECT;
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() { return None; }
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                Some((r.left, r.top, r.right, r.bottom))
            } else {
                None
            }
        }
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

    /// Current event-bus sequence. Increments on each system foreground change.
    /// Pair with wait_for_event for race-free event-driven waits.
    pub fn event_seq(&self) -> u64 {
        EventBus::global().seq()
    }

    /// Wait for the next system event (foreground change) or timeout.
    /// Returns Ok(new_seq) on event, Err(Timeout) on no-event-within-deadline.
    /// Use `since_seq = event_seq()` taken before triggering the action you're awaiting.
    #[tracing::instrument(skip(self), fields(since_seq, timeout_ms))]
    pub async fn wait_for_event(&self, since_seq: u64, timeout_ms: u64) -> Result<u64> {
        EventBus::global()
            .wait_for_change(since_seq, timeout_ms)
            .await
            .map_err(|_| GhostError::Timeout {
                action: "wait_for_event".into(),
                ms: timeout_ms,
            })
    }

    /// Return structured list of interactive elements. window: optional partial window title to scope.
    pub async fn describe_screen(&self, window: Option<&str>) -> Result<Vec<ghost_core::uia::ElementDescriptor>> {
        self.tree.describe_screen(window).map_err(GhostError::Core)
    }

    /// Fast describe scoped to the foreground window subtree only.
    /// 5-50x faster than describe_screen(None); preferred default for agent loops.
    #[tracing::instrument(skip(self))]
    pub async fn describe_screen_fast(&self) -> Result<Vec<ghost_core::uia::ElementDescriptor>> {
        self.tree.describe_screen_fast().map_err(GhostError::Core)
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
        // Tolerate transient FocusFailed — the navigation action itself will surface a real
        // error if the window is truly unreachable. Other errors (ProcessNotFound, etc.) still propagate.
        if let Err(e) = self.focus_window(window_name).await {
            match e {
                GhostError::Core(ghost_core::error::CoreError::FocusFailed { .. }) => {
                    tracing::warn!("focus not confirmed for '{}', proceeding", window_name);
                }
                other => return Err(other),
            }
        }
        // Ctrl+L focuses the address bar in Edge/Chrome/Firefox.
        self.hotkey(&["Ctrl"], "l").await?;
        ghost_core::input::keyboard::type_text(url).map_err(GhostError::Core)?;
        self.press("Enter").await?;
        self.wait_for_idle(Some(window_name), 3, idle_timeout_ms).await
    }

    /// Click an element, then wait for `expected_text` to appear (or disappear) on screen.
    /// Uses scoped (foreground-window) search instead of full desktop describe walks.
    #[tracing::instrument(skip(self), fields(text = %expected_text, appears, timeout_ms))]
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
        let bus = EventBus::global();
        let mut last_seq = bus.seq();
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            let probe = self.tree.find_by_name_fast(expected_text)
                .map_err(GhostError::Core)?;
            let found = probe.is_some();
            if found == appears {
                tracing::debug!(elapsed_ms = start.elapsed().as_millis() as u64, "wait_for_text hit");
                return Ok(());
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout {
                    action: format!("wait_for_text:{expected_text}"),
                    ms: timeout_ms,
                });
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let backoff = if elapsed_ms < 500 { 25 }
                          else if elapsed_ms < 2000 { 75 }
                          else { 150 };
            match bus.wait_for_change(last_seq, backoff).await {
                Ok(s) => { last_seq = s; }
                Err(_) => {}
            }
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

    /// PostMessage-based background click: does not steal foreground focus.
    /// Resolves `window_name` via `list_windows`, then sends WM_LBUTTONDOWN/UP to the HWND.
    pub async fn click_background(&self, window_name: &str, client_x: i32, client_y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let windows = self.list_windows().await?;
        let target = windows.into_iter()
            .find(|w| w.name.contains(window_name))
            .ok_or_else(|| GhostError::ProcessNotFound { name: window_name.into() })?;
        let hwnd = windows::Win32::Foundation::HWND(target.hwnd);
        ghost_core::input::BackgroundClicker::click(hwnd, (client_x, client_y))
            .map_err(GhostError::Core)
    }

    /// Atomic find → set_focus → action → report.
    ///
    /// Resolves the target element by `by`, sets UIA focus, then performs:
    ///   - `action = "click"`: InvokePattern (focus-independent, preferred)
    ///   - `action = "type"`: ValuePattern set (focus-independent, preferred)
    ///
    /// Returns `{ok, name, rect}` on success.
    ///
    /// This eliminates the cross-call focus race: instead of focus_window + find + click
    /// in three separate MCP round-trips, one ghost_act call does them atomically.
    pub async fn act(&self, by: By, action: &str, text: Option<&str>) -> Result<serde_json::Value> {
        // find() already upserts into the locator cache on success.
        let el = self.find(by).await?;
        // Best-effort UIA focus (non-fatal: InvokePattern/ValuePattern work without it)
        let _ = el.set_focus();
        let name = el.name();
        let rect = el.bounding_rect();

        // MEDIUM-5: snapshot the foreground HWND before the action to detect focus changes.
        let fg_before = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };

        // LOW-9: use foreground_window_rect() for both captures so the region stays valid
        // even if the element moves after the action.
        let capture_rect = self.foreground_window_rect();
        let before_capture = capture_region_raw(capture_rect).ok();

        match action {
            "click" => {
                el.click()?;
            }
            "type" => {
                let t = text.ok_or_else(|| GhostError::Vision("ghost_act: action=type requires text param".into()))?;
                el.type_text(t)?;
            }
            other => {
                return Err(GhostError::Vision(format!("ghost_act: unknown action '{other}'; use click or type")));
            }
        }

        // Capture AFTER frame and compute screen-delta verification.
        // Small delay to allow the action to render.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // LOW-9: re-query foreground rect for after-capture (element may have moved/closed).
        let after_capture_rect = self.foreground_window_rect();
        let after_capture = capture_region_raw(after_capture_rect).ok();

        let verification = match (before_capture, after_capture) {
            (Some((before, bw, bh)), Some((after, aw, ah))) if bw == aw && bh == ah => {
                // MEDIUM-5: fg_ok = same window stayed focused, not just any window.
                let fg_after = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                let fg_ok = !fg_before.is_invalid() && fg_before == fg_after;
                Some(compute_verification(&before, &after, bw, bh, fg_ok))
            }
            _ => None,
        };

        let rect_json = rect.map(|(l, t, r, b)| serde_json::json!({"left": l, "top": t, "right": r, "bottom": b}))
            .unwrap_or(serde_json::Value::Null);
        let verification_json = verification.map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);
        Ok(serde_json::json!({
            "ok": true,
            "name": name,
            "rect": rect_json,
            "verification": verification_json,
        }))
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
                let bus = EventBus::global();
                let mut last_seq = bus.seq();
                loop {
                    let probe = self.session.tree.find_by_name_fast(text)
                        .map_err(|e| IntentError::OpFailed(e.to_string()))?;
                    let found = probe.is_some();
                    if found == *appears { break; }
                    if start.elapsed() >= deadline {
                        return Err(IntentError::OpFailed(format!("wait_for_text:{text}")));
                    }
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let backoff = if elapsed_ms < 500 { 25 }
                                  else if elapsed_ms < 2000 { 75 }
                                  else { 150 };
                    match bus.wait_for_change(last_seq, backoff).await {
                        Ok(s) => { last_seq = s; }
                        Err(_) => {}
                    }
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
