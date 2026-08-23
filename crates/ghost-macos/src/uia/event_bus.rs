//! Event-driven wakeups for `ghost_wait`/act-then-verify polling loops.
//!
//! On Windows this is backed by `SetWinEventHook`, a real OS event source
//! that bumps the sequence counter on foreground/focus/value changes. macOS
//! has no such hook wired up here (it would be an Accessibility notification
//! observer, part of the native backend). What is NOT OS-dependent is the
//! counter and waiter machinery itself -- `AtomicU64` + `tokio::Notify` are
//! plain Rust -- so that part is real: `bump()` genuinely wakes every waiter,
//! `seq()` genuinely reflects how many times it was called. Nothing in this
//! crate calls `bump()` on its own (there is no OS source to call it), so in
//! practice every `wait_for_change` here runs to its timeout and the caller
//! falls back to polling -- the exact same path Windows takes when no event
//! arrives, not a new failure mode.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

pub struct EventBus {
    seq: AtomicU64,
    notify: Notify,
}

/// The wait timed out without the sequence advancing (named so the signature
/// avoids `Result<_, ()>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTimeout;

static GLOBAL_BUS: OnceLock<&'static EventBus> = OnceLock::new();

impl EventBus {
    pub fn global() -> &'static EventBus {
        GLOBAL_BUS.get_or_init(|| {
            Box::leak(Box::new(EventBus { seq: AtomicU64::new(0), notify: Notify::new() }))
        })
    }

    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    pub fn bump(&self) {
        self.seq.fetch_add(1, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub async fn wait_for_change(&self, since_seq: u64, timeout_ms: u64) -> Result<u64, WaitTimeout> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let now = self.seq();
            if now > since_seq {
                return Ok(now);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(WaitTimeout);
            }
            match timeout(remaining, self.notify.notified()).await {
                Ok(()) => continue,
                Err(_) => return Err(WaitTimeout),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_bus_singleton_returns_same_instance() {
        let a = EventBus::global() as *const _;
        let b = EventBus::global() as *const _;
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_seq_already_advanced() {
        let bus = EventBus::global();
        let start = bus.seq();
        bus.bump();
        let r = bus.wait_for_change(start, 100).await;
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn wait_times_out_with_no_bump() {
        let bus = EventBus::global();
        let cur = bus.seq();
        let r = bus.wait_for_change(cur + 1_000_000, 30).await;
        assert!(r.is_err());
    }
}
