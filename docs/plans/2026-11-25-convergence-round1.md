# Ghost convergence round 1 - 2026-11-26

HEAD `8679c06` (stages 1-5 of `2026-11-25-ghost-improvements.md` applied),
re-measured with the Stage 1 matrix. Raw output: `docs/benches/2026-11-25-final/`.
All binaries from `target-improve` (plus one clean build in `target-final` for
timing); the live `target/release` and `~/.local/bin` were not touched.

**Contamination, and what was discarded.** Between about 00:31 and 00:33 the
reviewer ran its own verify and latency probes at the same time as this matrix,
with testbeds of the same title. Every verify, latency-probe and tab-screenshot
result from that window was deleted and re-run afterwards; only the re-runs are
reported here. Build, tests, clippy and doctor do not drive windows and stand.

## Baseline vs now

| Measurement | Baseline (5bb4cf2) | Now (8679c06) |
|---|---|---|
| Release build, 214 crates | 72 s, 0 warnings | 72 s (clean, fresh target dir), 0 warnings |
| `cargo test --workspace` | 550 passed / 0 failed / 27 ignored | **580 / 0 / 27**, identical on 2 runs |
| `cargo clippy --workspace --all-targets` | 0 warnings | 0 warnings |
| `ghost doctor` | 7/7 PASS | 7/7 PASS |
| `ghost verify` | 2/4 runs all-pass (2 false FAILs) | **3/3** here; 10/10 in stage 4, 3/3 in stage 5 |
| verify: tab screenshot claim | 1705-1795 ms | **40-66 ms** |
| verify: whole run | 8.0 s | 4.7-4.8 s |
| Cold start, spawn -> initialize (median) | 18.7 ms | 19.0 ms |
| `tools/list` | 54 tools, 32,094 bytes | unchanged |
| see text, user / hidden (median) | 51.6 / 16.4 ms | 34.0 / 15.4 ms |
| see tree, user / hidden | 47.0 / 22.0 ms | 35.0 / 15.7 ms |
| find, user / hidden | 19.7 / 7.9 ms | 12.8 / 5.2 ms |
| act click, user: median, verified on real clicks | 123.4 ms, **0/20** | **43.2 ms, 20/20** |
| act click, hidden: median, verified | 113.4 ms, 10/20 | **41.9 ms, 20/20** |
| act click whose effect is a distant label, user / hidden | not measurable | 20/20 / 20/20 verified (163.6 / 147.4 ms) |
| act click on a disabled button, user | `ok:true` 20/20 | **refused 20/20** (same error as hidden) |
| no-effect clicks reported verified=true (label + caret-label) | - | user 0/40, hidden **1/40** (see C5) |
| act type, user / hidden | 7.8 / 7.1 ms, 20/20 | 5.8 / 5.3 ms, 20/20 |
| tab screenshot, owned headless (median / first) | 1007 / 1807 ms | **31.6 / 34.9 ms**, 31.2 / 62.8 ms; page centre 20/20 |
| Criterion microbenches | all < 4 ms | not re-run: no code on those paths changed (convert, grounding, intent) and the run takes 4 min |

## Ranked candidates

Each has evidence (a measurement or a file:line with a concrete failure). Ranked
by harm, then by how sure the evidence is.

### C1 - `ghost_click_at` without an hwnd delivers the click to whatever window covers the point (safety)

**Evidence, and what it did during this review.** The round-1 review probe called
`ghost_click_at` 10 times on the centre of a testbed button, on the user's desktop,
without `hwnd`. The testbed had been shown without activation, so other windows
covered it. All 10 clicks missed the testbed (title counter +0, 10/10), and
`WindowFromPoint` at that point, (288, 238), returns the terminal hosting this
session: top-level "ghost-improve", class `CASCADIA_HOSTING_WINDOW_CLASS`, pid 6404
(`docs/benches/2026-11-25-final/e1-covering-window.txt`). So **10 posted left
clicks went into that terminal window**, a window this session did not create.
No other window was involved. The probe case has been removed from
`review_probes.py` and replaced by an assert that a testbed hwnd is present.

