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
//! The sampler also ACTS. A per-call foreground guard (see
//! `GhostSession::foreground_guard_end_within`) can only watch while the call
//! runs, and a Chromium window activates itself again after the verb has
//! answered - measured 2026-09-06 on a real desktop: the browser took the
//! human's foreground between calls and held it for 30 s while their keystrokes
//! went into the web page. So the sampler is also a sentinel: while the policy
//! is `background`, a foreground change to a window Ghost is driving that the
//! human did not make is handed straight back. It stands down the moment the
//! human takes that window themselves, and after a few rounds of losing.
//!
//! Windows only (foreground and last-input are Win32 concepts); `GHOST_AUDIT=off`
//! disables the sampler, and with it the sentinel.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
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
/// While a window Ghost is driving could still activate itself, the foreground
/// is checked this often instead: what a human could notice is one sample
/// interval plus the hand-back, so it is deliberately small.
#[cfg(windows)]
const WATCH_INTERVAL: Duration = Duration::from_millis(25);
const MAX_RECENT: usize = 20;

/// How long after a call the window it drove stays protected. Chromium's
/// self-activation lands within ~90 ms of the UIA call, but a page still
/// loading can activate later; this covers the tail without keeping the
/// sentinel alive while Ghost is idle (once it lapses, the human can click into
/// that window and stay there).
const PROTECT_MS: u64 = 6_000;
/// This many hand-backs inside `FIGHT_WINDOW` means something is taking the
/// foreground faster than Ghost can give it back. Stand down rather than
/// fight: a flickering desktop is worse than a window that stays up.
///
/// The budget is generous because the event hook makes a hand-back cost about
/// a millisecond of detection: fourteen rounds take well under half a second,
/// and a window that loses fourteen times in that span is genuinely stuck.
/// With the old 25 ms polling this had to be small, and the pause that
/// followed handed the screen over for seconds (measured: 2.6 s and 3.6 s).
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_CONSECUTIVE_RESTORES: u32 = 14;
/// Hand-backs further apart than this are separate events, not a fight.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
const FIGHT_WINDOW: Duration = Duration::from_millis(1_500);
/// How long a fight pauses the sentinel before it tries again. Short on
/// purpose: a pause is time the other window keeps the screen.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
const STAND_DOWN: Duration = Duration::from_millis(600);

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
#[cfg_attr(not(windows), allow(dead_code))]
struct State {
    samples: u64,
    human_changes: u64,
    incidents: Vec<Incident>,
    /// Changes classed as the human's (real input in the last 1.5 s) whose
    /// target is a window Ghost drives, made with no click or Alt+Tab behind
    /// them: Ghost's window took the foreground while the human typed. The
    /// synthetic rule cannot see these - GetLastInputInfo counts the human's
    /// typing - and `verify` used to miss them (round 1, C6).
    during_input: Vec<Incident>,
    /// The window the human most recently chose themselves (a foreground
    /// change with real input behind it). The foreground guard hands the
    /// foreground back here when the window it recorded before a verb was
    /// already one of the target's own popups.
    last_human_hwnd: isize,
    /// Foreground thefts the sentinel undid, and the ones it could not.
    restored: u64,
    restore_failures: u64,
    /// Hand-backs in quick succession, and whether the sentinel has given up.
    /// Only a BURST counts as a fight: a window that activates itself once per
    /// verb, is put back, and stays put is the normal case and must not be
    /// allowed to exhaust the budget.
    consecutive: u32,
    last_restore: Option<Instant>,
    /// The most recent foreground window that was NOT one Ghost is driving:
    /// where the foreground goes back to. Shared state, because two things
    /// watch the foreground now - an event hook that reacts in about a
    /// millisecond, and a poller behind it as a safety net.
    last_free_hwnd: isize,
    /// When set, the sentinel is not acting until this moment passes. A
    /// stand-down is a pause, never a surrender: an earlier version stood down
    /// permanently until real human input arrived, and a single burst at the
    /// end of one phase then let a browser hold the screen for the rest of the
    /// run (measured 2026-09-06, 5.9 s and 46 of the human's keystrokes).
    stood_down_until: Option<Instant>,
}

