//! Q86 Q-PURGE C2: the user's removal of a wallet goes through the same
//! fail-closed gate as Close Wallet (`holds_unresolved`), plus a read of the
//! wallet's `shielded_pending_spends` rows on disk for a shielded store not
//! opened this session (`shielded_pool::removal_hold`). A refused removal
//! changes nothing: the raw row, the marks, the seed, the wallet's rows and
//! its app metadata all stay, and the grant is not redeemed.

use std::path::Path;
use std::sync::Arc;

use dw_vault::{Credential, GrantKind, GrantPurpose, UnlockScope};
use platform_wallet::wallet::shielded::{ShieldedNote, ShieldedStore};
use zeroize::Zeroizing;

use super::close_unload_gate_tests::{
    ACTIVITY, ANCHOR, NULLIFIER, bind, engine_at, holds, launch, open, redrive, redrive_row,
    reservation, store_redrive, stored_rows, subwallet, with_store,
};
use super::shielded_close_tests::disposition_tests::{ForkRow, fork_dispositions, fork_rows};
use super::shielded_close_tests::{MARKED, MARKED_NULLIFIERS, mark, markers, spent_seen};
use super::shielded_pool as pool;
use super::topup_tests::{PASS, PHRASE};
use crate::{Engine, EngineError, ImportOptions, NetworkSession, ShieldedHold, WalletId};

const NAME: &str = "Kept";

fn wipe(session: &NetworkSession, wallet: WalletId) -> String {
    (session.vault)
        .authorize(
            GrantPurpose::Wipe,
            Some(&wallet.0),
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id
}

/// [`launch`], the wallet named [`NAME`] (an app metadata row).
fn named_launch(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    let (session, wallet) = launch(engine);
    engine
        .block_on(session.rename_wallet(wallet, NAME.into()))
        .unwrap();
    (session, wallet)
}

/// Quits (a quit keeps a held wallet, Q86 (e)), runs `between` on the
/// shielded store's path, and opens the network again, unlocked. The
/// shielded store is not opened.
fn relaunch(
    engine: &Engine,
    session: Arc<NetworkSession>,
    between: impl FnOnce(&Path),
) -> Arc<NetworkSession> {
    let path = session.data_dir().join(pool::SHIELDED_DB_FILE);
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    between(&path);
    let session = open(engine).expect("the reopen in this process");
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert!(!opened(engine, &session), "the shielded store opens on use");
    session
}

/// Whether the session's shielded store is open (a coordinator exists).
fn opened(engine: &Engine, session: &NetworkSession) -> bool {
    let manager = session.manager().unwrap();
    engine.block_on(manager.shielded_coordinator()).is_some()
}

/// The user's removal of `wallet` is refused with `want`, and nothing of
/// the wallet changed: still loaded, its seed in the vault, the grant not
/// redeemed, its wallet.sqlite and app.sqlite rows in place.
fn removal_refused(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    want: ShieldedHold,
    what: &str,
) {
    let grant = wipe(session, wallet);
    let leased = session.leases.background(&wallet).is_some();
    let removed = engine.block_on(session.remove_wallet(wallet, grant.clone()));
    assert!(
        matches!(&removed, Err(EngineError::ShieldedSpendHeld(h)) if *h == want),
        "{what}: {removed:?}"
    );
    assert_eq!(
        removed.unwrap_err().code(),
        "shielded_spend_unresolved",
        "{what}"
    );
    assert!(session.require_wallet(&wallet).is_ok(), "{what}: loaded");
    assert_eq!(
        session.leases.background(&wallet).is_some(),
        leased,
        "{what}: its leases were not revoked"
    );
    assert!(
        session.vault.has_wallet_secret(&wallet.0),
        "{what}: its seed stays"
    );
    assert!(
        (session.vault)
            .check_grant(&grant, GrantKind::Wipe, Some(&wallet.0))
            .is_ok(),
        "{what}: the grant is not redeemed"
    );
    let db = session.data_dir().join(crate::session::WALLET_DB_FILE);
    assert!(
        (crate::store::registered_wallet_ids(&db).unwrap()).contains(&wallet.0),
        "{what}: its wallet.sqlite rows stay"
    );
    let names = session.live().unwrap().appdb.wallet_names().unwrap();
    assert!(
        names.contains(&(wallet.to_string(), NAME.to_string())),
        "{what}: its app.sqlite rows stay"
    );
}

fn removed(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    let grant = wipe(session, wallet);
    engine
        .block_on(session.remove_wallet(wallet, grant))
        .unwrap();
    assert!(session.require_wallet(&wallet).is_err(), "removed");
    assert!(!session.vault.has_wallet_secret(&wallet.0), "its seed gone");
}

fn hold(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Option<ShieldedHold> {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move { pool::removal_hold(&s, &m, wallet).await })
}

/// One stored redrive refuses the removal (in flight); the raw row is on
/// disk as stored, and the wallet keeps everything.
#[test]
fn a_stored_redrive_refuses_the_removal_and_the_raw_row_survives() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    engine.block_on(session.ensure_background());
    assert!(
        session.leases.background(&wallet).is_some(),
        "a background lease to keep"
    );

    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::InFlight,
        "a stored redrive",
    );
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    let redrives = with_store(&engine, &session, move |store| {
        (store.pending_redrives(subwallet(wallet)).unwrap())
            .into_iter()
            .map(|r| r.activity_id)
            .collect::<Vec<_>>()
    });
    assert_eq!(redrives, [ACTIVITY], "and its record in the library");
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
}

