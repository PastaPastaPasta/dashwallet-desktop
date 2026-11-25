//! Q100 E1: a coin whose parent payment may never confirm is withheld. It
//! is out of spending and out of every balance, and shows as pending.
//! The beaten payment of #818 S2a/S2b is the case: a rival mined and
//! ChainLocked before the block's reconcile ran, so key-wallet's note of
//! the rival's spend was pruned and nothing proves the payment dead. Also
//! a child built on such a coin, a slow but valid payment (its change
//! comes back once it is mined, with no restart or rescan), an
//! InstantSend-locked payment (not withheld), and an abandon of the beaten
//! payment.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{OutPoint, Transaction};

use super::own_spend_balance_tests::{
    foreign_winner, funded_wallet_and_funding, live_in_history, reconciled,
};
use super::own_spend_disk_tests::{
    Accept, COIN, TIP, change_of, coins, eventually, own_scripts, prepare, send, spendable_for,
    wait_stored,
};
use super::{balances, instant_lock, mempool, mined, reopen, wait_stored_locked};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{CoinFilter, Engine, ExclusionReason, WalletId};

/// When the rival's ChainLock arrives against the block's reconcile.
#[derive(Clone, Copy, Debug)]
enum Order {
    /// S2a: the ChainLock is applied in the block's own op.
    ChainLockInTheBlocksOp,
    /// S2b: the block's reconcile is held at the drop gate while the
    /// ChainLock arrives.
    ChainLockWhileTheReconcileWaits,
}

/// A wallet after #818's beaten payment: P of A and B with change,
/// restored at a load, beaten on A by a rival paying only someone else,
/// mined at `TIP + 1` and ChainLocked in `order`.
struct Beaten {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    p: Transaction,
    change: OutPoint,
}

/// [`Beaten`], with `child` (a payment of P's change made before the
/// restart) when asked for.
fn beaten(order: Order, with_child: bool) -> (Beaten, Option<(Transaction, OutPoint)>) {
    let (dir, engine, session, id, paid) = funded_wallet_and_funding();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN * 3 / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    wait_stored(&session, id, &[p.txid()]);
    let child = with_child.then(|| {
        let c = send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![change]),
            COIN / 10,
            false,
        );
        mempool(&engine, &session, id, &c);
        wait_stored(&session, id, &[c.txid()]);
        let c_change = change_of(&c, &own_scripts(&engine, &session, id));
        (c, c_change)
    });

    let session = reopen(&engine, session);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    let hash = super::block_with(vec![winner.clone()]).block_hash();
    match order {
        Order::ChainLockInTheBlocksOp => {
            let w = winner.clone();
            super::deliver(&engine, &session, id, move |wm| {
                Box::pin(async move {
                    use platform_wallet::key_wallet_manager::WalletInterface;
                    let block = super::block_with(vec![w]);
                    wm.apply_chain_lock(dashcore::ChainLock {
                        block_height: TIP + 1,
                        block_hash: block.block_hash(),
                        signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
                    });
                    wm.process_block_for_wallets(
                        &block,
                        block.block_hash(),
                        TIP + 1,
                        &BTreeSet::from([id.0]),
                    )
                    .await;
                    wm.update_wallet_synced_height(&id.0, TIP + 1);
                })
            });
            reconciled(&engine, &session, id);
        }
        Order::ChainLockWhileTheReconcileWaits => {
            let gate = engine.block_on(session.hub.drop_gate.lock());
            mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
            super::own_spend_disk_tests::chain_lock(&engine, &session, id, TIP + 1, hash);
            drop(gate);
            reconciled(&engine, &session, id);
        }
    }
    let state = Beaten {
        _dir: dir,
        engine,
        session,
        id,
        p,
        change,
    };
    (state, child)
}

/// The value of `coin`, one of the wallet's.
fn value_of(engine: &Engine, session: &NetworkSession, id: WalletId, coin: OutPoint) -> u64 {
    (coins(engine, session, id).into_iter())
        .find(|(o, _)| *o == coin)
        .map(|(_, v)| v)
        .expect("the wallet holds the coin")
}

/// What the wallet holds besides `withheld`.
fn others(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    withheld: &BTreeSet<OutPoint>,
) -> u64 {
    (coins(engine, session, id).into_iter())
        .filter(|(o, _)| !withheld.contains(o))
        .map(|(_, v)| v)
        .sum()
}

/// What shows as pending (the balances' withheld amount).
fn pending_amount(session: &NetworkSession, id: WalletId) -> u64 {
    balances(session, id).withheld
}

