//! The load-on-startup list across a removal whose app-database write fails
//! (#113).

use std::path::Path;
use std::sync::Arc;

use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

use crate::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, NetworkSession,
    SessionOptions, WalletId,
};

const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const LEGAL_12: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: EngineEvent) {}
}

fn engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap()
}

fn import(engine: &Engine, s: &Arc<NetworkSession>, phrase: &str) -> WalletId {
    engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap()
}

fn on_startup(s: &NetworkSession) -> Vec<(WalletId, bool)> {
    (s.wallet_load_states().unwrap().into_iter())
        .map(|w| (w.wallet_id, w.load_on_startup))
        .collect()
}

fn stored_list(s: &NetworkSession) -> Option<String> {
    let live = s.live().unwrap();
    live.appdb
        .setting(dw_appdb::GLOBAL_SCOPE, "load_on_startup")
        .unwrap()
}

/// Two wallets, only the first on the list; the first is removed with
/// `hooks` set to fail.
fn remove_the_only_listed(
    hooks: &[&std::sync::Mutex<Vec<std::path::PathBuf>>],
) -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    WalletId,
) {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();
    assert_eq!(stored_list(&s), Some(a.to_string()));
    for hook in hooks {
        hook.lock().unwrap().push(s.data_dir().to_path_buf());
    }
    let wipe = s
        .vault()
        .authorize(GrantPurpose::Wipe, Some(&a.0), Credential::None)
        .unwrap()
        .id;
    assert!(engine.block_on(s.remove_wallet(a, wipe)).is_err());
    (dir, engine, s, a, b)
}

/// A removal whose app-database transaction fails stores the list on its
/// own: the next open wallet is listed, on disk and in memory.
#[test]
fn a_failed_removal_transaction_still_stores_the_list() {
    let (_dir, engine, s, _a, b) = remove_the_only_listed(&[&crate::wallets::FAIL_LISTING_IN]);
    assert_eq!(on_startup(&s), vec![(b, true)]);
    assert_eq!(stored_list(&s), Some(b.to_string()));
    engine.block_on(engine.shutdown()).unwrap();
}

/// If the list cannot be stored on its own either, the list in memory is
/// still the repaired one; the stored one names only the removed wallet,
/// which the next start repairs.
#[test]
fn a_list_that_cannot_be_stored_is_repaired_in_memory() {
    let (_dir, engine, s, a, b) = remove_the_only_listed(&[
        &crate::wallets::FAIL_LISTING_IN,
        &crate::wallets::FAIL_LISTING_RETRY_IN,
    ]);
    assert_eq!(on_startup(&s), vec![(b, true)]);
    assert_eq!(stored_list(&s), Some(a.to_string()));
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let s = open(&engine);
    assert_eq!(on_startup(&s), vec![(b, true)]);
    assert_eq!(stored_list(&s), Some(b.to_string()));
    engine.block_on(engine.shutdown()).unwrap();
}

/// A removal whose store deletion fails leaves the stored list naming the
/// wallet: it is still in the store, so the next start loads it and the
/// removal can be tried again. The session shows the next wallet ticked.
#[test]
fn a_failed_store_deletion_keeps_the_stored_list() {
    let (_dir, engine, s, a, b) = remove_the_only_listed(&[&crate::wallets::FAIL_DELETE_IN]);
    assert!(on_startup(&s).contains(&(b, true)));
    assert_eq!(stored_list(&s), Some(a.to_string()));
    engine.block_on(engine.shutdown()).unwrap();
}

/// The first wallet added after the last one was removed shows ticked even
/// when the list cannot be stored with it on.
#[test]
fn a_wallet_added_after_the_last_removal_is_ticked_when_the_write_fails() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let wipe = s
        .vault()
        .authorize(GrantPurpose::Wipe, Some(&a.0), Credential::None)
        .unwrap()
        .id;
    engine.block_on(s.remove_wallet(a, wipe)).unwrap();
    assert_eq!(stored_list(&s).as_deref(), Some("none"));
    (crate::wallets::FAIL_LISTING_RETRY_IN.lock().unwrap()).push(s.data_dir().to_path_buf());
    let b = import(&engine, &s, LEGAL_12);
    assert_eq!(on_startup(&s), vec![(b, true)]);
    assert_eq!(stored_list(&s).as_deref(), Some("none"));
    engine.block_on(engine.shutdown()).unwrap();
}
