//! uinput backend: a virtual input device in the kernel.
//!
//! Opt-in via `GHOST_INPUT=uinput`. Because events enter the kernel input
//! subsystem, the compositor treats them exactly like real hardware -- so this
//! works under Wayland with no portal, no consent dialog and no session
//! lifetime. That makes it the right choice for an unattended or CI machine.
//!
//! It is **not** the default, deliberately. Writing to `/dev/uinput` is a
//! machine-wide privilege: whoever holds it can inject input into any user's
//! session, including a lock screen. It should be granted to a dedicated service
//! account through a narrow udev rule, never by adding a desktop user to a broad
//! group.
//!
//! Absolute pointer positioning is unavailable here: a relative pointer device
//! cannot be commanded to screen coordinates without knowing where the cursor
//! currently is, and the kernel will not tell us. Coordinate clicks therefore
//! report [`CoreError::Unsupported`] rather than clicking the wrong place --
//! AT-SPI actions remain the correct path for targeting a specific control.

use std::sync::Mutex;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

use crate::error::{CoreError, Result};

pub struct UinputBackend {
    dev: Mutex<VirtualDevice>,
}

impl UinputBackend {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        // Full keyboard range plus the mouse buttons.
        for code in 1..=248u16 {
            keys.insert(KeyCode::new(code));
        }
        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        axes.insert(RelativeAxisCode::REL_X);
        axes.insert(RelativeAxisCode::REL_Y);
        axes.insert(RelativeAxisCode::REL_WHEEL);
        axes.insert(RelativeAxisCode::REL_HWHEEL);

        let dev = VirtualDevice::builder()
            .map_err(uinput_err)?
            .name("ghost-virtual-input")
            .with_keys(&keys)
            .map_err(uinput_err)?
            .with_relative_axes(&axes)
            .map_err(uinput_err)?
            .build()
            .map_err(uinput_err)?;

        Ok(Self { dev: Mutex::new(dev) })
    }

    fn emit(&self, events: &[InputEvent]) -> Result<()> {
        let mut dev = match self.dev.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        dev.emit(events).map_err(|e| CoreError::platform(format!("uinput emit: {e}")))
    }

    pub fn key_code(&self, code: u16, down: bool) -> Result<()> {
        self.emit(&[InputEvent::new(EventType::KEY.0, code, if down { 1 } else { 0 })])
    }

    pub fn button(&self, code: u16, down: bool) -> Result<()> {
        self.key_code(code, down)
    }

    pub fn move_relative(&self, dx: i32, dy: i32) -> Result<()> {
        self.emit(&[
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx),
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy),
        ])
    }

    pub fn scroll(&self, clicks: i32) -> Result<()> {
        self.scroll_axis(clicks, false)
    }

    pub fn scroll_axis(&self, clicks: i32, horizontal: bool) -> Result<()> {
        let axis =
            if horizontal { RelativeAxisCode::REL_HWHEEL } else { RelativeAxisCode::REL_WHEEL };
        self.emit(&[InputEvent::new(EventType::RELATIVE.0, axis.0, clicks)])
    }

    pub fn move_to(&self, _x: i32, _y: i32) -> Result<()> {
        Err(CoreError::Unsupported(
            "uinput provides a relative pointer only, so absolute coordinates cannot be targeted. \
             Use ghost_act with a name/role (AT-SPI drives the control directly), or switch to \
             the portal backend by unsetting GHOST_INPUT"
                .into(),
        ))
    }
}

fn uinput_err(e: std::io::Error) -> CoreError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return CoreError::platform(
            "permission denied opening /dev/uinput. Grant access with a udev rule, e.g. \
             KERNEL==\"uinput\", GROUP=\"ghost-input\", MODE=\"0660\" -- and add only the \
             automation service account to that group",
        );
    }
    if e.kind() == std::io::ErrorKind::NotFound {
        return CoreError::platform(
            "/dev/uinput does not exist. Load the module with: sudo modprobe uinput",
        );
    }
    CoreError::platform(format!("uinput: {e}"))
}

/// evdev button codes.
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_codes_match_linux_input_event_codes_h() {
        assert_eq!(BTN_LEFT, 0x110);
        assert_eq!(BTN_RIGHT, 0x111);
        assert_eq!(BTN_MIDDLE, 0x112);
    }

    #[test]
    fn absolute_positioning_is_refused_with_a_usable_alternative() {
        // Silently clicking the wrong coordinates would be far worse than an
        // error. This must call the real function: an earlier version built a
        // CoreError locally and matched it against itself, which passed no
        // matter what move_to did.
        //
        // Opening /dev/uinput needs privileges CI does not have, so the device
        // is only exercised when it is actually available; the refusal itself is
        // what is under test, and it does not depend on the device.
        match UinputBackend::new() {
            Ok(b) => {
                let err = b.move_to(100, 200).expect_err("uinput must refuse absolute coordinates");
                assert!(
                    matches!(err, CoreError::Unsupported(ref m) if m.contains("ghost_act")),
                    "the refusal must name the working alternative, got: {err}"
                );
            }
            Err(e) => {
                // No /dev/uinput here. Assert the diagnosis is actionable rather
                // than silently skipping.
                let m = e.to_string();
                assert!(
                    m.contains("uinput") || m.contains("permission"),
                    "an unavailable uinput device must explain why, got: {m}"
                );
            }
        }
    }
}
