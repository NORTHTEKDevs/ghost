//! Argument validation at the dispatch boundary (round-3 probe findings). Pure:
//! nothing here touches a window, the keyboard, the clipboard or the stop events.

mod see_mode {
    //! H1: an unknown `ghost_see` mode, or mode=marks|selection with a window,
    //! must never fall through to a read of the human's foreground window.
    use crate::{is_window_scoped, see_mode, see_scope_hwnd, validate_args, SEE_MODES};
    use serde_json::json;

    #[test]
    fn omitted_or_null_mode_is_fast() {
        assert_eq!(see_mode(&json!({})), Ok("fast"));
        assert_eq!(see_mode(&json!({ "mode": null })), Ok("fast"));
    }

    #[test]
    fn every_schema_mode_parses_case_insensitively() {
        for m in SEE_MODES {
            assert_eq!(see_mode(&json!({ "mode": m })), Ok(m));
            assert_eq!(see_mode(&json!({ "mode": m.to_uppercase() })), Ok(m));
        }
        assert_eq!(see_mode(&json!({ "mode": "Text" })), Ok("text"));
        assert_eq!(see_mode(&json!({ "mode": "FULL" })), Ok("full"));
    }

    #[test]
    fn unknown_mode_is_invalid_params_naming_the_modes() {
        for bad in [json!("bogus"), json!(""), json!("  "), json!(3), json!(true)] {
            let e = see_mode(&json!({ "mode": bad })).unwrap_err();
            assert!(e.starts_with("invalid params: "), "{e}");
            assert!(e.contains("fast|full|delta|text|marks|selection"), "{e}");
        }
    }

    #[test]
    fn validate_rewrites_mode_to_canonical_and_rejects_unknown() {
        let out = validate_args("ghost_see", &json!({ "mode": "Text", "window": "X" })).unwrap();
        assert_eq!(out.unwrap()["mode"], "text");
        assert!(validate_args("ghost_see", &json!({ "mode": "text" })).unwrap().is_none());
        assert!(validate_args("ghost_see", &json!({ "mode": "bogus", "window": "X" })).is_err());
    }

    #[test]
    fn every_see_mode_is_window_scoped() {
        for m in SEE_MODES {
            assert!(
                is_window_scoped("ghost_see", &json!({ "mode": m, "window": "Notepad" })),
                "mode={m} must resolve the window"
            );
        }
        assert!(is_window_scoped("ghost_see", &json!({})));
    }

    #[test]
    fn marks_and_selection_read_the_resolved_window() {
        // The resolver put the handle on the args: that window, nothing else.
        let p = json!({ "mode": "selection", "window": "Notepad", "_target_hwnd": 4242 });
        assert_eq!(see_scope_hwnd(&p), Ok(Some(4242)));
        let p = json!({ "mode": "marks", "window": "Notepad", "_target_hwnd": 4242 });
        assert_eq!(see_scope_hwnd(&p), Ok(Some(4242)));
        // A window was named but no handle could be honoured: refuse.
        let p = json!({ "mode": "marks", "window": "Notepad" });
        assert!(see_scope_hwnd(&p).is_err());
        let p = json!({ "mode": "selection", "window": "Notepad", "_target_hwnd": 0 });
        assert!(see_scope_hwnd(&p).is_err());
        // Nothing named, no anchor: the documented foreground default.
        assert_eq!(see_scope_hwnd(&json!({ "mode": "marks" })), Ok(None));
    }
}

mod wait_timeout {
    //! L3: a wait's timeout_ms is capped at the documented maximum.
    use crate::{validate_args, MAX_WAIT_TIMEOUT_MS};
    use serde_json::json;

    const WAITS: [&str; 8] = [
        "ghost_wait", "ghost_wait_for_element", "ghost_wait_for_value", "ghost_wait_for_idle",
        "ghost_wait_for_event", "ghost_wait_until", "ghost_navigate_and_wait",
        "ghost_click_and_wait_for_text",
    ];

    #[test]
    fn the_cap_matches_ghost_shell() {
        assert_eq!(MAX_WAIT_TIMEOUT_MS, 600_000);
    }

    #[test]
    fn a_huge_timeout_is_capped_on_every_wait() {
        for name in WAITS {
            let args = json!({ "for": "element", "name": "x", "timeout_ms": 1u64 << 62 });
            let out = validate_args(name, &args).unwrap().expect("rewritten");
            assert_eq!(out["timeout_ms"], json!(MAX_WAIT_TIMEOUT_MS), "{name}");
        }
    }

    #[test]
    fn a_timeout_within_the_cap_is_untouched() {
        let args = json!({ "for": "value", "timeout_ms": 600_000 });
        assert!(validate_args("ghost_wait", &args).unwrap().is_none());
        assert!(validate_args("ghost_wait", &json!({ "for": "element" })).unwrap().is_none());
    }

    #[test]
    fn a_non_integer_timeout_is_invalid_params() {
        for bad in [json!(-1), json!(2.5), json!("5000")] {
            let e = validate_args("ghost_wait", &json!({ "for": "element", "timeout_ms": bad })).unwrap_err();
            assert!(e.starts_with("invalid params: ") && e.contains("timeout_ms"), "{e}");
        }
    }
}
