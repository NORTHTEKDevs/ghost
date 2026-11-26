# Stage 10 - R2-1 key latency, R2-2 kill exit code (2026-11-26)

Binaries built with `CARGO_TARGET_DIR=target-improve`. The probe launches its
own two testbeds (`SW_SHOWNOACTIVATE`) and sends input only to them.

## R2-1 - ghost_key waited two back-to-back foreground watches

`key_background` watched 60 ms for the target taking the foreground, then the
MCP post-call guard watched 100 ms for the same thing. With a resolved handle
and the background policy the guard always runs, so the key path now passes
`settle_ms = 0`: it hands back a theft already visible and leaves the watching
to the guard. `background=true` under the foreground policy has no post-call
guard and keeps 60 ms.

| Acceptance | Before (final2) | After |
|---|---|---|
| ghost_key background median, n=20 | 190.6 ms | **126.6 ms** (125.3-132.1) |
| key in target / decoy (decoy in front) | 20/20 / 0 | **20/20 / 0** |
| Ctrl+A response names target | 20/20 | 20/20 |
| focus_preserved | - | 20/20 |
| theft after the key returns is still handed back | - | unit tests `theft_watch_tests` (3) |

The theft evidence is a unit test, not a live probe: provoking a real
foreground theft on this shared desktop would take the human's foreground,
which the probe-safety rule forbids. The tests pin the composition: a theft
visible at once is caught by the key path's 0 ms check, one 40 ms later by the
following 100 ms guard, and a 0 ms check does not wait.

Files: `key_routing_probe.py` (stage-10 copy, adds the latency and guard
lines), `key-routing.txt`, `key-routing/key_routing.json`, `r2-1-*.txt`
(cargo test 622/0/27 x2, clippy 0, lint 0).

## R2-2 - process::kill exit code

`kill` now ends a process with `KILLED_EXIT_CODE` = 0xDEAD (57005) instead of 1,
so a recurrence of the once-in-~40-runs ghost-core test-binary exit says
whether Ghost's kill caused it. Nothing reads the value: `ghost_shell`'s
`exit_code` comes from std `Child::wait`, `desktop_close` reports only the pids
it ended, and no script under `bench/` or `docs/benches/` checks it. Test
`a_killed_process_exits_with_the_distinctive_code` spawns `ping`, kills it and
reads 57005.

Files: `r2-2-*.txt` (cargo test 623/0/27 x2, clippy 0, lint 0).
