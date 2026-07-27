//! Tripwire: the host's backend must report exactly the functionality Ghost has
//! actually shipped for that OS — `true` on Windows, `false` on macOS/Linux.
//!
//! This test exists to fail loudly if someone flips a scaffold to
//! `functional: true` before its native backend is built and verified on-device
//! (see the checklist in `docs/plans/2026-07-cross-platform-plan.md` §7). Turning
//! a platform on is a deliberate act that must edit this file in the same commit.

use ghost_platform::{capabilities_for, current, Feature, Platform};

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
            "{:?} is a scaffold: no native backend has been built and verified \
             on-device, so it must not report functional",
            backend.platform()
        );
        assert!(
            caps.supported.is_empty(),
            "a scaffold must not advertise any Feature"
        );
    }
}

#[test]
fn scaffold_platforms_never_claim_capabilities() {
    for platform in [Platform::MacOS, Platform::Linux] {
        let caps = capabilities_for(platform);
        assert!(!caps.functional, "{platform:?} must not claim functional yet");
        assert!(caps.supported.is_empty(), "{platform:?} must advertise no Feature");
        assert!(
            !caps.supports(Feature::BackgroundDispatch),
            "background dispatch is a Windows posted-message primitive; it stays \
             unclaimed off Windows until it is measured on-device"
        );
    }
}
