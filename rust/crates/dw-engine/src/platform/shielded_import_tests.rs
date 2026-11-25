//! The release of a spend the 0.1 build stored (an imported record): the
//! row it wrote in `shielded_pending_spends`, under the fork's schema, with
//! the anchor the spend was built on and no anchor height (the column is
//! upstream's), over the commitment tree the 0.1 build grew in the same
//! file. The engine never re-sends it (`EngineDispatch::admit` refuses an
//! imported record), so its notes come free only through the library's own
//! pass: the prune release judges an anchor named without its height
//! against the largest tree the store held when upstream first opened it
//! (`legacy_anchor_tree_bound`), or the scan finds the notes spent.
//!
//! Each test opens the 0.1 file as the engine does (`configure_shielded`:
//! the store over the file, the coordinator over the store) with a mock
//! SDK, and runs one pass. The note is saved in the store before the pass,
//! as the pass's rescan finds it at this pin (the wallet's store keeps no
//! notes across a restart).
//!
//! The library keeps its history rows in memory only, so the imported
//! spend has none after the upgrade. The engine saves it a pending one at
//! the store's open and before each pass (`shielded_pool::keep_import_row`,
//! for the journal's open artifact of the spend), which the release fails
//! or the engine confirms once the scan finds its notes spent; the journal
//! reads the outcome from it (`shielded_pool::spend_outcome`). Once the
//! journal settled it, the engine saves the row its settlement reads after
//! each restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dash_sdk::query_types::{
    NoParamQuery, ShieldedAnchors, ShieldedEncryptedNote, ShieldedEncryptedNotes,
    ShieldedEncryptedNotesQuery, ShieldedNotesCount,
};
use dw_appdb::dispatch::{ArtifactOrigin, ArtifactStatus, ShieldedArtifact};
use platform_wallet::WalletPersister;
use platform_wallet::wallet::persister::NoPlatformPersistence;
use platform_wallet::wallet::shielded::{
    FileBackedShieldedStore, NetworkShieldedCoordinator, OrchardKeySet, ShieldedNote,
    ShieldedStore, SubwalletId,
};
use platform_wallet::wallet::shielded::{ShieldedActivityEntry, ShieldedActivityStatus};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::shielded_pool::{SpendOutcome, keep_import_row, spend_outcome};

/// The commitment tree the 0.1 build held: its spend's anchor is a root of
/// it, so no larger.
const TREE: u64 = 4;
/// The height Platform's anchor set, the pool's count and the scan's
/// chunks are proved at.
const H: u64 = 50;
const WALLET: [u8; 32] = [0x11; 32];
const ANCHOR: [u8; 32] = [0x62; 32];
const ACTIVITY: [u8; 32] = [0x63; 32];
const NULLIFIER: [u8; 32] = [0x61; 32];
/// The spent note's position in the tree: below [`TREE`].
const POSITION: u64 = 2;
/// The 0.1 build's signed transition of the spend.
const ST: &[u8] = &[0xDE, 0xAD];
/// When the 0.1 build stored it, in seconds.
const AT: u64 = 1_700_000_000;

fn subwallet() -> SubwalletId {
    SubwalletId::new(WALLET, dw_vault::SHIELDED_ACCOUNT)
}

/// A shielded store as the 0.1 build left it at `path`: a tree of [`TREE`]
/// leaves, and its record of a spend of [`NULLIFIER`] on [`ANCHOR`] in the
/// fork's `shielded_pending_spends` (no `anchor_height`), written
/// `armed_recording` and not `unsent`. None of upstream's own tables.
fn zero_one_store(path: &Path) {
    {
        let mut store = FileBackedShieldedStore::open_path(path, 100).unwrap();
        for leaf in 0..TREE {
            store.append_commitment(&cmx(leaf), true).unwrap();
        }
    }
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "DROP TABLE shielded_pending_spends;
         DROP TABLE shielded_store_meta;
         DROP TABLE shielded_private_payment_journal;
         DROP TABLE shielded_private_payment_journal_wallets;
         CREATE TABLE shielded_pending_spends (
            wallet_id     BLOB    NOT NULL,
            account_index INTEGER NOT NULL,
            activity_id   BLOB    NOT NULL,
            anchor        BLOB    NOT NULL,
            nullifiers    BLOB    NOT NULL,
            st_bytes      BLOB    NOT NULL,
            attempts      INTEGER NOT NULL DEFAULT 0,
            identity_nonce_finalized INTEGER NOT NULL DEFAULT 0,
            identity_user_abandoned INTEGER NOT NULL DEFAULT 0,
            unsent        INTEGER NOT NULL DEFAULT 0,
            armed_recording INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (wallet_id, account_index, activity_id)
         );",
    )
    .unwrap();
    db.execute(
        "INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, anchor, \
         nullifiers, st_bytes, attempts, unsent, armed_recording) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 0, 1)",
        rusqlite::params![
            &WALLET[..],
            dw_vault::SHIELDED_ACCOUNT,
            &ACTIVITY[..],
            &ANCHOR[..],
            &NULLIFIER[..],
            ST,
        ],
    )
    .unwrap();
}

