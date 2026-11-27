//! #1047: a payment's verified InstantSend lock is heard before the payment
//! itself, which the wallet first sees in a block with no ChainLock. A fork
//! takes that block away, and key-wallet (which never recorded the lock)
//! removes the payment and gives its coin back. The payment stays pending
//! and locked in Activity, Abandon refuses it, and the coin it spends is
//! neither offered, nor counted, nor taken by Max until the payment is
//! mined again.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{InstantLock, OutPoint, Transaction};

use super::own_spend_balance_tests::funded_wallet_and_funding;
use super::own_spend_disk_tests::{
    Accept, COIN, TIP, coins, fresh_address, own_scripts, prepare, spendable,
};
use super::{balances, deliver, mined, rows};
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

    /// SPV verifies P's lock before the wallet knows P.
    fn lock_heard_first(&self) {
        use dash_spv::EventHandler;
        self.session
            .hub
            .on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
                instant_lock: self.lock(),
                validated: true,
            });
    }

    /// P is first seen in a block at `TIP + 1` with no ChainLock; the lock
    /// then reaches key-wallet, which holds P in a block and records no
    /// lock for it.
    fn mined_then_locked(&self) {
        self.mined();
        let lock = self.lock();
        deliver(&self.engine, &self.session, self.id, move |wm| {
            Box::pin(async move {
                use platform_wallet::key_wallet_manager::WalletInterface;
                wm.process_instant_send_lock(lock);
            })
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

    /// P's Activity row: its status and whether it shows locked.
    fn row(&self) -> Option<(TxStatusKind, bool)> {
        let txid = self.p.txid().to_string();
        (rows(&self.engine, &self.session, self.id).into_iter())
            .find(|r| r.0 == txid)
            .map(|r| (r.1, r.2))
    }

    fn abandon(&self) -> Result<Vec<dashcore::Txid>, EngineError> {
        (self.engine).block_on(self.session.abandon_transaction(self.id, self.p.txid()))
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
        assert_eq!(
            self.row(),
            Some((TxStatusKind::Unconfirmed, true)),
            "P stays in Activity, pending and locked"
        );
        assert!(
            matches!(
                self.abandon(),
                Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked))
            ),
            "Abandon refuses P: {:?}",
            self.abandon()
        );
        assert!(
            !spendable(&self.engine, &self.session, self.id, self.a),
            "a is not offered"
        );
        assert!(!self.a_listed(), "the coin list leaves a out");
        let b = balances(&self.session, self.id);
        assert_eq!(
            (b.total, b.confirmed, b.unconfirmed),
            (11 * COIN, 11 * COIN, 0),
            "a is not counted: {b:?}"
        );
        let (available, inputs) = self.max();
        assert!(available < 11 * COIN, "Max leaves a out: {available}");
        assert!(!inputs.contains(&self.a), "Max does not spend a");
        assert_eq!(inputs.len(), 11, "Max spends every other coin");
    }
}

/// The issue's Desktop test: lock first, then the payment in a block
/// without a ChainLock, then a fork below that block. Once the payment is
/// mined again the hold ends and its change counts.
#[test]
fn a_locked_payment_a_fork_removed_stays_pending_and_keeps_its_coin_spent() {
    let s = scenario();
    s.lock_heard_first();
    s.mined_then_locked();
    let own = own_scripts(&s.engine, &s.session, s.id);
    assert!(!s.a_held(), "P spends a");
    s.fork();
    s.assert_kept();

    s.mined();
    assert!(!s.a_held(), "mined again, P spends a");
    assert!(matches!(s.row(), Some((k, _)) if k != TxStatusKind::Unconfirmed));
    let b = balances(&s.session, s.id);
    assert_eq!(b.total, 11 * COIN + CHANGE, "P's change counts: {b:?}");
    assert!(
        (s.session.hub.history).read_wallet(&s.id, |h| h.fork_kept_locks.is_empty()),
        "the hold ended"
    );
    let change = OutPoint::new(s.p.txid(), 1);
    assert!(own.contains(&s.p.output[1].script_pubkey));
    assert!(spendable(&s.engine, &s.session, s.id, change));
}

/// The same, with the lock heard only after the payment's block: the
/// history then holds the payment in a block, so it marks nothing, yet
/// keeps the lock for a fork.
#[test]
fn a_lock_heard_after_the_block_keeps_the_payment_across_a_fork() {
    let s = scenario();
    s.mined_then_locked();
    s.lock_heard_first();
    s.fork();
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

/// A lock that names no transaction of the wallet's, and spends none of
/// its coins, is not kept.
#[test]
fn a_lock_of_another_wallets_payment_is_not_kept() {
    let s = scenario();
    let other = super::spend_of(
        OutPoint::new(dashcore::Txid::from_byte_array([4; 32]), 0),
        vec![(super::elsewhere(), COIN)],
    );
    use dash_spv::EventHandler;
    s.session
        .hub
        .on_sync_event(&dash_spv::SyncEvent::InstantLockReceived {
            instant_lock: InstantLock {
                txid: other.txid(),
                inputs: vec![other.input[0].previous_output],
                ..InstantLock::default()
            },
            validated: true,
        });
    s.lock_heard_first();
    let kept = (s.session.hub.history).read_wallet(&s.id, |h| h.session_locks.clone());
    assert_eq!(kept, [s.p.txid()].into_iter().collect());
}
