# Ghost improvements - 2026-11-25

Goal: make Ghost measurably better on reliability, accuracy, latency, safety and
agent ergonomics. Every item below is backed by a number measured on this machine
today against a fresh build of `HEAD` (`5bb4cf2`, 0.23.4). Raw output lives in
`docs/benches/2026-11-25-baseline/`.

All builds for this work go to `CARGO_TARGET_DIR=target-improve`. `target/release`
and `~/.local/bin` serve live sessions and are never touched.

## Baseline

Machine: Windows 11 build 26300, one 2560x1440 monitor at 100%, rustc in
`toolchain.txt`. Other Claude sessions and a driver app were active throughout, so
the desktop was shared and not idle.

### Build and static gates

| Gate | Result | Wall | Raw |
|---|---|---|---|
| `cargo build --release --workspace` | exit 0, 0 warnings | 72 s (incremental; target-improve existed) | `build-release.txt` |
| `cargo test --workspace --no-run` | exit 0 | 55 s | `test-build.txt` |
| `cargo test --workspace` run 1 | **550 passed, 0 failed, 27 ignored** (39 suites) | 33 s | `test-run1.txt` |
| `cargo test --workspace` run 2 | 550 / 0 / 27, identical | 6 s | `test-run2.txt` |
| `cargo clippy --workspace --all-targets` | exit 0, **0 warnings** | 22 s | `clippy.txt` |
| `ghost doctor` | 7/7 PASS | - | `doctor.txt` |

