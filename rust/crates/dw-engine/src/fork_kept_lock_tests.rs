//! #1047: a payment's verified InstantSend lock is heard before the payment
//! itself, which the wallet first sees in a block with no ChainLock. A fork
//! takes that block away, and key-wallet (which never recorded the lock)
//! removes the payment and gives its coin back. The payment stays pending
//! and locked in Activity, Abandon refuses it, and the coin it spends is
//! neither offered, nor counted, nor taken by Max until the payment is
//! mined again.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{InstantLock, OutPoint, Transaction, Txid};

use super::own_spend_disk_tests::{
    Accept, COIN, COINS, FUNDED_AT, TIP, coins, foreign, fresh_address, funding, prepare, rival,
    second_wallet, send, sent_marks, spendable, spendable_for, start_offline_spv, wait_stored,
};
use super::{balances, deliver, engine_wallet, mempool, mined, reopen, rows};
use crate::history::TxStatusKind;
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::tx_actions::TxActionRefusal;
use crate::{CoinFilter, Engine, EngineError, WalletId};

/// What P pays someone else.
const PAID_OUT: u64 = COIN * 2 / 5;
/// P's change.
const CHANGE: u64 = COIN / 2;

/// A wallet of twelve one-DASH coins, and P: a payment of coin `a` made
/// on another device, paying someone else and the wallet change.
struct Scenario {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    a: OutPoint,
    p: Transaction,
}

fn scenario() -> Scenario {
    let (s, paid) = before_funding();
    s.fund(paid);
    s
}

fn before_funding() -> (Scenario, Transaction) {
    let (dir, engine, session, id) = engine_wallet();
    let addresses: Vec<_> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let paid = funding(7, &addresses, COIN);
    let a = OutPoint::new(paid.txid(), 0);
    let change = fresh_address(&engine, &session, id).script_pubkey();
    let p = super::spend_of(a, vec![(super::elsewhere(), PAID_OUT), (change, CHANGE)]);
    let s = Scenario {
        _dir: dir,
        engine,
        session,
        id,
        a,
        p,
    };
    (s, paid)
}

impl Scenario {
    fn fund(&self, funding: Transaction) {
        mined(
            &self.engine,
            &self.session,
            self.id,
            FUNDED_AT,
            vec![funding],
        );
        mined(&self.engine, &self.session, self.id, TIP, vec![foreign()]);
    }

    fn lock(&self) -> InstantLock {
        InstantLock {
            txid: self.p.txid(),
            inputs: self.p.input.iter().map(|i| i.previous_output).collect(),
            ..InstantLock::default()
        }
    }

    /// SPV verifies P's lock without handing P to the wallet's mempool.
    fn hear_lock(&self) {
        self.hear_verified_lock(self.lock());
    }

