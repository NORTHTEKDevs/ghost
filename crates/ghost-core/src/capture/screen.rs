use std::sync::Mutex;
use crate::error::CoreError;

// DESIGN: CaptureContext caches the full DXGI duplication session.
//
// D3D11 device + context are initialized once and reused across all captures.
// IDXGIOutputDuplication + the matching staging texture are also cached:
// - DuplicateOutput is expensive (~15ms) and creates a persistent "desktop
//   duplication session" that should be held open between frames.
// - The staging texture dimensions are tied to the duplication session and
//   don't change while the session is valid; reusing it saves CreateTexture2D
//   on every call.
//
// Re-acquire path: on DXGI_ERROR_ACCESS_LOST (0x887A0026) or
// DXGI_ERROR_ACCESS_DENIED (0x887A002B) the duplication is dropped and
// recreated exactly once before returning an error.
//
// Thread-safety note: all capture functions hold CAPTURE_STATE mutex for the
// entire duration of the capture, so only one thread uses the DXGI session
// at a time. COM is MTA (COINIT_MULTITHREADED) which is correct for D3D11/DXGI.
// IUIAutomation uses STA on the same OS thread (the tokio main thread); DXGI
// capture is MTA-safe and does not interact with the STA apartment.

struct CaptureContext {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    output1: windows::Win32::Graphics::Dxgi::IDXGIOutput1,
    /// Cached duplication session. None means it must be (re-)acquired.
    duplication: Option<windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication>,
    /// Cached staging texture and its dimensions. None until first frame.
    staging: Option<(windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, usize, usize)>,
    /// How many times the duplication has been re-acquired (telemetry / cfg(test)).
    #[cfg(test)]
    pub reacquire_count: u32,
    #[cfg(not(test))]
    _reacquire_count: u32,
}

// Safety: same as before — D3D11 COM objects use internal ref-counting safe
// across threads; only one thread holds the mutex at a time.
unsafe impl Send for CaptureContext {}
unsafe impl Sync for CaptureContext {}

static CAPTURE_STATE: Mutex<Option<CaptureContext>> = Mutex::new(None);

// DXGI_ERROR_ACCESS_LOST / DXGI_ERROR_ACCESS_DENIED are used via
// windows::Win32::Graphics::Dxgi directly in capture_rgba_inner.

/// Image output format for `capture_screen_region`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    Png,
    /// JPEG with quality 0-100 (75 is a good default for vision payloads).
    Jpeg(u8),
}

/// Capture the primary monitor as PNG bytes (full screen).
pub fn capture_screen() -> Result<Vec<u8>, CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    let (rgba, w, h) = capture_rgba(guard.as_mut().unwrap())?;
    encode_png_rgba(&rgba, w as u32, h as u32)
}

/// Capture the primary monitor as a tightly-packed RGBA buffer with dimensions.
/// Used by OCR and other consumers that want raw pixels without encoding.
pub fn capture_screen_full_rgba() -> Result<(Vec<u8>, usize, usize), CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    capture_rgba(guard.as_mut().unwrap())
}

