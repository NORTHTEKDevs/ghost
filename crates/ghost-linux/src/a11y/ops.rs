//! Raw AT-SPI operations, executed on the accessibility thread.
//!
//! Everything here takes an explicit [`ObjAddr`] and rebuilds its proxy rather
//! than holding one across calls. That is deliberate: applications routinely
//! destroy and recreate widgets while the visible UI looks unchanged, so a proxy
//! cached across an action is a stale-reference bug waiting to happen. Resolving
//! by address means a vanished object fails loudly as
//! [`CoreError::WindowGone`] instead of acting on the wrong thing.

use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::{CoordType, Interface, Role, ScrollType, State};

use super::handles::ObjAddr;
use super::roles::{atspi_role_to_id, role_id_to_name};
use crate::bridge::Ctx;
use crate::error::{CoreError, Result};

/// A point-in-time reading of one accessible object.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub name: String,
    pub role_id: u32,
    pub rect: Option<Rect>,
    pub enabled: bool,
    pub showing: bool,
    pub focusable: bool,
    pub has_action: bool,
    pub has_editable_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

/// Action names worth trying, most-specific first.
///
/// AT-SPI does not guarantee action 0 is "click", so picking by name matters:
/// on a GTK combo box, index 0 can be "press" while a menu item exposes
/// "click". Falling back to index 0 only when no name matches keeps the common
/// case correct without giving up on unusual widgets.
const PREFERRED_ACTIONS: &[&str] =
    &["click", "press", "activate", "jump", "open", "toggle", "expand or contract"];

pub async fn accessible<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<AccessibleProxy<'a>> {
    AccessibleProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("accessible {}: {e}", addr.path)))
}

pub async fn component<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<ComponentProxy<'a>> {
    ComponentProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("component {}: {e}", addr.path)))
}

pub async fn action<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<ActionProxy<'a>> {
    ActionProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("action {}: {e}", addr.path)))
}

pub async fn text_iface<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<TextProxy<'a>> {
    TextProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("text {}: {e}", addr.path)))
}

pub async fn editable<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<EditableTextProxy<'a>> {
    EditableTextProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("editable {}: {e}", addr.path)))
}

pub async fn value_iface<'a>(ctx: &'a Ctx, addr: &'a ObjAddr) -> Result<ValueProxy<'a>> {
    ValueProxy::builder(ctx.zbus())
        .destination(addr.bus.as_str())?
        .path(addr.path.as_str())?
        .build()
        .await
        .map_err(|e| CoreError::Platform(format!("value {}: {e}", addr.path)))
}

/// Children of an object, as addresses.
pub async fn children(ctx: &Ctx, addr: &ObjAddr) -> Result<Vec<ObjAddr>> {
    let acc = accessible(ctx, addr).await?;
    let kids = acc.get_children().await.map_err(|e| CoreError::Platform(format!("children: {e}")))?;
    Ok(kids
        .iter()
        .filter_map(|k| {
            let bus = k.name_as_str()?;
            Some(ObjAddr::new(bus, k.path_as_str()))
        })
        .collect())
}