**Cause.** `click_at_background` (`crates/ghost-session/src/session.rs:988-991`):
with no `hwnd_override` the target is `WindowFromPoint(x, y)`, the window on top
at that point, which is any window the human or another agent has in front of the
intended one. Under the default background policy the click is posted to that
window with no check that it is the anchored or intended target. The CRYSTAL
already lists "occluded-window coordinate clicks hit the covering window" as a
gotcha; the tool still does it.

**Change (proposed).** Under the background policy, refuse a coordinate click
whose `WindowFromPoint` top-level window is not the anchored/target window (or a
window Ghost launched), unless `hwnd` is given; say which window covers the point.

**Payoff.** Removes a path by which an agent's input reaches a window the human
is using. Testable without touching anyone's window: cover a testbed with a
second testbed this script launched and require a refusal naming the cover, n=20.

### C2 - `ghost_key` and edit commands go to a different window than the one they report (safety, accuracy)

**Evidence.** Two testbeds launched by the probe, "Ghost Key Bed" and
"Ghost Key Bed 2", the second shown last (first in the window list). With the
anchor on "Ghost Key Bed" and its edit focused, `ghost_key keys=q window="Ghost Key Bed"`
answered `ok` with `target.title = "Ghost Key Bed"` while the key went to
**"Ghost Key Bed 2"** (response `window` field), 3/3 runs
(`final/review-run{2,3,4}/review-e2-decoy-on-top.json`). With the target on top
it went to the right window, 1/1 (`review-run1/review-e2.json`).

**Cause.** `key_background` and `edit_command_background`
(`crates/ghost-session/src/session.rs`, `windows_list.iter().find(|w| w.name.to_lowercase().contains(..))`)
re-resolve the window by first substring match in z-order. The MCP layer passes
only the title (`crates/ghost-mcp/src/main.rs:2656, 2665, 2679`), not the resolved
handle it already passes to `act_background` (`_target_hwnd`, main.rs:2307).
README promises exact title beats prefix beats substring.

**Change (proposed).** Pass `_target_hwnd` to both, resolve by handle first,
exactly as `act_background` does.

**Payoff.** Keystrokes and paste/undo land in the window the response names. Any
two windows where one title contains the other trigger it (an editor with
"file.txt" and "file.txt - copy", two browser windows on related pages).
Acceptance: the decoy-on-top case, key in target 20/20 and in decoy 0/20.

### C3 - background `double_click` reports verified=false for double-clicks that worked (accuracy)

**Evidence.** User desktop, Increment button, n=10: the title counter rose by 2
every time (10/10 correct), `verified` was true 1/10
(`final/review-run1/review.json`, `e3_double_click`).

**Cause.** The `double_click` branch of `act_background`
(`crates/ghost-session/src/session.rs`, after the click branch) still sleeps a
fixed 80 ms and compares only the control's own rect: the defect stage 2 fixed
for `click` (0/20 -> 20/20), left on this action.

**Change.** Route `double_click` through `click_verify` (title, UIA state,
whole-window pixels with persistence).

**Payoff.** Same as stage 2 for this action. Acceptance: real double-clicks
verified 20/20 on both surfaces, no-effect double-clicks false >= 19/20.

### C4 - `ghost_click_at` with an hwnd: the posted click never reaches a child control, every call falls back to UIA Invoke (latency, accuracy, focus risk)

