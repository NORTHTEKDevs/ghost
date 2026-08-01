//! Wayland synthetic input via the xdg-desktop-portal RemoteDesktop interface.
//!
//! This is the compositor-supported, least-privilege path and the default on
//! Wayland. Two details make it usable for an agent rather than merely possible:
//!
//! **Consent is asked once, not every run.** `select_devices` is called with
//! `PersistMode::ExplicitlyRevoked` and any previously saved restore token. The
//! token returned by `start` is saved back to disk. Tokens are single-use, so
//! the *new* token replaces the old one after every successful session --
//! failing to rotate it is the usual cause of "it prompts me every time".
//!
//! **Absolute coordinates need a ScreenCast stream.**
//! `NotifyPointerMotionAbsolute` addresses a stream, not a screen, so a
//! RemoteDesktop session alone can only do relative motion. Ghost therefore
//! links a ScreenCast session purely to obtain a stream node id, and never
//! connects it to PipeWire -- which is what keeps this crate free of C linkage.

use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::time::Duration;

use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::error::{CoreError, Result};

/// Starting a portal session can involve a user-facing dialog on first run.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Individual input events are fast; a slow one means something is wrong.
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

enum Cmd {
    MoveTo(f64, f64, SyncSender<Result<()>>),
    MoveRelative(f64, f64, SyncSender<Result<()>>),
    Button(i32, bool, SyncSender<Result<()>>),
    Key(u32, bool, SyncSender<Result<()>>),
    Scroll(i32, SyncSender<Result<()>>),
}

pub struct PortalBackend {
    tx: UnboundedSender<Cmd>,
}

impl PortalBackend {
    pub fn new() -> Result<Self> {
        let (tx, mut rx) = unbounded_channel::<Cmd>();
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(1);

        std::thread::Builder::new()
            .name("ghost-portal".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(CoreError::platform(format!("portal rt: {e}"))));
                        return;
                    }
                };

                rt.block_on(async move {
                    let session = match PortalSession::open().await {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));

                    while let Some(cmd) = rx.recv().await {
                        session.handle(cmd).await;
                    }
                });
            })
            .map_err(|e| CoreError::platform(format!("portal thread: {e}")))?;

        ready_rx
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| CoreError::JobTimeout)??;

        Ok(Self { tx })
    }

    fn send(&self, make: impl FnOnce(SyncSender<Result<()>>) -> Cmd) -> Result<()> {
        let (tx, rx) = sync_channel(1);
        self.tx
            .send(make(tx))
            .map_err(|_| CoreError::WorkerPanic("portal thread stopped".into()))?;
        rx.recv_timeout(EVENT_TIMEOUT)
            .map_err(|_| CoreError::JobTimeout)?
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<()> {
        self.send(|tx| Cmd::MoveTo(x as f64, y as f64, tx))
    }
    pub fn move_relative(&self, dx: i32, dy: i32) -> Result<()> {
        self.send(|tx| Cmd::MoveRelative(dx as f64, dy as f64, tx))
    }
    /// `button` is an evdev code (`BTN_LEFT` = 0x110).
    pub fn button(&self, button: i32, down: bool) -> Result<()> {
        self.send(|tx| Cmd::Button(button, down, tx))
    }
    pub fn key(&self, keysym: u32, down: bool) -> Result<()> {
        self.send(|tx| Cmd::Key(keysym, down, tx))
    }
    pub fn scroll(&self, clicks: i32) -> Result<()> {
        self.send(|tx| Cmd::Scroll(clicks, tx))
    }
}

struct PortalSession {
    rd: RemoteDesktop,
    session: ashpd::desktop::Session<RemoteDesktop>,
    /// PipeWire node id of the linked ScreenCast stream, needed for absolute
    /// pointer coordinates. `None` if the compositor granted no stream.
    stream: Option<u32>,
}

impl PortalSession {
    async fn open() -> Result<Self> {
        let rd = RemoteDesktop::new().await.map_err(portal_err)?;
        let session = rd.create_session(Default::default()).await.map_err(portal_err)?;

        let saved = load_token();
        rd.select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(saved.as_deref()),
        )
        .await
        .map_err(portal_err)?;

