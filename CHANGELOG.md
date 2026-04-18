# Changelog

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
