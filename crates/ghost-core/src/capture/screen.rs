use std::sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use crate::error::CoreError;

// ---------------------------------------------------------------------------
// GDI BitBlt fallback capture
// ---------------------------------------------------------------------------
// Used when DXGI Desktop Duplication returns all-black frames. This happens on
// systems with HDR displays, certain NVIDIA driver versions, and some RDP/VM configs.
// GDI BitBlt is slower (~30-100ms vs ~5ms for DXGI) but works universally.
//
// NOTE: GDI captures in BGRA order. We swap to RGBA on read.

// ---------------------------------------------------------------------------
// RAII guards for GDI handles — guarantee release on every path including panic.
// ---------------------------------------------------------------------------

struct ScreenDcGuard(windows::Win32::Graphics::Gdi::HDC);
impl Drop for ScreenDcGuard {
    fn drop(&mut self) {
        unsafe {
            use windows::Win32::Graphics::Gdi::ReleaseDC;
            use windows::Win32::Foundation::HWND;
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), self.0);
        }
    }
}

struct MemDcGuard(windows::Win32::Graphics::Gdi::HDC);
impl Drop for MemDcGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(self.0);
        }
    }
}

struct BitmapGuard(windows::Win32::Graphics::Gdi::HBITMAP);
impl Drop for BitmapGuard {
    fn drop(&mut self) {
        unsafe {
            use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

/// Capture the primary monitor using GDI BitBlt.
/// Returns tightly-packed RGBA plus width and height.
pub fn capture_screen_gdi() -> Result<(Vec<u8>, usize, usize), CoreError> {
    let (width, height) = primary_screen_size()?;
    capture_rect_gdi(0, 0, width, height)
}

/// Primary monitor pixel size via GetSystemMetrics.
fn primary_screen_size() -> Result<(usize, usize), CoreError> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) } as usize;
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as usize;
    if width == 0 || height == 0 {
        return Err(CoreError::Win32 { code: 0, context: "GDI GetSystemMetrics returned 0" });
    }
    Ok((width, height))
}

/// Virtual-screen bounds (left, top, right, bottom) spanning ALL monitors.
/// Left/top can be negative when a secondary monitor sits left of / above the primary.
pub fn virtual_screen_bounds() -> (i32, i32, i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    unsafe {
        let l = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let t = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (l, t, l + GetSystemMetrics(SM_CXVIRTUALSCREEN), t + GetSystemMetrics(SM_CYVIRTUALSCREEN))
    }
}

/// True if `rect` is a VALID (non-inverted) rect lying entirely within `bounds`
/// (both (l, t, r, b)). The validity check (right>left, bottom>top) matters: an
/// inverted rect like (50,0,-1,100) must NOT be judged "on primary" — otherwise
/// the DXGI crop path clamps the negative edge to the screen edge and silently
/// returns a huge region instead of erroring. Pure + unit-tested.
pub fn rect_within(rect: (i32, i32, i32, i32), bounds: (i32, i32, i32, i32)) -> bool {
    let (l, t, r, b) = rect;
    r > l && b > t
        && l >= bounds.0 && t >= bounds.1 && r <= bounds.2 && b <= bounds.3
}

/// True if `rect` lies entirely on the primary monitor — the fast DXGI path
/// only covers the primary output (EnumOutputs(0)).
///
/// FOLLOW-UP: this trusts index 0 == primary and SM_CXSCREEN as its size. On
/// unusual adapters EnumOutputs(0) may not be the Windows-designated primary;
/// to fully close that gap, match each output's DXGI_OUTPUT_DESC.DesktopCoordinates
/// against this rect instead of assuming index 0. Pre-existing behavior; the GDI
/// virtual-rect fallback already covers the off-primary case correctly.
pub fn rect_on_primary(rect: (i32, i32, i32, i32)) -> bool {
    match primary_screen_size() {
        Ok((w, h)) => rect_within(rect, (0, 0, w as i32, h as i32)),
        Err(_) => false,
    }
}

