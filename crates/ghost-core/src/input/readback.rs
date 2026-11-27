//! Judging a typing read-back.

/// Did typing `want` into a field that held `before` produce `after`?
///
/// `replace`: the dispatch sets the whole value (WM_SETTEXT, UIA SetValue), so
/// the field must now hold exactly `want`. Otherwise the characters were
/// inserted at the caret (posted keys), so `after` must be `before` with
/// `want` inserted at some position, or `want` alone (it replaced a selection).
/// `None` when there is nothing to judge (`want` is empty). Newlines are folded
/// because some providers store `\r\n` for a typed `\n`; one trailing newline a
/// provider adds is tolerated when `want` itself ends in no whitespace.
///
/// A `contains` check is not enough: a field that already held the text, or a
/// whitespace-only `want`, passed it with nothing typed.
pub fn typed_ok(before: &str, after: &str, want: &str, replace: bool) -> Option<bool> {
    if want.is_empty() {
        return None;
    }
    let fold = |s: &str| s.replace("\r\n", "\n");
    let (b, a, w) = (fold(before), fold(after), fold(want));
    let tail_ok = w.trim_end() == w;
    if replace {
        return Some(a == w || (tail_ok && a.trim_end() == w));
    }
    let inserted = |b: &str, a: &str| {
        a.len() == b.len() + w.len()
            && (0..=b.len()).filter(|&i| b.is_char_boundary(i)).any(|i| {
                a.starts_with(&b[..i]) && a[i..].starts_with(w.as_str()) && a[i + w.len()..] == b[i..]
            })
    };
    let over_selection = a != b && a == w;
    Some(inserted(&b, &a) || over_selection || (tail_ok && inserted(b.trim_end(), a.trim_end())))
}

#[cfg(test)]
mod tests {
    use super::typed_ok;

    #[test]
    fn pre_existing_text_is_not_a_typed_append() {
        // "15" already held "5": the old contains() check said verified.
        assert_eq!(typed_ok("15", "15", "5", false), Some(false));
        assert_eq!(typed_ok("15", "15", "5", true), Some(false));
    }

    #[test]
    fn empty_or_unchanged_whitespace_is_never_verified() {
        assert_eq!(typed_ok("", "", "", false), None);
        assert_eq!(typed_ok("abc", "abc", "", true), None);
        assert_ne!(typed_ok("", "", " ", false), Some(true));
        assert_ne!(typed_ok("", "", " ", true), Some(true));
        assert_ne!(typed_ok("", "", "\n", false), Some(true));
    }

    #[test]
    fn a_real_type_is_verified() {
        assert_eq!(typed_ok("", "5", "5", false), Some(true));
        assert_eq!(typed_ok("", "5", "5", true), Some(true));
        assert_eq!(typed_ok("", " ", " ", false), Some(true));
    }

    #[test]
    fn replace_needs_the_field_to_hold_exactly_the_text() {
        assert_eq!(typed_ok("old", "new", "new", true), Some(true));
        assert_eq!(typed_ok("old", "oldnew", "new", true), Some(false));
        // A provider that stores \r\n for a typed \n, or adds a trailing newline.
        assert_eq!(typed_ok("", "a\r\nb", "a\nb", true), Some(true));
        assert_eq!(typed_ok("", "hello\r\n", "hello", true), Some(true));
    }

    #[test]
    fn append_needs_the_text_inserted_at_some_caret() {
        assert_eq!(typed_ok("ab", "abX", "X", false), Some(true));
        assert_eq!(typed_ok("ab", "Xab", "X", false), Some(true));
        assert_eq!(typed_ok("ab", "aXb", "X", false), Some(true));
        assert_eq!(typed_ok("ab", "ab", "X", false), Some(false));
        assert_eq!(typed_ok("ab", "aX", "X", false), Some(false));
        // Posted keys over a selected field replace it.
        assert_eq!(typed_ok("old", "new", "new", false), Some(true));
        // Partial landing.
        assert_eq!(typed_ok("", "he", "hello", false), Some(false));
        // Multi-byte text and \r\n folding.
        assert_eq!(typed_ok("é", "éñ", "ñ", false), Some(true));
        assert_eq!(typed_ok("x", "xa\r\nb", "a\nb", false), Some(true));
    }
}
