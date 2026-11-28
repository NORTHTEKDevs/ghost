# Ghost improvement audit and ranked plan (2026-11-27)

**Baseline:** main at e40a7aa. `cargo test -j 2 --workspace` on Windows:
839 passed, 0 failed, 28 ignored (46 test binaries). CI and Linux green on
e40a7aa's parent; CI on e40a7aa was in progress at audit time.

**Method:** read-only audit in six areas (tool coverage, reliability,
test coverage, performance, security, docs/listings). Every evidence line was
read in the current tree; items fixed in 0.24.4-0.24.6 are not repeated.
Public listing pages were read without signing in.

**Scoring:** impact H/M/L, effort S/M/L, risk = chance the fix itself breaks
something. "Contract" = changes a tool schema, argument, result shape or a
default; those need Kristian. "Behaviour" = same contract, different outcome
in some case; those ship as open PRs, not self-merged.

## Ranked list

Ordered by impact over effort, then risk. Items 1-9 are in scope for this
run; the rest are listed with what blocks them.

| # | Item | Impact | Effort | Risk | Kind | Evidence |
|---|------|--------|--------|------|------|----------|
| 1 | STA pool `orphaned_jobs` wraps to ~usize::MAX when a caller is cancelled: the worker decrements on a failed reply send, but only the timeout branch increments | M | S | L | internal | `crates/ghost-core/src/uia/sta_pool.rs:83,94` vs `:152` |
| ~~2~~ | Moved to "Needs Kristian": `ghost_http_get` is classed Observe, so it still runs after the human presses stop; the class doc says outbound requests are Act | M | S | L | behaviour | `crates/ghost-mcp/src/main.rs:914-915` vs `:945` |
| 3 | `ghost_assert value-equals/value-contains` turns a failed value read (dead tab, CDP timeout) into `""`, so `value-equals text=""` passes falsely | M | S | L | behaviour | `crates/ghost-mcp/src/main.rs:1387` |
| ~~4~~ | Moved to "Needs Kristian": ghost-http waits for the session thread's reply with no deadline; one stuck UIA call hangs every later request, and the queue is unbounded | H (http) | S | L | behaviour (new 504) | `crates/ghost-http/src/main.rs:175-178`, `:68` |
| 5 | std `Mutex` + `.unwrap()` on shared state: one panic while locked poisons it and every later CDP call / locate call panics until restart. Demoted on a closer read: the code under these locks (HashMap insert/remove, the reflection recorders) cannot realistically panic | L | S | L | internal | `crates/ghost-browser/src/cdp.rs:186,191,196,210,269`; `crates/ghost-session/src/session.rs:988,1032,1046,1050,1251,1276` |
| 6 | `ghost_shell` reads the child's whole stdout/stderr into memory before capping at 24k; `type bigfile` can exhaust memory | M | S | L | internal | `crates/ghost-session/src/shell.rs:1433-1437`, cap at `:1471` |
| 7 | Flaky test: `shell_disabled_reads_env` / `warm_flag_reads_env` set process-global env vars while parallel tests read them | M | S | L | test-only | `crates/ghost-session/src/shell.rs:2108-2112`, `:2372-2375`; readers `:344`, `:1365`, `:1404` |
| 8 | No guard that `server.json`, `plugin.json`, `lhm.plugin.json` versions match Cargo, or that the registry description stays within 100 chars (it is at exactly 100) | M | S | L | test-only | `server.json`, `Cargo.toml:23` |
| 9 | Untested pure helpers that guard agent input: `parse_index` (7 call sites, its doc says a silent fallback is the bug it prevents), ext.rs `req_str`/`browser_tab`/`str_array`, `panic_message`, ghost-cli `base64_encode` | M | S | L | test-only | `crates/ghost-mcp/src/main.rs:4570,773`; `crates/ghost-mcp/src/ext.rs:25,555-600`; `crates/ghost-cli/src/main.rs:683` |
| 10 | Hidden-desktop `Desktop::exec` blocks on `rx.recv()` with no timeout from async fns, and the worker loop has no `catch_unwind`: one hung job pins a tokio worker; one panic kills the desktop | H | M | M | internal | `crates/ghost-core/src/desktop.rs:128-130,151-164,170` |
| 11 | `desktops` async mutex held across the blocking `windows()` call: one wedged hidden desktop stalls every target resolution | H | S | M | internal (after 10) | `crates/ghost-session/src/target.rs:268-273` |
| 12 | Sync UIA walks and `PrintWindow` run inline on tokio workers; the per-call deadline cannot fire while a worker is blocked, so N hung calls freeze the runtime (the 1800s client hang pattern) | H | M-L | M | internal | `crates/ghost-session/src/session.rs:1380,1421,1929-1931`; `crates/ghost-core/src/capture/screen.rs:339` |
| 13 | Flaky timing asserts (a 40 ms theft timer, <200/400 ms elapsed checks) on a machine known to stall | M | S-M | L | test-only | `crates/ghost-session/src/session.rs:3562,3569,3964`; `shell.rs:2706` |
| 14 | Docs overclaim: "Web requests cannot reach your network" (tabs may load any http(s) host); "a pipe or a script is refused" for `ghost reset` (a pty passes `is_terminal`) | M | S | L | docs | `docs/security-model.md:56,101`; `crates/ghost-browser/src/policy.rs:3-6`; `crates/ghost-cli/src/main.rs:295` |
| 15 | Docs say nothing about elevated (UAC) windows, multi-monitor or DPI | M | S | L | docs | grep over `docs/` finds no elevat/UAC/DPI/monitor |
| 16 | Unbounded small state: `cdp_route` bindings, closed tabs kept in the registry until browser close, no cap on in-flight MCP calls | L | S | L | internal | `crates/ghost-mcp/src/cdp_route.rs:35-40`; `crates/ghost-session/src/registries.rs:355-365`; `crates/ghost-mcp/src/main.rs:612` |
| 17 | Ctrl+Alt+G already registered by another app: retried silently, never reported on Windows | L-M | S | L | internal (log) / contract (stats field) | `crates/ghost-core/src/hotkey.rs:358-378`; `doctor.rs:344` is X11-only |
| 18 | ghost-http `/launch` ignores the background focus policy (MCP routes launches to a hidden desktop) | L | S | L | behaviour | `crates/ghost-http/src/main.rs:117-118` vs `crates/ghost-mcp/src/main.rs:2299-2302` |
| 19 | Clipboard restore after a paste fallback returns `""` for non-text clipboards, so it may wipe an image/file clipboard (suspected, not traced); pasted text is not excluded from clipboard history | L | S | M | behaviour | `crates/ghost-core/src/system/clipboard.rs:40`; `crates/ghost-session/src/session.rs:1711-1722` |

