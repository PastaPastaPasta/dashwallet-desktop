//! The drain of a fork-written `shielded.sqlite` at the session's open
//! (Q110 110-2; DRAIN rev 4, engine tests 2, 3, 4, 5 and 8): the v0.1.0
//! release's data root, its shielded store replaced by a fork store, opened
//! by this build. The open's store upgrade takes the before-upgrade
//! snapshot, the journal open drains, and upstream's store opens only
//! after it. No network.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_appdb::dispatch::{ArtifactKind, ArtifactOrigin, NewShieldedArtifact};
use dw_shielded_drain::test_support::{ForkRow, fork_store};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, SHIELDED_ACCOUNT, UnlockScope, VaultConfig};
use platform_wallet::changeset::{
    PlatformWalletChangeSet, PlatformWalletPersistence, ShieldedChangeSet,
};
use platform_wallet::wallet::shielded::{
    DispatchedNote, NetworkShieldedCoordinator, ShieldedDispatch, ShieldedDispatchKind,
    ShieldedDispatchPayload, ShieldedStore, SubwalletId,
};

use super::super::errors::{ShieldedError, ShieldedStoreClosed};
use super::super::shielded_pool::{self as pool, EngineDispatch, ShieldedLib, on_pool};
use super::super::topup_tests::options;
use super::{NOTE, bound_address, note};
use dw_appdb::dispatch::DISPATCH_DB_FILE;
use sha2::Digest;

use crate::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, NetworkSession, WalletId};

/// "Fixture main" of the v0.1.0 fixture (`testdata/upgrade/v0.1.0`).
const MAIN: &str = "0ff35f248c69653efce8336ce14b78e29cd3bbaf4162c42246259850af2c2add";
/// The fixture's vault passphrase.
const PASS: &[u8] = b"upgrade fixture passphrase";

fn main_wallet() -> WalletId {
    let mut id = [0; 32];
    hex::decode_to_slice(MAIN, &mut id).unwrap();
    WalletId(id)
}

/// The nullifier of the test note at `position` ([`note`]).
fn nf(position: u64) -> [u8; 32] {
    note(position, NOTE).nullifier
}

fn executed(st: u8, positions: &[u64]) -> ForkRow {
    ForkRow::Disposition {
        st,
        kind: "executed",
        reason: None,
        nullifiers: positions.iter().map(|p| nf(*p)).collect(),
    }
}

fn unresolved(st: u8, positions: &[u64]) -> ForkRow {
    ForkRow::Disposition {
        st,
        kind: "unresolved",
        reason: Some("purged"),
        nullifiers: positions.iter().map(|p| nf(*p)).collect(),
    }
}

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _: EngineEvent) {}
}