/// Capture an arbitrary virtual-screen rect via GDI BitBlt — works on ANY
/// monitor (GetDC(NULL) spans the virtual desktop; negative source coords are
/// valid for displays left of / above the primary). Rect is clamped to the
/// virtual-screen bounds. Slower than DXGI (~30-100ms) but the only path for
/// off-primary windows until per-output duplication lands.
pub fn capture_virtual_rect_gdi(
    l: i32, t: i32, r: i32, b: i32,
) -> Result<(Vec<u8>, usize, usize), CoreError> {
    let (vl, vt, vr, vb) = virtual_screen_bounds();
    let l = l.max(vl);
    let t = t.max(vt);
    let r = r.min(vr);
    let b = b.min(vb);
    if r <= l || b <= t {
        return Err(CoreError::Win32 { code: 0, context: "capture_virtual_rect_gdi: rect outside virtual screen" });
    }
    capture_rect_gdi(l, t, (r - l) as usize, (b - t) as usize)
}

/// GDI BitBlt capture of `width` x `height` pixels starting at virtual-screen
/// coordinate (src_x, src_y). Returns tightly-packed RGBA.
fn capture_rect_gdi(
    src_x: i32,
    src_y: i32,
    width: usize,
    height: usize,
) -> Result<(Vec<u8>, usize, usize), CoreError> {
    unsafe {
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, SelectObject, BitBlt, GetDIBits,
            BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY, BI_RGB, HGDIOBJ,
        };
        use windows::Win32::Foundation::HWND;

        if width == 0 || height == 0 {
            return Err(CoreError::Win32 { code: 0, context: "capture_rect_gdi: zero-sized rect" });
        }

        let raw_screen_dc = windows::Win32::Graphics::Gdi::GetDC(HWND(std::ptr::null_mut()));
        if raw_screen_dc.is_invalid() {
            return Err(CoreError::Win32 { code: 0, context: "GDI GetDC failed" });
        }
        // RAII: screen DC is released on all paths from here.
        let screen_dc = ScreenDcGuard(raw_screen_dc);

        let raw_mem_dc = CreateCompatibleDC(screen_dc.0);
        if raw_mem_dc.is_invalid() {
            return Err(CoreError::Win32 { code: 0, context: "GDI CreateCompatibleDC failed" });
        }
        // RAII: mem DC deleted on all paths from here.
        let mem_dc = MemDcGuard(raw_mem_dc);

        let raw_bmp = CreateCompatibleBitmap(screen_dc.0, width as i32, height as i32);
        if raw_bmp.is_invalid() {
            return Err(CoreError::Win32 { code: 0, context: "GDI CreateCompatibleBitmap failed" });
        }
        // RAII: bitmap deleted on all paths from here.
        let bmp = BitmapGuard(raw_bmp);

        // LOW: allocate before SelectObject so an OOM panic can't fire while
        // bmp is selected into mem_dc (which would violate the GDI contract on drop).
        let mut bgra = vec![0u8; width * height * 4];

        // Select our bitmap into the mem DC; save the old one so we can restore it
        // before deleting the DC (required by GDI contract).
        let old_gdi = SelectObject(mem_dc.0, HGDIOBJ(bmp.0.0));
        // Restore the original bitmap on drop so DeleteDC is safe.
        // We do this inline after GetDIBits rather than in a Drop because we need
        // the bmp handle to still be alive at restore time.

        let blit_result = BitBlt(mem_dc.0, 0, 0, width as i32, height as i32, screen_dc.0, src_x, src_y, SRCCOPY);

        // Read pixels regardless of BitBlt result (partial data is better than nothing).
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // negative = top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        };
        let scanlines = GetDIBits(
            mem_dc.0,
            bmp.0,
            0,
            height as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // Restore the original bitmap before the RAII guards delete mem_dc and bmp.
        // This is required: a DC must not be deleted while a non-stock GDI object
        // is selected into it.
        let _ = SelectObject(mem_dc.0, old_gdi);
        // bmp and mem_dc and screen_dc are all dropped (freed) here when this scope exits.

        if blit_result.is_err() {
            return Err(CoreError::Win32 { code: 0, context: "GDI BitBlt failed" });
        }
        if scanlines == 0 {
            return Err(CoreError::Win32 { code: 0, context: "GDI GetDIBits returned 0 scanlines" });
        }

        // BGRA (GDI) → RGBA
        let rgba = bgra_to_rgba(&bgra, width, height, width * 4);
        Ok((rgba, width, height))
    }
}

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
// Re-acquire path: on DXGI_ERROR_ACCESS_LOST (0x887A0026),
// DXGI_ERROR_ACCESS_DENIED (0x887A002B), or DXGI_ERROR_INVALID_CALL
// (0x887A0001 — returned when a frame was not released before the next
// AcquireNextFrame) the duplication is dropped and recreated exactly once.
// DXGI_ERROR_INVALID_CALL is added so a previously-wedged session self-heals.
//
// Frame release: a FrameGuard RAII type ensures ReleaseFrame is called on
// every exit path after AcquireNextFrame succeeds, preventing the wedge.
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
    /// Last successfully captured frame. Returned on DXGI_ERROR_WAIT_TIMEOUT so
    /// callers never block longer than the reduced AcquireNextFrame timeout.
    last_frame: Option<(Vec<u8>, usize, usize)>,
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

