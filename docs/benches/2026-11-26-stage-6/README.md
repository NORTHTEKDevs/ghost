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