fn copy_tree(src: &Path, dest: &Path) {
    fs::create_dir(dest).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// A data root holding the v0.1.0 fixture with a fork shielded store of
/// `rows` (the main wallet's account 0) in place of its own.
struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Root {
    fn new(rows: &[ForkRow]) -> Self {
        let tmp = dw_testutil::private_tempdir();
        let root = tmp.path().join("data");
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade/v0.1.0/data"),
            &root,
        );
        let net = root.join("regtest");
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = fs::remove_file(net.join(format!("shielded.sqlite{suffix}")));
        }
        fork_store(
            &net.join("shielded.sqlite"),
            &main_wallet().0,
            SHIELDED_ACCOUNT,
            rows,
        )
        .unwrap();
        Self { _tmp: tmp, root }
    }

    fn net(&self) -> PathBuf {
        self.root.join("regtest")
    }

    /// An engine over the root and its regtest session, the vault unlocked.
    fn open(&self) -> Opened {
        let engine = Engine::new(
            EngineConfig {
                data_root: self.root.clone(),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(NullSink),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, options()))
            .expect("the fixture opens");
        engine
            .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
            .unwrap();
        Opened { engine, session }
    }

    /// Gives disposition `st` a reason that is not UTF-8: the fork's
    /// CHECKs and SQLite's integrity check take it, the drain can't decode
    /// it (`Corrupt`).
    fn damage_reason(&self, st: u8) {
        self.set_reason(st, "CAST(x'ff' AS TEXT)");
    }

    /// Puts disposition `st`'s reason back to one the fork writes.
    fn repair_reason(&self, st: u8) {
        self.set_reason(st, "'purged'");
    }

    fn set_reason(&self, st: u8, sql: &str) {
        let conn = rusqlite::Connection::open(self.net().join("shielded.sqlite")).unwrap();
        let changed = conn
            .execute(
                &format!(
                    "UPDATE shielded_spend_dispositions SET reason = {sql} WHERE st_hash = ?1"
                ),
                [&[st; 32][..]],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    /// Whether the store still holds the fork's dispositions table.
    fn fork_rows_live(&self) -> bool {
        let conn = rusqlite::Connection::open(self.net().join("shielded.sqlite")).unwrap();
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'shielded_spend_dispositions'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }
}

struct Opened {
    engine: Engine,
    session: Arc<NetworkSession>,
}

impl Opened {
    fn verdict(&self) -> Option<Result<(), ShieldedStoreClosed>> {
        self.session.platform.shielded_drain.get().cloned()
    }

    /// The main wallet bound, and the coordinator its bind configured.
    fn bound(&self) -> Arc<NetworkShieldedCoordinator> {
        assert!(bound_address(&self.engine, &self.session, main_wallet()).0);
        let manager = self.session.manager().unwrap();
        self.engine
            .block_on(manager.shielded_coordinator())
            .expect("the bind configured the shielded store")
    }

    fn holds(&self) -> Vec<(u32, [u8; 32])> {
        let s = Arc::clone(&self.session);
        self.engine
            .block_on(async move { s.leases.shielded_drained_holds(main_wallet()).await })
            .unwrap()
    }

    /// Gives the account the test notes at `positions`.
    fn save_notes(&self, c: &Arc<NetworkShieldedCoordinator>, positions: &[u64]) {
        let c = Arc::clone(c);
        let notes: Vec<_> = positions.iter().map(|p| note(*p, NOTE)).collect();
        self.engine.block_on(async move {
            let mut store = c.store().write().await;
            for n in &notes {
                store.save_note(subwallet(), n).unwrap();
            }
        });
    }

    /// The nullifiers a spend may select, and the spendable balance.
    fn selectable(&self, c: &Arc<NetworkShieldedCoordinator>) -> (Vec<[u8; 32]>, u64) {
        let c = Arc::clone(c);
        self.engine.block_on(async move {
            let store = c.store().read().await;
            let mut unspent: Vec<_> = (store.get_unspent_notes(subwallet()).unwrap().into_iter())
                .map(|n| n.nullifier)
                .collect();
            unspent.sort();
            (unspent, store.spendable_balance(subwallet()).unwrap())
        })
    }

    fn close(self) {
        drop(self.session);
        self.engine.block_on(self.engine.shutdown()).unwrap();
    }
}

fn subwallet() -> SubwalletId {
    SubwalletId::new(main_wallet().0, SHIELDED_ACCOUNT)
}

/// DRAIN rev 4 test 2 and Q110 110-4: the open drains the fork store, and
/// once upstream's store opens and the account binds, no note a drained
/// executed or unresolved disposition holds is selectable or counted
/// spendable. Upstream's open holds the store's owner lock.
#[test]
fn no_held_note_is_selectable_after_the_drain_and_the_upstream_open() {
    let root = Root::new(&[executed(1, &[1, 2]), unresolved(2, &[3])]);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    assert!(!root.fork_rows_live(), "the drain retired the fork rows");
    assert_eq!(o.holds(), vec![(0, nf(1)), (0, nf(2)), (0, nf(3))]);

    let c = o.bound();
    o.save_notes(&c, &[1, 2, 3, 4]);
    assert_eq!(o.selectable(&c), (vec![nf(4)], NOTE));

    let lock = (OpenOptions::new().write(true))
        .open(root.net().join("shielded.sqlite.lock"))
        .unwrap();
    assert!(
        lock.try_lock().is_err(),
        "upstream's open holds shielded.sqlite.lock"
    );
    o.close();
}

/// Q110 110-2: a drain that refuses keeps upstream's store closed for the
/// session, with the reason, while the rest of the session opens; once the
/// fork store reads, the next start drains and opens it. (A held store
/// lock stops the whole open earlier, at the store upgrade's check.)
#[test]
fn a_refused_drain_keeps_the_shielded_store_closed_until_a_start_drains() {
    let root = Root::new(&[unresolved(1, &[1])]);
    root.damage_reason(1);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Err(ShieldedStoreClosed::Damaged)));

    let status = {
        let s = Arc::clone(&o.session);
        (o.engine).block_on(async move {
            on_pool(&s, main_wallet(), |_, lib| async move {
                pool::status(&lib).await
            })
            .await
        })
    };
    assert_eq!(
        status.unwrap_err(),
        ShieldedError::StoreClosed {
            reason: ShieldedStoreClosed::Damaged
        }
    );
    let configured = {
        let (s, manager) = (Arc::clone(&o.session), o.session.manager().unwrap());
        (o.engine).block_on(async move { pool::coordinator(&s, &manager, None).await })
    };
    assert!(configured.is_err(), "no path configures the store");
    assert!(
        (o.engine)
            .block_on(o.session.manager().unwrap().shielded_coordinator())
            .is_none()
    );
    assert!(root.fork_rows_live(), "nothing opened the fork store");
    assert!(o.holds().is_empty());
    assert!(
        (o.session.wallet_infos().unwrap().iter()).any(|w| w.wallet_id == main_wallet()),
        "the wallet's Core side is open"
    );
    o.close();
    root.repair_reason(1);

    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    assert!(!root.fork_rows_live());
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.selectable(&c), (vec![nf(2)], NOTE));
    o.close();
}

