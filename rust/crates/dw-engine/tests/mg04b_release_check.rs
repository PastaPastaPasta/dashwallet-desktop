//! MG-04b r2-m4's release check: the shared-masternode capability override
//! (`test_support::shared_masternodes_with_send_override`) exists only in a
//! debug build with the `test-support` feature. Run both ways:
//!
//! ```text
//! cargo test --release -p dw-engine --features test-support --test mg04b_release_check
//! cargo test -p dw-engine --features test-support --test mg04b_release_check
//! ```
//!
//! The release run proves the override absent (the nightly's release-check
//! job runs it); the debug run, in PR CI, proves the probe can see it, so
//! the release result isn't vacuous.

/// `SHARED_SEND_OVERRIDE` where `test_support` doesn't give
/// `TestSupportProbe` its inherent constant: an inherent associated
/// constant shadows a trait's, so the probe reads `true` only when the
/// override's module is compiled in.
trait Absent {
    const SHARED_SEND_OVERRIDE: bool = false;
}

impl Absent for dw_engine::TestSupportProbe {}

/// The trait's own value, checked so the trait is used in every build.
const DEFAULT: bool = <dw_engine::TestSupportProbe as Absent>::SHARED_SEND_OVERRIDE;
const _: () = assert!(!DEFAULT, "the trait's default is the override's absence");

const COMPILED_IN: bool = dw_engine::TestSupportProbe::SHARED_SEND_OVERRIDE;

// A release build with the override compiled in doesn't build.
#[cfg(not(debug_assertions))]
const _: () = assert!(
    !COMPILED_IN,
    "the capability override is in a release build"
);

#[test]
fn the_override_is_compiled_only_in_a_debug_test_support_build() {
    assert_eq!(
        COMPILED_IN,
        cfg!(all(debug_assertions, feature = "test-support")),
        "the probe disagrees with the build's cfg"
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_has_no_override() {
    assert!(
        !COMPILED_IN,
        "the capability override is in a release build"
    );
}

/// No shipped binary, and no other crate linking dw-engine, names the
/// override (validator r1 of the 1b fix round, NIT-3).
#[test]
fn no_binary_source_names_the_override() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut stack = vec![manifest.join("../../bin"), manifest.join("..")];
    let mut scanned = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("dw-engine") || path.ends_with("target") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                scanned += 1;
                let text = std::fs::read_to_string(&path).unwrap();
                for name in ["shared_masternodes_with_send_override", "with_shared_send"] {
                    assert!(!text.contains(name), "{} names {name}", path.display());
                }
            }
        }
    }
    assert!(scanned > 0, "no binary sources found");
}

/// The debug counterpart: the shipped facade says `can_send: true` (slice
/// 1c, DEC-678), and the override sets it either way.
#[cfg(all(debug_assertions, feature = "test-support"))]
#[test]
fn the_override_sets_can_send() {
    use std::sync::Arc;

    use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions};
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    use zeroize::Zeroizing;

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _: EngineEvent) {}
    }

    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NoEvents),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            dw_engine::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                    .as_bytes()
                    .to_vec(),
            ),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    let shipped = engine
        .block_on(session.masternodes().shared_capability(wallet))
        .unwrap();
    assert!(shipped.can_send);
    for can_send in [false, true] {
        let overridden =
            dw_engine::test_support::shared_masternodes_with_send_override(&session, can_send);
        assert_eq!(
            engine
                .block_on(overridden.shared_capability(wallet))
                .unwrap()
                .can_send,
            can_send
        );
    }
}
