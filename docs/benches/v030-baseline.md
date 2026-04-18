# Ghost v0.3.0 Performance Baseline

Run: `cargo bench -p ghost-intent`

## Budgets (v0.3.0)

| Operation | Budget | Rationale |
|-----------|--------|-----------|
| JSONLogic eval (single op) | < 1 µs | Polled per `wait_until` tick |
| Intent compile (3 ops) | < 50 µs | Single-shot at `execute_intent` entry |
| Describe-screen delta (no change) | < 2 ms | Polled for idle detection |
| Cached UIA tree walk (50 elements) | < 8 ms | Populates delta + locator cache |
| sonic-rs encode (75 KB response) | < 3 ms (3-5x serde_json) | MCP stdout serialization |

## Regression Gate

`scripts/bench-check.sh` (TBD) compares current run to committed `v030-baseline`
and fails CI with non-zero exit on >20% regression.

## Re-baseline

```bash
cargo bench -p ghost-intent --save-baseline v030
# review numbers; if acceptable, copy criterion output summary into this file.
```