No test flaked across two runs. Run 1's extra 27 s is first-launch cost of 39 fresh
test executables (each suite's own time is under 2.1 s in both runs), not test work.

### `ghost verify` (target-improve binaries, `GHOST_MCP_PATH` pinned)

| Run | Result | init | tab screenshot | Failing claim |
|---|---|---|---|---|
| 1 | 10/10 | 1437 ms (first launch of a new binary) | 1705 ms | - |
| 2 | 10/10 | 21 ms | 1770 ms | - |
| 3 | **9/10** | 20 ms | 1758 ms | foreground window never changed (1116950 -> 22022736) |
| 4 | **9/10** | 20 ms | 1795 ms | foreground window never changed (22022736 -> 1116950) |

Hwnd 1116950 is the "Grok Bot" Chrome window driving this session; 22022736 is a
Windows Terminal running `node.exe`. Ghost drives a headless browser during
`verify` and has no handle on either window. These are false FAILs (see item 3).

### Real-use latency and accuracy (`latency_probe.py`, n=20 after 2 warmups)

Target: Ghost Testbed (`crates/ghost-testbed`). Accuracy is re-observed, never taken
from the tool's return: a click must raise the title's `[clicks=N]` by exactly 1, a
typed token must be read back from the window text.

Hidden desktop (`ghost_window op=launch`, the default for Ghost-started apps):

| op | median ms | p95 ms | correct | `verified=true` |
|---|---|---|---|---|
| see mode=text | 16.4 / 18.0 | 25.1 / 25.3 | 20/20 | n/a |
| see (full tree) | 22.0 / 14.2 | 27.5 / 23.6 | 20/20 | n/a |
| find | 7.9 / 10.0 | 10.7 / 15.1 | 20/20 | n/a |
| act click | **113.4 / 121.4** | 122.8 / 128.3 | 20/20 | **10/20 / 10/20** |
| act type | 7.1 / 8.1 | 8.5 / 10.8 | 20/20 | 20/20 |

User desktop (testbed shown with `SW_SHOWNOACTIVATE`, the path agents use most):

| op | median ms | p95 ms | correct | `verified=true` |
|---|---|---|---|---|
| see mode=text | 51.6 / 30.8 | 58.7 / 39.4 | 20/20 | n/a |
| see (full tree) | 47.0 / 29.0 | 70.7 / 36.3 | 20/20 | n/a |
| find | 19.7 / 10.4 | 24.6 / 13.0 | 20/20 | n/a |
| act click | **123.4 / 122.3** | 143.3 / 127.6 | 20/20 | **0/20 / 0/20** |
| act type | 7.8 / 4.8 | 12.0 / 6.3 | 20/20 | 20/20 |

(two runs each, `run1 / run2`; raw in `probe-run{1,2}/`, `probe-user-run{1,2}/`)

Other measurements:

| Measurement | Value | Raw |
|---|---|---|
| Cold start, spawn -> `initialize` reply (n=10) | median 18.7 ms, p95 34.8 ms | `probe-run1/latency.json` |
| `tools/list` payload | 54 tools, 32,094 bytes (13,849 of descriptions) | `tools-list-breakdown.txt` |
| `ghost_tab_screenshot`, owned headless browser (n=20) | **first 1807 / 1923 ms, median 1007 / 1005 ms**, 20/20 valid PNG | `tabshot-run{1,2}/` |
| `ghost_tab_text`, same tab | 0-1 ms | `tab-screenshot-probe.txt` |
| Criterion microbenches (convert, grounding, intent) | all under 4 ms; most in ns | `criterion.txt` (239 s) |

Not run: `bench/run_bench.py` and `bench/soak.py`. Both set `GHOST_FOCUS_LOCK=off`
and raise Calculator onto the live desktop with `op=focus`, which would steal focus
from the sessions sharing this machine. Their last published numbers (2026-08-23)
were not re-measured.

## Ranked items

### 1. `ghost_act` click reports `verified=false` for clicks that worked - accuracy

**Problem.** In 80 real clicks that each verifiably landed (title counter rose by
exactly 1), `verified` was `true` in 0/40 on the user's desktop and 20/40 on the hidden
desktop. Every false one carries a note telling the agent to read state to confirm,
so the agent pays an extra `ghost_see` round trip or, worse, retries and double-clicks.
The hidden-path response already shows the effect: `"window": "Ghost Probe Bed [clicks=1]"`
next to "the window's pixels did not change".

**Cause.**
- User desktop, `crates/ghost-session/src/session.rs:2422-2434`: after the click it
  sleeps a fixed 80 ms, then `verify_pixels(before)` diffs **only the control's own
  rect**. A button looks the same after a click, so this is false by construction.
  The first click on a fresh window draws a focus cue, which is the only reason the
  first click ever verifies.
- Hidden desktop, `crates/ghost-session/src/hidden.rs:511-525`: fixed 80 ms sleep,
  then one whole-window PrintWindow diff (`pixels_changed`, `hidden.rs:232`).
  Whether the button's pressed-state repaint lands in that one capture is timing, so
  it is a coin flip (10/20 in both runs).
- Neither path looks at the cheapest state signals a click usually changes: the
  window title, the element's own UIA state (Toggle / Value / Selection / Name), or the
  focused element.
- The foreground path already polls with early exit (`verify_screen_change`,
  `session.rs:2736`); the two background paths do not.

**Change.** One shared background-click verifier used by both paths: snapshot title +
target element UIA state + whole-window pixels before; poll at 10/20/40/80 ms;
`verified=true` on the first observed change in any signal, which signal fired goes in
the response (`verified_by: "title" | "uia" | "pixels"`). It stays `false` only when
nothing changed by 80 ms. Add a `Noop` button to the testbed (changes nothing) as the
negative control, and a `--noop` op to `latency_probe.py`.

**Amended in review (2026-11-25), and how it was done:** pixels stay whole-window
on both surfaces (a click's effect is often elsewhere); noise is filtered by
requiring a change to persist in a second capture 20 ms later; a testbed `Note`
button that changes only a distant label must verify 20/20; a disabled button is
the negative control (>= 19/20 false); `verified` stays a bool and `verified_by` is
additive. Results: `docs/benches/2026-11-25-stage-2/README.md`.

**Acceptance** (`P = python docs/benches/2026-11-25-baseline/latency_probe.py --exe <target-improve ghost-mcp.exe> --testbed <target-improve ghost-testbed.exe> --n 20 --out <dir>`):
1. `P --surface user` and `P --surface hidden`: act_click `correct 20/20` **and**
   `verified=true 20/20` in each (baseline 0/20 and 10/20).
2. `P --surface user --noop` and `P --surface hidden --noop`: the Noop click reports
   `verified=false` in at least 19/20 (a verifier that always says true fails this).
3. act_click median at most 50 ms and p95 at most 100 ms on both surfaces (baseline
   113-123 ms / 123-143 ms), with type/see/find medians no worse than 1.2x baseline.
4. `cargo test --workspace` 550+ passed, 0 failed; `cargo clippy --workspace --all-targets`
   0 warnings.

### 2. `ghost_tab_screenshot` on a Ghost-owned headless browser takes ~1 s - latency

**Problem.** Measured median 1007 ms / 1005 ms (n=20, two runs), first capture
1807-1923 ms. The code documents 58 ms for this exact path (`crates/ghost-browser/src/tab.rs:618-625`,
"`Page.bringToFront`, then capture"). The steady state sits within 2% of 1000 ms on
every capture after the third, which points to waiting on a once-a-second frame
rather than to slow encoding. `ghost verify`'s own budget is 2 s, and it measures
1705-1795 ms (88% of budget) because it takes a first capture. Screenshots are the
fallback agents reach for whenever text or tree reads are not enough, so every one
costs a second.

**Change.** Measure first, then fix. Extend `examples/shot_probe.rs` (or `tab_shot_probe.py`)
to time, on an owned headless tab, (a) current path, (b) metrics-override toggle,
(c) `captureScreenshot` with `optimizeForSpeed` / `fromSurface`, and (d) forcing a frame
(e.g. `HeadlessExperimental.beginFrame` or a no-op rAF via `Runtime.evaluate`). Pick
the fastest that returns the correct image, and keep the attached-browser path
unchanged (it must never switch the user's tab).

**Acceptance** (`python docs/benches/2026-11-25-baseline/tab_shot_probe.py --exe <target-improve ghost-mcp.exe> --n 20 --out <dir>`, run twice):
1. Median at most 150 ms and first capture at most 500 ms in both runs (baseline
   ~1005 ms and ~1850 ms).
2. 20/20 valid PNG, and the image is the page: centre pixel matches the page's
   `#cc3333` background (add this check to the probe before the fix, so it is shown
   passing on the old code too).
3. `ghost verify` "background tab screenshot" at most 500 ms in 3/3 runs (baseline
   1705-1795 ms).
4. `cargo test -p ghost-browser` green.

### 3. `ghost verify` reports a false FAIL when anyone else changes the foreground - reliability of the proof

**Problem.** 2 of 4 `ghost verify` runs failed "foreground window never changed" while
focus moved between the session's driver app and a terminal, neither of which Ghost
touched (see baseline). The check is a raw before/after hwnd compare
(`crates/ghost-cli/src/verify.rs:356-361`). On any machine with a human or another
agent at work, `verify` exits 1, so it can't serve as a gate. The cursor check next
to it already accepts that the human moves things (`verify.rs:367`); the foreground
check doesn't. The server already has the right signal:
`ghost_stats.interference_audit.synthetic_foreground_changes`
(`crates/ghost-mcp/src/audit.rs:428-447`) counts only foreground changes with no real
hardware input in the prior 1.5 s, and names the tool calls in flight.

**Change.** Fail the claim when the audit recorded a synthetic change during the run
(or when hwnd changed and the audit is unavailable). Pass with a note when the hwnd
changed and the audit attributes it to real input. Put the decision in a pure
function with unit tests. Same item, same file family: `bench/run_bench.py:477`
hard-codes `target/release/ghost-testbed.exe` even when `--exe` points elsewhere, so a
bench of a side build silently uses the live build's testbed. Resolve it next to
`--exe`.

**Acceptance.**
1. Unit tests in `crates/ghost-cli/src/verify.rs`: (unchanged hwnd, 0 incidents) ->
   PASS; (changed, 0 incidents, audit present) -> PASS with note; (any hwnd, 1+
   incidents) -> FAIL; (changed, audit absent) -> FAIL. `cargo test -p ghost-cli` green.
2. 10 consecutive `GHOST_MCP_PATH=<target-improve ghost-mcp.exe> target-improve/release/ghost.exe verify`
   runs on this shared desktop: 10/10 exit 0 (baseline 2/4).
3. `grep -n "target\", \"release\", \"ghost-testbed" bench/run_bench.py` returns
   nothing, and `value_equals_assert` resolves the testbed from `os.path.dirname(--exe)`.

## Stages

Each stage is independently built, measured against the baseline above, and committed
on its own, with the evidence saved under `docs/benches/2026-11-25-stage-N/`.

| Stage | Items | Why this order |
|---|---|---|
| 2 | Item 1 (act click verification) | Biggest accuracy defect on the most-used verb; causes extra calls or double clicks. |
| 3 | Item 2 (headless tab screenshot) | 17x off the documented number; fix needs a measurement first, isolated to `ghost-browser`. |
| 4 | Item 3 (verify attribution + bench testbed path) | Makes the proof tooling usable as a gate on a shared machine; smallest change. |

After stage 4, re-run the full baseline and close only when nothing measured still
shows a meaningful gap.

## Considered and rejected

- **Shrink `tools/list` (54 tools, 32 KB, about 8k tokens per session).** The payoff
  is real but only in tokens; the 12 `ghost_desktop_*` and 14 `ghost_tab_*` tools are
  the browser and isolated-desktop surfaces some agents rely on. Removing or hiding them
  changes the public contract, and there is no measured agent failure that traces to
  the list size. It's a product decision for the owner, not a measured fix.
- **Cold start.** Median 18.7 ms. The 1437 ms seen once is the first launch of a new
  binary (OS scanning), not Ghost code.
- **see / find / type latency.** 5-50 ms medians, all 20/20 correct. No headroom
  worth a change.
- **Criterion hot paths.** All under 4 ms, most in nanoseconds, against 100 ms+ costs
  in item 1 and 2.
- **Clippy, compiler warnings, flaky tests.** 0 warnings, 0 failures, identical on two
  runs. Nothing to fix.
- **README soak figure (says p50 85 ms; `bench/results/soak.md` says 148.9 ms).**
  Docs-only drift, out of scope for this session.
- **Re-running `run_bench.py` / `soak.py`.** They raise Calculator on the live
  desktop. Worth doing on an idle machine, not here.
