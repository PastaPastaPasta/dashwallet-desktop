//! Q86 at a pin carrying the persister release (P16, platform #6270). The
//! close, the network close, Close Wallet and a restore's rollback reach
//! `remove_wallet` only through the fail-closed Refuse gate (Q86 (c)), whose
//! purge at this pin deletes a wallet's `shielded_pending_spends` rows
//! outright. A wallet the gate keeps stays loaded through the quit, and P16
//! still releases the wallet database (Q86 (b), (e)).

use std::path::Path;
use std::sync::{Arc, Weak};

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};
use platform_wallet::wallet::shielded::store::{PendingRedrive, StalePendingSpend};
use platform_wallet::wallet::shielded::{FileBackedShieldedStore, ShieldedStore, SubwalletId};
use platform_wallet_storage::SqlitePersister;
use zeroize::Zeroizing;

use super::shielded_close_tests::{ANCHOR_HEIGHT, reserve_and_record};
use super::shielded_pool::{self as pool, on_pool};
use super::topup_tests::{PASS, PHRASE, options};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

pub(super) const ACTIVITY: [u8; 32] = [0x51; 32];
pub(super) const ANCHOR: [u8; 32] = [0xa5; 32];
pub(super) const NULLIFIER: [u8; 32] = [0x15; 32];

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

pub(super) fn engine_at(root: &Path) -> Engine {
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

pub(super) fn open(engine: &Engine) -> Result<Arc<NetworkSession>, EngineError> {
    engine.block_on(engine.open_network(DashNetwork::Regtest, options()))
}

/// A new vault and the "abandon … about" wallet, its viewing key bound.
pub(super) fn launch(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    let session = open(engine).unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    engine
        .block_on(session.adopt_signing_window_default())
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

pub(super) fn bind(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> bool {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok(pool::ShieldedLib::bind(&lib).await?.is_bound())
        }))
        .unwrap()
}

pub(super) fn subwallet(wallet: WalletId) -> SubwalletId {
    SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT)
}

/// One shielded transfer of unknown result, as the library stores it.
pub(super) fn redrive() -> PendingRedrive {
    PendingRedrive {
        activity_id: ACTIVITY,
        st_bytes: transfer(),
        anchor: ANCHOR,
        nullifiers: vec![NULLIFIER],
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
        anchor_height: Some(ANCHOR_HEIGHT),
    }
}

fn transfer() -> Vec<u8> {
    use dpp::serialization::PlatformSerializable;
    use dpp::shielded::SerializedAction;
    use dpp::state_transition::StateTransition;
    use dpp::state_transition::shielded_transfer_transition::ShieldedTransferTransition;
    use dpp::state_transition::shielded_transfer_transition::v0::ShieldedTransferTransitionV0;
    let st = StateTransition::ShieldedTransfer(ShieldedTransferTransition::V0(
        ShieldedTransferTransitionV0 {
            actions: vec![SerializedAction {
                nullifier: NULLIFIER,
                rk: [0x61; 32],
                cmx: [0x62; 32],
                encrypted_note: vec![0x63; 216],
                cv_net: [0x64; 32],
                spend_auth_sig: [0x65; 64],
            }],
            value_balance: 1_000,
            anchor: ANCHOR,
            proof: vec![0x66; 512],
            binding_signature: [0x67; 64],
        },
    ));
    st.serialize_to_bytes().unwrap()
}

/// The reservation [`redrive`] holds.
pub(super) fn reservation() -> StalePendingSpend {
    (NULLIFIER, ANCHOR, Some(ACTIVITY), Some(ANCHOR_HEIGHT))
}

pub(super) fn with_store<T: Send + 'static>(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    f: impl FnOnce(&mut FileBackedShieldedStore) -> T + Send + 'static,
) -> T {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        f(&mut store)
    })
}

/// Stores [`redrive`] for `wallet` as the library's spend call does.
pub(super) fn store_redrive(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    with_store(engine, session, move |store| {
        reserve_and_record(store, subwallet(wallet), redrive());
    });
}

