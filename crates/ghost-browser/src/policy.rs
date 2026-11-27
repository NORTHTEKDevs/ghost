//! Pure policy and result-shaping helpers for the tab tools.

/// Schemes a tab may be pointed at. A tab is a real browser, so http(s) to any
/// host (including localhost dev servers) is allowed; the ghost_http_get SSRF
/// policy does not apply. Everything that reads local files or browser
/// internals is refused.
pub fn check_nav_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    // Browsers strip tabs/newlines inside a scheme ("jav\tascript:"), so a
    // control character anywhere is refused rather than normalized.
    let ok = !u.chars().any(|c| c.is_control()) && {
        let lower = u.to_ascii_lowercase();
        lower.strip_prefix("about:blank").is_some_and(|r| r.is_empty() || r.starts_with(['#', '?']))
            || lower.strip_prefix("http://").is_some_and(|r| !r.is_empty())
            || lower.strip_prefix("https://").is_some_and(|r| !r.is_empty())
    };
    if ok {
        Ok(())
    } else {
        let shown: String = u.chars().take(60).collect();
        Err(format!(
            "invalid params: url scheme not allowed ('{shown}'); tabs navigate only to http://, https:// or about:blank"
        ))
    }
}

/// Default and ceiling for `max_chars` on ghost_tab_text / ghost_tab_eval.
pub const DEFAULT_MAX_CHARS: usize = 100_000;
pub const MAX_MAX_CHARS: usize = 2_000_000;

/// Resolve the optional `max_chars` parameter.
pub fn resolve_max_chars(requested: Option<&serde_json::Value>) -> Result<usize, String> {
    match requested {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_MAX_CHARS),
        Some(v) => match v.as_u64() {
            Some(n) if (1..=MAX_MAX_CHARS as u64).contains(&n) => Ok(n as usize),
            _ => Err(format!(
                "invalid params: max_chars must be an integer from 1 to {MAX_MAX_CHARS} (default {DEFAULT_MAX_CHARS})"
            )),
        },
    }
}

/// Cut `s` to at most `max` chars on a char boundary. Returns the text and,
/// when something was cut, the original length in chars.
pub fn truncate_chars(s: &str, max: usize) -> (String, Option<usize>) {
    // Byte length >= char count, so a short byte length proves no cut is needed.
    if s.len() <= max {
        return (s.to_string(), None);
    }
    match s.char_indices().nth(max) {
        None => (s.to_string(), None),
        Some((cut, _)) => (s[..cut].to_string(), Some(s.chars().count())),
    }
}

/// Decide whether a navigation worked, from the `Page.navigate` result and
/// (once known) the URL the tab landed on. A failed load (DNS error, refused
/// connection) is a successful CDP call that parks the tab on chrome-error://.
///
/// `net::ERR_ABORTED` is what Chromium reports when the response was a
/// download or a 204/205 (nothing to render, the tab stays put): that request
/// worked, so it is Ok with a note, unless the tab landed on an error page.
pub fn classify_navigation(
    navigate_result: &serde_json::Value,
    landed_url: Option<&str>,
) -> Result<Option<&'static str>, String> {
    let mut note = None;
    if let Some(err) = navigate_result.get("errorText").and_then(|e| e.as_str()) {
        if err == "net::ERR_ABORTED" {
            note = Some(NAV_ABORTED_NOTE);
        } else if !err.is_empty() {
            return Err(format!("navigation failed: {err}"));
        }
    }
    match landed_url {
        Some(u) if u.starts_with("chrome-error://") => {
            Err(format!("navigation failed: the page failed to load (tab landed on {u})"))
        }
        _ => Ok(note),
    }
}

