# Stage 4 - verify attribution, bench testbed path, disabled-control refusal

Items 3 and 4 of `docs/plans/2026-11-25-ghost-improvements.md`, committed
separately. All builds in `target-improve`; live binaries untouched.

## Item 3 - `ghost verify` attributes foreground changes; run_bench finds its testbed

**Change.** `crates/ghost-cli/src/verify.rs`: the last claim compared the
foreground handle before and after the run, and failed 2 of 4 baseline runs on a
desktop where a human and another agent were switching windows. It now reads
both servers' interference audits (`ghost_stats.interference_audit`, incidents
since the run started) and decides with a pure function, `foreground_verdict`:

- a synthetic foreground change with a Ghost tool call in flight FAILS, even if
  the foreground is back where it started by the end;
- a change to a window owned by a process this run launched (either server,
  either browser) FAILS, even with nothing in flight;
- a change with real input behind it, or a synthetic change by another program,
  PASSES with a note saying so;
- with no audit (disabled, or a server that did not answer), only an unchanged
  handle passes, as before.

The second server's calls are invisible to the first server's audit, so its own
audit is read before it is closed. The claim is renamed from "foreground window
never changed" to "foreground never taken by Ghost", which is what it now proves.

`bench/run_bench.py`: the testbed is resolved next to `--exe` instead of
`target/release`, so benching a side build no longer drives the live build's
testbed.

**Residual.** Attribution is as good as the audit's: a Ghost-caused change the
sampler sees only after the call has returned, to a window Ghost did not launch,
would read as another program's. The launched-pid rule covers the case where it
mattered before (a launched browser activating itself after launch returned).

| Criterion | Target | Measured |
|---|---|---|
| Unit: unchanged, nothing recorded | PASS | pass |
| Unit: human change (no incident) | PASS with note | pass |
| Unit: synthetic change by another program | PASS with note | pass |
| Unit: Ghost tool in flight, foreground back where it started | FAIL | pass (FAILS, names the tool) |
| Unit: launched process's window took it, nothing in flight | FAIL | pass |
| Unit: no audit -> handle compare only; unreadable start never passes | as stated | pass |
| Unit: incidents before the run ignored; audit off -> None | as stated | pass |
| 8 new tests red on a stub, green on the implementation | - | 5 failed on stub, 8/8 green |
| `ghost verify` x10 consecutive, `GHOST_MCP_PATH` = target-improve | 10/10 exit 0 (baseline 2/4) | **10/10** (`item3-verify-run*.txt`) |
| run_bench hardcoded path gone | grep finds nothing | grep exit 1 |
| run_bench testbed resolves from `--exe` | value_equals_assert passes against target-improve | PASS, testbed `target-improve\release\ghost-testbed.exe` (`run-bench-testbed-path.txt`) |
| `cargo test --workspace` | 0 failed | 575 / 0 / 27, both runs |
| clippy, vibecode lint on changed files | 0 / 0 HIGH | 0 / 0 findings |

The foreground did not change during any of the 10 runs, so they show the false
FAIL is gone on an unchanged desktop but do not exercise attribution live; the
unit tests do. No verdict carried "(interference audit off...)", so both audits
were on and read in all 10 runs.
