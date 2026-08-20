# Changelog

## [0.5.0] - Concurrency

Multiple Claude sessions, each with its own ghost MCP server, can now automate at
full speed simultaneously - and a single session's parallel tool calls actually run
in parallel instead of queueing.

### Fixed

- **The MCP server ran one request at a time.** The stdio loop awaited each handler
  before reading the next line, so one slow call (a 15s `ghost_tab_wait_for`, a
  browser launch) stalled every request behind it - the "ghost lags between tasks"
  experience, since Claude issues parallel tool calls. Each request now runs as its
  own tokio task against a shared session; responses are written on completion and
  correlated by JSON-RPC id. Measured: five fast calls issued after a 4s call now
  finish in 5ms instead of 4s.
- **The emergency stop never fired.** `RegisterHotKey(None, ..)` binds the hotkey to
  the *calling* thread's message queue, but the message pump ran on a different
  thread, so WM_HOTKEY arrived at a queue nobody read. Registration and the pump now
  share one thread.
- **A second ghost process had no emergency stop at all.** Only one process can own a
  hotkey; the second got ERROR_HOTKEY_ALREADY_REGISTERED and silently carried on
  unstoppable. The stop signal is now a named kernel event shared across the session:
  the hotkey owner broadcasts, every ghost process watches, one Ctrl+Alt+G stops them
  all, and `ghost_reset` resumes them all. Ownership migrates if the owner exits.
  Verified live across two MCP server processes.
- A named kernel event was being opened, signaled, and immediately closed - and a
  named object is destroyed with its last handle, taking the signal with it. The
  event handle is now held for the life of the process.

### Added

- **`GhostSession` is `Send + Sync`** and shared across request tasks. UIA runs in
  the multithreaded COM apartment, where cross-thread calls are legal; every tokio
  worker joins the MTA at startup, and a compile-time assertion in the server keeps
  the session shareable.
- **Named browser selection**: `ghost_browser_launch {"browser": "comet"}` launches a
  specific installed Chromium-family browser - Comet, Chrome, Edge, or Brave - all
  driven identically over CDP. `ghost_browser_list_installed` reports what is on the
  machine. Verified live against Comet.
- Concurrency e2e covering parallel dispatch in one server, three tabs driven by
  interleaved request bursts with no cross-talk, and three MCP servers (three Claude
  sessions) automating simultaneously in 1.24s wall clock with the foreground
  untouched throughout.

### Changed

- `ghost-intent`'s `OpsDispatcher` is now a `Send` trait (`async_trait` without
  `?Send`); dispatchers must be `Sync`.

## [0.4.1] - Performance

Measured with `cargo run --release -p ghost-session --example bench`, on the same
machine and target app before and after.

| Operation | Before | After |
|---|---:|---:|
| Browser click | 5.01s | 1.44ms |
| Browser screenshot | 96.9ms | 64.2ms |
| `describe_screen` (whole desktop) | 757.7ms | 127.1ms |
| `describe_screen` (one window) | 46.7ms | 35.6ms |
| `find(role)` unscoped | 33.3ms | 22.4ms |
| 3 tabs driven concurrently | 5.78s | 0.54s |
| 3 concurrent ghost processes | 31.8s | 1.90s |
| Benchmark wall clock | 113.8s | 5.3s |

### Fixed

- **Every browser click cost 5.01 seconds.** `Input.dispatchMouseEvent` with
  `mouseMoved` is only acknowledged once the renderer produces a compositor frame, and
  a background or headless tab produces none, so the reply arrived on Chrome's internal
  ~5s timeout. The move is now dispatched without awaiting its acknowledgement: CDP
  processes a session's commands in arrival order, so the button press that follows is
  still handled after it. Verified with `examples/click_probe.rs`, and
  `browser_background_proof` now fails if the 3-tab run exceeds a 3s budget.
- **`ControlViewWalker()` was called at every node of every tree walk.** It is itself a
  cross-process COM call, so a thousand-element tree paid a thousand extra round trips
  before reading a single property. Created once now.
