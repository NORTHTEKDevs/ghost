//! Global event bus for system UI events. Provides event-driven wakeups for
//! Ghost wait primitives so they don't have to poll.
//!
//! Currently subscribes to `EVENT_SYSTEM_FOREGROUND` via `SetWinEventHook`.
//! A dedicated OS thread runs the Win32 message pump that drives the hook
//! callbacks; events bump a sequence counter and notify any tokio task that
//! is awaiting `wait_for_change`.
//!
//! Future work: subscribe to UIA structure-changed and focus-changed events
//! via `IUIAutomation::AddXxxEventHandler` for fuller coverage.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND,
    MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::Win32::Foundation::HWND;

pub struct EventBus {
    seq: AtomicU64,
    notify: Notify,
}

static GLOBAL_BUS: OnceLock<&'static EventBus> = OnceLock::new();

impl EventBus {
    /// Get the process-global event bus; lazily spawns the pump thread on first use.
    pub fn global() -> &'static EventBus {
        GLOBAL_BUS.get_or_init(|| {
            let bus: &'static EventBus = Box::leak(Box::new(EventBus {
                seq: AtomicU64::new(0),
                notify: Notify::new(),
            }));
            thread::Builder::new()
                .name("ghost-event-pump".into())
                .spawn(move || pump_thread(bus))
                .expect("failed to spawn ghost-event-pump thread");
            bus
        })
    }

    /// Current event sequence. Increments monotonically on each foreground change.
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Bump the sequence counter and notify any waiters. Called from event
    /// callbacks (SetWinEventHook + UIA event handlers). Sources are coalesced.
    pub fn bump(&self) {
        self.seq.fetch_add(1, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    /// Wait until seq advances past `since_seq` or the timeout elapses.
    /// Returns Ok(new_seq) on event, Err(()) on timeout.
    /// Loops on spurious wakeups: re-checks seq after each notify; only returns
    /// Ok when seq has actually advanced past `since_seq`.
    pub async fn wait_for_change(&self, since_seq: u64, timeout_ms: u64) -> Result<u64, ()> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let now = self.seq();
            if now > since_seq {
                return Ok(now);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(());
            }
            match timeout(remaining, self.notify.notified()).await {
                Ok(()) => continue,
                Err(_) => return Err(()),
            }
        }
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_thread: u32,
    _time: u32,
) {
    if let Some(&bus) = GLOBAL_BUS.get() {
        bus.seq.fetch_add(1, Ordering::Relaxed);
        bus.notify.notify_waiters();
    }
}

fn pump_thread(_bus: &'static EventBus) {
    unsafe {
        let _hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0);
            if !r.as_bool() {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
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
    async fn wait_returns_immediately_when_seq_advanced() {
        let bus = EventBus::global();
        let start = bus.seq();
        // Manually bump seq + notify to simulate an event.
        bus.seq.fetch_add(1, Ordering::Relaxed);
        bus.notify.notify_waiters();
        let r = bus.wait_for_change(start, 100).await;
        assert!(r.is_ok(), "expected immediate Ok when seq already advanced");
    }

    #[tokio::test]
    async fn wait_times_out_when_no_events() {
        let bus = EventBus::global();
        let cur = bus.seq();
        // Drain any background-driven advances by capturing the latest seq.
        let r = bus.wait_for_change(cur + 100_000, 50).await;
        assert!(r.is_err(), "expected timeout when no event arrives");
    }
}
