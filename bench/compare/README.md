# Cross-tool comparison harness

Measures a computer-use tool against a shared, tool-agnostic task set
([`tasks.py`](tasks.py)) by **re-observing the real result**, never by trusting a
tool's return value. It is deliberately honest about scope: it only runs tools
that have an adapter, and never fills in competitor numbers it didn't measure.

```bash
cargo build --release -p ghost-mcp
python bench/compare/run.py          # runs the Ghost adapter over every task
python bench/compare/run.py --json   # also writes results/compare.json
```

Latest Ghost run: **6/6** — see [`results/compare.md`](results/compare.md). Exit
code is 0 only if the runnable adapter passes every task, so this is also a
regression self-test.

## The task set

Six capabilities a computer-use agent tool should have — including the three that
separate tools in practice:

- **background dispatch** — act inside an app without taking foreground/cursor,
- **per-action verification** — the tool tells you whether the action worked,
- **no-API reach** — drive native apps with no automation hooks or CDP.

## Adding a competitor (the honest way to earn "best")

The point of this harness is that the "best" claim should be *measured*, not
asserted. To benchmark another tool:

1. Subclass `Adapter` in [`adapters.py`](adapters.py) and implement
   `run(task_id) -> Result`. Return `Result(applicable=False)` for tasks the tool
   genuinely can't attempt (e.g. a browser-only tool on a native-app task — that
   shows as `N/A`, not `FAIL`).
2. Register an instance in `ADAPTERS` in [`run.py`](run.py).
3. Re-run — that tool becomes a measured column.

Why competitor columns are `-` today: fairly running Playwright-MCP, Anthropic
Computer Use, and cua-driver requires standing each one up (a browser context, an
Anthropic key + VM, the Hermes harness) on this same task set. That's real work
and hasn't been done here — so the matrix shows `-` for them rather than numbers
we didn't earn. The architectural "when to use which" picture (not measured) lives
in [`docs/comparison.md`](../../docs/comparison.md).
