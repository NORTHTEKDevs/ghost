//! CoreFoundation glue: converting between Rust values and the `CFTypeRef`s the
//! Accessibility API traffics in.
//!
//! Two rules hold everywhere in this module, because getting either wrong is a
//! use-after-free:
//! - Anything from a `Copy`/`Create` function is owned and must be released. We
//!   wrap it in a `core_foundation` smart type immediately via `wrap_under_create_rule`.
//! - Anything from a `Get` function is borrowed and must NOT be released
//!   (`wrap_under_get_rule`).

use core::ffi::c_void;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFRelease, CFTypeRef};

/// Build a `CFString` from a Rust `&str`. Every `kAX*` constant in
/// `accessibility-sys` is a plain `&str`, so this is the bridge used by every AX
/// attribute and action call in the backend.
pub fn cfstr(s: &str) -> CFString {
    CFString::new(s)
}

/// Take ownership of a `CFTypeRef` returned by a Create/Copy-rule function.
///
/// Returns `None` for null, which is how the AX API reports "no value" alongside
/// a success status for some attributes.
///
/// # Safety
/// `raw` must be null, or a valid `CFTypeRef` that the caller owns a reference to
/// (i.e. it came from a function whose name contains `Create` or `Copy`).
pub unsafe fn owned(raw: CFTypeRef) -> Option<CFType> {
    if raw.is_null() {
        None
    } else {
        Some(CFType::wrap_under_create_rule(raw))
    }
}

/// Release a `CFTypeRef` we own but have no wrapper for.
///
/// # Safety
/// `raw` must be null, or a pointer the caller owns exactly one reference to.
/// After this call the pointer must not be used again.
pub unsafe fn release(raw: CFTypeRef) {
    if !raw.is_null() {
        CFRelease(raw);
    }
}

/// Read a `CFType` as a Rust `String`, if it really is a `CFString`.
pub fn as_string(value: &CFType) -> Option<String> {
    value.downcast::<CFString>().map(|s| s.to_string())
}

/// Read a `CFType` as a `bool`, accepting both `CFBoolean` and the `CFNumber` 0/1
/// some accessibility providers return for `kAXEnabledAttribute` instead.
pub fn as_bool(value: &CFType) -> Option<bool> {
    if let Some(b) = value.downcast::<CFBoolean>() {
        return Some(b.into());
    }
    value
        .downcast::<CFNumber>()
        .and_then(|n| n.to_i64())
        .map(|n| n != 0)
}

/// Read a `CFType` as an `f64`, accepting either integer or floating `CFNumber`s.
pub fn as_f64(value: &CFType) -> Option<f64> {
    let n = value.downcast::<CFNumber>()?;
    n.to_f64().or_else(|| n.to_i64().map(|i| i as f64))
}

/// Read a `CFType` as an `i64`.
pub fn as_i64(value: &CFType) -> Option<i64> {
    value.downcast::<CFNumber>().and_then(|n| n.to_i64())
}

/// Read a `CFType` as a vector of the CF objects it contains.
///
/// `core_foundation` only implements the runtime type check for the untyped
/// `CFArray<*const c_void>`, so the elements come back as raw pointers and each one
/// is retained here under the get rule — the array owns its elements, and returning
/// borrowed pointers that outlive it would be a use-after-free. The retain is
/// balanced by `CFType`'s `Drop`.
///
/// Elements that are not CF objects would be a contract violation by the API that
/// produced the array; the null check is what keeps that from being a segfault.
pub fn as_array(value: &CFType) -> Option<Vec<CFType>> {
    let array = value.downcast::<CFArray<*const c_void>>()?;
    let mut out = Vec::with_capacity(array.len() as usize);
    for item in array.iter() {
        let raw = *item as CFTypeRef;
        if raw.is_null() {
            continue;
        }
        // SAFETY: the array holds a reference to each element, so `raw` is live for
        // at least as long as `array`; retaining extends that past this scope.
        out.push(unsafe { CFType::wrap_under_get_rule(raw) });
    }
    Some(out)
}

