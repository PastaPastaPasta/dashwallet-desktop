//! DEC-1046 (E1 review r1, R1-1): no close, unload or quit deletes a
//! shielded spend whose result is not known. At the pin, platform-wallet's
//! `remove_wallet` always purges the wallet's shielded store, its stored
//! transitions and reservations included (`wallet_lifecycle.rs:847-850` →
//! `coordinator.rs:878-902` → `file_store.rs:880-901`), and it has no detach.
//! So Close Wallet and an in-app network close or switch are refused while a
//! wallet holds one, and the quit keeps that wallet loaded: its rows are on
//! disk, and the next launch rehydrates them as pending.

use std::path::Path;
use std::sync::Arc;

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};
use platform_wallet::wallet::shielded::store::{PendingRedrive, StalePendingSpend};
use platform_wallet::wallet::shielded::{ShieldedStore, SubwalletId};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::runtime::StoredSpends;
use super::shielded::UnresolvedSpend;
use super::shielded_pool::{self as pool, on_pool};
use super::topup_tests::options;
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASS: &[u8] = b"pass phrase";
/// Where the quitting child process keeps its data root.
const QUIT_ROOT: &str = "DW_E1_QUIT_ROOT";

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

/// An engine over `root` whose passphrase vault needs no OS store, so a
/// second process opens it as the first left it.
fn engine_at(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.join("data"),
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
        .block_on(engine.open_network(DashNetwork::Regtest, options()))
        .unwrap()
}

/// A new vault and the "abandon … about" wallet, its viewing key bound
/// (and so persisted in wallet.sqlite).
fn first_launch(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    let session = open(engine);
    engine
        .block_on(session.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(bind(engine, &session, wallet));
    (session, wallet)
}

fn bind(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> bool {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok(pool::ShieldedLib::bind(&lib).await?)
        }))
        .unwrap()
}

/// Two spends of unknown result as the library stores them (operations.rs
/// `arm_redrive_record`): a transfer's and an unshield's signed bytes, their
/// nullifiers reserved against the anchor they were built on.
fn spends() -> Vec<PendingRedrive> {
    [
        (1u8, b"a signed shielded transfer".as_slice()),
        (2, b"a signed unshield".as_slice()),
    ]
    .into_iter()
    .map(|(n, st)| PendingRedrive {
        activity_id: [n; 32],
        anchor: [0xa0 + n; 32],
        nullifiers: vec![[0x10 + n; 32], [0x20 + n; 32]],
        st_bytes: st.to_vec(),
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
    })
    .collect()
}

fn subwallet(wallet: WalletId) -> SubwalletId {
    SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT)
}

/// Stores [`spends`] for `wallet` and puts them on record in the dispatch
/// journal (a pass's admission, with the background lease).
fn store_spends(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m).await.unwrap();
        {
            let mut store = coordinator.store().write().await;
            for r in spends() {
                for n in &r.nullifiers {
                    assert!(store.mark_pending(subwallet(wallet), n).unwrap());
                    (store.set_pending_spend(subwallet(wallet), n, r.anchor, r.activity_id))
                        .unwrap();
                }
                store.arm_redrive(subwallet(wallet), r).unwrap();
            }
        }
        s.ensure_background().await;
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
}

/// What must outlive the close: the stored transitions, their
/// reservations, the journal's records and the facade's pending rows.
#[derive(Debug, PartialEq)]
struct Kept {
    redrives: Vec<PendingRedrive>,
    reserved: Vec<StalePendingSpend>,
    audit: Vec<([u8; 32], [u8; 32])>,
    unresolved: Vec<UnresolvedSpend>,
}

fn kept(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Kept {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    let (mut redrives, mut reserved) = engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m).await.unwrap();
        let store = coordinator.store().read().await;
        (
            store.pending_redrives(subwallet(wallet)).unwrap(),
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
        )
    });
    redrives.sort_by_key(|r| r.activity_id);
    reserved.sort();
    let mut audit = engine
        .block_on(session.leases.shielded_redrives(wallet))
        .unwrap();
    audit.sort();
    let mut unresolved = engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok(pool::ShieldedLib::unresolved(&lib).await?)
        }))
        .unwrap();
    unresolved.sort_by(|a, b| a.activity_id.cmp(&b.activity_id));
    Kept {
        redrives,
        reserved,
        audit,
        unresolved,
    }
}

/// [`Kept`] right after [`store_spends`].
fn expected() -> Kept {
    let redrives = spends();
    let mut reserved: Vec<StalePendingSpend> = (redrives.iter())
        .flat_map(|r| (r.nullifiers.iter()).map(|n| (*n, r.anchor, Some(r.activity_id))))
        .collect();
    reserved.sort();
    let hash = |st: &[u8]| -> [u8; 32] { Sha256::digest(st).into() };
    Kept {
        audit: (redrives.iter())
            .map(|r| (r.activity_id, hash(&r.st_bytes)))
            .collect(),
        unresolved: (redrives.iter())
            .map(|r| UnresolvedSpend {
                activity_id: hex::encode(r.activity_id),
                transition: hex::encode(hash(&r.st_bytes)),
                attempts: 0,
            })
            .collect(),
        reserved,
        redrives,
    }
}