/// A window Ghost is driving, protected until `until`.
#[derive(Clone, Copy)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Protected {
    hwnd: isize,
    pid: u32,
    until: Instant,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static IN_FLIGHT: OnceLock<Mutex<HashMap<u64, (String, Instant)>>> = OnceLock::new();
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
static PROTECTED: OnceLock<Mutex<Vec<Protected>>> = OnceLock::new();
/// Every window (and process) Ghost has driven this session. Protection
/// EXPIRES, and when it did, the window that had stolen the foreground was
/// being recorded as "somewhere the human might be" and then used as the
/// hand-back destination - which silenced the sentinel for the rest of the run
/// (measured 2026-09-06: 5.8 s of held foreground and 46 of the human's
/// keystrokes into a web page). A window Ghost has driven is never that place
/// unless the human clicks into it themselves.
static EVER_DRIVEN: OnceLock<Mutex<Vec<(isize, u32)>>> = OnceLock::new();
static LAST_CALL_END: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

fn protected() -> &'static Mutex<Vec<Protected>> {
    PROTECTED.get_or_init(|| Mutex::new(Vec::new()))
}

fn ever_driven() -> &'static Mutex<Vec<(isize, u32)>> {
    EVER_DRIVEN.get_or_init(|| Mutex::new(Vec::new()))
}

/// Whether Ghost has driven this window, or another window of its process, at
/// any point this session.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
fn is_ever_driven(hwnd: isize, pid_of: impl Fn(isize) -> u32) -> bool {
    if hwnd == 0 {
        return false;
    }
    let list = ever_driven().lock().unwrap_or_else(|p| p.into_inner());
    if list.iter().any(|(h, _)| *h == hwnd) {
        return true;
    }
    drop(list);
    let pid = pid_of(hwnd);
    pid != 0
        && ever_driven()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|(_, p)| *p == pid)
}

fn last_call_end() -> &'static Mutex<Option<Instant>> {
    LAST_CALL_END.get_or_init(|| Mutex::new(None))
}

/// Register the window a tool call is about to drive. The sentinel hands the
/// foreground back if this window, or any window of its process, takes it
/// without the human. Repeated calls extend the protection.
pub fn protect(hwnd: isize, pid: u32) {
    if hwnd == 0 || !enabled() {
        return;
    }
    let until = Instant::now() + Duration::from_millis(PROTECT_MS);
    let mut list = protected().lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    list.retain(|p| p.until > now && p.hwnd != hwnd);
    list.push(Protected { hwnd, pid, until });
    drop(list);
    {
        let mut seen = ever_driven().lock().unwrap_or_else(|p| p.into_inner());
        if !seen.iter().any(|(h, p)| *h == hwnd && *p == pid) {
            seen.push((hwnd, pid));
            // One agent drives a handful of windows; keep the memory bounded.
            if seen.len() > 32 {
                seen.remove(0);
            }
        }
    }
    let mut list = protected().lock().unwrap_or_else(|p| p.into_inner());
    // A handful of targets is plenty; an agent drives one or two windows.
    if list.len() > 4 {
        let excess = list.len() - 4;
        list.drain(0..excess);
    }
}

/// Whether a window belongs to something Ghost is driving right now.
/// `pid_of` resolves a handle's process (injected so the rule is testable).
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
fn is_protected(hwnd: isize, now: Instant, pid_of: impl Fn(isize) -> u32) -> bool {
    if hwnd == 0 {
        return false;
    }
    // A call still running keeps its target protected however long it takes.
    // Protection is a fixed span after a call STARTS, and a verb that runs
    // longer than that span (a navigate waiting on a page: 15 s) would
    // otherwise go unprotected in the middle of its own work - measured
    // 2026-09-06, a browser held the foreground for 15.9 s inside one call.
    let in_flight = !in_flight_names().is_empty();
    let list = protected().lock().unwrap_or_else(|p| p.into_inner());
    let live: Vec<Protected> = list
        .iter()
        .copied()
        .filter(|p| p.until > now || in_flight)
        .collect();
    drop(list);
    if live.iter().any(|p| p.hwnd == hwnd) {
        return true;
    }
    let pid = pid_of(hwnd);
    pid != 0 && live.iter().any(|p| p.pid == pid)
}

/// The decision the sentinel makes, kept pure so it is unit-tested.
///
/// A window Ghost is driving has the foreground. Hand it back unless the human
/// actually chose that window - which they do by clicking it or alt-tabbing to
/// it, never by typing somewhere else (see `realinput::human_chose_a_window`).
///
/// `hooks_live` is whether the real-input hooks are installed. Without them
/// there is no way to tell a human's key from a program's, so the sentinel does
/// nothing rather than risk yanking a window from under someone.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub fn should_hand_back(
    to_is_protected: bool,
    hooks_live: bool,
    human_chose_it: bool,
    stood_down: bool,
) -> bool {
    to_is_protected && hooks_live && !human_chose_it && !stood_down
}

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
        *last_call_end().lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
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
#[cfg_attr(not(windows), allow(dead_code))]
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