    fn hear_verified_lock(&self, instant_lock: InstantLock) {
        use dash_spv::EventHandler;
        self.session
            .hub
            .on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
                instant_lock,
                validated: true,
            });
    }

    fn overflow_early_locks(&self) {
        use dash_spv::EventHandler;
        for n in 0..=crate::history::EARLY_LOCK_LIMIT {
            let mut id = [0xa5; 32];
            id[..4].copy_from_slice(&(n as u32).to_le_bytes());
            self.session
                .hub
                .on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
                    instant_lock: InstantLock {
                        txid: Txid::from_byte_array(id),
                        ..InstantLock::default()
                    },
                    validated: true,
                });
        }
    }

    /// P is mined at `TIP + 1`.
    fn mined(&self) {
        mined(
            &self.engine,
            &self.session,
            self.id,
            TIP + 1,
            vec![self.p.clone()],
        );
    }

    /// A fork takes the block at `TIP + 1` away, and the hub hears it.
    fn fork(&self) {
        let heard = self.session.hub.truncations();
        deliver(&self.engine, &self.session, self.id, |wm| {
            Box::pin(async move {
                use platform_wallet::key_wallet_manager::WalletInterface;
                wm.truncate_above(TIP).unwrap();
            })
        });
        assert!(
            self.session.hub.truncations() > heard,
            "the hub heard the fork"
        );
    }

    fn truncate_silently(&self) -> Vec<platform_wallet::key_wallet_manager::WalletEvent> {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let manager = self.session.manager().unwrap();
        let reconciles = std::mem::take(&mut *self.session.hub.reconciles.lock().unwrap());
        self.engine.block_on(async {
            for reconcile in reconciles {
                reconcile.await.unwrap();
            }
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let mut events = wm.subscribe_events();
            wm.truncate_above(TIP).unwrap();
            std::iter::from_fn(|| events.try_recv().ok()).collect()
        })
    }

    fn detect_fork(&self) -> Vec<platform_wallet::key_wallet_manager::WalletEvent> {
        let events = self.truncate_silently();
        let manager = self.session.manager().unwrap();
        self.engine
            .block_on(self.session.refresh_wallet_state(&manager, self.id));
        self.session.hub.settle_rereads();
        events
    }

    fn reopen(self) -> Self {
        let session = reopen(&self.engine, self.session);
        Self { session, ..self }
    }

    fn row(&self) -> Option<(TxStatusKind, bool, i64)> {
        let txid = self.p.txid().to_string();
        (rows(&self.engine, &self.session, self.id).into_iter())
            .find(|r| r.0 == txid)
            .map(|r| (r.1, r.2, r.3))
    }

    fn assert_abandon_refused(&self, refusal: TxActionRefusal) {
        start_offline_spv(&self.engine, &self.session);
        let result = self
            .engine
            .block_on(self.session.abandon_transaction(self.id, self.p.txid()));
        assert!(
            matches!(
                result,
                Err(EngineError::TxActionRefused(reason)) if reason == refusal
            ),
            "Abandon refuses P: {result:?}"
        );
    }

    /// Key-wallet holds `a` unspent.
    fn a_held(&self) -> bool {
        (coins(&self.engine, &self.session, self.id).iter()).any(|(o, _)| *o == self.a)
    }

    /// The coin list shows `a`, locked or not.
    fn a_listed(&self) -> bool {
        let filter = CoinFilter {
            include_locked: true,
            ..CoinFilter::default()
        };
        let rows = (self.engine)
            .block_on(self.session.utxos(self.id, filter))
            .unwrap();
        rows.iter().any(|r| r.outpoint == self.a)
    }

    /// Send's Max: the available amount and the coins a payment of it with
    /// the fee taken from it spends.
    fn max(&self) -> (u64, BTreeSet<OutPoint>) {
        let fee = crate::send::FeeMode::PerKb(1_000);
        let available = (self.engine)
            .block_on(self.session.max_spendable(self.id, CoinSource::Any, fee))
            .unwrap();
        let (_draft, prepared) = prepare(
            &self.engine,
            &self.session,
            self.id,
            CoinSource::Any,
            available,
            true,
            &Arc::new(Accept::default()),
        );
        let inputs = (prepared.unwrap().summary().inputs.iter())
            .map(|i| i.outpoint)
            .collect();
        (available, inputs)
    }

    /// After the fork: P shows pending and locked, Abandon refuses it, and
    /// `a`, which key-wallet gave back, is neither offered, listed,
    /// counted nor taken by Max.
    fn assert_kept(&self, locked: bool, refusal: TxActionRefusal) {
        assert!(self.a_held(), "key-wallet gave a back: {:?}", self.p.txid());
        let b = balances(&self.session, self.id);
        let (available, inputs) = self.max();
        let spendable = spendable(&self.engine, &self.session, self.id, self.a);
        let listed = self.a_listed();
        assert_eq!(
            (
                self.row(),
                (b.total, b.confirmed, b.unconfirmed),
                (listed, spendable),
                (available, inputs.len(), inputs.contains(&self.a)),
            ),
            (
                Some((
                    TxStatusKind::Unconfirmed,
                    locked,
                    CHANGE as i64 - COIN as i64
                )),
                (11 * COIN, 11 * COIN, 0),
                (false, false),
                (11 * COIN, 11, false),
            ),
            "Activity, balance, coin control and Max after the fork"
        );
        self.assert_abandon_refused(refusal);
    }

    fn assert_restarted_from_old_block(&self) {
        assert!(matches!(
            self.row(),
            Some((TxStatusKind::Confirming | TxStatusKind::Confirmed, false, _))
        ));
        assert!(!spendable(&self.engine, &self.session, self.id, self.a));
    }

    fn assert_fork_holds_funding_coin(&self) {
        let b = balances(&self.session, self.id);
        let (max, inputs) = self.max();
        assert_eq!(
            (
                b.total,
                b.confirmed,
                b.withheld,
                max,
                inputs.len(),
                inputs.contains(&self.a),
                self.a_listed(),
                spendable(&self.engine, &self.session, self.id, self.a)
            ),
            (11 * COIN, 11 * COIN, 0, 11 * COIN, 11, false, false, false),
            "balance, Max, Send selection and coin control after the fork"
        );
        assert!(
            !self
                .session
                .hub
                .wallet_state(&self.id)
                .unwrap()
                .excluded
                .waiting()
                .contains(&self.a)
        );
    }
}

