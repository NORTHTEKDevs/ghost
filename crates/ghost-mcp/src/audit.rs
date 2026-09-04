//! Built-in interference audit: proof, per session, that Ghost never took the
//! human's foreground.
//!
//! `ghost verify` checks the claim once. This checks it continuously: a sampler
//! reads the foreground window and `GetLastInputInfo` every 100 ms; a foreground
//! change that happens with no real hardware input in the last 1.5 s cannot have
//! been the human, so it is recorded as a SYNTHETIC incident together with the
//! tool calls in flight at that moment. `ghost_stats` and `ghost_session_state`
//! report the tally. Ghost does not grade itself: the sampler is independent of
//! every dispatch path and cannot be told to look away.
//!
//! Windows only (foreground and last-input are Win32 concepts); `GHOST_AUDIT=off`
//! disables the sampler.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::time::Duration;
use std::time::Instant;
#[cfg(any(windows, test))]
use std::time::{SystemTime, UNIX_EPOCH};

/// Real input within this window of a foreground change attributes the change
/// to the human. Matches the independent observer used during development.
#[cfg(any(windows, test))]
const HUMAN_INPUT_WINDOW_MS: u64 = 1_500;
#[cfg(windows)]
const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const MAX_RECENT: usize = 20;

/// One synthetic foreground change.
#[derive(Debug, Clone)]
pub struct Incident {
    pub epoch_ms: u64,
    pub from_hwnd: isize,
    pub to_hwnd: isize,
    pub to_title: String,
    pub idle_ms: u64,
    pub in_flight: Vec<String>,
}

#[derive(Default)]
struct State {
    samples: u64,
    human_changes: u64,
    incidents: Vec<Incident>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static IN_FLIGHT: OnceLock<Mutex<HashMap<u64, (String, Instant)>>> = OnceLock::new();
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn in_flight() -> &'static Mutex<HashMap<u64, (String, Instant)>> {
    IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn enabled() -> bool {
    cfg!(windows)
        && !matches!(std::env::var("GHOST_AUDIT"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
}

/// Marks a tool call as in flight for as long as the guard lives.
pub struct InFlight(u64);

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// Register a tool call; drop the guard when it answers.
pub fn begin(tool: &str) -> InFlight {
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    in_flight()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(ticket, (tool.to_string(), Instant::now()));
    InFlight(ticket)
}

#[cfg(any(windows, test))]
fn in_flight_names() -> Vec<String> {
    let mut v: Vec<String> = in_flight()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .map(|(n, _)| n.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The attribution rule, kept pure so it is unit-tested: a foreground change is
/// the human's when real input arrived within the window, synthetic otherwise.
#[cfg(any(windows, test))]
pub fn classify(prev_hwnd: isize, hwnd: isize, idle_ms: u64) -> Option<bool> {
    if prev_hwnd == hwnd {
        return None;
    }
    Some(idle_ms >= HUMAN_INPUT_WINDOW_MS)
}

/// Record one sample. Returns the incident if this sample was a synthetic change.
#[cfg(any(windows, test))]
fn observe(prev: &mut Option<isize>, hwnd: isize, title: String, idle_ms: u64) -> Option<Incident> {
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
    st.samples += 1;
    let last = prev.replace(hwnd)?;
    match classify(last, hwnd, idle_ms) {
        None => None,
        Some(false) => {
            st.human_changes += 1;
            None
        }
        Some(true) => {
            let incident = Incident {
                epoch_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                from_hwnd: last,
                to_hwnd: hwnd,
                to_title: title,
                idle_ms,
                in_flight: in_flight_names(),
            };
            st.incidents.push(incident.clone());
            if st.incidents.len() > MAX_RECENT {
                let excess = st.incidents.len() - MAX_RECENT;
                st.incidents.drain(0..excess);
            }
            Some(incident)
        }
    }
}

/// The tally, as reported by `ghost_stats` and `ghost_session_state`.
pub fn snapshot() -> Value {
    let st = state().lock().unwrap_or_else(|p| p.into_inner());
    json!({
        "enabled": enabled(),
        "samples": st.samples,
        "human_foreground_changes": st.human_changes,
        "synthetic_foreground_changes": st.incidents.len(),
        "incidents": st.incidents.iter().rev().take(MAX_RECENT).map(|i| json!({
            "epoch_ms": i.epoch_ms,
            "from_hwnd": i.from_hwnd,
            "to_hwnd": i.to_hwnd,
            "to_title": i.to_title,
            "idle_ms": i.idle_ms,
            "in_flight": i.in_flight,
        })).collect::<Vec<_>>(),
        "rule": "a foreground change with no real hardware input in the previous 1.5 s is synthetic (GetLastInputInfo); tool calls in flight at that moment are listed",
    })
}

/// Start the sampler thread. Idempotent; a no-op when disabled or off Windows.
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if !enabled() {
        return;
    }
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("ghost-audit".into())
            .spawn(sampler);
    });
}

#[cfg(windows)]
fn sampler() {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    let mut prev: Option<isize> = None;
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut lii = LASTINPUTINFO {
                cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            let idle_ms = if GetLastInputInfo(&mut lii).as_bool() {
                GetTickCount().wrapping_sub(lii.dwTime) as u64
            } else {
                0
            };
            let h = hwnd.0 as isize;
            let title = if prev.map(|p| p != h).unwrap_or(true) {
                let mut buf = [0u16; 256];
                let n = GetWindowTextW(hwnd, &mut buf);
                String::from_utf16_lossy(&buf[..n.max(0) as usize])
            } else {
                String::new()
            };
            if let Some(incident) = observe(&mut prev, h, title, idle_ms) {
                tracing::warn!(
                    to = %incident.to_title,
                    idle_ms = incident.idle_ms,
                    in_flight = ?incident.in_flight,
                    "audit: SYNTHETIC foreground change"
                );
            }
        }
    }
}

#[cfg(not(windows))]
fn sampler() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_attributes_by_recent_input() {
        assert_eq!(classify(1, 1, 0), None, "no change is no event");
        assert_eq!(classify(1, 2, 200), Some(false), "input 200 ms ago: the human");
        assert_eq!(classify(1, 2, 1_499), Some(false));
        assert_eq!(classify(1, 2, 1_500), Some(true), "no input for 1.5 s: synthetic");
        assert_eq!(classify(1, 2, 60_000), Some(true));
    }

    #[test]
    fn observe_counts_and_records_in_flight_tools() {
        let mut prev = None;
        assert!(observe(&mut prev, 10, "a".into(), 5_000).is_none(), "first sample sets the baseline");
        assert!(observe(&mut prev, 10, String::new(), 5_000).is_none());
        assert!(observe(&mut prev, 11, "human".into(), 100).is_none());
        let guard = begin("ghost_window");
        let inc = observe(&mut prev, 12, "stolen".into(), 9_000).expect("synthetic change");
        assert_eq!(inc.to_title, "stolen");
        assert_eq!(inc.in_flight, vec!["ghost_window".to_string()]);
        drop(guard);
        assert!(in_flight_names().is_empty());
        let snap = snapshot();
        assert!(snap["synthetic_foreground_changes"].as_u64().unwrap() >= 1);
        assert!(snap["human_foreground_changes"].as_u64().unwrap() >= 1);
        assert_eq!(snap["incidents"][0]["to_title"], "stolen");
    }
}