/// Whether a human-classed foreground change was really Ghost's: its target is
/// a window Ghost drives and the input hooks saw no click or Alt+Tab choosing
/// it. `chose` is `None` when the hooks are not installed - then nothing is
/// claimed either way.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub fn taken_during_input(to_is_ghosts: bool, chose: Option<bool>) -> bool {
    to_is_ghosts && chose == Some(false)
}

/// The attribution rule, kept pure so it is unit-tested: a foreground change is
/// the human's when real input arrived within the window, synthetic otherwise.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub fn classify(prev_hwnd: isize, hwnd: isize, idle_ms: u64) -> Option<bool> {
    if prev_hwnd == hwnd {
        return None;
    }
    Some(idle_ms >= HUMAN_INPUT_WINDOW_MS)
}

/// Record one sample. Returns the incident if this sample was a synthetic change.
///
/// `to_is_protected` says the window taking the foreground is one Ghost is
/// driving. It matters because `GetLastInputInfo` counts SYNTHETIC input too:
/// while a human types, every foreground change looks like theirs, so a browser
/// that activates itself mid-keystroke would otherwise be recorded as the
/// window the human chose - and the sentinel would then hand the foreground
/// "back" to the thief. Measured 2026-09-06: that is exactly how a browser kept
/// the foreground for 28 s while the human typed.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
fn observe(
    prev: &mut Option<isize>,
    hwnd: isize,
    title: String,
    idle_ms: u64,
    to_is_protected: bool,
    chose: Option<bool>,
) -> Option<Incident> {
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
    st.samples += 1;
    let last = prev.replace(hwnd)?;
    match classify(last, hwnd, idle_ms) {
        None => None,
        Some(false) => {
            st.human_changes += 1;
            if !to_is_protected {
                st.last_human_hwnd = hwnd;
            }
            if taken_during_input(to_is_protected, chose) {
                st.during_input.push(Incident {
                    epoch_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                    from_hwnd: last,
                    to_hwnd: hwnd,
                    to_title: title,
                    idle_ms,
                    in_flight: in_flight_names(),
                });
                if st.during_input.len() > MAX_RECENT {
                    let excess = st.during_input.len() - MAX_RECENT;
                    st.during_input.drain(0..excess);
                }
            }
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

/// The last window the human chose themselves, or 0 before any human change
/// has been seen this session.
pub fn last_human_foreground() -> isize {
    state().lock().unwrap_or_else(|p| p.into_inner()).last_human_hwnd
}

/// Hand the foreground back to `dest` after a protected window took it.
/// Returns whether `dest` holds the foreground afterwards.
#[cfg(windows)]
fn hand_back(dest: isize, taken_by: isize, title: &str) -> bool {
    // A window that takes the foreground straight back after a hand-back is in
    // a race Ghost cannot win by raising the human's window again - and every
    // round of it flickers the desktop. Take it out of the race instead: put
    // the thief behind the human's work, without activating or moving it.
    let repeat = {
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        st.last_restore.map(|t| t.elapsed() <= FIGHT_WINDOW).unwrap_or(false)
    };
    if repeat {
        let lowered = ghost_session::engine::uia::tree::send_to_back(taken_by);
        tracing::debug!(taken_by, lowered, "sentinel: repeat offender pushed to the back");
    }
    let mut ok = ghost_session::engine::uia::tree::ensure_foreground(dest, 250).unwrap_or(false);
    if !ok {
        std::thread::sleep(Duration::from_millis(40));
        ok = ghost_session::engine::uia::tree::ensure_foreground(dest, 250).unwrap_or(false);
    }
    // The hand-back must never leave the human's window hidden.
    let _ = ghost_session::engine::uia::tree::restore_if_hidden(dest);
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
    if ok {
        st.restored += 1;
        let now = Instant::now();
        let burst = st.last_restore.map(|t| now.duration_since(t) <= FIGHT_WINDOW).unwrap_or(false);
        st.consecutive = if burst { st.consecutive + 1 } else { 1 };
        st.last_restore = Some(now);
        if st.consecutive >= MAX_CONSECUTIVE_RESTORES {
            st.stood_down_until = Some(now + STAND_DOWN);
            st.consecutive = 0;
            tracing::warn!(
                window = %title,
                "sentinel: {taken_by:#x} is taking the foreground faster than it can be handed back; pausing {} ms before trying again",
                STAND_DOWN.as_millis()
            );
        }
    } else {
        st.restore_failures += 1;
    }
    drop(st);
    tracing::info!(
        taken_by = taken_by,
        window = %title,
        handed_back_to = dest,
        ok,
        "sentinel: a window Ghost is driving took the foreground; handed it back"
    );
    ok
}

/// The tally, as reported by `ghost_stats` and `ghost_session_state`.
pub fn snapshot() -> Value {
    let st = state().lock().unwrap_or_else(|p| p.into_inner());
    json!({
        "enabled": enabled(),
        "samples": st.samples,
        "human_foreground_changes": st.human_changes,
        "synthetic_foreground_changes": st.incidents.len(),
        "foreground_handed_back": st.restored,
        "foreground_hand_back_failures": st.restore_failures,
        "sentinel_paused": st.stood_down_until.map(|t| t > Instant::now()).unwrap_or(false),
        "incidents": st.incidents.iter().rev().take(MAX_RECENT).map(|i| json!({
            "epoch_ms": i.epoch_ms,
            "from_hwnd": i.from_hwnd,
            "to_hwnd": i.to_hwnd,
            "to_title": i.to_title,
            "idle_ms": i.idle_ms,
            "in_flight": i.in_flight,
        })).collect::<Vec<_>>(),
        "foreground_taken_during_input": st.during_input.len(),
        "during_input_incidents": st.during_input.iter().rev().take(MAX_RECENT).map(|i| json!({
            "epoch_ms": i.epoch_ms,
            "from_hwnd": i.from_hwnd,
            "to_hwnd": i.to_hwnd,
            "to_title": i.to_title,
            "idle_ms": i.idle_ms,
            "in_flight": i.in_flight,
        })).collect::<Vec<_>>(),
        "rule": "a foreground change with no real hardware input in the previous 1.5 s is synthetic (GetLastInputInfo); tool calls in flight at that moment are listed. A change with real input behind it that lands on a window Ghost drives, with no real click or Alt+Tab choosing it, is listed under during_input_incidents",
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
        #[cfg(windows)]
        {
            let _ = std::thread::Builder::new()
                .name("ghost-fg-hook".into())
                .spawn(foreground_hook);
        }
    });
}

/// One look at the foreground: is a window Ghost drives holding it when it
/// should not be, and if so, give it back. Returns whether it handed back.
///
/// Called from two places on purpose. The event hook below reaches it about a
/// millisecond after the foreground moves, which is what keeps a keystroke from
/// landing in the wrong window; the poller reaches it every 25 ms as a safety
/// net, because a hook can be dropped and because the question is
/// level-triggered - a window that took the foreground and KEPT it must still
/// be handed back, and no further event will ever fire for it.
#[cfg(windows)]
fn consider_foreground(h: isize, title_hint: Option<String>) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    if h == 0 {
        return false;
    }
    let now = Instant::now();
    let pid_of = ghost_session::engine::system::window_pid;
    let chose = crate::realinput::human_chose_a_window(
        crate::realinput::since_real_click(),
        crate::realinput::since_real_alt(),
    );
    if !is_protected(h, now, pid_of) {
        // Somewhere the human may legitimately be: remember it as the place to
        // hand the foreground back to. A window Ghost has driven only counts
        // once the human has actually chosen it - protection EXPIRES, and
        // without this the thief becomes its own hand-back destination.
        if chose || !is_ever_driven(h, pid_of) {
            state().lock().unwrap_or_else(|p| p.into_inner()).last_free_hwnd = h;
        }
        return false;
    }
    if !ghost_session::engine::focus::is_background_only() {
        return false;
    }
    let (stood_down, last_human, last_free) = {
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        (
            st.stood_down_until.map(|t| t > now).unwrap_or(false),
            st.last_human_hwnd,
            st.last_free_hwnd,
        )
    };
    if !should_hand_back(true, crate::realinput::watching(), chose, stood_down) {
        return false;
    }
    let dest = if last_free != 0 && last_free != h { last_free } else { last_human };
    if dest == 0 || dest == h {
        return false;
    }
    let title = title_hint.filter(|t| !t.is_empty()).unwrap_or_else(|| unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(HWND(h as *mut core::ffi::c_void), &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    });
    hand_back(dest, h, &title);
    true
}

/// The hook's queue: one handle per foreground change, handled OFF the hook
/// thread so a hand-back (which can take a few hundred milliseconds) never
/// stalls delivery of the next event.
#[cfg(windows)]
static HOOK_TX: OnceLock<Mutex<Option<std::sync::mpsc::Sender<isize>>>> = OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn on_foreground(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _ms: u32,
) {
    // idObject 0 is the window itself; carets and other children raise this
    // event too and are not foreground changes.
    if id_object != 0 {
        return;
    }
    if let Some(lock) = HOOK_TX.get() {
        if let Ok(guard) = lock.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(hwnd.0 as isize);
            }
        }
    }
}

