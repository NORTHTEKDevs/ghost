//! Which element a VALUE read (wait for=value, assert value-*, window_value,
//! hidden_value) reads, for a name lookup.
//!
//! A name usually belongs to a field's label as well as to the field: a Win32
//! dialog has a Static "Field" next to its EDIT, and on a non-displayed desktop
//! the EDIT's proxy name becomes the text typed into it, so after typing only
//! the label still matches "Field". Reading the first name match then reads the
//! label ("Field"), not the field ("hello"). The rule here: an editable match
//! wins; failing that, a text label whose name IS the query (not merely
//! contains it) stands for the editable control that follows it, which is how
//! Win32 itself labels an EDIT. A non-editable `role` (e.g. `text`) reads the
//! label itself.

use crate::element::is_editable_role_name;
use crate::error::{GhostError, Result};
use crate::locator::By;

/// What [`pick_value_match`] chose among a name lookup's matches.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ValuePick {
    /// Read match `i` itself.
    At(usize),
    /// Match `label` is a label: read the editable control that follows it,
    /// else match `fallback` (if any).
    FromLabel { label: usize, fallback: Option<usize> },
    Nothing,
}

fn role_ok(role: &str, filter: Option<&str>, alias: &dyn Fn(&str, &str) -> bool) -> bool {
    filter.is_none_or(|f| role == f || alias(f, role))
}

/// A label's name stands for its field only when it IS the query, ignoring
/// case, surrounding space, a trailing colon and `&` accelerator marks.
fn is_label_for(label: &str, query: &str) -> bool {
    let norm = |s: &str| s.replace('&', "").trim().trim_end_matches(':').trim().to_lowercase();
    !query.trim().is_empty() && norm(label) == norm(query)
}

/// Choose among a name lookup's matches, each `(name, role)` in tree order,
/// found WITHOUT the role filter (the filter is applied here, so a field whose
/// own name no longer matches can still be reached through its label).
pub(crate) fn pick_value_match(
    matches: &[(String, &str)],
    query: &str,
    role_filter: Option<&str>,
    alias: &dyn Fn(&str, &str) -> bool,
) -> ValuePick {
    if let Some(i) = matches
        .iter()
        .position(|(_, r)| is_editable_role_name(r) && role_ok(r, role_filter, alias))
    {
        return ValuePick::At(i);
    }
    let first_ok = matches.iter().position(|(_, r)| role_ok(r, role_filter, alias));
    let may_hop = role_filter.is_none_or(is_editable_role_name);
    if may_hop {
        if let Some(label) = matches
            .iter()
            .position(|(n, r)| *r == "text" && is_label_for(n, query))
        {
            return ValuePick::FromLabel { label, fallback: first_ok };
        }
    }
    first_ok.map_or(ValuePick::Nothing, ValuePick::At)
}

/// How far past a label to look for its field.
const LABEL_REACH: usize = 8;

/// The editable control a label stands for: the first editable element (that
/// passes the role filter) among the elements following the label in tree
/// order, `roles_after[0]` being the one right after it. Another text label
/// first means the field belongs to that one, so the search stops.
pub(crate) fn following_editable(
    roles_after: &[&str],
    role_filter: Option<&str>,
    alias: &dyn Fn(&str, &str) -> bool,
) -> Option<usize> {
    for (k, r) in roles_after.iter().take(LABEL_REACH).enumerate() {
        if is_editable_role_name(r) && role_ok(r, role_filter, alias) {
            return Some(k);
        }
        if *r == "text" {
            break;
        }
    }
    None
}

fn role_of(el: &crate::engine::uia::element::UiaElement) -> &'static str {
    crate::element::effective_role_name(el)
}