/// Read everything Ghost needs about one object.
///
/// Extents are only fetched when the object actually implements Component;
/// asking a non-Component for extents is an error round trip per element, which
/// on a large tree is the difference between a fast and an unusable walk.
pub async fn snapshot(ctx: &Ctx, addr: &ObjAddr) -> Result<Snapshot> {
    let acc = accessible(ctx, addr).await?;

    let states = acc.get_state().await.map_err(|e| CoreError::Platform(format!("state: {e}")))?;
    if states.contains(State::Defunct) {
        return Err(CoreError::WindowGone);
    }

    let role = acc.get_role().await.unwrap_or(Role::Invalid);
    let name = acc.name().await.unwrap_or_default();
    let ifaces = acc.get_interfaces().await.unwrap_or_default();

    let rect = if ifaces.contains(Interface::Component) {
        match component(ctx, addr).await {
            Ok(c) => match c.get_extents(CoordType::Screen).await {
                // saturating: extents are app-supplied, and a hostile or buggy
                // value would otherwise overflow into a corrupted rect that
                // Ghost would report -- and click on -- as if it were valid.
                Ok((x, y, w, h)) => Some(Rect {
                    left: x,
                    top: y,
                    right: x.saturating_add(w),
                    bottom: y.saturating_add(h),
                }),
                Err(_) => None,
            },
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(Snapshot {
        name,
        role_id: atspi_role_to_id(role),
        rect,
        enabled: states.contains(State::Enabled) || states.contains(State::Sensitive),
        showing: states.contains(State::Showing) && states.contains(State::Visible),
        focusable: states.contains(State::Focusable),
        has_action: ifaces.contains(Interface::Action),
        has_editable_text: ifaces.contains(Interface::EditableText),
    })
}

/// The role Ghost reports for an element.
///
/// Toolkits disagree about what a single-line text field is: depending on the
/// GTK/Qt version an editable entry reports AT-SPI `entry`, `text`, or a plain
/// panel-ish role. Measured on GTK 3 under CI, `zenity --forms` exposes its
/// entry as `text`, which would otherwise be invisible to `role="edit"`.
///
/// The reliable discriminator is not the role name but the **EditableText
/// interface**: an object that implements it is, by definition, something an
/// agent can type into. So anything editable that would otherwise be reported
/// as generic `text` is normalised to `edit` -- the same name the Windows engine
/// uses for the same thing, which is what keeps the MCP surface consistent.
pub fn effective_role(snap: &Snapshot) -> &'static str {
    let actual = role_id_to_name(snap.role_id);
    if snap.has_editable_text && actual == "text" {
        return "edit";
    }
    actual
}

/// Invoke the element's best action. This is Ghost's background dispatch on
/// Linux: it drives the app through its own toolkit without synthetic input,
/// without moving the pointer and without taking foreground.
pub async fn do_best_action(ctx: &Ctx, addr: &ObjAddr) -> Result<bool> {
    let act = action(ctx, addr).await?;
    // Capped: `n_actions` is self-reported by the target application. A buggy or
    // hostile one returning a huge count would drive a multi-gigabyte allocation
    // and a D-Bus flood. No real widget has more than a handful of actions.
    const MAX_ACTIONS: i32 = 32;
    let n = act.n_actions().await.unwrap_or(0).min(MAX_ACTIONS);
    if n <= 0 {
        return Err(CoreError::NotActionable(format!("{} exposes no actions", addr.path)));
    }

    let mut names: Vec<String> = Vec::with_capacity(n as usize);
    for i in 0..n {
        names.push(act.get_name(i).await.unwrap_or_default().to_ascii_lowercase());
    }

    let idx = PREFERRED_ACTIONS
        .iter()
        .find_map(|want| names.iter().position(|got| got == want))
        .unwrap_or(0) as i32;

    act.do_action(idx)
        .await
        .map_err(|e| CoreError::Platform(format!("do_action({idx}): {e}")))
}

/// Replace an element's text. Prefers EditableText, falls back to Value for
/// widgets (spinners, sliders) that carry a numeric value instead.
pub async fn set_text_value(ctx: &Ctx, addr: &ObjAddr, text: &str) -> Result<bool> {
    let acc = accessible(ctx, addr).await?;
    let ifaces = acc.get_interfaces().await.unwrap_or_default();

    if ifaces.contains(Interface::EditableText) {
        let ed = editable(ctx, addr).await?;
        return ed
            .set_text_contents(text)
            .await
            .map_err(|e| CoreError::Platform(format!("set_text_contents: {e}")));
    }

    if ifaces.contains(Interface::Value) {
        if let Ok(n) = text.trim().parse::<f64>() {
            let v = value_iface(ctx, addr).await?;
            v.set_current_value(n)
                .await
                .map_err(|e| CoreError::Platform(format!("set_current_value: {e}")))?;
            return Ok(true);
        }
    }

    Err(CoreError::NotActionable(format!(
        "{} is not editable (no EditableText or numeric Value interface)",
        addr.path
    )))
}

/// Read an element's text: Text interface first, then Value, then the
/// accessible name -- which for buttons and labels *is* the visible text.
pub async fn read_text(ctx: &Ctx, addr: &ObjAddr) -> Result<String> {
    let acc = accessible(ctx, addr).await?;
    let ifaces = acc.get_interfaces().await.unwrap_or_default();

    if ifaces.contains(Interface::Text) {
        let t = text_iface(ctx, addr).await?;
        // Bounded by the real character count. Passing an arbitrarily large end
        // offset is rejected or truncated inconsistently across toolkits.
        let count = t.character_count().await.unwrap_or(0);
        if count > 0 {
            if let Ok(s) = t.get_text(0, count).await {
                return Ok(s);
            }
        }
    }

    if ifaces.contains(Interface::Value) {
        if let Ok(v) = value_iface(ctx, addr).await {
            if let Ok(cur) = v.current_value().await {
                return Ok(format!("{cur}"));
            }
        }
    }

    Ok(acc.name().await.unwrap_or_default())
}

/// Currently selected text within an element.
///
/// AT-SPI reports selections as offset pairs, so this resolves them against the
/// element's text. An element with no Text interface has no selection -- that is
/// an empty string, not an error.
pub async fn read_selection(ctx: &Ctx, addr: &ObjAddr) -> Result<String> {
    let acc = accessible(ctx, addr).await?;
    if !acc.get_interfaces().await.unwrap_or_default().contains(Interface::Text) {
        return Ok(String::new());
    }
    let t = text_iface(ctx, addr).await?;
    let n = t.get_n_selections().await.unwrap_or(0);
    if n <= 0 {
        return Ok(String::new());
    }
    let mut out = String::new();
    for i in 0..n {
        if let Ok((start, end)) = t.get_selection(i).await {
            if end > start {
                if let Ok(part) = t.get_text(start, end).await {
                    out.push_str(&part);
                }
            }
        }
    }
    Ok(out)
}

/// Move keyboard focus to an element without raising its window where the
/// toolkit allows it.
pub async fn grab_focus(ctx: &Ctx, addr: &ObjAddr) -> Result<bool> {
    let c = component(ctx, addr).await?;
    c.grab_focus()
        .await
        .map_err(|e| CoreError::FocusFailed { window: format!("grab_focus: {e}") })
}

/// Scroll an element into view.
pub async fn scroll_into_view(ctx: &Ctx, addr: &ObjAddr) -> Result<bool> {
    let c = component(ctx, addr).await?;
    c.scroll_to(ScrollType::Anywhere)
        .await
        .map_err(|e| CoreError::Platform(format!("scroll_to: {e}")))
}

/// Hit-test: the deepest accessible object at a screen point.
pub async fn accessible_at_point(ctx: &Ctx, addr: &ObjAddr, x: i32, y: i32) -> Result<Option<ObjAddr>> {
    let c = component(ctx, addr).await?;
    match c.get_accessible_at_point(x, y, CoordType::Screen).await {
        Ok(obj) => Ok(obj.name_as_str().map(|bus| ObjAddr::new(bus, obj.path_as_str()))),
        Err(_) => Ok(None),
    }
}

/// The PID behind a D-Bus name, so `list_windows` can report a real process id.
pub async fn pid_for_bus(ctx: &Ctx, bus: &str) -> Option<u32> {
    let dbus = zbus::fdo::DBusProxy::new(ctx.zbus()).await.ok()?;
    let name = zbus::names::BusName::try_from(bus.to_string()).ok()?;
    dbus.get_connection_unix_process_id(name).await.ok()
}

/// Clipboard and selection commands, performed by the application itself.
///
/// This is the Linux analogue of the Windows engine's `WM_COPY`/`WM_CUT`/
/// `WM_PASTE`/`EM_SETSEL` messages: AT-SPI's `EditableText` interface asks the
/// application to run its own copy/cut/paste, so the operation needs no
/// synthetic keystrokes, does not move the pointer, and -- critically -- does
/// **not** change which control has focus.
///
/// The previous implementation grabbed focus and fired Ctrl+C, which both
/// disturbed the desktop and could land on the wrong control if the focus grab
/// silently failed. This cannot.
///
/// Copy and cut act on the current selection, matching `WM_COPY`. With nothing
/// selected they report rather than silently doing nothing, since "copied
/// successfully, clipboard unchanged" is indistinguishable from a bug.
pub async fn edit_command(ctx: &Ctx, addr: &ObjAddr, cmd: EditKind) -> Result<()> {
    let acc = accessible(ctx, addr).await?;
    let ifaces = acc.get_interfaces().await.unwrap_or_default();

    if !ifaces.contains(Interface::Text) {
        return Err(CoreError::NotActionable(format!(
            "{} exposes no Text interface, so {cmd:?} cannot be performed without synthetic keys",
            addr.path
        )));
    }
    let text = text_iface(ctx, addr).await?;

    // SelectAll needs only the Text interface; the rest need EditableText.
    if cmd == EditKind::SelectAll {
        let count = text.character_count().await.unwrap_or(0);
        if count <= 0 {
            return Ok(());
        }
        text.add_selection(0, count)
            .await
            .map_err(|e| CoreError::Platform(format!("add_selection: {e}")))?;
        return Ok(());
    }

    if !ifaces.contains(Interface::EditableText) {
        return Err(CoreError::NotActionable(format!(
            "{} is not editable, so {cmd:?} cannot be performed",
            addr.path
        )));
    }
    let ed = editable(ctx, addr).await?;

    match cmd {
        EditKind::Paste => {
            // NOT unwrap_or(0): a failed caret read is not "the caret is at the
            // start". Guessing 0 pastes at the beginning of the field and looks
            // like success, which is worse than refusing.
            let at = text.caret_offset().await.map_err(|e| {
                CoreError::Platform(format!(
                    "could not read the caret position, so paste has no known insertion point: {e}"
                ))
            })?;
            ed.paste_text(at)
                .await
                .map_err(|e| CoreError::Platform(format!("paste_text: {e}")))?;
            Ok(())
        }
        EditKind::Copy | EditKind::Cut => {
            let (start, end) = selection_range(&text).await.ok_or_else(|| {
                CoreError::NotActionable(
                    "nothing is selected; select a range first (op=select_all) before copy/cut"
                        .into(),
                )
            })?;
            if cmd == EditKind::Copy {
                ed.copy_text(start, end)
                    .await
                    .map_err(|e| CoreError::Platform(format!("copy_text: {e}")))?;
            } else {
                // Cut is destructive and AT-SPI offers no atomic "cut the
                // selection" verb, only offsets. Widgets that reformat as you
                // type can shift them between the read and the act, and
                // cut_text would then delete whatever had moved into that
                // range. Re-read and refuse on a shift rather than delete the
                // wrong text.
                match selection_range(&text).await {
                    Some(now) if now == (start, end) => {}
                    _ => {
                        return Err(CoreError::NotActionable(
                            "the selection moved while cutting, so that range is no longer the \
                             selected text; select again and retry"
                                .into(),
                        ))
                    }
                }
                ed.cut_text(start, end)
                    .await
                    .map_err(|e| CoreError::Platform(format!("cut_text: {e}")))?;
            }
            Ok(())
        }
        // AT-SPI has no undo verb. Refusing is better than firing Ctrl+Z at
        // whatever happens to be focused, which is not necessarily this element.
        EditKind::Undo => Err(CoreError::NotActionable(
            "AT-SPI exposes no undo action; send ghost_key ctrl+z with the element focused"
                .into(),
        )),
        EditKind::SelectAll => unreachable!("handled above"),
    }
}

/// The current selection range, if there is one.
async fn selection_range(text: &TextProxy<'_>) -> Option<(i32, i32)> {
    if text.get_n_selections().await.unwrap_or(0) <= 0 {
        return None;
    }
    let (start, end) = text.get_selection(0).await.ok()?;
    if end > start {
        Some((start, end))
    } else {
        None
    }
}

/// Which edit command to perform. Mirrors `ghost_core::input::EditCommand`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Copy,
    Cut,
    Paste,
    Undo,
    SelectAll,
}