/// Watch the foreground by EVENT rather than by polling.
///
/// Polling at 25 ms means up to 25 ms of the wrong window in front before Ghost
/// even looks, and someone typing at 25 characters a second lands a key in that
/// gap - measured, one key per run. `EVENT_SYSTEM_FOREGROUND` arrives about a
/// millisecond after the change instead.
#[cfg(windows)]
fn foreground_hook() {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG,
        WINEVENT_OUTOFCONTEXT,
    };
    let (tx, rx) = std::sync::mpsc::channel::<isize>();
    if HOOK_TX.set(Mutex::new(Some(tx))).is_err() {
        return;
    }
    // The worker: everything slow happens here, never in the callback.
    let _ = std::thread::Builder::new()
        .name("ghost-sentinel".into())
        .spawn(move || {
            while let Ok(h) = rx.recv() {
                consider_foreground(h, None);
            }
        });
    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            tracing::warn!(
                "foreground event hook could not be installed; the sentinel falls back to polling"
            );
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(windows)]
fn sampler() {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    let mut prev: Option<isize> = None;
    loop {
        // Fast while a window Ghost drove could still activate itself.
        let watching = {
            let now = Instant::now();
            let list = protected().lock().unwrap_or_else(|p| p.into_inner());
            list.iter().any(|p| p.until > now)
        };
        std::thread::sleep(if watching { WATCH_INTERVAL } else { SAMPLE_INTERVAL });
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
            let changed = prev.map(|p| p != h).unwrap_or(true);
            let title = if changed {
                let mut buf = [0u16; 256];
                let n = GetWindowTextW(hwnd, &mut buf);
                String::from_utf16_lossy(&buf[..n.max(0) as usize])
            } else {
                String::new()
            };
            // A real keystroke or click means the human is back: try again for
            // them. The burst counter is left alone - `FIGHT_WINDOW` ages it
            // out on its own, and clearing it here would make a fight while the
            // human is present impossible to detect.
            if crate::realinput::since_real_input()
                .map(|ms| ms < HUMAN_INPUT_WINDOW_MS)
                .unwrap_or(false)
            {
                let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
                if st.stood_down_until.is_some() {
                    st.stood_down_until = None;
                    st.consecutive = 0;
                }
            }
            let pid_of = ghost_session::engine::system::window_pid;
            // For "did the human choose this?", a window Ghost has EVER driven
            // is suspect, not just one currently protected: protection expires
            // while a thief still holds the screen.
            let to_is_ghosts = is_protected(h, Instant::now(), pid_of) || is_ever_driven(h, pid_of);
            let chose = crate::realinput::watching().then(|| {
                crate::realinput::human_chose_a_window(
                    crate::realinput::since_real_click(),
                    crate::realinput::since_real_alt(),
                )
            });
            if let Some(incident) = observe(&mut prev, h, title.clone(), idle_ms, to_is_ghosts, chose) {
                tracing::warn!(
                    to = %incident.to_title,
                    idle_ms = incident.idle_ms,
                    in_flight = ?incident.in_flight,
                    "audit: SYNTHETIC foreground change"
                );
            }
            if consider_foreground(h, Some(title)) {
                // The hand-back moved the foreground; read it back so the next
                // sample does not report it as a change the human made.
                prev = Some(GetForegroundWindow().0 as isize);
            }
        }
    }
}