/// Each trigger alone refuses the removal: a reserved note with no record
/// (in flight), a marker the journal cannot read, a DEC-1057 marker, a
/// fork-held mark and a shielded store that does not read (carried over).
/// Between them nothing holds.
#[test]
fn each_trigger_alone_refuses_the_removal() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    assert_eq!(hold(&engine, &session, wallet), None);

    with_store(&engine, &session, move |store| {
        assert!(store.mark_pending(subwallet(wallet), &NULLIFIER).unwrap());
        (store.set_pending_spend(
            subwallet(wallet),
            &NULLIFIER,
            ANCHOR,
            ACTIVITY,
            Some(super::shielded_close_tests::ANCHOR_HEIGHT),
        ))
        .unwrap();
        assert!(
            (store.pending_redrives(subwallet(wallet)).unwrap()).is_empty(),
            "no record"
        );
        assert_eq!(
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
            [reservation()]
        );
    });
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::InFlight,
        "a reserved note",
    );
    with_store(&engine, &session, move |store| {
        assert!(store.clear_pending(subwallet(wallet), &NULLIFIER).unwrap());
    });
    assert_eq!(hold(&engine, &session, wallet), None);

    let journal = rusqlite::Connection::open(
        session
            .data_dir()
            .join(dw_appdb::dispatch::DISPATCH_DB_FILE),
    )
    .unwrap();
    journal
        .execute_batch("ALTER TABLE shielded_unrecorded RENAME TO shielded_unrecorded_away")
        .unwrap();
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "an unreadable marker",
    );
    journal
        .execute_batch("ALTER TABLE shielded_unrecorded_away RENAME TO shielded_unrecorded")
        .unwrap();
    drop(journal);
    assert_eq!(hold(&engine, &session, wallet), None);

    pool::FAIL_DISK_READ_IN
        .lock()
        .unwrap()
        .push(session.data_dir().to_path_buf());
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "a shielded store that does not read",
    );
    assert_eq!(hold(&engine, &session, wallet), None);

    mark(&engine, &session, wallet);
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "a DEC-1057 marker",
    );
    assert_eq!(markers(&engine, &session, wallet), [MARKED]);
    spent_seen(&session, wallet, &MARKED_NULLIFIERS);
    assert_eq!(hold(&engine, &session, wallet), None);

    (session.leases).hold_fork_wallets(Some([wallet].into()));
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "a fork-held mark",
    );
    assert!(engine.block_on(session.leases.marks_unrecorded(wallet)));
    engine.block_on(engine.shutdown()).unwrap();
}

