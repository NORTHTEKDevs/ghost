# E2E test plan: the published Ghost 0.24.3

Target: `ghost-windows-x64.zip` from the v0.24.3 GitHub release, extracted to a
temp dir. Everything below drives THAT `ghost-mcp.exe` over stdio JSON-RPC
(`e2e.mjs`), not the installed MCP. The only window it touches is a
`ghost-testbed` Ghost launches on its own hidden desktop
(`target-improve/release/ghost-testbed.exe`, the zip does not ship one) and a
headless browser Ghost launches.

## Safety rails (checked, not assumed)

- `sampler.ps1` runs for the whole session: every 50 ms it reads (never sets,
  never creates) `Local\ghost-emergency-stop-event` and
  `GetClipboardSequenceNumber()`. Pass: 0 samples signalled, clipboard sequence
  unchanged from first to last sample.
- Stops only as `ghost_stop scope=process`; no `ghost_stop` without scope, no
  `ghost_reset` without scope, no Ctrl+Alt+G.
- `GHOST_FOCUS_POLICY=background`, `GHOST_FOCUS_LOCK=on`: no input reaches the
  user's desktop.
- Only processes this run started are ended, by PID; no `taskkill /T`.

## Areas and pass criteria

| # | Area | Exact checks | Pass |
|---|---|---|---|
| A | Download + integrity | sha256 of the zip = `8d714c58...6ebde5` and = the release's `.sha256` asset; unzip; `--version` of ghost, ghost-mcp, ghost-http | all three say 0.24.3, both hashes match |
| B | Protocol | initialize, tools/list, `ping` -> `{}`, unknown method -> -32601, unknown tool / unknown arg key -> invalid params | as stated |
| C | Screen/window reads | launch testbed (hidden desktop, anchored); `ghost_see` (default + mode=text), `ghost_snapshot`, `ghost_find name=Increment`, `ghost_see mode=bogus` refused | reads name the testbed's controls; bogus mode is invalid params |
| D | Type + click with read-back | `ghost_act type` "e2e-0.24.3" into Field, read back via `ghost_see`/`ghost_find`; `ghost_act click` Increment -> title gains `[clicks=1]`; Count 0 -> Count 1; click Locked -> refused / no effect | read-back equals the typed text; title/caption change observed; Locked no effect |
| E | Shell (GHOST_SHELL on, second server) | one-shot `echo`; failing PowerShell -> exit 1; missing cwd refused; persistent session start/send/read/list/kill | outputs as stated |
| F | Job objects | persistent shell starts a child (`ping -n 60 127.0.0.1`), record child PID; `op=kill` / `ghost_stop scope=process` -> child PID gone | child gone within 3 s |
| G | GHOST_SHELL=off | `ghost_shell` refused; `ghost_window op=launch exe=cmd.exe` / `powershell.exe` / 8.3 name of powershell refused | each refused with a shell-off error |
| H | Browser | `ghost_browser_launch mode=headless`; tab open about:blank, navigate `data:`/`file:` refused, navigate `https://example.com`, eval `document.title`, text, close | eval/text show "Example Domain"; file: refused |
| I | SSRF (`ghost_http_get`) | 127.0.0.1, localhost, [::1], [::ffff:127.0.0.1], [0:0:0:0:0:0:0:1], 10.0.0.1, 172.16.0.1, 192.168.1.1, 169.254.169.254, 100.64.0.1, 0.0.0.0, 198.18.0.1, fec0::1, [64:ff9b::7f00:1], [2002:7f00:1::], [2001:0:...] Teredo, a name resolving to 127.0.0.1 (`localtest.me`), decimal `2130706433`, caller `Host` header; control: `https://example.com` ok | every private form refused, control ok |
| J | Close tools on Ghost-owned things | `ghost_tab_close`, `ghost_browser_close`, `ghost_desktop_close` (reports terminated testbed pid) | each ok; pids gone |
| K | Stop/resume (process scope) | `ghost_stop scope=process` -> act refused -32004; owned tab close allowed; `ghost_reset scope=process` -> act works | as stated, real event never signalled |
| L | CLI | `ghost verify` (extracted ghost.exe); `ghost reset` with piped stdin exits 2 | verify all claims PASS (or each FAIL explained); exit 2 |
| M | ghost-http | start on 127.0.0.1:<free port>; GET with foreign Host -> 403; with Origin -> 403; `--addr 0.0.0.0:...` refused | as stated |
| N | Unit/integration | not re-run here (0.24.3 release CI covered them); run only if a defect needs a fix | n/a |

Dropped after review: `ghost_stop`/`ghost_reset` with an unknown scope. If that
fix had regressed, the call would raise (or lift) the real machine-wide stop,
which the hard rules forbid; it stays covered by the unit tests only.

## On failure

`systematic-debugging-bshr`; for a real bug: failing test first, fix, code
review, `code-vibecode-lint` 0 HIGH, cargo test + clippy
(`CARGO_TARGET_DIR=target-improve`), conventional commit, push to main. No
0.24.4, no tag.
