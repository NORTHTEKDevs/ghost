# Ghost v0.6.0 — "Best-in-World" Overhaul Design

**Date:** 2026-05-30
**Status:** Approved (full overhaul, all phases; local tier = OmniParser-YOLO only)
**Recon source:** `docs/research/2026-05-30-ghost-recon.json` (6 competitor teardowns + 8 crate audits)

## Goal

Make Ghost the fastest, most accurate, most performant desktop + browser automation
system. Optimize four axes simultaneously: **reliability, intelligence, speed, DX.**

## Diagnosis — why it currently fails across the board

Root causes, each confirmed in multiple crate audits:

| # | Failure | Root cause | Crates affected |
|---|---------|-----------|-----------------|
| 1 | **Focus race** | `SendInput` is focus-dependent; `focus_window()` is fire-and-forget (`SetForegroundWindow`, no verify/retry); no UIA `SetFocus`/`ValuePattern.SetValue`/`InvokePattern` bypass. Claude Code terminal steals focus between tool calls. | core, session, intent, mcp, http, cli |
| 2 | **Vision silently dead** | `NVIDIA_API_KEY` only set in comet-mcp wrapper; direct `ghost` registration has `env: {}`. `describe_screen`/`locate_by_description` hard-fail or silently fall back. | core, session, mcp, intent |
| 3 | **Stale binary** | Live `ghost-mcp.exe` is v0.3.0; v0.5.0 work (vision.rs, ocr.rs, event_bus.rs, core/session edits) is uncommitted and never built into the registered binary. | all |
| 4 | **Screenshot bloat** | `ghost_screenshot` returns full-res lossless PNG (1–5 MB) inline as base64 every call. | core, mcp, http |
| 5 | **Cache is dead code** | `UiaCache.apply_snapshot` has no caller; `LocatorStore` constructed nowhere; `cache_seq` never advances → `wait_until` silently broken. | cache, session |
| 6 | **`execute_intent` dead-end** | FSM has no feedback loop; `retry_if` can't observe what failed. | intent |

Performance bugs (HIGH):
- DXGI: new staging texture + `DuplicateOutput` **per screenshot** (should cache the duplication per session).
- UIA walker re-acquires `TreeWalker` **per node** (N+1 COM proxies); walks **whole desktop** not foreground scope.
- `IdleDetector::wait_stable` hashes **full PNG bytes** instead of downsampled raw pixels.
- OCR calls `IAsyncOperation::get()` (blocking spin-wait) **inside a tokio task** → starves the runtime.
- `IUIAutomation` initialized in **MTA (COINIT_MULTITHREADED)** but it is **STA-only**.
- `role_id_to_name` uses **wrong numeric constants** → role locators mislabel/miss.
- JSON-RPC error objects **missing required `code` integer field**.

## Ghost's unfair advantage

Every competitor (UI-TARS, Agent-TARS, Midscene, GUI-Owl) is **pure-vision**: a VLM
regresses click coordinates in a 0–1000 normalized space from the raw screenshot
(accurate, but 1–4 s/step, cloud-bound, costly). OmniParser adds YOLO icon detection +
set-of-marks. **None have the accessibility tree.**

Ghost is the only stack with **real OS input (undetectable) + UIA tree + on-device OCR +
VLM in one Rust process.** The winning architecture is a **hybrid grounding cascade** the
pure-vision rivals cannot copy.

## Target architecture

### Grounding cascade (`ghost-ground`, new crate)

A target (name | role | description | text | coords) resolves through tiers; the first
tier above its confidence threshold wins. Each tier returns `Grounded { rect, center,
confidence, source }`.

```
[1] Validated locator cache    ~0 ms    resurrected ghost-cache; rect re-validated via ElementFromPoint before use
[2] UIA tree match             ~5 ms    exact, free; scoped to foreground; cached TreeWalker; correct role constants
[3] On-device OCR (WinRT)       ~50 ms   free; Windows.Media.Ocr; for text targets
[4] OmniParser-YOLO (ONNX)      ~150 ms  local icon/interactable detector → set-of-marks; VLM picks an ID (no coord regression)
[5] Cloud VLM coord regression  ~1-3 s   last resort; 0–1000 normalized coordinate contract; typed action parser
```

Canonical coordinate contract: **0–1000 normalized integers**, `px = round(coord * dim / 1000)`.
Matches UI-TARS / Qwen2.5-VL / Midscene native output, DPI-safe, resolution-agnostic.

### Reliability core

- **Focus-safe execution** (`ghost-session::input`): before any `SendInput`, assert
  foreground (`SetForegroundWindow` + poll `GetForegroundWindow()==target` with bounded
  retry + small settle). Prefer **focus-independent UIA** where the element supports it:
  `IUIAutomationElement::SetFocus()`, `ValuePattern::SetValue` for text, `InvokePattern`/
  `TogglePattern` for activation. `SendInput` only as last-resort fallback.
- **Act-then-verify:** every mutating action returns a `Verification { changed, delta_score,
  foreground_ok }` computed from a downsampled perceptual hash (raw pixels, not PNG) of the
  target region before/after.
