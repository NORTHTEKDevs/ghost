visibility/focus on attach: hidden/false

| strategy | n | first ms | median ms | p95 ms | valid | page centre | visibility after |
|---|---|---|---|---|---|---|---|
| current: bringToFront + capture | 20 | 270.7 | 1003.8 | 1540.4 | 20/20 | 20/20 | visible/true |
| metrics override toggle | 20 | 1000.2 | 36.3 | 1533.0 | 20/20 | 20/20 | visible/true |
| bringToFront + optimizeForSpeed | 20 | 1002.7 | 1005.7 | 1544.7 | 20/20 | 20/20 | visible/true |
| bringToFront + fromSurface | 20 | 1007.7 | 1006.5 | 1014.2 | 20/20 | 20/20 | visible/true |
| bringToFront + rAF await + capture | 20 | 1007.1 | 1006.8 | 1013.3 | 20/20 | 20/20 | visible/true |
| bringToFront + focus emulation | 20 | 1007.9 | 1004.4 | 1015.7 | 20/20 | 20/20 | visible/true |
| bringToFront + lifecycle active | 20 | 1013.1 | 1002.7 | 1013.1 | 20/20 | 20/20 | visible/true |
| current again (order check) | 20 | 1003.3 | 1005.4 | 1015.0 | 20/20 | 20/20 | visible/true |