/// Sticky flag: once DXGI is observed returning all-black frames, skip DXGI and go
/// straight to GDI. Re-probed every `DXGI_REPROBE_INTERVAL` captures so a transient
/// black-frame event (resume from sleep, driver reset) doesn't permanently downgrade
/// every future capture to the 30-100ms GDI path.
static DXGI_ALWAYS_BLACK: AtomicBool = AtomicBool::new(false);
static GDI_CAPTURES_SINCE_BLACK: AtomicUsize = AtomicUsize::new(0);
const DXGI_REPROBE_INTERVAL: usize = 100;

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
    let mut guard = CAPTURE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    let (rgba, w, h) = capture_rgba(guard.as_mut().unwrap())?;
    encode_png_rgba(&rgba, w as u32, h as u32)
}

/// Capture the primary monitor as a tightly-packed RGBA buffer with dimensions.
/// Used by OCR and other consumers that want raw pixels without encoding.
pub fn capture_screen_full_rgba() -> Result<(Vec<u8>, usize, usize), CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    capture_rgba(guard.as_mut().unwrap())
}

/// Capture an on-primary-monitor rect as tightly-packed RGBA, converting ONLY
/// that region (skips converting/cloning the whole frame — ~20x cheaper convert
/// for a small window, see benches/convert.rs). Caller must pass an on-primary
/// rect (off-primary goes through the GDI virtual path in capture_region_raw).
pub fn capture_screen_region_fast(rect: (i32, i32, i32, i32)) -> Result<(Vec<u8>, usize, usize), CoreError> {
    let (w, h) = primary_screen_size()?;
    let l = rect.0.max(0) as usize;
    let t = rect.1.max(0) as usize;
    let r = (rect.2.max(0) as usize).min(w);
    let b = (rect.3.max(0) as usize).min(h);
    if r <= l || b <= t {
        return Err(CoreError::Win32 { code: 0, context: "capture_screen_region_fast: degenerate rect" });
    }
    let mut guard = CAPTURE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    capture_rgba_cropped(guard.as_mut().unwrap(), Some((l, t, r, b)))
}

/// Capture the primary monitor, downsample to `target_dim x target_dim` cells,
/// and return the raw averaged BGRA bytes (target_dim * target_dim * 4 bytes).
/// This is the fast path for idle detection and perceptual hashing: no PNG encode needed.
/// The BGRA channel order is preserved from the DXGI surface to avoid an extra swap.
pub fn capture_screen_downsample_raw(target_dim: usize) -> Result<Vec<u8>, CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap_or_else(|e| e.into_inner());
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
    // Any region rect: GDI BitBlt of exactly that rect. Measured flat ~16.5ms vs
    // the DXGI full-frame acquire+convert+crop path which costs more and hits a
    // large-window cliff (see tests/capture_latency_probe.rs). GDI also works on
    // any monitor (DXGI only duplicates the primary output). Full-screen (rect =
    // None) still uses DXGI below, where its cached duplicator beats a full BitBlt.
    if let Some((l, t, r, b)) = rect {
        let (rgba, w, h) = capture_virtual_rect_gdi(l, t, r, b)?;
        return encode_region(rgba, w, h, max_dim, format);
    }
    let mut guard = CAPTURE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    // rect is None here (all Some rects returned via the GDI path above).
    let (full_rgba, full_w, full_h) = capture_rgba(guard.as_mut().unwrap())?;
    encode_region(full_rgba, full_w, full_h, max_dim, format)
}

