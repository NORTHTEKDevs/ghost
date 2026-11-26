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

