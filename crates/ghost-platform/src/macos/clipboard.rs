//! Clipboard access, over `NSPasteboard`.
//!
//! | Ghost operation | Apple API |
//! | --- | --- |
//! | read text | `[[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString]` |
//! | write text | `clearContents` then `setString:forType:` |
//!
//! This is the one part of the macOS backend that needs Objective-C messaging;
//! there is no C-level pasteboard API. It needs no TCC grant — the general
//! pasteboard is readable by any process in the user's session.
//!
//! Writing requires `clearContents` first. Skipping it leaves the previous
//! owner's other representations (RTF, HTML) in place, so a paste can deliver
//! stale content that does not match the string just written.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use super::error::{MacError, MacResult};

/// Read the clipboard as text — `stringForType:NSPasteboardTypeString`.
///
/// `Ok(None)` means the pasteboard holds no string representation (an image, say),
/// which is different from holding an empty string.
pub fn get_text() -> MacResult<Option<String>> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let value = unsafe { pasteboard.stringForType(NSPasteboardTypeString) };
    Ok(value.map(|s| s.to_string()))
}

/// Replace the clipboard contents with `text` — `clearContents` then
/// `setString:forType:`.
pub fn set_text(text: &str) -> MacResult<()> {
    let pasteboard = NSPasteboard::generalPasteboard();
    // Must clear first, or stale non-string representations survive.
    pasteboard.clearContents();

    let value = NSString::from_str(text);
    let ok = unsafe { pasteboard.setString_forType(&value, NSPasteboardTypeString) };
    if ok {
        Ok(())
    } else {
        Err(MacError::Clipboard(
            "NSPasteboard rejected the string (another process may hold the pasteboard)",
        ))
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn clipboard_round_trips_text_including_non_ascii() {
        // The general pasteboard exists on a headless runner, so this is a real
        // end-to-end check of the one Objective-C bridge in the backend.
        let original = get_text().expect("read clipboard");

        for probe in ["hello ghost", "", "café — naïve 日本語 😀", "line1\nline2\ttabbed"] {
            set_text(probe).expect("write clipboard");
            assert_eq!(
                get_text().expect("read back").as_deref(),
                Some(probe),
                "clipboard did not round trip {probe:?}"
            );
        }

        // Leave the runner's clipboard as we found it.
        if let Some(original) = original {
            let _ = set_text(&original);
        }
    }
}