/// Read a `CFType` as a string-keyed dictionary.
///
/// `kCGWindowBounds` is a nested dictionary rather than a `CGRect`, so this is how
/// window geometry is unpacked. As with [`as_array`], `core_foundation` only
/// implements the runtime type check for the fully untyped dictionary, so the check
/// is done there and the re-wrap into a keyed type is done here — the type
/// parameters are a compile-time convenience for the caller, not a claim the runtime
/// verified.
pub fn as_dictionary(value: &CFType) -> Option<CFDictionary<CFString, CFType>> {
    if !value.instance_of::<CFDictionary<*const c_void, *const c_void>>() {
        return None;
    }
    let raw = value.as_CFTypeRef() as CFDictionaryRef;
    // SAFETY: the type ID was just checked, and the get rule retains, so the
    // wrapper does not outlive its referent.
    Some(unsafe { CFDictionary::<CFString, CFType>::wrap_under_get_rule(raw) })
}

/// A best-effort human-readable string for any `CFType`, used only in error text.
pub fn describe(value: &CFType) -> String {
    as_string(value)
        .or_else(|| as_i64(value).map(|n| n.to_string()))
        .or_else(|| as_bool(value).map(|b| b.to_string()))
        .unwrap_or_else(|| "<non-textual CFType>".to_string())
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;
    use core_foundation::base::TCFType;

    #[test]
    fn cfstring_round_trips_including_non_ascii() {
        for original in ["AXRole", "", "hello ghost", "café — naïve 日本語", "a\tb\nc"] {
            let cf = cfstr(original);
            assert_eq!(cf.to_string(), original, "round trip failed for {original:?}");
        }
    }

    #[test]
    fn as_string_reads_a_cfstring_and_rejects_a_number() {
        let s = cfstr("AXTextArea").as_CFType();
        assert_eq!(as_string(&s).as_deref(), Some("AXTextArea"));

        let n = CFNumber::from(7i64).as_CFType();
        assert_eq!(as_string(&n), None);
    }

    #[test]
    fn as_bool_accepts_cfboolean_and_numeric_zero_one() {
        assert_eq!(as_bool(&CFBoolean::true_value().as_CFType()), Some(true));
        assert_eq!(as_bool(&CFBoolean::false_value().as_CFType()), Some(false));
        // Some AX providers hand back a CFNumber for kAXEnabledAttribute.
        assert_eq!(as_bool(&CFNumber::from(1i64).as_CFType()), Some(true));
        assert_eq!(as_bool(&CFNumber::from(0i64).as_CFType()), Some(false));
        assert_eq!(as_bool(&cfstr("true").as_CFType()), None);
    }

    #[test]
    fn as_f64_accepts_both_integer_and_floating_cfnumbers() {
        assert_eq!(as_f64(&CFNumber::from(42i64).as_CFType()), Some(42.0));
        assert_eq!(as_f64(&CFNumber::from(1.5f64).as_CFType()), Some(1.5));
        assert_eq!(as_f64(&cfstr("1.5").as_CFType()), None);
    }

    #[test]
    fn owned_maps_null_to_none() {
        // The AX API returns a success status with a null value for some
        // attributes; that must read as absence, not as a crash.
        assert!(unsafe { owned(std::ptr::null()) }.is_none());
    }

    #[test]
    fn release_of_null_is_a_no_op() {
        unsafe { release(std::ptr::null()) };
    }

    #[test]
    fn describe_never_panics_on_a_non_textual_type() {
        let arr = CFArray::from_CFTypes(&[cfstr("AXPress")]).as_CFType();
        assert_eq!(describe(&arr), "<non-textual CFType>");
    }

    #[test]
    fn as_array_reads_cf_objects_out_of_an_array() {
        // This is the shape kAXChildrenAttribute and AXUIElementCopyActionNames
        // both come back in.
        let arr = CFArray::from_CFTypes(&[cfstr("AXPress"), cfstr("AXShowMenu")]).as_CFType();
        let items = as_array(&arr).expect("should read as an array");
        let names: Vec<String> = items.iter().filter_map(as_string).collect();
        assert_eq!(names, vec!["AXPress", "AXShowMenu"]);
    }

    #[test]
    fn as_array_rejects_a_non_array() {
        assert!(as_array(&cfstr("AXPress").as_CFType()).is_none());
        assert!(as_array(&CFNumber::from(1i64).as_CFType()).is_none());
    }

    #[test]
    fn as_array_of_an_empty_array_is_empty_not_none() {
        // "no children" must be distinguishable from "not an array".
        let empty = CFArray::<CFString>::from_CFTypes(&[]).as_CFType();
        assert_eq!(as_array(&empty).map(|v| v.len()), Some(0));
    }
}
