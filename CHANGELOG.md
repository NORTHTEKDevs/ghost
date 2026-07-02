# Changelog

## [0.7.3] - 2026-07-02 — Actionability, Waits, Structured Errors

### Added

- **`ghost_wait for=element`**: wait for an element (by name/role) to appear or
  disappear WITHOUT clicking anything first — the "wait until Save exists"
  primitive agents constantly need. Event-bus-driven backoff.
- **Structured errors**: every failing tool call now carries an `error_code`
  and a `suggested_action` (e.g. element-not-found → "call ghost_see to confirm
  focus, or retry with mode=deliberate"), classified from the error. Was a bare
  opaque string with a single generic -32000 code.
- **`ghost_query` provenance + correctness**: now reads each field's VALUE
  (ValuePattern/get_text) instead of echoing the element's NAME (which returned
  labels like "Email:" instead of the actual value), and reports a per-field
  `sources` map (uia|vlm).
- **Clear-before-type**: `type` now replaces existing field content instead of
  appending, on the keyboard-fallback and coordinate paths too (UIA
  ValuePattern already replaced). Gated behind an editable-role check so a
  mis-grounded type can never fire Ctrl+A+Delete on a file list / non-text focus.
- **Retry-until-verified for `type`**: an unverified `type` re-dispatches once
  (safe — SetValue/clear-then-type are idempotent). Click/double/right-click are
  deliberately NOT auto-retried: a slow-but-successful click must never be
  double-fired (double-submit/charge/delete). Results carry an `attempts` count.
- **Occlusion diagnostic**: coordinate-dispatch actions report `hit_element`
  (what actually sits at the click point) so a mis-hit is diagnosable.

### Tests

- 338 passing (was 337). New coverage for error classification.

## [0.7.2] - 2026-07-01 — Multi-Monitor & Interaction Robustness

### Added / Fixed

- **Multi-monitor capture**: screenshots and act-verification now work on ANY
  monitor. The DXGI fast path only duplicates the primary output; rects that
  fall off-primary (or straddle a monitor boundary) now route to a GDI
  virtual-screen capture that spans the whole desktop (negative coordinates for
  monitors left of / above the primary included). Previously an action or
  screenshot on a secondary monitor silently cropped garbage from the primary.
- **Scroll-into-view before acting**: `ghost_act` detects UIA-offscreen elements
  (scrolled out of a list, collapsed, hidden tab) and calls ScrollItemPattern +
  a 60ms settle before re-reading the rect, so clicks in long/virtualized lists
  land on the real element instead of empty space.
- **Disabled-control guard on all actions**: `double_click`/`right_click`/`hover`
  now fail fast on a disabled element like `click`/`type` already did (was a
  silent no-op returning ok).

### Tests

- 337 passing (was 334). New coverage for virtual-screen / on-primary rect
  routing helpers.

## [0.7.1] - 2026-07-01 — Targeting, Reading, Preemption

### Added

- **`window` param on `ghost_find`/`ghost_act`**: focuses + confirms the target
  window (title substring) before resolution, so UIA fast paths, OCR crops, and
  cache keys all anchor to the INTENDED window in multi-window flows — closes
  the remaining wrong-window-first-match hole.
- **`index` param on `ghost_find`/`ghost_act`**: act on / return the nth match
  (0-based) when several elements share a name/role (e.g. multiple "Close Tab"
  buttons); name+role AND-combine on this path and responses carry a `matches`
  count. Out-of-range gives an actionable error with the match count.
- **`ghost_see mode=text`**: extract the readable text of a window/page
  directly from the accessibility tree (names of text-carrying roles +
  ValuePattern content of edit/document). The cheapest way to READ a page —
  no screenshot, no element dump. `limit` = char cap (default 20000).
- **Preemptible emergency stop**: stdin now runs on a dedicated reader thread;
  a `ghost_stop` request sets the stop flag the moment it ARRIVES instead of
  waiting in the serial queue. Live-verified: a queued 10s `ghost_wait` is
  interrupted in ~1ms. `ghost_wait` also polls the stop flag (100ms) and
  reports interruption as an error instead of sleeping through it.
- **Content-change WinEvent hooks** (`EVENT_OBJECT_VALUECHANGE` / `REORDER` /
  `SHOW`): find()/wait primitives now wake on text updates, list mutations,
  and elements becoming visible instead of falling back to 25-150ms polling;
  a 10ms debounce on the wake path prevents walk-thrash during event bursts.

### Tests

- 333 passing (was 330). Live e2e: stop preemption 1ms, mode=text reads typed
  content, index disambiguation with matches count.

## [0.7.0] - 2026-07-01 — Reliability & Latency Overhaul

