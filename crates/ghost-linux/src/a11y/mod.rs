//! AT-SPI2 accessibility layer.
//!
//! Aliased as `uia` at the crate root so `ghost-session` and `ghost-mcp` can
//! refer to it by the same path they use for the Windows engine.
//!
//! [`roles`], [`patterns`] and [`handles`] are platform-independent and compile
//! everywhere so their parity tests can run on any host; the modules that speak
//! D-Bus are gated to Linux.

pub mod handles;
pub mod patterns;
pub mod roles;

#[cfg(target_os = "linux")]
pub mod element;
#[cfg(target_os = "linux")]
pub mod ops;
#[cfg(target_os = "linux")]
pub mod tree;

pub use handles::{intern, resolve, ObjAddr};
pub use roles::{role_alias_matches, role_id_to_name, INTERACTIVE_ROLES};

#[cfg(target_os = "linux")]
pub use element::{A11yElement, BoundingRect, ElementDescriptor, UiaElement};
#[cfg(target_os = "linux")]
pub use tree::{focus_window_under_point, A11yTree, UiaTree, WindowInfo, WindowState};

#[cfg(target_os = "linux")]
mod live {
    use std::sync::{Arc, OnceLock};

    use crate::bridge::A11yBridge;

    /// The process-wide accessibility connection.
    ///
    /// One connection per process is both sufficient and correct: AT-SPI is a
    /// bus protocol, and opening a second connection would double the traffic
    /// while giving no isolation. Created on first use so that merely linking
    /// the crate never touches D-Bus.
    pub fn global_bridge() -> crate::error::Result<Arc<A11yBridge>> {
        static BRIDGE: OnceLock<std::result::Result<Arc<A11yBridge>, String>> = OnceLock::new();
        BRIDGE
            .get_or_init(|| A11yBridge::new().map(Arc::new).map_err(|e| e.to_string()))
            .as_ref()
            .map(Arc::clone)
            .map_err(|e| crate::error::CoreError::platform(e.clone()))
    }

    /// Mirrors `ghost_core::uia::init_com`.
    ///
    /// Windows must initialise COM before any UIA call. Linux has no
    /// equivalent -- the accessibility connection is established lazily by the
    /// bridge thread -- so this is a no-op that exists purely to keep shared
    /// code identical.
    pub fn init_com() -> crate::error::Result<ComGuard> {
        Ok(ComGuard::new())
    }

    /// Mirrors `ghost_core::uia::ComGuard`.
    ///
    /// On Windows this is an RAII guard that calls `CoUninitialize` on drop, so
    /// COM lives exactly as long as the session. Linux has no apartment model;
    /// the accessibility connection is owned by the bridge thread and closes
    /// with the process. This exists so `GhostSession` can hold the same field
    /// on both platforms.
    pub struct ComGuard {
        _private: (),
    }

    impl ComGuard {
        pub fn new() -> Self {
            Self { _private: () }
        }
    }

    impl Default for ComGuard {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Mirrors `ghost_core::uia::EventBus`.
    ///
    /// Windows pumps UIA focus-change events to bump a sequence number that
    /// `ghost_wait` uses to detect UI activity. The Linux engine does not
    /// subscribe to AT-SPI signals yet, so the sequence never advances on its
    /// own and waiters fall back to their polling path -- which is how they
    /// behave on Windows when no event arrives either. `bump` still works, so
    /// callers that advance it explicitly behave identically.
    pub struct EventBus {
        seq: std::sync::atomic::AtomicU64,
    }

    static GLOBAL_BUS: OnceLock<&'static EventBus> = OnceLock::new();

    /// The wait timed out without the sequence advancing (named so the
    /// signature matches the Windows engine and avoids `Result<_, ()>`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WaitTimeout;

    impl EventBus {
        pub fn global() -> &'static EventBus {
            GLOBAL_BUS.get_or_init(|| {
                Box::leak(Box::new(EventBus { seq: std::sync::atomic::AtomicU64::new(0) }))
            })
        }

        pub fn seq(&self) -> u64 {
            self.seq.load(std::sync::atomic::Ordering::Relaxed)
        }

        pub fn bump(&self) {
            self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        /// Wait for the sequence to move past `since_seq`, or time out.
        ///
        /// Signature matches the Windows engine's. Because the Linux engine does
        /// not yet subscribe to AT-SPI focus signals, this normally runs to its
        /// deadline and the caller falls back to polling -- the same path
        /// Windows takes when no event arrives.
        pub async fn wait_for_change(&self, since_seq: u64, timeout_ms: u64) -> Result<u64, WaitTimeout> {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            loop {
                let now = self.seq();
                if now != since_seq {
                    return Ok(now);
                }
                if std::time::Instant::now() >= deadline {
                    return Err(WaitTimeout);
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use live::{global_bridge, init_com, ComGuard, EventBus};