/// Downscale (to `max_dim` longest edge, if given) then encode raw RGBA.
/// Shared by the DXGI and GDI-virtual-rect paths of `capture_screen_region`.
fn encode_region(
    rgba: Vec<u8>,
    w: usize,
    h: usize,
    max_dim: Option<u32>,
    format: CaptureFormat,
) -> Result<Vec<u8>, CoreError> {
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
            last_frame: None,
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
///
/// Black-frame fallback: DXGI Desktop Duplication returns all-zero frames on some
/// systems (HDR monitors, certain NVIDIA driver versions, RDP sessions). When detected,
/// the function falls back to GDI BitBlt capture which works universally.
///
/// Sticky optimisation: once is_frame_black() fires, DXGI_ALWAYS_BLACK is set and
/// all subsequent calls skip DXGI entirely (avoiding ~50ms of wasted work per frame).
fn capture_rgba(ctx: &mut CaptureContext) -> Result<(Vec<u8>, usize, usize), CoreError> {
    capture_rgba_cropped(ctx, None)
}

/// Capture, optionally converting only a sub-rect (`crop` = clamped l,t,r,b in
/// primary-monitor pixels). The crop path converts far fewer pixels for a small
/// window (~20x faster than full-frame convert per the `convert` bench) and
/// skips the full-frame cache clone. GDI/black fallbacks use the region GDI path.
fn capture_rgba_cropped(
    ctx: &mut CaptureContext,
    crop: Option<(usize, usize, usize, usize)>,
) -> Result<(Vec<u8>, usize, usize), CoreError> {
    let gdi_fallback = || match crop {
        Some((l, t, r, b)) => capture_virtual_rect_gdi(l as i32, t as i32, r as i32, b as i32),
        None => capture_screen_gdi(),
    };
    // Fast path: DXGI known-broken — skip it, but re-probe periodically in case the
    // black frames were transient (sleep/resume, driver reset).
    if DXGI_ALWAYS_BLACK.load(Ordering::Relaxed) {
        let n = GDI_CAPTURES_SINCE_BLACK.fetch_add(1, Ordering::Relaxed) + 1;
        if !n.is_multiple_of(DXGI_REPROBE_INTERVAL) {
            return gdi_fallback();
        }
        tracing::debug!(gdi_captures = n, "re-probing DXGI after sticky black-frame flag");
        DXGI_ALWAYS_BLACK.store(false, Ordering::Relaxed);
    }
    let result = match unsafe { capture_rgba_inner(ctx, true, crop) } {
        Ok(r) => r,
        // A region capture can't serve a DXGI static-screen timeout from the
        // full-frame cache (region captures don't populate it, and a stale-size
        // cache returns Err). Rather than propagate — which would silently null
        // out act-then-verify — fall back to the GDI region capture, which always
        // produces a correct (if slower) frame. Full captures keep propagating.
        Err(e) => {
            if crop.is_some() {
                return gdi_fallback();
            }
            return Err(e);
        }
    };
    // If the captured frame is entirely black, DXGI is not working correctly on this
    // system. Set the sticky flag so future calls skip straight to GDI.
    if is_frame_black(&result.0) {
        tracing::warn!("DXGI capture returned all-black frame; setting sticky GDI-only flag and falling back to GDI BitBlt (HDR/driver/RDP compat)");
        DXGI_ALWAYS_BLACK.store(true, Ordering::Relaxed);
        return gdi_fallback();
    }
    Ok(result)
}

/// Returns true if the RGBA buffer is entirely black (all R, G, B channels are zero).
/// Used to detect the DXGI first-frame blank artifact.
fn is_frame_black(rgba: &[u8]) -> bool {
    if rgba.is_empty() {
        return false; // empty is not a black frame we can skip
    }
    // Sample at most 256 evenly-spaced pixels to avoid scanning huge buffers.
    let pixels = rgba.len() / 4;
    let step = (pixels / 256).max(1);
    for i in (0..pixels).step_by(step) {
        let base = i * 4;
        if rgba[base] != 0 || rgba[base + 1] != 0 || rgba[base + 2] != 0 {
            return false;
        }
    }
    true
}

/// RAII guard: calls ReleaseFrame on every exit path after AcquireNextFrame succeeds.
/// Prevents the DXGI_ERROR_INVALID_CALL wedge caused by early-return ? ops holding a frame.
struct FrameGuard<'a>(&'a windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication);
impl Drop for FrameGuard<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.ReleaseFrame() };
    }
}