Root-caused and fixed the three classes of field failures: actions that need
multiple calls to fire, silent wrong-window input, and perceived lag.

### Fixed — actions that "don't fire"

- **OS-foreground anchoring on every action path.** `ghost_act` now brings the
  target element's own window to the foreground (AttachThreadInput + confirm)
  before dispatching input. Previously only UIA `SetFocus()` was attempted —
  it fails silently for a background console process, so SendInput fallbacks
  (double_click, right_click, pattern-miss typing) landed in whichever window
  had focus, usually the MCP client's own terminal.
- **`ghost_key` gained a `window` param.** Keyboard SendInput routes to the OS
  focus owner; with `window` set the target is focused + confirmed first and
  the call FAILS LOUDLY if focus can't be confirmed, instead of typing into the
  wrong app and returning ok:true.
- **Honest verification.** `ghost_act` responses now carry `verified`
  (screen-delta detected), `focus_confirmed`, and a `warning` when an action
  dispatched but nothing visibly changed. Previously `ok:true` was hardcoded,
  so silent no-ops looked like success and clients re-issued the action.
- **Coordinate-tier actions (OCR/VLM) get the same verification** — that path
  previously had none at all.
- **Adaptive post-action verify window** (40→240ms early-exit polling) instead
  of a single fixed 50ms capture that false-negatived async renders
  (web/Electron).

### Fixed — windows agents lose track of

- `ghost_window op=list` now includes minimized windows (with a `state` field:
  normal|minimized) — Win11 cloaks some minimized windows (e.g. Notepad) and
  they vanished from the list entirely while still alive.
- `ghost_window op=focus` auto-restores minimized windows before focusing.
- `ghost_see mode=full window=X` with an unknown window is now an ERROR that
  lists the open windows — previously it silently walked the ENTIRE desktop and
  returned a huge dump including -32000 garbage coords from minimized windows.