## Needs Kristian (contract, default or design decision)

| Item | Why it needs a decision | Evidence |
|------|-------------------------|----------|
| MCP tool annotations (`readOnlyHint`, `destructiveHint`, `openWorldHint`), `title`, `outputSchema`/`structuredContent`, protocol version 2024-11-05 | Changes `tools/list`; some clients auto-approve on `readOnlyHint`, so the labels are a security call. The Observe/Act/Control table at `main.rs:926` is a ready source for `readOnlyHint`. Glama scores "Disambiguation 3/5" partly on this | `crates/ghost-mcp/src/main.rs:2011,4210`; no `annotations` anywhere in `crates/` |
| Mark untrusted screen/page/clipboard text and add server `instructions` saying it is data | Changes result shapes; highest-impact prompt-injection defence while `ghost_shell` is on by default | `crates/ghost-mcp/src/ext.rs:203-220`; `main.rs:2013` |
| ghost-http authentication (per-start token file or named pipe with a DACL) | Changes how every HTTP client connects; today any local process that sends `Host: 127.0.0.1` drives the desktop | `crates/ghost-http/src/main.rs:389-406` |
| Human stop event is `EVENT_ALL_ACCESS` for the user, so a process outside the shell job (`ghost_window op=launch`, WMI) can reset it | Needs a design: hotkey owner keeps the stop in memory and re-raises on tamper | `crates/ghost-core/src/hotkey.rs:168-179,256-286`; `crates/ghost-session/src/hidden.rs:324-326`; `docs/security-model.md:80` |
| Tab navigation to link-local/private hosts (e.g. 169.254.169.254) | Blocking it changes what tabs may load (local dev servers are a real use) | `crates/ghost-browser/src/policy.rs:3-25` |
| Page scripts keep running after a stop | Needs a stop hook into Ghost-owned tabs (`Runtime.terminateExecution` / about:blank) and a stance on attached browsers | `crates/ghost-browser/src/tab.rs:171-174` |
| `ghost reset` CLI resume: keep it or rely on the physical Ctrl+Alt+Shift+G only | Removing a resume path | `crates/ghost-cli/src/main.rs:295-307` |
| Tool surface: overlapping inspect/click tools, inconsistent `window`/`name`/`hwnd`, 18 descriptions over 300 chars, empty param descriptions, `ghost_snapshot` vs `ghost_see` merge | Descriptions and names are the contract agents read | `docs/tools.md` (`ghost_shell` 1061 chars, `ghost_run` 1040, `ghost_act` 883) |
| Window move/resize (`ghost_window op=move\|resize`) | New op | `docs/tools.md:198` |
| `ghost_clipboard get` opt-out env var; allowed while stopped | New default/knob | `crates/ghost-mcp/src/main.rs:959` |
| `ghost_http_get` while stopped (plan item 2) | The Observe class is deliberate (pinned by the test comment at `main.rs:4808`); making it Act also turns off its default `ghost_run` retry (`main.rs:3721`) | `crates/ghost-mcp/src/main.rs:914-915,945` |
| ghost-http reply deadline (plan item 4) | `/run` carries its own `max_duration_ms`, and the single serial worker keeps a stuck call at the head of the queue, so the deadline length, the 504 contract and whether queued work is cancelled are design choices | `crates/ghost-http/src/main.rs:68-82,175-178` |
| ghost-macos never runs in CI (33 unit + 9 integration tests) | macOS runner minutes cost ~10x | `crates/ghost-macos/src/lib.rs`; `.github/workflows/*.yml` |
| LobeHub shows grade F and a stale README; Glama scored 3.2/5 on the 39-tool Linux build | External listing actions are out of scope for this run | lobehub.com/mcp/northtekdevs-ghost; glama.ai/mcp/servers/NORTHTEKDevs/ghost |

