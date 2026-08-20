//! Per-window background capture.
//!
//! The DXGI desktop-duplication path in `screen.rs` captures whatever is currently
//! composited on the monitor. That is wrong for background automation twice over: it
//! cannot see a window the user has covered with their own work, and it hands the
//! agent a picture of the user's private screen.
//!
//! `PrintWindow` asks a specific window to render itself into an offscreen DC. The
//! window is not raised, not focused, not un-occluded - the user never sees anything
//! happen. `PW_RENDERFULLCONTENT` is what makes this work for DWM-composited windows
//! (Chromium, Electron, WPF); without that flag those render blank.

use super::screen::{bgra_to_rgba, encode_png_rgba};
use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
// PrintWindow lives under Storage::Xps in the windows crate, not WindowsAndMessaging.
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, IsIconic, IsWindow, IsWindowVisible,
};

/// Render the window's full content including DWM-composited (GPU) surfaces.
/// Without this, Chromium/Electron/WPF windows come back blank.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(0x0000_0002);
/// Restrict the render to the client area, excluding the title bar and frame.
const PW_CLIENTONLY: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(0x0000_0001);

/// Guard against a bogus rect (a minimized or mid-teardown window can report
/// enormous or negative dimensions) allocating an absurd bitmap.
const MAX_DIMENSION: i32 = 16_384;

/// RAII wrappers so an early return can never leak a GDI object. GDI handle leaks
/// are process-wide and eventually make every window in the app fail to paint, which
/// is a spectacularly confusing failure to debug from a screenshot function.
struct ScreenDc(windows::Win32::Graphics::Gdi::HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(None, self.0);
        }
    }
}

struct MemDc(windows::Win32::Graphics::Gdi::HDC);
impl Drop for MemDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}

/// Capture a single window as PNG bytes without raising, focusing, or un-occluding it.
///
/// `client_only` excludes the title bar and border. Returns `CaptureFailed` when the
/// window refuses to render (some hardware-overlay video surfaces and protected
/// windows come back blank) so the caller can fall back rather than act on a black
/// image it mistakes for real content.
pub fn capture_window(hwnd: HWND, client_only: bool) -> Result<Vec<u8>, CoreError> {
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err(CoreError::WindowGone);
        }
        if IsIconic(hwnd).as_bool() {
            return Err(CoreError::CaptureFailed(
                "window is minimized; restore it or capture a different window".into(),
            ));
        }

        let mut rect = RECT::default();
        if client_only {
            GetClientRect(hwnd, &mut rect)
        } else {
            GetWindowRect(hwnd, &mut rect)
        }
        .map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: "GetWindowRect/GetClientRect",
        })?;

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return Err(CoreError::CaptureFailed(format!(
                "window has no drawable area ({width}x{height})"
            )));
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(CoreError::CaptureFailed(format!(
                "window reports implausible size {width}x{height}"
            )));
        }

        let screen = ScreenDc(GetDC(None));
        if screen.0 .0.is_null() {
            return Err(CoreError::CaptureFailed("GetDC(NULL) returned null".into()));
        }
        let mem = MemDc(CreateCompatibleDC(screen.0));
        if mem.0 .0.is_null() {
            return Err(CoreError::CaptureFailed("CreateCompatibleDC failed".into()));
        }
        // Compatible with the *screen* DC, not the memory DC: a bitmap made compatible
        // with a fresh memory DC is 1bpp monochrome and the capture comes back black.
        let bmp = Bitmap(CreateCompatibleBitmap(screen.0, width, height));
        if bmp.0 .0.is_null() {
            return Err(CoreError::CaptureFailed("CreateCompatibleBitmap failed".into()));
        }
        let prev = SelectObject(mem.0, HGDIOBJ(bmp.0 .0));

        let mut flags = PW_RENDERFULLCONTENT;
        if client_only {
            flags = PRINT_WINDOW_FLAGS(flags.0 | PW_CLIENTONLY.0);
        }
        let printed = PrintWindow(hwnd, mem.0, flags).as_bool();

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height requests a top-down DIB, matching the row order
                // `bgra_to_rgba` expects. A positive height yields a vertically
                // mirrored screenshot.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let stride = (width as usize) * 4;
        let mut buffer = vec![0u8; stride * height as usize];
        let scanlines = GetDIBits(
            mem.0,
            bmp.0,
            0,
            height as u32,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem.0, prev);

        if scanlines == 0 {
            return Err(CoreError::CaptureFailed("GetDIBits returned no scanlines".into()));
        }
        if is_blank(&buffer) {
            // Hardware-overlay video surfaces and some protected windows render
            // nothing into a PrintWindow DC, so the buffer comes back all black. A
            // black image that looks like a successful screenshot is the worst
            // outcome - an agent will act on it. Try to recover a real image, and
            // fail loudly if that is not possible. The GDI objects are owned by RAII
            // wrappers, so returning early here still releases them.
            return capture_via_screen_crop(hwnd, rect, printed);
        }

        let rgba = bgra_to_rgba(&buffer, width as usize, height as usize, stride);
        encode_png_rgba(&rgba, width as u32, height as u32)
    }
}

