//! Window-manager control over X11 (EWMH).
//!
//! AT-SPI deliberately has no concept of minimising or maximising a window —
//! it describes *content*, not window management. That left
//! `ghost_window op=state` unsupported on Linux while it worked on Windows.
//!
//! X11 does have a standard for exactly this: EWMH. A client asks the window
//! manager to change a window's state by sending a `ClientMessage` to the root
//! window, which is how `wmctrl` and every taskbar do it. That closes the gap on
//! X11 and, because XWayland apps are real X11 clients, for XWayland windows
//! under a Wayland session too.
//!
//! # Bridging AT-SPI to X11
//!
//! Ghost's window handles are interned AT-SPI `(bus name, object path)` pairs,
//! not X11 window ids, so the two namespaces have to be matched up. The join is
//! **PID first, then title**: `_NET_CLIENT_LIST` gives every managed window,
//! `_NET_WM_PID` gives its owning process, and AT-SPI gives the PID of the
//! application that owns the accessible window. A process with one window is
//! then unambiguous; a process with several is disambiguated by title.
//!
//! PID-first matters. Matching on title alone would pick the wrong window
//! whenever two applications show the same text — "Untitled Document" across two
//! editors is the ordinary case, not a corner case.
//!
//! # Honesty
//!
//! Under a native Wayland session there is no root window and no EWMH, so every
//! operation here reports [`CoreError::Unsupported`] rather than silently doing
//! nothing. A caller must be able to tell "your window did not minimise" from
//! "I minimised it".

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::error::{CoreError, Result};
use crate::session::{session_kind, SessionKind};

/// `WM_CHANGE_STATE`'s iconic state, from ICCCM.
const ICONIC_STATE: u32 = 3;

/// `_NET_WM_STATE` actions.
const NET_WM_STATE_REMOVE: u32 = 0;
const NET_WM_STATE_ADD: u32 = 1;

struct Ewmh {
    conn: RustConnection,
    root: Window,
}

/// Atoms resolved once per connection.
struct Atoms {
    net_client_list: Atom,
    net_wm_pid: Atom,
    net_wm_name: Atom,
    net_wm_state: Atom,
    net_wm_state_max_vert: Atom,
    net_wm_state_max_horz: Atom,
    net_wm_state_hidden: Atom,
    net_active_window: Atom,
    net_close_window: Atom,
    wm_change_state: Atom,
    utf8_string: Atom,
}

impl Ewmh {
    fn open() -> Result<Self> {
        if session_kind() != SessionKind::X11 {
            return Err(CoreError::Unsupported(
                "window state control needs an X11 session. Native Wayland exposes no window \
                 management protocol to ordinary clients; XWayland applications are reachable \
                 only from an X11 session"
                    .into(),
            ));
        }
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| CoreError::platform(format!("X11 connect: {e}")))?;
        let root = conn.setup().roots[screen_num].root;
        Ok(Self { conn, root })
    }

    fn atom(&self, name: &str) -> Result<Atom> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(|e| CoreError::platform(format!("intern_atom {name}: {e}")))?
            .reply()
            .map_err(|e| CoreError::platform(format!("intern_atom {name} reply: {e}")))?
            .atom)
    }

    fn atoms(&self) -> Result<Atoms> {
        Ok(Atoms {
            net_client_list: self.atom("_NET_CLIENT_LIST")?,
            net_wm_pid: self.atom("_NET_WM_PID")?,
            net_wm_name: self.atom("_NET_WM_NAME")?,
            net_wm_state: self.atom("_NET_WM_STATE")?,
            net_wm_state_max_vert: self.atom("_NET_WM_STATE_MAXIMIZED_VERT")?,
            net_wm_state_max_horz: self.atom("_NET_WM_STATE_MAXIMIZED_HORZ")?,
            net_wm_state_hidden: self.atom("_NET_WM_STATE_HIDDEN")?,
            net_active_window: self.atom("_NET_ACTIVE_WINDOW")?,
            net_close_window: self.atom("_NET_CLOSE_WINDOW")?,
            wm_change_state: self.atom("WM_CHANGE_STATE")?,
            utf8_string: self.atom("UTF8_STRING")?,
        })
    }

    /// Every window the window manager currently manages.
    fn client_list(&self, a: &Atoms) -> Result<Vec<Window>> {
        let reply = self
            .conn
            .get_property(false, self.root, a.net_client_list, AtomEnum::WINDOW, 0, u32::MAX)
            .map_err(|e| CoreError::platform(format!("_NET_CLIENT_LIST: {e}")))?
            .reply()
            .map_err(|e| {
                CoreError::platform(format!(
                    "_NET_CLIENT_LIST reply: {e}. The window manager may not support EWMH"
                ))
            })?;
        Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
    }

    fn window_pid(&self, a: &Atoms, w: Window) -> Option<u32> {
        let r = self
            .conn
            .get_property(false, w, a.net_wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let pids: Vec<u32> = r.value32()?.collect();
        pids.first().copied()
    }

    fn window_title(&self, a: &Atoms, w: Window) -> Option<String> {
        // _NET_WM_NAME (UTF-8) is the modern property; fall back to WM_NAME.
        if let Ok(Ok(r)) = self
            .conn
            .get_property(false, w, a.net_wm_name, a.utf8_string, 0, 1024)
            .map(|c| c.reply())
        {
            if !r.value.is_empty() {
                return Some(String::from_utf8_lossy(&r.value).into_owned());
            }
        }
        let r = self
            .conn
            .get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if r.value.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&r.value).into_owned())
        }
    }

    /// Ask the window manager to do something, the EWMH way: a ClientMessage to
    /// the root window with substructure redirect/notify.
    fn send_root_message(&self, w: Window, type_: Atom, data: [u32; 5]) -> Result<()> {
        let ev = ClientMessageEvent::new(32, w, type_, data);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                ev,
            )
            .map_err(|e| CoreError::platform(format!("send_event: {e}")))?;
        self.conn.flush().map_err(|e| CoreError::platform(format!("x11 flush: {e}")))?;
        Ok(())
    }
}

