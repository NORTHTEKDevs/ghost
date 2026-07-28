//! Screen and window capture, and the point-vs-pixel arithmetic around it.
//!
//! | Ghost operation | Apple API |
//! | --- | --- |
//! | full screen | `CGWindowListCreateImage` over `CGDisplayBounds(CGMainDisplayID())` |
//! | one window | `CGWindowListCreateImage` with `kCGWindowListOptionIncludingWindow` |
//! | raw pixels | `CGImageGetDataProvider` → `CFData` (via `CGImage::data`) |
//! | display geometry | `CGDisplayBounds` (points) and `CGDisplayPixelsWide` (pixels) |
//!
//! # Retina: the scale factor is the bug to worry about
//!
//! macOS has two coordinate spaces and they differ by an integer factor on every
//! Apple Silicon laptop:
//!
//! - **Points** — what Accessibility reports, what `CGWindowListCreateImage`
//!   takes, and what a click must be expressed in.
//! - **Pixels** — what the returned `CGImage` actually contains. On a 2x Retina
//!   display a 400x300-point window yields an 800x600-pixel image.
//!
//! Getting this wrong is the classic silent Retina failure: everything "works",
//! but every coordinate derived from an image is off by exactly 2x, so clicks land
//! in the upper-left quadrant of their target. Ghost therefore never infers the
//! scale from the display — it measures it per capture as
//! `image_pixels / requested_points` ([`Capture::scale`]) and exposes
//! [`Capture::to_points`] so any pixel coordinate that comes back from vision
//! grounding is converted with the factor that actually applied to *that* image.
//!
//! `ScreenCaptureKit` is the modern replacement for `CGWindowListCreateImage` and
//! is the right eventual home for this module, but it is an async,
//! block-based Objective-C API. `CGWindowListCreateImage` is deprecated yet fully
//! functional, and it is synchronous C — a far smaller surface to get right on a
//! machine we cannot compile against locally. See `docs/mac-testing.md`.

use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::window::{
    create_image, kCGNullWindowID, kCGWindowImageBoundsIgnoreFraming,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
    kCGWindowListOptionOnScreenOnly,
};

use super::error::{MacError, MacResult};
use super::perms::require_screen_recording;
use crate::types::{Point, Rect};

/// A captured image plus the point-space region it came from.
///
/// Holding both is what makes the scale recoverable; an image alone cannot tell a
/// caller whether it is 1x or 2x.
pub struct Capture {
    /// PNG-encoded image bytes.
    pub png: Vec<u8>,
    /// Pixel dimensions of the image.
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// The region, in points, that was asked for.
    pub region: Rect,
    /// Whether every pixel was the same colour.
    ///
    /// Computed here, while the RGBA buffer is still in hand, because that buffer is
    /// dropped once the PNG exists and answering this later would mean decoding the
    /// PNG again. It matters because macOS returns a valid, fully black image rather
    /// than an error when Screen Recording is missing — see [`is_blank`].
    pub blank: bool,
}

impl Capture {
    /// Pixels per point for this specific capture: 1.0 on a non-Retina display,
    /// 2.0 on Retina.
    ///
    /// Measured from the image rather than queried from the display, so a window
    /// straddling a Retina and a non-Retina monitor still yields the factor that
    /// really applied.
    pub fn scale(&self) -> f64 {
        scale_factor(self.pixel_width, self.region.width())
    }

    /// Convert a pixel coordinate within this image into a screen point.
    ///
    /// This is the conversion vision grounding needs: a VLM picks a location in
    /// the image, and a click must happen at the corresponding screen point.
    pub fn to_points(&self, pixel: Point) -> Point {
        let scale = self.scale();
        Point {
            x: self.region.left + (pixel.x as f64 / scale).round() as i32,
            y: self.region.top + (pixel.y as f64 / scale).round() as i32,
        }
    }
}

/// Pixels per point, guarding the degenerate cases.
///
/// Returns 1.0 rather than infinity or NaN for a zero-width region, because a
/// poisoned scale factor would silently corrupt every coordinate derived from it.
pub fn scale_factor(pixel_width: u32, point_width: i32) -> f64 {
    if point_width <= 0 || pixel_width == 0 {
        return 1.0;
    }
    pixel_width as f64 / point_width as f64
}

/// Convert a point coordinate to pixels for a given scale.
pub fn points_to_pixels(points: i32, scale: f64) -> i32 {
    (points as f64 * scale).round() as i32
}

/// Convert a pixel coordinate to points for a given scale.
pub fn pixels_to_points(pixels: i32, scale: f64) -> i32 {
    if scale <= 0.0 {
        return pixels;
    }
    (pixels as f64 / scale).round() as i32
}