/// The element a value read of `by` in window `hwnd` reads; see the module docs.
pub(crate) fn find_value_element(
    tree: &crate::engine::uia::tree::UiaTree,
    hwnd: isize,
    by: &By,
    role_filter: Option<&str>,
) -> Result<Option<crate::engine::uia::element::UiaElement>> {
    let alias = |s: &str, r: &str| crate::engine::uia::tree::role_alias_matches(s, r);
    let n = match by {
        By::Role(r) => return tree.find_by_role_in_hwnd(hwnd, r).map_err(GhostError::Core),
        By::Description(d) => {
            return Err(GhostError::Vision(format!(
                "value reads need an element name or role, not a description; desc={d}"
            )))
        }
        By::Name(n) => n,
    };
    // A role narrows the walk itself, so a field behind many same-named
    // links or cells is still reached; the label hop below is for a name
    // that only a label carries (the field's own name is its contents).
    if let Some(r) = role_filter {
        let hits = tree.find_all_in_hwnd(hwnd, Some(n), Some(r), 1).map_err(GhostError::Core)?;
        if let Some(first) = hits.into_iter().next() {
            return Ok(Some(first));
        }
    }
    let matches = tree.find_all_in_hwnd(hwnd, Some(n), None, 16).map_err(GhostError::Core)?;
    let described: Vec<(String, &str)> = matches.iter().map(|e| (e.name(), role_of(e))).collect();
    let (label, fallback) = match pick_value_match(&described, n, role_filter, &alias) {
        ValuePick::At(i) => return Ok(matches.into_iter().nth(i)),
        ValuePick::Nothing => return Ok(None),
        ValuePick::FromLabel { label, fallback } => (label, fallback),
    };
    if let Some(field) = field_after_label(tree, hwnd, n, label, role_filter, &alias)? {
        return Ok(Some(field));
    }
    Ok(fallback.and_then(|i| matches.into_iter().nth(i)))
}

/// The editable control after the `label`-th name match of `query`. The
/// whole-window walk visits the same tree in the same order as the name
/// lookup, so the `label`-th name match there is the same element.
#[cfg(not(target_os = "macos"))]
fn field_after_label(
    tree: &crate::engine::uia::tree::UiaTree,
    hwnd: isize,
    query: &str,
    label: usize,
    role_filter: Option<&str>,
    alias: &dyn Fn(&str, &str) -> bool,
) -> Result<Option<crate::engine::uia::element::UiaElement>> {
    let all = tree.collect_in_hwnd(hwnd, 3000).map_err(GhostError::Core)?;
    let q = query.to_lowercase();
    let Some(pos) = all
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name().to_lowercase().contains(&q))
        .nth(label)
        .map(|(i, _)| i)
    else {
        return Ok(None);
    };
    let after: Vec<&str> = all[pos + 1..].iter().take(LABEL_REACH).map(role_of).collect();
    Ok(following_editable(&after, role_filter, alias).and_then(|k| all.into_iter().nth(pos + 1 + k)))
}