/// Capture the primary monitor, downsample to `target_dim x target_dim` cells,
/// and return the raw averaged BGRA bytes (target_dim * target_dim * 4 bytes).
/// This is the fast path for idle detection and perceptual hashing: no PNG encode needed.
/// The BGRA channel order is preserved from the DXGI surface to avoid an extra swap.
pub fn capture_screen_downsample_raw(target_dim: usize) -> Result<Vec<u8>, CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    let (rgba, w, h) = capture_rgba(guard.as_mut().unwrap())?;
    // Downsample to target_dim x target_dim by averaging cells.
    let dim = target_dim.max(1);
    let channels = 4usize;
    let mut out = vec![0u8; dim * dim * channels];
    if w == 0 || h == 0 {
        return Ok(out);
    }
    let cell_w = (w / dim).max(1);
    let cell_h = (h / dim).max(1);
    for by in 0..dim {
        for bx in 0..dim {
            let mut acc = [0u64; 4];
            let mut n: u64 = 0;
            for y in (by * cell_h)..(((by + 1) * cell_h).min(h)) {
                for x in (bx * cell_w)..(((bx + 1) * cell_w).min(w)) {
                    let idx = (y * w + x) * channels;
                    if idx + 3 < rgba.len() {
                        acc[0] += rgba[idx] as u64;
                        acc[1] += rgba[idx + 1] as u64;
                        acc[2] += rgba[idx + 2] as u64;
                        acc[3] += rgba[idx + 3] as u64;
                        n += 1;
                    }
                }
            }
            let dst = (by * dim + bx) * channels;
            if n > 0 {
                out[dst]     = (acc[0] / n) as u8;
                out[dst + 1] = (acc[1] / n) as u8;
                out[dst + 2] = (acc[2] / n) as u8;
                out[dst + 3] = (acc[3] / n) as u8;
            }
        }
    }
    Ok(out)
}

/// In-place RGBA -> BGRA channel swap. Used by WinRT OCR which requires Bgra8.
pub fn rgba_to_bgra_in_place(buf: &mut [u8]) {
    let mut i = 0;
    while i + 3 < buf.len() {
        buf.swap(i, i + 2);
        i += 4;
    }
}

/// Capture a region of the screen, optionally downscale, and encode.
/// `rect` = (left, top, right, bottom) in pixels; `None` = full screen.
/// `max_dim` = longest-edge size after downscale; `None` = no downscale.
/// Typical vision payload: rect=focused-window-bbox, max_dim=Some(768), format=Jpeg(75).
/// 10-50x smaller payloads + 3-5x faster vision inference vs full PNG.
pub fn capture_screen_region(
    rect: Option<(i32, i32, i32, i32)>,
    max_dim: Option<u32>,
    format: CaptureFormat,
) -> Result<Vec<u8>, CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    let (full_rgba, full_w, full_h) = capture_rgba(guard.as_mut().unwrap())?;

    let (rgba, w, h) = if let Some((l, t, r, b)) = rect {
        let l = l.max(0) as usize;
        let t = t.max(0) as usize;
        let r = (r as usize).min(full_w);
        let b = (b as usize).min(full_h);
        if r <= l || b <= t {
            return Err(CoreError::Win32 { code: 0, context: "invalid region rect" });
        }
        let cw = r - l;
        let ch = b - t;
        let mut crop = vec![0u8; cw * ch * 4];
        for y in 0..ch {
            let src_off = ((t + y) * full_w + l) * 4;
            let dst_off = y * cw * 4;
            crop[dst_off..dst_off + cw * 4]
                .copy_from_slice(&full_rgba[src_off..src_off + cw * 4]);
        }
        (crop, cw, ch)
    } else {
        (full_rgba, full_w, full_h)
    };

    let (final_rgba, fw, fh) = if let Some(target) = max_dim {
        let long_edge = w.max(h) as u32;
        if long_edge > target {
            let scale = target as f32 / long_edge as f32;
            let nw = ((w as f32) * scale).round().max(1.0) as u32;
            let nh = ((h as f32) * scale).round().max(1.0) as u32;
            let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
                .ok_or(CoreError::Win32 { code: 0, context: "RgbaImage from_raw" })?;
            let resized = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
            (resized.into_raw(), nw, nh)
        } else {
            (rgba, w as u32, h as u32)
        }
    } else {
        (rgba, w as u32, h as u32)
    };

    match format {
        CaptureFormat::Png => encode_png_rgba(&final_rgba, fw, fh),
        CaptureFormat::Jpeg(quality) => encode_jpeg_rgba(&final_rgba, fw, fh, quality),
    }
}