fn assert_forked_parent_protected(child_mined: bool) {
    let s = scenario();
    let change = OutPoint::new(s.p.txid(), 1);
    let child = super::spend_of(change, vec![(super::elsewhere(), CHANGE / 2)]);
    let child_id = child.txid();
    let mut payments = vec![s.p.clone()];
    if child_mined {
        payments.push(child);
    }
    mined(&s.engine, &s.session, s.id, TIP + 1, payments.clone());
    s.hear_verified_lock(InstantLock {
        txid: child_id,
        inputs: vec![change],
        ..InstantLock::default()
    });
    assert!(!s.a_held());
    s.fork();
    assert!(s.a_held(), "key-wallet restored the parent's funding coin");
    let history = &s.session.hub.history;
    match history.get(&s.id, &child_id) {
        Some(entry) => {
            assert!(child_mined && entry.context.block_info().is_none());
            assert!(crate::history::instant_locked(&entry));
        }
        None => assert!(!child_mined),
    }
    assert!(history.read_wallet(&s.id, |h| !h.session_locks.contains(&s.p.txid())));

    s.assert_fork_holds_funding_coin();

    mined(&s.engine, &s.session, s.id, TIP + 1, payments);
    assert!(!s.a_held(), "the parent spends its coin again");
    assert!(matches!(s.row(), Some((status, false, _)) if status != TxStatusKind::Unconfirmed));
}

#[test]
fn a_locked_child_keeps_its_forked_parents_funding_coin_unavailable() {
    assert_forked_parent_protected(true);
}

#[test]
fn a_lock_without_a_child_record_keeps_its_forked_parents_coin_unavailable() {
    assert_forked_parent_protected(false);
}

#[test]
fn a_child_lock_through_a_pending_parent_protects_its_forked_grandparent() {
    let s = scenario();
    s.mined();
    let owner =
        dashcore::Address::from_script(&s.p.output[1].script_pubkey, dashcore::Network::Regtest)
            .unwrap();
    let change = fresh_address(&s.engine, &s.session, s.id).script_pubkey();
    let parent = super::spend_of(OutPoint::new(s.p.txid(), 1), vec![(change, CHANGE / 2)]);
    let parent = super::signed_by_wallet(&s.engine, &s.session, s.id, &owner, parent);
    mempool(&s.engine, &s.session, s.id, &parent);
    let change = OutPoint::new(parent.txid(), 0);
    let child = super::spend_of(change, vec![(super::elsewhere(), CHANGE / 4)]);
    s.hear_verified_lock(InstantLock {
        txid: child.txid(),
        inputs: vec![change],
        ..InstantLock::default()
    });
    let history = &s.session.hub.history;
    assert!(history.get(&s.id, &child.txid()).is_none());
    assert!(!s.a_held());
    let before = balances(&s.session, s.id);
    let (max_before, inputs_before) = s.max();

    s.fork();
    assert!(
        s.a_held(),
        "key-wallet restored the grandparent's funding coin"
    );
    assert!(
        history
            .get(&s.id, &parent.txid())
            .is_some_and(|entry| { entry.context.block_info().is_none() && !entry.is_abandoned() })
    );
    let after = balances(&s.session, s.id);
    let (max, inputs) = s.max();
    assert_eq!(
        (
            after.confirmed,
            max,
            inputs,
            s.a_listed(),
            spendable(&s.engine, &s.session, s.id, s.a)
        ),
        (before.confirmed, max_before, inputs_before, false, false),
        "balance, Max, Send selection and coin control after the fork"
    );
}

