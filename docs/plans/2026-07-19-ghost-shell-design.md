# ghost_shell — full shell control (v0.16.0)

Date: 2026-07-19
Status: approved (standing no-approval-gates directive; design decisions recorded here)

## Problem

Ghost drives GUIs but has no shell verb. The agent gap: launching CLIs, running builds,
spawning new Claude Code sessions, editing files on machines whose MCP client has no file
tools, and driving PowerShell/terminal workflows. Users route around it with brittle
`ghost_window op=launch` + keystroke chains.

Measured context (2026-07-19, this box): ghost-mcp.exe boots in 27ms, RSS 13MB,
ghost_see fast = 21-69ms. Boot/footprint are NOT the problem — perceived slowness is
agent-loop round-trips and lingering parent `claude` processes holding old server
instances. The upgrade is capability, not boot speed.

## Decision

One new lean verb, `ghost_shell`, implemented in `ghost-session` (state holder), exposed
in `ghost-mcp`. 19 → 20 tools.

### Ops

| op | args | behavior |
|----|------|----------|
| `run` (default) | `cmd` (req), `shell`=powershell\|pwsh\|cmd, `cwd`, `timeout_ms` (default 30000, cap 600000) | One-shot: spawn shell, run cmd, capture merged output, kill process on timeout. |
| `open` | `id` (optional name, default auto `s1`,`s2`…), `cwd` | Start a persistent PowerShell session (state: vars, cwd, env persist across `send`). |
| `send` | `id` (req), `cmd` (req), `timeout_ms` | Run cmd in the persistent session. On timeout returns partial output, session marked busy until the command completes (drain via `read`). |
| `read` | `id` (req), `timeout_ms` (default 0 = just drain) | Collect output that arrived since last read; completes a busy command if its sentinel arrived. |
| `list` | — | Sessions with id, busy, pid, age. |
| `kill` | `id` (req) | Kill the session process. |

### Persistent-session protocol

Driver: `powershell -NoProfile -NoLogo -NonInteractive -Command <loop>` where the loop
reads `<nonce> <base64(cmd)>` lines from stdin, `Invoke-Expression`s the decoded command
with output merged (`2>&1 | Out-String`), then prints a sentinel line
`__GHOST_DONE_<nonce>__ <exitcode>`. Base64 framing means any command text is safe (no
quoting/injection into the driver). Nonce = per-session counter — a stale sentinel from
a timed-out earlier command can never be mistaken for the current one.

`cmd` shell is one-shot only (`cmd /S /C`); persistent sessions are PowerShell only.

### Output discipline

- Merged stdout+stderr (PowerShell semantics), UTF-8 lossy.
- Tail-truncate at 24 000 chars per response with `truncated: true` + total byte count —
  protects the agent context window, matches ghost_see limits philosophy.
- Result JSON: `{ ok, output, exit_code, duration_ms, truncated, timed_out, id? }`.
  `exit_code` is `$LASTEXITCODE` (native commands) — may be null for pure-PS commands.

### Safety

- `GHOST_SHELL=off` env kill-switch: every op returns a clear error. Documented in
  README security section (Ghost is public; some deployments want automation-only).
- Stop flag (Ctrl+Alt+G / ghost_stop) checked every poll interval during waits — a
  runaway command wait is interruptible; the shell process is killed on stop.
- Timeout on `run` kills the spawned process (direct child; grandchildren of GUI
  launches like `Start-Process` are intentionally not reaped).
- Serial dispatch (COM-STA invariant) unchanged: a long shell command blocks other Ghost
  calls, same as ghost_wait today; ghost_stop preempts.

### Spawning a Claude Code session (documented recipe, not a dedicated tool)

`ghost_shell op=run cmd='Start-Process wt -ArgumentList "pwsh","-NoExit","-Command","claude"'`
then drive the new terminal like any window (ghost_see / ghost_act / ghost_key). Headless
`claude -p` loops stay banned on this machine (Defender storm history) — visible
terminal sessions only.

### Explicitly not building

- Dedicated file read/write tools — `ghost_shell` covers it; MCP clients that edit code
  (Claude Code) already have better file tools. YAGNI.
- ConPTY interactive TUI driving — TUIs are driven as windows via UIA/keys, which is
  Ghost's existing strength.
- Persistent cmd.exe / bash sessions — PowerShell covers Windows; revisit on demand.

## Alternatives considered

1. **Per-command fresh PowerShell only** — simplest, no state, ~200-400ms startup per
   call and no cwd/var persistence. Rejected as the only mode; kept as `run`.
2. **ConPTY pseudo-terminal sessions** — true interactivity (TUIs), heavy implementation
   (~1k lines, winpty semantics). Rejected: UIA already drives TUAs-as-windows; sentinel
   framing covers 95% of agent shell needs.
3. **Shell in ghost-mcp with own registry** — avoids touching GhostSession, but splits
   session state across layers and ghost_run flows couldn't share it cleanly. Rejected.
