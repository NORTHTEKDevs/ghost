# Stage 9 - ghost_see mode=delta does what the docs say

Probe-safety rule as before: the probe's own testbeds are the only windows that
get input.

**Defect** (stage 8, `docs/benches/2026-11-26-stage-8/delta-*`): `mode=delta`,
advertised as "delta=changed only", diffed a global UIA mirror that nothing in
production filled and ignored its `window`; it answered "nothing changed" after
real changes 10/10.

**Change.**
- `crates/ghost-session/src/see_delta.rs` (new): a pure `diff(old, new)` keyed by
  (role, name, occurrence) -> added / removed / updated (same key, changed rect
  or enabled); a `DeltaStore` with one snapshot per window, at most 16 windows
  (least recently read dropped). First read of a window, or a `since_seq` that is
  not that window's last seq: the whole tree as `added` with `baseline: true`.
  Otherwise the change since the window's last read. Reply keys unchanged:
  `added`, `removed`, `updated`, `seq` (+ `baseline`).
- `GhostSession::see_delta(window, hwnd_hint, since_seq)` walks the resolved
  window through `describe_window` (stage 7, C9b) and keys snapshots by the
  resolved handle (title when there is none). Elements carry the full-mode
  fields and offscreen filter plus `enabled`.
- `crates/ghost-mcp/src/main.rs`: `mode=delta` is window-scoped now (anchor and
  `_target_hwnd` injected) and the handler uses `see_delta`.
- Testbed: a "Count N" button whose own caption counts its clicks - a change the
  interactive tree can see.

| Criterion | Target | Before (stage 8) | After run 1 / run 2 (`delta-after-run*`) |
|---|---|---|---|
| Real change in the target reported, explicit `window=` | 10/10 | 0/10 | 10/10 / 10/10 |
| ... anchored (no `window`) | 10/10 | 0/10 | 10/10 / 10/10 |
| No change -> empty, explicit / anchored | 10/10 | 10/10 (always empty) | 10/10 / 10/10 both |
| Change only in the same-title decoy -> not reported, explicit / anchored | 10/10 | (always empty) | 10/10 / 10/10 both |
| Unit: identical / renamed / moved or disabled / duplicates / baseline / change-only + seq / per-window isolation / stale since_seq / bounded memory | 9 cases | - | 9/9; a mutation that drops added/updated detection fails 3 |
| `cargo test --workspace` | 0 failed | 610 / 0 / 27 | 619 / 0 / 27, both runs |
| clippy, build warnings, vibecode lint | 0 / 0 / 0 HIGH | - | 0 / 0 / 0 findings |

Not changed: `UiaCache::snapshot_delta` and `GhostSession::describe_screen_delta`
remain for their tests and other callers; the MCP tool no longer uses them.
