# Stage 8 - error kinds, C6, C7 / delta measurement, C5 / C8 measurement

Probe-safety rule as in stage 7: probes send input only to windows they create;
the cover probe checks `WindowFromPoint` before every click.

## 1. Refusals raised with the right error kind

The click_at cover refusal was `GhostError::Vision`, so agents read
"Vision error: ...". It is now `ElementNotInteractable` ("Element not
interactable: the point (x,y) in '<target>' - it is covered by '<cover>' (pid
N) ..."), the kind and code (-32002) a disabled element already gets, whose hint
says "disabled or covered". The same nit applied to errors added in stages 6-7,
all fixed here: a resolved window that has closed (`pick_window`, `read_text`,
`describe_window`) is `ProcessNotFound`, like C11's closed anchor; a resolved
window that is minimised is `ElementNotInteractable`. The pre-existing "no
visible window matching" message keeps its original kind.

| Criterion | Target | Measured |
|---|---|---|
| Covered click_at refusal starts "Element not interactable" (`err-cover`) | 20/20 | 20/20; uncovered and popup unchanged 20/20 |
| Unit: cover refusal kind and text; closed -> ProcessNotFound; minimised -> ElementNotInteractable | as stated | 2/2 (red at compile before) |
| `cargo test --workspace` | 0 failed | 606 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | 0 / 0 findings |

## 2. C6 - a Ghost window taking the foreground while the human types is recorded, and verify fails on it

**Gap.** The interference audit recorded only SYNTHETIC foreground changes (no
real input in 1.5 s). `GetLastInputInfo` counts the human's typing, so a window
Ghost drives that took the foreground while the human typed was classed as the
human's change: only a count was kept, not where the foreground went, and
`ghost verify` could not see it unless the Ghost window was still in front at
the end (stage 5a).

**Change.**
- `crates/ghost-mcp/src/audit.rs`: a pure `taken_during_input(to_is_ghosts,
  chose)`: a human-classed change whose target is a window Ghost drives, with
  the real-input hooks reporting no click or Alt+Tab choosing it
  (`realinput::human_chose_a_window`), is recorded in its own list
  (`during_input`, capped like `incidents`). A deliberate human switch to that
  window (click / Alt+Tab) is not recorded; without the hooks nothing is
  claimed. `ghost_stats.interference_audit` gains
  `foreground_taken_during_input` and `during_input_incidents`; the existing
  synthetic counts are unchanged.
- `crates/ghost-cli/src/verify.rs`: `incidents_since` also reads
  `during_input_incidents`; `foreground_verdict` fails on any of them, even when
  the foreground ended where it started.

| Criterion | Target | Measured |
|---|---|---|
| Unit (audit): recorded only for a Ghost window with no click/Alt+Tab; not with hooks off; not for other windows | as stated | pass (red on a stub) |
| Unit (audit): observe records it while the human types and exposes it in the snapshot; a deliberate switch is not recorded | as stated | pass (red on a stub) |
| Unit (verify): such an incident FAILS even when the foreground ended where it started; it is read from the audit | as stated | pass (verdict red on a stub) |
| `ghost_stats` exposes the new fields (`c6-stats.txt`) | present | `foreground_taken_during_input: 0`, `during_input_incidents` present |
| `ghost verify` x3 (`c6-verify-run*.txt`) | 3/3 pass | 3/3 pass |
| `cargo test --workspace` | 0 failed | run 1: **ghost-core lib binary exited abnormally** (87 passed when it stopped); runs 2 and 3: 610 / 0 / 27 |
| clippy, vibecode lint | 0 / 0 HIGH | 0 / 0 findings |

**The run-1 abnormal exit.** `ghost_core` lib tests stopped with exit code 1 and
"test exited abnormally", no test reporting a failure (`c6-test-run1.txt`).
Nothing in this change touches ghost-core. Re-run 5 times on its own: 153/0/4
each (`c6-core-flake-run*.txt`). Over the session this binary ran about 37 times
with this one abnormal exit. Recorded as a flake candidate: finding the test
needs a `--no-capture` loop.

Not live-reproducible: making a Ghost-driven window take the foreground while a
human types would need synthetic typing into the human's session, which the
probe-safety rule excludes. The unit tests carry the proof.

## 3a. C7 - hidden-desktop click_at did not click at all

**Measured** (`c7-measure-run*`, hidden desktop, testbed launched there with
`ghost_window op=launch`): `ghost_click_at` on the Increment button and on the
Note button changed NOTHING - correct 0/20 and 0/20 in both runs - and reported
verified=false 19-20/20 (one pixel-noise true per run). The static-label control
read false 20/20. Hidden `double_click` was already fixed in stage 6 (C3) and
re-measured 20/20 / 20/20 here.

**Cause.** `hidden_click_at` posted the click to the top-level window, which a
child control never sees - the same defect C4 fixed on the user desktop - and
then took one whole-window diff after a fixed 80 ms.

**Change** (`crates/ghost-session/src/hidden.rs`). Post to the deepest visible
child under the point (`session::deepest_child_at`, now `pub(crate)`); verify
with the window's and its owner's titles every poll and whole-window persisting
pixels at 80 ms (`session::titles_of`); `verified_by` and a note when nothing
changed. The last user of the old single-diff `pixels_changed` is gone, so it
and an unused import are removed.

| Criterion | Target | Before (`c7-measure-run*`) | After run 1 / run 2 (`c7-after-run*`) |
|---|---|---|---|
| click_at on Increment lands (+1) | 20/20 | **0/20 / 0/20** | 20/20 / 20/20 |
| ... verified | 20/20 | 1/20 / 1/20 | 20/20 / 20/20 (`title`), median 22.5 / 21.8 ms |
| click_at on Note (effect elsewhere) lands and verifies | 20/20 | 0/20 / 0/20 | 20/20 / 20/20 (`pixels`) |
| click_at on the static label: verified=false | >= 19/20 | 20/20 | 20/20 / 20/20 |
| double_click on Increment (+2, verified) | 20/20 | 20/20 | 20/20 / 20/20 |
| `cargo test --workspace` | 0 failed | 610 / 0 / 27 | 610 / 0 / 27, both runs |
| clippy, build warnings, vibecode lint | 0 / 0 / 0 HIGH | - | 0 / 0 / 0 findings |

No new unit test: the decision is `click_verify::decide` and the hit test is C4's
`deepest_child_at`; the probe (before 0/20) is the test.