#[test]
fn a_child_lock_in_another_wallet_protects_the_senders_forked_input() {
    let s = scenario();
    let recipient = second_wallet(&s.engine, &s.session);
    let script = fresh_address(&s.engine, &s.session, recipient).script_pubkey();
    let payment = super::spend_of(s.a, vec![(script, PAID_OUT)]);
    let sent = OutPoint::new(payment.txid(), 0);
    mined(&s.engine, &s.session, s.id, TIP + 1, vec![payment.clone()]);
    mined(&s.engine, &s.session, recipient, TIP + 1, vec![payment]);
    assert!(!s.a_held());

    let child = super::spend_of(sent, vec![(super::elsewhere(), PAID_OUT / 2)]);
    s.hear_verified_lock(InstantLock {
        txid: child.txid(),
        inputs: vec![sent],
        ..InstantLock::default()
    });
    let history = &s.session.hub.history;
    assert!(history.read_wallet(&recipient, |h| h.session_locks.contains(&child.txid())));
    assert!(history.read_wallet(&s.id, |h| !h.session_locks.contains(&child.txid())));

    s.fork();
    assert!(s.a_held(), "key-wallet restored the sender's funding coin");
    s.assert_fork_holds_funding_coin();
}

/// P and C, its child spending P's change, are mined and forked out before
/// any lock is heard; C's lock verifies afterward.
fn forked_parent_and_late_locked_child(s: &Scenario) -> Transaction {
    let change = OutPoint::new(s.p.txid(), 1);
    let child = super::spend_of(change, vec![(super::elsewhere(), CHANGE / 2)]);
    let payments = vec![s.p.clone(), child.clone()];
    mined(&s.engine, &s.session, s.id, TIP + 1, payments);
    s.fork();
    assert!(s.a_held(), "key-wallet restored the parent's funding coin");
    assert!(s.max().1.contains(&s.a));
    s.hear_verified_lock(InstantLock {
        txid: child.txid(),
        inputs: vec![change],
        ..InstantLock::default()
    });
    s.session.hub.settle_rereads();
    child
}

fn child_row(s: &Scenario, child: &Transaction) -> Option<(TxStatusKind, bool)> {
    let txid = child.txid().to_string();
    (rows(&s.engine, &s.session, s.id).into_iter())
        .find(|r| r.0 == txid)
        .map(|r| (r.1, r.2))
}

#[test]
fn a_child_recovered_by_a_late_lock_keeps_its_forked_parents_funding_coin_unavailable() {
    let s = scenario();
    let child = forked_parent_and_late_locked_child(&s);
    assert_eq!(
        child_row(&s, &child),
        Some((TxStatusKind::Unconfirmed, true))
    );
    s.assert_fork_holds_funding_coin();
}

#[test]
fn a_child_recovered_by_a_late_lock_stays_locked_across_a_later_fork() {
    let s = scenario();
    let child = forked_parent_and_late_locked_child(&s);
    s.overflow_early_locks();
    mined(
        &s.engine,
        &s.session,
        s.id,
        TIP + 1,
        vec![s.p.clone(), child.clone()],
    );
    assert!(!s.a_held(), "the parent spends its coin again");
    s.fork();
    assert_eq!(
        child_row(&s, &child),
        Some((TxStatusKind::Unconfirmed, true))
    );
    s.assert_fork_holds_funding_coin();
}

