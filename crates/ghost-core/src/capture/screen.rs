use std::sync::Mutex;
use crate::error::CoreError;

// Cached D3D11 device, context, and output. Created once on first screenshot,
// reused on all subsequent calls. DuplicateOutput is still acquired per call
// because IDXGIOutputDuplication is tied to a single frame-capture session.
struct CachedCapture {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    output1: windows::Win32::Graphics::Dxgi::IDXGIOutput1,
}

// Safety: D3D11 uses COM's internal reference counting with thread-safe
// AddRef/Release, and we initialized COM with COINIT_MULTITHREADED. All
// D3D11 interfaces are designed for concurrent use from MTA apartments.
unsafe impl Send for CachedCapture {}
unsafe impl Sync for CachedCapture {}

static CAPTURE_STATE: Mutex<Option<CachedCapture>> = Mutex::new(None);

/// Capture the primary monitor as PNG bytes.
pub fn capture_screen() -> Result<Vec<u8>, CoreError> {
    let mut guard = CAPTURE_STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(init_capture_state()?);
    }
    capture_with_state(guard.as_ref().unwrap())
}

fn init_capture_state() -> Result<CachedCapture, CoreError> {
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

        Ok(CachedCapture { device, context, output1 })
    }
}

fn capture_with_state(s: &CachedCapture) -> Result<Vec<u8>, CoreError> {
    unsafe {
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::*;
        use windows::core::Interface;

        let duplication = s.output1.DuplicateOutput(&s.device)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "DuplicateOutput" })?;

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        duplication.AcquireNextFrame(500, &mut frame_info, &mut resource)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "AcquireNextFrame" })?;

        let resource = resource.ok_or(CoreError::Win32 { code: 0, context: "frame resource null" })?;
        let texture: ID3D11Texture2D = resource.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "texture cast" })?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let width = desc.Width as usize;
        let height = desc.Height as usize;

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        s.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateTexture2D staging" })?;
        let staging = staging.ok_or(CoreError::Win32 { code: 0, context: "staging texture null" })?;

        let resource_view: ID3D11Resource = texture.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "texture resource cast" })?;
        let staging_view: ID3D11Resource = staging.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "staging resource cast" })?;
        s.context.CopyResource(&staging_view, &resource_view);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        s.context.Map(&staging_view, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Map" })?;

        let pitch = mapped.RowPitch as usize;
        let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * height);
        let rgba = bgra_to_rgba(data, width, height, pitch);

        s.context.Unmap(&staging_view, 0);
        duplication.ReleaseFrame()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "ReleaseFrame" })?;

        encode_png_rgba(&rgba, width as u32, height as u32)
    }
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
}
