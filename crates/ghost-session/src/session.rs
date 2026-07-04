use std::sync::Arc;
use std::cell::RefCell;
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
use ghost_ground::engine::{GroundingEngine, GroundingStats, LocateMode};
use ghost_ground::types::{Grounded, Target, Tier};
use crate::{
    locator::By,
    error::{GhostError, Result},
    tiers::{CacheTier, UiaTier, OcrTier, VlmTier, foreground_hwnd},
    env_key_is_set,
};
#[cfg(feature = "yolo")]
use crate::tiers::YoloTier;

pub struct Region;

impl Region {
    pub fn full() -> Self {
        Region
    }
}

/// Upper bound on the on-device WinRT OCR call (typical cost is 50-200ms).
const OCR_TIMEOUT_MS: u64 = 3000;

/// Post-action verification polling schedule (ms between captures). Total worst
/// case ~240ms for actions with no visible effect; fast path exits at first
/// detected change (~40ms), so instant UI stays instant while async renders
/// (web/Electron) get time to paint before we declare "nothing changed".
const VERIFY_POLL_MS: [u64; 3] = [40, 80, 120];

pub struct GhostSession {
    timeout_ms: u64,
    tree: UiaTree,
    cache: Arc<UiaCache>,
    /// In-session in-memory locator cache. Validated via ElementFromPoint on hit.
    locator_cache: LocatorCache,
    /// Reflection ring buffer: records recent grounding/action failures so the
    /// next VLM prompt can be prefixed with a negative hint.
    /// RefCell is sound: GhostSession is !Send and all calls run on the single
    /// block_on thread; no tokio::spawn moves this session across threads.
    pub reflection: RefCell<crate::reflection::ReflectionBuffer>,
    /// Cumulative grounding cascade telemetry. RefCell because `ground()` takes `&self`
    /// (MCP handlers only have &GhostSession) but needs to mutate stats. Safe because
    /// all session calls run on the single STA tokio block_on thread.
    grounding_stats: RefCell<GroundingStats>,
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
        // Treat empty/whitespace-only values the same as unset — an empty key causes
        // confusing provider 500s rather than a clear error.
        let nvidia_ok = env_key_is_set("NVIDIA_API_KEY");
        let anthropic_ok = env_key_is_set("ANTHROPIC_API_KEY");
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
            reflection: RefCell::new(crate::reflection::ReflectionBuffer::default()),
            grounding_stats: RefCell::new(GroundingStats::default()),
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
                if let Ok(s) = bus.wait_for_change(last_seq, backoff).await {
                    last_seq = s;
                    // Debounce: content-change hooks (VALUECHANGE/REORDER/SHOW)
                    // can burst; coalesce before paying for another UIA walk.
                    tokio::time::sleep(Duration::from_millis(10)).await;
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
    /// Set-of-Marks grounding: overlay numbered badges on the foreground window's
    /// detected (UIA) elements, send the marked screenshot to the VLM, and have it
    /// pick the badge NUMBER matching the description. Maps the number back to the
    /// element's exact rect center. Far more reliable than asking the model to
    /// regress raw pixel coordinates. Returns None when there are no candidate
    /// elements to mark (caller then falls back to coordinate regression).
    /// Collect Set-of-Marks candidates for the foreground window: the marked JPEG,
    /// the candidate absolute rects, and their labels. Shared by SoM grounding and
    /// the debug tool. Returns None when there are no markable elements.
    async fn build_marks(&self)
        -> Result<Option<(Vec<u8>, Vec<(i32, i32, i32, i32)>, Vec<String>)>> {
        let rect = self.foreground_window_rect()
            .ok_or_else(|| GhostError::Vision("no foreground window for vision crop".into()))?;
        let els = self.tree.describe_screen_fast().map_err(GhostError::Core)?;
        // Keep on-window, non-degenerate elements; cap so badges stay legible and
        // the prompt stays small.
        const MAX_MARKS: usize = 50;
        let mut candidates: Vec<(i32, i32, i32, i32)> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        for e in &els {
            if e.right <= e.left || e.bottom <= e.top { continue; }
            if e.left < rect.0 - 2 || e.top < rect.1 - 2 || e.right > rect.2 + 2 || e.bottom > rect.3 + 2 { continue; }
            candidates.push((e.left, e.top, e.right, e.bottom));
            labels.push(e.name.clone());
            if candidates.len() >= MAX_MARKS { break; }
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        let marks: Vec<ghost_core::capture::Mark> = candidates.iter().enumerate().map(|(i, c)| {
            ghost_core::capture::Mark { label: (i + 1) as u32, x: c.0 - rect.0, y: c.1 - rect.1 }
        }).collect();
        let jpeg = tokio::task::spawn_blocking(move || {
            ghost_core::capture::capture_region_marked_jpeg(Some(rect), &marks, 1400, 82)
        })
        .await
        .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?
        .map_err(GhostError::Core)?;
        Ok(Some((jpeg, candidates, labels)))
    }

    /// Debug: the Set-of-Marks annotated screenshot (raw JPEG bytes) plus the
    /// numbered label list — exactly what the VLM sees when grounding by
    /// description. The fastest way to diagnose "why did grounding pick wrong".
    /// Returns (Some(jpeg) or None if nothing to mark, marks_json).
    pub async fn render_marks(&self) -> Result<(Option<Vec<u8>>, Vec<serde_json::Value>)> {
        match self.build_marks().await? {
            Some((jpeg, candidates, labels)) => {
                let marks: Vec<serde_json::Value> = labels.iter().zip(candidates.iter()).enumerate()
                    .map(|(i, (name, c))| serde_json::json!({
                        "number": i + 1, "name": name,
                        "center": {"x": (c.0 + c.2) / 2, "y": (c.1 + c.3) / 2}
                    })).collect();
                Ok((Some(jpeg), marks))
            }
            None => Ok((None, Vec::new())),
        }
    }

    async fn locate_by_description_som(&self, description: &str) -> Result<Option<(i32, i32)>> {
        let Some((jpeg, candidates, labels)) = self.build_marks().await? else {
            return Ok(None);
        };
        match crate::vision::vision_pick_mark(description, &jpeg, &labels).await? {
            Some(idx) => {
                let (l, t, r, b) = candidates[idx - 1];
                Ok(Some(((l + r) / 2, (t + b) / 2)))
            }
            None => Ok(None),
        }
    }

    #[tracing::instrument(skip(self), fields(desc = %description))]
    pub async fn locate_by_description(&self, description: &str) -> Result<(i32, i32)> {
        // Prefer Set-of-Marks: pick a real detected element by number (exact rect)
        // rather than trusting the model to regress pixel coordinates.
        match self.locate_by_description_som(description).await {
            Ok(Some((sx, sy))) => {
                let obs = crate::reflection::hash_obs(description);
                self.reflection.borrow_mut().record_success(obs, format!("som '{description}' -> ({sx},{sy})"));
                return Ok((sx, sy));
            }
            Ok(None) => { /* no marks or model picked none — fall back to coord regression */ }
            Err(e) => { tracing::warn!(error=%e, "set-of-marks grounding failed; falling back to coordinate regression"); }
        }

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
        let effective_description = if let Some(hint) = self.reflection.borrow().failure_hint() {
            augmented_description = format!("{hint}\n\nTarget: {description}");
            &augmented_description
        } else {
            description
        };

        let coords = crate::vision::vision_locate(effective_description, &jpeg, final_size).await?;
        // HIGH-4: wire the reflection buffer so failure_hint() is not always empty.
        // RefCell borrow is safe: session runs on a single block_on thread (!Send).
        let obs = crate::reflection::hash_obs(description);
        match coords {
            Some((vx, vy)) => {
                let (sx, sy) = crop.to_screen(vx, vy);
                self.reflection.borrow_mut().record_success(obs, format!("locate '{description}' -> ({sx},{sy})"));
                Ok((sx, sy))
            }
            None => {
                self.reflection.borrow_mut().record_failure(obs, format!("locate '{description}'"), "not found");
                Err(GhostError::ElementNotFound {
                    query: format!("description={description}"),
                    screenshot: None,
                })
            }
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

    /// VLM-based structured field extraction for ghost_query.
    ///
    /// Takes a foreground screenshot (or region if provided), asks the VLM to extract
    /// `fields` by name, and returns a JSON map of `{ field: value_or_null }`.
    /// Fields the VLM cannot find are returned as `null`.
    ///
    /// This is the VLM fallback tier for `handle_ghost_query`; it is called only for
    /// fields that UIA/OCR did not fill (i.e., the `unmatched` list).
    pub async fn query_extract(
        &self,
        fields: &[String],
        region: Option<(i32, i32, i32, i32)>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        if fields.is_empty() {
            return Ok(serde_json::Map::new());
        }
        let rect = region.or_else(|| self.foreground_window_rect());
        let jpeg = ghost_core::capture::capture_screen_region(
            rect,
            Some(1024),
            ghost_core::capture::CaptureFormat::Jpeg(80),
        ).map_err(GhostError::Core)?;
        crate::vision::vision_extract(fields, &jpeg).await
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
        let task = tokio::task::spawn_blocking(move || {
            ghost_core::ocr::find_text_local(&needle, region).map_err(GhostError::Core)
        });
        // The WinRT OCR spin-wait has no internal timeout; bound it here so a hung
        // engine can't occupy a blocking-pool thread (and this call) forever.
        match timeout(Duration::from_millis(OCR_TIMEOUT_MS), task).await {
            Ok(joined) => joined
                .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?,
            Err(_elapsed) => Err(GhostError::Core(ghost_core::error::CoreError::JobTimeout)),
        }
    }

    // -----------------------------------------------------------------------
    // Grounding cascade (W2)
    // -----------------------------------------------------------------------

    /// Run the grounding cascade for `target`.
    ///
    /// Tier order: Cache → UIA → OCR → [YOLO (optional T4)] → VLM.
    /// T4 (YOLO) is included only when the crate is built with `--features yolo`
    /// AND `GHOST_YOLO_MODEL` env var points at a valid `.onnx` file.
    ///
    /// `mode = Instant` skips VLM; the engine auto-escalates to Deliberate on miss.
    /// `mode = Deliberate` tries VLM from the start.
    ///
    /// Returns `Grounded` (rect + center + confidence + source) on success.
    /// Updates cumulative telemetry via the session's `grounding_stats` RefCell.
    ///
    /// # COM safety
    /// Must be called on the STA thread (the tokio block_on loop). CacheTier and
    /// UiaTier touch COM directly; they never cross thread boundaries here.
    #[tracing::instrument(skip(self), fields(target = ?target, ?mode))]
    pub async fn ground(&self, target: Target, mode: LocateMode) -> Result<Grounded> {
        // Coords bypass all tiers.
        if let Target::Coords(x, y) = target {
            return Ok(Grounded::from_point((x, y), 1.0, Tier::Cache));
        }

        let hwnd = foreground_hwnd();

        // Build tiers fresh per call: lifetimes borrow from &self which is valid
        // for the entire async scope below (no Send needed, same STA thread).
        let cache = CacheTier {
            locator_cache: &self.locator_cache,
            tree: &self.tree,
            hwnd,
        };
        let uia = UiaTier::new(&self.tree, &self.locator_cache, hwnd);
        let ocr = OcrTier { session: self };
        let vlm = VlmTier { session: self };

        // T4: YOLO tier inserted between OCR and VLM only when feature is enabled
        // and the model file is present (from_env() reads GHOST_YOLO_MODEL).
        #[cfg(feature = "yolo")]
        let yolo_detector = ghost_ground::yolo::YoloDetector::from_env().ok();

        #[cfg(not(feature = "yolo"))]
        let tiers: Vec<Box<dyn ghost_ground::engine::GroundingTier + '_>> = vec![
            Box::new(cache),
            Box::new(uia),
            Box::new(ocr),
            Box::new(vlm),
        ];

        #[cfg(feature = "yolo")]
        let tiers: Vec<Box<dyn ghost_ground::engine::GroundingTier + '_>> = {
            let mut t: Vec<Box<dyn ghost_ground::engine::GroundingTier + '_>> = vec![
                Box::new(cache),
                Box::new(uia),
                Box::new(ocr),
            ];
            if let Some(detector) = yolo_detector {
                t.push(Box::new(YoloTier { session: self, detector }));
            }
            t.push(Box::new(vlm));
            t
        };

        // Engine is local to this call — no need to store it.
        let mut engine = GroundingEngine::new(tiers);

        let result = engine.locate(&target, mode).await;

        // Merge stats into session's cumulative stats.
        {
            let call_stats = engine.stats();
            let mut sess_stats = self.grounding_stats.borrow_mut();
            sess_stats.instant_hits += call_stats.instant_hits;
            sess_stats.deliberate_hits += call_stats.deliberate_hits;
            sess_stats.total_misses += call_stats.total_misses;
            for (tier_key, ts) in &call_stats.by_tier {
                let entry = sess_stats.by_tier
                    .entry(tier_key.clone())
                    .or_default();
                entry.attempts += ts.attempts;
                entry.hits += ts.hits;
                entry.total_ms += ts.total_ms;
            }
        }

        // Also bump cache seq so wait_until conditions on cache_seq progress.
        self.cache.bump_seq();

        result.ok_or_else(|| GhostError::ElementNotFound {
            query: format!("{target:?}"),
            screenshot: None,
        })
    }

    /// Snapshot current grounding cascade telemetry (across all `ground()` calls this session).
    pub fn grounding_stats(&self) -> GroundingStats {
        self.grounding_stats.borrow().clone()
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
            if let Ok(s) = bus.wait_for_change(last_seq, backoff).await { last_seq = s; }
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
    /// A single character with no VK mapping (an operator/punctuation symbol like
    /// `*`, `/`, `-`, `.`, `=`) is sent as a Unicode character — layout-independent
    /// and exactly the intended glyph — instead of failing. Multi-char unknown
    /// names still error.
    pub async fn press(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if let Some(vk) = name_to_vk(key) {
            return press_key(vk).map_err(GhostError::Core);
        }
        if key.chars().count() == 1 {
            return ghost_core::input::keyboard::type_text(key).map_err(GhostError::Core);
        }
        Err(GhostError::Core(ghost_core::error::CoreError::Win32 {
            code: 0, context: "unknown key name",
        }))
    }

    /// Press a modifier+key combo: modifiers=["Ctrl"], key="c" for Ctrl+C.
    pub async fn hotkey(&self, modifiers: &[&str], key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let mut mod_vks = Vec::new();
        for m in modifiers {
            let vk = name_to_vk(m).ok_or(GhostError::Core(
                ghost_core::error::CoreError::Win32 { code: 0, context: "unknown modifier name" }
            ))?;
            mod_vks.push(vk);
        }
        let key_vk = name_to_vk(key).ok_or(GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        // Release held modifiers on EVERY exit path so a SendInput failure can
        // never leave Ctrl/Shift/Alt stuck down (a stuck modifier corrupts all
        // later keyboard input system-wide). This must cover a failure DURING the
        // modifiers-down loop too — if key-down succeeds for Ctrl but fails for
        // Shift, the already-pressed Ctrl must still be released before returning.
        let mut pressed = Vec::new();
        let down_result: Result<()> = (|| {
            for vk in &mod_vks {
                core_key_down(*vk).map_err(GhostError::Core)?;
                pressed.push(*vk);
            }
            Ok(())
        })();
        if let Err(e) = down_result {
            for vk in pressed.iter().rev() {
                let _ = core_key_up(*vk);
            }
            return Err(e);
        }
        let result = press_key(key_vk).map_err(GhostError::Core);
        for vk in pressed.iter().rev() {
            let _ = core_key_up(*vk);
        }
        result
    }

    /// Hold a key down without releasing.
    pub async fn key_down(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let vk = name_to_vk(key).ok_or(GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "unknown key name" }
        ))?;
        core_key_down(vk).map_err(GhostError::Core)
    }

    /// Release a key held by key_down.
    pub async fn key_up(&self, key: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let vk = name_to_vk(key).ok_or(GhostError::Core(
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

    /// Enter `text` by pasting: save the current clipboard, set it to `text`,
    /// send Ctrl+V, then restore the original clipboard. This is the reliable
    /// path for rich-text web editors (Monaco, ProseMirror, Slate) that ignore
    /// both ValuePattern.SetValue and synthesized per-char keystrokes but DO
    /// honor a real paste. The clipboard is always restored, even on error.
    pub async fn paste_text(&self, text: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        // Best-effort save of the existing clipboard (may be empty/non-text).
        let saved = core_get_clipboard().ok();
        core_set_clipboard(text).map_err(GhostError::Core)?;
        // Small settle so the clipboard write is visible to the target app.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let paste_result = self.hotkey(&["Ctrl"], "v").await;
        // Let the paste consume the clipboard before we overwrite it back.
        tokio::time::sleep(Duration::from_millis(40)).await;
        if let Some(prev) = saved {
            let _ = core_set_clipboard(&prev);
        }
        paste_result
    }

    /// Bring the window whose title contains `name` to the foreground and confirm it,
    /// restoring it first if minimized. Errors loudly on failure — callers use this
    /// to guarantee keyboard input lands in the intended window.
    pub async fn ensure_window_foreground(&self, name: &str) -> Result<()> {
        let n = name.to_string();
        tokio::task::spawn_blocking(move || core_focus_window(&n))
            .await
            .map_err(|e| GhostError::Core(ghost_core::error::CoreError::WorkerPanic(e.to_string())))?
            .map_err(GhostError::Core)
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
        let ws = WindowState::from_str(state).ok_or(GhostError::Core(
            ghost_core::error::CoreError::Win32 { code: 0, context: "invalid window state" }
        ))?;
        set_window_state(name, ws).map_err(GhostError::Core)
    }

    /// Wait N milliseconds. Polls the emergency-stop flag so ghost_stop (or
    /// Ctrl+Alt+G) can interrupt a long sleep; returns false if interrupted.
    pub async fn wait(&self, ms: u64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        loop {
            if is_stopped() {
                return false;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return true;
            }
            tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
        }
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
            if let Ok(s) = bus.wait_for_change(last_seq, backoff).await { last_seq = s; }
        }
    }

    /// Wait until an element matching `by` (name or role) appears (`appears=true`)
    /// or disappears (`appears=false`) in the foreground window, WITHOUT clicking
    /// anything first. The common "wait until the Save button exists" primitive.
    /// Event-bus-driven backoff, same schedule as click_and_wait_for_text.
    #[tracing::instrument(skip(self), fields(?by, appears, timeout_ms))]
    pub async fn wait_for_element(&self, by: By, appears: bool, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let bus = EventBus::global();
        let mut last_seq = bus.seq();
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            let probe = match &by {
                By::Name(n) => self.tree.find_by_name_fast(n).map_err(GhostError::Core)?,
                By::Role(r) => self.tree.find_by_role_fast(r).map_err(GhostError::Core)?,
                By::Description(_) => return Err(GhostError::Vision(
                    "wait_for_element supports name or role, not description".into())),
            };
            if probe.is_some() == appears {
                return Ok(());
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout {
                    action: format!("wait_for_element:{by:?}:appears={appears}"),
                    ms: timeout_ms,
                });
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let backoff = if elapsed_ms < 500 { 25 } else if elapsed_ms < 2000 { 75 } else { 150 };
            if let Ok(s) = bus.wait_for_change(last_seq, backoff).await {
                last_seq = s;
            }
        }
    }

    /// Wait until an element's VALUE (ValuePattern/get_text) satisfies a predicate.
    /// `pred`: "equals" | "contains" | "changes" (changes = differs from the initial
    /// value observed at call start). Returns the final value on success. Polls with
    /// the same event-bus backoff as wait_for_element. The "wait until this field
    /// fills / the total updates / autofill lands" primitive.
    pub async fn wait_for_value(&self, by: By, pred: &str, expected: &str, timeout_ms: u64) -> Result<String> {
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let bus = EventBus::global();
        let mut last_seq = bus.seq();
        // Baseline for "changes".
        let initial = self.find(by.clone()).await.ok().map(|e| e.get_text()).unwrap_or_default();
        loop {
            if is_stopped() { return Err(GhostError::Stopped); }
            let current = match self.find(by.clone()).await {
                Ok(el) => el.get_text(),
                Err(_) => String::new(),
            };
            let satisfied = match pred {
                "equals" => current == expected,
                "contains" => current.contains(expected),
                "changes" => current != initial,
                other => return Err(GhostError::Vision(format!(
                    "wait_for_value: unknown predicate '{other}'; use equals|contains|changes"))),
            };
            if satisfied {
                return Ok(current);
            }
            if start.elapsed() >= deadline {
                return Err(GhostError::Timeout {
                    action: format!("wait_for_value:{by:?}:{pred}={expected:?}"),
                    ms: timeout_ms,
                });
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let backoff = if elapsed_ms < 500 { 25 } else if elapsed_ms < 2000 { 75 } else { 150 };
            if let Ok(s) = bus.wait_for_change(last_seq, backoff).await {
                last_seq = s;
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
        self.act_on_element(el, action, text).await
    }

    /// Collect all elements in the FOREGROUND window matching name and/or role
    /// (both criteria AND-combined when both given). Enables disambiguation:
    /// callers pick the nth match instead of trusting first-match-wins.
    pub async fn find_all_foreground(
        &self,
        name: Option<&str>,
        role: Option<&str>,
        cap: usize,
    ) -> Result<Vec<crate::GhostElement>> {
        let hwnd = crate::tiers::foreground_hwnd();
        if hwnd == 0 {
            return Err(GhostError::Core(ghost_core::error::CoreError::WindowGone));
        }
        let els = self.tree
            .find_all_in_hwnd(windows::Win32::Foundation::HWND(hwnd as *mut _), name, role, cap)
            .map_err(GhostError::Core)?;
        Ok(els.into_iter().map(crate::GhostElement::new).collect())
    }

    /// Extract readable text from a window (or the foreground scope handled by
    /// the caller). Cheap page/app READING without screenshots.
    pub async fn read_text(&self, window: Option<&str>, max_chars: usize) -> Result<(String, bool)> {
        self.tree.collect_text(window, max_chars).map_err(GhostError::Core)
    }

    /// Dispatch an action against an already-resolved element, with foreground
    /// anchoring and screen-delta verification. Shared by act() and the
    /// index-selected (nth-match) path.
    pub async fn act_on_element(
        &self,
        el: crate::GhostElement,
        action: &str,
        text: Option<&str>,
    ) -> Result<serde_json::Value> {
        let name = el.name();

        // Fail fast on disabled controls for ALL actions — the coordinate paths
        // (double_click/right_click/hover) previously bypassed the click/type
        // disabled guard and clicked a dead control, returning a misleading ok.
        if !el.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: name,
                reason: "element is disabled".into(),
            });
        }

        // If the element is scrolled out of view, bring it into view first so its
        // rect is live (a stale off-screen rect makes clicks land on empty space).
        if el.is_offscreen() {
            let _ = el.scroll_into_view();
            // Give the container a beat to settle its layout before re-reading.
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        let rect = el.bounding_rect();

        // Anchor the OS foreground to the window that owns the target element.
        // UIA SetFocus alone fails silently for a background console process
        // (foreground-lock timeout), and every SendInput fallback (double_click,
        // right_click, pattern-miss type path) routes to whichever window holds
        // OS focus — which between MCP calls is usually the client's terminal.
        let focus_confirmed = match rect {
            Some((l, t, r, b)) => {
                let (cx, cy) = ((l + r) / 2, (t + b) / 2);
                tokio::task::spawn_blocking(move || {
                    ghost_core::uia::tree::focus_window_under_point(cx, cy)
                })
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(false)
            }
            None => false,
        };
        // Best-effort UIA focus on top (non-fatal: InvokePattern/ValuePattern work without it)
        let _ = el.set_focus();

        // Reject unknown actions once, up front, before the retry loop.
        if !matches!(action, "click" | "type" | "double_click" | "right_click" | "hover") {
            return Err(GhostError::Vision(format!("ghost_act: unknown action '{action}'; use click|type|double_click|right_click|hover")));
        }

        // Retry-until-verified: dispatch, then check for a screen change. If the
        // action produced NO detected change, re-focus and dispatch once more.
        //
        // ONLY `type` is retryable — it is idempotent (ValuePattern.SetValue and
        // the clear-then-type fallback both REPLACE the field, so a second
        // dispatch can't double the text). click/double_click/right_click are
        // NOT retried: a verified=false is ambiguous (the effect may simply be
        // slower than the ~240ms verify window, e.g. a Submit that navigates), so
        // re-clicking risks a double-submit / double-charge / double-delete.
        // Those actions dispatch once and report verified honestly instead.
        let retryable = action == "type";
        let mut verification;
        let mut attempts = 0u32;
        loop {
            attempts += 1;

            // MEDIUM-5: snapshot the foreground HWND before the action to detect focus changes.
            let fg_before = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            // LOW-9/HIGH-2: capture the foreground region off the tokio thread.
            let capture_rect = self.foreground_window_rect();
            let before_capture = tokio::task::spawn_blocking(move || capture_region_raw(capture_rect))
                .await
                .ok()
                .and_then(|r| r.ok());

            match action {
                "click" => el.click()?,
                "type" => {
                    let t = text.ok_or_else(|| GhostError::Vision("ghost_act: action=type requires text param".into()))?;
                    el.type_text(t)?;
                }
                // HIGH-2: no clean UIA pattern — dispatch coordinate equivalents.
                "double_click" | "right_click" | "hover" => {
                    let (cx, cy) = rect.map(|(l, t, r, b)| ((l + r) / 2, (t + b) / 2))
                        .ok_or_else(|| GhostError::Vision(format!("ghost_act: action={action} requires element with bounding rect")))?;
                    match action {
                        "double_click" => self.double_click_at(cx, cy).await?,
                        "right_click" => self.right_click_at(cx, cy).await?,
                        "hover" => self.hover(cx, cy).await?,
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }

            // Adaptive post-action verification: fast UIs confirm on the first
            // ~40ms capture; async renders get up to ~240ms before "no change".
            verification = self.verify_screen_change(before_capture, fg_before).await;

            let changed = verification.as_ref().map(|v| v.changed).unwrap_or(false);
            let can_verify = verification.is_some();
            if changed || !retryable || !can_verify || attempts >= 2 {
                break;
            }
            // No change and we can verify — re-anchor focus and try once more.
            if let Some((l, t, r, b)) = rect {
                let (cx, cy) = ((l + r) / 2, (t + b) / 2);
                let _ = tokio::task::spawn_blocking(move || {
                    ghost_core::uia::tree::focus_window_under_point(cx, cy)
                }).await;
            }
            let _ = el.set_focus();
        }

        // Paste fallback: if a `type` still shows no change after keystroke
        // retries, the target is likely a rich-text web editor (Monaco,
        // ProseMirror) that ignores both SetValue and synthesized keystrokes but
        // honors a real paste. Escalate once via clipboard paste (save/restore).
        //
        // Gated to EDITABLE elements and made idempotent (select-all before
        // paste = replace): if the earlier type actually worked but verify simply
        // missed a small change (false negative), clear+paste re-produces the SAME
        // final content instead of doubling it.
        let mut used_paste = false;
        if action == "type"
            && el.is_editable()
            && verification.as_ref().map(|v| !v.changed).unwrap_or(false)
        {
            if let Some(t) = text {
                let _ = el.set_focus();
                let fg_before = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                let capture_rect = self.foreground_window_rect();
                let before_capture = tokio::task::spawn_blocking(move || capture_region_raw(capture_rect))
                    .await.ok().and_then(|r| r.ok());
                let _ = ghost_core::input::keyboard::clear_focused_field();
                if self.paste_text(t).await.is_ok() {
                    used_paste = true;
                    verification = self.verify_screen_change(before_capture, fg_before).await;
                }
            }
        }

        let rect_json = rect.map(|(l, t, r, b)| serde_json::json!({"left": l, "top": t, "right": r, "bottom": b}))
            .unwrap_or(serde_json::Value::Null);
        let mut out = Self::act_result_json(name.into(), rect_json, verification, focus_confirmed);
        if let Some(obj) = out.as_object_mut() {
            obj.insert("attempts".into(), serde_json::json!(attempts));
            if used_paste {
                obj.insert("used_paste_fallback".into(), serde_json::json!(true));
            }
        }
        Ok(out)
    }

    /// Coordinate-based action with the same focus-anchoring and verification
    /// guarantees as the UIA path. Used for OCR/VLM-grounded dispatch.
    pub async fn act_at(&self, x: i32, y: i32, action: &str, text: Option<&str>) -> Result<serde_json::Value> {
        // Bring the window under the target point to the foreground BEFORE any input,
        // so clicks/keystrokes land in the window that owns the coordinates.
        let focus_confirmed = tokio::task::spawn_blocking(move || {
            ghost_core::uia::tree::focus_window_under_point(x, y)
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false);

        // Occlusion diagnostic: what element actually sits at (x,y) right now?
        // This is a blind coordinate dispatch (OCR/VLM tier), so the point may be
        // covered by a modal/tooltip that appeared after grounding. Non-fatal —
        // surfaced as hit_element so a mis-hit is diagnosable instead of silent.
        // Also capture whether the hit element is a text field, to gate the
        // destructive clear-before-type below (a mis-grounded type onto an
        // Explorer file list must NOT fire Ctrl+A+Delete).
        let hit = self.tree.element_from_point(x, y).ok().flatten();
        let hit_is_editable = hit.as_ref()
            .map(|e| ghost_core::uia::patterns::is_editable_role(e.control_type()))
            .unwrap_or(false);
        let hit_element = hit.map(|e| e.name());

        let fg_before = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        let capture_rect = self.foreground_window_rect();
        let before_capture = tokio::task::spawn_blocking(move || capture_region_raw(capture_rect))
            .await
            .ok()
            .and_then(|r| r.ok());

        match action {
            "click" => self.click_at(x, y).await?,
            "type" => {
                let t = text.ok_or_else(|| GhostError::Vision("act_at: action=type requires text param".into()))?;
                self.click_at(x, y).await?;
                // Clear existing content first so type replaces rather than
                // appends — but ONLY when the point is a known text field. On a
                // non-editable hit (a file row, a button, the desktop) Ctrl+A +
                // Delete would select-all + delete, so skip the clear there.
                if hit_is_editable {
                    let _ = ghost_core::input::keyboard::clear_focused_field();
                }
                ghost_core::input::keyboard::type_text(t).map_err(GhostError::Core)?;
            }
            "double_click" => self.double_click_at(x, y).await?,
            "right_click" => self.right_click_at(x, y).await?,
            "hover" => self.hover(x, y).await?,
            other => {
                return Err(GhostError::Vision(format!(
                    "act_at: unknown action '{other}'; use click|type|double_click|right_click|hover"
                )));
            }
        }

        let verification = self.verify_screen_change(before_capture, fg_before).await;
        let mut out = Self::act_result_json(serde_json::Value::Null, serde_json::Value::Null, verification, focus_confirmed);
        if let (Some(obj), Some(hit)) = (out.as_object_mut(), hit_element) {
            obj.insert("hit_element".into(), serde_json::Value::String(hit));
        }
        Ok(out)
    }

    /// Poll for a post-action screen delta on the VERIFY_POLL_MS schedule,
    /// early-exiting as soon as a change is detected.
    async fn verify_screen_change(
        &self,
        before_capture: Option<(Vec<u8>, usize, usize)>,
        fg_before: windows::Win32::Foundation::HWND,
    ) -> Option<ghost_core::capture::Verification> {
        let before = before_capture?;
        let mut last = None;
        for delay in VERIFY_POLL_MS {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            // LOW-9: re-query foreground rect each poll (element may have moved/closed).
            let after_rect = self.foreground_window_rect();
            let after_capture = tokio::task::spawn_blocking(move || capture_region_raw(after_rect))
                .await
                .ok()
                .and_then(|r| r.ok());
            if let Some((after, aw, ah)) = after_capture {
                let (ref b, bw, bh) = before;
                if bw == aw && bh == ah {
                    // MEDIUM-5: fg_ok = same window stayed focused, not just any window.
                    let fg_after = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                    let fg_ok = !fg_before.is_invalid() && fg_before == fg_after;
                    let v = compute_verification(b, &after, bw, bh, fg_ok);
                    let changed = v.changed;
                    last = Some(v);
                    if changed {
                        break;
                    }
                }
            }
        }
        last
    }

    /// Shared result shape for act()/act_at(): honest `verified` signal plus a
    /// warning string the calling agent can surface instead of blindly retrying.
    fn act_result_json(
        name: serde_json::Value,
        rect_json: serde_json::Value,
        verification: Option<ghost_core::capture::Verification>,
        focus_confirmed: bool,
    ) -> serde_json::Value {
        let verified = verification.as_ref().map(|v| v.changed);
        let verification_json = verification
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);
        let mut out = serde_json::json!({
            "ok": true,
            "name": name,
            "rect": rect_json,
            "verified": verified,
            "focus_confirmed": focus_confirmed,
            "verification": verification_json,
        });
        if verified == Some(false) {
            out["warning"] = serde_json::Value::String(
                "action dispatched but no screen change detected within 240ms; verify with ghost_see before retrying".into(),
            );
        } else if !focus_confirmed {
            out["warning"] = serde_json::Value::String(
                "target window foreground could not be confirmed; input may have landed elsewhere".into(),
            );
        }
        out
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
                    if let Ok(s) = bus.wait_for_change(last_seq, backoff).await { last_seq = s; }
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

#[cfg(test)]
mod act_result_tests {
    use super::GhostSession;
    use ghost_core::capture::Verification;
    use serde_json::json;

    fn v(changed: bool, fg: bool) -> Verification {
        Verification { changed, delta_score: if changed { 1.0 } else { 0.0 }, foreground_ok: fg }
    }

    #[test]
    fn verified_action_has_no_warning() {
        let out = GhostSession::act_result_json(json!("OK"), json!(null), Some(v(true, true)), true);
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["verified"], json!(true));
        assert_eq!(out["focus_confirmed"], json!(true));
        assert!(out.get("warning").is_none());
    }

    #[test]
    fn unverified_action_carries_warning_not_silent_success() {
        let out = GhostSession::act_result_json(json!("OK"), json!(null), Some(v(false, true)), true);
        assert_eq!(out["verified"], json!(false));
        let w = out["warning"].as_str().unwrap();
        assert!(w.contains("no screen change"), "warning: {w}");
    }

    #[test]
    fn unconfirmed_focus_carries_warning() {
        let out = GhostSession::act_result_json(json!("OK"), json!(null), Some(v(true, true)), false);
        assert_eq!(out["verified"], json!(true));
        let w = out["warning"].as_str().unwrap();
        assert!(w.contains("foreground"), "warning: {w}");
    }

    #[test]
    fn missing_verification_reports_null_not_true() {
        let out = GhostSession::act_result_json(json!("OK"), json!(null), None, true);
        assert!(out["verified"].is_null());
    }
}
