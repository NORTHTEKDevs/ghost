# Ghost improvements - progress log (2026-11-25)

Driver: Grok Bot, via cc-bridge session `ghost-improve` (id f23e9f). Plan: `2026-11-25-ghost-improvements.md`.
Rules: builds only in `CARGO_TARGET_DIR=target-improve`; never touch `target/release` or `~/.local/bin`
(live ghost-mcp); never commit the pre-existing untracked files (.claude/ .gemini/ .kiro/ AGENTS.md GEMINI.md
opencode.json .mcp.json .ignore .github/copilot-instructions.md *.ps1) or the modified .gitignore; no push.
cargo: `export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"`.

## Stage 1 - baseline + plan - DONE (commit d0384b3)
Verified by Grok Bot: tests 550 passed / 0 failed / 27 ignored (two runs), clippy 0 warnings, doctor 7/7,
verify 10/10, 10/10, 9/10, 9/10 (false FAIL: foreground change from other apps). act click verified=true
0/20 user desktop, 10/20 hidden; tab screenshot median ~1005 ms.
Review amendments sent: disabled-button negative control; distant-label positive control; whole-window
pixel diff with noise filtering (not element-rect-only); item 3 must FAIL Ghost-caused foreground changes.

## Stage 2 - act click verification (item 1) - DONE (commit 9300281)
Grok Bot re-ran on target-improve (11:50 PM AKST): build ok; cargo test 566 passed / 0 failed / 27 ignored;
clippy 0 warnings; vibecode lint on 4 changed .rs files 0 findings. Independent probe n=20:
user increment 20/20 verified, median 43.8 ms (baseline 123.4, 0/20 verified); hidden increment 20/20,
35.7 ms (baseline 113.4, 10/20); distant-label 20/20 verified both surfaces (152-170 ms, by pixels);
static label (no effect) 0/20 verified both surfaces (no false positives). Claude's runs: 160/160 real,
0 false positives in 240 no-effect clicks. Follow-up found: user-desktop background path posts BM_CLICK
to a DISABLED control and returns ok:true (hidden + foreground paths refuse) -> queued as item 4.

## Stage 3 - headless tab screenshot latency (item 2) - DONE (commit a095122)
Change: `--disable-gpu-compositing` for headless launches only (crates/ghost-browser/src/launch.rs) + unit test.
Grok Bot re-ran on target-improve (12:10 AM AKST): cargo test 567 / 0 / 27; clippy 0; lint 0 findings;
tab_shot_probe n=20: first 36.6 ms, median 31.3 ms, p95 50 ms, valid PNG 20/20, page centre pixel 20/20
(baseline median ~1005 ms, first ~1850 ms). ghost verify 2/2 ALL CLAIMS HOLD, screenshot claim 42 / 39 ms
(baseline 1705-1795 ms), whole run 4.7-4.8 s (was ~8 s). Cost: ~3.7% of one core per idle headless Chrome.
Not tested: WebGL-heavy pages under the new flag; attached-browser path (unchanged code).

## Stage 4 - verify attribution (item 3, commit 5465d10) + disabled-control refusal (item 4, commit b8dd3a5) - DONE
Grok Bot re-ran on target-improve (12:25 AM AKST): cargo test 575 / 0 / 27; clippy 0; lint 0 findings on
verify.rs, session.rs, run_bench.py. ghost verify 5/5 ALL CLAIMS HOLD (Claude: 10/10; baseline 2/4).
Probe user surface n=20: disabled control refused 20/20 (errors=20, 18 ms), increment verified 20/20
(43.4 ms), label verified=false 20/20 - unchanged from stage 2.
Review follow-ups queued (stage 5): (a) audit only records SYNTHETIC changes, so a Ghost foreground theft
while the human is typing would be classed as real input and pass verify -> also fail when the final
foreground window's pid is a Ghost-launched pid; (b) UIElement::is_enabled maps a UIA error to false, so a
flaky provider gets refused on all three paths -> refuse only on a definite false.

## Stage 5 - review follow-ups: (a) verify fails if a Ghost-launched window holds the foreground at the end (334d085); (b) act refuses only a definite is_enabled false (8679c06) - GATES VERIFIED
Grok Bot (12:32 AM AKST): cargo test 580 / 0 / 27; clippy 0; lint 0 findings on 7 changed files; verify 3/3.
Probe re-check CONTAMINATED: my probes overlapped Claude's final-matrix probes (12:31-12:33 AM), giving
label false positives (user 8/20) and a 919 ms p95. Claude told to discard/re-run its overlapping output;
I re-run mine only once the session is idle. Lesson: never run probes while the driven session is probing.
Not tested: Linux/macOS edits (no C cross-compiler for ring here; CI would compile them).
Next: convergence round 1 (docs/benches/2026-11-25-final/, docs/plans/2026-11-25-convergence-round1.md).