fn assert_recordless_child_protects_pending_parent(buffered: bool) {
    let (mut s, funding) = before_funding();
    s.fund(funding.clone());
    s.p = rival(&s.engine, &s.session, s.id, &funding, 0);
    let change = OutPoint::new(s.p.txid(), 1);
    let child_id = super::spend_of(change, vec![(super::elsewhere(), CHANGE / 2)]).txid();
    let lock = InstantLock {
        txid: child_id,
        inputs: vec![change],
        ..InstantLock::default()
    };
    if buffered {
        s.hear_verified_lock(lock.clone());
    }
    mempool(&s.engine, &s.session, s.id, &s.p);
    if !buffered {
        s.hear_verified_lock(lock);
    }
    assert!(s.session.hub.history.get(&s.id, &child_id).is_none());
    assert!(matches!(
        s.row(),
        Some((TxStatusKind::Unconfirmed, false, _))
    ));
    let (max_before, inputs_before) = s.max();
    start_offline_spv(&s.engine, &s.session);

    let abandon = || {
        s.engine
            .block_on(s.session.abandon_transaction(s.id, s.p.txid()))
    };
    let remove = || s.engine.block_on(s.session.drop_unconfirmed(Some(s.id)));
    let (abandoned, removed) = if buffered {
        let removed = remove();
        (abandon(), removed)
    } else {
        (abandon(), remove())
    };
    assert!(
        matches!(
            abandoned,
            Err(EngineError::TxActionRefused(
                TxActionRefusal::SettledDescendant
            ))
        ) && matches!(removed, Ok(0)),
        "recordless child lock protects P: Abandon {abandoned:?}, Remove Unconfirmed {removed:?}"
    );
    assert!(matches!(
        s.row(),
        Some((TxStatusKind::Unconfirmed, false, _))
    ));
    let (max, inputs) = s.max();
    assert_eq!(balances(&s.session, s.id).confirmed, 11 * COIN);
    assert_eq!((max, &inputs), (max_before, &inputs_before));
    assert!(!inputs.contains(&s.a));
    assert!(!s.a_listed());
    assert!(!spendable(&s.engine, &s.session, s.id, s.a));
}

#[test]
fn a_live_lock_without_a_child_record_protects_its_pending_parent() {
    assert_recordless_child_protects_pending_parent(false);
}

#[test]
fn a_buffered_lock_without_a_child_record_protects_its_pending_parent() {
    assert_recordless_child_protects_pending_parent(true);
}

/// The issue's Desktop test: lock first, then the payment in a block
/// without a ChainLock, then a fork below that block. Once the payment is
/// mined again the hold ends and its change counts.
#[test]
fn a_locked_payment_a_fork_removed_stays_pending_and_keeps_its_coin_spent() {
    let s = scenario();
    s.hear_lock();
    s.mined();
    assert!(!s.a_held(), "P spends a");
    s.fork();
    s.assert_kept(true, TxActionRefusal::InstantLocked);

    s.mined();
    assert!(!s.a_held(), "mined again, P spends a");
    assert!(matches!(s.row(), Some((k, _, _)) if k != TxStatusKind::Unconfirmed));
    let b = balances(&s.session, s.id);
    assert_eq!(b.total, 11 * COIN + CHANGE, "P's change counts: {b:?}");
    let change = OutPoint::new(s.p.txid(), 1);
    assert!(spendable_for(
        &s.engine,
        &s.session,
        s.id,
        change,
        CHANGE / 2
    ));
}