- Minimized-window scope requests return an actionable error ("restore it
  first") instead of garbage coordinates.

### Fixed — latency

- **VLM timeout 30s → 8s** (configurable via `GHOST_VLM_TIMEOUT_MS`), with one
  bounded retry on connect errors/5xx (never on timeout). A silent VLM
  escalation could previously block the serial stdio loop — and every queued
  request behind it — for 30 seconds.
- `ghost_find`/`ghost_act` responses expose `escalated: true` when local tiers
  missed and a network VLM call was paid, so hidden latency is visible.
- **On-device OCR bounded at 3s** (WinRT spin-wait previously had no timeout).
- **UIA desktop walks capped at 3000 visited nodes** — a DOM-heavy Chromium
  window could previously turn one find into an unbounded COM-call storm.
- **DXGI black-frame flag is no longer permanent**: re-probes every 100 GDI
  captures so a transient event (sleep/resume, driver reset) doesn't downgrade
  every future screenshot to the 30-100ms GDI path forever.
- `ghost_screenshot full=true` returns a 1280px JPEG by default instead of a
  native-resolution lossless PNG (multi-MB base64 over stdio); pass `max_dim=0`
  for the old behavior.
- `ghost_see`/describe responses filter zero-area and off-screen elements and
  cap at 150 elements by default (`limit` param, 0 = unlimited) — element dumps
  were the top source of client-side context bloat.
- Every `tools/call` response envelope now includes `ms` (server-side latency).
- Locator cache entries expire after 30s (TTL) — bounds the window where a
  re-rendered UI could pass point-validation on a coincidentally-matching
  element and cause a wrong-target click.
- Tokio runtime pinned to `current_thread` flavor, making the COM-STA
  single-thread invariant structural instead of accidental.

### Tests

- 330 passing (was 320), including new coverage for element filtering/limits,
  act-result honesty (verified/warning), verification sensitivity (typed-text
  detection, noise tolerance), and cache behavior.

## [0.5.0] - 2026-05-07 — Local OCR + Multi-Provider Vision

### Added

- **NVIDIA Build vision provider** (default): OpenAI-compatible client in
  `ghost-session/src/vision.rs`. Uses `meta/llama-3.2-90b-vision-instruct`
  by default at `https://integrate.api.nvidia.com/v1/chat/completions`.
  Free with NVIDIA Developer signup (~zero local disk).
  Set `NVIDIA_API_KEY`. Falls back to Anthropic if key absent.
- **Multi-provider selection**: `GHOST_VISION_PROVIDER=nvidia|anthropic`
  for explicit choice. `GHOST_VISION_BASE_URL` overrides for self-hosted
  Ollama / vLLM / llama.cpp servers (any OpenAI-compat endpoint).
  `GHOST_VISION_MODEL` overrides per-provider default.
- Robust JSON response parsing: handles bare JSON, code-fenced JSON, and
  prose-wrapped JSON across providers.
- 5 new unit tests on response parsing + provider selection (97 total).
- **Windows.Media.Ocr integration** (`ghost-core/src/ocr.rs`):
  on-device, free, no API. SoftwareBitmap built from raw BGRA via
  IBufferByteAccess memcpy. OcrEngine init via TryCreateFromUserProfileLanguages
  with en-US fallback. Returns Vec<OcrWord{text, BoundingRect}> in absolute
  screen coords.
- Session methods: `find_text_local(needle, foreground)` and
  `click_text_local(needle, timeout_ms)` with event-driven backoff.
- 2 new MCP tools: `ghost_find_text_local`, `ghost_click_text_local`.
  Pair with vision fallback: try OCR first (free, ~50-200ms), then
  ghost_locate_by_description (paid API, ~1-3s) only on miss.
- Public helpers in `capture/screen.rs`: `capture_screen_full_rgba`,
  `rgba_to_bgra_in_place` for non-encoding consumers.

### Dependencies

- `windows-core = "0.58"` direct dep on ghost-core (required by
  `windows::core::implement` macro expansions).
- Workspace `windows` features added: `Foundation`, `Foundation_Collections`,
  `Globalization`, `Graphics_Imaging`, `Media_Ocr`, `Storage_Streams`,
  `Win32_System_WinRT`.

### Deferred from v0.5

- **Full IUIAutomation event handlers**: windows-rs 0.58 does not
  auto-generate `_Impl` traits for `IUIAutomationFocusChangedEventHandler`
  / `IUIAutomationStructureChangedEventHandler`. Implementing them needs
  raw VTABLE-level COM (200+ LOC unsafe, deadlock risk on UIA threadpool,
  SAFEARRAY param marshaling). `EventBus::bump()` retained as the public
  hook for when this gets wired (~5 line integration).
- **LocatorStore read path**: write path is trivial; the value comes from
  the read path which needs `IUIAutomation::ElementFromPoint` integration
  to verify a cached rect before skipping the walk. Without that,
  populating the store gains nothing.

## [0.4.0] - 2026-05-07 — Hot Path Overhaul

Targets browser-automation-grade latency for the desktop. Six-phase upgrade to
the action loop, screenshot pipeline, and locator system. 92 tests passing.

### Added

- **Scoped UIA search** (`tree.rs`): `find_by_name_fast` / `find_by_role_fast` /
  `describe_screen_fast` use `IUIAutomation::ElementFromHandle` rooted at the
  foreground HWND. Typically 10-100x faster than the prior full-desktop walk;
  falls back to desktop scope on miss.
- **System event bus** (`uia/event_bus.rs`): `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`
  on a dedicated pump thread populates a global `EventBus` with sequence
  counter + `tokio::sync::Notify`. `wait_for_change(since_seq, timeout_ms)`
  is spurious-wakeup-safe and races to <5ms wakeup latency.
- **Region/JPEG capture** (`capture/screen.rs`): `capture_screen_region(rect, max_dim, format)`
  + `CaptureFormat::{Png, Jpeg(quality)}`. Refactored DXGI path into shared
  `capture_rgba` helper. Recommended vision payload preset:
  foreground rect + max_dim=768 + jpeg q75 = 10-50x smaller than full PNG.
- **Vision fallback** (`session/vision.rs`): `By::Description("the blue Submit button")`
  routes through Claude Messages API with model tiering (Haiku default, Opus
  via `GHOST_VISION_MODEL`). Capture → ROI → downscale → vision_locate → coord
  back-translation. New session methods: `locate_by_description`,
  `click_by_description`, `type_by_description`. Requires `ANTHROPIC_API_KEY`.
- **8 new MCP tools** (45 total):
  - `ghost_describe_screen_fast` — foreground-scoped describe.
  - `ghost_screenshot_region` — ROI + downscale + JPEG/PNG selection.
  - `ghost_event_seq` / `ghost_wait_for_event` — direct event-bus access.
  - `ghost_locate_by_description` / `ghost_click_by_description` /
    `ghost_type_by_description` — vision fallback ergonomic surface.
  - `ghost_batch_actions` — single MCP round-trip for N ops, replaces
    multiple sequential tool calls in agent flows.
- **Tracing instrumentation** on `find`, `click_and_wait_for_text`,
  `screenshot_region`, `wait_for_event`, `describe_screen_fast` with
  per-action lookup-microsecond fields. `RUST_LOG=ghost_session=debug` for
  per-call latency.

### Changed

- `find()` polling: fixed 100ms → tiered backoff (25ms warm / 75ms / 150ms),
  now driven by `EventBus::wait_for_change` for event wakeups during the
  backoff window.
- `click_and_wait_for_text` and FSM `Op::WaitForText`: replaced full
  `describe_screen` rewalks with scoped `find_by_name_fast` probes.
  ~50x cheaper per poll.
- `By` enum gained `Description` variant for vision-bound locators.
- Workspace `image` dep gains `jpeg` feature.

### Fixed

- Spurious-wakeup safety in `EventBus::wait_for_change`: re-checks `seq` after
  each `Notify` wake; only returns Ok when seq has actually advanced past
  `since_seq`.

## [0.3.0] - 2026-04-18 — Speed Overhaul

### Added

- **`ghost-cache` crate**: event-driven UIA mirror with snapshot/delta API, 8-slot
  history ring, SQLite-backed `LocatorStore` with schema v1 and cold/warm/drift
  lookup + eviction.
- **`ghost-intent` crate**: JSON intent compiler, JSONLogic subset evaluator, FSM
  executor with `abort_if` / `retry_if` + exponential backoff + deadline gate.
- **`StaPool`**: STA-threaded UIA worker pool with `catch_unwind` panic recovery,
  3-panics-in-60s circuit breaker, and per-job tokio timeout.
- **`CachedTreeWalker`**: batched `IUIAutomationCacheRequest` + `FindAllBuildCache`
  for 10 UIA properties in one round-trip.
- **`IdleDetector`**: blake3-hashed frame capture with stable-frame detection.
- **`BackgroundClicker`**: PostMessage-based `WM_LBUTTONDOWN/UP` with `IsWindow` gate.
- 10 new MCP tools: `ghost_wait_until`, `ghost_wait_for_idle`, `ghost_navigate_and_wait`,
  `ghost_click_and_wait_for_text`, `ghost_fill_form`, `ghost_execute_intent`,
  `ghost_describe_screen_delta`, `ghost_click_background`, `ghost_cache_stats`,
  `ghost_cache_invalidate` — total 37.
- sonic-rs response encoder (3-5x faster on large payloads) with serde_json fallback.
- Criterion benches (`cargo bench -p ghost-intent`) and `docs/benches/v030-baseline.md`.
- `chaos` feature flag for failure-injection tests.

### Changed

- `OpsDispatcher` trait is `?Send` to accommodate `!Send` COM handles on `GhostSession`.
- `ghost-mcp` `recursion_limit = "512"` to fit the 37-tool `json!` macro.

### See

- Design: `docs/2026-04-17-ghost-v030-speed-overhaul.md`
- Plan: `docs/plans/2026-04-18-ghost-v030-speed-overhaul.md`

## [0.2.0] - 2026-04-17

### Added

- `ghost_reset` MCP tool: resume automation after `ghost_stop`
- MCP protocol compliance: `initialize`, `initialized`, and `tools/list` methods
- `tools/list` returns full inputSchema for all 25 tools (MCP 2024-11-05 spec)
- 17 new MCP tools bringing total to 25 (full human-input parity)
- **Input:** `ghost_press`, `ghost_hotkey`, `ghost_key_down`, `ghost_key_up`
- **Mouse:** `ghost_hover`, `ghost_right_click`, `ghost_double_click`, `ghost_drag`, `ghost_scroll`
- **Clipboard:** `ghost_get_clipboard`, `ghost_set_clipboard`
- **Windows:** `ghost_list_windows`, `ghost_focus_window`, `ghost_window_state`
- **Perception:** `ghost_describe_screen`, `ghost_get_text`
- **Control:** `ghost_wait`
- `name_to_vk`: string-to-VIRTUAL_KEY mapping (Enter, Tab, Escape, F1-F12, arrows, A-Z, 0-9)
- `ElementDescriptor` and `WindowInfo` types exported from `ghost-session`
- Emergency stop (Ctrl+Alt+G) now idempotent across multiple GhostSession instances

### Fixed

- `SetClipboardData` failure now properly frees HGLOBAL handle before returning error
- `EmptyClipboard` errors are now propagated instead of silently ignored
- Clipboard null-terminator scan bounded to 10M characters (prevents runaway on malformed data)
- `SendInput` partial failures now return `Err` instead of logging a warning and returning `Ok`
- `RegisterHotKey` error now uses windows-rs error code directly (no GetLastError race)

## [0.1.0] - 2026-04-01

### Added

- Initial release: 7 MCP tools over stdio JSON-RPC
- `ghost_find`, `ghost_click`, `ghost_type`, `ghost_click_at`, `ghost_screenshot`, `ghost_launch`, `ghost_stop`
- UI Automation element tree search with `By::name` and `By::role` locators
- DXGI Desktop Duplication screen capture (PNG, base64)
- Emergency stop: Ctrl+Alt+G global hotkey, STOP_FLAG atomic
- 3-crate workspace: `ghost-core`, `ghost-session`, `ghost-mcp`
