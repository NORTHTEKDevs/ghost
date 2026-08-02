//! Screen-idle detection.
//!
//! Used to wait for an application to settle after an action -- a spinner
//! stopping, a page finishing render. Works by sampling the same luma grid the
//! verifier uses and declaring idle once consecutive samples stop differing.

use std::time::{Duration, Instant};

use super::screen::capture_screen_full_rgba;
use super::verify::{grid_delta, luma_grid};

/// Per-sample delta below which the screen counts as still.
const IDLE_DELTA: f32 = 0.001;

pub struct IdleDetector {
    /// Consecutive still samples required before declaring idle. More than one
    /// avoids calling a slow animation "idle" between two frames.
    required_still: u32,
    interval: Duration,
}

impl Default for IdleDetector {
    fn default() -> Self {
        Self { required_still: 2, interval: Duration::from_millis(120) }
    }
}

impl IdleDetector {
    /// Signature matches `ghost_core::capture::idle::IdleDetector::new`.
    pub fn new() -> crate::error::Result<Self> {
        Ok(Self::default())
    }

    pub fn with_settings(required_still: u32, interval: Duration) -> Self {
        Self { required_still: required_still.max(1), interval }
    }

    /// Wait until the screen has been unchanged for `stable_frames` consecutive
    /// samples. Signature matches `ghost_core::capture::idle::wait_stable`.
    ///
    /// `Err(JobTimeout)` means it never settled -- the same signal the Windows
    /// engine gives, so callers branch identically.
    pub async fn wait_stable(
        &self,
        stable_frames: u32,
        timeout_ms: u64,
    ) -> crate::error::Result<()> {
        let deadline =
            std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let want = stable_frames.max(1);
        let mut previous: Option<Vec<f32>> = None;
        let mut still = 0u32;

        loop {
            // Honour the emergency stop between samples. On Linux `ghost_stop`
            // is the only out-of-band control -- there is no global hotkey -- so
            // a long wait that ignores the flag is genuinely uninterruptible.
            if crate::input::hotkey::is_stopped() {
                return Err(crate::error::CoreError::JobTimeout);
            }
            // Capture is blocking; keep it off the async worker.
            let shot = tokio::task::spawn_blocking(capture_screen_full_rgba)
                .await
                .map_err(|e| crate::error::CoreError::WorkerPanic(e.to_string()))?;
            let Ok((rgba, w, h)) = shot else {
                return Err(crate::error::CoreError::JobTimeout);
            };
            let Some(grid) = luma_grid(&rgba, w, h) else {
                return Err(crate::error::CoreError::JobTimeout);
            };

            if let Some(prev) = &previous {
                if grid_delta(prev, &grid) <= IDLE_DELTA {
                    still += 1;
                    if still >= want {
                        return Ok(());
                    }
                } else {
                    still = 0;
                }
            }
            previous = Some(grid);

            if std::time::Instant::now() >= deadline {
                return Err(crate::error::CoreError::JobTimeout);
            }
            tokio::time::sleep(self.interval).await;
        }
    }

    /// Block until the screen stops changing, or `timeout` elapses.
    ///
    /// Returns `true` if it settled, `false` on timeout. A capture failure
    /// mid-wait ends the wait as "not settled" rather than looping forever.
    pub fn wait_for_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut previous: Option<Vec<f32>> = None;
        let mut still = 0u32;

        while Instant::now() < deadline {
            if crate::input::hotkey::is_stopped() {
                return false;
            }
            let Ok((rgba, w, h)) = capture_screen_full_rgba() else { return false };
            let Some(grid) = luma_grid(&rgba, w, h) else {
                return false;
            };

            if let Some(prev) = &previous {
                if grid_delta(prev, &grid) <= IDLE_DELTA {
                    still += 1;
                    if still >= self.required_still {
                        return true;
                    }
                } else {
                    still = 0;
                }
            }
            previous = Some(grid);
            std::thread::sleep(self.interval);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_still_is_never_zero() {
        // Zero would make the very first sample "idle", defeating the point.
        assert_eq!(IdleDetector::with_settings(0, Duration::from_millis(10)).required_still, 1);
    }

    #[test]
    fn default_requires_more_than_one_still_sample() {
        assert!(IdleDetector::default().required_still >= 2);
    }

    #[test]
    fn a_zero_timeout_returns_immediately_without_settling() {
        assert!(!IdleDetector::default().wait_for_idle(Duration::ZERO));
    }
}
