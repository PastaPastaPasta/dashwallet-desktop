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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dash_sdk::query_types::{
    NoParamQuery, ShieldedAnchors, ShieldedEncryptedNote, ShieldedEncryptedNotes,
    ShieldedEncryptedNotesQuery, ShieldedNotesCount,
};
use platform_wallet::WalletPersister;
use platform_wallet::wallet::persister::NoPlatformPersistence;
use platform_wallet::wallet::shielded::{
    FileBackedShieldedStore, NetworkShieldedCoordinator, OrchardKeySet, ShieldedNote,
    ShieldedStore, SubwalletId,
};
use tokio_util::sync::CancellationToken;

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
            &[0xDE_u8, 0xAD][..],
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