/// `wallet`'s rows of `shielded_pending_spends` on disk: activity, anchor,
/// nullifiers, transition, attempts.
pub(super) fn stored_rows(data_dir: &Path, wallet: WalletId) -> Vec<StoredRow> {
    let db = rusqlite::Connection::open_with_flags(
        data_dir.join(pool::SHIELDED_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut rows = db
        .prepare(
            "SELECT activity_id, anchor, nullifiers, st_bytes, attempts \
             FROM shielded_pending_spends WHERE wallet_id = ?1 ORDER BY activity_id",
        )
        .unwrap();
    rows.query_map([&wallet.0[..]], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

pub(super) type StoredRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, i64);

/// The row [`store_redrive`] writes.
pub(super) fn redrive_row() -> Vec<StoredRow> {
    let r = redrive();
    vec![(
        r.activity_id.to_vec(),
        r.anchor.to_vec(),
        r.nullifiers.concat(),
        r.st_bytes,
        0,
    )]
}

fn refused<T: std::fmt::Debug>(result: Result<T, EngineError>, what: &str) {
    assert!(
        matches!(result, Err(EngineError::ShieldedSpendUnresolved)),
        "{what}: {result:?}"
    );
}

/// Close Wallet (its own check first, then the gate's) and the restore's
/// rollback (the gate alone) both refuse `wallet`.
fn both_refuse(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId, what: &str) {
    refused(
        engine.block_on(session.unload_wallet(wallet)),
        &format!("Close Wallet, {what}"),
    );
    refused(
        engine.block_on(session.remove_registered(wallet)),
        &format!("the restore's rollback, {what}"),
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
}

pub(super) fn holds(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> bool {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move { pool::holds_unresolved(&s.leases, &m, wallet).await })
}

fn persister(session: &NetworkSession) -> Weak<SqlitePersister> {
    Arc::downgrade(&session.live().unwrap().persister)
}

/// Q86 (c): one stored redrive refuses Close Wallet, the network close and
/// a restore's rollback, and the close (the quit) keeps the wallet loaded;
/// after each the row is on disk as it was stored.
#[test]
fn one_stored_redrive_refuses_every_gated_unload_and_its_row_survives() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    refused(
        engine.block_on(session.unload_wallet(wallet)),
        "Close Wallet",
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    let closed = engine.block_on(engine.close_network(DashNetwork::Regtest));
    refused(closed, "the network close");
    assert!(session.live().is_ok(), "the session stays open");
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    refused(
        engine.block_on(session.remove_registered(wallet)),
        "the restore's rollback",
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    engine.block_on(engine.shutdown()).unwrap();
    assert!(session.live().is_err(), "closed");
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
}

/// Q86 (c): the gate also refuses on a reserved note with no record
/// (`stale_pending_spends`), on a marker the journal cannot read and on a
/// DEC-1057 marker, each alone, on Close Wallet and the restore's rollback.
#[test]
fn a_reserved_note_an_unreadable_marker_and_a_marker_each_refuse_the_unload() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    assert!(!holds(&engine, &session, wallet));

    with_store(&engine, &session, move |store| {
        assert!(store.mark_pending(subwallet(wallet), &NULLIFIER).unwrap());
        (store.set_pending_spend(
            subwallet(wallet),
            &NULLIFIER,
            ANCHOR,
            ACTIVITY,
            Some(ANCHOR_HEIGHT),
        ))
        .unwrap();
        assert!(
            store
                .pending_redrives(subwallet(wallet))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
            [reservation()]
        );
    });
    both_refuse(&engine, &session, wallet, "a reserved note");
    with_store(&engine, &session, move |store| {
        assert!(store.clear_pending(subwallet(wallet), &NULLIFIER).unwrap());
    });
    assert!(!holds(&engine, &session, wallet));

    let journal = rusqlite::Connection::open(
        session
            .data_dir()
            .join(dw_appdb::dispatch::DISPATCH_DB_FILE),
    )
    .unwrap();
    journal
        .execute_batch("ALTER TABLE shielded_unrecorded RENAME TO shielded_unrecorded_away")
        .unwrap();
    both_refuse(&engine, &session, wallet, "an unreadable marker");
    journal
        .execute_batch("ALTER TABLE shielded_unrecorded_away RENAME TO shielded_unrecorded")
        .unwrap();
    drop(journal);
    assert!(!holds(&engine, &session, wallet));

    engine
        .block_on(
            session
                .leases
                .record_unrecorded_spend(wallet, ACTIVITY, ANCHOR, vec![NULLIFIER]),
        )
        .unwrap();
    both_refuse(&engine, &session, wallet, "a marker");
    engine.block_on(engine.shutdown()).unwrap();
}

/// Q86 (e): a quit with a stored redrive keeps its wallet loaded, and the
/// wallet database is released all the same (P16): the network reopens in
/// this process, and the row and its reservation are back as stored.
#[test]
fn a_quit_with_a_stored_redrive_releases_the_database_and_the_reopen_keeps_it() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    let released = persister(&session);

    engine.block_on(engine.shutdown()).unwrap();
    assert!(
        released.upgrade().is_none(),
        "the wallet database is released"
    );
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    let session = open(&engine).expect("the reopen in this process");
    assert!(bind(&engine, &session, wallet), "bound from wallet.sqlite");
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    let (redrives, reserved) = with_store(&engine, &session, move |store| {
        (
            store.pending_redrives(subwallet(wallet)).unwrap(),
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
        )
    });
    assert_eq!(redrives, [redrive()]);
    assert_eq!(reserved, [reservation()]);
    assert!(holds(&engine, &session, wallet));
    refused(
        engine.block_on(session.unload_wallet(wallet)),
        "Close Wallet after the reopen",
    );
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    engine.block_on(engine.shutdown()).unwrap();
}

/// Q86 (b) guard: a close that keeps a wallet loaded (here a DEC-1057
/// marker holds it) leaves only the persister release (P16, platform #6270)
/// to free the wallet database. A pin without P16 fails here: the close and
/// every failed-open arm, which unload nothing, would hold the database
/// until the process exits.
#[test]
fn p16_a_close_keeping_a_wallet_loaded_releases_the_wallet_database() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    engine
        .block_on(
            session
                .leases
                .record_unrecorded_spend(wallet, ACTIVITY, ANCHOR, vec![NULLIFIER]),
        )
        .unwrap();
    let released = persister(&session);
    engine.block_on(engine.shutdown()).unwrap();
    assert!(
        released.upgrade().is_none(),
        "a pin without P16 keeps the wallet database held"
    );
    let reopened = open(&engine).expect("the reopen in this process");
    assert!(reopened.require_wallet(&wallet).is_ok());
    engine.block_on(engine.shutdown()).unwrap();
}

/// Q86 F1: a failed journal open, after a quit that kept a wallet with a
/// stored redrive, configures no shielded store (the probe, and the
/// failed-open end's debug assertion) and leaves the `shielded.sqlite` rows
/// as they were: a removal there with a store configured would delete them.
#[test]
fn a_failed_journal_open_leaves_the_stored_redrives_unchanged() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    let journal = data_dir.join(dw_appdb::dispatch::DISPATCH_DB_FILE);
    rusqlite::Connection::open(&journal)
        .unwrap()
        .execute(
            "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, status, at) \
             VALUES (?1, ?2, 'imported_redrive', ?3, 'unclassified', 1)",
            rusqlite::params![[0x5a_u8; 32], [0x77_u8; 32], [0x88_u8; 32]],
        )
        .unwrap();
    dw_appdb::fault::arm_fault(&journal, dw_appdb::fault::Fault::ClassifyWrite);
    let failed = open(&engine).err().expect("the journal read fails once");
    assert!(
        matches!(&failed, EngineError::Storage(e) if e.contains("imported shielded spends")),
        "{failed:?}"
    );
    assert_eq!(
        crate::session::failed_open_probe::seen(&data_dir),
        [(false, 1)]
    );
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    engine.block_on(engine.shutdown()).unwrap();
}
