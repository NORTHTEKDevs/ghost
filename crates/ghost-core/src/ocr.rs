//! Local OCR via Windows.Media.Ocr (WinRT). Free, on-device, no API calls.
//!
//! Use cases:
//!  - Wait for a specific text label to appear without round-tripping vision
//!  - Read text from canvas-rendered apps where UIA names are missing
//!  - Validate post-action UI text changes
//!
//! Requires Windows 10+ (the OCR engine is part of the platform).

use crate::error::CoreError;
use crate::capture::screen::{capture_screen_full_rgba, rgba_to_bgra_in_place};
use crate::uia::element::BoundingRect;
use windows::Foundation::IAsyncOperation;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::System::WinRT::IBufferByteAccess;
use windows::core::Interface;

/// One OCR-detected word with its on-screen bounding rect.
#[derive(Debug, Clone)]
pub struct OcrWord {
    pub text: String,
    pub rect: BoundingRect,
}

/// Capture the screen (or a region of it), run OCR, and return the words in
/// absolute screen coordinates.
///
/// `region` = (left, top, right, bottom) in pixels; `None` = full screen.
pub fn capture_and_ocr(region: Option<(i32, i32, i32, i32)>) -> Result<Vec<OcrWord>, CoreError> {
    let (rgba, full_w, full_h) = capture_screen_full_rgba()?;
    let (origin, w, h, mut bgra) = match region {
        Some((l, t, r, b)) => {
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
                let src = ((t + y) * full_w + l) * 4;
                let dst = y * cw * 4;
                crop[dst..dst + cw * 4].copy_from_slice(&rgba[src..src + cw * 4]);
            }
            ((l as i32, t as i32), cw, ch, crop)
        }
        None => ((0, 0), full_w, full_h, rgba),
    };

    rgba_to_bgra_in_place(&mut bgra);
    let bitmap = create_bitmap_from_bgra(&bgra, w as u32, h as u32)?;
    let words = run_ocr(&bitmap)?;

    Ok(words.into_iter().map(|w| OcrWord {
        text: w.text,
        rect: BoundingRect {
            left: w.rect.left + origin.0,
            top: w.rect.top + origin.1,
            right: w.rect.right + origin.0,
            bottom: w.rect.bottom + origin.1,
        },
    }).collect())
}

/// Find the first OCR word whose text contains `needle` (case-insensitive).
/// Returns the screen-pixel center of the matched word, or None if not found.
pub fn find_text_local(
    needle: &str,
    region: Option<(i32, i32, i32, i32)>,
) -> Result<Option<(i32, i32)>, CoreError> {
    let needle_lower = needle.to_lowercase();
    let words = capture_and_ocr(region)?;
    for w in words {
        if w.text.to_lowercase().contains(&needle_lower) {
            return Ok(Some(w.rect.center()));
        }
    }
    Ok(None)
}

/// Wrap raw BGRA bytes as a SoftwareBitmap for OCR.
/// Uses IBufferByteAccess to memcpy our buffer into a WinRT IBuffer.
fn create_bitmap_from_bgra(bgra: &[u8], width: u32, height: u32) -> Result<SoftwareBitmap, CoreError> {
    let buf = Buffer::Create(bgra.len() as u32)
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Buffer::Create" })?;
    buf.SetLength(bgra.len() as u32)
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Buffer::SetLength" })?;
    let access: IBufferByteAccess = buf.cast()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Buffer cast IBufferByteAccess" })?;
    unsafe {
        let ptr = access.Buffer()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IBufferByteAccess::Buffer" })?;
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), ptr, bgra.len());
    }

    SoftwareBitmap::CreateCopyFromBuffer(
        &buf,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateCopyFromBuffer" })
}

fn run_ocr(bitmap: &SoftwareBitmap) -> Result<Vec<OcrWord>, CoreError> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            let lang = Language::CreateLanguage(&windows::core::HSTRING::from("en-US"))?;
            OcrEngine::TryCreateFromLanguage(&lang)
        })
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OcrEngine::TryCreate" })?;

    let result = block_on_async(
        engine.RecognizeAsync(bitmap)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "RecognizeAsync" })?
    )?;

    let lines = result.Lines()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OcrResult::Lines" })?;
    let count = lines.Size()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Lines::Size" })?;

    let mut out = Vec::new();
    for i in 0..count {
        let line = lines.GetAt(i)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Lines::GetAt" })?;
        let words = line.Words()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Line::Words" })?;
        let wcount = words.Size()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Words::Size" })?;
        for j in 0..wcount {
            let word = words.GetAt(j)
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Words::GetAt" })?;
            let text = word.Text()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Word::Text" })?
                .to_string();
            let rect = word.BoundingRect()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Word::BoundingRect" })?;
            out.push(OcrWord {
                text,
                rect: BoundingRect {
                    left: rect.X.round() as i32,
                    top: rect.Y.round() as i32,
                    right: (rect.X + rect.Width).round() as i32,
                    bottom: (rect.Y + rect.Height).round() as i32,
                },
            });
        }
    }
    Ok(out)
}

fn block_on_async<T>(op: IAsyncOperation<T>) -> Result<T, CoreError>
where
    T: windows::core::RuntimeType + 'static,
{
    op.get()
        .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IAsyncOperation::get" })
}
