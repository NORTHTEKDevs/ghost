//! "Did you mean": the closest element names for a lookup that missed.
//!
//! An element miss used to cost the agent a `ghost_see` round trip (seconds of
//! model time) just to learn what the window does call the thing. Ranking the
//! window's element names against the query and putting the best few in the
//! error removes most of those round trips.

/// Lower-cased, whitespace-normalised form used for every comparison.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Similarity in [0, 1]: exact 1.0, prefix/substring high, otherwise token
/// overlap blended with a bounded edit distance. Pure and cheap.
pub fn similarity(query: &str, candidate: &str) -> f32 {
    let q = norm(query);
    let c = norm(candidate);
    if q.is_empty() || c.is_empty() {
        return 0.0;
    }
    if q == c {
        return 1.0;
    }
    if c.starts_with(&q) || q.starts_with(&c) {
        return 0.9;
    }
    if c.contains(&q) || q.contains(&c) {
        return 0.8;
    }
    let qt: Vec<&str> = q.split(' ').collect();
    let ct: Vec<&str> = c.split(' ').collect();
    let overlap = qt.iter().filter(|t| ct.contains(t)).count();
    let token_score = overlap as f32 / qt.len().max(ct.len()) as f32;
    let whole = |a: &str, b: &str| {
        let dist = levenshtein(a, b) as f32;
        1.0 - (dist / a.len().max(b.len()) as f32).min(1.0)
    };
    // Whole-string edit distance catches "cancl" -> "Cancel"; the best
    // per-token edit distance catches "submit" -> "Sumbit order", where the
    // typo lives in one word of a longer name.
    let edit_score = whole(&q, &c);
    let token_edit = qt
        .iter()
        .flat_map(|qw| ct.iter().map(move |cw| whole(qw, cw)))
        .fold(0.0_f32, f32::max);
    token_score.max(edit_score * 0.85).max(token_edit * 0.75)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().take(64).collect();
    let b: Vec<char> = b.chars().take(64).collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Up to `max` distinct names from `names`, best match first, dropping anything
/// with no meaningful similarity to `query`.
pub fn closest_names<'a, I>(query: &str, names: I, max: usize) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut scored: Vec<(f32, String)> = Vec::new();
    for n in names {
        let n = n.trim();
        if n.is_empty() || n.chars().count() > 80 {
            continue;
        }
        let s = similarity(query, n);
        if s < 0.34 {
            continue;
        }
        if scored.iter().any(|(_, existing)| existing.eq_ignore_ascii_case(n)) {
            continue;
        }
        scored.push((s, n.to_string()));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(max).map(|(_, n)| n).collect()
}

/// The sentence appended to a not-found error, or `None` when nothing is close.
pub fn did_you_mean(query: &str, names: impl IntoIterator<Item = String>, max: usize) -> Option<String> {
    let owned: Vec<String> = names.into_iter().collect();
    let best = closest_names(query, owned.iter().map(String::as_str), max);
    if best.is_empty() {
        return None;
    }
    Some(format!(
        "Closest element names in that window: {}",
        best.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_exact_prefix_substring_then_fuzzy() {
        let names = ["Post to Facebook", "Post", "Posting rules", "Cancel", "Settings"];
        let got = closest_names("post", names, 5);
        assert_eq!(got[0], "Post");
        assert!(got[1..3].contains(&"Post to Facebook".to_string()), "{got:?}");
        assert!(!got.contains(&"Settings".to_string()), "{got:?}");
    }

    #[test]
    fn tolerates_typos_and_drops_noise() {
        let names = ["Submit", "Sumbit order", "Close Tab", "Minimize", ""];
        let got = closest_names("submit", names, 3);
        assert_eq!(got[0], "Submit");
        assert!(got.contains(&"Sumbit order".to_string()), "{got:?}");
        assert!(!got.contains(&"Minimize".to_string()), "{got:?}");
    }

    #[test]
    fn dedupes_case_variants_and_caps_the_list() {
        let names = ["save", "Save", "SAVE", "Save as", "Save all", "Saved items"];
        let got = closest_names("save", names, 3);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].to_lowercase(), "save");
    }

    #[test]
    fn sentence_is_absent_when_nothing_is_close() {
        assert!(did_you_mean("zzqq", vec!["Cancel".into(), "OK".into()], 5).is_none());
        let s = did_you_mean("cancl", vec!["Cancel".into(), "OK".into()], 5).unwrap();
        assert!(s.contains("'Cancel'"), "{s}");
    }
}
