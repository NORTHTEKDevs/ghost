//! Page-idle detection via repeated downsample+hash comparison.
//!
//! The real implementation needs a real capture on every poll
//! (ScreenCaptureKit) to know whether the screen is still changing, so
//! `wait_stable` cannot do anything useful without one. `new()` does no OS
//! call on Windows either (it is a placeholder constructor there too), so it
//! succeeds here for the same reason `UiaTree::new()` and `init_com()` do:
//! failing at construction would be a bigger, less specific claim than
//! failing at the first real use.

use crate::error::CoreError;

pub struct IdleDetector {
    _placeholder: (),
}

impl IdleDetector {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self { _placeholder: () })
    }

    pub async fn wait_stable(&self, _stable_frames: u32, _timeout_ms: u64) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "wait_stable", needs: "ScreenCaptureKit" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_stable_fails_honestly_instead_of_hanging() {
        let d = IdleDetector::new().unwrap();
        let r = d.wait_stable(3, 50).await;
        assert!(matches!(r, Err(CoreError::Unsupported { .. })));
    }
}