/// DRAIN rev 4 test 3: a record the fork stored and never sent is retired
/// with its notes freed, and upstream keeps no record of it to re-send.
#[test]
fn a_never_sent_record_is_retired_and_its_notes_freed() {
    let root = Root::new(&[ForkRow::Pending {
        activity: 7,
        nullifiers: vec![nf(1)],
        st_bytes: b"never sent".to_vec(),
        unsent: true,
    }]);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    let c = o.bound();
    o.save_notes(&c, &[1]);
    assert_eq!(o.selectable(&c), (vec![nf(1)], NOTE));
    let c2 = Arc::clone(&c);
    let records = o.engine.block_on(async move {
        c2.store()
            .read()
            .await
            .pending_redrives(subwallet())
            .unwrap()
    });
    assert!(records.is_empty(), "nothing to re-send");
    o.close();
}

/// DRAIN rev 4 test 4: a re-send of a transition a drained disposition
/// names is refused by `admit`, even with an artifact that would let it
/// go.
#[test]
fn admit_refuses_a_transition_a_drained_disposition_names() {
    let root = Root::new(&[unresolved(5, &[1])]);
    let o = root.open();
    let c = o.bound();
    let s = Arc::clone(&o.session);
    o.engine.block_on(async move {
        s.ensure_background().await;
        assert!(s.leases.background(&main_wallet()).is_some());
    });
    for st in [5u8, 6] {
        let s = Arc::clone(&o.session);
        let recorded = o.engine.block_on(async move {
            s.leases
                .record_shielded_artifact(
                    main_wallet(),
                    NewShieldedArtifact {
                        activity: [st; 32],
                        origin: ArtifactOrigin::Dispatched,
                        st_hash: Some([st; 32]),
                        kind: Some(ArtifactKind::Transfer),
                        anchor: Some([0xa0; 32]),
                        identity_id: None,
                        selected: vec![nf(1)],
                        published: vec![nf(1), [0xee; 32]],
                    },
                )
                .await
        });
        assert_eq!(recorded, Ok(true));
    }
    let hook = Arc::new(EngineDispatch::new(&o.session, &c));
    let admit = |st: u8| {
        let hook = Arc::clone(&hook);
        o.engine
            .block_on(async move {
                hook.admit(main_wallet().0, SHIELDED_ACCOUNT, [st; 32], [st; 32])
                    .await
            })
            .map_err(|e| e.reason)
    };
    assert_eq!(admit(6), Ok(()), "an artifact no drained row names");
    assert_eq!(
        admit(5),
        Err("a drained fork disposition names the spend".into())
    );
    o.close();
}

/// DRAIN rev 4 test 8: once the persister stores a changeset that spends a
/// held note, the journal reads it spent, and the holds' sync each pass
/// ends with (called here as the pass calls it; a pass needs the network)
/// lifts the engine's hold of it and keeps the others.
#[test]
fn the_hold_lifts_when_the_journal_reads_the_note_spent() {
    let root = Root::new(&[executed(1, &[1, 2])]);
    let o = root.open();
    let c = o.bound();
    o.save_notes(&c, &[1, 2, 3]);
    assert_eq!(o.selectable(&c), (vec![nf(3)], NOTE));

    let mut spent = ShieldedChangeSet::default();
    spent.record_nullifier_spent(subwallet(), nf(1));
    let changeset = PlatformWalletChangeSet {
        shielded: Some(spent),
        ..Default::default()
    };
    (o.session.live().unwrap().store)
        .store(main_wallet().0, changeset)
        .unwrap();
    assert_eq!(o.holds(), vec![(0, nf(2))]);
    let (s, c2) = (Arc::clone(&o.session), Arc::clone(&c));
    o.engine.block_on(async move {
        pool::sync_drained_holds(&s, &c2, &[main_wallet().0])
            .await
            .unwrap();
    });
    assert_eq!(
        (o.session.platform.drained_held.lock().unwrap())
            .get(&main_wallet().0)
            .cloned()
            .unwrap_or_default(),
        [(0, nf(2))].into_iter().collect()
    );
    let c2 = Arc::clone(&c);
    let (reserved_1, reserved_2) = o.engine.block_on(async move {
        let mut store = c2.store().write().await;
        (
            store.mark_pending_by(subwallet(), &nf(1), 1).unwrap(),
            store.mark_pending_by(subwallet(), &nf(2), 1).unwrap(),
        )
    });
    assert!(reserved_1, "the lifted note's reservation is free");
    assert!(!reserved_2, "the held note's is the engine's");
    let c2 = Arc::clone(&c);
    o.engine.block_on(async move {
        (c2.store().write().await)
            .clear_pending_reserved_by(subwallet(), &nf(1), 1)
            .unwrap();
    });
    assert_eq!(o.selectable(&c), (vec![nf(1), nf(3)], 2 * NOTE));
    o.close();
}

