//! Screen capture, mirroring `ghost_core::capture::screen` signature-for-signature.
//!
//! | Session | Method | Notes |
//! |---|---|---|
//! | X11 | `GetImage` on the root window | full desktop, any region, no prompt |
//! | Wayland | Screenshot portal (one-shot) | consent on first use; whole screen |
//!
//! The Wayland path deliberately uses the **Screenshot** portal rather than
//! ScreenCast + PipeWire. Ghost captures stills for verification and vision
//! grounding, never video, and the Screenshot portal is pure D-Bus -- so this
//! crate needs no `pipewire-devel` at build time and no `libpipewire` at run
//! time. ScreenCast would have bought a frame stream Ghost does not use, at the
//! cost of C linkage and DMA-BUF handling.
//!
//! Region capture under Wayland crops a full capture, because the portal takes
//! no region parameter.

use std::io::Cursor;
use std::time::Duration;

use image::{DynamicImage, RgbaImage};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

use super::marks::Mark;
use crate::error::{CoreError, Result};
use crate::session::{session_kind, SessionKind};

/// Identical to `ghost_core::capture::screen::CaptureFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    Png,
    /// JPEG with quality 0-100 (75 is a good default for vision payloads).
    Jpeg(u8),
}

/// Capture the screen as PNG bytes.
pub fn capture_screen() -> Result<Vec<u8>> {
    let (rgba, w, h) = capture_screen_full_rgba()?;
    encode_rgba(&rgba, w, h, CaptureFormat::Png)
}

/// Full screen as raw RGBA plus its dimensions.
pub fn capture_screen_full_rgba() -> Result<(Vec<u8>, usize, usize)> {
    match session_kind() {
        SessionKind::X11 => x11_capture(None),
        SessionKind::Wayland => portal_capture(),
        SessionKind::Headless => Err(CoreError::Unsupported(
            "no display session detected, so the screen cannot be captured".into(),
        )),
    }
}

/// Raw RGBA of a rect, or the whole screen when `rect` is `None`.
pub fn capture_region_raw(rect: Option<(i32, i32, i32, i32)>) -> Result<(Vec<u8>, usize, usize)> {
    match rect {
        None => capture_screen_full_rgba(),
        Some(r) => capture_screen_region_fast(r),
    }
}

/// Raw RGBA of a rect given as (left, top, right, bottom).
pub fn capture_screen_region_fast(rect: (i32, i32, i32, i32)) -> Result<(Vec<u8>, usize, usize)> {
    let (l, t, r, b) = rect;
    if r <= l || b <= t {
        return Err(CoreError::Platform(
            "capture_screen_region_fast: degenerate rect".into(),
        ));
    }
    // X11 sends coordinates as i16 and extents as u16. Casting an out-of-range
    // value wraps silently, and the server will happily return a real,
    // plausible-looking image of the WRONG part of the screen -- worse than an
    // error, because nothing indicates it. Reject before the cast. The bound is
    // i16::MAX, the largest coordinate the protocol can express.
    const MAX_COORD: i32 = i16::MAX as i32;
    if l < 0 || t < 0 || r > MAX_COORD || b > MAX_COORD {
        return Err(CoreError::Platform(format!(
            "capture region ({l},{t})-({r},{b}) is outside the range X11 can express              (0..{MAX_COORD}); coordinates would wrap and capture the wrong area"
        )));
    }
    match session_kind() {
        SessionKind::X11 => x11_capture(Some((l, t, (r - l) as u32, (b - t) as u32))),
        SessionKind::Wayland => {
            // No region parameter on the Screenshot portal: capture and crop.
            let (full, fw, fh) = portal_capture()?;
            let l0 = l.max(0) as usize;
            let t0 = t.max(0) as usize;
            let cw = ((r.max(0) as usize).min(fw)).saturating_sub(l0);
            let ch = ((b.max(0) as usize).min(fh)).saturating_sub(t0);
            if cw == 0 || ch == 0 {
                return Err(CoreError::Platform("crop region is empty".into()));
            }
            Ok((crop_rgba(&full, fw, l0, t0, cw, ch), cw, ch))
        }
        SessionKind::Headless => Err(CoreError::Unsupported(
            "no display session detected, so the screen cannot be captured".into(),
        )),
    }
}

