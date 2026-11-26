# Stage 2 - background click verification

Item 1 of `docs/plans/2026-11-25-ghost-improvements.md`, with the reviewer's
amendments: whole-window pixels (no rect scoping), noise filtered rather than
traded away, a distant-effect positive case, a disabled-button negative control,
`verified` still a bool, `verified_by` additive, and unit tests for the pure
decision logic.

All numbers are from `target-improve` builds; the live `target/release` and
`~/.local/bin` binaries were not touched.

## What changed

- `crates/ghost-session/src/click_verify.rs` (new): the decision (`decide`), the
  poll schedule (`POLL_AT_MS`, deadlines 10/20/40/80 ms from dispatch), per-pixel
  cell diff (`changed_cells`), the persistence filter (`persisted`,
  `PERSIST_GAP_MS`), and the capture wrappers `pixel_effect` /
  `pixel_effect_async`. 16 unit tests.
- `session.rs` (user desktop) and `hidden.rs` (hidden desktop): the click branch
  polls the window title on every poll; the element's UIA name/value and the
  whole-window pixels once, at 80 ms. First signal that moved wins; the response
  gains `verified_by: "title" | "uia" | "pixels"`.
- `crates/ghost-testbed`: a disabled `Locked` button, and a `Note` button whose
  only effect is rewriting a distant `Status` label.
- `latency_probe.py`: `--control increment|distant|disabled|label|caret-label`.

## How the design got here (each step was measured, raw output in this folder)

1. Polling all three signals every time verified every real click, but made a
   no-effect click 60 ms slower and produced a pixel false positive
   (`after-*` runs, since replaced). Pixels and UIA moved to the last poll only.
2. One remaining false positive was idle repaint noise. `idle-noise-hidden.txt`
   and `idle-noise-hidden-long.txt`: over 950 captures of an untouched window,
   the only differences were single frames (10-25 ms) that reverted. A change
   now has to persist in a second capture.
3. Per-cell averages missed "Status: noted 2" -> "noted 3" (19/20 on the distant
   case). PrintWindow re-renders unchanged content bit-identically, so the diff
   is now per pixel (tolerance 16 per channel), located on a 32x32 grid.
4. No-effect clicks were still 30 ms slower than before. Instrumented timings:
   UIA reads 1-2 ms, pixels 8-13 ms, but four relative sleeps of 10/10/20/40 ms
   took 15/16/32/48 ms (Windows timer tick). Polls are now deadlines from one
   instant.
5. Back-to-back confirming captures still let one long repaint span both
   (1 false positive in 80). The confirming capture is now 20 ms later.

## Acceptance (final build, two runs of n=20 each, `final-run{1,2}-*`)

| Criterion | Target | Baseline (old code) | Measured run 1 / run 2 |
|---|---|---|---|
| Real click verified, user desktop | 20/20, correct 20/20 | 0/20 (`baseline/probe-user-run*`) | 20/20 / 20/20, correct 20/20 |
| Real click verified, hidden desktop | 20/20, correct 20/20 | 10/20 | 20/20 / 20/20, correct 20/20 |
| Distant-label click verified, user | 20/20 | not measurable (no such control) | 20/20 / 20/20 (`pixels`) |
| Distant-label click verified, hidden | 20/20 | not measurable | 20/20 / 20/20 (`pixels`) |
| Disabled button, user: verified=false | >= 19/20 | 20/20 (`before-user-disabled`) | 20/20 / 20/20 |
| Disabled button, hidden: verified=false | >= 19/20 | refused before dispatch, 20/20 | **refused before dispatch (`ElementNotInteractable`), 20/20 / 20/20** - see note |
| Static label (no effect): verified=false | >= 19/20 | user 20/20, hidden 20/20 | user 20/20 / 20/20, hidden 20/20 / 20/20 |
| Label after focusing the edit: verified=false | >= 19/20 | - | user 20/20 / 20/20, hidden 20/20 / 20/20 |
| Real click median / p95, user | <= 50 / <= 100 ms | 123.4 / 143.3 ms | 43.2 / 43.9 ms, 43.3 / 49.3 ms |
| Real click median / p95, hidden | <= 50 / <= 100 ms | 113.4 / 122.8 ms | 35.3 / 47.7 ms, 35.0 / 47.9 ms |
| No-effect click median (label) | no worse than baseline | user 123.2, hidden 117.6 ms | user 123.2 / 131.6, hidden 112.6 / 117.2 ms |
| `cargo test --workspace` | 0 failed | 550 / 0 / 27 | 566 passed / 0 failed / 27 ignored, both runs |
| `cargo clippy --workspace --all-targets` | 0 warnings | 0 | 0 |
| vibecode lint on changed files | 0 HIGH | - | 0 findings |

Across both final runs: 0 false `verified=true` in 240 no-effect clicks, and
160/160 real clicks verified.

**Note on the disabled control on the hidden desktop.** The hidden-desktop path
(and the foreground path, `session.rs` `act_on_element`) refuses a disabled
element before dispatching anything, so the verifier is never reached and there
is no `verified` field to be false. That refusal predates this change and is a
stronger answer than `verified=false`; the static-label and caret-label controls
cover "the verifier says false when nothing happened" on both surfaces. The
user-desktop background path does NOT refuse: it posts `BM_CLICK` to the disabled
control and returns `ok: true, verified: false`. Making it refuse like the other
two paths is a small, separate behaviour change, not made here.

**Cost.** A click whose only effect is in pixels (the distant case) now takes
130-162 ms instead of returning `verified=false` at about 123 ms: the confirming
capture is 20 ms later by design.

## Reproduce

```bash
R=<abs path to target-improve/release>
python docs/benches/2026-11-25-stage-2/latency_probe.py --surface user   --control distant --exe "$R/ghost-mcp.exe" --testbed "$R/ghost-testbed.exe" --n 20 --out <dir>
python docs/benches/2026-11-25-stage-2/latency_probe.py --surface hidden --control label   --exe "$R/ghost-mcp.exe" --testbed "$R/ghost-testbed.exe" --n 20 --out <dir>
```

Raw files: `before-*` (old server, new testbed), `final-run{1,2}-*` (this build),
`test-run{1,2}.txt`, `clippy.txt`, `build-release.txt`, `vibecode-lint.txt`,
`idle-noise-hidden*.txt`.
