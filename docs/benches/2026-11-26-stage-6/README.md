# Stage 6 - convergence round 1, C2 and C3

Probe-safety rule: every probe here sends input only to testbed windows it
launched itself.

## C2 - ghost_key and edit commands drive the window the target resolver chose

**Change.** `crates/ghost-session/src/session.rs`: a pure `pick_window(windows,
title, hint)` takes the caller's resolved handle first (when that window is still
open and not minimised) and falls back to the first title match only when no
handle came with the call. `act_background`, `key_background` and
`edit_command_background` all resolve through it; the last two gained an
`hwnd_hint` parameter. `crates/ghost-mcp/src/main.rs`, `handle_ghost_key`, passes
the `_target_hwnd` the target resolver already injects (it was passed to
`act_background` only).

**Probe** (`key_routing_probe.py`): testbeds "Ghost Key Bed" (target, launched
first) and "Ghost Key Bed 2" (decoy, launched last, so first in the window list).
Anchor and focus the target's edit through Ghost, then 20 x `ghost_key q` and
20 x background `Ctrl+A`. Edit contents are read with `WM_GETTEXT`, not through
Ghost: `ghost_see mode=text` returned the decoy's text for the target during
this stage (new candidate C9, below).

| Criterion | Target | Before (fa7fef6, `c2-before`) | After run 1 / run 2 |
|---|---|---|---|
| `q` in the target's edit | 20/20 | 0/20 | **20/20 / 20/20** |
| `q` in the decoy's edit | 0/20 | 0/20 (sent to the decoy's frame, no focused control) | 0/20 / 0/20 |
| `ghost_key` response window is the target | 20/20 | 0/20 | 20/20 / 20/20 |
| background Ctrl+A response window is the target | 20/20 | 0/20 | 20/20 / 20/20 |
| Unit: handle beats an earlier title match; no handle = first match; gone/minimised handle falls back; nothing = None | as stated | 3 of 4 red on a stub | 4/4 green |
| `cargo test --workspace` | 0 failed | 580 / 0 / 27 | 584 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

The "before" build is fa7fef6 built in a gitignored worktree
(`target-before-c2/`).

### New candidates found during C2 (not fixed here)

- **C9 - `ghost_see mode=text` reads a different window from the one it
  reports.** With the decoy in front, `ghost_see mode=text window="Ghost Key Bed"`
  answered with `target.hwnd` = the target but the text of the decoy's empty
  edit, while `WM_GETTEXT` showed the target's edit held "abq". Cause:
  `GhostSession::read_text` (session.rs) calls `tree.collect_text(window_title)`,
  which re-resolves by title and drops the injected handle. Other title-only
  read paths need the same audit.
- **C10 - under the default background policy, `ghost_key` with a combo posts
  only the letter.** `handle_ghost_key` (main.rs), background-policy branch:
  `let (_, key_only) = parse_key_combo(keys)?;` drops the modifiers, so
  "Ctrl+A" would post "a", while the comment above says combos are refused. Not
  measured yet (safe to measure on a testbed).

## C3 - background double_click verified like a click

**Change.** The polling loop from stage 2's click branch became one helper per
path: `poll_click_effect` in `session.rs` (async; takes the element by value
because it is not `Sync`) and in `hidden.rs` (blocking, for the hidden desktop's
worker thread). `click` and `double_click` both use it: title every poll, UIA
name/value and whole-window persisting pixels at 80 ms. The user-desktop
`double_click` used to compare only the control's own rect after a fixed 80 ms;
the hidden one took a single whole-window diff after 80 ms. The rect-scoped
pixel closure (`verify_pixels`, `win_origin`) had no other user and is removed.
No new unit test: the helper only sequences captures and reads; the decision it
applies is `click_verify`'s, already covered by its 16 tests.

| Criterion | Target | Before (`c3-before-*`) | After run 1 / run 2 |
|---|---|---|---|
| Real double-click (counter +2), user: correct / verified | 20/20 / 20/20 | 20/20 / **0/20** | 20/20 / 20/20, 20/20 / 20/20 |
| Real double-click, hidden: correct / verified | 20/20 / 20/20 | 20/20 / 5/20 | 20/20 / 20/20, 20/20 / 20/20 |
| Double-click whose only effect is a distant label, user / hidden: verified | 20/20 | 0/20 / 19/20 | 20/20 / 20/20 both runs |
| Double-click with no effect (label), user / hidden: verified=false | >= 19/20 | 20/20 / 20/20 | 20/20 / 20/20 both runs |
| Real double-click median, user / hidden | - | 123.1 / 115.2 ms | 43.5 / 41.5 ms, 44.1 / 38.1 ms |
| Plain click after the refactor, user / hidden: verified | 20/20 | - | 20/20 / 20/20 (`c3-after-click-*`) |
| `cargo test --workspace` | 0 failed | 584 / 0 / 27 | 584 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |

The "before" distant rows report correct 0/20 because that probe version
expected +1 for a double-click; the `verified` counts in them are valid. The
probe now expects +2.

## Review fix to C2 - a supplied handle that is gone or minimised is an error

**Change.** `pick_window` (`crates/ghost-session/src/session.rs`) now returns
`Result<&WindowInfo, PickMiss>`. When the caller supplied a resolved handle and
that window has closed (`HintGone`) or is minimised (`HintMinimized`), the verb
fails with an error naming the cause; it no longer falls back to a title match,
which would hand the input to the decoy exactly when the target has just closed.
The title fallback remains only for calls that came with no handle.
`act_background`, `key_background` and `edit_command_background` all resolve
through it. Also moved `cursor_unchanged`'s doc comment back onto its own
function (C2 and C3 had inserted code between the two).

**What the live probe can and cannot show** (`target_closed_probe.py`, both
windows launched by the probe, decoy edit focused through Ghost and read with
WM_GETTEXT). The target closes, then `ghost_key q`:

| Case | Before (fa7fef6) | After |
|---|---|---|
| no `window` arg, relying on the anchor: refused / decoy got the key | 10/10 / 0/10 | 10/10 / 0/10 |
| `window="Ghost Gone Bed"`: refused / decoy got the key | 10/10 / 0/10 | 10/10 / 0/10 (`pick-after`) |

The MCP target resolver already rejects a closed anchor before `pick_window` is
reached, so the hole is only the window closing between resolution and
dispatch - a race a probe cannot hit on demand. The unit tests are the proof:

| Criterion | Target | Measured |
|---|---|---|
| Unit: resolved handle closed, decoy open -> `HintGone`, not the decoy | error | pass (5 of 5 red on a stub, all green) |
| Unit: resolved handle minimised -> `HintMinimized`, not another match | error | pass |
| Unit: handle beats an earlier title match; no handle -> first visible match; nothing -> `NoTitleMatch` | as stated | pass |
| C2 decoy probe still correct (`pick-c2-regression`) | target 20/20, decoy 0/20 | 20/20, 0/20; response window 20/20; Ctrl+A 20/20 |
| `cargo test --workspace` | 0 failed | 585 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | 0 / 0 findings |

New candidate found here (not fixed):

- **C11 - a closed anchor yields a misleading error.** `ghost_key` with no
  `window` after the anchored window closed answers "'press_key' has no
  background path on the user's desktop and the focus policy is 'background'"
  (10/10, `pick-after/target_closed.json`), which points the agent at the focus
  policy instead of saying the anchored window is gone.
