//! Every target's seeds, and every input saved under `regressions/<target>/`
//! (a minimised fuzz finding, kept after its fix), through the harness on
//! stable. The engine targets run with `--features engine`.

use std::path::Path;

fn runs(name: &str) -> bool {
    cfg!(feature = "engine") || !dw_fuzz::ENGINE_TARGETS.contains(&name)
}

#[test]
fn seeds_run_clean() {
    for name in dw_fuzz::TARGETS.into_iter().filter(|n| runs(n)) {
        let seeds = dw_fuzz::seeds::seeds(name);
        assert!(seeds.len() >= 4, "{name}: {} seeds", seeds.len());
        for seed in seeds {
            assert!(dw_fuzz::run(name, &seed), "{name}");
        }
    }
}

#[test]
fn regressions_run_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("regressions");
    for name in dw_fuzz::TARGETS.into_iter().filter(|n| runs(n)) {
        let Ok(dir) = std::fs::read_dir(root.join(name)) else {
            continue;
        };
        for entry in dir {
            let path = entry.unwrap().path();
            eprintln!("{}", path.display());
            assert!(dw_fuzz::run(name, &std::fs::read(&path).unwrap()), "{name}");
        }
    }
}

/// The saved finding is a version 2 PSBT with 99 bytes after its
/// transaction, which the parser now refuses; the special-payload fix it
/// found is replayed by an accepted version 3 PSBT whose round trips the
/// harness checks.
#[test]
fn psbt_findings_reach_their_outcome() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("regressions/psbt");
    let found = std::fs::read(root.join("special-payload-roundtrip")).unwrap();
    assert!(dw_psbt::parse(&found).is_err());
    let special = dw_fuzz::seeds::special_psbt();
    let parsed = dw_psbt::parse(&special).expect("the special PSBT parses");
    assert_eq!(parsed.unsigned_tx.version, 3);
    assert!(parsed.unsigned_tx.special_transaction_payload.is_some());
    dw_fuzz::psbt(&special);
    dw_fuzz::psbt(dw_psbt::to_base64(&parsed).as_bytes());
}

/// The encrypted wallet seed costs few enough rounds for the harness to
/// try its passphrase, and the passphrase opens it.
#[test]
fn the_encrypted_wallet_seed_decrypts() {
    let seed = dw_fuzz::seeds::cheap_encrypted_walletdat();
    assert!(dw_fuzz::seeds::seeds("walletdat").contains(&seed));
    let wallet = dw_fuzz::seeds::walletdat_wallet(&seed).expect("the seed parses");
    assert!(wallet.encrypted());
    assert!(wallet.derivation_rounds() <= dw_fuzz::FUZZ_ROUNDS);
    let root = wallet
        .hd_root(Some(dw_fuzz::seeds::CHEAP_PASSPHRASE))
        .expect("the passphrase opens the seed");
    assert!(root.mnemonic.is_some());
}

/// A Berkeley DB seed walks an internal page and an overflow chain.
#[test]
fn a_bdb_seed_walks_an_internal_page_and_an_overflow_chain() {
    let tree = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/compat/walletdat/bdb_tree_overflow.dat"),
    )
    .unwrap();
    let seeds = dw_fuzz::seeds::seeds("walletdat_bdb");
    assert!(seeds.iter().any(|s| s.ends_with(&tree)));
    let wallet = dw_compat::walletdat::read_bdb_bytes(&tree).expect("the tree reads");
    assert!(wallet.hd_root(None).is_ok());
}

/// The invitation seeds include links the claim's parser accepts.
#[cfg(feature = "engine")]
#[test]
fn invitation_seeds_include_claimable_links() {
    let accepted = dw_fuzz::seeds::seeds("invitation")
        .iter()
        .filter(|s| dw_engine::platform::fuzzing::invitation(&String::from_utf8_lossy(s)))
        .count();
    assert!(accepted >= 2, "{accepted}");
}

#[test]
fn the_linked_sqlite_has_the_wal_close_deadlock_fix() {
    assert!(
        rusqlite::version_number() >= 3_051_002,
        "SQLite {} can deadlock closing a WAL database (#105); 3.51.2 fixes it",
        rusqlite::version()
    );
}