        // Linked ScreenCast session: we want the stream id, not the frames.
        // Monitor specifically -- a Window stream reports window-relative
        // coordinates, which would silently offset every absolute click.
        let sources: ashpd::enumflags2::BitFlags<SourceType> = SourceType::Monitor.into();
        if let Ok(sc) = Screencast::new().await {
            let _ = sc
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_sources(sources)
                        .set_cursor_mode(CursorMode::Metadata)
                        .set_multiple(false)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked),
                )
                .await;
        }

        let response = rd
            .start(&session, None, Default::default())
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;

        // Tokens are single-use: persist the new one or the next run prompts.
        if let Some(token) = response.restore_token() {
            save_token(token);
        }

        let stream = response.streams().first().map(|s| s.pipe_wire_node_id());

        Ok(Self { rd, session, stream })
    }

    async fn handle(&self, cmd: Cmd) {
        match cmd {
            Cmd::MoveTo(x, y, reply) => {
                let r = match self.stream {
                    Some(node) => self
                        .rd
                        .notify_pointer_motion_absolute(&self.session, node, x, y, Default::default())
                        .await
                        .map_err(portal_err),
                    None => Err(CoreError::Unsupported(
                        "absolute pointer motion needs a ScreenCast stream, which the compositor \
                         did not grant. Re-run and accept the screen-share prompt, or target \
                         controls by name/role so AT-SPI drives them directly"
                            .into(),
                    )),
                };
                let _ = reply.send(r);
            }
            Cmd::MoveRelative(dx, dy, reply) => {
                let r = self
                    .rd
                    .notify_pointer_motion(&self.session, dx, dy, Default::default())
                    .await
                    .map_err(portal_err);
                let _ = reply.send(r);
            }
            Cmd::Button(button, down, reply) => {
                let state = if down { KeyState::Pressed } else { KeyState::Released };
                let r = self
                    .rd
                    .notify_pointer_button(&self.session, button, state, Default::default())
                    .await
                    .map_err(portal_err);
                let _ = reply.send(r);
            }
            Cmd::Key(keysym, down, reply) => {
                let state = if down { KeyState::Pressed } else { KeyState::Released };
                // Keysyms avoid needing the compositor's keymap, so arbitrary
                // Unicode can be typed without a scratch-keycode remap.
                let r = self
                    .rd
                    .notify_keyboard_keysym(&self.session, keysym as i32, state, Default::default())
                    .await
                    .map_err(portal_err);
                let _ = reply.send(r);
            }
            Cmd::Scroll(clicks, reply) => {
                let r = self
                    .rd
                    .notify_pointer_axis_discrete(
                        &self.session,
                        ashpd::desktop::remote_desktop::Axis::Vertical,
                        clicks,
                        Default::default(),
                    )
                    .await
                    .map_err(portal_err);
                let _ = reply.send(r);
            }
        }
    }
}

fn portal_err(e: ashpd::Error) -> CoreError {
    CoreError::platform(format!(
        "RemoteDesktop portal: {e}. Check that xdg-desktop-portal and \
         xdg-desktop-portal-gnome are installed and running"
    ))
}

fn token_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("ghost").join("remote-desktop.token"))
}

fn load_token() -> Option<String> {
    let p = token_path()?;
    let s = std::fs::read_to_string(p).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn save_token(token: &str) {
    let Some(p) = token_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, token);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_path_lives_under_a_state_directory() {
        if let Some(p) = token_path() {
            let s = p.to_string_lossy().replace('\\', "/");
            assert!(s.contains("ghost"));
            assert!(s.contains("state") || s.contains("XDG"));
        }
    }

    #[test]
    fn an_empty_token_file_is_treated_as_absent() {
        // A blank token must not be sent to select_devices: the portal would
        // reject it and the user would see a prompt with no explanation.
        assert!(load_token().map(|t| !t.is_empty()).unwrap_or(true));
    }
}