/// A row of the wallet on disk that the open store did not load (as the
/// library keeps a damaged record it never re-sends): the close's gate does
/// not see it, the removal's does (carried over, a confirmation does not
/// clear it), and the row survives.
#[test]
fn a_row_on_disk_the_open_store_did_not_load_refuses_the_removal() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    assert!(opened(&engine, &session));
    let r = redrive();
    rusqlite::Connection::open(data_dir.join(pool::SHIELDED_DB_FILE))
        .unwrap()
        .execute(
            "INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, \
             anchor, nullifiers, st_bytes, attempts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            rusqlite::params![
                &wallet.0[..],
                dw_vault::SHIELDED_ACCOUNT,
                &r.activity_id[..],
                &r.anchor[..],
                r.nullifiers.concat(),
                r.st_bytes,
            ],
        )
        .unwrap();
    assert!(!holds(&engine, &session, wallet), "not loaded");

    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "a row the store did not load",
    );
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    engine.block_on(engine.shutdown()).unwrap();
}

/// The fork's disposition rows of `wallet` as the int-4 upgrade carries
/// them: one readable (a DEC-1057 marker at the first open), or, with
/// `unreadable`, one whose nullifiers do not read (a fork-held mark).
fn fork_era(path: &Path, wallet: WalletId, unreadable: bool) {
    if !unreadable {
        let rows: [ForkRow; 1] = [(1, "unresolved", Some("cancelled"), vec![[61; 32]], 0)];
        fork_dispositions(path, wallet, &rows);
        return;
    }
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE shielded_spend_dispositions (wallet_id BLOB, account_index INTEGER, \
         st_hash BLOB, activity_id BLOB, disposition TEXT, reason TEXT, at INTEGER, \
         nullifiers BLOB, released_by_scan INTEGER)",
    )
    .unwrap();
    db.execute(
        "INSERT INTO shielded_spend_dispositions VALUES \
         (?1, ?2, ?3, ?3, 'unresolved', 'cancelled', 1, ?4, 0)",
        rusqlite::params![
            &wallet.0[..],
            dw_vault::SHIELDED_ACCOUNT,
            &[2_u8; 32][..],
            &[62_u8; 33][..]
        ],
    )
    .unwrap();
}

/// A fork-era pending spend (its `shielded_pending_spends` row) with the
/// mark its disposition gets at the first open (`import_fork_holds`): a
/// DEC-1057 marker, or a fork-held mark when the row does not read. Left
/// undrained (no copy from an upgrade), upstream's store stays closed this
/// session. The removal is refused (carried over), and the raw row, the
/// mark and the fork's rows all survive. The drained case, upstream's store
/// open: `store_drain_tests::a_drained_fork_spend_and_its_stored_row_refuse_the_removal`.
#[test]
fn a_fork_era_pending_spend_and_its_mark_refuse_the_removal_and_survive() {
    for unreadable in [false, true] {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = named_launch(&engine);
        let data_dir = session.data_dir().to_path_buf();
        store_redrive(&engine, &session, wallet);
        let mut fork = Vec::new();
        let session = relaunch(&engine, session, |path| {
            fork_era(path, wallet, unreadable);
            fork = fork_rows(path);
        });
        let marked = markers(&engine, &session, wallet);
        assert_eq!(marked, if unreadable { vec![] } else { vec![[1; 32]] });
        assert!(engine.block_on(session.leases.marks_unrecorded(wallet)));
        assert!(
            pool::drain_verdict(&session).is_err(),
            "undrained (no upgrade copy): upstream's store stays closed"
        );
        assert!(!opened(&engine, &session));
        assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

        removal_refused(
            &engine,
            &session,
            wallet,
            ShieldedHold::CarriedOver,
            if unreadable { "fork-held" } else { "marked" },
        );
        assert_eq!(stored_rows(&data_dir, wallet), redrive_row(), "the raw row");
        assert_eq!(markers(&engine, &session, wallet), marked, "the marker");
        assert!(
            engine.block_on(session.leases.marks_unrecorded(wallet)),
            "the mark"
        );
        assert_eq!(fork_rows(&data_dir.join(pool::SHIELDED_DB_FILE)), fork);
        engine.block_on(engine.shutdown()).unwrap();
        assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
    }
}

