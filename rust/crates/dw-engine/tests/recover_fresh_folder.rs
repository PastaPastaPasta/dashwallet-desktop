//! Q107 C1's way out of a refused open: the network starts over in a new
//! folder beside the refused one, with the restore's RESTORED mark set, and
//! the refused folder stays byte for byte as it was through the whole setup.
//! Its own test binary: the probe is cached reading P9a Absent, so a retired
//! contact-pool row is a real refusal.

mod store_upgrade_common;

use std::path::Path;
use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};
use dw_store_upgrade::{Adopters, P9aStatus, UPGRADE_BACKUPS_DIR};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use store_upgrade_common::*;
use zeroize::Zeroizing;

const FIRST: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const SECOND: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// An engine on `root` whose OS store outlives it, as the keychain does.
fn engine_on(root: &Path, os_store: &Arc<MemoryOsStore>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: os_store.clone(),
                ..VaultConfig::default()
            },
        },
        Arc::new(NoEvents),
    )
    .unwrap()
}

/// The data root's entries, by name.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn restored_mark_set(dir: &Path) -> bool {
    dir.join(UPGRADE_BACKUPS_DIR).join("RESTORED").is_file()
}

/// The onboarding restore: a wallet from its recovery phrase.
fn restore(engine: &Engine, session: &Arc<NetworkSession>, phrase: &str) -> WalletId {
    engine
        .block_on(session.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap()
}

fn wallet_count(session: &NetworkSession) -> usize {
    session.wallet_load_states().unwrap().len()
}

/// Takes every permission off `dir` and returns its mode, or `None` when
/// this process reads it anyway (root, CAP_DAC_READ_SEARCH): then the
/// check proves nothing and the folder is left as it is.
#[cfg(unix)]
fn make_unreadable(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o7777;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(dir).is_ok() {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        eprintln!("unreadable refused folder: skipped, this process reads it anyway");
        return None;
    }
    eprintln!("unreadable refused folder: yes");
    Some(mode)
}

#[test]
fn a_refused_network_starts_over_in_a_new_folder_and_never_touches_the_refused_one() {
    let probe_root = dw_testutil::private_tempdir();
    Adopters::cache_with_p9a_for_test(probe_root.path(), P9aStatus::Absent);
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    let os_store = Arc::new(MemoryOsStore::new());
    let pristine = tree(&f.net);

    let engine = engine_on(&f.root, &os_store);
    let not_yet = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(not_yet, Err(EngineError::InvalidArgument(_))),
        "only after a refused open: {not_yet:?}"
    );
    let refused = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(
        matches!(refused, Err(EngineError::StoreRefused(_))),
        "{:?}",
        refused.err()
    );
    let before = tree(&f.net);
    assert_eq!(refused_open_diff(&pristine, &before), Vec::<String>::new());
    assert!(
        before.values().filter(|v| !v.ends_with(" dir")).count() >= 3,
        "every file in the refused folder is hashed: {before:?}"
    );
    let root_before = listing(&f.root);
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), f.net);

    let fresh = engine
        .block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(fresh, f.root.join("regtest-recovered-1"));
    assert_ne!(fresh, f.net);
    assert_eq!(
        tree_diff(&before, &tree(&f.net)),
        Vec::<String>::new(),
        "the refused folder is byte-identical, listing included"
    );
    let mut root_after = root_before.clone();
    root_after.extend(["network-folders.json".into(), "regtest-recovered-1".into()]);
    root_after.sort();
    assert_eq!(
        listing(&f.root),
        root_after,
        "nothing else in the data root"
    );
    assert_eq!(
        listing(&fresh),
        [UPGRADE_BACKUPS_DIR],
        "empty but for the mark"
    );
    assert_eq!(listing(&fresh.join(UPGRADE_BACKUPS_DIR)), ["RESTORED"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), fresh);
    let again = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(again, Err(EngineError::InvalidArgument(_))),
        "once per refusal: {again:?}"
    );
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    // From here on nothing may read the refused folder.
    #[cfg(unix)]
    let locked = make_unreadable(&f.net);

    // The restart: the new folder opens, empty, with the gate's mark; the
    // first wallet is restored from its phrase, and a second through Add a
    // wallet, both under the same mark.
    let engine = engine_on(&f.root, &os_store);
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), fresh);
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("the new folder opens");
    assert_eq!(session.data_dir(), fresh.as_path());
    assert_eq!(wallet_count(&session), 0);
    assert!(restored_mark_set(&fresh), "the first open keeps the mark");
    let busy = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(busy, Err(EngineError::InvalidArgument(_))),
        "never while open: {busy:?}"
    );
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let first = restore(&engine, &session, FIRST);
    let second = restore(&engine, &session, SECOND);
    assert_ne!(first, second);
    assert_eq!(wallet_count(&session), 2);
    assert!(
        restored_mark_set(&fresh),
        "the mark covers every restored wallet"
    );
    let _ = engine.block_on(session.start_spv());
    let listed = engine.block_on(engine.existing_networks()).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|n| (n.network.clone(), n.directory.clone()))
            .collect::<Vec<_>>(),
        [(DashNetwork::Regtest, fresh.to_string_lossy().into_owned())],
        "listed by its new folder, never the refused one"
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    // The second restart opens the new folder again, never the refusal.
    let engine = engine_on(&f.root, &os_store);
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("the set-up folder opens again");
    assert_eq!(session.data_dir(), fresh.as_path());
    assert_eq!(wallet_count(&session), 2);
    assert!(restored_mark_set(&fresh), "held until its first full sync");
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    #[cfg(unix)]
    if let Some(mode) = locked {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f.net, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    assert_eq!(
        tree_diff(&before, &tree(&f.net)),
        Vec::<String>::new(),
        "the refused folder is byte-identical after the whole setup: every name, mode and hash"
    );
    assert_eq!(
        refused_open_diff(&pristine, &tree(&f.net)),
        Vec::<String>::new(),
        "only the refused open's own snapshot was ever added"
    );
}

#[test]
fn a_damaged_folder_record_fails_every_open_closed() {
    let f = fixture("v0.1.0");
    std::fs::write(f.root.join("network-folders.json"), b"{\"regtest\": ").unwrap();
    let engine = engine(&f.root);
    assert!(matches!(
        engine.network_dir(&DashNetwork::Regtest),
        Err(EngineError::Storage(_))
    ));
    let opened = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(
        matches!(opened, Err(EngineError::Storage(_))),
        "never the default folder: {:?}",
        opened.err()
    );
    assert!(matches!(
        engine.block_on(engine.existing_networks()),
        Err(EngineError::Storage(_))
    ));
    engine.block_on(engine.shutdown()).unwrap();
}
