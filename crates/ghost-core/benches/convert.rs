//! Micro-benchmark proving the region-convert optimization: converting only the
//! sub-rect a small window occupies vs converting the whole monitor frame.
//!
//! Run: cargo bench -p ghost-core --bench convert
//!
//! `ghost-core` compiles to nothing off Windows, so the benchmark body is gated
//! and a stub `main` keeps the target buildable on other platforms (a bench
//! target must always produce an executable).

#[cfg(windows)]
mod windows_bench {
    use criterion::{black_box, criterion_group, Criterion};
    use ghost_core::capture::screen::{bgra_to_rgba, bgra_to_rgba_region};

    fn make_frame(w: usize, h: usize, pitch: usize) -> Vec<u8> {
        let _ = (w, h);
        let mut v = vec![0u8; pitch * h];
        for (i, b) in v.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        v
    }

    fn bench_convert(c: &mut Criterion) {
        // 1080p monitor, tight pitch, converting the full frame vs a typical
        // dialog-sized window (400x300).
        let (w, h) = (1920usize, 1080usize);
        let pitch = w * 4;
        let frame = make_frame(w, h, pitch);

        let mut g = c.benchmark_group("bgra_to_rgba_1080p");
        g.bench_function("full_frame", |b| {
            b.iter(|| black_box(bgra_to_rgba(black_box(&frame), w, h, pitch)));
        });
        g.bench_function("region_400x300", |b| {
            b.iter(|| black_box(bgra_to_rgba_region(black_box(&frame), pitch, 100, 100, 400, 300)));
        });
        g.finish();
    }

    criterion_group!(benches, bench_convert);

    pub fn run() {
        benches();
        criterion::Criterion::default().configure_from_args().final_summary();
    }
}

fn main() {
    #[cfg(windows)]
    windows_bench::run();
}