/// A commitment the pool holds at `position`: a small value, a valid Pallas
/// element.
fn cmx(position: u64) -> [u8; 32] {
    let mut cmx = [0u8; 32];
    cmx[0] = position as u8 + 1;
    cmx
}

/// A mock SDK whose pass proves, at [`H`], an anchor set without
/// [`ANCHOR`], a pool of `pool` leaves, and a pool whose notes are
/// [`TREE`] of nobody's, then those of `spent` (each nullifier the spend of
/// an action at the next position).
async fn network(pool: u64, spent: &[[u8; 32]]) -> dash_sdk::Sdk {
    let mut sdk = dash_sdk::Sdk::new_mock();
    let count = 2048
        * u32::from(
            sdk.version()
                .drive_abci
                .query
                .shielded_queries
                .max_query_chunks,
        );
    let nullifiers = std::iter::repeat_n([0xAA; 32], TREE as usize).chain(spent.iter().copied());
    let notes: Vec<ShieldedEncryptedNote> = nullifiers
        .enumerate()
        .map(|(position, nullifier)| ShieldedEncryptedNote {
            cmx: cmx(position as u64).to_vec(),
            nullifier: nullifier.to_vec(),
            cv_net: vec![],
            encrypted_note: vec![],
        })
        .collect();
    let total_count = notes.len() as u64;
    for chunk in 0..16 {
        let notes = if chunk == 0 {
            notes.clone()
        } else {
            Vec::new()
        };
        sdk.mock()
            .expect_fetch(
                ShieldedEncryptedNotesQuery {
                    start_index: u64::from(count) * chunk,
                    count,
                },
                Some(ShieldedEncryptedNotes { notes, total_count }),
            )
            .await
            .unwrap();
    }
    sdk.mock()
        .expect_fetch(NoParamQuery {}, Some(ShieldedAnchors(vec![[0xEE; 32]])))
        .await
        .unwrap();
    sdk.mock()
        .expect_fetch(NoParamQuery {}, Some(ShieldedNotesCount(pool)))
        .await
        .unwrap();
    sdk.mock()
        .set_response_height_of::<ShieldedAnchors>(H)
        .set_response_height_of::<ShieldedNotesCount>(H)
        .set_response_height_of::<ShieldedEncryptedNotes>(H);
    sdk
}

/// The 0.1 file at `path` opened as the engine opens it, over `sdk`, with
/// the wallet's account registered and its spent note saved.
async fn open(path: PathBuf, sdk: dash_sdk::Sdk) -> NetworkShieldedCoordinator {
    let store = FileBackedShieldedStore::open_path(&path, 100).unwrap();
    let coordinator =
        NetworkShieldedCoordinator::new(Arc::new(sdk), dashcore::Network::Testnet, path, store);
    let views = OrchardKeySet::from_seed(&[0x42; 64], dashcore::Network::Testnet, 0)
        .unwrap()
        .viewing_keys();
    coordinator
        .register_wallet(
            WALLET,
            BTreeMap::from([(dw_vault::SHIELDED_ACCOUNT, views)]),
            WalletPersister::new(WALLET, Arc::new(NoPlatformPersistence)),
        )
        .await
        .unwrap();
    let note = ShieldedNote {
        position: POSITION,
        cmx: cmx(POSITION),
        nullifier: NULLIFIER,
        block_height: 1,
        is_spent: false,
        value: 1_000,
        note_data: vec![0; 115],
    };
    coordinator
        .store()
        .write()
        .await
        .save_note(subwallet(), &note)
        .unwrap();
    coordinator
}