/// The same, with the lock heard only after the payment's block: the
/// history then holds the payment in a block, so it marks nothing, yet
/// keeps the lock for a fork.
#[test]
fn a_lock_heard_after_the_block_keeps_the_payment_across_a_fork() {
    let s = scenario();
    s.mined();
    s.hear_lock();
    s.fork();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

#[test]
fn a_lock_heard_before_scanning_its_funding_parent_keeps_the_payment_across_a_fork() {
    let (s, funding) = before_funding();
    s.hear_lock();
    s.fund(funding);
    s.mined();
    s.fork();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

#[test]
fn an_evicted_lock_still_holds_its_input_when_the_wallet_later_forks() {
    let (s, funding) = before_funding();
    s.hear_lock();
    s.overflow_early_locks();
    s.fund(funding);
    s.mined();
    s.fork();
    s.assert_kept(false, TxActionRefusal::RescanFirst);
}

#[test]
fn a_buffered_lock_is_matched_only_to_the_wallet_that_owns_its_input() {
    let (s, funding) = before_funding();
    let other = second_wallet(&s.engine, &s.session);
    s.hear_lock();
    let holds_lock = |id| {
        s.session
            .hub
            .history
            .read_wallet(&id, |h| h.session_locks.contains(&s.p.txid()))
    };
    assert!(!holds_lock(s.id));
    assert!(!holds_lock(other));
    s.fund(funding);
    assert!(holds_lock(s.id));
    assert!(!holds_lock(other));
}

#[test]
fn loading_a_wallet_matches_buffered_locks_to_its_stored_outputs() {
    let (s, funding) = before_funding();
    let parent = funding.txid();
    s.fund(funding);
    wait_stored(&s.session, s.id, &[parent]);
    s.engine.block_on(s.session.unload_wallet(s.id)).unwrap();
    s.hear_lock();
    s.engine.block_on(s.session.load_wallet(s.id)).unwrap();
    assert!(
        s.session
            .hub
            .history
            .read_wallet(&s.id, |h| h.session_locks.contains(&s.p.txid()))
    );
}

fn unrecorded_spender(hear_lock: bool) -> Scenario {
    let (mut s, funding) = before_funding();
    s.p.output.truncate(1);
    if hear_lock {
        s.hear_lock();
    }
    s.mined();
    s.fund(funding);
    // The spend was observed before funding, and its block never replayed.
    assert_eq!(s.row(), None);
    assert!(!s.a_held());
    assert_eq!(balances(&s.session, s.id).confirmed, 11 * COIN);
    s
}

fn assert_restored_input(s: &Scenario, held: bool) {
    assert!(s.a_held(), "key-wallet restored the input");
    assert_eq!(s.row(), None);
    let (available, inputs) = s.max();
    let expected = if held { 11 * COIN } else { 12 * COIN };
    assert_eq!(
        (
            balances(&s.session, s.id).confirmed,
            available,
            inputs.contains(&s.a),
            s.a_listed(),
            spendable(&s.engine, &s.session, s.id, s.a),
        ),
        (expected, expected, !held, !held, !held),
    );
}

#[test]
fn a_fork_holds_the_coin_restored_for_an_unrecorded_locked_spender() {
    let s = unrecorded_spender(true);
    s.fork();
    assert_restored_input(&s, true);
}

#[test]
fn a_fork_detected_without_an_event_holds_the_unrecorded_locked_spenders_input() {
    let s = unrecorded_spender(true);
    s.detect_fork();
    assert_restored_input(&s, true);
}

#[test]
fn max_reconciles_a_silent_fork_before_offering_coins() {
    let s = unrecorded_spender(true);
    s.truncate_silently();
    let count = s.session.hub.truncations();
    let max = s
        .engine
        .block_on(s.session.max_spendable(
            s.id,
            CoinSource::Any,
            crate::send::FeeMode::PerKb(1_000),
        ))
        .unwrap();
    assert_eq!(max, 11 * COIN);
    assert!(s.session.hub.truncations() > count);
}

#[test]
fn coin_control_reconciles_a_silent_fork_before_listing_coins() {
    let s = unrecorded_spender(true);
    s.truncate_silently();
    let count = s.session.hub.truncations();
    assert!(!s.a_listed());
    assert!(s.session.hub.truncations() > count);
}

#[test]
fn a_verified_lock_during_detection_holds_an_unrecorded_spenders_input() {
    use std::sync::Barrier;

    let s = unrecorded_spender(false);
    let reached = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *s.session.hub.pause_truncation.lock().unwrap() =
        Some((Arc::clone(&reached), Arc::clone(&release)));

    std::thread::scope(|scope| {
        let detecting = scope.spawn(|| s.detect_fork());
        reached.wait();
        s.hear_lock();
        release.wait();
        detecting.join().unwrap();
    });

    assert_restored_input(&s, true);
}

#[test]
fn a_verified_lock_after_detection_holds_an_unrecorded_spenders_input() {
    let s = unrecorded_spender(false);
    s.detect_fork();
    assert_restored_input(&s, false);

    s.hear_lock();
    s.session.hub.settle_rereads();
    assert_restored_input(&s, true);
}

#[test]
fn a_verified_lock_after_a_truncation_event_holds_an_unrecorded_spenders_input() {
    let s = unrecorded_spender(false);
    s.fork();
    assert_restored_input(&s, false);

    s.hear_lock();
    s.session.hub.settle_rereads();
    assert_restored_input(&s, true);
}

#[test]
fn a_truncation_event_after_detection_holds_the_restored_input_after_overflow() {
    let s = unrecorded_spender(false);
    s.overflow_early_locks();
    let events = s.detect_fork();
    let seen = s.session.hub.truncations();
    for event in &events {
        s.session.hub.apply_wallet_event(event);
    }
    s.session.hub.settle_rereads();
    assert_eq!(s.session.hub.truncations(), seen);
    assert_restored_input(&s, true);
}

#[test]
fn a_fork_restores_an_unrecorded_spenders_input_without_a_lock_or_overflow() {
    let s = unrecorded_spender(false);
    s.fork();
    assert_restored_input(&s, false);
}

#[test]
fn a_truncation_without_a_wallet_event_keeps_the_locked_payment() {
    let s = scenario();
    s.hear_lock();
    s.mined();
    s.detect_fork();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

/// A fork's removal of a payment no verified lock named this session is
/// unchanged: the payment goes and its coin is offered again.
#[test]
fn a_payment_without_a_lock_a_fork_removed_goes_and_frees_its_coin() {
    let s = scenario();
    s.mined();
    s.fork();
    assert_eq!(s.row(), None, "P is gone");
    assert!(spendable(&s.engine, &s.session, s.id, s.a), "a is offered");
    assert_eq!(balances(&s.session, s.id).total, 12 * COIN);
}

#[test]
fn a_lock_verified_after_the_fork_recovers_the_payment_and_holds_its_input() {
    let s = scenario();
    s.mined();
    s.fork();
    assert_eq!(s.row(), None);
    assert!(s.max().1.contains(&s.a));

    s.hear_lock();
    s.session.hub.settle_rereads();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

#[test]
fn a_restart_loads_the_old_block_record_for_a_payment_sent_from_another_device() {
    let s = scenario();
    s.hear_lock();
    s.mined();
    wait_stored(&s.session, s.id, &[s.p.txid()]);
    s.fork();
    wait_stored(&s.session, s.id, &[s.p.txid()]);
    s.reopen().assert_restarted_from_old_block();
}

#[test]
fn a_restart_loads_the_old_block_record_and_keeps_a_local_payments_sent_mark() {
    let mut s = scenario();
    s.p = send(
        &s.engine,
        &s.session,
        s.id,
        CoinSource::Outpoints(vec![s.a]),
        PAID_OUT,
        false,
    );
    s.hear_lock();
    s.mined();
    wait_stored(&s.session, s.id, &[s.p.txid()]);
    s.fork();
    wait_stored(&s.session, s.id, &[s.p.txid()]);

    let s = s.reopen();
    s.assert_restarted_from_old_block();
    assert!(sent_marks(&s.session, s.id).contains(&s.p.txid()));
}

fn with_stored_payment_copied(s: &Scenario, while_paused: impl FnOnce()) {
    use std::sync::atomic::Ordering;

    wait_stored(&s.session, s.id, &[s.p.txid()]);
    let copied = Arc::new(crate::coins::TestPause::default());
    let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&copied));
    *s.session.hub.pause_history.lock().unwrap() = Some(Arc::clone(&copied));
    let (session, id) = (Arc::clone(&s.session), s.id);
    let load = s
        .session
        .rt
        .spawn(async move { session.load_history_for(vec![id]).await });
    super::own_spend_disk_tests::eventually("the history copy", || {
        copied.reached.load(Ordering::SeqCst)
    });
    while_paused();
    copied.release.notify_one();
    s.engine.block_on(load).unwrap().unwrap();
    s.session.hub.settle_rereads();
}

#[test]
fn stale_history_after_a_fork_preserves_the_removed_spend_for_a_late_lock() {
    let s = scenario();
    s.mined();
    with_stored_payment_copied(&s, || {
        s.fork();
        assert_eq!(s.row(), None);
    });
    assert_eq!(s.row(), None, "the copied row belongs to the removed block");

    s.hear_lock();
    s.session.hub.settle_rereads();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

#[test]
fn stale_history_after_a_fork_keeps_the_input_held_after_lock_overflow() {
    let s = scenario();
    s.overflow_early_locks();
    s.mined();
    with_stored_payment_copied(&s, || {
        s.fork();
        assert_eq!(
            s.row().map(|r| (r.0, r.1)),
            Some((TxStatusKind::Unconfirmed, false))
        );
    });
    s.assert_kept(false, TxActionRefusal::RescanFirst);
}

#[test]
fn only_a_verified_lock_relevant_to_the_wallet_is_kept() {
    use dash_spv::EventHandler;
    use platform_wallet::key_wallet_manager::WalletEvent;
    let s = scenario();
    let other = super::spend_of(
        OutPoint::new(Txid::from_byte_array([4; 32]), 0),
        vec![(super::elsewhere(), COIN)],
    );
    let hub = &s.session.hub;
    hub.on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
        instant_lock: InstantLock {
            txid: other.txid(),
            inputs: vec![other.input[0].previous_output],
            ..InstantLock::default()
        },
        validated: true,
    });
    hub.on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
        instant_lock: s.lock(),
        validated: false,
    });
    hub.on_wallet_event(&WalletEvent::TransactionInstantLocked {
        wallet_id: s.id.0,
        txid: s.p.txid(),
        instant_lock: s.lock(),
        balance: Default::default(),
        account_balances: Default::default(),
    });
    assert!(
        hub.history
            .read_wallet(&s.id, |h| h.session_locks.is_empty())
    );

    s.hear_lock();
    let kept = hub.history.read_wallet(&s.id, |h| h.session_locks.clone());
    assert_eq!(kept, [s.p.txid()].into_iter().collect());
}

