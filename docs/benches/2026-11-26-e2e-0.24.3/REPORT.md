# E2E report: the published Ghost 0.24.3

Plan: `PLAN.md`. Driver: `e2e.mjs` (stdio JSON-RPC, one PASS/FAIL per step);
scenarios `s*.json`, transcripts `s*.txt`, `cli*.txt`, `http.txt`.

**Verdict: one real defect, fixed and pushed (69ebddf).** The published
`ghost verify` failed 3 of 8 claims against the published server because it
opened its test page from a `file:` URL, which 0.24.3 now refuses in tabs.
Everything else passed, or behaved as designed once the contract was checked.
No 0.24.4 and no tag: the fixed `ghost verify` reaches users only in a release.

## Safety record

| Check | Result |
|---|---|
| Real `Local\ghost-emergency-stop-event`, sampled every 50 ms, 22:20:54-22:29:44 (covers every run) | 8476 samples, **0 signalled**, never absent |
| Clipboard (`GetClipboardSequenceNumber`, read-only) | 5 -> 5, **0 changes** |
| Stops raised | `scope:"process"` only (and `ghost verify`, which also uses `scope:"process"`) |
| Windows touched | only `ghost-testbed` windows Ghost launched on its own hidden desktop, plus Ghost-launched headless browsers |
| Processes ended | only by PID, all started by this run; all testbed and browser PIDs gone afterwards (`tasklist` "No tasks") |
| Installed binaries | untouched; the fixed `ghost.exe` ran from a temp dir next to a copy of the published `ghost-mcp.exe` |

`ghost_window op=list` lines in `s1-core.txt` keep only the anchor (the
testbed). The titles of the human's own windows, which were read but never
acted on, are redacted.

Dropped from the plan: `ghost_stop`/`ghost_reset` with an unknown scope. A
regression there would raise the real machine-wide stop. The unit tests still
cover it.

## Results by area

| # | Area | Result | Evidence |
|---|---|---|---|
| A | Download + integrity | PASS | zip sha256 `8d714c58...6ebde5` = given value = `.sha256` asset; `ghost 0.24.3`, `ghost-http 0.24.3`; `ghost-mcp` has no `--version` flag (it starts serving and logs `version="0.24.3"`) |
| B | Protocol | PASS | `ping` -> `{"result":{}}`; unknown method -> -32601; unknown tool -> error |
| C | Screen/window reads | PASS | launch -> `started on a hidden desktop ... anchored`; `ghost_see`, `mode=text`, `ghost_snapshot`, `ghost_find` all name the testbed's controls; `mode=bogus` -> -32602 |
| D | Type + click with read-back | PASS | `ghost_act type` -> `verified:true`; `ghost_assert value-equals` reads back `"e2e-0.24.3 typed"`; a wrong value -> `assert failed ... actual="e2e-0.24.3 typed"`; retyping gives `"second"` (not `"secondsecond"`); Increment -> title `[clicks=1]`; `Count 0` -> `Count 1`; Locked -> refused, title unchanged; typing into a button -> -32002 `not a text field`; `ghost_key NotAKey` -> -32602 |
| E | Shell, one-shot and persistent | PASS | output, `cmd` shell, failing PowerShell -> `exit_code:1`, missing cwd refused, `~` -> `C:\Users\<user>`, timeout -> `timed_out:true`; session open/send (`41+1` -> `42`)/list/read/kill |
| F | Job objects | PASS (contract) | `ghost_stop scope:"process"` ended the session's child `ping` (28880 gone) and the shell reports `ended by an emergency stop (since lifted)`. `op=kill` and one-shot completion leave a command's child running: **by design on Windows** (`shell.rs:504`: "the job is not kill-on-close ... only a stop ends it"; README: programs end with the stop). The `op=kill` ends-the-group behavior in the changelog is Linux-only |
| G | `GHOST_SHELL=off` | PASS | `ghost_shell` run/open refused; launches of `cmd.exe`, `powershell.exe -c 1`, the 8.3 name `POWERS~1.EXE`, `wscript.exe`, and `ghost_desktop_launch cmd.exe` all refused |
| H | Browser on a Ghost-launched headless browser | PASS | `file:` and `data:` navigation refused; `https://example.com` loads, eval `document.title` = `Example Domain`, text read, `max_chars:40` -> `truncated:true, original_length:917`; bad host -> `net::ERR_NAME_NOT_RESOLVED` |
| I | `ghost_http_get` SSRF | PASS, 28/28 refused | 127.0.0.1, localhost, 127.1, 2130706433, 0x7f000001, [::1], [0:..:1], [::ffff:127.0.0.1], [::127.0.0.1], 10/8, 172.16/12, 192.168/16, 169.254.169.254, 100.64/10, 0.0.0.0, 198.18/15, 224.0.0.1, fec0::, fd00::, fe80::, NAT64 (64:ff9b::7f00:1, ::a00:1), 6to4 (2002:7f00:1::), Teredo, DNS to loopback (localtest.me, 127.0.0.1.nip.io), caller `Host` header, `file:`. Control `https://example.com` -> ok |
| J | Close Ghost-owned things | PASS | `ghost_tab_close` (tab gone from the list), `ghost_browser_close`, `ghost_desktop_close` -> `terminated_pids`; testbed and browser PIDs gone |
| K | Stop/resume, `scope:"process"` | PASS | stopped: `ghost_act` and a window close -> -32004, `ghost_shell` -> -32004, an owned tab close -> ok; after `ghost_reset scope:"process"` the click works (`[clicks=2]`) |
| L | CLI | **FAIL -> fixed** | `ghost verify` (published): `3 of 8 claims FAILED` (tabs `1ms for all three`, second server, stop/reset). Root cause below. Fixed build against the published `ghost-mcp.exe`: `ALL CLAIMS HOLD: 10 checks passed`. `echo yes \| ghost reset` -> `refused - run it yourself in an interactive terminal`, exit 2 |
| M | ghost-http | PASS | `/health` 200; foreign Host -> 403; `Host: 127.0.0.1:1` -> 403; any Origin (incl. `null`) -> 403; text/plain `/run` -> 415; `--addr` 0.0.0.0, 192.168.1.10, `[::]` refused, exit 2. `Host: localhost:<port>` -> 200 is on the documented allow-list |