/// What the library holds of the imported spend: its record, its note's
/// reservation (with the anchor and no height), the note spendable, the
/// note read spent, and the record's row on disk.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    record: bool,
    reservation: Option<([u8; 32], Option<u64>)>,
    spendable: bool,
    spent: bool,
    row: bool,
}

async fn held(coordinator: &NetworkShieldedCoordinator, path: &Path) -> Held {
    let store = coordinator.store().read().await;
    let id = subwallet();
    let note = (store.get_all_notes(id).unwrap().into_iter())
        .find(|n| n.nullifier == NULLIFIER)
        .unwrap();
    let db = rusqlite::Connection::open(path).unwrap();
    let rows: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shielded_pending_spends WHERE activity_id = ?1",
            [&ACTIVITY[..]],
            |r| r.get(0),
        )
        .unwrap();
    Held {
        record: (store.pending_redrives(id).unwrap().iter()).any(|r| r.activity_id == ACTIVITY),
        reservation: (store.stale_pending_spends(id).unwrap().into_iter())
            .find(|(n, ..)| *n == NULLIFIER)
            .map(|(_, anchor, _, height)| (anchor, height)),
        spendable: (store.get_unspent_notes(id).unwrap().iter()).any(|n| n.nullifier == NULLIFIER),
        spent: note.is_spent,
        row: rows == 1,
    }
}

/// Opened, the imported record is a reservation on its anchor with no
/// height, its note held.
const IMPORTED: Held = Held {
    record: true,
    reservation: Some((ANCHOR, None)),
    spendable: false,
    spent: false,
    row: true,
};

/// An imported record that never executed: once the pool is proved, at or
/// below the anchor set's height, to hold the tree the 0.1 build held, and
/// the set lacks its anchor with the scan covering the set, the pass
/// releases its note and drops its record, on disk too. One leaf short of
/// that tree, the anchor may be newer than the set: it stays held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imported_record_that_never_executed_is_released_by_the_prune() {
    for (pool, released) in [(TREE - 1, false), (TREE, true)] {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("shielded.sqlite");
        zero_one_store(&path);
        let coordinator = open(path.clone(), network(pool, &[]).await).await;
        assert_eq!(held(&coordinator, &path).await, IMPORTED, "pool {pool}");

        coordinator.sync(true, &CancellationToken::new()).await;
        let want = if released {
            Held {
                record: false,
                reservation: None,
                spendable: true,
                spent: false,
                row: false,
            }
        } else {
            IMPORTED
        };
        assert_eq!(held(&coordinator, &path).await, want, "pool {pool}");
    }
}

/// An imported record that executed: the scan finds its note spent, which
/// drops its record and reservation; the note is not spendable again, even
/// with the pool short of the tree (no prune release could explain it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imported_record_that_executed_is_marked_spent_by_the_scan() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("shielded.sqlite");
    zero_one_store(&path);
    let coordinator = open(path.clone(), network(TREE - 1, &[NULLIFIER]).await).await;
    assert_eq!(held(&coordinator, &path).await, IMPORTED);

    coordinator.sync(true, &CancellationToken::new()).await;
    assert_eq!(
        held(&coordinator, &path).await,
        Held {
            record: false,
            reservation: None,
            spendable: false,
            spent: true,
            row: false,
        }
    );
}

/// The journal's artifact of the imported spend (schema 11, as the 0.1
/// journal's row of it imports), read with `status`.
fn artifact(status: ArtifactStatus) -> ShieldedArtifact {
    ShieldedArtifact {
        activity: ACTIVITY,
        origin: ArtifactOrigin::ImportedRedrive,
        st_hash: Some(Sha256::digest(ST).into()),
        kind: None,
        anchor: None,
        identity_id: None,
        status,
        disposition: None,
        acked: false,
        at: AT,
        selected: Vec::new(),
        published: Vec::new(),
    }
}

/// Classified, as the session's load leaves an imported spend.
const OPEN: ArtifactStatus = ArtifactStatus::Unresolved { unrecorded: false };

