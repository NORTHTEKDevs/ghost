# Ghost benchmark

A reproducible benchmark for Ghost's MCP surface. It drives the compiled
`ghost-mcp` binary over stdio JSON-RPC — the same transport a real MCP client
uses — through a suite of real Windows desktop tasks, and **scores each task by
reading an actual post-condition back from the screen**, not by trusting that a
tool call returned `ok`. A task passes only when the observable world changed the
way it should (the Calculator display really reads `42`, the typed value is
really present, and so on).

## Why this design

Most automation "benchmarks" measure whether an API call succeeded. That is not
the same as the task working — an action can return success and do nothing (wrong
window, disabled control, silent no-op). This suite deliberately closes that gap:
every task launches its own app, performs the action, then **re-observes** the
result independently before deciding pass/fail.

Every task is self-contained and cleans up after itself, so the suite runs on any
Windows 10/11 machine with no setup and is safe to re-run. It uses only apps that
ship with Windows (Calculator, Notepad), so results are portable and comparable.

## What it covers

| Task | What it proves |
|---|---|
| `find_button` | Perception + UIA grounding return real coordinates |
| `click_compute` | Click a sequence, verify the computed result on screen |
| `keyboard_compute` | Keyboard input reaches the target and computes correctly |
| `act_verified` | Actions report honest `verified` / `focus_confirmed` |
| `wait_for_element` | Wait-for-element-to-appear works without polling by the caller |
| `window_list_state` | Window enumeration includes state (normal/minimized) |
| `read_text` | Reads UI labels straight from the accessibility tree |
| `index_disambiguation` | Selects the nth of several same-role elements, with a count |
| `run_chaining` | A flow step's output feeds the next step (`${steps.N.path}`) |
| `structured_error` | A missing element yields a typed code + suggested action |
| `window_minimize_restore` | Minimize then restore a window; verify state changes |
| `clipboard_roundtrip` | Clipboard set then get round-trips |
| `screenshot_element` | Screenshot cropped to a single element (real image bytes) |
| `value_equals_assert` | Asserts an element's real value after typing (fresh tab) |

## Run it

```bash
cargo build --release -p ghost-mcp
python bench/run_bench.py              # the 14-task suite
python bench/run_bench.py --self-test  # the negative controls (see below)
```

Output: a live PASS/FAIL log, plus `bench/results/latest.json` (machine-readable)
and `bench/results/latest.md` (a table). Exit code is `0` only if every task
passed, so it can gate CI. The harness saves the user's clipboard before the run
and restores it after.

## Proving the benchmark can fail

A benchmark that only ever goes green proves nothing. `--self-test` runs a set of
**negative controls** — tasks whose real-world outcome is deliberately wrong
(assert the Calculator display reads 99 when it really reads 42; treat a missing
element as "found"; validate junk bytes as an image). It passes only if the
harness scores *every* one of them as FAIL. So a green suite run means the
scoring genuinely distinguishes working from broken, not that it always says yes.

The run takes over the mouse/keyboard for ~1-2 minutes (it is real OS-level
automation). Don't type during the run. Latencies include full app launch, so
they are end-to-end task times, not per-call microbenchmarks (the per-call
latency benches live under `docs/benches/`).

Latest measured results: [`results/latest.md`](results/latest.md).

## Comparing against other tools — honestly

We do **not** publish numbers for other tools that we did not run, and this repo
contains none. A fair head-to-head is harder than it looks, and inventing a
rival's column is the fastest way to lose credibility. What actually differs:

- **Playwright / Playwright-MCP** is browser-only (Chromium/WebKit/Firefox via
  its own protocol). It cannot run the desktop tasks here at all (Calculator,
  native windows), so a same-suite comparison is not apples-to-apples. Compare it
  to Ghost only on browser tasks, and note Playwright drives the browser's
  internal automation protocol (fast, but browser-restricted and exposes the
  standard automation signals sites check for) whereas Ghost drives the OS with
  real input events and works on any native app, not just browsers.
- **Claude Computer Use / UI-TARS / OmniParser** are vision-first: they locate
  targets from screenshots via a model. They need an API key and typically a VM.
  To compare, run the equivalent task descriptions through that agent in its own
  harness and score the same post-conditions (does the Calculator really read 42?).

If you want a cross-tool comparison, the reproducible protocol is:

1. Take the task list above as the shared spec (each has an objective
   post-condition, e.g. "display shows 42").
2. Implement each task in the other tool's harness.
3. Score with the SAME independent re-observation this harness uses.
4. Report the machine, OS, and date alongside the numbers.

That yields a comparison a reader can trust because they can rerun it. We publish
Ghost's column from a real run; we leave the others for whoever runs them.

## Limitations (stated plainly)

- The shipped suite is desktop-focused (Calculator + Notepad). Browser tasks are
  a documented extension, not included, because portably and non-disruptively
  automating an arbitrary user's browser session is genuinely hard and would make
  the suite unsafe to run on a machine someone is using.
- Numbers are a snapshot of one machine (recorded in `results/latest.json`).
  The durable, comparable artifact is the harness + task spec, not the snapshot.
- `latency_ms` is whole-task wall-clock and is **dominated by fixed harness
  sleeps and app-launch time**, not by ghost-mcp's own speed — treat it as "these
  tasks complete in a couple of seconds each," NOT as a measure of tool-call
  latency. Per-call latency benches (nanoseconds/microseconds) live in
  `docs/benches/`.
- Pass/fail is the trustworthy signal here; each task re-observes a real
  post-condition (exact display value, decodable image bytes, independent value
  read-back) rather than trusting a tool call's own return.