/// Close Wallet and an in-app close (the network switch's) are refused
/// while a spend is unresolved, and leave everything of it as it was; so
/// does any removal but the user's. Once the scan resolves the spends,
/// both go ahead.
#[test]
fn close_wallet_and_the_network_close_are_refused_while_a_spend_is_unresolved() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    assert_eq!(kept(&engine, &session, wallet), expected());

    let unloaded = engine.block_on(session.unload_wallet(wallet));
    assert!(
        matches!(unloaded, Err(EngineError::ShieldedSpendUnresolved)),
        "{unloaded:?}"
    );
    assert_eq!(
        unloaded.unwrap_err().to_string(),
        "a shielded payment is still being confirmed; wait, or quit and reopen the app"
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    let closed = engine.block_on(engine.close_network(DashNetwork::Regtest));
    assert!(
        matches!(closed, Err(EngineError::ShieldedSpendUnresolved)),
        "{closed:?}"
    );
    assert!(
        engine
            .block_on(engine.session(&DashNetwork::Regtest))
            .is_some()
    );
    assert!(session.live().is_ok(), "the session stays open");
    // The restore's rollback takes the same refusal.
    let manager = session.manager().unwrap();
    let s = Arc::clone(&session);
    let removed = engine.block_on(async move {
        (s.platform
            .remove_from_manager(&manager, wallet, StoredSpends::Refuse))
        .await
    });
    assert!(matches!(removed, Err(EngineError::ShieldedSpendUnresolved)));
    assert_eq!(kept(&engine, &session, wallet), expected());

    // The scan finds both spent: nothing is held any more.
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m).await.unwrap();
        let mut store = coordinator.store().write().await;
        for r in spends() {
            for n in &r.nullifiers {
                store.mark_spent(subwallet(wallet), n).unwrap();
            }
            store
                .clear_redrive(subwallet(wallet), &r.activity_id)
                .unwrap();
        }
    });
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// The quitting half of the next test, run in a child process: the first
/// launch stores the spends and issues a receive address, then quits
/// (`Engine::shutdown`, the app's quit) and exits at once, so nothing is
/// dropped after the quit (the kill-after-quit check of DEC-1046 (b)).
#[test]
#[ignore = "the child process of a_quit_keeps_unresolved_spends_for_the_next_launch"]
fn quit_with_unresolved_spends_child() {
    let Some(root) = std::env::var_os(QUIT_ROOT) else {
        return;
    };
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let address = engine
        .block_on(session.next_receive_address(wallet, Some("before the quit".into())))
        .unwrap();
    std::fs::write(Path::new(&root).join("address"), &address.address).unwrap();
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
    std::process::exit(0);
}

/// DEC-1046 (a) and (b): a quit with two unresolved spends (a transfer
/// and an unshield), then a relaunch. The stored transitions are byte for
/// byte the ones signed, their nullifiers are reserved again, the journal
/// still names them by hash, the facade lists them pending, and the first
/// admitted pass hands the library those identical bytes to re-send
/// (`redrive_pending_spends`, operations.rs:3212-3257, deserializes
/// `st_bytes` and broadcasts them; this offline network never reaches
/// that call). What the quit left is durable in every store, though the
/// quitting process exited without dropping anything: the viewing key in
/// wallet.sqlite (bound again with the vault locked), the address book in
/// app.sqlite, the spends in the shielded store and their records in the
/// dispatch journal.
#[test]
fn a_quit_keeps_unresolved_spends_for_the_next_launch() {
    let dir = dw_testutil::private_tempdir();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::quit_with_unresolved_spends_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(QUIT_ROOT, dir.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the quitting process failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let address = std::fs::read_to_string(dir.path().join("address")).unwrap();

    let engine = engine_at(dir.path());
    let session = open(&engine);
    let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
        .parse()
        .unwrap();
    assert!(session.vault.lock_state() != dw_vault::LockState::Unlocked);
    assert!(bind(&engine, &session, wallet), "bound from wallet.sqlite");
    let book = engine
        .block_on(session.address_book(wallet, Some(crate::BookPurpose::Receive), None))
        .unwrap();
    assert!(book.iter().any(|e| e.address == address), "{book:?}");
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert_eq!(kept(&engine, &session, wallet), expected());

    // The first pass admitted after the relaunch carries the same bytes.
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        s.ensure_background().await;
        let coordinator = pool::admit_pass(&s, &m).await.unwrap();
        let mut stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(wallet.0)
            .unwrap();
        stored.sort_by_key(|(_, r)| r.activity_id);
        let stored: Vec<PendingRedrive> = stored.into_iter().map(|(_, r)| r).collect();
        assert_eq!(stored, spends());
    });
    assert_eq!(kept(&engine, &session, wallet), expected());
}
