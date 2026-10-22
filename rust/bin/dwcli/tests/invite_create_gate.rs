//! DP5-02 D18: `invite create`, the test-only invitation maker, is in no
//! build without dwcli's `invitation-create-test` feature, and that feature
//! does not compile without `debug_assertions`, so no release or dist build
//! has it.

use std::path::Path;
use std::process::Command;

/// The binary these tests run is built without the feature: the command
/// does not parse, before any engine or wallet exists.
#[cfg(not(feature = "invitation-create-test"))]
#[test]
fn a_build_without_the_feature_has_no_invite_create() {
    let dir = dw_testutil::private_tempdir();
    let out = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.path().join("data"))
        .args(["invite", "create", "--duffs", "1000000"])
        .args(["--phrase-file", "p", "--out", "o"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unrecognized subcommand"), "{stderr}");
    assert!(!dir.path().join("data").exists(), "nothing ran");
}

/// A release build with the feature stops at `main.rs`'s `compile_error!`.
/// A check of the whole release graph takes minutes, so it is run on
/// demand (the DP5-02 gate does): `cargo test -p dwcli --test
/// invite_create_gate -- --ignored`.
#[test]
#[ignore = "a release check of dwcli's graph; run by the DP5-02 gate"]
fn a_release_build_refuses_the_feature() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let out = Command::new(env!("CARGO"))
        .args(["check", "--release", "--message-format", "short"])
        .args(["--features", "invitation-create-test", "--manifest-path"])
        .arg(&manifest)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("the invitation-create-test feature is for debug builds only"),
        "{stderr}"
    );
}