unsafe fn capture_rgba_inner(
    ctx: &mut CaptureContext,
    allow_retry: bool,
    crop: Option<(usize, usize, usize, usize)>, // clamped l,t,r,b in monitor px
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
    // Reduced from 500ms to 50ms: on static screens we return last_frame instead of blocking.
    let acquire_result = dup.AcquireNextFrame(50, &mut frame_info, &mut resource);

    if let Err(ref e) = acquire_result {
        let hr = e.code();
        // DXGI_ERROR_WAIT_TIMEOUT: no new frame within the window — return cached
        // frame (cropped to the region if a crop was requested).
        if hr == DXGI_ERROR_WAIT_TIMEOUT {
            if let Some((ref cached, cw, ch)) = ctx.last_frame {
                return match crop {
                    Some((l, t, r, b)) => {
                        // Re-clamp against the CACHED frame's own dims (not the
                        // current monitor size) — they can differ if resolution
                        // changed since the last full capture. Degenerate → Err so
                        // capture_rgba_cropped falls back to a fresh GDI region.
                        let l = l.min(cw); let t = t.min(ch);
                        let r = r.min(cw); let b = b.min(ch);
                        if r <= l || b <= t {
                            return Err(CoreError::Win32 { code: 0, context: "cached-frame crop degenerate after re-clamp" });
                        }
                        Ok((crop_rgba(cached, cw, l, t, r - l, b - t), r - l, b - t))
                    }
                    None => Ok((cached.clone(), cw, ch)),
                };
            }
            // No prior frame; re-try once with a fresh acquire (session just started).
            return Err(CoreError::Win32 { code: hr.0 as u32, context: "AcquireNextFrame timeout, no cached frame" });
        }
        // Access-lost / invalid-call (wedged): drop + recreate + retry once.
        let is_access_error = hr == DXGI_ERROR_ACCESS_LOST
            || hr == DXGI_ERROR_ACCESS_DENIED
            || hr == DXGI_ERROR_INVALID_CALL;
        if allow_retry && is_access_error {
            acquire_duplication(ctx)?;
            return capture_rgba_inner(ctx, false, crop);
        }
        return Err(CoreError::Win32 { code: hr.0 as u32, context: "AcquireNextFrame" });
    }

    // Frame acquired — RAII guard ensures ReleaseFrame runs on every exit path below.
    let dup_ref = ctx.duplication.as_ref().unwrap();
    let _frame_guard = FrameGuard(dup_ref);

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

    let out = match crop {
        // Convert only the requested sub-rect — ~20x fewer pixels for a small
        // window (see benches/convert.rs). Clamp to frame bounds defensively.
        Some((l, t, r, b)) => {
            let l = l.min(width);
            let t = t.min(height);
            let r = r.min(width);
            let b = b.min(height);
            if r <= l || b <= t {
                ctx.context.Unmap(&staging_view, 0);
                return Err(CoreError::Win32 { code: 0, context: "capture crop rect degenerate after clamp" });
            }
            let (cw, ch) = (r - l, b - t);
            let rgba = bgra_to_rgba_region(data, pitch, l, t, cw, ch);
            ctx.context.Unmap(&staging_view, 0);
            // Do NOT populate last_frame from a region capture (wrong dims for a
            // later full request); the full-frame cache is only for full captures.
            (rgba, cw, ch)
        }
        None => {
            let rgba = bgra_to_rgba(data, width, height, pitch);
            ctx.context.Unmap(&staging_view, 0);
            // Cache the frame so DXGI_ERROR_WAIT_TIMEOUT on static screens returns it.
            ctx.last_frame = Some((rgba.clone(), width, height));
            (rgba, width, height)
        }
    };
    // FrameGuard drop releases the frame — do NOT call ReleaseFrame explicitly here.
    Ok(out)
}

/// Convert BGRA pixel data (with row pitch) to tightly-packed RGBA.
/// Exported for testing.
pub fn bgra_to_rgba(data: &[u8], width: usize, height: usize, pitch: usize) -> Vec<u8> {
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

/// Convert ONLY the `cw`x`ch` sub-rect at (`src_x`, `src_y`) of a BGRA frame
/// (with row `pitch`) into a tightly-packed `cw`x`ch` RGBA buffer. Identical
/// output to `bgra_to_rgba(full)` followed by a crop, but touches only the
/// requested pixels — for a small window on a large monitor this converts a
/// tiny fraction of the frame instead of the whole thing. The caller must
/// ensure the rect lies within the frame bounds.
pub fn bgra_to_rgba_region(
    data: &[u8], pitch: usize, src_x: usize, src_y: usize, cw: usize, ch: usize,
) -> Vec<u8> {
    let mut rgba = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let row_src = (src_y + y) * pitch + src_x * 4;
        let row_dst = y * cw * 4;
        for x in 0..cw {
            let src = row_src + x * 4;
            let dst = row_dst + x * 4;
            rgba[dst]     = data[src + 2]; // R <- B
            rgba[dst + 1] = data[src + 1]; // G
            rgba[dst + 2] = data[src];     // B <- R
            rgba[dst + 3] = 255;           // A
        }
    }
    rgba
}

