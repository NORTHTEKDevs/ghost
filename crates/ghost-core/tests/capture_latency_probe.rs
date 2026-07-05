//! One-off measurement: GDI region capture vs DXGI region capture on the SAME
//! primary rect, to decide whether the off-primary GDI path is a real latency
//! penalty worth a per-output DXGI duplicator. Run:
//!   cargo test -p ghost-core --test capture_latency_probe -- --ignored --nocapture

use ghost_core::capture::screen::{capture_screen_region_fast, capture_virtual_rect_gdi};

fn median_ms<F: FnMut() -> bool>(iters: usize, mut f: F) -> (f64, usize) {
    let mut samples = Vec::with_capacity(iters);
    let mut ok = 0usize;
    for _ in 0..iters {
        let t = std::time::Instant::now();
        let good = f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
        if good { ok += 1; }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (samples[samples.len() / 2], ok)
}

#[test]
#[ignore]
fn gdi_vs_dxgi_region_latency() {
    let iters = 60;
    // (label, l,t,r,b) — small window, medium window, large window.
    let cases = [
        ("400x300  ", (100, 100, 500, 400)),
        ("1000x700 ", (100, 100, 1100, 800)),
        ("1600x900 ", (100, 100, 1700, 1000)),
    ];
    println!("REGION-CAPTURE LATENCY (median of {iters}):");
    for (label, rect) in cases {
        for _ in 0..5 {
            let _ = capture_screen_region_fast(rect);
            let _ = capture_virtual_rect_gdi(rect.0, rect.1, rect.2, rect.3);
        }
        let (dxgi_ms, _) = median_ms(iters, || capture_screen_region_fast(rect).is_ok());
        let (gdi_ms, _) = median_ms(iters, || {
            capture_virtual_rect_gdi(rect.0, rect.1, rect.2, rect.3).is_ok()
        });
        println!(
            "  {label}  DXGI {dxgi_ms:6.3} ms | GDI {gdi_ms:6.3} ms | GDI/DXGI {:.2}x",
            gdi_ms / dxgi_ms.max(0.0001)
        );
    }
    // Idle-screen case: let the screen go quiet, then one DXGI region capture.
    // On a truly static screen DXGI AcquireNextFrame can burn its full timeout.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let rect = (100, 100, 500, 400);
    let t = std::time::Instant::now();
    let _ = capture_screen_region_fast(rect);
    let idle_dxgi = t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    let _ = capture_virtual_rect_gdi(rect.0, rect.1, rect.2, rect.3);
    let idle_gdi = t.elapsed().as_secs_f64() * 1000.0;
    println!("  IDLE 400x300  DXGI {idle_dxgi:6.3} ms | GDI {idle_gdi:6.3} ms (after 600ms quiet)");
}