#[cfg(not(windows))]
fn sampler() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here mutates the same process-global audit state, and cargo
    /// runs test functions on parallel threads.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn classify_attributes_by_recent_input() {
        let _serial = serial();
        assert_eq!(classify(1, 1, 0), None, "no change is no event");
        assert_eq!(classify(1, 2, 200), Some(false), "input 200 ms ago: the human");
        assert_eq!(classify(1, 2, 1_499), Some(false));
        assert_eq!(classify(1, 2, 1_500), Some(true), "no input for 1.5 s: synthetic");
        assert_eq!(classify(1, 2, 60_000), Some(true));
    }

    #[test]
    fn hand_back_unless_the_human_chose_that_window() {
        let _serial = serial();
        // Not a window Ghost drives: never touch the foreground.
        assert!(!should_hand_back(false, true, false, false));
        // The case this exists for: a driven window took the screen and the
        // human did not choose it (they were typing elsewhere, or away).
        assert!(should_hand_back(true, true, false, false));
        // The human clicked it or alt-tabbed to it: it is theirs now.
        assert!(!should_hand_back(true, true, true, false));
        // Stood down after a losing fight: never flicker the desktop.
        assert!(!should_hand_back(true, true, false, true));
        // Without the hooks, "the human did nothing" is not knowable: do nothing.
        assert!(!should_hand_back(true, false, false, false));
    }

    /// Windows-only: `protect` is a no-op where the audit does not run, so
    /// there is nothing to register and nothing to match.
    #[cfg(windows)]
    #[test]
    fn protection_matches_by_handle_and_by_process_and_expires() {
        let _serial = serial();
        let now = Instant::now();
        protect(0, 0); // ignored
        protect(4242, 77);
        assert!(is_protected(4242, now, |_| 0), "the window itself");
        assert!(is_protected(9999, now, |_| 77), "a popup of the same process");
        assert!(!is_protected(9999, now, |_| 5), "an unrelated window");
        assert!(!is_protected(0, now, |_| 77), "no window");
        let later = now + Duration::from_millis(PROTECT_MS + 1);
        assert!(!is_protected(4242, later, |_| 77), "protection lapses");
        protected().lock().unwrap().clear();
    }

    #[test]
    fn a_driven_window_never_becomes_the_hand_back_destination() {
        let _serial = serial();
        // GetLastInputInfo counts synthetic input, so while a human types every
        // foreground change looks like theirs. A window Ghost is driving must
        // still not be recorded as the window the human chose, or the sentinel
        // would hand the foreground back to the thief.
        let mut prev = None;
        observe(&mut prev, 700, "human's editor".into(), 50, false, None);
        observe(&mut prev, 701, "human's editor".into(), 50, false, None);
        assert_eq!(last_human_foreground(), 701);
        observe(&mut prev, 702, "the browser Ghost drives".into(), 50, true, None);
        assert_eq!(last_human_foreground(), 701, "the driven window must not overwrite it");
        observe(&mut prev, 703, "human's other window".into(), 50, false, None);
        assert_eq!(last_human_foreground(), 703);
    }

    #[test]
    fn taken_during_input_needs_a_ghost_window_and_no_click_or_alt_tab() {
        assert!(taken_during_input(true, Some(false)));
        assert!(!taken_during_input(true, Some(true)), "the human clicked it or alt-tabbed to it");
        assert!(!taken_during_input(true, None), "no hooks: no claim");
        assert!(!taken_during_input(false, Some(false)), "not a window Ghost drives");
    }

    #[test]
    fn a_ghost_window_taking_the_foreground_while_the_human_types_is_recorded() {
        let _serial = serial();
        let mut prev = None;
        observe(&mut prev, 900, "human's editor".into(), 50, false, Some(false));
        let before = snapshot()["foreground_taken_during_input"].as_u64().unwrap();
        // idle 50 ms: the human is typing, so the synthetic rule calls it theirs
        assert!(observe(&mut prev, 901, "ghost-driven".into(), 50, true, Some(false)).is_none());
        let snap = snapshot();
        assert_eq!(snap["foreground_taken_during_input"].as_u64().unwrap(), before + 1);
        assert_eq!(snap["during_input_incidents"][0]["to_hwnd"], 901);
        // the human deliberately switching to it is not recorded
        observe(&mut prev, 902, "human's editor".into(), 50, false, Some(false));
        observe(&mut prev, 901, "ghost-driven".into(), 50, true, Some(true));
        assert_eq!(snapshot()["foreground_taken_during_input"].as_u64().unwrap(), before + 1);
    }

    #[test]
    fn observe_counts_and_records_in_flight_tools() {
        let _serial = serial();
        let mut prev = None;
        assert!(observe(&mut prev, 10, "a".into(), 5_000, false, None).is_none(), "first sample sets the baseline");
        assert!(observe(&mut prev, 10, String::new(), 5_000, false, None).is_none());
        assert!(observe(&mut prev, 11, "human".into(), 100, false, None).is_none());
        let guard = begin("ghost_window");
        let inc = observe(&mut prev, 12, "stolen".into(), 9_000, false, None).expect("synthetic change");
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