- **Tree searches read one property at a time, per element.** `find`, `describe_screen`,
  and the accelerator lookup now issue a single `FindAllBuildCache` that traverses
  inside UI Automation and returns the batch with properties pre-fetched; matching runs
  against the local snapshot. Role lookups became a real `FindFirstBuildCache` property
  condition resolved server-side.
- **A permanent device-metrics override made screenshots 8.4s each.** It is the
  *transition* of setting the override that forces a repaint, not its presence, so
  leaving it in place let the tab go stale and stall again. Ghost-owned browsers now use
  `Page.bringToFront` (invisible, 58ms); attached browsers keep the set/capture/clear
  cycle (83ms) because bringing a tab to the front in the user's own browser would
  switch their tab. Measured with `examples/shot_probe.rs`.
- **Wait helpers polled on a flat 100ms interval**, so every wait paid up to 100ms even
  though almost all succeed on the first check. Now 5ms with exponential backoff to
  100ms.
- `Tab::click` tries locating the element directly and only falls back to waiting when
  it is genuinely absent, turning the common case into one round trip instead of two.

### Removed

- `uia::cached_walker`, superseded by the batched search in `uia::tree`, and the four
  recursive walk helpers it duplicated. Two implementations of the same traversal, one
  of them dead, was how the slow path survived unnoticed.

### Added

- `Cdp::notify` for commands whose acknowledgement is slow and worthless.
- `Tab::hover(selector)`, with an explicit barrier so it does not return before the
  renderer has processed the move.
- `examples/bench.rs`, `examples/click_probe.rs`, `examples/shot_probe.rs`.

## [0.4.0] - Background Automation

Ghost now runs without taking over the screen, which was previously a claim rather
than a behaviour. The user can keep typing and using their mouse, and several ghost
processes can automate different apps and browser tabs on one machine simultaneously.

### Added

- **Focus policy** (`ghost-core::focus`): `background` (new default),
  `prefer_background`, `foreground`. Every `SendInput` / `SetForegroundWindow` path is
  gated behind it and returns `NoBackgroundPath` rather than grabbing the screen.
  Settable via `GHOST_FOCUS_POLICY` or `ghost_set_focus_policy`.
- **`ghost-browser` crate**: Chrome DevTools Protocol client. Per-tab navigation,
  trusted synthetic clicks, typing, key events, JS evaluation, screenshots, and a
  structured element description - all delivered into a tab's own renderer, so a tab
  does not need to be in front and its window does not need focus. Isolated launch
  (own process, profile, and port per id), plus attach-to-existing.
- **Expanded UIA action chain**: Invoke, SelectionItem, Toggle, ExpandCollapse, and
  the LegacyIAccessible bridge for activation; Value and LegacyIAccessible for text;
  plus Scroll, ScrollItem, RangeValue, and TextPattern. Actions report the route they
  took (`uia:invoke`, `foreground:sendinput`, ...) so a fallback is never silent.
- **Window-message input backend**: hit-tests to the deepest child HWND, correct
  scan-code and extended-key lParams, WM_CHAR typing, wheel, right/double click, and
  WM_SETTEXT.
- **`capture_window`**: `PrintWindow` with `PW_RENDERFULLCONTENT`, so a single window
  can be captured while occluded and without being raised. Blank results are detected
  and reported instead of returned as a black image.
- **`DesktopSnapshot`/`DesktopDelta`** and `ghost_desktop_state`: measure whether the
  foreground window or cursor moved, so "it ran in the background" is verifiable.
- **Cross-process foreground lease**: a session-local named mutex serializing the one
  shared input desktop whenever a fallback does need real input.
- **Window-scoped locators**: `find_in` / a `window` parameter on every element tool.
  Unscoped searches walk every open window and can match another ghost process's
  target.
- 40 new MCP tools covering the background element actions, window-scoped input,
  browsers, and tabs.