/// Full screen downsampled to a square-ish `target_dim` long edge, raw RGBA.
pub fn capture_screen_downsample_raw(target_dim: usize) -> Result<Vec<u8>> {
    let (rgba, w, h) = capture_screen_full_rgba()?;
    let dim = target_dim.max(1) as u32;
    let img = RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| CoreError::Platform("capture buffer does not match dimensions".into()))?;
    let scaled = DynamicImage::ImageRgba8(img)
        .resize(dim, dim, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Ok(scaled.into_raw())
}

/// Encoded capture of a region, optionally downscaled to `max_dim` on the long edge.
pub fn capture_screen_region(
    rect: Option<(i32, i32, i32, i32)>,
    max_dim: Option<u32>,
    format: CaptureFormat,
) -> Result<Vec<u8>> {
    let (rgba, w, h) = capture_region_raw(rect)?;
    let img = RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| CoreError::Platform("capture buffer does not match dimensions".into()))?;
    let img = match max_dim {
        Some(m) if w as u32 > m || h as u32 > m => DynamicImage::ImageRgba8(img)
            .resize(m, m, image::imageops::FilterType::Triangle)
            .to_rgba8(),
        _ => img,
    };
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    encode_rgba(&img.into_raw(), iw, ih, format)
}

/// Capture one window, seeing through anything on top of it where possible.
///
/// This is the `PrintWindow` analogue, and it matters more than it sounds:
/// background mode drives windows that are *by definition* not in front, so
/// verifying an action by cropping the screen would mostly photograph whatever
/// is covering them.
///
/// Two paths, best first:
///
/// 1. **XComposite** — `NameWindowPixmap` hands back the window's own backing
///    pixmap, which is exactly what a compositor draws from. Reading that gives
///    the window's real content regardless of what overlaps it. Every modern
///    desktop runs a compositor, so this is the normal case.
/// 2. **Screen-rectangle crop** — the fallback when composite is unavailable
///    (bare X11, no compositor). Here an occluded window really does capture
///    whatever is on top, so the result must be treated as "cannot verify".
pub fn capture_window_printwindow(hwnd_raw: isize) -> Result<(Vec<u8>, usize, usize)> {
    if let Some(shot) = composite_capture(hwnd_raw) {
        return Ok(shot);
    }
    let rect = crate::system::window::window_rect(hwnd_raw).ok_or(CoreError::WindowGone)?;
    capture_screen_region_fast(rect)
}

/// Capture a window's backing pixmap via XComposite. `None` when unavailable,
/// so the caller can fall back rather than fail.
fn composite_capture(hwnd_raw: isize) -> Option<(Vec<u8>, usize, usize)> {
    use x11rb::protocol::composite::{ConnectionExt as CompositeExt, Redirect};

    if session_kind() != SessionKind::X11 {
        return None;
    }

    // Ghost handles are AT-SPI addresses, so go through the same PID/title join
    // the window-manager module uses to reach a real X11 window id.
    let (pid, title) = crate::system::window::window_identity(hwnd_raw)?;
    let win = crate::wm::x11_window_for(pid, title.as_deref())?;

    let (conn, _screen) = x11rb::connect(None).ok()?;
    conn.composite_query_version(0, 4).ok()?.reply().ok()?;

    // Redirecting is only needed when nothing else has: with a compositor
    // running the window is already redirected and this is a harmless no-op.
    // Automatic keeps the window drawn on screen as usual.
    let _ = conn.composite_redirect_window(win, Redirect::AUTOMATIC);
    conn.flush().ok()?;

    let geom = conn.get_geometry(win).ok()?.reply().ok()?;
    let (w, h) = (geom.width, geom.height);
    if w == 0 || h == 0 {
        return None;
    }

    let pixmap = conn.generate_id().ok()?;
    conn.composite_name_window_pixmap(win, pixmap).ok()?.check().ok()?;

    let reply = conn.get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, w, h, !0).ok()?.reply();
    let _ = conn.free_pixmap(pixmap);
    let reply = reply.ok()?;

    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for px in reply.data.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    if rgba.len() < w as usize * h as usize * 4 {
        return None;
    }
    Some((rgba, w as usize, h as usize))
}

