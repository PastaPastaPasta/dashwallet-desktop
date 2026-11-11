//! MG-13 P16 on MG-04's side: a payout-changing ProUpRegTx saves its
//! payout-change barrier with its row, a discard deletes both in one
//! transaction or neither, and the open backfills the barriers of updates
//! signed before barriers existed, once.

use dw_appdb::fault::{MN_BARRIER_BACKFILL, MN_BARRIER_DISCARD, MN_BARRIER_WRITE};
use dw_appdb::fault::{Stage, arm};
use dw_appdb::{GLOBAL_SCOPE, MasternodeOpRow, NewPayoutBarrier, PAYOUT_BARRIERS_BACKFILLED};

use super::*;

/// A wallet id no session loads.
const ABSENT: &str = "cc";

impl Fx {
    fn barriers(&self) -> Vec<NewPayoutBarrier> {
        let rows = self.db().payout_barriers().unwrap();
        rows.into_iter().map(|r| r.barrier).collect()
    }

    fn app_db_path(&self) -> std::path::PathBuf {
        self.session.data_dir().join(dw_appdb::APP_DB_FILE)
    }

    fn discard(&self, id: &str) -> Result<(), MasternodesError> {
        self.engine
            .block_on(self.mn().discard_operation(self.wallet, id.into()))
    }

    fn mode(&self, mode: Library) {
        *self.lib.mode.lock().unwrap() = mode;
    }

    /// The barrier [`Self::update`]'s operation `op` should have saved.
    fn barrier_of(&self, op: &ProTxOperation) -> NewPayoutBarrier {
        let row = self.rows().into_iter().find(|r| r.id == op.id).unwrap();
        let tx = super::super::ops::signed_tx(&row).unwrap();
        NewPayoutBarrier {
            pro_tx_hash: op.pro_tx_hash.clone().unwrap(),
            op_id: op.id.clone(),
            created_by: self.wallet.to_string(),
            txid: tx.txid().to_string(),
            payout_script: up_payload(&tx).script_payout.to_bytes(),
        }
    }

    fn barrier_state(&self, barrier: &NewPayoutBarrier) -> Option<ProTxOperationState> {
        let row = (self.db().payout_barriers().unwrap().into_iter())
            .find(|r| r.barrier == *barrier)
            .unwrap();
        let wallet = row.barrier.created_by.parse().unwrap();
        let op = (self.db())
            .masternode_op(&row.barrier.created_by, &row.barrier.op_id)
            .unwrap()?;
        self.engine
            .block_on(self.session.barrier_operation_state(wallet, &op))
            .unwrap()
    }

    fn unset_backfill_marker(&self) {
        (self.db())
            .set_setting_durable(GLOBAL_SCOPE, PAYOUT_BARRIERS_BACKFILLED, None)
            .unwrap();
    }

    fn backfilled(&self) -> bool {
        self.db().payout_barriers_backfilled().unwrap()
    }
}

/// The update's payout, kept: the voting key changes, the payout is named
/// as the masternode's current one (D10).
fn kept_payout(hash: &str, registration: &ProTxOperation) -> UpdateRegistrarRequest {
    UpdateRegistrarRequest {
        voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
        payout: PayoutChoice::Address {
            address: registration.terms.payout_address.clone(),
        },
        ..keep(hash)
    }
}

/// P16: a ProUpRegTx that changes the payout saves its barrier (ProTx
/// hash, op id, txid, new payout script, the wallet) with its signed row;
/// one that keeps the payout saves none. The barrier's operation reads as
/// MG-04 reads the row, for the quote's clearing terms.
#[test]
fn mg13_a_payout_change_saves_its_barrier_with_its_row() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let kept = f.update(kept_payout(&hash, &reg), f.grant(COIN)).unwrap();
    assert_eq!(kept.state, ProTxOperationState::Sent);
    assert!(f.barriers().is_empty(), "a kept payout holds nothing");

    let f = fx(Library::NoPeers);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    assert_eq!(
        f.preview_update(keep(&hash)).unwrap().payout_changes,
        Some(true)
    );
    let op = f.update(keep(&hash), f.grant(COIN)).unwrap();
    assert_eq!(op.state, ProTxOperationState::NotSent);
    let barrier = f.barrier_of(&op);
    assert_eq!(f.barriers(), std::slice::from_ref(&barrier));
    assert_eq!(
        dashcore::Address::from_script(
            &dashcore::ScriptBuf::from(barrier.payout_script.clone()),
            dashcore::Network::Regtest
        )
        .unwrap()
        .to_string(),
        op.terms.payout_address,
        "the issued address's script"
    );

    // What commit 4's clearing terms read.
    assert_eq!(
        f.barrier_state(&barrier),
        Some(ProTxOperationState::NotSent)
    );
    let tx = f.lib.sent().pop().unwrap();
    f.confirm(tx, 9);
    assert_eq!(
        f.barrier_state(&barrier),
        Some(ProTxOperationState::Confirmed { height: 9 })
    );
    // The wallet's rows deleted (a wallet removal): the barrier stays, and
    // no wallet history answers for it.
    f.db().delete_wallet(&f.wallet.to_string()).unwrap();
    assert_eq!(f.barrier_state(&barrier), None, "no wallet holds the row");
}

