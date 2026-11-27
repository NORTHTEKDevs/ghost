# Quality pass after 0.24.2 (2026-11-26)

Reviews: security audit (all crates), code review x3 (session+core, mcp/cli/http/browser,
linux/macos/ground/intent/cache), docs-vs-code review, generalization probe on the core MCP tools.
Baseline on 05244aa: `cargo test --workspace` green, Windows clippy green; Linux clippy (rustc 1.99,
WSL) failed on two new lints, fixed in c84043d. Two tests called the real clipboard writer, fixed
in 6bd9b8c.

Rules for every fix: a failing regression test first; no test touches the real clipboard or the
shared stop events; nothing drives the human's windows.

## Findings to fix (H = high, M = medium)

Security / front end
- S1 H ghost-http: no Host/Origin check, `/run` takes a text/plain body (CSRF, DNS rebinding).
  Fix: reject non-loopback Host, reject any Origin, `/run` takes Json, refuse non-loopback --addr.
- S2 H `GHOST_SHELL=off` is bypassed by launch verbs (`ghost_launch "powershell -c ..."`).
- S3 M `ghost_http_get/post`: DNS names resolving to loopback/private pass; caller `Host` header
  forwarded; 100.64/10 etc. not blocked.
- F1 H `ghost_wait for=text|element|value` ignores the resolved window (can click the human's).
- F3 M tool deadline detaches the task instead of aborting it; `notifications/cancelled` ignored.
- F4 M `ghost_run` retries Act steps (double effects); its documented short op names fail.
- F5 M `ping` / unknown methods: wrong error, gated by the stop.
- F6 M one CDP socket error kills every tab of that browser; 16 MiB frame cap.

Session / core (Windows)
- C1 H anchored hidden window loses to a same-title user window (`target.rs` pick tie).
- C2 H type sends WM_SETTEXT to non-text controls (rewrites a button caption, verified=true).
- C3 M typed read-back has no before/after comparison (empty text always verified).
- C4 M BM_CLICK on a button that opens a modal: 2 s stall then reported as failure.
- C5 M off-screen coordinates are clamped to the edge and clicked.

Linux / support crates
- L-H1 H VLM pixel answers within 0..1000 are rescaled as 0-1000 coordinates.
- L-H2 H AT-SPI queued job runs after the caller timed out and after a stop.
- L-H3 H Linux stop is per process; `ghost reset` / docs claim every process.
- L-M1 M stop cost grows with every shell run (/proc scan per dead group).
- L-M2 M shell timeout / op=kill kills only the leader, not its group.
- L-M3 M resume-key XTEST filter misses devices added after register.
- L-M4 M `ghost-cache` invalidate rewinds seq (stale delta served as fresh).
- L-M5 M VLM escalation enabled by ambient ANTHROPIC/NVIDIA/OPENAI keys; README says only
  `GHOST_VISION_API_KEY` enables it. Code is brought in line with the README.

Docs (MEDIUM): vision key claim, comparison.md "no DOM", CONTRIBUTING version list; plus LOW items.

## Status
(updated as fixes land)
- Committed on main: 6bd9b8c (clipboard tests), c84043d (clippy 1.99), 0dc0d86 (probe scripts),
  66c4382 (docs). Linux clippy runs in WSL Ubuntu-24.04 (`CARGO_TARGET_DIR=/root/ghost-target-improve`,
  DISPLAY/WAYLAND_DISPLAY unset; `wsl` exit codes are unreliable, read the log).
- Fix agents in worktrees: A = S1+S2, C = C1-C5, D = Linux L-H2/H3/M1-M3, E = L-H1/M4/M5,
  B = F1/F3-F6/S3. Integrate by cherry-pick, then full gates, then re-review.
- Integrated on main (all gates green: Windows 694 tests, Linux 304 tests in WSL, both clippies):
  S1 b393d1c, S2 71c1dcc; L-H1 38d1d61, L-M4 e10082e, L-M5 0d5b57e; C1 5a97b0d, C2 3fb2a6d,
  C3 8f3c8ef, C4 caadf0e, C5 17a7028; L-H2 c27bd8b, L-H3 4762c7e, L-M1 37bd99a, L-M2 ac4fe42,
  L-M3 08dfe58; docs c131d3c. Waiting: agent B (F1/F3-F6/S3 + new F7: unknown stop scope = everywhere).
- INCIDENT 19:00-19:10: the probe sent ghost_stop {"scope":"nope"}; StopScope::of maps unknown values
  to Everywhere, so the real shared stop was raised. Lifted by ResetEvent (agent stop, human event
  absent); probe stopped, its 8 testbed processes ended. Rerun the probe with ghost_stop/ghost_reset
  forbidden in the harness.
- Round 1 complete and pushed (9831f08; CI + Linux CI green). Agent B: F5 947569c, F3 c6b745e,
  F4 10fa5b6, F7 9bcf0d6, S3 7832e82, F1 3d014fa, F6 9831f08.
- Re-review of 05244aa..9831f08: 19/22 fix commits VERIFIED. Round 2 (two worktree agents):
  R2a = H2 typing reformat retypes (8f3c8ef), M1 abort skips clipboard/foreground restore (c6b745e),
  M2 wait appears=false on a closing window, M3 wiring tests (C2, C5, F1);
  R2b = H1 GHOST_SHELL=off launch denylist bypasses (8.3 names, PS flag prefixes, proxy launchers,
  Linux terminals) + GHOST_LAUNCH_ALLOW, S3 resolver wiring test, LOW: http Host forms, fec0::/10,
  ::a.b.c.d, no_proxy doc, Linux geteuid.
- Probe rerun on 9831f08 (harness deny-list enforced; stop event False before/after; logs in
  scratchpad/probe2): H1 ghost_see unknown/selection/marks modes read the human's foreground;
  M ghost_shell read never drains, busy session vanishes, PS failures exit 0, bad cwd warm path;
  M browser file:/chrome: navigation + no output cap on tab_eval/tab_text; M wait for=value reads
  the name. Round 3 agents: R3a see/wait/validation, R3b shell, R3c browser; NAT64/6to4/Teredo
  forwarded to R2b. Decision: tab navigation keeps localhost (dev servers), refuses non-http(s)
  schemes.
- Rounds 2-3 merged and pushed (ba95d54; CI + Linux CI green). Flaky stop-shell test root-caused
  (spawn-then-adopt race + async watcher) and fixed in 7c21157 (shells start suspended).
- Live re-probe on ba95d54: 12/14 FIXED, value read by name still lands on the label (V1), L3
  capped by design. Re-review of 9831f08..ba95d54: 0 HIGH, 7 MEDIUM. Round 4: R4a = V1 label vs
  field, V2 label name fallback, V3 wait for=text closing window, V4 typing early exit, L1 batch
  validate_args, L3 doubled type, L12 doc; R4b = B1 confine fail-closed, B2 shell-off arg scan
  false positives, B3 launch id leak on abort, B4 ERR_ABORTED downloads, L4 NAT64 /48, L10
  about:blank#, L6 cwd ~, L8 kill lock order.
- Kept on purpose: warm PowerShell reports any error record as exit 1 (cold path returns 0).
- Residual LOW: a click that only opens a modal dialog reports verified=false (C4 note); Linux has no
  watcher releasing held modifiers on a stop raised by another process.
- Remaining docs LOW (done in c131d3c except mcpb wording, done in S2): hidden env vars (GHOST_HTTP_ALLOW_PRIVATE, GHOST_BROWSER_PATH,
  GHOST_TOOL_DEADLINE_MS) undocumented; ghost_click_at/execute_intent legacy names in README;
  CLI --help text "Windows ... no CDP"; signing-policy `ghost_http` typo; mcpb shell wording (A).
