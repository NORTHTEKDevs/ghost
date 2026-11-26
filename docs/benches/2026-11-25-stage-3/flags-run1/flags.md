| launch flags | n | first ms | median ms | p95 ms | valid | page centre | idle CPU (% of a core) |
|---|---|---|---|---|---|---|---|
| A: ghost headless as shipped | 20 | 1525.4 | 1003.9 | 1525.4 | 20/20 | 20/20 | 2.8 |
| B: + --disable-gpu-vsync --disable-frame-rate-limit | 20 | 1535.8 | 1009.2 | 1535.8 | 20/20 | 20/20 | 5.3 |
| C: + --disable-frame-rate-limit | 20 | 1531.4 | 1008.2 | 1531.4 | 20/20 | 20/20 | 9.4 |
| D: + --disable-gpu-vsync | 20 | 1567.3 | 1009.5 | 1567.3 | 20/20 | 20/20 | 2.5 |
