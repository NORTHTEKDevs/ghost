# Ghost v0.3.0 — Speed Overhaul Design

**Date:** 2026-04-18
**Author:** Kristian Baer (with Claude)
**Status:** Design approved, ready for implementation planning
**Bundle:** C (full — all core speedups + all five engineering-level additions)
**Non-goal:** Predictive / speculative execution. Ghost never guesses what the agent will do next.

## 1. Motivation

Measured this session: a single Kimi research flow (open tab, type URL, wait, click input, type prompt, press Enter) took ~30 seconds wall-clock. Breakdown:

- ~70% LLM round-trip latency (one Claude turn per primitive).
- ~20% Windows UIA tree walks (Edge accessibility tree is ~75 KB; every `describe_screen`/`find` re-walks it).
- ~10% defensive `wait N ms` guesses because there is no page-idle signal.

Ghost's v0.2 thesis ("SendInput + Vision, no CDP, unblockable") stays intact. The slowness is not fundamental to that thesis — it is an implementation-level tax. v0.3.0 removes that tax without introducing CDP, without predicting future actions, and without making Ghost detectable.

## 2. Scope

Fourteen independent improvements, all non-predictive, all additive:

### Core speedups (Bundle A)
1. UIA event-driven cache (`ghost-cache::UiaCache`).
2. Delta-only `describe_screen` (`ghost_describe_screen_delta`).
3. `wait_until` + `wait_for_idle` primitives.
4. Macro tools: `ghost_navigate_and_wait`, `ghost_click_and_wait_for_text`, `ghost_fill_form`.

### Intent compiler (Bundle B)
5. `ghost_execute_intent` — JSON step list + JSONLogic `abort_if`/`retry_if`, executed by a non-LLM Rust FSM (`ghost-intent::FsmExecutor`). One Claude turn → N OS ops.

### Non-predictive novelties (Bundle C add-ons)
6. PostMessage-based background clicks (`ghost_click_background`).
7. Semantic locator cache persisted to disk (SQLite, AX-checksum verified).
8. Parallel STA-threaded UIA query pool (4 pre-warmed workers).
9. GPU framebuffer diff via DXGI Desktop Duplication for page-idle detection.

