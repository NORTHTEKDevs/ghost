# GHOST COMPLETION LOOP - STATE GRAPH
<!-- Machine-resumable work graph. Any session (human or agent) picks the first
     READY node, executes it, verifies with real command output, updates status,
     repeats until DONE. Never mark a node done without fresh executed evidence.
     Gate command for every node: powershell -File scripts\bshr-loop.ps1 -->

## STATUS: DONE (2026-08-23) - all exit criteria hold, certified inline 10/10

## EXIT CRITERIA (all hold, evidence below)
- [x] bench 14/14 (`python bench/run_bench.py` exit 0) - twice consecutively
- [x] bench self-test (`--self-test` exit 0) - 3/3 negative controls caught
- [x] soak PASS (`python bench/soak.py` exit 0) - effect-mismatch 0, p50 ~148ms
- [x] bshr-loop.ps1 ALL GATES GREEN (clippy/tests/release/doctor/verify/live-contract)
- [x] production-cert: inline cert 10/10 (subagent infra returned empty twice;
      all evidence gathered fresh in-session instead)
- [x] README claims each verified by an executed command (table below)

## GRAPH (final state)

### N1 [done] Isolated-desktop typing gap
UIA ValuePattern fallback + rescue of partial message-path failures. Works on
both Win11 Store Notepad AND classic notepad.exe on isolated desktops.

### N2 [done] Background-policy routing regression
Anchored find/act/key/click_at route to background machinery under default
policy. New: find_background (window+element retry, index disambiguation,
hwnd in response), click_at_background (posted messages -> UIA Invoke fallback,
occlusion-safe via hwnd param).

### N3 [done] Bench to 14/14
Root causes fixed: anchor-focus regression (product), index count cap bug
(product), launch/tree-population races (product retries), occluded-window
coordinate clicks (hwnd chaining find->click_at), stale bench assertions
updated to 0.19 documented contracts (background response fields; policy-raise
pattern for foreground-only steps; multi-window Calculator reality).

### N4 [done] Self-test + soak
Self-test 3/3 caught; soak PASS with 0% effect mismatch.

### N5 [done] Full gate + regression sweep
ALL GATES GREEN; browser e2e 9/9; desktop e2e 12/12; HTTP endpoints verified
(/health /list-windows /tools /launch /screenshot); GHOST_SHELL=off refusal
verified (-32002, GUI verbs unaffected).

### N6 [done] Certify + memory write-back
Cert written to memory/cert_crystal_ghost.md; loop history updated.

## README CLAIM VERIFICATION TABLE (all executed evidence)
| Claim | Verified by | Status |
|---|---|---|
| 54 tools on Windows | tools/list count | PASS |
| ghost verify audits claims | ghost verify 10/10 | PASS |
| doctor exits non-zero on FAIL | doctor output | PASS |
| background enforced, errors name action+policy | manual JSON-RPC | PASS |
| anchored find/act/key work without focus | bench 14/14 | PASS |
| every action verified, never blind ok | act_verified task + soak | PASS |
| bench 14/14 | run_bench.py x2 | PASS |
| harness can fail (--self-test) | 3/3 controls caught | PASS |
| soak PASS | soak.py | PASS |
| HTTP server endpoints | curl /health /tools /launch /screenshot | PASS |
| GHOST_SHELL=off refuses, GUI verbs fine | env test | PASS |
| Ctrl+Alt+G session-wide stop | ghost verify | PASS |
| isolated desktops invisible + drivable | desktop e2e 12/12 | PASS |
| browser/tab background driving | browser e2e 9/9 | PASS |

## RULES (unchanged for future runs)
1. One node at a time. Evidence before status change.
2. Product wrong -> fix product. Bench asserting stale semantics -> fix bench
   AND note it here. Never weaken a verification to make it pass.
3. If stuck >2 attempts on one failure: stop, write findings, report honestly.