/// The coin list's reason for `coin`, which it lists.
fn listed_reason(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
) -> Option<ExclusionReason> {
    let filter = CoinFilter {
        include_locked: true,
        ..CoinFilter::default()
    };
    let rows = engine.block_on(session.utxos(id, filter)).unwrap();
    let row = (rows.iter().find(|r| r.outpoint == coin)).expect("the coin list shows the coin");
    assert!(!row.spendable || row.excluded.is_none(), "{row:?}");
    row.excluded
}

/// The transaction details' withheld outputs of `tx`.
fn detail_withheld(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    tx: &Transaction,
) -> Vec<OutPoint> {
    (engine
        .block_on(session.tx_detail_extras(id, tx.txid()))
        .unwrap())
    .withheld_outputs
}

/// Send's available amount: dash-qt's "Use available balance".
fn send_available(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> u64 {
    let fee = crate::send::FeeMode::PerKb(1_000);
    engine
        .block_on(session.max_spendable(id, CoinSource::Any, fee))
        .unwrap()
}

/// The coins Send's Max spends: a payment of the available amount with
/// the fee taken from it.
fn max_inputs(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    available: u64,
) -> BTreeSet<OutPoint> {
    let (_draft, prepared) = prepare(
        engine,
        session,
        id,
        CoinSource::Any,
        available,
        true,
        &Arc::new(Accept::default()),
    );
    (prepared.unwrap().summary().inputs.iter())
        .map(|i| i.outpoint)
        .collect()
}

/// The CoinJoin source: the mixing view's balance and the coins it may
/// take.
fn coinjoin_source(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (u64, BTreeSet<OutPoint>) {
    let view = engine.block_on(session.read_mix_view(id)).unwrap();
    let free = (view.coins.iter())
        .filter(|c| !c.coin.reserved && !c.coin.user_locked)
        .map(|c| c.coin.utxo.outpoint)
        .collect();
    (view.total, free)
}

/// The Core→Shielded and Platform-from-Core source (Core Max): the coins
/// the drain spends, and what it reports arriving.
fn core_source(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (BTreeSet<OutPoint>, u64, u64) {
    let s = Arc::clone(session);
    engine.block_on(async move {
        let fence = s.coin_fence(id).await.unwrap();
        let read = crate::platform::core_max::read(&s, &fence).await.unwrap();
        let total = read.coins.iter().map(|c| c.value).sum();
        let coins = read.coins.iter().map(|c| c.outpoint).collect();
        (coins, total, read.arriving_duffs)
    })
}

/// `coin` (worth `value`, with `withheld` the wallet's withheld coins) is
/// not spendable, counts in no balance surface, and shows as pending.
fn assert_withheld(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
    withheld: &BTreeSet<OutPoint>,
    when: &str,
) {
    let value = value_of(engine, session, id, coin);
    let pending: u64 = (coins(engine, session, id).into_iter())
        .filter(|(o, _)| withheld.contains(o))
        .map(|(_, v)| v)
        .sum();
    let rest = others(engine, session, id, withheld);
    assert!(
        !spendable_for(engine, session, id, coin, COIN / 100),
        "{when}: coin control refuses the coin"
    );
    eventually(&format!("{when}: the balances leave it out"), || {
        balances(session, id).total == rest
    });
    let b = balances(session, id);
    assert_eq!(b.total, rest, "{when}: Home total");
    assert_eq!(b.confirmed, rest, "{when}: confirmed bucket");
    assert_eq!(b.unconfirmed, 0, "{when}: unconfirmed bucket");
    assert_eq!(b.coinjoin, 0, "{when}: the CoinJoin balance");
    let available = send_available(engine, session, id);
    assert_eq!(available, rest, "{when}: Send's available amount");
    let max = max_inputs(engine, session, id, available);
    assert!(
        max.is_disjoint(withheld),
        "{when}: Send's Max spends none: {max:?}"
    );
    let (cj_total, cj_free) = coinjoin_source(engine, session, id);
    assert_eq!(cj_total, rest, "{when}: the CoinJoin source's balance");
    assert!(
        cj_free.is_disjoint(withheld),
        "{when}: the CoinJoin source takes none"
    );
    let (core_coins, core_total, arriving) = core_source(engine, session, id);
    assert!(
        core_coins.is_disjoint(withheld),
        "{when}: the Core→Shielded/Platform source takes none"
    );
    assert_eq!(
        core_total, rest,
        "{when}: the Core→Shielded/Platform amount"
    );
    assert_eq!(arriving, 0, "{when}: nothing reported arriving");
    assert!(value > 0);
    assert_eq!(
        pending_amount(session, id),
        pending,
        "{when}: shown as pending"
    );
    assert_eq!(
        listed_reason(engine, session, id, coin),
        Some(ExclusionReason::PendingUnproven),
        "{when}: the coin list's reason"
    );
}

/// `coin` is counted and spendable again, and nothing shows as pending.
fn assert_counted(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
    when: &str,
) {
    let all = others(engine, session, id, &BTreeSet::new());
    eventually(&format!("{when}: the balances count it"), || {
        balances(session, id).total == all && pending_amount(session, id) == 0
    });
    assert!(
        spendable_for(engine, session, id, coin, COIN / 100),
        "{when}: the coin is offered"
    );
    assert_eq!(
        listed_reason(engine, session, id, coin),
        None,
        "{when}: no reason"
    );
    assert_eq!(
        send_available(engine, session, id),
        all,
        "{when}: Send's available amount"
    );
}

fn s2(order: Order) {
    let (s, _) = beaten(order, false);
    let (engine, id) = (&s.engine, s.id);
    let withheld = BTreeSet::from([s.change]);
    assert!(
        live_in_history(&s.session, id, s.p.txid()),
        "P stays: E1 drops nothing"
    );
    assert_withheld(engine, &s.session, id, s.change, &withheld, "now");
    assert_eq!(
        detail_withheld(engine, &s.session, id, &s.p),
        vec![s.change],
        "P's detail"
    );
    let session = reopen(engine, Arc::clone(&s.session));
    assert_withheld(engine, &session, id, s.change, &withheld, "after a restart");
    assert_eq!(
        detail_withheld(engine, &session, id, &s.p),
        vec![s.change],
        "P's detail, restarted"
    );
}

#[test]
fn s2a_a_beaten_payments_change_is_withheld_when_the_chainlock_comes_in_the_blocks_op() {
    s2(Order::ChainLockInTheBlocksOp);
}

#[test]
fn s2b_a_beaten_payments_change_is_withheld_when_the_chainlock_comes_while_the_reconcile_waits() {
    s2(Order::ChainLockWhileTheReconcileWaits);
}

/// A payment built on the beaten payment's change before the restart: its
/// own change is withheld too.
#[test]
fn a_child_of_a_withheld_coin_is_withheld() {
    let (s, child) = beaten(Order::ChainLockInTheBlocksOp, true);
    let (c, c_change) = child.unwrap();
    let (engine, id) = (&s.engine, s.id);
    let withheld = BTreeSet::from([c_change]);
    assert!(
        !coins(engine, &s.session, id)
            .iter()
            .any(|(o, _)| *o == s.change),
        "the child spent P's change"
    );
    assert_withheld(engine, &s.session, id, c_change, &withheld, "now");
    assert_eq!(
        detail_withheld(engine, &s.session, id, &c),
        vec![c_change],
        "the child's detail"
    );
    let session = reopen(engine, Arc::clone(&s.session));
    assert_withheld(engine, &session, id, c_change, &withheld, "after a restart");
}

/// A payment restored at a load is withheld until a block mines it; then
/// its change counts at once, with no restart or rescan.
#[test]
fn a_slow_valid_payments_change_counts_once_it_is_mined() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    wait_stored(&session, id, &[p.txid()]);
    assert_counted(&engine, &session, id, change, "recorded this session");

    let session = reopen(&engine, session);
    assert_withheld(
        &engine,
        &session,
        id,
        change,
        &BTreeSet::from([change]),
        "restored",
    );
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    assert_counted(&engine, &session, id, change, "mined");
    assert!(
        detail_withheld(&engine, &session, id, &p).is_empty(),
        "P's detail"
    );
}

/// An InstantSend-locked payment restored at a load is not withheld (the
/// accepted residual).
#[test]
fn an_instantsend_locked_payment_is_not_withheld() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    instant_lock(&engine, &session, id, &p);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    wait_stored(&session, id, &[p.txid()]);
    wait_stored_locked(&session, id, p.txid());

    let session = reopen(&engine, session);
    assert_counted(&engine, &session, id, change, "restored");
}

/// Abandoning the beaten payment in the S2a state takes its pending change
/// away: nothing is left withheld and nothing counts it. What comes back of
/// the coins it spent, and when, is the rescan's (E2).
#[test]
fn abandoning_a_beaten_payment_takes_its_pending_change_away() {
    let (s, _) = beaten(Order::ChainLockInTheBlocksOp, false);
    let (engine, session, id) = (&s.engine, &s.session, s.id);
    let value = value_of(engine, session, id, s.change);
    assert_eq!(pending_amount(session, id), value, "pending before");
    let sess = Arc::clone(session);
    let txid = s.p.txid();
    engine
        .block_on(session.on_runtime(async move {
            let manager = sess.manager()?;
            sess.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert!(!live_in_history(session, id, s.p.txid()), "P is abandoned");
    eventually("the pending change goes", || {
        pending_amount(session, id) == 0
            && !coins(engine, session, id)
                .iter()
                .any(|(o, _)| *o == s.change)
    });
    assert!(
        !spendable_for(engine, session, id, s.change, COIN / 100),
        "never offered"
    );
}