- **Atomic composite tools:** `ghost_act` does find→focus→act→verify in **one MCP call**,
  eliminating the cross-tool-boundary focus race.
- **Reflection ring buffer:** per-session bounded buffer of last-5 `(obs_hash, action,
  outcome)`; on failure, the next VLM prompt is prefixed with a negative hint.

### Speed core

- DXGI `DuplicateOutput` + staging texture cached per session (re-init only on device-lost).
- UIA walk scoped to foreground HWND; `TreeWalker` cached; STA apartment.
- Perceptual hash on 4×4-downsampled raw BGRA, not full PNG (Blake3 over tiny buffer).
- OCR via `tokio::task::spawn_blocking` (never block the async runtime).
- **Two-tier dispatch:** `Instant` (cache/UIA/OCR/YOLO, no cloud, no thought) vs
  `Deliberate` (cloud VLM + thought chain). Default Instant; escalate on miss/low-confidence.

### Leaner API / DX

Collapse **47 tools → ~12 composable verbs** with polymorphic targets and structured
results. Legacy tool names kept as **hidden aliases** for back-compat (not advertised in
`tools/list`).

| New verb | Subsumes |
|----------|----------|
| `ghost_see(window?, mode=fast\|full\|delta)` | describe_screen, describe_screen_fast, describe_screen_delta, get_text |
| `ghost_find(target)` | find, find_text_local, locate_by_description |
| `ghost_act(target, action=click\|type\|...)` | click, click_at, click_by_description, click_text_local, double_click, right_click, type, type_by_description, fill_form, hover |
| `ghost_key(keys)` | press, hotkey, key_down, key_up |
| `ghost_scroll`, `ghost_drag` | scroll, drag |
| `ghost_wait(for=idle\|text\|event\|cond)` | wait, wait_for_idle, wait_until, wait_for_event, navigate_and_wait, click_and_wait_for_text |
| `ghost_query(schema)` | structured data extraction (new) |
| `ghost_assert(predicate)` | new |
| `ghost_run(script)` | YAML declarative flow (new); execute_intent rebuilt on top |
| `ghost_screenshot(region?, max_dim=768, fmt=jpeg, q=75)` | screenshot, screenshot_region — **budget-capped by default** |
| `ghost_window(op)` | list_windows, focus_window, window_state, launch, focus_window_verified |
| `ghost_clipboard`, `ghost_http_*`, `ghost_reset`, `ghost_stop` | unchanged utility set |

Every tool returns `{ ok, evidence?, foreground: {hwnd,title}, error_code?, events? }`.
Fix JSON-RPC: include integer `code` on every error. Declare `required_env` in schema for
vision tools. Emit **streaming progress events** (ActionStarted/ScreenshotCaptured/
VlmThinking/ActionExecuted) as the agent loop runs.

## Phase plan

- **P0 — Unbreak (make it correct).** Wire vision env into the direct `ghost` registration;
  focus-safe input (verify-retry + UIA SetFocus/SetValue); screenshot budget cap; fix
  JSON-RPC `code`, COM apartment (MTA→STA), role constants, non-blocking OCR; commit +
  rebuild v0.5.0 so the live binary matches source; add atomic `ghost_act` (find→focus→
  act→verify).
- **P1 — Reliability + perf core.** Act-then-verify on all mutating tools; DXGI duplication
  cached; UIA walk scoped + walker cached; pHash on raw downsampled pixels; resurrect
  ghost-cache (validated locator cache wired into find/act; `cache_seq` advancing →
  `wait_until` fixed); event bus subscribes to `EVENT_OBJECT_FOCUS` + `EVENT_OBJECT_STATECHANGE`.
- **P2 — Hybrid grounding + intelligence.** New `ghost-ground` crate implementing the
  5-tier cascade with unified confidence + 0–1000 coord contract + typed action parser;
  OmniParser-YOLO ONNX icon detector + set-of-marks fallback; reflection ring buffer;
  two-tier Instant/Deliberate dispatch.
- **P3 — Lean API + streaming + benchmarks.** Collapse 47→~12 verbs (legacy aliases hidden);
  streaming progress events; CLI parity + YAML `ghost_run`; `ghost_query` structured
  extraction; local ScreenSpot-style accuracy benchmark harness to **prove** "most accurate"
  + criterion latency budgets to prove "fastest."

## Testing strategy

- Unit tests per fix (regression guards: focus verify-retry, role constant mapping,
  JSON-RPC `code`, screenshot size cap, pHash on raw pixels).
- Integration suites against live Notepad + a browser (existing harness extended).
- Grounding cascade: golden-set of (screenshot, target, expected rect) for ScreenSpot-style
  accuracy %; per-tier hit-rate + latency telemetry.
- Failure-injection (existing `chaos` feature) extended to focus-steal + device-lost.
- `tools/call` contract test: transport never dies on tool error; every error has `code`.

## Out of scope (this overhaul)

- Local large VLM grounder (UI-TARS-7B / OS-Atlas) — deferred; cascade tiers 1–3 + 5 +
  YOLO tier 4 already beat pure-vision rivals on the common case.
- Cross-platform (macOS/Linux) — Windows-only remains.
- Model fine-tuning / trajectory collection.
