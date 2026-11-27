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

use super::own_spend_balance_tests::funded_wallet_and_funding;
use super::own_spend_disk_tests::{
    Accept, COIN, TIP, coins, fresh_address, prepare, send, sent_marks, spendable,
    start_offline_spv, wait_stored,
};
use super::{balances, deliver, mined, reopen, rows};
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
    let (dir, engine, session, id, paid) = funded_wallet_and_funding();
    let a = OutPoint::new(paid.txid(), 0);
    let change = fresh_address(&engine, &session, id).script_pubkey();
    let p = super::spend_of(a, vec![(super::elsewhere(), PAID_OUT), (change, CHANGE)]);
    Scenario {
        _dir: dir,
        engine,
        session,
        id,
        a,
        p,
    }
}

impl Scenario {
    fn lock(&self) -> InstantLock {
        InstantLock {
            txid: self.p.txid(),
            inputs: vec![self.a],
            ..InstantLock::default()
        }
    }

    /// SPV verifies P's lock without handing P to the wallet's mempool.
    fn hear_lock(&self) {
        use dash_spv::EventHandler;
        self.session
            .hub
            .on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
                instant_lock: self.lock(),
                validated: true,
            });
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

    fn assert_abandon_refused(&self) {
        start_offline_spv(&self.engine, &self.session);
        let result = self
            .engine
            .block_on(self.session.abandon_transaction(self.id, self.p.txid()));
        assert!(
            matches!(
                result,
                Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked))
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
    fn assert_kept(&self) {
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
                Some((TxStatusKind::Unconfirmed, true, CHANGE as i64 - COIN as i64)),
                (11 * COIN, 11 * COIN, 0),
                (false, false),
                (11 * COIN, 11, false),
            ),
            "Activity, balance, coin control and Max after the fork"
        );
        self.assert_abandon_refused();
    }

    fn assert_restarted_from_old_block(&self) {
        assert!(
            matches!(self.row(), Some((status, false, _)) if status != TxStatusKind::Unconfirmed)
        );
        assert!(!spendable(&self.engine, &self.session, self.id, self.a));
    }
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
    s.assert_kept();

    s.mined();
    assert!(!s.a_held(), "mined again, P spends a");
    assert!(matches!(s.row(), Some((k, _, _)) if k != TxStatusKind::Unconfirmed));
    let b = balances(&s.session, s.id);
    assert_eq!(b.total, 11 * COIN + CHANGE, "P's change counts: {b:?}");
    let change = OutPoint::new(s.p.txid(), 1);
    assert!(spendable(&s.engine, &s.session, s.id, change));
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
    s.assert_kept();
}

#[test]
fn a_truncation_without_a_wallet_event_keeps_the_locked_payment() {
    use platform_wallet::key_wallet_manager::WalletInterface;
    let s = scenario();
    s.hear_lock();
    s.mined();
    let manager = s.session.manager().unwrap();
    s.engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        wm.write().await.truncate_above(TIP).unwrap();
        s.session.refresh_wallet_state(&manager, s.id).await;
    });
    s.session.hub.settle_rereads();
    s.assert_kept();
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