#[test]
fn a_stored_row_read_before_the_fork_cannot_replace_the_kept_payment() {
    use std::sync::atomic::Ordering;
    let s = scenario();
    s.hear_lock();
    s.mined();
    wait_stored(&s.session, s.id, &[s.p.txid()]);
    let copied = Arc::new(crate::coins::TestPause::default());
    let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&copied));
    *s.session.hub.pause_history.lock().unwrap() = Some(Arc::clone(&copied));
    let (session, id) = (Arc::clone(&s.session), s.id);
    let load = s
        .session
        .rt
        .spawn(async move { session.load_history_for(vec![id]).await });
    super::own_spend_disk_tests::eventually("the history copy", || {
        copied.reached.load(Ordering::SeqCst)
    });
    s.fork();
    assert_eq!(
        s.row().map(|r| (r.0, r.1)),
        Some((TxStatusKind::Unconfirmed, true))
    );
    copied.release.notify_one();
    s.engine.block_on(load).unwrap().unwrap();
    s.session.hub.settle_rereads();
    s.assert_kept(true, TxActionRefusal::InstantLocked);
}

#[test]
fn an_evicted_lock_holds_a_second_input_funded_after_the_fork() {
    let (mut s, first) = before_funding();
    let second = funding(9, &[fresh_address(&s.engine, &s.session, s.id)], COIN);
    let b = OutPoint::new(second.txid(), 0);
    s.p.input.push(dashcore::TxIn {
        previous_output: b,
        ..Default::default()
    });
    s.hear_lock();
    s.overflow_early_locks();
    s.fund(first);
    s.mined();
    s.fork();
    s.fund(second);
    assert!(
        coins(&s.engine, &s.session, s.id)
            .iter()
            .any(|(o, _)| *o == b)
    );
    let (available, inputs) = s.max();
    assert_eq!(
        (
            balances(&s.session, s.id).confirmed,
            available,
            inputs.contains(&b)
        ),
        (11 * COIN, 11 * COIN, false)
    );
}