/// Whether the engine saved the imported spend a row for `artifact`.
async fn keep(coordinator: &NetworkShieldedCoordinator, artifact: &ShieldedArtifact) -> bool {
    let mut store = coordinator.store().write().await;
    keep_import_row(&mut *store, subwallet(), artifact).unwrap()
}

/// The engine's reading of the open artifact at a pass's end: the spend's
/// history rows (all of the account's with its activity) and the outcome
/// the journal settles it with.
async fn read(
    coordinator: &NetworkShieldedCoordinator,
) -> (Vec<ShieldedActivityEntry>, Option<SpendOutcome>) {
    let outcome = {
        let mut store = coordinator.store().write().await;
        spend_outcome(&mut *store, subwallet(), &artifact(OPEN)).unwrap()
    };
    (rows(coordinator).await, outcome)
}

/// The spend's history rows: all of the account's with its activity.
async fn rows(coordinator: &NetworkShieldedCoordinator) -> Vec<ShieldedActivityEntry> {
    let store = coordinator.store().read().await;
    (store
        .get_activity(subwallet(), 0, usize::MAX)
        .unwrap()
        .into_iter())
    .filter(|e| e.id == ACTIVITY)
    .collect()
}

/// The row the engine saves the imported spend: pending, dated when the
/// 0.1 build stored it, the notes it took named.
fn row(status: ShieldedActivityStatus) -> ShieldedActivityEntry {
    ShieldedActivityEntry {
        id: ACTIVITY,
        kind: platform_wallet::wallet::shielded::ShieldedActivityKind::ShieldedSpend,
        direction: platform_wallet::wallet::shielded::ShieldedDirection::Out,
        amount: 0,
        fee: None,
        counterparty: None,
        memo: None,
        block_height: None,
        status,
        created_at_ms: AT * 1000,
        min_note_position: None,
        note_cmxs: Vec::new(),
        spent_nullifiers: vec![NULLIFIER],
    }
}

/// An imported spend that never executed keeps its history row: pending
/// while its note is held, then failed by the release, which settles its
/// artifact `not_executed`. Nothing of it is left to re-send, and its note
/// is spendable: the user can pay again, as after any payment not sent. A
/// spend still held waits, pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_released_import_reads_failed_and_settles_not_executed() {
    for (pool, released) in [(TREE - 1, false), (TREE, true)] {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("shielded.sqlite");
        zero_one_store(&path);
        let coordinator = open(path.clone(), network(pool, &[]).await).await;
        assert!(keep(&coordinator, &artifact(OPEN)).await, "pool {pool}");
        assert_eq!(
            read(&coordinator).await,
            (vec![row(ShieldedActivityStatus::Pending)], None),
            "pool {pool}"
        );

        coordinator.sync(true, &CancellationToken::new()).await;
        let (rows, outcome) = read(&coordinator).await;
        if !released {
            assert_eq!(rows, [row(ShieldedActivityStatus::Pending)], "pool {pool}");
            assert_eq!(outcome, None, "pool {pool}");
            assert_eq!(held(&coordinator, &path).await, IMPORTED, "pool {pool}");
            continue;
        }
        assert_eq!(rows, [row(ShieldedActivityStatus::Failed)]);
        assert_eq!(outcome, Some((ArtifactStatus::NotExecuted, None)));
        let after = held(&coordinator, &path).await;
        assert!(!after.record && after.reservation.is_none() && !after.row);
        assert!(after.spendable && !after.spent);
        let settled = artifact(ArtifactStatus::NotExecuted);
        assert!(!keep(&coordinator, &settled).await);
        assert!(!keep(&coordinator, &artifact(OPEN)).await);
        assert_eq!(
            read(&coordinator).await.0,
            [row(ShieldedActivityStatus::Failed)]
        );
    }
}

