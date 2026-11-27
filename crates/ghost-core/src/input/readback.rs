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

/// What one typing dispatch did to the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// The field holds exactly what `typed_ok` expects.
    Landed,
    /// The field changed and holds the typed text, compared case- and
    /// whitespace-insensitively, but not verbatim: autocomplete ("goo" ->
    /// "google.com"), currency formatting ("12" -> "$12.00"), trimming,
    /// upper-casing. The text landed and the app reformatted it.
    Reformatted,
    /// The field changed but does not hold the typed text (a partial landing,
    /// an input mask that rejected it). Something was typed; it is not verified.
    Changed,
    /// The field reads exactly as before: nothing landed.
    Unchanged,
}

impl Typed {
    /// Whether the dispatch counts as verified. `Reformatted` does: the text
    /// is there, and the caller says the field reformatted it.
    pub fn verified(self) -> bool {
        matches!(self, Typed::Landed | Typed::Reformatted)
    }

    /// Whether another typing method may run after this one. Only when the
    /// field is unchanged: dispatching again into a field that visibly changed
    /// types the text twice ("$1212").
    pub fn may_try_next(self) -> bool {
        self == Typed::Unchanged
    }
}

/// Classify a typing read-back (see `typed_ok` for `replace` and the folding).
/// `None` when `want` is empty. `Reformatted` needs the typed text present in
/// `after` case- and whitespace-insensitively; for an insert (`!replace`) it
/// must also occur more often than in `before`, so text already there does not
/// count. A whitespace-only `want` is never `Reformatted`.
pub fn judge_typed(before: &str, after: &str, want: &str, replace: bool) -> Option<Typed> {
    if typed_ok(before, after, want, replace)? {
        return Some(Typed::Landed);
    }
    let fold = |s: &str| s.replace("\r\n", "\n");
    if fold(after) == fold(before) {
        return Some(Typed::Unchanged);
    }
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    let (b, a, w) = (norm(before), norm(after), norm(want));
    // Replace mode: exactly one copy, or a doubled dispatch reads as verified.
    let n = a.matches(&w).count();
    let holds = !w.is_empty() && if replace { n == 1 } else { n > b.matches(&w).count() };
    Some(if holds { Typed::Reformatted } else { Typed::Changed })
}

/// One typing method in a `type_ladder`: given the field's value before it,
/// dispatch and return the settled read-back and whether it replaces the whole
/// value, or `None` when it could not dispatch.
pub type Rung<'a, E> = dyn FnMut(&str) -> Result<Option<(String, bool)>, E> + 'a;

/// Where a typing ladder stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderEnd {
    /// Index of the last rung that dispatched; `None` when none did.
    pub rung: Option<usize>,
    /// Its verdict; `None` when `want` is empty (nothing was dispatched).
    pub outcome: Option<Typed>,
    /// The field as that rung last read it.
    pub got: String,
}

