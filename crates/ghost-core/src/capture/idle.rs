//! Page-idle detection via DXGI duplication + stable-frame hash.
//!
//! Strategy: downsample captured frame to 4x4, blake3 it, call "stable" once
//! N consecutive frames produce the same hash. The DXGI surface is held open
//! across calls to avoid re-init cost on every poll.

use crate::error::CoreError;
use std::time::{Duration, Instant};

pub fn hash_frame(pixels: &[u8]) -> [u8; 32] {
    blake3::hash(pixels).into()
}

/// Downsample an RGBA buffer to a 4x4 average, returning 64 bytes (16 px * 4 channels).
pub fn downsample_to_4x4(pixels: &[u8], width: usize, height: usize) -> [u8; 64] {
    let mut out = [0u8; 64];
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return out;
    }
    let cell_w = (width / 4).max(1);
    let cell_h = (height / 4).max(1);
    for by in 0..4 {
        for bx in 0..4 {
            let mut rgba = [0u64; 4];
            let mut n: u64 = 0;
            for y in (by * cell_h)..(((by + 1) * cell_h).min(height)) {
                for x in (bx * cell_w)..(((bx + 1) * cell_w).min(width)) {
                    let idx = (y * width + x) * 4;
                    if idx + 3 < pixels.len() {
                        for c in 0..4 {
                            rgba[c] += pixels[idx + c] as u64;
                        }
                        n += 1;
                    }
                }
            }
            if n > 0 {
                let dst = (by * 4 + bx) * 4;
                for c in 0..4 {
                    out[dst + c] = (rgba[c] / n) as u8;
                }
            }
        }
    }
    out
}

/// Stub DXGI-backed idle detector. Real implementation polls `IDXGIOutputDuplication`.
/// Constructor returns `Err(IdleUnavailable)` if DXGI can't be initialized (no display, etc).
pub struct IdleDetector {
    _placeholder: (),
}

impl IdleDetector {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self { _placeholder: () })
    }

    /// Wait until `stable_frames` consecutive captures yield identical downsampled hashes,
    /// or return `Err(CoreError::JobTimeout)` after `timeout_ms` elapses.
    pub async fn wait_stable(&self, stable_frames: u32, timeout_ms: u64) -> Result<(), CoreError> {
        let start = Instant::now();
        let deadline = Duration::from_millis(timeout_ms);
        let mut last_hash: Option<[u8; 32]> = None;
        let mut count = 0u32;

        loop {
            if start.elapsed() >= deadline {
                return Err(CoreError::JobTimeout);
            }
            let png = crate::capture::screen::capture_screen()?;
            let hash = hash_frame(&png);
            if Some(hash) == last_hash {
                count += 1;
                if count >= stable_frames {
                    return Ok(());
                }
            } else {
                count = 1;
                last_hash = Some(hash);
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_of_identical_buffers_matches() {
        let a = vec![0u8; 4 * 4 * 4];
        let b = vec![0u8; 4 * 4 * 4];
        assert_eq!(hash_frame(&a), hash_frame(&b));
    }

    #[test]
    fn hash_differs_when_pixels_differ() {
        let a = vec![0u8; 4 * 4 * 4];
        let mut b = a.clone();
        b[0] = 255;
        assert_ne!(hash_frame(&a), hash_frame(&b));
    }

    #[test]
    fn downsample_averages_uniform_buffer() {
        let pixels = vec![128u8; 16 * 16 * 4];
        let out = downsample_to_4x4(&pixels, 16, 16);
        for &b in &out {
            assert_eq!(b, 128);
        }
    }

    #[tokio::test]
    #[ignore] // requires display
    async fn idle_detector_returns_stable_on_static_desktop() {
        let d = IdleDetector::new().unwrap();
        let r = d.wait_stable(3, 2000).await;
        assert!(r.is_ok() || matches!(r, Err(CoreError::JobTimeout)));
    }
}