- **`ghost-cli` crate**: a `ghost` command-line binary exposing the same tool surface
  as the MCP server, sharing one dispatch layer so the two cannot drift. One-shot
  commands for window work, and `ghost run` for scripts that need state (isolated
  desktops, launched browsers) to survive across steps.
- **Isolated desktops** (`ghost-core::desktop`): create a second Windows desktop that
  is never displayed and launch an app onto it. Its windows never appear on the user's
  screen at all. UI Automation, window messages, and `PrintWindow` capture all work
  there; `SendInput` does not, and that is documented and measured rather than
  discovered at runtime. 13 new MCP tools plus CLI commands.
- **`input::shortcut`**: undo, cut, copy, paste, clear, and select-all as the standard
  control messages (`WM_UNDO`, `WM_CUT`, `WM_COPY`, `WM_PASTE`, `WM_CLEAR`,
  `EM_SETSEL`). Exposed as `ghost_shortcut_background` and folded into the
  `hotkey_background` chain.
- **Per-monitor DPI awareness** declared at session startup.
- **Screen-crop capture fallback** when `PrintWindow` renders an empty buffer, gated
  on verifying the window actually owns the pixels at its own centre.
- Five live proofs: `browser_background_proof`, `desktop_background_proof`,
  `concurrency_proof`, `isolated_desktop_proof`, `desktop_input_probe`.

### Fixed

- **Background Ctrl+Z corrupted documents.** Posting `WM_KEYDOWN` `VK_CONTROL` does not
  change the target thread's key state, so the shortcut arrived as a literal `z`.
  `hotkey_background` now tries the standard control message, then the command that
  advertises the accelerator, then errors - it never fakes a modifier.
- **Coordinates were wrong on scaled displays.** The process was DPI-unaware, so
  `GetWindowRect` and `GetSystemMetrics` returned virtualized logical pixels while UIA
  reported physical ones. On a 150% display every click computed from an element
  rectangle landed a third of the way off.
- **`PrintWindow` blank buffers were returned as black images.** Now detected, with a
  verified screen-crop recovery path and an explicit error when recovery is unsafe.
- **`ghost_navigate_and_wait` typed URLs with the real keyboard.** It now routes
  through CDP whenever a browser is registered, and reports which route it took.
- `EnumDesktopWindows` results are filtered to visible windows: an app has several
  helper windows (`GDI+ Window`, message-only) and title matching picked the wrong one.

- **UIA control type ids were wrong across the board** (`50` for button, `42` for
  edit, and others off by whole entries). `find_by_role` and `describe_screen` could
  not match most controls, which forced callers onto pixel coordinates and the
  foreground. Now mapped to the real `UIA_*ControlTypeId` constants, with
  `INTERACTIVE_ROLES` widened to cover menu items, list items, links, and more.
- **Background hotkeys silently corrupted documents.** Posting `WM_KEYDOWN`
  `VK_CONTROL` does not change the target thread's key state, so `Ctrl+Z` was
  delivered as a literal `z`. `hotkey_background` now resolves the accelerator to the
  command that owns it and invokes that, or errors; it never fakes a modifier.
- **A launched browser inherited ghost's stdio**, letting Chrome write into
  `ghost-mcp`'s JSON-RPC stream on stdout and keeping the parent's pipes open for the
  browser's lifetime.
- **`Page.captureScreenshot` hung forever on a non-compositing background tab.** Now
  forces a frame with `Emulation.setDeviceMetricsOverride`, with a bounded timeout, an
  owned-browser-only `Page.bringToFront` retry, and a clear error otherwise.
- The element-not-found diagnostic no longer captures the user's whole desktop under
  the background policy; a scoped search attaches only the window it searched.

### Changed

- `GhostElement::click` / `type_text` return the `ActionRoute` taken instead of `()`.
- `ghost_navigate_and_wait` is marked legacy/foreground-only; use
  `ghost_browser_launch` + `ghost_tab_navigate`.

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