## The defect: `ghost verify` opened a `file:` page

`crates/ghost-cli/src/verify.rs` wrote its test page to `%TEMP%` and opened it
as `file:///...`. 0.24.3 limits `ghost_tab_open`/`ghost_tab_navigate` to
http(s) and about:blank (`ghost-browser/src/policy.rs` `check_nav_url`), so
all three tab opens were refused, and every claim built on a tab failed. No
test ran `verify` against the new policy, so the release shipped with it broken.

Fix (69ebddf): verify serves the page from a loopback listener on an
ephemeral port, one thread per connection, headers ended by CRLF or a bare LF.
Test `the_verify_page_is_served_at_a_url_tabs_may_open` checks the URL against
the real `check_nav_url` and that both request styles are answered in under
1 s with an idle connection held open. The code review found the first version
of that test sent bare LFs and passed only by waiting out the 2 s read timeout.
Both review findings were fixed before the commit. `cargo test -p ghost-cli`
49 + 1 pass, `cargo clippy -p ghost-cli --all-targets -D warnings` clean,
`code-vibecode-lint` 0 findings.

## Observations (not fixed; Kristian's call)

1. **`ghost_tab_open url=...` returns before the page loads.** Called right
   after the open, eval saw `about:blank complete 0`, and 2 s later
   `https://example.com/ complete 917` (`s1c-tabopen.txt`). `ghost_tab_navigate`
   does wait for the load. The tool description promises no wait, but an agent
   that opens and then reads gets an empty page. Either wait for the load as
   navigate does, or say "does not wait; follow with ghost_tab_navigate or
   ghost_tab_wait_for" in the description. (`ghost_tab_wait_for selector=body`
   does not help: about:blank already has a body.)
2. **`op=kill` differs by platform**, and the tool description does not say
   so. On Windows it ends the shell but not what its commands started (only a
   stop does); on Linux it ends the shell's process group. A line in the
   `ghost_shell` description would prevent the surprise.
3. `ghost-mcp --version` starts the server instead of printing a version.
4. Unknown JSON argument keys (`ghost_see bogus_key`) are ignored, and
   `ghost_wait timeout_ms` above 600000 is clamped, not refused. Both match the
   documented behavior; listed only because a probe might expect a refusal.

## Not covered

- Unit/integration suites were not re-run beyond `ghost-cli` (release CI ran
  them for 0.24.3).
- The human stop (Ctrl+Alt+G) and machine-wide stop/reset: excluded by the rules.