/// Region capture with numbered Set-of-Marks badges, JPEG-encoded.
pub fn capture_region_marked_jpeg(
    rect: Option<(i32, i32, i32, i32)>,
    marks_native: &[Mark],
    max_dim: u32,
    quality: u8,
) -> Result<Vec<u8>> {
    let (rgba, w, h) = capture_region_raw(rect)?;
    let mut img = RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| CoreError::Platform("capture buffer does not match dimensions".into()))?;

    // Marks are drawn at native resolution, then the whole image is scaled, so
    // badge positions stay aligned with the elements they label.
    super::marks::draw_marks(&mut img, marks_native);

    let long_edge = img.width().max(img.height());
    let img = if long_edge > max_dim && max_dim > 0 {
        DynamicImage::ImageRgba8(img)
            .resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
            .to_rgba8()
    } else {
        img
    };
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    encode_rgba(&img.into_raw(), iw, ih, CaptureFormat::Jpeg(quality))
}

/// Identical to `ghost_core::capture::screen::crop_rgba`.
///
/// Out-of-bounds rows are left as transparent black rather than panicking: a
/// capture that races a window move must not take down the server.
pub fn crop_rgba(
    full: &[u8],
    full_w: usize,
    l: usize,
    t: usize,
    cw: usize,
    ch: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let src = ((t + y) * full_w + l) * 4;
        let dst = y * cw * 4;
        if src + cw * 4 <= full.len() {
            out[dst..dst + cw * 4].copy_from_slice(&full[src..src + cw * 4]);
        }
    }
    out
}

pub(crate) fn encode_rgba(rgba: &[u8], w: usize, h: usize, format: CaptureFormat) -> Result<Vec<u8>> {
    let img = RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec())
        .ok_or_else(|| CoreError::Platform("capture buffer does not match dimensions".into()))?;
    let mut out = Cursor::new(Vec::new());
    match format {
        CaptureFormat::Png => DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| CoreError::Platform(format!("png encode: {e}")))?,
        CaptureFormat::Jpeg(q) => {
            let rgb = DynamicImage::ImageRgba8(img).to_rgb8();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q.clamp(1, 100));
            enc.encode_image(&rgb)
                .map_err(|e| CoreError::Platform(format!("jpeg encode: {e}")))?;
        }
    }
    Ok(out.into_inner())
}

// ---------------------------------------------------------------------- X11

fn x11_capture(region: Option<(i32, i32, u32, u32)>) -> Result<(Vec<u8>, usize, usize)> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| CoreError::Platform(format!("X11 connect: {e}")))?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let (x, y, w, h) = region.unwrap_or((
        0,
        0,
        screen.width_in_pixels as u32,
        screen.height_in_pixels as u32,
    ));
    if w == 0 || h == 0 {
        return Err(CoreError::Platform("capture region is empty".into()));
    }

    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, x as i16, y as i16, w as u16, h as u16, !0)
        .map_err(|e| CoreError::Platform(format!("get_image: {e}")))?
        .reply()
        .map_err(|e| {
            CoreError::Platform(format!(
                "get_image failed ({e}). Under XWayland the root window generally cannot be \
                 captured; a real X11 session is required"
            ))
        })?;

    // Z_PIXMAP at depth 24/32 is BGRX/BGRA on little-endian servers.
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in reply.data.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    if rgba.len() < (w as usize) * (h as usize) * 4 {
        return Err(CoreError::Platform("short image reply from X server".into()));
    }
    Ok((rgba, w as usize, h as usize))
}

// ------------------------------------------------------------------- portal

/// Hard ceiling on a Screenshot-portal request.
///
/// Generous because the first call on a session can raise a consent dialog, but
/// finite because the alternative is fatal: see [`portal_capture`].
const PORTAL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(45);

