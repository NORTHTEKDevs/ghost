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