## Status after the 2026-11-27 run

| Item | PR | State |
|------|----|-------|
| Plan | #29 | merged (5e00e99) |
| 1 STA orphan count | #30 | merged (406bd89) |
| 3 `ghost_assert` read failure | #31 | open: behaviour change, for Kristian |
| 6 shell output tail | #32 | merged (4ed72cb) |
| 7 shell env test race | #33 | test-only |
| 8 manifest version guard | #34 | test-only |
| 9 argument helper tests | #35 | test-only; the audit's claim that ghost-mcp's base64 encoders were untested was wrong, so only ghost-cli's got one |
| 2, 4 | - | need a decision (above) |
| 5 | - | demoted to L |
| 10-19 | - | not started; 10 and 11 (hidden-desktop deadline, desktops lock) are the next highest value |

Follow-up found while doing item 6: the `ghost_shell op=open/read` session
reader also accumulates a command's whole output until its sentinel line.

## Notes

- Sound on inspection, not re-reported: ghost-http binds loopback only and
  checks Host/Origin; every tool passes the stop gate (`main.rs:1028`); an
  agent's `ghost_reset` cannot lift a human stop; the `ghost_http_*` SSRF
  filter (DNS-filtered, no redirects, IPv6-embedded IPv4); per-call panic and
  deadline guard in ghost-mcp (`main.rs:731-771`); CDP calls time out; shell
  has a 600 s cap and a stop job; typed text logs only at debug.
- The worktree `projects/worktrees/ghost-v0.3.0` has a corrupt index
  (`fatal: index file corrupt`); left untouched.
- Constraints for the implementation run: one branch and PR per item, test
  first, `cargo -j 2` into `target-improve`, no live desktop input, no
  clipboard writes, no machine-wide stop, no releases.
