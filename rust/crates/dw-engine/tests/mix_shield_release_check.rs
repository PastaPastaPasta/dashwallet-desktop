//! The regtest suite's test-only CoinJoin calls (`mix_shield_test`, dwcli's
//! `mix-shield-test` feature) exist only in a debug build with the
//! `test-mix-shield` feature. Run both ways:
//!
//! ```text
//! cargo test --release -p dw-engine --features test-mix-shield --test mix_shield_release_check
//! cargo test -p dw-engine --features test-mix-shield --test mix_shield_release_check
//! ```
//!
//! The release run (the nightly's release-check job) proves the module
//! absent with the feature on; the debug run proves the probe can see it, so
//! the release result isn't vacuous.

/// `MIX_SHIELD_TEST` where `mix_shield_test` doesn't give the probe its
/// inherent constant (an inherent associated constant shadows a trait's).
trait Absent {
    const MIX_SHIELD_TEST: bool = false;
}

impl Absent for dw_engine::TestSupportProbe {}

const DEFAULT: bool = <dw_engine::TestSupportProbe as Absent>::MIX_SHIELD_TEST;
const _: () = assert!(!DEFAULT, "the trait's default is the module's absence");

const COMPILED_IN: bool = dw_engine::TestSupportProbe::MIX_SHIELD_TEST;

// A release build with the module compiled in doesn't build.
#[cfg(not(debug_assertions))]
const _: () = assert!(!COMPILED_IN, "mix_shield_test is in a release build");

#[test]
fn the_test_calls_are_compiled_only_in_a_debug_feature_build() {
    assert_eq!(
        COMPILED_IN,
        cfg!(all(debug_assertions, feature = "test-mix-shield")),
        "the probe disagrees with the build's cfg"
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_has_no_test_calls() {
    assert!(!COMPILED_IN, "mix_shield_test is in a release build");
}