/// An imported spend that executed: the scan finds its note spent and
/// drops its record, and the engine confirms its row (done), which settles
/// its artifact `executed`. Nothing of it is left to re-send, and its note
/// stays spent; the reading repeats, and no row is saved again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_executed_import_reads_done_and_settles_executed() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("shielded.sqlite");
    zero_one_store(&path);
    let coordinator = open(path.clone(), network(TREE - 1, &[NULLIFIER]).await).await;
    assert!(keep(&coordinator, &artifact(OPEN)).await);

    coordinator.sync(true, &CancellationToken::new()).await;
    let done = (
        vec![row(ShieldedActivityStatus::Confirmed)],
        Some((ArtifactStatus::Executed, Some(("executed", None)))),
    );
    assert_eq!(read(&coordinator).await, done);
    let after = held(&coordinator, &path).await;
    assert!(!after.record && after.reservation.is_none() && !after.row);
    assert!(after.spent && !after.spendable);
    assert_eq!(read(&coordinator).await, done);
    assert!(!keep(&coordinator, &artifact(OPEN)).await);
    assert!(!keep(&coordinator, &artifact(ArtifactStatus::Executed)).await);
    assert_eq!(read(&coordinator).await, done);
}

/// A restart before the spend resolves: the library's row is gone with the
/// session, the record is not. The engine saves it again, once however
/// often it is asked, and never for an artifact the journal settled or one
/// the engine dispatched; the release then fails it as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_before_the_outcome_keeps_one_row() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("shielded.sqlite");
    zero_one_store(&path);
    {
        let coordinator = open(path.clone(), network(TREE, &[]).await).await;
        assert!(keep(&coordinator, &artifact(OPEN)).await);
    }

    let coordinator = open(path.clone(), network(TREE, &[]).await).await;
    assert_eq!(held(&coordinator, &path).await, IMPORTED);
    assert_eq!(read(&coordinator).await, (Vec::new(), None));
    for settled in [ArtifactStatus::NotExecuted, ArtifactStatus::Executed] {
        assert!(!keep(&coordinator, &artifact(settled)).await, "{settled:?}");
    }
    let dispatched = ShieldedArtifact {
        origin: ArtifactOrigin::Dispatched,
        ..artifact(OPEN)
    };
    assert!(!keep(&coordinator, &dispatched).await);
    let other = ShieldedArtifact {
        st_hash: Some([0x99; 32]),
        ..artifact(OPEN)
    };
    assert!(!keep(&coordinator, &other).await);
    assert_eq!(read(&coordinator).await, (Vec::new(), None));

    assert!(keep(&coordinator, &artifact(OPEN)).await);
    assert!(!keep(&coordinator, &artifact(OPEN)).await);
    assert_eq!(
        read(&coordinator).await,
        (vec![row(ShieldedActivityStatus::Pending)], None)
    );
    assert_eq!(held(&coordinator, &path).await, IMPORTED);

    coordinator.sync(true, &CancellationToken::new()).await;
    assert_eq!(
        read(&coordinator).await,
        (
            vec![row(ShieldedActivityStatus::Failed)],
            Some((ArtifactStatus::NotExecuted, None))
        )
    );
}

/// An imported spend whose note is free with its row still pending (the
/// release's failure of the row not in, or its record dropped some other
/// way) waits: unspent notes prove no spend, so the row reads neither
/// failed nor done and the journal keeps the artifact open. Its row reads
/// unknown from then on: a later payment of the free note, which the scan
/// then reads spent, proves nothing of this spend, so the row never reads
/// done and the journal still keeps it open. After a restart, with no
/// record left, no row is saved, and nothing settles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_freed_import_with_a_pending_row_waits() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("shielded.sqlite");
    zero_one_store(&path);
    let coordinator = open(path.clone(), network(TREE - 1, &[]).await).await;
    assert!(keep(&coordinator, &artifact(OPEN)).await);
    assert!(
        (coordinator.store().write().await)
            .clear_pending(subwallet(), &NULLIFIER)
            .unwrap()
    );
    let after = held(&coordinator, &path).await;
    assert!(!after.record && after.reservation.is_none() && after.spendable);
    assert_eq!(read(&coordinator).await.1, None);

    assert!(
        (coordinator.store().write().await)
            .mark_spent(subwallet(), &NULLIFIER)
            .unwrap()
    );
    coordinator.sync(true, &CancellationToken::new()).await;
    assert!(held(&coordinator, &path).await.spent);
    let unknown = (vec![row(ShieldedActivityStatus::Unknown)], None);
    assert_eq!(read(&coordinator).await, unknown);
    assert!(!keep(&coordinator, &artifact(OPEN)).await);
    assert_eq!(read(&coordinator).await, unknown);
    drop(coordinator);

    let coordinator = open(path.clone(), network(TREE - 1, &[]).await).await;
    assert!(
        (coordinator.store().write().await)
            .mark_spent(subwallet(), &NULLIFIER)
            .unwrap()
    );
    assert!(!keep(&coordinator, &artifact(OPEN)).await);
    assert_eq!(read(&coordinator).await, (Vec::new(), None));
}

