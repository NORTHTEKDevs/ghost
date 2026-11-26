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

