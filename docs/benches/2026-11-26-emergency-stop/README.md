# Emergency stop on every action path (2026-11-26)

Plan: `docs/plans/2026-11-26-emergency-stop-gate.md`; audit:
`docs/plans/2026-11-26-emergency-stop-audit.md`. Built with
`CARGO_TARGET_DIR=target-improve`.

The live test (`crates/ghost-session/tests/emergency_stop_live.rs`, `#[ignore]`)
launches a testbed on Ghost's hidden desktop and a headless browser, and gives
input to nothing else. For each path it raises the stop, acts, checks the
testbed's or page's state, resets, and acts again. The stop is raised with the
process-local flag only: the shared event would stop every Ghost process on the
machine, including other sessions' servers.

| Path | Before the fix (`live-before-fix.txt`, HEAD 9cbaf38) | After (`live-after-fix.txt`, `live-final.txt`) |
|---|---|---|
| hidden act click | ran, Count changed | refused, unchanged, works after reset |
| hidden act type | ran, text typed | refused, unchanged, works after reset |
| hidden key | ran, key landed | refused, unchanged, works after reset |
| hidden click_at | ran, Count changed | refused, unchanged, works after reset |
| hidden scroll | ran | refused, works after reset |
| launch onto the hidden desktop | a process started | refused, nothing started, works after reset |
| CDP tab click (headless) | ran, page changed | refused, unchanged, works after reset |

## Unit tests (in the default suite)

- `ghost-mcp` `stop_gate_tests` (6):
  - every dispatchable tool name has an explicit class (scanned from the schemas and every `"ghost_..." =>` arm in main.rs and ext.rs);
  - every Act tool is refused while stopped, with the -32004 stopped code, and allowed after reset;
  - reads, `ghost_stop` and `ghost_reset` stay available;
  - verbs whose args decide (window op, clipboard op, wait for, shell op, focus policy) are classified by those args;
  - an unknown tool is refused.
- `ghost-core` `tests/emergency_stop.rs` (4):
  - the 10 PostMessage primitives refuse while stopped and run after reset;
  - focus, window state, focus-under-point, launch, clipboard, scroll, SendInput click and key all refuse;
  - the hidden-desktop worker refuses `exec_action` / `with_uia_action` and its click/type/press/scroll/launch, but still reads;
  - a job queued before the stop and still waiting when it arrives does not run. A mutation check (the worker-side re-check removed) makes this test fail: "the queued job was not refused".
- `ghost-session` `tests/emergency_stop.rs` (2):
  - launch, hidden launch, focus, window state, clipboard, intents, desktop create/launch/click/type/close, browser launch, tab open and raising the focus policy all refuse;
  - dropping to `background` stays allowed;
  - the error names `ghost_reset`.
- `ghost-cli`: action subcommands are gated, reads are not.
- `ghost-http`: action routes answer 409 while stopped.

Calls made after a reset target nothing real (hwnd 0, a missing window name, a
missing program). Primitives whose un-gated call would touch the real desktop
(SendInput, the clipboard, raising the window under a point) are only called
while stopped.

## Totals

`cargo test --workspace`: 623 / 0 / 27 before, 623 / 0 / 28 after Stage 2 (the
live test is `#[ignore]`d), **637 / 0 / 28** after Stage 3, two runs each.
Clippy `-D warnings --all-targets` 0, lint 0 HIGH.