/// The shielded store not opened this session: the gate Close Wallet uses
/// sees no stored spend there, yet a row on disk refuses the removal, which
/// opens nothing. A re-import is refused (the wallet stayed), and once the
/// store opens the row's reservation holds its note: nothing it covers is
/// offered as spendable.
#[test]
fn a_row_in_a_shielded_store_not_opened_this_session_refuses_the_removal() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    let session = relaunch(&engine, session, |_| {});
    assert!(
        !holds(&engine, &session, wallet),
        "no coordinator: the close's gate sees no stored spend"
    );

    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::InFlight,
        "a row on disk",
    );
    assert!(!opened(&engine, &session), "the gate opens no store");
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());

    let imported = engine.block_on(session.import_wallet(
        Zeroizing::new(PHRASE.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(0),
            core_compat: false,
            ..Default::default()
        },
    ));
    assert!(
        matches!(imported, Err(EngineError::WalletAlreadyExists(_))),
        "{imported:?}"
    );

    assert!(bind(&engine, &session, wallet), "the store opens");
    let note = ShieldedNote {
        position: 7,
        cmx: [0x62; 32],
        nullifier: NULLIFIER,
        block_height: 1,
        is_spent: false,
        value: 1_000,
        note_data: Vec::new(),
    };
    let (redrives, reserved, unspent, spendable) = with_store(&engine, &session, move |store| {
        store.save_note(subwallet(wallet), &note).unwrap();
        (
            store.pending_redrives(subwallet(wallet)).unwrap(),
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
            store.get_unspent_notes(subwallet(wallet)).unwrap(),
            store.spendable_balance(subwallet(wallet)).unwrap(),
        )
    });
    assert_eq!(redrives, [redrive()]);
    assert_eq!(reserved, [reservation()]);
    assert!(unspent.is_empty(), "{unspent:?}");
    assert_eq!(spendable, 0);
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::InFlight,
        "the store open",
    );
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(stored_rows(&data_dir, wallet), redrive_row());
}

/// The way out: after a refusal the wallet is used as before, and once its
/// hold clears (the stored spend resolved; the marked spend's notes all
/// seen spent by a proved scan) the removal goes through.
#[test]
fn a_refused_wallet_stays_usable_and_is_removed_once_its_hold_clears() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    store_redrive(&engine, &session, wallet);
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::InFlight,
        "a stored redrive",
    );
    engine
        .block_on(session.rename_wallet(wallet, "Still here".into()))
        .unwrap();
    engine
        .block_on(session.rename_wallet(wallet, NAME.into()))
        .unwrap();
    assert!(
        session
            .wallet_infos()
            .unwrap()
            .iter()
            .any(|w| w.wallet_id == wallet)
    );
    assert!(
        bind(&engine, &session, wallet),
        "its shielded account answers"
    );

    // As the library does once the spend resolves.
    with_store(&engine, &session, move |store| {
        store.clear_redrive(subwallet(wallet), &ACTIVITY).unwrap();
        store.clear_pending(subwallet(wallet), &NULLIFIER).unwrap();
    });
    assert!(stored_rows(&data_dir, wallet).is_empty());
    mark(&engine, &session, wallet);
    removal_refused(
        &engine,
        &session,
        wallet,
        ShieldedHold::CarriedOver,
        "a DEC-1057 marker",
    );
    spent_seen(&session, wallet, &MARKED_NULLIFIERS);
    assert!(markers(&engine, &session, wallet).is_empty());

    removed(&engine, &session, wallet);
    engine.block_on(engine.shutdown()).unwrap();
}

/// Nothing held, the shielded store open or not: the removal goes through
/// as before.
#[test]
fn with_nothing_held_the_removal_goes_through() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = named_launch(&engine);
    assert!(opened(&engine, &session));
    removed(&engine, &session, wallet);

    // Another seed: the removal's pre-delete backup is named by wallet id
    // and second.
    let imported = engine.block_on(session.import_wallet(
        Zeroizing::new(b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong".to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions::default(),
    ));
    let other = imported.unwrap();
    assert_ne!(other, wallet);
    let session = relaunch(&engine, session, |_| {});
    removed(&engine, &session, other);
    engine.block_on(engine.shutdown()).unwrap();
}