/// The main display's backing scale factor, from `CGDisplayPixelsWide` divided by
/// `CGDisplayBounds().size.width`.
///
/// Uses CoreGraphics rather than AppKit's `NSScreen.backingScaleFactor` so that no
/// `NSApplication` needs to exist — Ghost's CLI is not a GUI app. The value is the
/// same.
pub fn main_display_scale() -> f64 {
    let display = CGDisplay::main();
    let bounds = display.bounds();
    scale_factor(display.pixels_wide() as u32, bounds.size.width.round() as i32)
}

/// The main display's bounds in points — `CGDisplayBounds(CGMainDisplayID())`.
pub fn main_display_bounds() -> Rect {
    let bounds = CGDisplay::main().bounds();
    Rect {
        left: bounds.origin.x.round() as i32,
        top: bounds.origin.y.round() as i32,
        right: (bounds.origin.x + bounds.size.width).round() as i32,
        bottom: (bounds.origin.y + bounds.size.height).round() as i32,
    }
}

/// Capture the whole main display.
pub fn capture_screen() -> MacResult<Capture> {
    let region = main_display_bounds();
    capture_region(region)
}

/// Capture an arbitrary screen region, given in points.
pub fn capture_region(region: Rect) -> MacResult<Capture> {
    require_screen_recording()?;
    if region.width() == 0 || region.height() == 0 {
        return Err(MacError::InvalidArgument(format!(
            "capture region has zero area: {region:?}"
        )));
    }
    let image = create_image(
        cg_rect(region),
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
        kCGWindowImageBoundsIgnoreFraming,
    )
    .ok_or_else(|| MacError::CaptureFailed(format!("region {region:?}")))?;

    encode(image, region)
}

/// Capture a single window by its `CGWindowID`, including any part of it that is
/// covered by another window.
///
/// `CGRect::null()` tells CoreGraphics to use the window's own bounds.
pub fn capture_window(window_id: u32, region: Rect) -> MacResult<Capture> {
    require_screen_recording()?;
    let image = create_image(
        CGRect::new(&CGPoint::new(f64::INFINITY, f64::INFINITY), &CGSize::new(0.0, 0.0)),
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming,
    )
    .ok_or_else(|| MacError::CaptureFailed(format!("window id {window_id}")))?;

    encode(image, region)
}

fn cg_rect(region: Rect) -> CGRect {
    CGRect::new(
        &CGPoint::new(region.left as f64, region.top as f64),
        &CGSize::new(region.width() as f64, region.height() as f64),
    )
}

/// Turn a `CGImage` into PNG bytes.
///
/// The pixel data comes back as BGRA (CoreGraphics' native order on Apple
/// Silicon), so the channels are swapped before encoding — otherwise every capture
/// looks blue-tinted, which is a subtle enough error to survive a casual eyeball
/// check. `bytes_per_row` is honoured rather than assumed to be `width * 4`, since
/// CoreGraphics pads rows for alignment.
fn encode(image: core_graphics::image::CGImage, region: Rect) -> MacResult<Capture> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(MacError::CaptureUnusable(format!(
            "{width}x{height} — the window may be minimized or on another Space"
        )));
    }

    let bytes_per_row = image.bytes_per_row();
    let bits_per_pixel = image.bits_per_pixel();
    if bits_per_pixel != 32 {
        return Err(MacError::CaptureUnusable(format!(
            "{bits_per_pixel} bits per pixel; only 32-bit BGRA is handled"
        )));
    }

    let data = image.data();
    let bytes = data.bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = y * bytes_per_row;
        for x in 0..width {
            let i = row + x * 4;
            let Some(px) = bytes.get(i..i + 4) else {
                return Err(MacError::CaptureUnusable(format!(
                    "pixel buffer ended early at ({x},{y}) of {width}x{height}"
                )));
            };
            // BGRA -> RGBA.
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(px[3]);
        }
    }

    let blank = is_blank(&rgba);
    let png = encode_png(&rgba, width as u32, height as u32)?;
    Ok(Capture {
        png,
        pixel_width: width as u32,
        pixel_height: height as u32,
        region,
        blank,
    })
}

/// PNG-encode an RGBA buffer.
///
/// Split out from [`encode`] so it can be tested without a window server.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> MacResult<Vec<u8>> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(MacError::Encode(format!(
            "buffer is {} bytes, expected {expected} for {width}x{height} RGBA",
            rgba.len()
        )));
    }
    let buffer = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| MacError::Encode("could not build an RGBA image".to_string()))?;

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| MacError::Encode(e.to_string()))?;
    Ok(png)
}

