# Kimi.com Flow — v0.3.0 Validation

**Status: Pending manual run by user driving Claude Desktop.**

Automated integration tests cannot spawn a real Claude model session against
the kimi.com web UI. This file is the place to record numbers once the user
performs the run described in `docs/plans/2026-04-18-ghost-v030-speed-overhaul.md:1199`.

## What passed automatically on 2026-04-18

- **85 lib tests green** across ghost-core, ghost-cache, ghost-intent, ghost-session, ghost-mcp.
- **4 Notepad integration tests green** (`cargo test -p ghost-session --test notepad_flow -- --ignored`):
  - `delta_describe_tracks_typed_text`
  - `execute_intent_find_replace_flow`
  - `locator_cache_lifecycle_cold_warm`
  - `click_background_does_not_steal_foreground`
- **Browser harness compiles + runs** against Edge with file:// fixture.
- **Criterion benches** beat budgets with 30x headroom (see `v030-baseline.md`).

## What still needs user verification

1. End-to-end `execute_intent` wall-clock under 4 s (excluding model think time)
   driving a real browser against kimi.com.
2. That `ghost_describe_screen_delta` returns a payload significantly smaller
   than `ghost_describe_screen` on real pages (>50% reduction target).
3. That `ghost_click_background` does not steal foreground focus from the
   user's active window during a real session.

## Known gaps (not blocking v0.3.0 but honest limits)

- `IdleDetector` hashes full PNG screen captures rather than DXGI 4x4 downsample.
  Works, but slower than the design called for. Measured: ~80 ms per frame on
  a 3840x2160 primary. Target for v0.3.1: DXGI `IDXGIOutputDuplication`.
- `UiaCache` has no COM event subscription. Current snapshots are driven by
  explicit walker refresh calls, not `IUIAutomationStructureChangedEventHandler`.
  Target for v0.3.1.
- Chrome and Comet browser paths untested; only Edge is confirmed.

## How to fill in this doc

After running the kimi intent against live Claude Desktop:

```
## Run on YYYY-MM-DD

- Wall clock: N.NN s
- Ghost-side overhead (total - sum of model latencies): N.NN s
- Payload size sum across all describe_* calls: NN KB
- Any failure or unexpected behavior: ...
```