/// Recover a window image by cropping a full-screen capture, when `PrintWindow`
/// rendered nothing.
///
/// Only valid if the window is genuinely visible and unoccluded on the user's screen:
/// a screen crop of a covered window returns whatever is on top of it, which would be
/// both wrong and a privacy leak. So this verifies the window actually owns the pixels
/// at its own centre before trusting the crop, and otherwise reports the failure.
fn capture_via_screen_crop(
    hwnd: HWND,
    rect: RECT,
    printed: bool,
) -> Result<Vec<u8>, CoreError> {
    let detail = if printed {
        "PrintWindow succeeded but produced an empty image (hardware overlay or \
         protected content)"
    } else {
        "PrintWindow declined to render this window"
    };

    if !owns_its_own_centre(hwnd, rect) {
        return Err(CoreError::CaptureFailed(format!(
            "{detail}, and it is off-screen or covered by another window, so a screen \
             crop would show the wrong content. Read this window's state with \
             describe_screen or document_text instead."
        )));
    }

    let png = super::screen::capture_screen()?;
    let img = image::load_from_memory(&png)
        .map_err(|e| CoreError::CaptureFailed(format!("{detail}; screen decode failed: {e}")))?;
    let (sw, sh) = (img.width() as i32, img.height() as i32);

    // Clamp to the captured screen: a window may extend past the monitor edge.
    let left = rect.left.max(0).min(sw);
    let top = rect.top.max(0).min(sh);
    let right = rect.right.max(0).min(sw);
    let bottom = rect.bottom.max(0).min(sh);
    let (w, h) = (right - left, bottom - top);
    if w <= 0 || h <= 0 {
        return Err(CoreError::CaptureFailed(format!(
            "{detail}, and the window lies outside the captured screen area"
        )));
    }

    let cropped = image::imageops::crop_imm(&img, left as u32, top as u32, w as u32, h as u32)
        .to_image();
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(cropped)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| CoreError::CaptureFailed(format!("{detail}; re-encode failed: {e}")))?;
    Ok(out)
}

/// Whether `hwnd` (or one of its children) is the window actually on screen at the
/// centre of its own rectangle. False means it is covered, minimized, or off-screen.
fn owns_its_own_centre(hwnd: HWND, rect: RECT) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let pt = POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        };
        let top = WindowFromPoint(pt);
        if top.0.is_null() {
            return false;
        }
        // WindowFromPoint returns the deepest child; compare roots so a hit on a
        // child control still counts as this window.
        GetAncestor(top, GA_ROOT) == GetAncestor(hwnd, GA_ROOT)
    }
}

/// True when every pixel is fully black, i.e. the window rendered nothing.
///
/// Sampled rather than exhaustive: a 4K window is 33MB and scanning all of it on
/// every capture would cost more than the capture itself. Every 64th pixel still
/// catches a genuinely empty buffer with certainty (all zeroes is all zeroes) while
/// a real screenshot trips the check on its first non-black sample.
fn is_blank(bgra: &[u8]) -> bool {
    bgra.chunks_exact(4)
        .step_by(64)
        .all(|px| px[0] == 0 && px[1] == 0 && px[2] == 0)
}

/// Whether a window can plausibly be captured right now, for pre-flight checks.
pub fn is_capturable(hwnd: HWND) -> bool {
    unsafe {
        IsWindow(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_of_a_dead_window_is_an_error_not_a_blank_image() {
        let err = capture_window(HWND(std::ptr::null_mut()), false);
        assert!(matches!(err, Err(CoreError::WindowGone)));
    }

    #[test]
    fn dead_window_is_not_capturable() {
        assert!(!is_capturable(HWND(std::ptr::null_mut())));
    }

    #[test]
    fn all_zero_buffer_is_blank() {
        assert!(is_blank(&vec![0u8; 4 * 1024]));
    }

    #[test]
    fn buffer_with_any_sampled_colour_is_not_blank() {
        let mut buf = vec![0u8; 4 * 1024];
        buf[0] = 1; // first sampled pixel
        assert!(!is_blank(&buf));
    }

    #[test]
    fn opaque_black_with_content_in_alpha_is_still_blank() {
        // PrintWindow leaves the 4th byte undefined, so alpha must not count as
        // content or every failed capture would look successful.
        let mut buf = vec![0u8; 4 * 1024];
        for px in buf.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        assert!(is_blank(&buf));
    }

    #[test]
    fn render_full_content_flag_is_set_for_composited_windows() {
        assert_eq!(PW_RENDERFULLCONTENT.0, 2);
        assert_eq!(PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT.0 | PW_CLIENTONLY.0).0, 3);
    }
}
