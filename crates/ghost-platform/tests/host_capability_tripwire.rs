//! Tripwire: the host's backend must report exactly the functionality Ghost has
//! actually shipped for that OS — `true` on Windows, `false` on macOS/Linux.
//!
//! This test exists to fail loudly if someone flips a scaffold to
//! `functional: true` before its native backend is built and verified on-device
//! (see the checklist in `docs/plans/2026-07-cross-platform-plan.md` §7). Turning
//! a platform on is a deliberate act that must edit this file in the same commit.
//!
//! # Why macOS may now list features while staying non-functional
//!
//! The original version of this file asserted that any non-Windows platform
//! advertises *no* `Feature` at all, because at the time both scaffolds contained
//! zero native code. macOS now has a real backend that compiles and links against
//! Apple's SDK, so its feature list describes which code exists. That is a
//! different claim from "this works", and `functional` is the field that makes the
//! second one. The invariant being defended here has therefore been made more
//! precise, not weaker: `functional` must still be false off Windows,
//! `BackgroundDispatch` must still be unclaimed off Windows, and Linux — which
//! genuinely has no native code — must still advertise nothing.

use ghost_platform::{capabilities_for, current, Feature, Platform, MAC_FEATURES};

#[test]
fn host_backend_functionality_matches_shipped_truth() {
    let backend = current();
    let caps = backend.capabilities();
    assert_eq!(caps.functional, backend.is_functional());

    if cfg!(windows) {
        assert!(
            backend.is_functional(),
            "the Windows backend is the shipped engine and must report functional"
        );
    } else {
        assert!(
            !backend.is_functional(),
            "{:?} has not been verified on-device, so it must not report functional",
            backend.platform()
        );
        assert!(
            !caps.supports(Feature::BackgroundDispatch),
            "background dispatch is a Windows posted-message primitive; it stays \
             unclaimed off Windows until it is measured on-device"
        );
    }

    if cfg!(target_os = "linux") {
        assert!(
            caps.supported.is_empty(),
            "Linux is a pure scaffold with no native code, so it must advertise \
             no Feature"
        );
    }
}

#[test]
fn no_scaffold_platform_claims_to_be_functional() {
    for platform in [Platform::MacOS, Platform::Linux] {
        let caps = capabilities_for(platform);
        assert!(!caps.functional, "{platform:?} must not claim functional yet");
        assert!(
            !caps.supports(Feature::BackgroundDispatch),
            "background dispatch is a Windows posted-message primitive; it stays \
             unclaimed off Windows until it is measured on-device"
        );
    }
}

#[test]
fn linux_advertises_nothing_because_it_has_no_native_code() {
    let caps = capabilities_for(Platform::Linux);
    assert!(caps.supported.is_empty(), "Linux must advertise no Feature");
}

#[test]
fn macos_advertises_exactly_the_features_whose_native_code_exists() {
    // The feature list is a statement about code, not about verification — see the
    // module docs. `functional` is asserted false above and stays that way until
    // `ghost doctor --mac` passes on real hardware.
    let caps = capabilities_for(Platform::MacOS);
    assert_eq!(caps.supported, MAC_FEATURES.to_vec());
    assert!(!caps.functional);
}