/// The note for a navigation Chromium answered with net::ERR_ABORTED.
pub const NAV_ABORTED_NOTE: &str = "the browser did not render a new page (net::ERR_ABORTED): the response was most likely a download or an empty 204/205 response, so the tab stayed on its previous page";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_navigation_is_an_error_naming_the_cause() {
        use serde_json::json;
        let ok = json!({ "frameId": "F", "loaderId": "L" });
        assert!(classify_navigation(&ok, None).is_ok());
        assert!(classify_navigation(&ok, Some("https://example.com/")).is_ok());
        assert!(classify_navigation(&ok, Some("about:blank")).is_ok());
        // errorText on the Page.navigate result, before any load.
        let dns = json!({ "frameId": "F", "errorText": "net::ERR_NAME_NOT_RESOLVED" });
        let e = classify_navigation(&dns, None).expect_err("dns");
        assert!(e.contains("net::ERR_NAME_NOT_RESOLVED"), "{e}");
        // Empty errorText is not an error.
        assert!(classify_navigation(&json!({ "errorText": "" }), None).is_ok());
        // Landed on the error page with no errorText in hand.
        let e = classify_navigation(&ok, Some("chrome-error://chromewebdata/")).expect_err("error page");
        assert!(e.contains("chrome-error://chromewebdata/") && e.contains("failed to load"), "{e}");
        // errorText wins when both are present.
        let e = classify_navigation(&dns, Some("chrome-error://chromewebdata/")).unwrap_err();
        assert!(e.contains("net::ERR_NAME_NOT_RESOLVED"), "{e}");
    }

    /// A download or a 204 answers Page.navigate with net::ERR_ABORTED: the
    /// request worked and the tab stayed where it was.
    #[test]
    fn an_aborted_navigation_from_a_download_or_204_is_not_a_failure() {
        use serde_json::json;
        let aborted = json!({ "frameId": "F", "loaderId": "L", "errorText": "net::ERR_ABORTED" });
        assert_eq!(classify_navigation(&aborted, None), Ok(Some(NAV_ABORTED_NOTE)));
        assert_eq!(classify_navigation(&aborted, Some("https://example.com/page")), Ok(Some(NAV_ABORTED_NOTE)));
        assert_eq!(classify_navigation(&json!({ "frameId": "F" }), Some("https://example.com/")), Ok(None));
        // Landing on the error page is still a failure.
        let e = classify_navigation(&aborted, Some("chrome-error://chromewebdata/")).expect_err("error page");
        assert!(e.contains("chrome-error://"), "{e}");
        // Other errorText still fails.
        for err in ["net::ERR_CONNECTION_REFUSED", "net::ERR_ABORTED_X", "net::ERR_BLOCKED_BY_CLIENT"] {
            assert!(classify_navigation(&json!({ "errorText": err }), None).is_err(), "{err}");
        }
    }

    #[test]
    fn truncation_is_char_boundary_safe_and_reports_original_length() {
        let (t, o) = truncate_chars("hello", 10);
        assert_eq!((t.as_str(), o), ("hello", None));
        let (t, o) = truncate_chars("hello", 5);
        assert_eq!((t.as_str(), o), ("hello", None), "exactly at the cap is not cut");
        let (t, o) = truncate_chars("hello world", 5);
        assert_eq!((t.as_str(), o), ("hello", Some(11)));
        // 4-byte emoji and 2-byte accents: a byte cut would split them.
        let s = "a\u{1F600}\u{e9}\u{1F600}b";
        let (t, o) = truncate_chars(s, 3);
        assert_eq!(t, "a\u{1F600}\u{e9}");
        assert_eq!(o, Some(5));
        let big = "x".repeat(50 * 1024 * 1024);
        let (t, o) = truncate_chars(&big, DEFAULT_MAX_CHARS);
        assert_eq!(t.len(), DEFAULT_MAX_CHARS);
        assert_eq!(o, Some(50 * 1024 * 1024));
        assert_eq!(truncate_chars("abc", 0), (String::new(), Some(3)));
    }

    #[test]
    fn max_chars_defaults_and_is_bounded() {
        use serde_json::json;
        assert_eq!(resolve_max_chars(None), Ok(100_000));
        assert_eq!(resolve_max_chars(Some(&json!(null))), Ok(100_000));
        assert_eq!(resolve_max_chars(Some(&json!(500))), Ok(500));
        assert_eq!(resolve_max_chars(Some(&json!(2_000_000))), Ok(2_000_000));
        for bad in [json!(0), json!(2_000_001), json!(-5), json!("10"), json!(1.5)] {
            let e = resolve_max_chars(Some(&bad)).expect_err("bad max_chars");
            assert!(e.starts_with("invalid params: ") && e.contains("max_chars"), "{e}");
        }
    }

    #[test]
    fn only_http_https_and_about_blank_navigate() {
        for ok in ["http://localhost:3000/x", "https://example.com", "HTTP://127.0.0.1/", " https://a.b ", "about:blank",
                   "about:blank#x", "about:blank?x", "About:Blank#"] {
            assert!(check_nav_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "file:///C:/Windows/win.ini", "chrome://version", "edge://settings", "devtools://devtools/x",
            "view-source:https://a.b", "javascript:alert(1)", "data:text/html,hi", "ftp://a.b/x",
            "chrome-extension://abc/p.html", "about:srcdoc", "about:config", "FILE:///etc/passwd",
            "jav\tascript:alert(1)", "\u{0}file:///x", "", "example.com", "//example.com",
            "about:blankx", "about:blank/x", "about:blank:x",
        ] {
            let e = check_nav_url(bad).expect_err(bad);
            assert!(e.starts_with("invalid params: "), "{e}");
            assert!(e.contains("http") && e.contains("https") && e.contains("about:blank"), "{e}");
        }
    }
}
