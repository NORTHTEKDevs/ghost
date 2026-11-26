# Stage 7 - C1, C4, C9b

Probe-safety rule: every click goes only to windows the probe created. The
cover probe checks `WindowFromPoint` before EVERY click and does not send it
unless the point belongs to its own target, cover or popup; its testbeds are
created topmost at fixed positions (`--topmost --pos`) so that nothing else can
lie between them, and a `finally` closes them by class and exact title whatever
happens.

## C1 - a coordinate click is not posted to someone else's window lying over the target

**Defect.** `click_at_background` with no `hwnd` posts to `WindowFromPoint(x,y)`,
whatever lies on top at that point. During round 1 that delivered 10 clicks to
the terminal hosting the session. Measured here with the anchor on the target
and the point under a second testbed process (`c1-before`): **39 clicks in 20
calls landed in the cover, 0 in the target.**

**Change.** `click_at_background(x, y, hwnd, target_hint)`; the MCP layer passes
the resolved `_target_hwnd`. With a resolved target and no `hwnd`, the window at
the point (its root) is checked by a pure `cover_allowed`: allowed if it is the
target, a window of the target's process, a window the target owns (owner chain:
menus, dropdowns, tooltips, dialogs), or a process Ghost launched (new
`ghost_browser::launch::job::is_bound_to_this_process`, `IsProcessInJob` on the
server's kill-on-close job). Anything else is refused before anything is sent,
naming the cover's title and pid and the way out (`hwnd=` posts to the target
itself, occlusion-proof). Unchanged: calls with `hwnd`, calls with no resolved
target (the caller addressed the screen), and the foreground-policy path.

Testbed additions (`crates/ghost-testbed`): a "Popup" button that opens an owned,
same-process popup with a "Ping" button (title gains `[popup=N]`), and the
`--label`, `--pos`, `--topmost` options.

| Criterion | Target | Before (`c1-before`) | After run 1 / run 2 |
|---|---|---|---|
| Covered by another process: refused, naming the cover | 20/20 | 0/20 (39 clicks into the cover) | **20/20 / 20/20**, 0 clicks into the cover |
| Same-process owned popup over the target: allowed and lands | 20/20 | 20/20 sent, 39 pings | 20/20 sent, 39 pings (double: C4) |
| Uncovered target: lands | 20/20 | 20/20 sent, 39 clicks (double: C4) | 4 sent per run (6 clicks), then skipped - see below |
| Unit: target / same process / owned / Ghost-launched allowed; other process refused; unknown pid never matches | as stated | 4 of 6 red on a stub | 6/6 green |
| `cargo test --workspace` | 0 failed | 595 / 0 / 27 | 601 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

**What the uncovered and popup rows show, and why they are C4's.** Without
`hwnd`, every allowed coordinate click fires TWICE, before and after this
change: the posted click lands on the child under the point, the 48-px pixel
check around it sees no change, and the UIA Invoke fallback clicks again (39 in
20). That fallback also ACTIVATES the target: it took the foreground from the
human's window, the foreground guard handed it back, and its circuit-breaker
pushed the testbed to `HWND_BOTTOM` (`send_to_back`, which also drops topmost) -
after which the safety check saw the human's terminal at the point and stopped
sending (4 sent, 16 skipped). So these runs briefly took the human's foreground;
no input reached any window the probe did not create. C4 (post to the right
child, verify, never invoke after an observed effect) is the fix and re-measures
both rows.

## C4 - a coordinate click is posted to the control under the point, verified, and never invoked twice

**Defects** (both measured on the old path).
- Pinned (`hwnd` given): the click was posted to the top-level window, which a
  child control never sees; every call fell through to the UIA Invoke fallback,
  337-360 ms, and Invoke activates the window (round 1, `final/review-run1`).
- Not pinned: `WindowFromPoint` reached the child, the posted click landed, the
  48-px pixel patch around it saw no change, and the fallback pressed the
  control a second time - 39 activations in 20 calls (`c1-before`). That
  activation took the human's foreground, and the guard's circuit-breaker then
  pushed the testbed to `HWND_BOTTOM`.

**Change** (`crates/ghost-session/src/session.rs`, `click_at_background`).
- The click is posted to the deepest visible child window under the point
  inside the target's top-level window (`deepest_child_at`, a
  `ChildWindowFromPointEx` walk) - pinned or not.
- Verified like an element click (`poll_point_effect`): the top-level window's
  and its owner's titles on every poll (a popup or dropdown usually acts on its
  owner), whole-window persisting pixels at 80 ms; `verified_by` added.
