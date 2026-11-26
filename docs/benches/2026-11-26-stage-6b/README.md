# Stage 6b - C10, C9, C11

Probe-safety rule: every probe sends input only to testbed windows it launched;
edit contents are read with WM_GETTEXT (a read, no input) unless a row says
otherwise.

## C10 - a background ghost_key combo is never posted as its bare key

**Defect.** Under the default background policy (`window` given, no
`background` flag), `handle_ghost_key` did `let (_, key_only) =
parse_key_combo(keys)?` and posted `key_only`: every combo's modifiers were
dropped. Measured (`c10-before`): "Ctrl+A", "Ctrl+S", "Ctrl+Shift+K" and "Alt+X"
each typed their bare letter into the focused edit, 10/10 each, 40/40 - so
"Ctrl+S" put an "S" into the document. The comment above the code said combos
were refused.

**Change** (`crates/ghost-mcp/src/main.rs`). A pure `background_key_plan(mods,
key)` decides: no modifiers -> post the key; Ctrl+C/X/V/A/Z -> the semantic edit
command (WM_COPY/CUT/PASTE/UNDO, EM_SETSEL); any other combo -> refuse.
`key_in_background` executes the plan and is now the only background key path,
used by both `background=true` and the background policy, so they cannot
disagree again. The refusal says the combo was not sent, why, which shortcuts
work in the background, and that other combos need the operator-unlocked
foreground policy.

| Criterion | Target | Before (`c10-before`) | After run 1 / run 2 |
|---|---|---|---|
| Combos that typed their bare key (4 combos x 10) | 0/40 | **40/40** | 0/40 / 0/40 |
| Ctrl+A delivered as the select_all edit command | 10/10 | 0/10 (typed "A") | 10/10 / 10/10 |
| Ctrl+S, Ctrl+Shift+K, Alt+X refused with the reason | 30/30 | 0/30 | 30/30 / 30/30 |
| A single key still delivered (`c10-key-regression`, decoy case) | target 20/20, decoy 0 | - | 20/20, 0 |
| Unit: single key -> post; Ctrl+A/C/V -> edit command; 7 other combos -> refuse | as stated | 2 of 3 red on a stub | 3/3 green |
| `cargo test --workspace` | 0 failed | 585 / 0 / 27 | 588 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

## C9 - ghost_see mode=text reads the window it reports

**Defect.** `GhostSession::read_text` called `tree.collect_text(window_title)`,
which re-resolves the window by first title match and drops the handle the
target resolver injected. With the target "Ghost Read Bed" and a decoy
"Ghost Read Bed 2" in front, an anchored `ghost_see mode=text` reported the
target's hwnd (10/10) and returned the decoy's text (target content 0/10,
`c9-before`).

**Change.** `read_text(window, max_chars, hwnd_hint)`: a pure `read_scope`
picks the resolved handle when there is one and the engine can read by handle
(Windows: `collect_text_in_hwnd`), else the title, else the foreground scope.
A resolved window that has closed is an error naming it, not a title fallback.
`ghost_read_text` passes `_target_hwnd`.

**Probe** (`read_routing_probe.py`): both testbeds launched by the probe; the
target's edit gets "abq" through Ghost; reads rely on the anchor. (A first
version clicked the target's Increment, which made its title unique and hid the
defect; it was corrected before any result was kept.)

| Criterion | Target | Before (`c9-before`) | After run 1 / run 2 |
|---|---|---|---|
| `see mode=text` contains the target's "abq" | 10/10 | **0/10** | 10/10 / 10/10 |
| reported hwnd is the target | 10/10 | 10/10 | 10/10 / 10/10 |
| Unit: handle read when possible; title otherwise; foreground with neither | as stated | 2 of 3 red on a stub | 3/3 green |
| see text / see full / find accuracy, user and hidden (`c9-regression-*`) | 20/20 | - | 20/20 each |
| `see mode=text` median, user desktop | - | ~34 ms (stage 6) | 14.1 ms (reads by handle, no title walk) |
| `cargo test --workspace` | 0 failed | 588 / 0 / 27 | 591 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

**Not covered - C9b.** The full-mode tree read (`describe_screen(title)`, also
`describe_screen_delta`) has the same title-based shape, but this probe cannot
tell the windows apart through it: a full-mode reply never contains an edit's
value (control without a decoy: text has "abq", tree does not). The tree rows
read 0/10 before and after for that reason, not as a measured defect. Needs a
marker the tree exposes (for example a title change on the target only) to
measure.

## C11 - a closed anchor is an error, never a silent switch to the foreground

**Defect.** `resolve_target(None)` used the anchor if its window was alive and
otherwise fell back to the human's foreground window, and `live_anchor` cleared a
dead anchor. So after the anchored window closed, an unanchored verb retargeted
whatever window the human was using - against the design's own rule ("once a
target has been anchored, unanchored verbs use the anchor ... never the human's
window by accident"). For `ghost_key` this surfaced as a focus-policy error that
never mentioned the closed window: "'press_key' has no background path on the
user's desktop ..." (10/10, `stage-6/pick-after/target_closed.json`).

**Change** (`crates/ghost-session/src/target.rs`). A pure
`anchor_status(anchor, candidates)` returns `Unset`, `Live` or `Closed`. With no
`window` argument, `Closed` is an error naming the anchored window and saying
nothing was done and how to continue; the foreground is used only when no window
was ever anchored. The closed anchor is kept (it is no longer cleared) so every
unanchored call keeps refusing until the agent passes a `window` or clears the
anchor with `ghost_window op=anchor clear=true`. `live_anchor`, which only
reports, still returns `None` for it.

**Probe** (`anchor_closed_probe.py`, both windows launched by the probe; the
closed-anchor `ghost_see` is not run on the old build because there it would
read the human's foreground window).

| Criterion | Target | Before | After run 1 / run 2 |
|---|---|---|---|
| `ghost_key` after the anchor closed: refused, naming the closed anchor | 10/10 | refused 10/10 with the focus-policy error, naming it 0/10 | 10/10 / 10/10 |
| `ghost_see mode=text` after the anchor closed: refused, naming it | 10/10 | not run (would read the human's window) | 10/10 / 10/10 |
| after re-anchoring with `window=`, unanchored read comes from that window | 10/10 | - | 10/10 / 10/10 |
| Unit: live anchor used; closed anchor `Closed` even with another window open; same handle on another desktop is not it; never anchored `Unset` | as stated | 3 of 4 red on a stub | 4/4 green |
| see / find / click / type accuracy, user and hidden (`c11-regression-*`) | 20/20 | - | 20/20 each |
| C2 decoy routing (`c11-key-regression`) | target 20/20, decoy 0 | - | 20/20, 0 |
| `ghost verify` | pass | - | 10/10 claims (`c11-verify.txt`) |
| `cargo test --workspace` | 0 failed | 591 / 0 / 27 | 595 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

The error reads "Process not found: the anchored window '<title>' (it has closed,
so nothing was done; pass window=<title> ... or ghost_window op=anchor
clear=true ...)": it keeps the error variant (and code) agents already get for a
title that matches nothing.