**Evidence.** Pinned to the testbed's own hwnd, n=10: every call took the
fallback ("posted messages had no effect; clicked via UIA Invoke ... the window
may have activated"), the click landed exactly once each time (no double fire),
`verified` true 6/10, latency 337-360 ms (one call 223 ms) against 43 ms for
`ghost_act` click on the same button (`review.json`, `e1_click_at`).

**Cause.** `BackgroundClicker::click_screen(hwnd, x, y)` posts the mouse messages
to the top-level `hwnd`; a child control (the button) is a separate window and
never sees them. The fallback then walks up to 200 elements, invokes, waits
60 ms and compares whole-window pixels once (session.rs:1005-1066).

**Change (proposed).** Post to the deepest child window under the point within
the pinned window (`ChildWindowFromPointEx` / `RealChildWindowFromPoint`) before
any fallback, and verify with `click_verify`.

**Payoff.** ~300 ms per coordinate click, an honest `verified`, and no UIA Invoke
(which can activate the window) for plain Win32 controls. Acceptance: pinned
click_at on the testbed button lands 20/20 without the fallback note, median
<= 60 ms, verified 20/20.

### C5 - hidden-desktop no-effect clicks still read verified=true about 0.7% of the time (accuracy)

**Evidence.** Across every clean hidden-desktop negative-control run since the
20 ms confirming capture went in (stage 2 final x2, stage 5, this round):
1 false `verified=true` in 140 no-effect clicks, always `verified_by: pixels`;
0 in 140 on the user desktop. This round: `final/probe-hidden-label` row 13.

**Cause (hypothesis, measured noise).** Idle hidden-desktop windows show
single-frame repaint blips (`stage-2/idle-noise-hidden*.txt`); one occasionally
spans both captures 20 ms apart.

**Change (to evaluate, not decided).** Learn a noise mask from two pre-click
captures, or require a third agreeing capture only when pixels are the sole
signal; either must keep distant-effect clicks at 20/20.

**Payoff.** Small: about 1 in 140 no-effect clicks on a hidden desktop. Needs
n >= 200 per arm to show a difference; rank it below C1-C4.

### C6 - `ghost verify` cannot see a Ghost-raised window the human then leaves (reliability of the proof)

**Evidence (code).** `crates/ghost-mcp/src/audit.rs:338-345`: a foreground change
with real input in the last 1.5 s increments `human_changes` and records nothing
else; `snapshot()` (audit.rs:428-447) exposes only that count. Stage 5(a) closed
the case where the Ghost window is still in front at the end; if Ghost raises a
window while the human types and the human then clicks away, no incident and no
final state shows it.

**Change (proposed).** Record human-classed changes whose target is a protected
(Ghost-driven) window as their own incident kind, and have `foreground_verdict`
fail on them.

**Payoff.** Closes the last known hole in verify's headline claim. Testable with
the existing unit-test shape plus an audit unit test.

### C7 - hidden-desktop `click_at` and `double_click` still use a fixed 80 ms and a single whole-window diff (accuracy)

**Evidence (code + measured noise class).** `crates/ghost-session/src/hidden.rs:601`
(double_click) and `:703` (click_at) sleep 80 ms, then one `pixels_changed`
whole-window comparison: no title/UIA signal, no persistence. The same noise that
produced stage 2's hidden false positives applies. Not measured on these two
actions yet; the measurement is cheap and safe (hidden desktop).

**Payoff.** Consistency with `click`; size unknown until measured. Measure first.

## Considered, not ranked

- **`tools/list` is 32 KB (54 tools).** Unchanged; a product decision (removing
  advertised tools breaks clients), no measured agent failure.
- **`bench/run_bench.py` and `bench/soak.py` cannot run on a shared desktop**
  (they raise Calculator with the focus lock off). An ergonomics gap for whoever
  re-measures; worth porting to the testbed on a hidden desktop, but no defect in
  Ghost itself.
- **Linux/macOS edits from stage 5(b) are not compiled locally**: `ring` needs a
  C cross-compiler here (`stage-5/b-check-*.txt`). CI's Linux job is the check.
- **README soak figure** (p50 85 ms vs `soak.md` 148.9 ms): docs drift, out of
  scope.

## Rule adopted for every later probe

No probe may deliver input to a window it did not create. Coordinate and
occlusion cases run on the hidden desktop, or with the testbed covered only by a
second testbed the same script launched, and the script checks the target hwnd
before every click.
