//! Cross-process lease on the shared foreground/input desktop.
//!
//! Background actions need no lease - they are per-window and can run fully
//! concurrently across any number of ghost processes. But the real cursor,
//! keyboard focus, and foreground window are a *single shared resource*: if two
//! ghost processes call `SendInput` at the same time, their keystrokes interleave
//! into whichever window happens to hold focus and both automations corrupt.
//!
//! Any code path that falls back to real input takes this lease first. The lease is
//! a session-local named mutex, so it serializes across every ghost process running
//! as the same user.

use crate::error::CoreError;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// `Local\` scopes the mutex to the current logon session, which is the same scope
/// as the input desktop the lease protects.
const LEASE_NAME: &str = r"Local\ghost-foreground-input-lease";

/// An acquired lease. Releases the mutex on drop.
pub struct ForegroundLease {
    handle: HANDLE,
}

impl ForegroundLease {
    /// Block until the foreground input resource is free, or `timeout_ms` elapses.
    ///
    /// `WAIT_ABANDONED` is treated as success: it means a previous holder died
    /// without releasing, so the resource is free and now ours.
    pub fn acquire(timeout_ms: u32) -> Result<Self, CoreError> {
        unsafe {
            let handle = CreateMutexW(None, false, &HSTRING::from(LEASE_NAME))
                .map_err(|e| CoreError::Win32 {
                    code: e.code().0 as u32,
                    context: "CreateMutexW: foreground lease",
                })?;
            let wait = WaitForSingleObject(handle, timeout_ms);
            if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
                Ok(Self { handle })
            } else {
                let _ = CloseHandle(handle);
                if wait == WAIT_TIMEOUT {
                    Err(CoreError::ForegroundBusy { ms: timeout_ms })
                } else {
                    Err(CoreError::Win32 {
                        code: wait.0,
                        context: "WaitForSingleObject: foreground lease",
                    })
                }
            }
        }
    }
}

impl Drop for ForegroundLease {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Run `f` holding the foreground lease. Every real-input fallback goes through here.
pub fn with_foreground_lease<T>(
    timeout_ms: u32,
    f: impl FnOnce() -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let _lease = ForegroundLease::acquire(timeout_ms)?;
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_reentrant_across_sequential_acquires() {
        {
            let _l = ForegroundLease::acquire(2000).expect("first acquire");
        }
        let _l2 = ForegroundLease::acquire(2000).expect("re-acquire after drop");
    }

    #[test]
    fn with_foreground_lease_returns_inner_value() {
        let v = with_foreground_lease(2000, || Ok(41 + 1)).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn second_acquire_from_another_thread_times_out_while_held() {
        let _held = ForegroundLease::acquire(2000).expect("hold lease");
        // A Win32 mutex is owned by the *thread*, so contention must be tested
        // from a second thread, not a second acquire on this one.
        // `ForegroundLease` is !Send (it owns a thread-affine HANDLE), so the worker
        // must collapse the result to a plain bool before handing it back.
        let timed_out = std::thread::spawn(|| {
            matches!(
                ForegroundLease::acquire(50),
                Err(CoreError::ForegroundBusy { .. })
            )
        })
        .join()
        .unwrap();
        assert!(timed_out, "contended acquire should have timed out");
    }
}