Stage 5 probes re-run clean by Grok Bot (12:42 AM AKST, Claude idle): user/hidden disabled refused 20/20;
increment verified 20/20 (median 55.8 / 45.3 ms - the 43-vs-59 ms timer-tick clustering); label
verified=false 20/20 both surfaces. Stage 5 VERIFIED.

## Convergence round 1 (commit fa7fef6, docs/plans/2026-11-25-convergence-round1.md) - NOT CONVERGED
Re-measure vs baseline: tests 550 -> 580; verify all-pass 2/4 -> 3/3 (+10/10, 3/3 earlier); tab screenshot
1007 -> 31.6 ms; act click user 123 ms 0/20 verified -> 43 ms 20/20; hidden 113 ms 10/20 -> 42 ms 20/20.
New candidates with evidence: C1 click_at without hwnd posts to whatever covers the point (during the review
10 probe clicks went into the ghost-improve terminal tab - Claude's own session window, nothing else);
C2 key/edit re-resolve window by substring and hit a decoy 3/3; C3 double_click verified 1/10;
C4 pinned click_at always falls back to UIA Invoke, 337-360 ms; C5 hidden no-effect false positive 1/140;
C6 verify blind to a Ghost raise the human then leaves; C7 hidden click_at/double_click fixed 80 ms.
Grok Bot added C8 (timer-tick bimodal click median). Rule adopted: no probe may send input to a window it
did not create.
Next: STAGE 6 = C2 + C3 (sent 12:43 AM AKST); STAGE 7 = C1 + C4; STAGE 8 = C6, C7 (measure), C5, C8.

## Stage 6 - C2 key/edit routing (94bfc03), C3 double_click verification (8d80333), C2 follow-up: refuse when the resolved window is gone (d586cce) - VERIFIED
Grok Bot (1:15-1:25 AM AKST, Claude holding idle, no probe overlap): 8d80333 gates in an isolated worktree
(584 / 0 / 27, clippy 0, lint 0); d586cce at HEAD: cargo test 585 / 0 / 27, clippy 0, lint 0 on main.rs,
session.rs, hidden.rs.
- C2 key_routing_probe n=20, decoy window in front: q landed in target 20/20, decoy 0/20, response names
  target 20/20; Ctrl+A response names target 20/20 (Claude's before: 0/20).
- C3 latency_probe n=20: double_click increment verified 20/20 user (50.6 ms) and hidden (32.9 ms)
  (before 0/20 and 5/20, ~120 ms); double_click label verified=false 20/20 both; plain click unchanged.
- d586cce target_closed_probe n=10: target closed -> refused 10/10 in anchor and explicit modes, decoy got
  the key 0/10. Anchor-mode error text is misleading ("press_key has no background path") -> C11.
Queued: stage 6b = C10 (background ghost_key drops combo modifiers), C9 (ghost_see mode=text resolves by
title), C11 (error text). Then stage 7 = C1 + C4, stage 8 = C6, C7, C5, C8.

## Stage 6b - C10 combo modifiers (8ecbf09), C9 see text by handle (eb1a2ed), C11 closed anchor refuses (363cf14) - VERIFIED
Grok Bot (1:35 AM AKST, Claude idle): cargo test 595 / 0 / 27; clippy 0; lint 0 on all crates files changed
since d586cce. Probes n=10, all windows launched by the probes:
- C10 combo_probe: bare key typed 0/40 (before: Claude measured 40/40 - "Ctrl+S" typed an "S"); Ctrl+A as
  select_all 10/10; Ctrl+S, Ctrl+Shift+K, Alt+X refused 30/30 with a clear "was not sent" error.
- C9 read_routing_probe: see mode=text reads the target 10/10 with decoy in front (before 0/10).
  see mode=full (tree) still reads the target 0/10 by this marker -> C9b (needs a tree-visible marker).
- C11 anchor_closed_probe: ghost_key and ghost_see after the anchor closed refuse and name it 10/10 each;
  re-anchoring works 10/10. Behaviour change: a closed anchor is sticky until window= or anchor clear.
- C2 key routing regression: target 10/10, decoy 0/10.
Next: STAGE 7 = C1 + C4 + C9b (sent 1:37 AM AKST).

## Stage 7 - C1 cover refusal (8aa410d), C4 click_at to the child under the point, no double fire (b406c10), C9b full-mode see by handle (757222c) - VERIFIED
Grok Bot (2:00 AM AKST, Claude idle): cargo test 604 / 0 / 27; clippy 0; lint 0 on crates changed since
363cf14; ghost verify ALL CLAIMS HOLD. Probes (only probe-created, topmost windows; WindowFromPoint checked
before each click):
- C1 cover_probe n=20: covered point refused 20/20, 0 clicks into the cover (Claude's before: 39 clicks in
  the cover); uncovered target click lands 20/20 verified; same-process owned popup reached 20/20.
- C4 pinned_click_probe n=20: increment lands exactly once 20/20, fallback 0, verified 20/20, foreground
  unchanged, median 23.9 ms (was ~340 ms via UIA Invoke); static label verified=false 20/20 (~230 ms incl.
  guard settle). Before the fix the unpinned path double-fired (39 activations in 20 calls).
- C9b read_routing_probe: see tree reads the target 10/10 (was 0/10); see text 10/10.
- Regressions: key routing 10/10 target / 0 decoy; act click user 44.4 ms, hidden 37.7 ms, 20/20 verified.
Nit for stage 8: cover refusal is raised as GhostError::Vision ("Vision error: ...") - wrong error kind.
Open: ghost_see mode=delta cache keyed by title (unmeasured); C6, C7, C5, C8.
Next: STAGE 8 (sent 2:01 AM AKST).

## Stage 8 - error kinds (e1c92f7), C6 audit during-input incidents (c95c4e2), C7 hidden click_at (1991c97), measurements (88fd892) - VERIFIED
Grok Bot (2:20 AM AKST, Claude idle): cargo test 610 / 0 / 27 on 3 consecutive runs (no flake reproduced);
clippy 0; lint 0 on crates changed since 757222c; ghost verify 2/2 ALL CLAIMS HOLD.
- Cover refusal now "Element not interactable" 20/20; uncovered clicks still land 20/20.
- C7 hidden_click_at_probe n=20: increment correct+verified 20/20, 22.6 ms (Claude's before: 0/20 - the
  click never reached the child control); distant note 20/20 verified (pixels); no-effect label
  verified=false 20/20; double_click 20/20.
- Measured, not fixed (by Claude): C5 0/200 false positives on each surface (gap closed); C8 bimodal
  43/59 ms, max ~9 ms average saving - not worth it.
- NEW DEFECT: ghost_see mode=delta never reports a change (snapshot_delta ignores its window and diffs a
  global mirror nothing fills). My delta_probe: explicit and anchored 10/10 empty despite real changes.
Decision (Grok Bot): fixing delta to honour its documented contract is in scope, not a contract change ->
STAGE 9. Then convergence round 2.

## Stage 9 - mode=delta per window (1a8c5cc) - VERIFIED; Convergence round 2 (f4e031e)
Grok Bot (2:35 AM AKST): cargo test 619 / 0 / 27 x2; clippy 0; lint 0 on crates changed since 88fd892;
ghost verify ALL CLAIMS HOLD. delta_routing_probe n=10, explicit and anchored: no change -> empty 10/10,
target change reported 10/10 (was 0/10), decoy change not reported 10/10. Key routing re-check 20/20 / 0.
Round 2 (docs/plans/2026-11-26-convergence-round2.md): no accuracy or safety defect with evidence left.
Remaining: R2-1 ghost_key waits two back-to-back guard settles (median 190.6 ms; drop inner 60 ms ->
~130 ms), R2-2 diagnostic exit code for a once-in-~40-runs ghost-core test exit, R2-3 port bench/soak to the
hidden-desktop testbed (tooling, not Ghost - deferred).
Decision: STAGE 10 = R2-1 (+ R2-2, tiny), then declare convergence unless it surfaces something new.

## Stage 10 - ghost_key single guard (52f46b0), kill exit code diagnostic (dfb0689) - VERIFIED
Grok Bot (2:48 AM AKST): cargo test 623 / 0 / 27 x2; clippy 0; lint 0 on crates changed since f4e031e.
key_routing_probe n=20: ghost_key median 126.6 ms (was 190.6), target 20/20, decoy 0/20, focus_preserved
20/20. The 60 ms watch is kept only for background=true under the foreground policy (no outer guard there).

## CONVERGED (2:50 AM AKST)
Round 2 found no remaining accuracy or safety defect with evidence; its only runtime item (R2-1) is done
and R2-2 is a diagnostic. R2-3 (port bench/run_bench.py + soak.py to the hidden-desktop testbed) is
measurement tooling with no runtime payoff - left for Kristian to decide.
Final independent re-measure (target-improve @ dfb0689): doctor all pass; verify 3/3 ALL CLAIMS HOLD
(screenshot claim 86 ms, run 4.9 s); act click verified 20/20 user (58.5 ms) / hidden (41.9 ms); find
18.1 / 5.7 ms; see text 17.6 / 19.1 ms; type 5.4 / 5.2 ms; tab screenshot median 31.6 ms, first 60 ms,
page correct 20/20.
Not deployed: target/release and ~/.local/bin ghost-mcp.exe are still the old build; nothing pushed.