/// Whether a captured image is entirely one colour.
///
/// `ghost doctor --mac` uses this: macOS returns a **valid, fully black image**
/// rather than an error when Screen Recording is missing, so "did it decode" is
/// not sufficient to prove capture works. A blank frame is the actual symptom.
pub fn is_blank(rgba: &[u8]) -> bool {
    if rgba.len() < 4 {
        return true;
    }
    let first = &rgba[0..4];
    rgba.chunks_exact(4).all(|px| px == first)
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn scale_factor_is_two_on_retina_and_one_on_a_standard_display() {
        assert_eq!(scale_factor(800, 400), 2.0);
        assert_eq!(scale_factor(400, 400), 1.0);
        // A 3x display (some external panels report this).
        assert_eq!(scale_factor(1200, 400), 3.0);
    }

    #[test]
    fn a_degenerate_region_yields_one_not_infinity_or_nan() {
        // A poisoned scale factor would silently corrupt every derived
        // coordinate, so it must never be non-finite.
        for (px, pt) in [(0u32, 0i32), (800, 0), (0, 400), (800, -10)] {
            let s = scale_factor(px, pt);
            assert!(s.is_finite(), "scale_factor({px},{pt}) = {s}");
            assert_eq!(s, 1.0);
        }
    }

    #[test]
    fn point_pixel_conversion_round_trips_at_every_common_scale() {
        for scale in [1.0, 2.0, 3.0] {
            for points in [0, 1, 37, 400, 1440, -50] {
                let px = points_to_pixels(points, scale);
                assert_eq!(
                    pixels_to_points(px, scale),
                    points,
                    "round trip failed at scale {scale} for {points} points"
                );
            }
        }
    }

    #[test]
    fn pixels_to_points_survives_a_nonsense_scale() {
        assert_eq!(pixels_to_points(100, 0.0), 100);
        assert_eq!(pixels_to_points(100, -2.0), 100);
    }

    #[test]
    fn retina_pixel_coordinates_map_back_onto_the_right_screen_point() {
        // This is the exact arithmetic that makes vision grounding land on the
        // target instead of a quarter of the way into it. A 400x300-point window
        // at (100,50) captured on a 2x display is an 800x600-pixel image.
        let capture = Capture {
            png: Vec::new(),
            blank: true,
            pixel_width: 800,
            pixel_height: 600,
            region: Rect {
                left: 100,
                top: 50,
                right: 500,
                bottom: 350,
            },
        };
        assert_eq!(capture.scale(), 2.0);

        // Image origin is the region's origin.
        assert_eq!(capture.to_points(Point { x: 0, y: 0 }), Point { x: 100, y: 50 });
        // Image centre is the region's centre, not its quarter point.
        assert_eq!(
            capture.to_points(Point { x: 400, y: 300 }),
            Point { x: 300, y: 200 }
        );
        assert_eq!(capture.to_points(Point { x: 400, y: 300 }), capture.region.center());
        // Far corner.
        assert_eq!(
            capture.to_points(Point { x: 800, y: 600 }),
            Point { x: 500, y: 350 }
        );
    }

    #[test]
    fn non_retina_capture_maps_pixels_straight_through() {
        let capture = Capture {
            png: Vec::new(),
            blank: true,
            pixel_width: 400,
            pixel_height: 300,
            region: Rect {
                left: 0,
                top: 0,
                right: 400,
                bottom: 300,
            },
        };
        assert_eq!(capture.scale(), 1.0);
        assert_eq!(
            capture.to_points(Point { x: 123, y: 45 }),
            Point { x: 123, y: 45 }
        );
    }

    #[test]
    fn a_capture_on_a_display_left_of_main_keeps_its_negative_offset() {
        let capture = Capture {
            png: Vec::new(),
            blank: true,
            pixel_width: 800,
            pixel_height: 600,
            region: Rect {
                left: -1920,
                top: -100,
                right: -1520,
                bottom: 200,
            },
        };
        assert_eq!(capture.scale(), 2.0);
        assert_eq!(
            capture.to_points(Point { x: 0, y: 0 }),
            Point { x: -1920, y: -100 }
        );
        assert_eq!(
            capture.to_points(Point { x: 400, y: 300 }),
            Point { x: -1720, y: 50 }
        );
    }

    #[test]
    fn png_encoding_produces_a_real_decodable_png() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let png = encode_png(&rgba, 4, 4).expect("encode");
        // PNG magic number.
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        let decoded = image::load_from_memory(&png).expect("decode");
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    #[test]
    fn png_encoding_rejects_a_mismatched_buffer_instead_of_producing_garbage() {
        let err = encode_png(&[0u8; 10], 4, 4).unwrap_err();
        assert!(err.to_string().contains("expected"), "{err}");
    }

    #[test]
    fn blank_detection_catches_the_all_black_frame_a_missing_grant_produces() {
        // macOS returns a valid, fully black image rather than an error when
        // Screen Recording is not granted, so this check is the real test.
        let black = vec![0u8; 4 * 16];
        assert!(is_blank(&black));

        let white = vec![255u8; 4 * 16];
        assert!(is_blank(&white));

        let mut mixed = vec![0u8; 4 * 16];
        mixed[20] = 200;
        assert!(!is_blank(&mixed));

        assert!(is_blank(&[]));
    }
}