/// macOS has no whole-window walk: the label is read itself.
#[cfg(target_os = "macos")]
fn field_after_label(
    _tree: &crate::engine::uia::tree::UiaTree,
    _hwnd: isize,
    _query: &str,
    _label: usize,
    _role_filter: Option<&str>,
    _alias: &dyn Fn(&str, &str) -> bool,
) -> Result<Option<crate::engine::uia::element::UiaElement>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(s: &str, r: &str) -> bool {
        s == "edit" && r == "document"
    }

    fn m(pairs: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        pairs.iter().map(|(n, r)| (n.to_string(), *r)).collect()
    }

    #[test]
    fn an_editable_match_wins_over_its_label() {
        // User desktop: the EDIT's proxy name is its label's text.
        let ms = m(&[("Field", "text"), ("Field", "edit")]);
        assert_eq!(pick_value_match(&ms, "Field", None, &alias), ValuePick::At(1));
        assert_eq!(pick_value_match(&ms, "Field", Some("edit"), &alias), ValuePick::At(1));
    }

    #[test]
    fn a_label_alone_stands_for_the_field_after_it() {
        // Hidden desktop after typing: only the Static still matches "Field".
        let ms = m(&[("Field", "text")]);
        assert_eq!(
            pick_value_match(&ms, "Field", None, &alias),
            ValuePick::FromLabel { label: 0, fallback: Some(0) }
        );
    }

    #[test]
    fn name_plus_role_edit_reaches_the_field_through_its_label() {
        // The probe: name=Field role=edit failed because no element matched both.
        let ms = m(&[("Field", "text")]);
        assert_eq!(
            pick_value_match(&ms, "Field", Some("edit"), &alias),
            ValuePick::FromLabel { label: 0, fallback: None }
        );
    }

    #[test]
    fn a_non_editable_role_reads_the_label_itself() {
        let ms = m(&[("Field", "text")]);
        assert_eq!(pick_value_match(&ms, "Field", Some("text"), &alias), ValuePick::At(0));
    }

    #[test]
    fn status_text_that_merely_contains_the_name_is_read_itself() {
        let ms = m(&[("Status: Done", "text")]);
        assert_eq!(pick_value_match(&ms, "Status", None, &alias), ValuePick::At(0));
    }

    #[test]
    fn a_label_with_colon_and_accelerator_still_counts() {
        let ms = m(&[("&Name:", "text")]);
        assert_eq!(
            pick_value_match(&ms, "name", None, &alias),
            ValuePick::FromLabel { label: 0, fallback: Some(0) }
        );
    }

    #[test]
    fn nothing_matches_is_nothing() {
        assert_eq!(pick_value_match(&[], "Field", None, &alias), ValuePick::Nothing);
        let ms = m(&[("Field", "button")]);
        assert_eq!(pick_value_match(&ms, "Field", Some("edit"), &alias), ValuePick::Nothing);
    }

    #[test]
    fn the_field_after_a_label_is_the_next_editable() {
        assert_eq!(following_editable(&["edit"], None, &alias), Some(0));
        assert_eq!(following_editable(&["image", "document"], Some("edit"), &alias), Some(1));
    }

    #[test]
    fn another_label_first_means_no_field() {
        assert_eq!(following_editable(&["text", "edit"], None, &alias), None);
        assert_eq!(following_editable(&["button"], None, &alias), None);
        assert_eq!(following_editable(&["combobox"], Some("text"), &alias), None);
    }
}

/// Wiring pins, read from the source (a behavioural test needs a live tree).
#[cfg(test)]
mod wiring_tests {
    fn body<'a>(src: &'a str, sig: &str) -> &'a str {
        let start = src.find(sig).unwrap_or_else(|| panic!("{sig} not found"));
        &src[start..start + src[start..].find("\n}").expect("end of fn")]
    }

    /// A role given with the name narrows the walk itself: filtering 16 name
    /// hits afterwards missed a field behind 16 same-named links or cells.
    #[test]
    fn a_role_filter_narrows_the_lookup_before_the_label_hop() {
        let src = include_str!("value_target.rs");
        let b = body(src, "pub(crate) fn find_value_element(");
        let filtered = b.find("find_all_in_hwnd(hwnd, Some(n), Some(").expect("no role-filtered lookup");
        let unfiltered = b.find("find_all_in_hwnd(hwnd, Some(n), None").expect("no name lookup");
        assert!(filtered < unfiltered, "the role-filtered lookup must come first:\n{b}");
    }

    /// Editability is judged by the role Ghost reports (Linux normalises
    /// editable AT-SPI text to "edit"), never by the raw role id.
    #[test]
    fn value_reads_judge_roles_by_the_effective_role() {
        let vt = include_str!("value_target.rs");
        assert!(body(vt, "fn role_of(").contains("effective_role_name("), "role_of uses the raw role");
        let el = include_str!("element.rs");
        let rv = body(el, "pub(crate) fn read_value(");
        assert!(rv.contains("effective_role_name(") && !rv.contains("role_id_to_name("), "{rv}");
    }
}
