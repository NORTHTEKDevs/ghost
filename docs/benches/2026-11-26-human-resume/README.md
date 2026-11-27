# Human-only resume and the stop's reach: live proof (0.24.2)

Plan and decisions: `docs/plans/2026-11-26-human-resume-0242.md`.

These runs use a **private, never-shipped debug build** with the
`isolated-stop-events` feature, so every stop event carries a test namespace
(`Local\ghost-emergency-stop-{event,human}-isolated-<ns>`) and the human's
running Ghost processes are never stopped. The feature cannot be built into a
release binary (`compile_error!` when it is on without `debug_assertions`; a
release build with it fails). The real shared event was sampled every 50 ms
during the runs and was never signalled (`h1-real-event-sample.txt`).

Built with:
`CARGO_TARGET_DIR=target-private cargo build -p ghost-mcp -p ghost-cli -p ghost-testbed --features ghost-core/isolated-stop-events`

## H1: a human stop is lifted only by the human (`h1-human-resume-private.txt`)

| Step | Result |
|---|---|
| launch the testbed (hidden desktop) | ok |
| a "human" stop: `ev.ps1 -Which both` raises the isolated stop and human events (what Ctrl+Alt+G does) | raised |
| `ghost_act` click | refused, -32004: `ghost_act refused: the human stopped Ghost (Ctrl+Alt+G); only the human can resume it - wait for them` (no way out named) |
| `ghost_reset` | refused, same text |
| `ghost_reset scope=process`, then `ghost_act` | reset ok (own flag only); act still refused |
| `ghost reset` run over a pipe (what an agent's own shell does) | exit 2: `refused - run it yourself in an interactive terminal ...`; both events stay SIGNALLED |
| `ghost reset` in a real pseudo-console (`pty_reset.py`, ConPTY): shows a word, the word typed back | `lifted the emergency stop the human raised (Ctrl+Alt+G); every Ghost process resumes`; events clear |
| `ghost_act` click | ok (Count 0 -> 1) |
| an agent-style stop (`-Which stop`), `ghost_reset scope=process`, `ghost_act` | refused: a process-scoped reset does not lift a shared stop |
| `ghost_reset`, `ghost_act` | ok: an agent's own stop is lifted by its reset |

The pseudo-console run also shows the documented limit: a same-user program
that drives a terminal can answer the challenge, and any same-user program can
change the named event directly. The design stops an agent acting through
Ghost and an agent's pipe-driven shell, not arbitrary code running as the user.

## Fail closed: the exact text when the stop event cannot be read (`fail-closed-private.txt`)

`squat.ps1` creates the isolated stop event with a DACL that grants only SYSTEM,
before the server starts. Then:

| Surface | Result |
|---|---|
| MCP `ghost_act` | -32004: `ghost_act refused: Ghost is stopped (fail closed) - cannot open the shared emergency-stop event Local\... (Access is denied. (0x80070005)); a stop raised by another Ghost process would go unseen here, so this process refuses every action. End the Ghost process that created the event ...` with hint `... no stop was raised and ghost_reset will not help ...` |
| MCP `ghost_reset` | the same error (it used to answer ok) |
| CLI `ghost launch` | exit 1, same reason |
| CLI `ghost reset` in a real terminal, word typed correctly | exit 1: `could not lift the stop - cannot open ...` |

## Covered by tests (in the default suite)

- `ghost-core` `hotkey`: only the human lifts a human stop; a default build ignores
  `GHOST_ISOLATED_STOP_NS`; only a physical Ctrl+Alt+Shift+G resumes (injected keys,
  missing modifiers refused); an unreadable event fails closed.
- `ghost-cli` `reset_needs_a_human`: `ghost reset` with piped stdin exits 2 before it
  touches any stop event; the challenge is random and must match.
- `ghost-mcp`: no agent-facing text or tool description names a way out; owned closes pass
  the gate, a user-window close does not.
- `ghost-session`: closing only what Ghost owns while stopped (unit + live: an attached
  browser's tab is refused, the owned browser's tab closes); a stop after a server restart
  ends the old server's shell children (child process plays the old server); snapshots
  expire after 24 h / at reboot; a persistent session's command is ended by the stop, says
  "ended by an emergency stop (since lifted)" after reset, and is pruned from the list;
  `exit 57005` is a normal exit.
- `ghost-linux` (CI): a stop kills the shell's process group; a reused group id is not
  signalled; only a real (non-XTEST) resume key resumes.