### Engineering-level speedups (the five adds)
10. `IUIAutomationCacheRequest` + batched property fetch across subtree (one COM IPC instead of N).
11. Server-side tree search via `FindAllBuildCache` + compound `IUIAutomationCondition` (runs inside target app's UIA provider).
12. COM apartment pool pre-warmed at startup (removes ~30 ms cold init per worker).
13. DXGI Desktop Duplication context held open for session lifetime (saves ~50 ms per idle check).
14. `sonic-rs` serialization for MCP responses >4 KB (3-5× on the big `describe_screen` payload).

### Out of scope
- Predictive / speculative execution (explicitly rejected).
- CDP, browser extensions, or in-renderer code injection (breaks the undetectable thesis).
- Kernel driver / HID filter input injection (too heavy, no real win over SendInput for non-gaming).

## 3. Architecture

Two new internal Rust crates, one new subsystem inside `ghost-core`:

- **`ghost-cache`** (new) — event-driven UIA mirror (`UiaCache`) + SQLite-backed semantic locator store (`LocatorStore`).
- **`ghost-intent`** (new) — JSON step compiler, JSONLogic condition evaluator, FSM executor.
- **`ghost-core::idle`** (new module) — DXGI framebuffer diff driving the page-idle signal.
- **`ghost-core::uia::sta_pool`** (new module) — 4 pre-warmed STA workers, each owning its own `IUIAutomation` instance.
- **`ghost-core::uia::cached_walker`** (new module) — wraps `IUIAutomationCacheRequest` + `FindAllBuildCache` to replace our recursive `TreeWalker` usage.
- **`ghost-core::input::postmessage`** (new module) — `BackgroundClicker` using `PostMessageW`.

`ghost-session` gains ~9 async methods wired to the 10 new MCP tools. Existing 24 tools untouched (decision: additive, no deprecation in v0.3.0).

### Dependencies added
`sonic-rs`, `rusqlite` (bundled), `crossbeam-channel`, `blake3`.

## 4. Public tool surface (10 new MCP tools, additive)

| Tool | Purpose | Key params |
| --- | --- | --- |
| `ghost_wait_until` | Block until JSONLogic condition is true | `{condition, timeout_ms, poll_ms?}` |
| `ghost_wait_for_idle` | Block until screen framebuffer is stable | `{window?, stable_frames?=3, timeout_ms}` |
| `ghost_navigate_and_wait` | Browser URL + page-loaded in one call | `{url, wait_for?: {text\|element\|idle}, timeout_ms}` |
| `ghost_click_and_wait_for_text` | Click + wait for text appear/disappear | `{target, wait_for_text, appears: bool, timeout_ms}` |
| `ghost_fill_form` | Multi-field form fill + optional submit | `{fields: [{target, text}], submit?: {target}}` |
| `ghost_execute_intent` | Run compiled step list, non-LLM FSM | `{steps, abort_if?, retry_if?, max_duration_ms}` |
| `ghost_describe_screen_delta` | Only elements changed since last call | `{window?, since_seq?}` → `{seq, added[], removed[], updated[]}` |
| `ghost_click_background` | PostMessage click, no focus steal | `{hwnd\|window, target}` |
| `ghost_cache_stats` | Observability | `{}` → hit/miss counts, mirror size, locator rows |
| `ghost_cache_invalidate` | Maintenance | `{app?, window?, all?: bool}` |

## 5. Data flows

### 5.1 `describe_screen_delta`
1. `UiaCache::snapshot(window, since_seq)`.
2. If `seq` unchanged: return `{added:[], removed:[], updated:[], seq}`, <1 ms, <200 bytes.
3. Otherwise walk the mirror, diff against `since_seq` snapshot.
4. `sonic-rs` serialize → MCP response.

Cold first call populates the mirror via `CachedTreeWalker::find_all_build_cache` (one COM IPC, fully hydrated subtree).

### 5.2 `execute_intent`
1. `IntentCompiler::compile(steps)` validates ops, resolves targets via `LocatorStore`, returns `CompiledIntent`.
2. `FsmExecutor::run` iterates ops. For each:
   - Dispatch to `StaPool` (UIA ops) or direct input (mouse/keyboard ops).
   - Await result.
   - Evaluate `abort_if(state)` → short-circuit with `{status: aborted, reason}`.
   - Evaluate `retry_if(state)` → retry with backoff up to N.
3. Single MCP response: `{executed, ops_results, duration_ms, status}`.

Partial failures always return `{status, completed_ops, failed_op, partial_state}` — never mark partial as success.

### 5.3 Cache invalidation (event-driven, zero polling)
UIA `StructureChangedEvent` / `PropertyChangedEvent` → `StaPool` worker callback → `UiaCache::apply_mutation` → updates subtree, invalidates `LocatorStore` entries whose `ax_checksum` now differs, bumps `seq`.

### 5.4 Semantic locator lookup (the "second visit is instant" path)
1. `LocatorStore::lookup(app_id, window_class, title_pattern, role, name)`.
2. Hit: verify `ax_checksum` against live `UiaCache` at stored rect. Match → return rect (~1 ms). Drift → evict, fall through.
3. Miss: `CachedTreeWalker::find_all_build_cache(condition)` — one COM IPC, hydrated subtree, pick match, upsert to store, return rect.

### 5.5 Page-idle detection
`IdleDetector` holds persistent `IDXGIOutputDuplication`. `wait_stable(stable_frames, timeout)`:
- Loop: `AcquireNextFrame` (blocking, 16 ms max) → downsample to 4×4 grid → blake3 hash.
- If hash == last_hash: `stable_count += 1`. Else reset.
- Return `Ok` when `stable_count >= stable_frames`, or `Err(Timeout)`.

Replaces every blind `wait 3000ms` with "wait until the screen actually stopped changing, or time out."

## 6. Error handling

**Principle:** Every component has an explicit degrade path. Never silent-fail, never crash the MCP server. Errors surface as typed JSON with a `recovery_hint` field.

| Component | Failure | Strategy |
| --- | --- | --- |
| `UiaCache` event handler | COM disconnect | Catch `RPC_E_*`, unsubscribe, clear affected subtree, re-subscribe on next query. Tools fall back to direct walks until rebuild. |
| `StaPool` worker panic | Bad element / OOM / COM exception | `catch_unwind` per job, worker restarts, panicked job returns typed error. Circuit breaker after 3 restarts in 60 s → single-threaded fallback with warning. |
| `LocatorStore` drift | `ax_checksum` mismatch | Evict, fall through to `CachedTreeWalker`. Bump `cache_drift_total`. Never silently use stale coords. |
| `LocatorStore` SQLite lock/corrupt | WAL busy or DB invalid | Retry 3× (10 ms backoff). Persistent fail → disable persistence for session, keep in-memory fallback, log, continue. |
| `FsmExecutor` op fail mid-intent | Target vanished / wait timeout | Check `retry_if` first, then `abort_if`. Otherwise halt with `{status: failed, completed_ops, failed_op, partial_state}`. |
| `abort_if`/`retry_if` eval error | Bad JSONLogic | Reject at compile time. Runtime eval errors treated as `false` (fail-open on condition, fail-closed on step). |
| `BackgroundClicker` HWND gone | Window closed between resolve and click | `IsWindow(hwnd)` gate, return `{error: window_gone, recovery_hint}`. |
| `IdleDetector` surface lost | Topology change / monitor off | Catch `DXGI_ERROR_ACCESS_LOST`, re-init duplication. Persistent fail → `{error: idle_detection_unavailable, recovery_hint}`. |
| `IdleDetector` timeout | Animated page | Return `{status: timeout_but_continuing, best_stable_frames}` — caller decides. |
| `CachedTreeWalker` oversized subtree | >50 K elements | Fall back to depth-limited incremental walks. Track `large_tree_fallback_total`. |
| `sonic-rs` serialization fail | Non-UTF-8 element name | Catch, fall back to `serde_json` for that one response. |

**Invariants:**
1. No stale click — every `LocatorStore` hit is checksum-verified before the input primitive fires.
2. No infinite wait — every `wait_*` has `timeout_ms`; FSM enforces `max_duration_ms`.
3. No silent cache poisoning — drift always evicts.
4. No MCP deadlock — StaPool jobs have a 30 s per-job timeout; hung workers are killed and replaced.
5. No lost telemetry — every failure path increments a counter exposed via `ghost_cache_stats`.

**Observability:** `tracing` spans on every new primitive with `op`, `target`, `duration_ms`, `cache_hit`, `fallback_reason`.

## 7. Testing strategy

**Layer 1 — Unit tests.** Mirror correctness, seq monotonicity, delta minimality, `LocatorStore` upsert/evict/drift, `IntentCompiler` rejects bad JSONLogic at compile time, `StaPool` panic recovery + circuit breaker, `CachedTreeWalker` parity with manual walk, `IdleDetector` hash stability, `BackgroundClicker` `IsWindow` gate.

**Layer 2 — Integration against Notepad.** `describe_screen_delta` mutation coverage; 3-5 op `execute_intent`; `wait_until` / `wait_for_idle` on an animated menu; `click_and_wait_for_text`; `fill_form` on Find+Replace; `ghost_click_background` with foreground-unchanged assert; full locator cache lifecycle (cold / warm / drift / cross-process-restart).

**Layer 3 — Browser integration.** Local `file://` fixture with known form + async content. Run against **Edge**, **Comet**, **Chrome**. `navigate_and_wait` → body-text assert. Scripted login via `execute_intent`. `describe_screen_delta` stress test on 75 KB AX tree → delta payload < 5 KB on small DOM change.

**Layer 4 — Performance regression (CI-gated, Criterion.rs, hard budgets).**

| Operation | Baseline | Budget |
| --- | --- | --- |
| `describe_screen` full, Edge | ~300 ms | <50 ms |
| `describe_screen_delta` no-change | n/a | <5 ms |
| `find_by_name` cold | ~100 ms | <20 ms |
| `find_by_name` warm (locator hit) | ~100 ms | <2 ms |
| `wait_for_idle` stable page | 3000 ms blind | <100 ms |
| `execute_intent` 5-op Notepad | ~30 s (15 LLM turns) | <400 ms Ghost-side |
| MCP serialize 75 KB payload | ~12 ms | <3 ms |

CI fails any benchmark regressing >20 % vs `cargo bench --save-baseline main`.

**Layer 5 — Failure injection (behind `ghost-chaos` feature flag).** Force COM disconnect; lock SQLite file; kill `StaPool` worker mid-submit; simulate `DXGI_ERROR_ACCESS_LOST`; randomize always-failing op with `retry_if=true`; corrupt `LocatorStore` on disk.

**Manual validation before v0.3.0 tag:**
- Drive kimi.com full flow via single `execute_intent`; target >10× wall-clock reduction vs. per-primitive script.
- `ghost_screenshot` regression test (unchanged path, needed for CAPTCHA-image workflows).
- `ghost_cache_stats` shows non-zero hits after 5-minute browser session.

**Coverage targets:** 80 %+ on `ghost-cache` and `ghost-intent`; 60 %+ on new `ghost-core` modules. Every new MCP tool has ≥1 happy-path and ≥1 failure-path integration test.

## 8. Expected results

After v0.3.0 ships, the kimi.com session that took ~30 seconds today should complete in roughly 2-4 seconds end-to-end, with similar gains across any multi-step browser or app flow. Speedup sources:

- `CachedTreeWalker` + `FindAllBuildCache` + `CacheRequest`: 10-20× on tree walks.
- COM apartment pool pre-warming: removes cold-start on every parallel query.
- Event-driven cache + delta `describe_screen`: 20-50× on query-heavy flows.
- Locator store with checksum verify: second-visit UIs resolve targets in ~1 ms.
- `IdleDetector` DXGI held-open: blind 3 s waits collapse to ~100 ms.
- `execute_intent` FSM: one Claude turn drives N OS ops — kills the 70 % LLM-in-the-loop tax.
- `sonic-rs` on large payloads: 3-5× serialization.
- PostMessage background clicks: no foreground steal + no input-queue latency for background windows.

## 9. Next step

Invoke the `writing-plans` skill to produce a step-by-step implementation plan (per-crate ordering, PR-sized commits, test-first schedule).
