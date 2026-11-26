# Emergency-stop gate + Linux CI fix (2026-11-26)

Source: private security report against v0.23.4. After `ghost_stop` returns ok (named manual-reset event
`Local\ghost-emergency-stop-event` still signaled), `ghost_act` click on a HIDDEN-desktop target
(app launched via ghost_window launch, surface hidden, mode instant_only) still dispatches and changes state.
Reporter's diagnosis: ghost-mcp/src/main.rs routes hidden targets to dispatch_hidden; ghost-session/src/hidden.rs
`hidden_act` calls BackgroundClicker / UIA patterns without checking is_stopped; ghost-core/src/input/postmessage.rs
click / click_screen / button_click don't check the stop flag (GhostElement.click_background does).
Suspected also: hidden key/scroll, launch, explicit desktop tools, CDP, compound ops.

Goal: EVERY state-changing path (all MCP tools, CLI, HTTP) is refused with a clear `stopped` error while stop is
active, checked (a) before queue submission and (b) immediately before dispatch on the desktop worker, until
ghost_reset. Observation-only tools may keep working.

Rules:
- PATH: export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"; CARGO_TARGET_DIR=target-improve.
- Never commit pre-existing untracked files (.claude/ .gemini/ .kiro/ AGENTS.md GEMINI.md opencode.json .mcp.json
  .ignore .github/copilot-instructions.md *.ps1) or the modified .gitignore. Stage files explicitly by path.
- Live tests touch ONLY windows the test itself created (ghost-testbed). Never charmap/third-party, never the terminal.
- Do NOT install into ~/.local/bin or target/release. No publishing, no push (the operator pushes), no email.
- Another worker may commit dependency bumps (Cargo.lock/manifests) on this branch; don't touch dependency versions.
- Check commands against ~/.claude/lessons/tripwires.json (no taskkill /T, no killing bash.exe, etc.).

## Stage 1: Audit (no code changes)
Write docs/plans/2026-11-26-emergency-stop-audit.md: a table of EVERY action entry point
(MCP tools in ghost-mcp, CLI subcommands in ghost-cli, HTTP routes in ghost-http, hidden-desktop dispatch,
postmessage/sendinput low-level fns, UIA pattern invokes, launch, CDP/ghost-browser, compound/batch/macro ops,
explicit desktop tools), each with: file:line, state-changing yes/no, currently gated yes/no (where).
Then STOP and print "STAGE 1 DONE".

## Stage 2: Central gate + defense in depth
- One central gate (e.g. `ghost_core::stop::ensure_not_stopped() -> Result<(), GhostError::Stopped>`) reading the
  same flag/event that ghost_stop sets, with a clear message like "emergency stop active; call ghost_reset".
- Call it (a) at the single tool-dispatch choke point(s) before queue submission for all state-changing tools,
  classified by an explicit allow-list of observation-only tools (default = gated, so new tools are safe by default),
  and (b) immediately before dispatch on the desktop worker (hidden + visible).
- Defense in depth: low-level dispatch fns (postmessage click/click_screen/button_click, key, scroll, type,
  SendInput wrappers, UIA Invoke/Toggle/SetValue/etc., hidden_act, launch, CDP input) check it too.
Commit: "fix(security): emergency stop gates every action path".

## Stage 3: Regression tests
- Unit tests per path: with stop set, each state-changing entry point returns Stopped and does not dispatch; after
  reset it proceeds. Include a test that the tool classification covers every registered MCP tool.
- Live test (#[ignore] or env-gated like existing live tests) using ghost-testbed: launch-hidden testbed, stop,
  act click -> assert error=stopped and state unchanged, reset, act click succeeds; repeat for key, scroll, type,
  launch; CDP if feasible.
- cargo test --workspace must be green (baseline 623 passed / 0 failed). Run the live test and show output.
Commit: "test(security): regression tests for emergency stop on all paths".

## Stage 4: Linux CI fix
CI run 45887039606 (cf18cd2) failed "Build and unit test" -> Clippy (workspace) with -D warnings on Linux:
- crates/ghost-session/src/session.rs:74 struct `Cover` never constructed; :93 `cover_allowed`, :103
  `invoke_fallback_due`, :181 `cover_refusal` never used (Windows-only callers -> cfg-gate them, or
  cfg_attr(not(windows), allow(dead_code)) if tests use them cross-platform).
- crates/ghost-session/src/click_verify.rs:84 "using `chunks_exact` with a constant chunk size" (x2) -> use
  as_chunks / array chunks as clippy suggests, keeping behavior identical.
Check the CI workflow for exact clippy flags/toolchain; mirror them. Try `rustup target add x86_64-unknown-linux-gnu`
and `cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings` (may fail at linking
native deps; type/lint errors are what matter). Also run Windows clippy to not regress.
Commit: "ci: fix Linux clippy (cfg-gate Windows-only cover helpers, as_chunks)".

## Stage 5: Release notes
CHANGELOG: new 0.24.1 section: Security - emergency stop now blocks every action path incl. hidden desktop
(credit "a privately reported security disclosure", no names); CI fix. Bump workspace version to 0.24.1.
Run node ~/.claude/code-kit/lint/code-vibecode-lint.mjs on changed files -> 0 HIGH.
Commit: "0.24.1: emergency stop gates every action path". Do NOT push.
