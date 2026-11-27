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
        lower == "about:blank"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_https_and_about_blank_navigate() {
        for ok in ["http://localhost:3000/x", "https://example.com", "HTTP://127.0.0.1/", " https://a.b ", "about:blank"] {
            assert!(check_nav_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "file:///C:/Windows/win.ini", "chrome://version", "edge://settings", "devtools://devtools/x",
            "view-source:https://a.b", "javascript:alert(1)", "data:text/html,hi", "ftp://a.b/x",
            "chrome-extension://abc/p.html", "about:srcdoc", "about:config", "FILE:///etc/passwd",
            "jav\tascript:alert(1)", "\u{0}file:///x", "", "example.com", "//example.com",
        ] {
            let e = check_nav_url(bad).expect_err(bad);
            assert!(e.starts_with("invalid params: "), "{e}");
            assert!(e.contains("http") && e.contains("https") && e.contains("about:blank"), "{e}");
        }
    }
}