/// DRAIN rev 4 test 5's engine half: after a restart the drain finds
/// nothing, and the account's bind holds the drained notes again.
#[test]
fn every_start_after_the_drain_holds_the_drained_notes() {
    let root = Root::new(&[executed(1, &[1]), unresolved(2, &[2])]);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    o.close();
    for _ in 0..2 {
        let o = root.open();
        assert_eq!(o.verdict(), Some(Ok(())));
        assert_eq!(o.holds(), vec![(0, nf(1)), (0, nf(2))]);
        let c = o.bound();
        o.save_notes(&c, &[1, 2, 3]);
        assert_eq!(o.selectable(&c), (vec![nf(3)], NOTE));
        o.close();
    }
}

/// Q110 110-2 (H1): a spend's first send is refused when it selects a note
/// a drained fork disposition holds, whatever upstream's store holds; a
/// spend of other notes passes this check (and is refused later only for
/// having no stored spend here).
#[test]
fn dispatch_refuses_a_spend_of_a_drained_note() {
    let root = Root::new(&[executed(1, &[1])]);
    let o = root.open();
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    let hook = Arc::new(EngineDispatch::new(&o.session, &c));
    let dispatch = |n: u8, position: u64| {
        let note = note(position, NOTE);
        let st_bytes = format!("signed transition {n}").into_bytes();
        let payload = ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Transfer,
            wallet_id: main_wallet().0,
            account_index: SHIELDED_ACCOUNT,
            activity_id: [n; 32],
            st_hash: sha2::Sha256::digest(&st_bytes).into(),
            st_bytes,
            anchor: [0xa0; 32],
            selected_notes: vec![DispatchedNote {
                nullifier: note.nullifier,
                cmx: note.cmx,
                value: note.value,
                position: note.position,
            }],
            published_nullifiers: vec![note.nullifier, [0xee; 32]],
        };
        let hook = Arc::clone(&hook);
        (o.engine)
            .block_on(async move { hook.dispatch(payload).await })
            .map_err(|e| e.reason)
    };
    assert_eq!(
        dispatch(7, 1),
        Err("the spend takes a note a drained fork disposition holds".into())
    );
    assert_eq!(
        dispatch(8, 2),
        Err("the library stored no spend for the activity".into())
    );
    o.close();
}

/// Q110 110-2 (H1): while the drained holds can't be read, the account
/// doesn't bind (no note of it is read or offered) and a spend's first
/// send is refused; once they read, it binds with its holds.
#[test]
fn an_unreadable_holds_table_keeps_the_account_unbound() {
    let root = Root::new(&[executed(1, &[1])]);
    let o = root.open();
    let journal = rusqlite::Connection::open(o.session.data_dir().join(DISPATCH_DB_FILE)).unwrap();
    journal
        .execute_batch("ALTER TABLE shielded_drained_hold RENAME TO held_away")
        .unwrap();
    let s = Arc::clone(&o.session);
    let bound = (o.engine).block_on(async move {
        on_pool(&s, main_wallet(), |_, lib| async move { lib.bind().await }).await
    });
    assert!(bound.is_err(), "{bound:?}");
    journal
        .execute_batch("ALTER TABLE held_away RENAME TO shielded_drained_hold")
        .unwrap();
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.selectable(&c), (vec![nf(2)], NOTE));

    journal
        .execute_batch("ALTER TABLE shielded_drained_hold RENAME TO held_away")
        .unwrap();
    let hook = Arc::new(EngineDispatch::new(&o.session, &c));
    let note = note(2, NOTE);
    let st_bytes = b"signed transition 9".to_vec();
    let payload = ShieldedDispatchPayload {
        kind: ShieldedDispatchKind::Transfer,
        wallet_id: main_wallet().0,
        account_index: SHIELDED_ACCOUNT,
        activity_id: [9; 32],
        st_hash: sha2::Sha256::digest(&st_bytes).into(),
        st_bytes,
        anchor: [0xa0; 32],
        selected_notes: vec![DispatchedNote {
            nullifier: note.nullifier,
            cmx: note.cmx,
            value: note.value,
            position: note.position,
        }],
        published_nullifiers: vec![note.nullifier, [0xee; 32]],
    };
    let refused = (o.engine)
        .block_on(async move { hook.dispatch(payload).await })
        .unwrap_err()
        .reason;
    assert!(
        refused.starts_with("the drained holds could not be read"),
        "{refused}"
    );
    drop(journal);
    o.close();
}