/// Capture via the Screenshot portal, bounded.
///
/// The bound is not a nicety. `ghost-mcp` runs on a current-thread runtime and
/// dispatches tool calls serially, so a blocking wait here does not stall one
/// call -- it wedges the entire MCP server, with no way for the client to
/// recover and (since the portal is asynchronous and silent) no log line
/// explaining why. A compositor that never answers -- busy, wedged
/// `xdg-desktop-portal`, or a consent dialog raised on another workspace where
/// nobody sees it -- must degrade to a timeout error, not a dead server.
///
/// The worker is deliberately **detached** rather than joined: if the portal
/// never replies, that thread stays parked forever. Leaking one parked thread is
/// strictly better than blocking the process, and the runtime it owns is dropped
/// with it if the call ever does return.
fn portal_capture() -> Result<(Vec<u8>, usize, usize)> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(Vec<u8>, usize, usize)>>(1);

    std::thread::Builder::new()
        .name("ghost-screenshot".into())
        .spawn(move || {
            let result = (|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| CoreError::Platform(format!("screenshot runtime: {e}")))?;

                rt.block_on(async {
                let response = ashpd::desktop::screenshot::Screenshot::request()
                    .interactive(false)
                    .modal(false)
                    .send()
                    .await
                    .map_err(portal_err)?
                    .response()
                    .map_err(portal_err)?;

                let path = file_path_from_uri(&response.uri().to_string()).ok_or_else(|| {
                    CoreError::Platform("screenshot portal returned a non-file URI".into())
                })?;

                let bytes = std::fs::read(&path)
                    .map_err(|e| CoreError::Platform(format!("reading screenshot: {e}")))?;
                // The portal writes a temp file it does not clean up.
                let _ = std::fs::remove_file(&path);

                let img = image::load_from_memory(&bytes)
                    .map_err(|e| CoreError::Platform(format!("decoding screenshot: {e}")))?
                    .to_rgba8();
                    let (w, h) = img.dimensions();
                    Ok((img.into_raw(), w as usize, h as usize))
                })
            })();
            // Receiver may already have timed out and gone; that is fine.
            let _ = tx.send(result);
        })
        .map_err(|e| CoreError::Platform(format!("could not spawn screenshot thread: {e}")))?;

    match rx.recv_timeout(PORTAL_CAPTURE_TIMEOUT) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(CoreError::Platform(format!(
            "the Screenshot portal did not respond within {}s. Check that \
             xdg-desktop-portal and xdg-desktop-portal-gnome are running, and that no consent \
             dialog is waiting on another workspace",
            PORTAL_CAPTURE_TIMEOUT.as_secs()
        ))),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(CoreError::WorkerPanic("screenshot thread panicked".into()))
        }
    }
}

/// Turn a `file://` URI into a path.
///
/// Written by hand rather than pulling in a URI crate: the portal always returns
/// a local `file://` URI, and the only real-world complication is
/// percent-encoding in temp paths.
fn file_path_from_uri(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Empty host ("file:///tmp/x") is the only readable form.
    let rest = rest.strip_prefix('/')?;
    Some(std::path::PathBuf::from(percent_decode(&format!("/{rest}"))))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn portal_err(e: ashpd::Error) -> CoreError {
    CoreError::Platform(format!(
        "Screenshot portal: {e}. Ensure xdg-desktop-portal and xdg-desktop-portal-gnome are \
         installed and running"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_returns_the_requested_size() {
        let full = vec![7u8; 100 * 80 * 4];
        let out = crop_rgba(&full, 100, 10, 10, 20, 30);
        assert_eq!(out.len(), 20 * 30 * 4);
    }

    #[test]
    fn crop_past_the_end_leaves_zeroes_instead_of_panicking() {
        // A capture racing a window move must not take down the server.
        let full = vec![1u8; 10 * 10 * 4];
        let out = crop_rgba(&full, 10, 0, 8, 10, 10);
        assert_eq!(out.len(), 10 * 10 * 4);
    }

    #[test]
    fn degenerate_rects_are_rejected() {
        assert!(capture_screen_region_fast((10, 10, 10, 20)).is_err());
        assert!(capture_screen_region_fast((10, 10, 20, 10)).is_err());
    }

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(
            file_path_from_uri("file:///tmp/shot.png"),
            Some(std::path::PathBuf::from("/tmp/shot.png"))
        );
    }

    #[test]
    fn percent_encoded_paths_decode() {
        assert_eq!(
            file_path_from_uri("file:///tmp/my%20shot.png"),
            Some(std::path::PathBuf::from("/tmp/my shot.png"))
        );
    }

    #[test]
    fn non_file_uris_are_rejected() {
        assert!(file_path_from_uri("https://example.com/x.png").is_none());
    }

    #[test]
    fn a_trailing_percent_does_not_panic() {
        assert_eq!(percent_decode("abc%"), "abc%");
        assert_eq!(percent_decode("abc%z"), "abc%z");
    }

    #[test]
    fn encoding_a_mismatched_buffer_errors_rather_than_panicking() {
        assert!(encode_rgba(&[0u8; 10], 100, 100, CaptureFormat::Png).is_err());
    }
}