/// P16 (r3 M1, r4 m1), MG-04 D6: discarding a `signed` or a `not_sent`
/// update deletes its barrier with the row; a discard refused because the
/// row may be out (`sent_at` set) keeps both. A discard that fails before
/// its commit keeps both too.
#[test]
fn mg13_a_discard_takes_the_barrier_only_with_an_unsent_row() {
    let f = fx(Library::NoPeers);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();

    let not_sent = f.update(keep(&hash), f.grant(COIN)).unwrap();
    assert_eq!(not_sent.state, ProTxOperationState::NotSent);
    assert_eq!(f.barriers(), [f.barrier_of(&not_sent)]);
    arm(&f.app_db_path(), MN_BARRIER_DISCARD, Stage::Before);
    assert!(f.discard(&not_sent.id).is_err());
    assert_eq!(f.barriers(), [f.barrier_of(&not_sent)], "neither deleted");
    assert_eq!(f.rows().len(), 2);
    f.discard(&not_sent.id).unwrap();
    assert!(f.barriers().is_empty());
    assert_eq!(f.rows().len(), 1, "the registration's");

    f.mode(Library::Accept);
    f.crash_at(Some(CrashPoint::Saved));
    assert!(f.update(keep(&hash), f.grant(COIN)).is_err());
    f.crash_at(None);
    let signed = (f.operations().into_iter())
        .find(|o| o.kind == ProTxOperationKind::UpdateRegistrar)
        .unwrap();
    assert_eq!(signed.state, ProTxOperationState::Signed);
    assert_eq!(f.barriers(), [f.barrier_of(&signed)]);
    f.discard(&signed.id).unwrap();
    assert!(f.barriers().is_empty());

    f.mode(Library::Unconfirmed);
    let sent = f.update(keep(&hash), f.grant(COIN)).unwrap();
    assert_eq!(sent.state, ProTxOperationState::Sent);
    let refused = f.discard(&sent.id).unwrap_err();
    assert_eq!(refused.code(), "masternodes.operation_state");
    assert_eq!(f.barriers(), [f.barrier_of(&sent)], "it may be out (D6)");
}

/// The `mn_barrier.write` fault points (MG-13 §3): before the commit
/// neither the row nor its barrier is saved and nothing goes out; after it
/// both are there after a restart, and the row is `signed`.
#[test]
fn mg13_a_barrier_write_fault_keeps_row_and_barrier_together() {
    let mut f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let sent = f.lib.sent().len();

    arm(&f.app_db_path(), MN_BARRIER_WRITE, Stage::Before);
    assert!(f.update(keep(&hash), f.grant(COIN)).is_err());
    assert_eq!(f.rows().len(), 1, "the registration's only");
    assert!(f.barriers().is_empty());

    arm(&f.app_db_path(), MN_BARRIER_WRITE, Stage::After);
    assert!(f.update(keep(&hash), f.grant(COIN)).is_err());
    assert_eq!(f.lib.sent().len(), sent, "nothing handed off");
    f.restart(&[]);
    let ups: Vec<_> = (f.operations().into_iter())
        .filter(|o| o.kind == ProTxOperationKind::UpdateRegistrar)
        .collect();
    assert_eq!(ups.len(), 1, "the second's row only");
    assert_eq!(ups[0].state, ProTxOperationState::Signed);
    assert_eq!(f.barriers(), [f.barrier_of(&ups[0])]);
}

