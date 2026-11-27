# E2E report: the 0.24.4 candidate

This is the same harness as `../2026-11-26-e2e-0.24.3/`, run against a release build of main
at e8895ae (`target-improve/release`, not installed anywhere). It drives `ghost-mcp` over
stdio, and touches only a `ghost-testbed` Ghost launched on its own hidden desktop and
headless browsers that Ghost launched.

**Result: 141 of 141 steps pass.** `ghost verify` passes 10 of 10 claims. The real stop event
was never signalled.

## Safety record

| Check | Result |
|---|---|
| Real `Local\ghost-emergency-stop-event`, sampled every 50 ms from 23:25:15 to 23:39:06. This window covers both `cargo test --workspace` runs and every run below. | 13243 samples, **0 signalled** |
| Clipboard (`GetClipboardSequenceNumber`, read-only) | 5 -> 5, **0 changes** |
| Stops | `scope:"process"` only |
| Processes ended by hand | only by PID: the ping the harness's own finished one-shot started (37764) |

The `ghost_window op=list` lines keep only the anchor. The titles of the human's
own windows were read, never acted on, and are redacted.

## Results

| Scenario | Result | Notes |
|---|---|---|
| `s1-core` (protocol, reads, type/click read-back, `GHOST_SHELL=off`, browser, 28 SSRF forms, stop/reset, closes) | 76/0 | Five expectations that were wrong in the 0.24.3 run are corrected here, each to the documented behavior. Unknown argument keys are ignored. The typed text is read back with `ghost_assert value-equals`. example.com has no `<h1>`. A huge wait timeout is clamped. |
| `s1b-followup` | 24/0 | |
| `s2-shell` | 28/0 | **New in 0.24.4.** After `op=kill`, the session's child ping 7856 is gone. When an `op=run` times out on the warm spare, both the shell and its child (22744) end. After a process-scoped stop, child 34264 is gone. **Unchanged:** a finished one-shot's child keeps running (37764, ended here by PID). |
| `s3-tabopen` | 13/0 | **New in 0.24.4.** Right after `ghost_tab_open`, the eval reads `https://example.com/ complete 917`, and the response has `loaded:true` with the landed URL. With `wait:false` the open answers at once. An unloadable page (`net::ERR_UNSAFE_PORT`) and `timeout_ms:1` are errors, and the tab is closed. `wait:"no"` and `timeout_ms:0` return -32602. `file:` is refused. |
| `cli.txt` | pass | `ghost-mcp --version` / `-V` print `ghost-mcp 0.24.3` (the pre-bump build) and exit 0. `--help` prints its usage. `ghost verify`: `ALL CLAIMS HOLD: 10 checks passed`. A piped `ghost reset` exits 2. |
| `http.txt` | pass | `/health` 200; a foreign Host gets 403; an Origin header gets 403; a text/plain `/run` gets 415; `--addr 0.0.0.0` exits 2. |

## Harness fix

`e2e.mjs` now substitutes `{{var}}` in its `contains`/`absent` checks too. Before this fix, the
0.24.3 run's `absent: "{{tab}}"` check after `ghost_tab_close` passed without testing
anything. Checked by hand, that claim still holds: the closed tab `31AA34EF...` is missing
from the listing that follows it in `../2026-11-26-e2e-0.24.3/s1-core.txt`.
