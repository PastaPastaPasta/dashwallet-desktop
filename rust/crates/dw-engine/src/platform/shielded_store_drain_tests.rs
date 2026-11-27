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
    DispatchedNote, FileBackedShieldedStore, NetworkShieldedCoordinator, ShieldedDispatch,
    ShieldedDispatchKind, ShieldedDispatchPayload, ShieldedStore, SubwalletId,
};

use super::super::close_unload_gate_tests::stored_rows;
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
/// "Fixture main"'s recovery phrase (`expected.json`).
const MAIN_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

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
        self.open_as(options())
    }

    /// [`Self::open`] with a pass's Platform answers ([`pass_sdk`]).
    fn open_answering(&self, sdk: dash_sdk::Sdk) -> Opened {
        self.open_as(crate::SessionOptions {
            sdk: Some(sdk),
            ..options()
        })
    }

    /// A shielded store whose tree held `size` commitments before this
    /// build first opened it, as a fork build's would (the store's
    /// `legacy_anchor_tree_bound`).
    fn with_tree(self, size: u64) -> Self {
        let mut store = FileBackedShieldedStore::open_path(self.net().join("shielded.sqlite"), 100)
            .expect("the fork store opens");
        for leaf in 0..size {
            store
                .append_commitment(&[leaf as u8 + 1; 32], true)
                .unwrap();
        }
        self
    }

    fn open_as(&self, options: crate::SessionOptions) -> Opened {
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
            .block_on(engine.open_network(DashNetwork::Regtest, options))
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
        assert!(
            bound_address(&self.engine, &self.session, main_wallet())
                .0
                .is_bound()
        );
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

    fn journal(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.session.data_dir().join(DISPATCH_DB_FILE)).unwrap()
    }

    /// The journal's artifact of the main wallet's transition `st`, built
    /// on `anchor`.
    fn artifact(&self, st: u8, anchor: [u8; 32], status: &str) {
        self.journal()
            .execute(
                "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, kind, anchor, \
                 status, at) VALUES (?1, ?2, 'dispatched', ?3, 'transfer', ?4, ?5, 1)",
                rusqlite::params![
                    &main_wallet().0[..],
                    &[st; 32][..],
                    &[st; 32][..],
                    &anchor[..],
                    status
                ],
            )
            .unwrap();
    }

    fn artifact_status(&self, st: u8) -> (String, Option<String>) {
        self.journal()
            .query_row(
                "SELECT status, disposition_reason FROM shielded_artifact WHERE st_hash = ?1",
                [&[st; 32][..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    /// The anchor height kept for drained transition `st`, and whether it
    /// was released.
    fn release_row(&self, st: u8) -> Option<(Option<u64>, bool)> {
        use rusqlite::OptionalExtension;
        self.journal()
            .query_row(
                "SELECT recorded_at, released_at IS NOT NULL FROM shielded_drained_release \
                 WHERE st_hash = ?1",
                [&[st; 32][..]],
                |r| Ok((r.get::<_, Option<i64>>(0)?.map(|h| h as u64), r.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    /// The activities of the main wallet's marked spends (DEC-1057).
    fn markers(&self) -> Vec<[u8; 32]> {
        let s = Arc::clone(&self.session);
        (self
            .engine
            .block_on(async move { s.leases.unrecorded_spends(main_wallet()).await }))
        .unwrap()
        .into_iter()
        .map(|m| m.activity)
        .collect()
    }

    /// Saves a note of the account with `nullifier`, read spent when
    /// `spent`, with no changeset: the persister's tap does not see it.
    fn store_note(&self, c: &Arc<NetworkShieldedCoordinator>, nullifier: [u8; 32], spent: bool) {
        let c = Arc::clone(c);
        self.engine.block_on(async move {
            let mut store = c.store().write().await;
            let note = platform_wallet::wallet::shielded::ShieldedNote {
                position: u64::from(nullifier[0]),
                cmx: [nullifier[0].wrapping_add(1); 32],
                nullifier,
                block_height: 1,
                is_spent: false,
                value: 1_000,
                note_data: vec![0; 115],
            };
            store.save_note(subwallet(), &note).unwrap();
            if spent {
                assert!(store.mark_spent(subwallet(), &nullifier).unwrap());
            }
        });
    }

    /// Close Wallet is refused: a spend of the wallet is unresolved.
    fn unload_refused(&self) {
        let r = self
            .engine
            .block_on(self.session.unload_wallet(main_wallet()));
        assert!(
            matches!(r, Err(crate::EngineError::ShieldedSpendUnresolved)),
            "{r:?}"
        );
    }

    /// The engine's dispatch hook's answer to the first send of a transfer
    /// `[n; 32]` of the main wallet selecting the test note at `position`.
    fn dispatch(
        &self,
        c: &Arc<NetworkShieldedCoordinator>,
        n: u8,
        position: u64,
    ) -> Result<(), String> {
        let hook = EngineDispatch::new(&self.session, c);
        let note = note(position, NOTE);
        let st_bytes = super::signed_spend(
            n,
            &ShieldedDispatchKind::Transfer,
            [0xa0; 32],
            &[note.nullifier, [0xee; 32]],
        );
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
        (self.engine)
            .block_on(async move { hook.dispatch(payload).await })
            .map_err(|e| e.reason)
    }

    /// One forced shielded pass.
    fn pass(&self) -> bool {
        let (s, m) = (Arc::clone(&self.session), self.session.manager().unwrap());
        self.engine
            .block_on(async move { pool::guarded_pass(&s, &m, true).await })
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

/// M1 on the store upgrade's real states: a fork hold whose notes the
/// library's store already reads spent, with no changeset the persister's
/// tap saw, is flagged spent from the store at the next pass. A hold all of
/// whose notes it reads spent is cleared; one with a note it reads unspent,
/// or does not hold, stays (a partial set clears nothing), and the close
/// stays refused until the store reads every one spent.
///
/// A drain retires the fork's table before the holds are read, so the
/// holds come from a start whose drain refused: the first start finds a
/// reason it can't decode and keeps the store closed, the holds are
/// marked, and the app quits. The next start, the row readable again,
/// drains and opens the store, and the marks stay until the store reads
/// their notes spent.
#[test]
fn a_fork_hold_over_notes_the_store_reads_spent_clears_at_the_next_pass() {
    let hold = |st, kind, reason, nullifiers: &[[u8; 32]]| ForkRow::Disposition {
        st,
        kind,
        reason,
        nullifiers: nullifiers.to_vec(),
    };
    let root = Root::new(&[
        hold(1, "unresolved", Some("cancelled"), &[[61; 32]]),
        hold(2, "executed", None, &[[62; 32], [63; 32]]),
        hold(3, "unresolved", Some("cancelled"), &[[64; 32]]),
    ]);
    root.damage_reason(1);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Err(ShieldedStoreClosed::Damaged)));
    assert_eq!(o.markers(), [[1; 32], [2; 32], [3; 32]]);
    o.close();
    root.repair_reason(1);

    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    assert!(!root.fork_rows_live(), "the drain retired the fork rows");
    let c = o.bound();
    assert_eq!(o.markers(), [[1; 32], [2; 32], [3; 32]]);
    o.store_note(&c, [61; 32], true);
    o.store_note(&c, [62; 32], true);
    o.store_note(&c, [63; 32], false);
    assert_eq!(
        o.markers(),
        [[1; 32], [2; 32], [3; 32]],
        "the tap saw nothing"
    );

    assert!(o.pass());
    assert_eq!(o.markers(), [[2; 32], [3; 32]]);
    let s = Arc::clone(&o.session);
    let flags: Vec<_> = (o.engine)
        .block_on(async move { s.leases.unrecorded_spends(main_wallet()).await })
        .unwrap()
        .into_iter()
        .map(|m| m.nullifiers)
        .collect();
    assert_eq!(
        flags,
        [
            vec![([62; 32], true), ([63; 32], false)],
            vec![([64; 32], false)]
        ]
    );
    o.unload_refused();

    o.store_note(&c, [64; 32], true);
    assert!(o.pass());
    assert_eq!(o.markers(), [[2; 32]], "unspent stays");
    o.unload_refused();

    o.store_note(&c, [63; 32], true);
    assert!(o.pass());
    assert!(o.markers().is_empty(), "all spent");
    (o.engine)
        .block_on(o.session.unload_wallet(main_wallet()))
        .unwrap();
    drop(c);
    let Opened { engine, session } = o;
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
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
    assert_eq!(
        o.dispatch(&c, 7, 1),
        Err("the spend takes a note a drained fork disposition holds".into())
    );
    assert_eq!(
        o.dispatch(&c, 8, 2),
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
        on_pool(
            &s,
            main_wallet(),
            |_, lib| async move { Ok(lib.bind().await?) },
        )
        .await
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
    let st_bytes = super::signed_spend(
        9,
        &ShieldedDispatchKind::Transfer,
        [0xa0; 32],
        &[note.nullifier, [0xee; 32]],
    );
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

/// The tree a fork build's store held before this build first opened it.
const TREE: u64 = 8;
/// The height Platform's answers to a pass are proved at.
const SET_HEIGHT: u64 = 50;
/// The anchor the drained spends below were built on.
const ANCHOR: [u8; 32] = [0xa1; 32];

/// A pass's Platform answers: no new notes, the recorded anchor set `set`
/// and a pool of `pool_leaves` notes, each proved at [`SET_HEIGHT`].
fn pass_sdk(pool_leaves: u64, set: &[[u8; 32]]) -> dash_sdk::Sdk {
    use dash_sdk::query_types::{
        NoParamQuery, ShieldedAnchors, ShieldedEncryptedNotes, ShieldedEncryptedNotesQuery,
        ShieldedNotesCount,
    };
    let set = set.to_vec();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mut sdk = dash_sdk::Sdk::new_mock();
        let count = 2048
            * u32::from(
                sdk.version()
                    .drive_abci
                    .query
                    .shielded_queries
                    .max_query_chunks,
            );
        for chunk in 0..16 {
            sdk.mock()
                .expect_fetch(
                    ShieldedEncryptedNotesQuery {
                        start_index: u64::from(count) * chunk,
                        count,
                    },
                    Some(ShieldedEncryptedNotes {
                        notes: Vec::new(),
                        total_count: 0,
                    }),
                )
                .await
                .unwrap();
        }
        sdk.mock()
            .expect_fetch(NoParamQuery {}, Some(ShieldedAnchors(set)))
            .await
            .unwrap();
        sdk.mock()
            .expect_fetch(NoParamQuery {}, Some(ShieldedNotesCount(pool_leaves)))
            .await
            .unwrap();
        sdk.mock()
            .set_response_height_of::<ShieldedAnchors>(SET_HEIGHT)
            .set_response_height_of::<ShieldedNotesCount>(SET_HEIGHT)
            .set_response_height_of::<ShieldedEncryptedNotes>(SET_HEIGHT);
        sdk
    })
}

fn sorted(mut v: Vec<[u8; 32]>) -> Vec<[u8; 32]> {
    v.sort();
    v
}

/// Q110 110-3 (E3), the call-site test: a drained unresolved spend whose
/// anchor no completed pass recorded a height for is released by the
/// first pass whose pool proves the tree it was built on recorded and its
/// anchor gone (pool = T), and only then; at T − 1 it stays held, and so
/// does a spend one of whose notes the store doesn't hold. A release is
/// the journal's, kept across a restart, with its artifact settled
/// `not_executed`/`pruned`.
#[test]
fn a_pass_releases_a_drained_spend_only_once_the_pool_proves_its_anchor_pruned() {
    for (pool_leaves, saved, released) in [
        (TREE - 1, &[1, 2, 3][..], false),
        (TREE, &[1, 3][..], false),
        (TREE, &[1, 2, 3][..], true),
    ] {
        let root = Root::new(&[unresolved(1, &[1, 2])]).with_tree(TREE);
        let o = root.open_answering(pass_sdk(pool_leaves, &[[0xee; 32]]));
        o.artifact(1, ANCHOR, "unresolved");
        let c = o.bound();
        o.save_notes(&c, saved);
        assert!(o.pass(), "the pass ran");
        let case = format!("pool {pool_leaves}, notes {saved:?}");
        if released {
            assert_eq!(o.holds(), vec![], "{case}");
            assert_eq!(
                o.selectable(&c).0,
                sorted(vec![nf(1), nf(2), nf(3)]),
                "{case}"
            );
            assert_eq!(o.release_row(1), Some((None, true)), "{case}");
            assert_eq!(
                o.artifact_status(1),
                ("not_executed".into(), Some("pruned".into()))
            );
        } else {
            assert_eq!(o.holds(), vec![(0, nf(1)), (0, nf(2))], "{case}");
            assert_eq!(o.selectable(&c).0, vec![nf(3)], "{case}");
            assert_eq!(o.release_row(1), None, "{case}");
            assert_eq!(o.artifact_status(1), ("unresolved".into(), None), "{case}");
        }
        drop(c);
        o.close();
        if released {
            let o = root.open();
            assert_eq!(o.holds(), vec![], "the release is kept across a restart");
            let c = o.bound();
            o.save_notes(&c, saved);
            assert_eq!(
                o.selectable(&c).0,
                sorted(vec![nf(1), nf(2), nf(3)]),
                "nothing holds the released notes at the next start"
            );
            o.close();
        }
    }
}

#[test]
fn a_retired_dispatch_is_released_only_after_a_covering_prune_proof() {
    let root = Root::new(&[]).with_tree(TREE);
    let o = root.open();
    o.artifact(1, ANCHOR, "registered");
    o.journal()
        .execute(
            "INSERT INTO shielded_artifact_note (wallet, activity, nullifier) VALUES (?1, ?2, ?3)",
            rusqlite::params![&main_wallet().0[..], &[1u8; 32][..], &nf(1)[..]],
        )
        .unwrap();
    o.engine
        .block_on(super::super::shielded_release::record_presence(
            &o.session,
            main_wallet(),
            [1; 32],
            SET_HEIGHT,
        ))
        .unwrap();
    o.close();

    let o = root.open_answering(pass_sdk(TREE, &[ANCHOR]));
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert!(o.pass());
    assert_eq!(o.selectable(&c).0, vec![nf(2)]);
    assert_eq!(o.artifact_status(1).0, "registered");
    drop(c);
    o.close();

    let o = root.open_answering(pass_sdk(TREE, &[[0xee; 32]]));
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert!(o.pass());
    assert_eq!(
        o.artifact_status(1),
        ("not_executed".into(), Some("pruned".into()))
    );
    assert_eq!(o.selectable(&c).0, sorted(vec![nf(1), nf(2)]));
    drop(c);
    o.close();

    let o = root.open();
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.selectable(&c).0, sorted(vec![nf(1), nf(2)]));
    o.close();
}

#[test]
fn a_migrated_cancelled_disposition_shows_held_then_released() {
    use crate::platform::shielded::{ShieldedActivityStatus, ShieldedRefusal};

    let root = Root::new(&[ForkRow::Disposition {
        st: 1,
        kind: "unresolved",
        reason: Some("cancelled"),
        nullifiers: vec![nf(1)],
    }])
    .with_tree(TREE);
    let o = root.open_answering(pass_sdk(TREE, &[[0xee; 32]]));
    o.artifact(1, ANCHOR, "unresolved");
    o.journal().execute(
        "UPDATE shielded_artifact SET disposition_kind = 'unresolved', disposition_reason = 'cancelled'
         WHERE wallet = ?1 AND st_hash = ?2",
        rusqlite::params![&main_wallet().0[..], &[1u8; 32][..]],
    ).unwrap();
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    let activity = |o: &Opened| {
        let row = o
            .engine
            .block_on(o.session.dashpay(main_wallet()).shielded_history(0, 10))
            .unwrap()
            .into_iter()
            .find(|row| row.id == hex::encode([1; 32]))
            .unwrap();
        (row.status, row.refusal)
    };
    assert_eq!(
        activity(&o),
        (ShieldedActivityStatus::Pending, Some(ShieldedRefusal::Held))
    );
    assert!(o.pass());
    assert_eq!(
        activity(&o),
        (
            ShieldedActivityStatus::Failed,
            Some(ShieldedRefusal::Released)
        )
    );
    drop(c);
    o.close();

    let o = root.open();
    let _c = o.bound();
    assert_eq!(
        activity(&o),
        (
            ShieldedActivityStatus::Failed,
            Some(ShieldedRefusal::Released)
        )
    );
    o.close();
}

/// Q110 110-3 (E1): while the pass's set still records a drained spend's
/// anchor, the spend stays held and the journal keeps that height, from
/// the completed pass and from nothing else; once a later start's pass
/// finds the anchor gone, that height alone proves it, with no tree bound
/// (pool T − 1 would hold a heightless one).
#[test]
fn a_recorded_anchors_height_is_kept_and_later_proves_the_spend() {
    let root = Root::new(&[unresolved(1, &[1])]).with_tree(TREE);
    let o = root.open_answering(pass_sdk(TREE, &[ANCHOR, [0xee; 32]]));
    o.artifact(1, ANCHOR, "unresolved");
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.release_row(1), None, "no height before a pass");
    assert!(o.pass());
    assert_eq!(o.release_row(1), Some((Some(SET_HEIGHT), false)));
    assert_eq!(o.holds(), vec![(0, nf(1))], "a recorded anchor holds");
    drop(c);
    o.close();

    let o = root.open_answering(pass_sdk(TREE - 1, &[[0xee; 32]]));
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert!(o.pass());
    assert_eq!(o.release_row(1), Some((Some(SET_HEIGHT), true)));
    assert_eq!(o.holds(), vec![]);
    assert_eq!(o.selectable(&c).0, sorted(vec![nf(1), nf(2)]));
    o.close();
}

/// Q110 110-3 (E2): only the drain's spends are fed or released, and only
/// once its drain is done. A spend of this build on the same pruned anchor
/// is neither: its artifact keeps its status.
#[test]
fn no_spend_of_this_build_is_fed_or_released() {
    let root = Root::new(&[unresolved(1, &[1])]).with_tree(TREE);
    let o = root.open_answering(pass_sdk(TREE, &[[0xee; 32]]));
    o.artifact(1, ANCHOR, "unresolved");
    o.artifact(9, ANCHOR, "unresolved");
    let candidates = {
        let s = Arc::clone(&o.session);
        (o.engine).block_on(async move { pool::drained_release_candidates(&s).await })
    };
    assert_eq!(
        candidates.iter().map(|c| c.st_hash[0]).collect::<Vec<_>>(),
        vec![1],
        "only the drained spend is a candidate"
    );
    o.journal()
        .execute("UPDATE shielded_drain SET progress = 'journaled'", [])
        .unwrap();
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert!(o.pass());
    assert_eq!(
        o.release_row(1),
        None,
        "nothing is released before the drain is done"
    );
    o.journal()
        .execute("UPDATE shielded_drain SET progress = 'done'", [])
        .unwrap();
    assert!(o.pass());
    assert_eq!(o.release_row(1), Some((None, true)));
    assert_eq!(o.release_row(9), None);
    assert_eq!(o.artifact_status(9), ("unresolved".into(), None));
    o.close();
}

/// Q110 110-3 (E4): the shielded store's open asks for the pass evidence,
/// before any pass, while a drained spend may be released, and not
/// otherwise.
#[test]
fn the_open_asks_for_evidence_only_while_a_drained_spend_may_be_released() {
    use std::sync::atomic::Ordering::SeqCst;
    for anchored in [false, true] {
        let root = Root::new(&[unresolved(1, &[1])]).with_tree(TREE);
        let o = root.open();
        if anchored {
            o.artifact(1, ANCHOR, "unresolved");
        }
        let c = o.bound();
        assert_eq!(o.session.platform.passes_run.load(SeqCst), 0, "no pass ran");
        assert_eq!(
            o.session.platform.evidence_wanted.load(SeqCst),
            anchored,
            "anchored {anchored}"
        );
        drop(c);
        o.close();
    }
}

/// Q110 110-3 (E3): a pass's drained spends settle only while no drain of
/// the passes (a lock's or the close's) began since the pass ended. One
/// between the pass and its settle leaves the spend held, and the next
/// pass releases it.
#[test]
fn a_drain_between_a_pass_and_its_settle_releases_nothing() {
    let root = Root::new(&[unresolved(1, &[1, 2])]).with_tree(TREE);
    let o = root.open_answering(pass_sdk(TREE, &[[0xee; 32]]));
    o.artifact(1, ANCHOR, "unresolved");
    let c = o.bound();
    o.save_notes(&c, &[1, 2, 3]);
    let (s, m) = (Arc::clone(&o.session), o.session.manager().unwrap());
    o.engine.block_on(async move {
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        *s.platform.settle_pause.lock().unwrap() = Some(pool::PassPause {
            reached: reached_tx,
            resume: resume_rx,
        });
        let pass = {
            let s = Arc::clone(&s);
            tokio::spawn(async move { pool::guarded_pass(&s, &m, true).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(60), reached)
            .await
            .expect("the pass reached its settle")
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(60), s.drain_shielded_pass())
            .await
            .expect("the drain ends while the pass waits to settle");
        resume.send(()).unwrap();
        assert!(pass.await.unwrap(), "the pass ran");
    });
    assert_eq!(o.release_row(1), None, "a drain since the pass holds it");
    assert_eq!(o.holds(), vec![(0, nf(1)), (0, nf(2))]);
    assert!(o.pass());
    assert_eq!(o.release_row(1), Some((None, true)));
    assert_eq!(o.holds(), vec![]);
    drop(c);
    o.close();
}

/// The user's removal of the main wallet, with a wipe grant: its result,
/// and the grant.
fn remove_main(o: &Opened) -> (Result<(), crate::EngineError>, String) {
    let grant = (o.session.vault)
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&main_wallet().0),
            dw_vault::Credential::Passphrase(PASS),
        )
        .unwrap()
        .id;
    let removed = o
        .engine
        .block_on(o.session.remove_wallet(main_wallet(), grant.clone()));
    (removed, grant)
}

/// Q86 Q-PURGE C2 after the drain: a fork build's stored spend of the main
/// wallet (its `shielded_pending_spends` row, which the drain keeps) and
/// the unresolved disposition the drain moved into the journal. The user's
/// removal is refused, with upstream's store not opened and once it is,
/// and changes nothing: the raw row, the drained hold, the seed and the
/// grant all stay, across a restart too.
#[test]
fn a_drained_fork_spend_and_its_stored_row_refuse_the_removal() {
    let root = Root::new(&[
        unresolved(1, &[1]),
        ForkRow::Pending {
            activity: 1,
            nullifiers: vec![nf(1)],
            st_bytes: b"fork-era spend".to_vec(),
            unsent: false,
        },
    ]);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    assert!(!root.fork_rows_live(), "the drain retired the fork rows");
    let raw = stored_rows(&root.net(), main_wallet());
    assert_eq!(raw.len(), 1, "the drain keeps a sent record");
    assert_eq!(o.holds(), vec![(0, nf(1))]);
    for open_store in [false, true] {
        if open_store {
            o.bound();
        }
        let (removed, grant) = remove_main(&o);
        assert!(
            matches!(
                removed,
                Err(crate::EngineError::ShieldedSpendHeld(
                    crate::ShieldedHold::InFlight
                ))
            ),
            "open_store={open_store}: {removed:?}"
        );
        assert!(o.session.require_wallet(&main_wallet()).is_ok());
        assert!(o.session.vault.has_wallet_secret(&main_wallet().0));
        assert!(
            (o.session.vault)
                .check_grant(&grant, dw_vault::GrantKind::Wipe, Some(&main_wallet().0))
                .is_ok(),
            "the grant is not redeemed"
        );
        assert_eq!(stored_rows(&root.net(), main_wallet()), raw, "the raw row");
        assert_eq!(o.holds(), vec![(0, nf(1))], "the drained hold");
    }
    o.close();
    assert_eq!(stored_rows(&root.net(), main_wallet()), raw);
    let o = root.open();
    assert_eq!(o.holds(), vec![(0, nf(1))]);
    let (removed, _) = remove_main(&o);
    assert!(
        matches!(removed, Err(crate::EngineError::ShieldedSpendHeld(_))),
        "{removed:?}"
    );
    o.close();
}

/// What the removal does with a drained hold alone (the fork's unresolved
/// disposition, no stored record left): `holds_unresolved` does not read
/// the journal's drained holds, so the removal goes through, upstream's
/// store open and the hold's reservation purged with the wallet. The hold
/// stays in the journal under the wallet's id: a re-import of the same
/// seed holds the note again at its bind, and the dispatch hook refuses
/// any spend of it.
#[test]
fn a_drained_hold_alone_leaves_the_removal_and_stays_in_the_journal() {
    let root = Root::new(&[unresolved(1, &[1])]);
    let o = root.open();
    assert_eq!(o.verdict(), Some(Ok(())));
    assert!(stored_rows(&root.net(), main_wallet()).is_empty());
    assert_eq!(o.holds(), vec![(0, nf(1))]);
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.selectable(&c), (vec![nf(2)], NOTE));
    drop(c);

    let (removed, _) = remove_main(&o);
    removed.unwrap();
    assert!(o.session.require_wallet(&main_wallet()).is_err(), "removed");
    assert_eq!(o.holds(), vec![(0, nf(1))], "the drained hold stays");

    let imported = o.engine.block_on(o.session.import_wallet(
        zeroize::Zeroizing::new(MAIN_PHRASE.as_bytes().to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(0),
            core_compat: false,
            ..Default::default()
        },
    ));
    assert_eq!(imported.unwrap(), main_wallet(), "the same seed");
    let c = o.bound();
    o.save_notes(&c, &[1, 2]);
    assert_eq!(o.selectable(&c), (vec![nf(2)], NOTE), "held again");
    assert_eq!(
        o.dispatch(&c, 7, 1),
        Err("the spend takes a note a drained fork disposition holds".into())
    );
    drop(c);
    o.close();
    let o = root.open();
    assert_eq!(o.holds(), vec![(0, nf(1))], "and at the next start");
    o.close();
}
