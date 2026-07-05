# Ghost reliability soak

**PASS** - 160 acts over 40 cycles (21.5s wall).

| Metric | Value | Threshold |
| --- | --- | --- |
| verify-null rate | 0.0 | <= 0.1 |
| verify-false rate | 0.0312 | <= 0.15 |
| focus-lost rate | 0.0 | <= 0.1 |
| error rate | 0.0 | <= 0.02 |
| effect-mismatch rate | 0.0 | == 0.0 |
| latency p50 / p95 / p99 / max (ms) | 84.6 / 116.8 / 365.4 / 367.3 | - |

Effect correctness is re-observed (the Calculator display must read the expected sum), never trusted from the tool's return. `--self-test` plants a wrong expected value and passes only if the harness flags every cycle.
