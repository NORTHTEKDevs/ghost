# Benchmarks

Ghost publishes only its own measured numbers, never invented columns for
other tools. [`bench/README.md`](../bench/README.md) gives a protocol for
comparing against Playwright-MCP, Computer Use or UI-TARS, and explains why a
naive same-suite comparison is not apples to apples.

## Task success

`bench/run_bench.py` drives the real `ghost-mcp` binary through 14 Windows
desktop tasks and scores each by **re-observing the result** (does the
Calculator display really read 42? is the typed value really there?), never by
trusting a tool's return value.

Latest run ([`bench/results/latest.md`](../bench/results/latest.md)): **14/14
tasks passed**, median about 2.9 s per task including app launch, covering
perception, click and keyboard action with verification, waits, window
management, text extraction, disambiguation, flow chaining, clipboard round
trip, structured errors, element screenshots and value assertions.

`--self-test` runs deliberately wrong negative controls (assert the display
reads 99 when it reads 42) and passes only if the harness scores every one as a
failure, so a green run is a real signal.

```bash
cargo build --release -p ghost-mcp
python bench/run_bench.py             # exit 0 if every task passed
python bench/run_bench.py --self-test # exit 0 if every planted failure was caught
```

## Reliability soak

`bench/soak.py` drives many act-then-verify cycles and gates on what unit tests
cannot see: how often `verified` comes back null or false, focus loss, error
rate, whether each action's real effect happened, and latency percentiles.

Latest (160 acts, [`bench/results/soak.md`](../bench/results/soak.md)):
**PASS**, verify-null 0.0, focus-loss 0.0, effect mismatches 0, p50 149 ms,
p95 155 ms.

```bash
python bench/soak.py                  # exit 0 if every threshold holds
python bench/soak.py --cycles 250     # about 1000 acts
python bench/soak.py --self-test      # exit 0 if a planted wrong effect is flagged
```

## Per-call speed

About 75 ms for a text read, 200 ms for a verified click, 1 ms to list windows.
`scripts/speed-probe.mjs` measures these against any `ghost-mcp` binary. What
makes an agent session slow is usually around the calls, and three habits remove
most of it:

- Name the window once and omit `window=` afterwards; the anchor follows it by
  handle.
- Wait for a condition, not a duration: `ghost_wait for=element`, `for=value`,
  `for=navigate` return the moment the thing happened.
- Batch steps with `ghost_run` so one model turn does several.

## Microbenchmarks

| Operation | Measured |
| --- | --- |
| Region capture, GDI, any size | about 16.5 ms |
| Region capture, DXGI, 1600x900 | 70 to 83 ms |
| BGRA to RGBA convert, 400x300 region | about 206 us |
| JSONLogic eq/var | 32.2 ns |
| Intent compile (3 ops) | 1.49 us |

Region captures (verification, screenshots, marks) use GDI because the DXGI
acquire dominates and degrades on large windows; full-screen capture still uses
DXGI. Run the convert microbench with `cargo bench -p ghost-core --bench
convert`. Engineering measurements behind each release live in
[`benches/`](benches/README.md).
