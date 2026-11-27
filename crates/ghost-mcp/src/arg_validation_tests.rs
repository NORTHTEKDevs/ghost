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

mod act_args {
    //! L6: an empty name must not match (and click) the first element, and an
    //! unknown dispatch mode must not silently mean "instant".
    use crate::validate_args;
    use serde_json::json;

    #[test]
    fn an_empty_name_or_role_is_invalid_params() {
        for verb in ["ghost_act", "ghost_find"] {
            for args in [
                json!({ "action": "click", "name": "" }),
                json!({ "action": "click", "name": "   " }),
                json!({ "action": "click", "role": "" }),
            ] {
                let e = validate_args(verb, &args).unwrap_err();
                assert!(e.starts_with("invalid params: "), "{verb} {args}: {e}");
            }
        }
        assert!(validate_args("ghost_act", &json!({ "action": "click", "name": "OK" })).unwrap().is_none());
    }

    #[test]
    fn an_unknown_mode_is_invalid_params_naming_the_modes() {
        for verb in ["ghost_act", "ghost_find"] {
            let e = validate_args(verb, &json!({ "action": "click", "name": "OK", "mode": "bogus" })).unwrap_err();
            assert!(e.starts_with("invalid params: ") && e.contains("instant|deliberate|instant_only"), "{e}");
            for m in ["instant", "deliberate", "instant_only"] {
                assert!(validate_args(verb, &json!({ "name": "OK", "mode": m })).is_ok());
            }
        }
    }
}

mod key_spec {
    //! L7: keys='down:Shift' / 'up:Shift' and unknown key names answer with a
    //! message naming the value, never "Win32 error 0x00000000".
    use crate::{bad_key_part, hold_refusal, key_hold, key_name_error, validate_args};
    use serde_json::json;

    fn known(k: &str) -> bool {
        matches!(k.to_lowercase().as_str(), "shift" | "ctrl" | "enter" | "s" | "+")
    }

    #[test]
    fn hold_prefixes_parse() {
        assert_eq!(key_hold("down:Shift"), Some((true, "Shift")));
        assert_eq!(key_hold("up:Shift"), Some((false, "Shift")));
        assert_eq!(key_hold("Shift"), None);
        assert_eq!(key_hold("Ctrl+S"), None);
    }

    #[test]
    fn a_hold_with_no_key_is_invalid_params() {
        for keys in ["down:", "up:", "down:  "] {
            let e = validate_args("ghost_key", &json!({ "keys": keys })).unwrap_err();
            assert!(e.starts_with("invalid params: "), "{e}");
        }
        assert!(validate_args("ghost_key", &json!({ "keys": "down:Shift" })).unwrap().is_none());
    }

    #[test]
    fn a_hold_off_the_foreground_path_says_why_and_names_the_keys() {
        let e = hold_refusal("down:Shift", "the background policy");
        assert!(e.contains("down:Shift") && e.contains("foreground"), "{e}");
        assert!(!e.contains("0x00000000"), "{e}");
    }

    #[test]
    fn the_unknown_part_of_a_spec_is_named() {
        assert_eq!(bad_key_part("Ctrl+Bogus", known), Some("Bogus".into()));
        assert_eq!(bad_key_part("Hyper+S", known), Some("Hyper".into()));
        assert_eq!(bad_key_part("down:Bogus", known), Some("Bogus".into()));
        assert_eq!(bad_key_part("Ctrl+S", known), None);
        // A single character with no VK name is typed as that character.
        assert_eq!(bad_key_part("*", known), None);
        assert_eq!(bad_key_part("FooBar", known), Some("FooBar".into()));
    }

    #[test]
    fn an_unknown_key_error_becomes_invalid_params_naming_it() {
        let e = key_name_error(
            "Ctrl+Bogus",
            "Core error: Win32 error 0x00000000 in unknown key name".into(),
            known,
        );
        assert!(e.starts_with("invalid params: ") && e.contains("'Bogus'"), "{e}");
        assert!(!e.contains("0x00000000"), "{e}");
        // Any other error passes through untouched.
        assert_eq!(key_name_error("Enter", "boom".into(), known), "boom");
    }

    #[test]
    fn an_unknown_window_state_is_invalid_params_naming_it() {
        let e = validate_args("ghost_window", &json!({ "op": "state", "name": "X", "state": "bogus" })).unwrap_err();
        assert!(e.starts_with("invalid params: ") && e.contains("bogus"), "{e}");
        assert!(e.contains("maximize|minimize|restore|close"), "{e}");
        let e = validate_args("ghost_window_state", &json!({ "name": "X", "state": "huge" })).unwrap_err();
        assert!(e.contains("huge"), "{e}");
        assert!(validate_args("ghost_window", &json!({ "op": "state", "name": "X", "state": "Minimize" })).is_ok());
        assert!(validate_args("ghost_window", &json!({ "op": "list" })).unwrap().is_none());
    }

    #[test]
    fn an_unknown_window_op_is_invalid_params() {
        let e = validate_args("ghost_window", &json!({ "op": "explode" })).unwrap_err();
        assert!(e.starts_with("invalid params: ") && e.contains("explode"), "{e}");
    }
}

mod event_wait {
    //! L4: for=event answers a timeout like every other wait: ok:false and
    //! timed_out:true, not ok:true with timed_out:true.
    use crate::{event_wait_result, wrap_envelope};
    use serde_json::json;

    #[test]
    fn an_event_is_ok_and_not_timed_out() {
        let v = event_wait_result(Ok(7)).unwrap();
        assert_eq!(v["seq"], json!(7));
        assert_eq!(v["timed_out"], json!(false));
        let env = wrap_envelope(Ok(v));
        assert_eq!(env["ok"], json!(true));
    }

    #[test]
    fn a_timeout_is_ok_false_and_timed_out() {
        let r = event_wait_result(Err("Timeout after 5ms waiting for: wait_for_event".into()));
        let env = wrap_envelope(r);
        assert_eq!(env["ok"], json!(false));
        assert_eq!(env["timed_out"], json!(true));
    }

    #[test]
    fn every_wait_timeout_carries_timed_out() {
        let env = wrap_envelope(Err("Timeout after 5ms waiting for: value equals \"x\" (last \"\")".into()));
        assert_eq!(env["ok"], json!(false));
        assert_eq!(env["timed_out"], json!(true));
        let env = wrap_envelope(Err("params must include 'name' or 'role'".into()));
        assert!(env.get("timed_out").is_none());
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