/// Try typing methods in order. Each rung gets the field's value before it and
/// returns `None` when it could not dispatch, or the settled read-back and
/// whether it replaces the whole value. The ladder stops at the first rung that
/// changed the field (`Typed::may_try_next`): a second method after a visible
/// change would type the text twice. Ends `Unchanged` when no rung changed it.
pub fn type_ladder<E>(
    want: &str,
    mut read: impl FnMut() -> String,
    rungs: &mut [&mut Rung<'_, E>],
) -> Result<LadderEnd, E> {
    let mut end = LadderEnd { rung: None, outcome: None, got: String::new() };
    if want.is_empty() {
        return Ok(end);
    }
    end.outcome = Some(Typed::Unchanged);
    for (i, rung) in rungs.iter_mut().enumerate() {
        let before = read();
        end.got = before.clone();
        let Some((after, replace)) = rung(&before)? else { continue };
        let outcome = judge_typed(&before, &after, want, replace);
        end = LadderEnd { rung: Some(i), outcome, got: after };
        if !outcome.is_some_and(Typed::may_try_next) {
            break;
        }
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::{judge_typed, type_ladder, typed_ok, Typed};
    use std::cell::{Cell, RefCell};

    #[test]
    fn a_reformatting_field_reads_as_reformatted_not_failed() {
        // Autocomplete, currency formatting, trimming, upper-casing.
        assert_eq!(judge_typed("", "google.com", "goo", false), Some(Typed::Reformatted));
        assert_eq!(judge_typed("", "$12.00", "12", true), Some(Typed::Reformatted));
        assert_eq!(judge_typed("", "abc", "abc ", true), Some(Typed::Reformatted));
        assert_eq!(judge_typed("", "HELLO WORLD", "hello world", false), Some(Typed::Reformatted));
        assert_eq!(judge_typed("old", "Hello", "hello", true), Some(Typed::Reformatted));
    }

    #[test]
    fn a_doubled_replace_is_changed_not_reformatted() {
        // Two methods both landed: the field holds the text twice, which is
        // not the typed text in another format.
        assert_eq!(judge_typed("", "hellohello", "hello", true), Some(Typed::Changed));
        assert_eq!(judge_typed("old", "Hello hello", "hello", true), Some(Typed::Changed));
    }

    #[test]
    fn the_verdicts_are_distinct() {
        assert_eq!(judge_typed("", "", "", false), None);
        assert_eq!(judge_typed("", "x", "x", false), Some(Typed::Landed));
        assert_eq!(judge_typed("ab", "ab", "X", false), Some(Typed::Unchanged));
        // \r\n folding alone is not a change.
        assert_eq!(judge_typed("a\r\nb", "a\nb", "zz", true), Some(Typed::Unchanged));
        // Changed, but the text is not in it: a partial landing, or a mask that ate it.
        assert_eq!(judge_typed("", "he", "hello", false), Some(Typed::Changed));
        assert_eq!(judge_typed("old", "", "new", true), Some(Typed::Changed));
        // Append: text that was already there does not count as typed.
        assert_eq!(judge_typed("abc", "abcd", "b", false), Some(Typed::Changed));
        // Only an unchanged field lets another method run.
        assert!(Typed::Unchanged.may_try_next());
        for t in [Typed::Landed, Typed::Reformatted, Typed::Changed] {
            assert!(!t.may_try_next(), "{t:?}");
        }
        assert!(Typed::Landed.verified() && Typed::Reformatted.verified());
        assert!(!Typed::Changed.verified() && !Typed::Unchanged.verified());
    }

    /// A stand-in edit control: `settext` and `keys` both dispatch, and the
    /// control applies `format` to whatever it is given.
    struct FakeEdit {
        value: RefCell<String>,
        dispatches: Cell<usize>,
        format: fn(&str, &str) -> String,
    }

    impl FakeEdit {
        fn new(format: fn(&str, &str) -> String) -> Self {
            Self { value: RefCell::new(String::new()), dispatches: Cell::new(0), format }
        }
        fn run(&self, want: &str) -> (Option<usize>, Option<Typed>) {
            let read = || self.value.borrow().clone();
            let mut settext = |_before: &str| -> Result<Option<(String, bool)>, ()> {
                self.dispatches.set(self.dispatches.get() + 1);
                let v = (self.format)("", want);
                *self.value.borrow_mut() = v;
                Ok(Some((read(), true)))
            };
            let mut keys = |before: &str| -> Result<Option<(String, bool)>, ()> {
                self.dispatches.set(self.dispatches.get() + 1);
                let v = (self.format)(before, want);
                *self.value.borrow_mut() = v;
                Ok(Some((read(), false)))
            };
            let end = type_ladder(want, read, &mut [&mut settext, &mut keys]).unwrap();
            (end.rung, end.outcome)
        }
    }

    #[test]
    fn a_reformatting_control_gets_exactly_one_dispatch() {
        let upper = FakeEdit::new(|b, w| format!("{b}{w}").to_uppercase());
        assert_eq!(upper.run("hello"), (Some(0), Some(Typed::Reformatted)));
        assert_eq!(upper.dispatches.get(), 1);
        assert_eq!(*upper.value.borrow(), "HELLO");

        let money = FakeEdit::new(|b, w| format!("${b}{w}.00"));
        assert_eq!(money.run("12"), (Some(0), Some(Typed::Reformatted)));
        assert_eq!(money.dispatches.get(), 1, "a second dispatch types $1212");
        assert_eq!(*money.value.borrow(), "$12.00");

        // A control that mangles the text is not retyped either.
        let mask = FakeEdit::new(|_, _| "***".into());
        assert_eq!(mask.run("12"), (Some(0), Some(Typed::Changed)));
        assert_eq!(mask.dispatches.get(), 1);
    }

    #[test]
    fn only_an_unchanged_field_falls_through() {
        let deaf = FakeEdit::new(|b, _| b.to_string());
        assert_eq!(deaf.run("hi"), (Some(1), Some(Typed::Unchanged)));
        assert_eq!(deaf.dispatches.get(), 2);

        let exact = FakeEdit::new(|b, w| format!("{b}{w}"));
        assert_eq!(exact.run("hi"), (Some(0), Some(Typed::Landed)));
        assert_eq!(exact.dispatches.get(), 1);

        // A rung that could not dispatch (None) is skipped, not judged.
        let mut skipped = |_: &str| -> Result<Option<(String, bool)>, ()> { Ok(None) };
        let mut none = |_: &str| -> Result<Option<(String, bool)>, ()> { Ok(None) };
        let end = type_ladder("x", String::new, &mut [&mut skipped, &mut none]).unwrap();
        assert_eq!((end.rung, end.outcome), (None, Some(Typed::Unchanged)));
        // Nothing to type: no rung runs.
        let mut boom = |_: &str| -> Result<Option<(String, bool)>, ()> { panic!("dispatched empty text") };
        assert_eq!(type_ladder("", String::new, &mut [&mut boom]).unwrap().outcome, None);
    }

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