/// A spend the engine dispatched reads its row as the library leaves it:
/// a pending row with nothing held and every note read spent is not
/// confirmed by the engine (the library's own scan confirms its rows), and
/// the journal keeps its artifact open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dispatched_spends_pending_row_is_not_confirmed_by_the_engine() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("shielded.sqlite");
    zero_one_store(&path);
    let coordinator = open(path.clone(), network(TREE - 1, &[]).await).await;
    let dispatched = ShieldedArtifact {
        origin: ArtifactOrigin::Dispatched,
        ..artifact(OPEN)
    };
    {
        let mut store = coordinator.store().write().await;
        store
            .save_activity(subwallet(), &row(ShieldedActivityStatus::Pending))
            .unwrap();
        assert!(store.mark_spent(subwallet(), &NULLIFIER).unwrap());
    }
    let after = held(&coordinator, &path).await;
    assert!(!after.record && after.reservation.is_none() && after.spent);
    let outcome = {
        let mut store = coordinator.store().write().await;
        spend_outcome(&mut *store, subwallet(), &dispatched).unwrap()
    };
    assert_eq!(outcome, None);
    assert_eq!(
        rows(&coordinator).await,
        [row(ShieldedActivityStatus::Pending)]
    );
}

/// The row the engine saves a spend the journal settled: as [`row`], with
/// the artifact's selected notes (none in [`artifact`]) as the ones it
/// took.
fn settled_row(status: ShieldedActivityStatus) -> ShieldedActivityEntry {
    ShieldedActivityEntry {
        spent_nullifiers: Vec::new(),
        ..row(status)
    }
}

/// A restart after the journal settled an imported spend, which the
/// library's rows (in memory only) do not survive: the engine saves its
/// row again, failed for `not_executed` and confirmed for `executed`,
/// exactly once however often it is asked and however often the store is
/// opened again; none for an artifact not classified or archived (never
/// shown). The journal reads only open artifacts, so nothing settles again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settled_imports_row_is_kept_after_a_restart() {
    let cases = [
        (
            TREE,
            &[][..],
            ArtifactStatus::NotExecuted,
            ShieldedActivityStatus::Failed,
        ),
        (
            TREE - 1,
            &[NULLIFIER][..],
            ArtifactStatus::Executed,
            ShieldedActivityStatus::Confirmed,
        ),
    ];
    for (pool, spent, settled, status) in cases {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("shielded.sqlite");
        zero_one_store(&path);
        {
            let coordinator = open(path.clone(), network(pool, spent).await).await;
            assert!(keep(&coordinator, &artifact(OPEN)).await, "{settled:?}");
            coordinator.sync(true, &CancellationToken::new()).await;
            let (rows, outcome) = read(&coordinator).await;
            assert_eq!(rows, [row(status)], "{settled:?}");
            assert_eq!(outcome.map(|o| o.0), Some(settled));
        }

        for restart in 0..2 {
            let coordinator = open(path.clone(), network(pool, spent).await).await;
            let after = held(&coordinator, &path).await;
            assert!(!after.record && after.reservation.is_none() && !after.row);
            assert!(rows(&coordinator).await.is_empty(), "{settled:?} {restart}");
            for other in [OPEN, ArtifactStatus::Unclassified, ArtifactStatus::Archived] {
                assert!(!keep(&coordinator, &artifact(other)).await, "{other:?}");
            }
            assert!(keep(&coordinator, &artifact(settled)).await, "{settled:?}");
            assert!(!keep(&coordinator, &artifact(settled)).await, "{settled:?}");
            assert_eq!(
                rows(&coordinator).await,
                [settled_row(status)],
                "{settled:?} {restart}"
            );
        }
    }
}
