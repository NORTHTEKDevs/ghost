# Ghost convergence round 2 - 2026-11-26

HEAD `1a8c5cc` (stages 1-9 applied), re-measured with the Stage 1 matrix plus
every probe added since. Raw output: `docs/benches/2026-11-26-final2/`. All
probes follow the safety rule: input only to windows the probe created, and a
`WindowFromPoint` check before every coordinate click.

## Baseline (5bb4cf2) vs now (1a8c5cc)

| Measurement | Baseline | Now |
|---|---|---|
| Clean release build, 214 crates | 72 s, 0 warnings | 64 s, 0 warnings |
| `cargo test --workspace` | 550 / 0 / 27 | **619 / 0 / 27**, two runs |
| clippy / doctor | 0 / 7 of 7 | 0 / 7 of 7 |
| `ghost verify` | 2 of 4 runs pass (false FAILs) | **3 of 3** (and every run in stages 4-9) |
| verify: tab screenshot claim / whole run | 1705-1795 ms / 8.0 s | **55-76 ms** / 4.8 s |
| Cold start (median) / `tools/list` | 18.7 ms / 54 tools, 32 KB | 18.8 ms / unchanged |
| see text, user / hidden (median) | 51.6 / 16.4 ms | 19.8 / 17.3 ms |
| see tree, user / hidden | 47.0 / 22.0 ms | 21.1 / 17.5 ms |
| find, user / hidden | 19.7 / 7.9 ms | 12.3 / 5.0 ms |
| act click verified (real clicks), user / hidden | **0/20 / 10/20**, 123 / 113 ms | **20/20 / 20/20**, 42.6 / 38.8 ms |
| act click, effect only in a distant label, user / hidden | not measurable | 20/20 / 20/20 |
| act click on a disabled control, user | `ok:true` | refused 20/20 (same error as hidden) |
| no-effect clicks reported verified, user / hidden (label + caret) | - | 0/40 / 0/40 (and 0/200 each in stage 8) |
| act type, user / hidden | 7.8 / 7.1 ms | 4.4 / 4.7 ms |
| double_click verified, user / hidden | 0/20 / 5/20 | 20/20 / 20/20 (stage 6, re-checked hidden here) |
| headless tab screenshot, median / first | 1007 / 1807 ms | **31.6 / 42.4 ms**, page correct 20/20 |
| ghost_key with a same-title decoy in front: key in the target | 0/20 | **20/20**, decoy 0 |
| background combos typed as their bare letter | 40/40 | **0/40** (Ctrl+A as select_all, others refused) |
| see text / tree from the target with a decoy in front | 0/10 / 0/10 | **10/10 / 10/10** |
| click_at on a point covered by another process | clicks landed in the cover | **refused 20/20**, 0 into the cover |
| pinned click_at: lands once / median | via UIA fallback, 337-360 ms | **20/20, 24.4 ms**, no fallback, foreground unchanged |
| hidden-desktop click_at lands | **0/20** | **20/20** (Increment and Note) |
| mode=delta reports a real change | **0/10** | **10/10**; no change empty 10/10; decoy change not reported 10/10 |
| unanchored verb after the anchor closed | silently retargeted the foreground | refused, naming it, 20/20 |

## Ranked candidates

### R2-1 - ghost_key waits out two back-to-back foreground-guard windows (latency)

**Evidence.** `ghost_key` with one background key: median **190.6 ms**
(n=20, 188-193; `final2/key-routing/key_routing.json`); a background edit
command 126.7 ms. `key_background` (`crates/ghost-session/src/session.rs`)
ends with `foreground_guard_end_within(..., settle 60)`, and the MCP wrapper
then runs the same guard again with `settle 100` for `ghost_key`
(`crates/ghost-mcp/src/main.rs`, the post-call guard). The second window begins
the moment the first ends and watches for the same theft.

**Change.** Drop the inner 60 ms settle (keep the hand-back if a theft is
already visible); the outer 100 ms watch is unchanged.

**Payoff.** About 60 ms per key (~30%), no change in what is watched.
Acceptance: median <= 135 ms with the C2 routing probe still 20/20, and a
unit/probe check that a theft inside the window is still handed back. Going
below ~100 ms would need key-effect verification (like click_at), which keys do
not have - not proposed.

### R2-2 - the intermittent ghost-core test-binary exit: make it identify itself (diagnostic only)

**Evidence.** One abnormal exit (code 1, no failed test) of the `ghost_core`
lib test binary in about 40 runs this session (`stage-8/c6-test-run1.txt`);
0 in 5 targeted re-runs, 0 in the reviewer's 3 full runs, 0 in the 4 runs
since. Lead (unproven): `process::kill` ends a process with `TerminateProcess(..,
1)` (`process/manager.rs:190`), the only exit-code-1 path in ghost-core, and the
orphan-sweep code and test kill processes found through `parent_pid` links,
which Windows pid reuse can make stale; no path to the test binary itself was
found.

**Change.** Use a distinctive exit code in `kill` (e.g. 0xDEAD) so a recurrence
says whether it came from there. No behaviour change.

**Payoff.** Diagnostic only; rank low.

### R2-3 - the published benchmark cannot run on a shared desktop (measurement tooling, not Ghost)

**Evidence.** `bench/run_bench.py` and `bench/soak.py` set `GHOST_FOCUS_LOCK=off`
and raise Calculator onto the live desktop with `op=focus`; they were not run in
stage 1 or here for that reason. Their last published numbers are from
2026-08-23.

**Change.** Port both suites to the testbed on Ghost's hidden desktop (as every
probe in stages 2-9 does), so the README's benchmark and soak figures can be
regenerated safely.

**Payoff.** Lets the published numbers be re-measured; no runtime gain.

## Considered, no measurable payoff

- **`tools/list` is 32 KB (54 tools).** Owner's product decision; no measured
  agent failure.
- **No-effect clicks are slower than effective ones** (pinned click_at 231 ms,
  act click ~123 ms): concluding "no effect" needs the full 80 ms window plus the
  guard's settle; not a defect.
- **C5** (0/200 per surface) and **C8** (at most ~9 ms mean from a
  high-resolution timer): measured in stage 8, nothing worth a change.
- **Linux/macOS compile of the stage 5(b) edits**: cannot be cross-checked here
  (`ring` needs a C cross-compiler); CI's Linux job is the check.

Honest bottom line: the remaining candidates are one modest latency win (R2-1),
one diagnostic (R2-2) and one tooling port (R2-3). No accuracy or safety defect
with measured evidence remains open from rounds 1-2.