fn init_capture_state() -> Result<CaptureContext, CoreError> {
    unsafe {
        use windows::Win32::Graphics::Direct3D::*;
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::*;
        use windows::core::Interface;

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        ).map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "D3D11CreateDevice" })?;

        let device = device.ok_or(CoreError::Win32 { code: 0, context: "D3D11 device null" })?;
        let context = context.ok_or(CoreError::Win32 { code: 0, context: "D3D11 context null" })?;

        let dxgi_device: IDXGIDevice = device.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IDXGIDevice cast" })?;
        let adapter = dxgi_device.GetAdapter()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "GetAdapter" })?;
        let output: IDXGIOutput = adapter.EnumOutputs(0)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "EnumOutputs" })?;
        let output1: IDXGIOutput1 = output.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IDXGIOutput1 cast" })?;

        Ok(CaptureContext {
            device,
            context,
            output1,
            duplication: None,  // acquired lazily on first capture_rgba call
            staging: None,
            #[cfg(test)]
            reacquire_count: 0,
            #[cfg(not(test))]
            _reacquire_count: 0,
        })
    }
}

/// Acquire (or re-acquire) `IDXGIOutputDuplication` on `ctx`.
/// Called when `ctx.duplication` is None or after an access-lost error.
unsafe fn acquire_duplication(ctx: &mut CaptureContext) -> Result<(), CoreError> {
    use windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication;
    // Drop any stale duplication before creating a new one.
    ctx.duplication = None;
    ctx.staging = None;
    let dup: IDXGIOutputDuplication = ctx.output1.DuplicateOutput(&ctx.device)
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "DuplicateOutput" })?;
    ctx.duplication = Some(dup);
    #[cfg(test)]
    { ctx.reacquire_count += 1; }
    #[cfg(not(test))]
    { ctx._reacquire_count += 1; }
    Ok(())
}

/// Capture the primary monitor as a tightly-packed RGBA buffer plus dimensions.
/// Reuses the cached IDXGIOutputDuplication and staging texture where possible.
/// Re-acquires on DXGI_ERROR_ACCESS_LOST / DXGI_ERROR_ACCESS_DENIED (once per call).
fn capture_rgba(ctx: &mut CaptureContext) -> Result<(Vec<u8>, usize, usize), CoreError> {
    unsafe { capture_rgba_inner(ctx, true) }
}

unsafe fn capture_rgba_inner(
    ctx: &mut CaptureContext,
    allow_retry: bool,
) -> Result<(Vec<u8>, usize, usize), CoreError> {
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::Interface;

    // Ensure duplication is acquired.
    if ctx.duplication.is_none() {
        acquire_duplication(ctx)?;
    }
    let dup = ctx.duplication.as_ref().unwrap();

    let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    let acquire_result = dup.AcquireNextFrame(500, &mut frame_info, &mut resource);

    if let Err(ref e) = acquire_result {
        let hr = e.code();
        // Compare against windows-rs HRESULT constants for access-lost errors.
        let is_access_error = hr == windows::Win32::Graphics::Dxgi::DXGI_ERROR_ACCESS_LOST
            || hr == windows::Win32::Graphics::Dxgi::DXGI_ERROR_ACCESS_DENIED;
        if allow_retry && is_access_error {
            // Drop the bad session and retry once with a fresh duplication.
            acquire_duplication(ctx)?;
            return capture_rgba_inner(ctx, false);
        }
        return Err(CoreError::Win32 { code: hr.0 as u32, context: "AcquireNextFrame" });
    }

    let resource = resource.ok_or(CoreError::Win32 { code: 0, context: "frame resource null" })?;
    let texture: ID3D11Texture2D = resource.cast()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "texture cast" })?;

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    texture.GetDesc(&mut desc);
    let width = desc.Width as usize;
    let height = desc.Height as usize;

    // Reuse or create the staging texture. Invalidate if dimensions changed (monitor config change).
    let need_new_staging = match &ctx.staging {
        Some((_, sw, sh)) => *sw != width || *sh != height,
        None => true,
    };
    if need_new_staging {
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut new_staging: Option<ID3D11Texture2D> = None;
        ctx.device.CreateTexture2D(&staging_desc, None, Some(&mut new_staging))
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateTexture2D staging" })?;
        let s = new_staging.ok_or(CoreError::Win32 { code: 0, context: "staging texture null" })?;
        ctx.staging = Some((s, width, height));
    }
    let (staging, _, _) = ctx.staging.as_ref().unwrap();

    let resource_view: ID3D11Resource = texture.cast()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "texture resource cast" })?;
    let staging_view: ID3D11Resource = staging.cast()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "staging resource cast" })?;
    ctx.context.CopyResource(&staging_view, &resource_view);

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.context.Map(&staging_view, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Map" })?;

    let pitch = mapped.RowPitch as usize;
    let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * height);
    let rgba = bgra_to_rgba(data, width, height, pitch);

    ctx.context.Unmap(&staging_view, 0);
    // Release the frame back to the duplication — keep the session alive for next call.
    let dup = ctx.duplication.as_ref().unwrap();
    dup.ReleaseFrame()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "ReleaseFrame" })?;

    Ok((rgba, width, height))
}