- UIA Invoke only when there is no child control under the point (windowless
  content) AND nothing changed: a pure `invoke_fallback_due`.
- `timing_ms {prepare, verify, total}` in the response.
- `crates/ghost-mcp/src/main.rs`: the foreground guard's fixed 100 ms settle after
  `ghost_click_at` is skipped when the response is `verified: true` (the target's
  thread has already handled the click - the stage 2 argument for `ghost_act`);
  it was 125 ms of a 157 ms pinned click. Unverified clicks keep the 100 ms;
  `ghost_scroll` and `ghost_key` are unchanged.

| Criterion | Target | Before | After run 1 / run 2 |
|---|---|---|---|
| Pinned click on Increment lands exactly once (`c4-after-run*`) | 20/20 | 20/20 via the fallback (round 1) | **20/20 / 20/20** |
| ... without the UIA Invoke fallback note | 20/20 | 0/20 | 20/20 / 20/20 |
| ... verified | 20/20 | 6/10 (round 1) | 20/20 / 20/20 (`title`) |
| ... median / p95 | <= 60 ms | 337-360 ms | **23.7 / 39.2 ms, 24.0 / 33.4 ms** |
| ... foreground changed during the call | 0 | (fallback activates) | 0/20 / 0/20 |
| Pinned click on the static label (no effect): verified=false (`c4-label-run*`) | >= 19/20 | - | 20/20 / 20/20, no fallback, foreground unchanged |
| Uncovered, not pinned: lands once and verified (`c4-cover-run*`) | 20/20 | 39 activations in 20 calls | 20/20 once, 20/20 verified, both runs |
| Owned popup: lands once and verified | 20/20 | 39 pings in 20 calls, verified 1/20 | 20 pings in 20, verified 20/20, both runs |
| Covered by another process: still refused (C1) | 20/20 | - | 20/20 / 20/20 |
| Unit: fallback only for windowless content with no observed effect | as stated | 2 of 3 red on a stub | 3/3 green |
| `cargo test --workspace` | 0 failed | 601 / 0 / 27 | 604 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

A no-effect `click_at` takes ~230 ms (the full 80 ms poll plus the guard's
100 ms settle, kept because nothing was observed), against ~340 ms before.

## C9b - ghost_see full mode and ghost_snapshot walk the window they report

**Defect.** `describe_screen(window_title)` re-resolves the window by first
title match. With the target "Ghost Read Bed" and a decoy "Ghost Read Bed 2" in
front, an anchored `ghost_see` (full mode) reported the target's hwnd and
returned the DECOY's elements: its "Increment" buttons at left=798 (the decoy's
position) instead of 218, 10/10 (`c9b-before`).

**Change.** `GhostSession::describe_window(window, hwnd_hint)` makes C9's
`read_scope` decision: the resolved handle on Windows (`describe_hwnd`, the same
interactive walk and node budget as the title path, from a different root),
else the title, else the foreground scope; a resolved window that has closed is
an error. `ghost_see` full mode and `ghost_snapshot` with a window use it with
the injected `_target_hwnd`. `describe_screen` itself is unchanged for its other
callers (CLI, HTTP server, examples). No new unit test: the decision is
`read_scope`, covered by C9's 3 tests.

**Marker.** A full-mode reply lists only interactive elements - no edit values
(stage 6b) and no static labels (checked here: `--label` markers did not appear)
- so the probe places the two windows apart (`--pos`: target 120,120, decoy
700,400) and reads which window the returned "Increment" buttons lie in.

| Criterion | Target | Before (`c9b-before`) | After run 1 / run 2 |
|---|---|---|---|
| Full-mode tree comes from the target (decoy in front) | 10/10 | **0/10** (decoy's elements, left=798) | 10/10 / 10/10 |
| Reported hwnd is the target | 10/10 | 10/10 | 10/10 / 10/10 |
| C9 text read still the target | 10/10 | 10/10 | 10/10 / 10/10 |
| see / find / click / type accuracy, user and hidden (`c9b-regression-*`) | 20/20 | - | 20/20 each |
| `ghost verify` | pass | - | 10/10 claims |
| `cargo test --workspace` | 0 failed | 604 / 0 / 27 | 604 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

Not covered: `ghost_see mode=delta` reads the UIA mirror cache keyed by title
(`describe_screen_delta` -> `cache.snapshot_delta(window, ..)`), a different
mechanism; it needs its own measurement.
