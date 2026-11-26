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