/// P16 (r3 n3): the first open after the upgrade bars every wallet's
/// signed ProUpRegTx, loaded or not, the payout read from the signed
/// payload or else the stored terms; a stored row can't show a kept
/// payout, so a kept one is barred too (fail closed; the quote's terms
/// clear the settled ones). Unsigned rows and other kinds get none. A
/// later open does nothing; a failed one leaves the marker unset and the
/// session's `barriers_ready` false, and the next open runs it again.
#[test]
fn mg13_the_backfill_bars_signed_updates_once_and_fails_closed() {
    let mut f = fx(Library::Accept);
    assert!(f.backfilled(), "an empty database is backfilled at once");
    assert!(f.session.masternodes.barriers_ready());
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let kept = f.update(kept_payout(&hash, &reg), f.grant(COIN)).unwrap();
    assert!(f.barriers().is_empty());
    let original = f.rows().into_iter().find(|r| r.id == kept.id).unwrap();

    // The rows as a database from before MG-13 holds them.
    let copy = |id: char| MasternodeOpRow {
        id: id.to_string().repeat(32),
        sent_at: None,
        ..original.clone()
    };
    let unsigned = MasternodeOpRow {
        signed_tx: None,
        txid: None,
        signed_at: None,
        ..copy('1')
    };
    let undecodable = MasternodeOpRow {
        signed_tx: Some(vec![0xff]),
        ..copy('2')
    };
    let unloaded = copy('3');
    let (db, w) = (f.db(), f.wallet.to_string());
    db.insert_masternode_op_durable(&w, &unsigned, &[], 1)
        .unwrap();
    db.insert_masternode_op_durable(&w, &undecodable, &[], 1)
        .unwrap();
    db.insert_masternode_op_durable(&ABSENT.repeat(32), &unloaded, &[], 1)
        .unwrap();
    f.unset_backfill_marker();

    f.restart(&[]);
    assert!(f.backfilled() && f.session.masternodes.barriers_ready());
    let script = super::super::barriers::signed_payout_script(&original).unwrap();
    let barrier = |wallet: &str, row: &MasternodeOpRow| NewPayoutBarrier {
        pro_tx_hash: hash.clone(),
        op_id: row.id.clone(),
        created_by: wallet.to_owned(),
        txid: original.txid.clone().unwrap(),
        payout_script: script.to_bytes(),
    };
    let mut expected = vec![
        barrier(&w, &original),
        barrier(&w, &undecodable),
        barrier(&ABSENT.repeat(32), &unloaded),
    ];
    let mut got = f.barriers();
    expected.sort_by(|a, b| a.op_id.cmp(&b.op_id));
    got.sort_by(|a, b| a.op_id.cmp(&b.op_id));
    assert_eq!(got, expected, "the kept, undecodable and unloaded updates");

    // A later open writes nothing, even for a row it would bar.
    let late = copy('4');
    f.db()
        .insert_masternode_op_durable(&w, &late, &[], 1)
        .unwrap();
    f.restart(&[]);
    assert!(f.session.masternodes.barriers_ready(), "already backfilled");
    assert_eq!(f.barriers().len(), 3);

    // A failed backfill: nothing written, the marker unset, quotes held.
    f.unset_backfill_marker();
    arm(&f.app_db_path(), MN_BARRIER_BACKFILL, Stage::Before);
    f.restart(&[]);
    assert!(!f.backfilled());
    assert!(!f.session.masternodes.barriers_ready());
    assert_eq!(f.barriers().len(), 3);

    // A barrier due and impossible to build (no script anywhere) fails it
    // too, rather than being skipped.
    let unbuildable = MasternodeOpRow {
        signed_tx: Some(vec![0xff]),
        terms: "{}".into(),
        ..copy('5')
    };
    f.db()
        .insert_masternode_op_durable(&w, &unbuildable, &[], 1)
        .unwrap();
    f.restart(&[]);
    assert!(!f.backfilled() && !f.session.masternodes.barriers_ready());
    assert_eq!(f.barriers().len(), 3);

    // Once it can run, the next open completes it.
    assert!(f.discard(&unbuildable.id).is_ok());
    f.restart(&[]);
    assert!(f.backfilled() && f.session.masternodes.barriers_ready());
    assert!(f.barriers().iter().any(|b| b.op_id == late.id));
    assert_eq!(f.barriers().len(), 4);
}