/// Crop an already-packed RGBA frame (`full_w` wide) to a sub-rect. Used to serve
/// a region request from the cached full frame on a DXGI static-screen timeout.
pub fn crop_rgba(full: &[u8], full_w: usize, l: usize, t: usize, cw: usize, ch: usize) -> Vec<u8> {
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

/// Capture the given screen rect, downscale to `max_dim` longest-edge, draw the
/// Set-of-Marks badges (positions given in NATIVE captured pixels, scaled here so
/// badges stay crisp after downscale), and JPEG-encode. Returns (jpeg, scale)
/// where `scale` = final/native (unused by the caller today but returned for
/// completeness). Marks whose native position is off the captured region are
/// clamped by the drawer.
pub fn capture_region_marked_jpeg(
    rect: Option<(i32, i32, i32, i32)>,
    marks_native: &[super::marks::Mark],
    max_dim: u32,
    quality: u8,
) -> Result<Vec<u8>, CoreError> {
    let (rgba, w, h) = super::verify::capture_region_raw(rect)?;
    let (rw, rh) = (w as u32, h as u32);
    let long_edge = rw.max(rh);
    let scale = if max_dim > 0 && long_edge > max_dim {
        max_dim as f32 / long_edge as f32
    } else {
        1.0
    };
    let mut img = image::RgbaImage::from_raw(rw, rh, rgba)
        .ok_or(CoreError::Win32 { code: 0, context: "SoM RgbaImage from_raw" })?;
    if scale < 1.0 {
        let nw = ((rw as f32 * scale).round() as u32).max(1);
        let nh = ((rh as f32 * scale).round() as u32).max(1);
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    // Scale mark positions into the (possibly downscaled) image space, then draw.
    let scaled: Vec<super::marks::Mark> = marks_native.iter().map(|m| super::marks::Mark {
        label: m.label,
        x: (m.x as f32 * scale).round() as i32,
        y: (m.y as f32 * scale).round() as i32,
    }).collect();
    super::marks::draw_marks(&mut img, &scaled);
    let (fw, fh) = (img.width(), img.height());
    encode_jpeg_rgba(img.as_raw(), fw, fh, quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_within_basic() {
        let bounds = (0, 0, 1920, 1080);
        assert!(rect_within((10, 10, 100, 100), bounds));
        assert!(rect_within((0, 0, 1920, 1080), bounds)); // exact edges
        assert!(!rect_within((-1, 0, 100, 100), bounds)); // left of bounds
        assert!(!rect_within((0, 0, 1921, 100), bounds)); // past right edge
        assert!(!rect_within((1900, 1000, 2100, 1200), bounds)); // second-monitor rect
    }

    #[test]
    fn rect_within_rejects_inverted_rects() {
        let bounds = (0, 0, 1920, 1080);
        assert!(!rect_within((50, 0, -1, 100), bounds));  // negative right (the wrap-bug trigger)
        assert!(!rect_within((50, 100, 60, 20), bounds)); // bottom < top
        assert!(!rect_within((100, 0, 100, 50), bounds)); // zero width
        assert!(!rect_within((0, 50, 50, 50), bounds));   // zero height
    }

    #[test]
    fn rect_within_negative_origin_bounds() {
        // Secondary monitor to the left: virtual bounds start negative.
        let bounds = (-1920, 0, 1920, 1080);
        assert!(rect_within((-1900, 10, -100, 500), bounds)); // on the left monitor
        assert!(!rect_within((-2000, 10, -100, 500), bounds)); // past left edge
    }

    #[test]
    fn virtual_bounds_contains_primary() {
        // Virtual screen must always contain the primary (0,0)-(w,h).
        let (vl, vt, vr, vb) = virtual_screen_bounds();
        assert!(vl <= 0 && vt <= 0, "virtual origin should be <= (0,0): {vl},{vt}");
        assert!(vr > 0 && vb > 0, "virtual extent should be positive: {vr},{vb}");
    }

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
    fn bgra_to_rgba_region_matches_full_then_crop() {
        // Synthetic 8x6 BGRA frame with pitch padding (pitch = 8*4 + 8 = 40).
        let (w, h, pitch) = (8usize, 6usize, 8 * 4 + 8);
        let mut bgra = vec![0u8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                let s = y * pitch + x * 4;
                bgra[s] = (x * 10) as u8;       // B
                bgra[s + 1] = (y * 20) as u8;   // G
                bgra[s + 2] = (x + y) as u8;    // R
                bgra[s + 3] = 0xFF;
            }
        }
        let full = bgra_to_rgba(&bgra, w, h, pitch);
        // For every sub-rect, region-convert must equal full-then-crop.
        for &(l, t, cw, ch) in &[(0usize, 0usize, 8usize, 6usize), (2, 1, 4, 3), (5, 4, 3, 2), (0, 5, 1, 1)] {
            let region = bgra_to_rgba_region(&bgra, pitch, l, t, cw, ch);
            let cropped = crop_rgba(&full, w, l, t, cw, ch);
            assert_eq!(region, cropped, "region convert must equal full-then-crop for rect ({l},{t},{cw},{ch})");
        }
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
        use windows::Win32::Graphics::Dxgi::{
            DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_ACCESS_DENIED,
            DXGI_ERROR_WAIT_TIMEOUT, DXGI_ERROR_INVALID_CALL,
        };
        // 0x887A0026 and 0x887A002B are the canonical DXGI access-lost HRESULTs.
        assert_eq!(DXGI_ERROR_ACCESS_LOST.0 as u32, 0x887A_0026);
        assert_eq!(DXGI_ERROR_ACCESS_DENIED.0 as u32, 0x887A_002B);
        // 0x887A0027 = DXGI_ERROR_WAIT_TIMEOUT (static screen, no new frame).
        assert_eq!(DXGI_ERROR_WAIT_TIMEOUT.0 as u32, 0x887A_0027);
        // 0x887A0001 = DXGI_ERROR_INVALID_CALL (wedged session — frame not released).
        assert_eq!(DXGI_ERROR_INVALID_CALL.0 as u32, 0x887A_0001);
    }

    // --- is_frame_black tests ---

    #[test]
    fn is_frame_black_all_zero_returns_true() {
        let buf = vec![0u8; 4 * 100]; // 25 RGBA pixels, all black
        assert!(is_frame_black(&buf));
    }

    #[test]
    fn is_frame_black_single_nonzero_r_returns_false() {
        let mut buf = vec![0u8; 4 * 100];
        buf[0] = 1; // R channel non-zero
        assert!(!is_frame_black(&buf));
    }

    #[test]
    fn is_frame_black_only_alpha_nonzero_returns_true() {
        // Alpha (index 3) is ignored — only R/G/B matter
        let mut buf = vec![0u8; 4 * 100];
        for i in (3..buf.len()).step_by(4) {
            buf[i] = 255; // all alpha set, but R/G/B = 0
        }
        assert!(is_frame_black(&buf));
    }

    #[test]
    fn is_frame_black_empty_returns_false() {
        assert!(!is_frame_black(&[]));
    }

    #[test]
    fn is_frame_black_single_white_pixel() {
        let buf = vec![255u8, 255, 255, 255]; // 1 white pixel
        assert!(!is_frame_black(&buf));
    }

    /// Verify the sticky flag starts false and can be set.
    /// The actual auto-set path is exercised by the live test below;
    /// this test just validates the AtomicBool wiring is reachable from tests.
    #[test]
    fn dxgi_always_black_flag_starts_false_and_toggles() {
        // Save current state and reset to false for deterministic test.
        let prior = DXGI_ALWAYS_BLACK.swap(false, Ordering::Relaxed);
        assert!(!DXGI_ALWAYS_BLACK.load(Ordering::Relaxed));
        DXGI_ALWAYS_BLACK.store(true, Ordering::Relaxed);
        assert!(DXGI_ALWAYS_BLACK.load(Ordering::Relaxed));
        // Restore original state so other tests are unaffected.
        DXGI_ALWAYS_BLACK.store(prior, Ordering::Relaxed);
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