/// Convert BGRA pixel data (with row pitch) to tightly-packed RGBA.
/// Exported for testing.
pub(crate) fn bgra_to_rgba(data: &[u8], width: usize, height: usize, pitch: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let src = y * pitch + x * 4;
            let dst = (y * width + x) * 4;
            rgba[dst]     = data[src + 2]; // R <- B
            rgba[dst + 1] = data[src + 1]; // G <- G
            rgba[dst + 2] = data[src];     // B <- R
            rgba[dst + 3] = 255;           // A
        }
    }
    rgba
}

/// Encode tightly-packed RGBA bytes as PNG. Exported for testing.
pub(crate) fn encode_png_rgba(rgba_data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CoreError> {
    use image::RgbaImage;
    let img = RgbaImage::from_raw(width, height, rgba_data.to_vec())
        .ok_or(CoreError::Win32 { code: 0, context: "RgbaImage from_raw" })?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|_| CoreError::Win32 { code: 0, context: "PNG encode" })?;
    Ok(png_bytes)
}

/// Encode tightly-packed RGBA bytes as JPEG with the given quality (0-100).
/// JPEG is lossy and discards alpha; alpha channel is dropped.
pub(crate) fn encode_jpeg_rgba(rgba_data: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, CoreError> {
    use image::RgbaImage;
    use image::codecs::jpeg::JpegEncoder;
    let img = RgbaImage::from_raw(width, height, rgba_data.to_vec())
        .ok_or(CoreError::Win32 { code: 0, context: "RgbaImage from_raw" })?;
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut buf = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut buf, quality);
    enc.encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
        .map_err(|_| CoreError::Win32 { code: 0, context: "JPEG encode" })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgba_swaps_channels() {
        // 1x1 pixel: BGRA = [0x10, 0x20, 0x30, 0xFF]
        // Expected RGBA = [0x30, 0x20, 0x10, 0xFF]
        let bgra = vec![0x10u8, 0x20, 0x30, 0xFF];
        let rgba = bgra_to_rgba(&bgra, 1, 1, 4);
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 0xFF]);
    }

    #[test]
    fn bgra_to_rgba_alpha_always_255() {
        // Source alpha byte (index 3) is ignored; output alpha is always 255
        let bgra = vec![0x00u8, 0x00, 0x00, 0x00]; // transparent black BGRA
        let rgba = bgra_to_rgba(&bgra, 1, 1, 4);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn bgra_to_rgba_2x1_with_pitch() {
        // 2x1, pitch=8 (no padding), two pixels: red and blue in BGRA
        // Red in BGRA = [0x00, 0x00, 0xFF, 0xFF]
        // Blue in BGRA = [0xFF, 0x00, 0x00, 0xFF]
        let bgra = vec![
            0x00u8, 0x00, 0xFF, 0xFF, // pixel 0: BGRA red
            0xFF,   0x00, 0x00, 0xFF, // pixel 1: BGRA blue
        ];
        let rgba = bgra_to_rgba(&bgra, 2, 1, 8);
        assert_eq!(&rgba[0..4], &[0xFF, 0x00, 0x00, 0xFF]); // RGBA red
        assert_eq!(&rgba[4..8], &[0x00, 0x00, 0xFF, 0xFF]); // RGBA blue
    }

    #[test]
    fn bgra_to_rgba_output_size_matches_dimensions() {
        let bgra = vec![0u8; 4 * 16 * 9]; // 16x9 pixels
        let rgba = bgra_to_rgba(&bgra, 16, 9, 4 * 16);
        assert_eq!(rgba.len(), 16 * 9 * 4);
    }

    #[test]
    fn bgra_to_rgba_with_row_padding() {
        // 1x2 image, pitch=8 (4 bytes padding per row after 1 pixel)
        let mut bgra = vec![0u8; 16]; // 2 rows * 8 bytes pitch
        bgra[0] = 0x10; bgra[1] = 0x20; bgra[2] = 0x30; bgra[3] = 0xFF; // row 0
        bgra[8] = 0x40; bgra[9] = 0x50; bgra[10] = 0x60; bgra[11] = 0xFF; // row 1
        let rgba = bgra_to_rgba(&bgra, 1, 2, 8);
        assert_eq!(&rgba[0..4], &[0x30, 0x20, 0x10, 0xFF]); // row 0 RGBA
        assert_eq!(&rgba[4..8], &[0x60, 0x50, 0x40, 0xFF]); // row 1 RGBA
    }

    #[test]
    fn encode_png_rgba_produces_valid_png_header() {
        // 1x1 red pixel
        let rgba = vec![0xFF, 0x00, 0x00, 0xFF];
        let png = encode_png_rgba(&rgba, 1, 1).unwrap();
        // PNG magic bytes: 0x89 P N G \r \n 0x1A \n
        assert_eq!(&png[0..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn encode_png_rgba_size_mismatch_returns_error() {
        // 2x2 requires 16 bytes, give only 4
        let rgba = vec![0xFF, 0x00, 0x00, 0xFF];
        assert!(encode_png_rgba(&rgba, 2, 2).is_err());
    }

    #[test]
    fn encode_png_rgba_roundtrip_dimensions() {
        // 4x3 solid green image
        let rgba = vec![0x00u8, 0xFF, 0x00, 0xFF].repeat(4 * 3);
        let png = encode_png_rgba(&rgba, 4, 3).unwrap();
        assert!(!png.is_empty());
        // Verify PNG is well-formed by checking IHDR chunk width/height
        // IHDR starts at byte 16: width (4 bytes big-endian), height (4 bytes)
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!(w, 4);
        assert_eq!(h, 3);
    }

    /// Verify DXGI error constants have the expected raw HRESULT values.
    #[test]
    fn dxgi_error_constants_have_correct_values() {
        use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_ACCESS_DENIED};
        // 0x887A0026 and 0x887A002B are the canonical DXGI access-lost HRESULTs.
        assert_eq!(DXGI_ERROR_ACCESS_LOST.0 as u32, 0x887A_0026);
        assert_eq!(DXGI_ERROR_ACCESS_DENIED.0 as u32, 0x887A_002B);
    }

    /// Live test: repeated captures reuse the duplication session (reacquire_count stays low).
    /// Marked ignore because it requires a real display/GPU.
    #[test]
    #[ignore]
    fn repeated_captures_reuse_duplication_session() {
        // First capture initializes + acquires duplication (count: 1).
        // Subsequent captures must NOT re-acquire (count stays 1).
        let _ = capture_screen().expect("first capture");
        let _ = capture_screen().expect("second capture");
        let _ = capture_screen().expect("third capture");
        let guard = CAPTURE_STATE.lock().unwrap();
        if let Some(ctx) = guard.as_ref() {
            // reacquire_count should be 1 (one initial acquire, no re-acquires).
            assert_eq!(ctx.reacquire_count, 1,
                "expected exactly 1 duplication acquire; repeated captures must reuse the session");
        }
    }
}
