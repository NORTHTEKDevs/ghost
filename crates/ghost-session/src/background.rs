//! Window-scoped background input.
//!
//! Everything in this module addresses one window by handle. Nothing here reads or
//! writes the shared foreground state, so any number of `WindowTarget`s - in this
//! process or in other ghost processes - can run at the same time as the user works.

use crate::error::{GhostError, Result};
use ghost_core::error::CoreError;
use ghost_core::input::hotkey::is_stopped;
use ghost_core::input::{keyboard::name_to_vk, postmessage as pm};
use ghost_core::uia::tree::{list_windows, UiaTree};
use windows::Win32::Foundation::HWND;

/// A resolved window plus the metadata callers want back in tool results.
pub struct WindowTarget {
    pub hwnd: HWND,
    pub title: String,
    pub pid: u32,
}

// Safety: an HWND is a kernel-object identifier, not a pointer into this process;
// window messages may be posted to it from any thread.
unsafe impl Send for WindowTarget {}
unsafe impl Sync for WindowTarget {}

impl WindowTarget {
    /// Resolve a top-level window by case-insensitive partial title match.
    ///
    /// Ties break toward the shortest title: searching "Notepad" should find
    /// "Untitled - Notepad" rather than some unrelated window that merely mentions
    /// Notepad in a longer title, and the result must be stable across calls rather
    /// than depending on Z-order.
    pub fn resolve(name: &str) -> Result<Self> {
        let needle = name.to_lowercase();
        let mut matches: Vec<_> = list_windows()
            .map_err(GhostError::Core)?
            .into_iter()
            .filter(|w| w.name.to_lowercase().contains(&needle))
            .collect();
        matches.sort_by_key(|w| (w.name.len(), w.name.clone()));
        let w = matches.into_iter().next().ok_or_else(|| GhostError::ProcessNotFound {
            name: name.to_string(),
        })?;
        Ok(Self { hwnd: HWND(w.hwnd), title: w.name, pid: w.pid })
    }

    /// Resolve the main top-level window of a specific process.
    ///
    /// Title matching is ambiguous in a way that matters: "Notepad" also matches the
    /// user's own open document, and typing into the wrong one silently edits their
    /// work. An agent that launched a process should address it by pid.
    pub fn resolve_by_pid(pid: u32) -> Result<Self> {
        let mut candidates: Vec<_> = list_windows()
            .map_err(GhostError::Core)?
            .into_iter()
            .filter(|w| w.pid == pid && !w.name.is_empty())
            .collect();
        // Longest title wins here: a process's main window carries the document name,
        // while its tooltips and popups have short or generic ones.
        candidates.sort_by_key(|w| std::cmp::Reverse(w.name.len()));
        let w = candidates.into_iter().next().ok_or_else(|| GhostError::ProcessNotFound {
            name: format!("pid {pid}"),
        })?;
        Ok(Self { hwnd: HWND(w.hwnd), title: w.name, pid: w.pid })
    }

    fn guard(&self) -> Result<()> {
        if is_stopped() {
            return Err(GhostError::Stopped);
        }
        Ok(())
    }

    /// Translate a screen point (what UIA bounding rectangles report) into this
    /// window's client space.
    pub fn screen_to_client(&self, sx: i32, sy: i32) -> Result<(i32, i32)> {
        pm::screen_to_client(self.hwnd, sx, sy).map_err(GhostError::Core)
    }

    pub fn client_size(&self) -> Result<(i32, i32)> {
        pm::client_size(self.hwnd).map_err(GhostError::Core)
    }

    pub fn click(&self, x: i32, y: i32) -> Result<()> {
        self.guard()?;
        pm::click(self.hwnd, (x, y)).map_err(GhostError::Core)
    }

    pub fn right_click(&self, x: i32, y: i32) -> Result<()> {
        self.guard()?;
        pm::right_click(self.hwnd, (x, y)).map_err(GhostError::Core)
    }

    pub fn double_click(&self, x: i32, y: i32) -> Result<()> {
        self.guard()?;
        pm::double_click(self.hwnd, (x, y)).map_err(GhostError::Core)
    }

    pub fn hover(&self, x: i32, y: i32) -> Result<()> {
        self.guard()?;
        pm::hover(self.hwnd, (x, y)).map_err(GhostError::Core)
    }