/// Find the focused descendant of a window, if any.
///
/// Bounded like every other walk: a window with a pathological tree must not be
/// able to stall the caller while looking for the caret.
pub async fn focused_descendant(ctx: &Ctx, root: &ObjAddr) -> Result<Option<ObjAddr>> {
    const FOCUS_BUDGET: usize = 2000;

    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root.clone());
    // Every other walk carries a seen-set; this one was missed. Without it a
    // cyclic tree burns the whole budget on the loop and reports "nothing is
    // focused" with full confidence when the focused element is elsewhere.
    let mut seen: std::collections::HashSet<ObjAddr> = std::collections::HashSet::new();
    seen.insert(root.clone());
    let mut visited = 0usize;

    while let Some(addr) = queue.pop_front() {
        if visited >= FOCUS_BUDGET {
            break;
        }
        visited += 1;

        if let Ok(acc) = accessible(ctx, &addr).await {
            if let Ok(states) = acc.get_state().await {
                if states.contains(State::Focused) {
                    return Ok(Some(addr));
                }
            }
        }
        if let Ok(kids) = children(ctx, &addr).await {
            for k in kids {
                if seen.insert(k.clone()) {
                    queue.push_back(k);
                }
            }
        }
    }
    Ok(None)
}

/// Whether an object is currently the active (focused) window.
pub async fn is_active(ctx: &Ctx, addr: &ObjAddr) -> bool {
    match accessible(ctx, addr).await {
        Ok(acc) => match acc.get_state().await {
            Ok(s) => s.contains(State::Active),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_centre_matches_the_windows_engine_formula() {
        let r = Rect { left: 10, top: 20, right: 30, bottom: 60 };
        assert_eq!(r.center(), (20, 40));
    }

    #[test]
    fn degenerate_rects_are_empty() {
        assert!(Rect { left: 5, top: 5, right: 5, bottom: 9 }.is_empty());
        assert!(Rect { left: 5, top: 5, right: 9, bottom: 5 }.is_empty());
        assert!(!Rect { left: 0, top: 0, right: 1, bottom: 1 }.is_empty());
    }

    #[test]
    fn preferred_actions_are_lowercase_for_case_insensitive_matching() {
        // do_best_action lowercases the names it reads; if this list were not
        // already lowercase, nothing would ever match and every widget would
        // silently fall back to action 0.
        for a in PREFERRED_ACTIONS {
            assert_eq!(*a, a.to_ascii_lowercase());
        }
    }

    #[test]
    fn click_outranks_the_generic_actions() {
        let pos = |n: &str| PREFERRED_ACTIONS.iter().position(|a| *a == n);
        assert!(pos("click") < pos("activate"));
        assert!(pos("press") < pos("toggle"));
    }
}