/// Resolve an X11 window from the owning process id and (optionally) a title.
///
/// Returns `None` when the process manages no X11 window — normal for a native
/// Wayland application even under an X11-capable session.
fn find_window(e: &Ewmh, a: &Atoms, pid: u32, title: Option<&str>) -> Result<Option<Window>> {
    let clients = e.client_list(a)?;

    let by_pid: Vec<Window> =
        clients.iter().copied().filter(|w| e.window_pid(a, *w) == Some(pid)).collect();

    if by_pid.is_empty() {
        return Ok(None);
    }
    if by_pid.len() == 1 {
        return Ok(Some(by_pid[0]));
    }

    // Several windows in one process: disambiguate by title. Note the search is
    // already scoped to the right process, so a generic title like "Untitled"
    // cannot pull in a different application's window.
    if let Some(want) = title {
        let want = want.to_ascii_lowercase();
        if let Some(w) = by_pid.iter().copied().find(|w| {
            e.window_title(a, *w).map(|t| t.to_ascii_lowercase().contains(&want)).unwrap_or(false)
        }) {
            return Ok(Some(w));
        }
    }
    Ok(Some(by_pid[0]))
}

/// What `ghost_window op=state` can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmAction {
    Minimize,
    Maximize,
    Restore,
    Activate,
    Close,
}

/// Apply a window-manager action to the window owned by `pid`.
///
/// `title` disambiguates when the process owns more than one window.
pub fn apply(pid: u32, title: Option<&str>, action: WmAction) -> Result<()> {
    let e = Ewmh::open()?;
    let a = e.atoms()?;

    let Some(w) = find_window(&e, &a, pid, title)? else {
        return Err(CoreError::Unsupported(format!(
            "process {pid} manages no X11 window. A native Wayland application cannot be \
             minimised, maximised or restored by any external client"
        )));
    };

    match action {
        // ICCCM: iconify is a WM_CHANGE_STATE message, not a _NET_WM_STATE one.
        WmAction::Minimize => {
            e.send_root_message(w, a.wm_change_state, [ICONIC_STATE, 0, 0, 0, 0])
        }
        WmAction::Maximize => e.send_root_message(
            w,
            a.net_wm_state,
            [NET_WM_STATE_ADD, a.net_wm_state_max_vert, a.net_wm_state_max_horz, 1, 0],
        ),
        WmAction::Restore => {
            // Restore means both "un-maximise" and "de-iconify"; a minimised
            // window is raised by activating it.
            e.send_root_message(
                w,
                a.net_wm_state,
                [NET_WM_STATE_REMOVE, a.net_wm_state_max_vert, a.net_wm_state_max_horz, 1, 0],
            )?;
            e.send_root_message(w, a.net_active_window, [1, 0, 0, 0, 0])
        }
        WmAction::Activate => e.send_root_message(w, a.net_active_window, [1, 0, 0, 0, 0]),
        WmAction::Close => e.send_root_message(w, a.net_close_window, [0, 1, 0, 0, 0]),
    }
}

/// Whether the window owned by `pid` is currently minimised.
///
/// Used to report `state` in `list_windows` from the window manager's own view
/// rather than inferring it, which is what the Windows engine does.
pub fn is_minimized(pid: u32, title: Option<&str>) -> Option<bool> {
    let e = Ewmh::open().ok()?;
    let a = e.atoms().ok()?;
    let w = find_window(&e, &a, pid, title).ok()??;

    let r = e
        .conn
        .get_property(false, w, a.net_wm_state, AtomEnum::ATOM, 0, 64)
        .ok()?
        .reply()
        .ok()?;
    let states: Vec<u32> = r.value32()?.collect();
    Some(states.contains(&a.net_wm_state_hidden))
}

/// The X11 window id backing an accessible window, if there is one.
///
/// Exposed so occluded-window capture can redirect the same window this module
/// resolves, instead of duplicating the AT-SPI-to-X11 join.
pub fn x11_window_for(pid: u32, title: Option<&str>) -> Option<Window> {
    let e = Ewmh::open().ok()?;
    let a = e.atoms().ok()?;
    find_window(&e, &a, pid, title).ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iconic_state_matches_icccm() {
        // ICCCM defines IconicState as 3. A wrong value here silently does
        // nothing rather than erroring, so it is worth pinning.
        assert_eq!(ICONIC_STATE, 3);
    }

    #[test]
    fn net_wm_state_actions_match_the_spec() {
        assert_eq!(NET_WM_STATE_REMOVE, 0);
        assert_eq!(NET_WM_STATE_ADD, 1);
    }

    #[test]
    fn every_windows_window_state_has_a_linux_action() {
        // ghost_window op=state accepts maximize|minimize|restore|close on
        // Windows. If a variant here were dropped, parity would regress
        // silently.
        for a in [
            WmAction::Minimize,
            WmAction::Maximize,
            WmAction::Restore,
            WmAction::Close,
            WmAction::Activate,
        ] {
            assert!(!format!("{a:?}").is_empty());
        }
    }
}