    pub fn scroll(&self, x: i32, y: i32, direction: &str, amount: i32) -> Result<()> {
        self.guard()?;
        let (notches, horizontal) = match direction {
            "up" => (amount, false),
            "down" => (-amount, false),
            "right" => (amount, true),
            "left" => (-amount, true),
            _ => {
                return Err(GhostError::Core(CoreError::Win32 {
                    code: 0,
                    context: "invalid scroll direction",
                }))
            }
        };
        pm::scroll(self.hwnd, (x, y), notches, horizontal).map_err(GhostError::Core)
    }

    pub fn type_text(&self, text: &str) -> Result<()> {
        self.guard()?;
        pm::type_text(self.hwnd, text).map_err(GhostError::Core)
    }

    pub fn press(&self, key: &str) -> Result<()> {
        self.guard()?;
        let vk = vk_for(key)?;
        pm::press_key(self.hwnd, vk).map_err(GhostError::Core)
    }

    /// Perform a keyboard shortcut against a background window.
    ///
    /// **Not** by posting modifier key messages. `PostMessage(WM_KEYDOWN,
    /// VK_CONTROL)` does not change the target thread's keyboard state, so the
    /// target's own message loop sees an unmodified keystroke: a posted "Ctrl+Z"
    /// arrives as the literal character "z" and silently corrupts the document
    /// instead of undoing anything. That failure is invisible - the calls all
    /// "succeed".
    ///
    /// So ghost tries, in order: the standard control message for that operation
    /// (`WM_UNDO`, `WM_PASTE`, `EM_SETSEL`, ...), then the command that advertises the
    /// accelerator in the window's automation tree (Edit > Undo advertises "Ctrl+Z").
    /// Both perform the genuine action. If neither exists there is no correct
    /// background path, and this returns an error naming the alternatives rather than
    /// typing garbage.
    pub fn hotkey(&self, modifiers: &[String], key: &str) -> Result<()> {
        self.guard()?;
        // Validate the key name up front so a typo fails here rather than looking
        // like "this app has no such shortcut".
        vk_for(key)?;
        let mut combo: Vec<String> = modifiers.to_vec();
        combo.push(key.to_string());
        let accelerator = combo.join("+");

        // 1. Standard control messages. Undo, cut, copy, paste, clear, and select-all
        //    are real messages on every edit and rich-edit control, so these need no
        //    keyboard simulation at all and work on any window.
        if let Some(sc) = ghost_core::input::Shortcut::parse(&accelerator) {
            if ghost_core::input::shortcut::apply_shortcut(self.hwnd, sc, 5_000).is_ok() {
                return Ok(());
            }
        }

        // 2. The command that advertises this accelerator (Edit > Undo and friends).
        let tree = UiaTree::new().map_err(GhostError::Core)?;
        let found = tree
            .find_by_accelerator(Some(&self.title), &accelerator)
            .map_err(GhostError::Core)?;
        match found {
            Some(el) => {
                ghost_core::uia::patterns::invoke(&el).map_err(GhostError::Core)?;
                Ok(())
            }
            None => Err(GhostError::ElementNotInteractable {
                element: format!("{accelerator} in '{}'", self.title),
                reason: format!(
                    "no standard control message and no command advertising that shortcut. \
                     Window messages cannot set modifier key state, so ghost will not fake it \
                     (a faked Ctrl+Z types a literal 'z'). Shortcuts with a message equivalent: \
                     {}. Otherwise click the menu item directly, or set the focus policy to \
                     'foreground' for this one action.",
                    ghost_core::input::Shortcut::all().join(", ")
                ),
            }),
        }
    }

    /// Replace the target's text wholesale.
    pub fn set_text(&self, text: &str, timeout_ms: u32) -> Result<()> {
        self.guard()?;
        pm::set_text(self.hwnd, text, timeout_ms).map_err(GhostError::Core)
    }

    /// PNG of this window alone, captured without raising or un-occluding it.
    pub fn capture(&self, client_only: bool) -> Result<Vec<u8>> {
        ghost_core::capture::capture_window(self.hwnd, client_only).map_err(GhostError::Core)
    }
}

fn vk_for(key: &str) -> Result<u16> {
    name_to_vk(key)
        .map(|vk| vk.0)
        .ok_or_else(|| GhostError::Core(CoreError::Win32 { code: 0, context: "unknown key name" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving_a_nonexistent_window_is_an_error() {
        let r = WindowTarget::resolve("no-such-window-zzqq-9182");
        assert!(matches!(r, Err(GhostError::ProcessNotFound { .. })));
    }

    #[test]
    fn unknown_key_names_are_rejected_before_any_message_is_posted() {
        assert!(vk_for("NotARealKey").is_err());
        assert!(vk_for("Enter").is_ok());
        assert!(vk_for("ctrl").is_ok(), "modifier names must resolve too");
    }
}
