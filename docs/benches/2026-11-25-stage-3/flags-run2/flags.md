| launch flags | n | first ms | median ms | p95 ms | valid | page centre | idle CPU (% of a core) |
|---|---|---|---|---|---|---|---|
| A: ghost headless as shipped | 20 | 1520.2 | 1001.8 | 1520.2 | 20/20 | 20/20 | 2.5 |
| E: + --disable-gpu | 20 | 32.4 | 31.2 | 33.2 | 20/20 | 20/20 | 27.2 |
| F: + --disable-gpu-compositing | 20 | 38.8 | 31.2 | 38.8 | 20/20 | 20/20 | 28.7 |
| G: + --run-all-compositor-stages-before-draw | 20 | 1503.6 | 1010.2 | 1503.6 | 20/20 | 20/20 | 3.1 |
| H: + --disable-features=CalculateNativeWinOcclusion | 20 | 1544.9 | 1003.0 | 1544.9 | 20/20 | 20/20 | 0.6 |
