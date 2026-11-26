# Stage 3 - headless tab screenshot latency

Item 2 of `docs/plans/2026-11-25-ghost-improvements.md`. All builds in
`target-improve`; live binaries untouched.

## Result

One launch flag for Ghost's headless browsers, `--disable-gpu-compositing`
(`crates/ghost-browser/src/launch.rs`, `base_args`, headless branch only).
`ghost_tab_screenshot` on an owned headless tab: median **1004 ms -> 31 ms**,
first capture **~1.93 s -> 37-62 ms**, image identical. The capture code
(`tab.rs`) and the attached-browser path are unchanged; only a doc comment there
was corrected (it cited a probe file that was never committed).

## Measured before choosing (raw files in this folder)

1. `before-run{1,2}` - the shipped build through Ghost: median 1004.8 / 1003.8 ms,
   first 1919 / 1934 ms, valid PNG 20/20 and page centre 20/20 (the new
   correctness check passes on the old code).
2. `variants-run1` - capture strategies over raw CDP on a tab Ghost launched and
   opened. The tab reads `hidden` on attach and `visible` after bringToFront, yet
   every strategy waits ~1000 ms: bringToFront (1003.8), optimizeForSpeed
   (1005.7), fromSurface (1006.5), awaiting a requestAnimationFrame (1006.8),
   focus emulation (1004.4), lifecycle `active` (1002.7). Only the device-metrics
   override toggle forces a frame, and not reliably: ~30 ms on 15/20, 745-1533 ms
   on 5/20. So it is not visibility throttling; the capture is waiting for a
   frame that comes about once a second.
3. `flags-run1` - Ghost's exact headless command line launched directly (median
   1003.9 ms, so the delay is Chrome's, not Ghost's code), plus
   `--disable-gpu-vsync` / `--disable-frame-rate-limit` (the hidden-desktop fix
   in launch.rs): no change, ~1008 ms. `--disable-frame-rate-limit` alone tripled
   idle CPU for nothing.
4. `targets-run1` - how the tab is created (background, foreground, new window,
   the browser's initial tab): all 1003-1007 ms.
5. `flags-run2` - `--disable-gpu` 31.2 ms, `--disable-gpu-compositing` 31.2 ms,
   `--run-all-compositor-stages-before-draw` 1010 ms,
   `--disable-features=CalculateNativeWinOcclusion` 1003 ms. Every image correct.
6. `flags-run3-idlecpu` - the cost: steady-state idle CPU over 30 s after a 10 s
   settle. Shipped ~0%, `--disable-gpu` 3.5%, `--disable-gpu-compositing` 3.7% of
   one core. (A 5 s window right after the captures read 27-29%: a burst, not
   steady state.)
7. `flags-run4-edge` - Edge, which the default browser can resolve to: 614 ms ->
   40.3 ms, idle CPU ~0.

`--disable-gpu-compositing` over `--disable-gpu`: same speed, but it leaves GPU
rasterization and WebGL alone; `--disable-gpu` removes the GPU, and current
Chrome no longer falls back to software WebGL without an extra flag.

## Acceptance

| Criterion | Target | Baseline | Measured |
|---|---|---|---|
| Probe median, run 1 / run 2 (`after-run*`) | <= 150 ms | 1007.3 / 1004.8 | **31.1 / 31.2 ms** |
| Probe first capture, run 1 / run 2 | <= 500 ms | 1807 / 1923 | **36.7 / 61.8 ms** |
| Valid PNG | 20/20 | 20/20 | 20/20, 20/20 |
| Page centre pixel is #cc3333 (added this stage) | 20/20, also on old code | 20/20 (old code, `before-run*`) | 20/20, 20/20 |
| `ghost verify` "background tab screenshot", 3 runs | <= 500 ms in 3/3 | 1705-1795 ms | **76 / 63 / 55 ms**, all 10 claims pass 3/3 |
| Attached-browser path unchanged | no code change | - | `tab.rs` diff is doc comment only |
| `cargo test --workspace` | 0 failed | 566 / 0 / 27 | 567 passed / 0 failed / 27 ignored, both runs |
| `cargo clippy --workspace --all-targets` | 0 warnings | 0 | 0 |
| vibecode lint, changed files | 0 HIGH | - | 0 findings |
| Cost: idle CPU of a headless browser | disclosed | ~0% | ~3.7% of one core (Chrome), ~0% (Edge) |

## Reproduce

```bash
R=<abs path to target-improve/release>
python docs/benches/2026-11-25-stage-3/tab_shot_probe.py --exe "$R/ghost-mcp.exe" --n 20 --out <dir>
python docs/benches/2026-11-25-stage-3/flag_variants.py --chrome "<chrome.exe>" --only A,F --settle-s 10 --idle-s 30 --out <dir>
```
