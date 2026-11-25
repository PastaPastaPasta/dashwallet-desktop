//! The balance after the wallet's own payment, before it is mined (#88): a
//! coin the payment spent counts in neither bucket, and its change counts
//! once. Driven through a real draft (selection, signing, reservation) and
//! the production wallet manager, fed the payment as dash-spv feeds a local
//! broadcast and its echo from a peer; the manager's events reach the hub as
//! SPV's event monitor delivers them.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{Address, OutPoint, Transaction, Txid};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::own_spend_disk_tests::{
    COIN, COINS, FUNDED_AT, TIP, abandon, another_wallet_opening, assert_holds, change_of, coins,
    eventually, foreign, fresh_address, funding, own_scripts, rival, second_wallet, send,
    sent_marks, spendable, start_offline_spv, store_raw, synced, utxo, wait_stored,
    wallet_funded_unmined,
};
use super::{balances, engine_wallet, instant_lock, mempool, mined, reopen, wait_stored_locked};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{Engine, WalletBalances, WalletId};

/// A wallet holding [`COINS`] confirmed coins of one DASH each, mined at
/// [`FUNDED_AT`] and scanned to [`TIP`], and the transaction paying them.
fn funded_wallet_and_funding() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
) {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let paid = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let b = balances(&session, id);
    assert!(!b.catching_up);
    assert_eq!(
        (b.confirmed, b.unconfirmed, b.total),
        (12 * COIN, 0, 12 * COIN)
    );
    (dir, engine, session, id, paid)
}

fn funded_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>, WalletId) {
    let (dir, engine, session, id, _) = funded_wallet_and_funding();
    (dir, engine, session, id)
}

/// A peer relays `tx` again: dash-spv hands it to the wallet whether or not
/// it is still relevant (a spend whose coins are gone and which pays the
/// wallet nothing is not).
fn echo(engine: &Engine, session: &NetworkSession, id: WalletId, tx: &Transaction) {
    let tx = tx.clone();
    super::deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&tx, None).await;
        })
    });
}

/// What the wallet holds after `tx`, given what it held before: the coins
/// `tx` did not spend and the outputs of `tx` paying the wallet.
fn expected_after(
    before: &[(OutPoint, u64)],
    tx: &Transaction,
    own: &BTreeSet<dashcore::ScriptBuf>,
) -> (u64, u64) {
    let spent: BTreeSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    let kept: u64 = (before.iter())
        .filter(|(o, _)| !spent.contains(o))
        .map(|(_, v)| v)
        .sum();
    let change: u64 = (tx.output.iter())
        .filter(|o| own.contains(&o.script_pubkey))
        .map(|o| o.value)
        .sum();
    (kept, change)
}

/// The invariant: no spent coin is held or counted, no outpoint is held
/// twice, and the buckets add up to the coins kept plus the change, each
/// counted once.
fn assert_counted_once(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    before: &[(OutPoint, u64)],
    tx: &Transaction,
    state: &str,
) -> WalletBalances {
    let own = own_scripts(engine, session, id);
    let held = coins(engine, session, id);
    let (kept, change) = expected_after(before, tx, &own);
    let b = balances(session, id);
    let unique: BTreeSet<OutPoint> = held.iter().map(|(o, _)| *o).collect();
    assert_eq!(
        unique.len(),
        held.len(),
        "{state}: an outpoint held twice: {held:?}"
    );
    for input in &tx.input {
        assert!(
            !unique.contains(&input.previous_output),
            "{state}: the spent coin {} is still held; kept {kept}, change {change}: {b:?}",
            input.previous_output
        );
    }
    assert_eq!(
        b.confirmed + b.unconfirmed + b.immature + b.locked,
        b.total,
        "{state}: {b:?}"
    );
    assert_eq!(
        b.total,
        kept + change,
        "{state}: kept {kept}, change {change}: {b:?}"
    );
    assert_eq!(b.immature + b.locked, 0, "{state}: {b:?}");
    assert!(
        b.confirmed >= kept,
        "{state}: the coins kept are confirmed: {b:?}"
    );
    b
}

/// #88: before any block, the coins a payment spent are counted in neither
/// bucket and its change once; seen again from a peer, then InstantSend
/// locked, then after a restart, then mined, nothing changes the sum.
#[test]
fn an_own_payment_with_change_counts_each_coin_once_until_mined() {
    let (_dir, engine, session, id) = funded_wallet();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    assert!(
        tx.input.len() >= 3,
        "three coins at least: {}",
        tx.input.len()
    );

    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    mempool(&engine, &session, id, &tx);
    let echo = assert_counted_once(&engine, &session, id, &before, &tx, "peer echo");
    assert_eq!(echo, own, "the echo changes nothing");

    instant_lock(&engine, &session, id, &tx);
    let locked = assert_counted_once(&engine, &session, id, &before, &tx, "InstantSend lock");
    assert_eq!(locked.total, own.total);
    wait_stored_locked(&session, id, tx.txid());

    let session = reopen(&engine, session);
    let restarted = assert_counted_once(&engine, &session, id, &before, &tx, "after a restart");
    assert_eq!(restarted.total, own.total);
    mempool(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "echo after a restart");

    mined(&engine, &session, id, TIP + 1, vec![tx.clone()]);
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "mined");
    assert_eq!((b.confirmed, b.unconfirmed), (own.total, 0));
}

/// The same with a payment that has no change: everything it spent leaves
/// the balance at once.
#[test]
fn an_own_payment_without_change_removes_its_coins_at_once() {
    let (_dir, engine, session, id) = funded_wallet();
    let before = coins(&engine, &session, id);
    let coin = before[0].0;
    let tx = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN,
        true,
    );
    assert_eq!(tx.input.len(), 1);
    assert_eq!(tx.output.len(), 1, "no change");

    mempool(&engine, &session, id, &tx);
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    assert_eq!((b.confirmed, b.unconfirmed), (11 * COIN, 0));
    echo(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "peer echo");
    instant_lock(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "InstantSend lock");
}

/// A payment that spends a coin still unconfirmed itself: the coin leaves
/// the unconfirmed bucket and only the change is added.
#[test]
fn an_own_payment_of_an_unconfirmed_coin_counts_each_coin_once() {
    let (_dir, engine, session, id) = funded_wallet();
    let to = fresh_address(&engine, &session, id);
    let incoming = funding(8, std::slice::from_ref(&to), 3 * COIN);
    mempool(&engine, &session, id, &incoming);
    instant_lock(&engine, &session, id, &incoming);
    let before = coins(&engine, &session, id);
    let coin = OutPoint::new(incoming.txid(), 0);
    let tx = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN,
        false,
    );
    assert_eq!(tx.input[0].previous_output, coin);

    mempool(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    mempool(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "peer echo");
}

/// `dwcli` runs each command in a new process: the payment is stored
/// unmined and not InstantSend-locked, and the next command reads the
/// balance from the store.
#[test]
fn an_own_payment_stored_unmined_counts_each_coin_once_after_a_restart() {
    let (_dir, engine, session, id) = funded_wallet();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let session = reopen(&engine, session);
    let restarted = assert_counted_once(&engine, &session, id, &before, &tx, "after a restart");
    assert_eq!(restarted.total, own.total);
    mempool(&engine, &session, id, &tx);
    let echoed = assert_counted_once(&engine, &session, id, &before, &tx, "echo after a restart");
    assert_eq!(echoed.total, own.total);
}

/// #88: the wallet pays from coins it was just paid, before the block
/// paying them is scanned; `dwcli` exits after the send. In the next
/// process the scan reaches that block while the payment is still unmined:
/// a coin the payment spent must not be credited again.
#[test]
fn a_coin_spent_unmined_is_not_credited_again_by_its_funding_block_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let session = reopen(&engine, session);
    let restarted = assert_counted_once(&engine, &session, id, &before, &tx, "after a restart");
    assert_eq!(restarted.total, own.total);

    mined(&engine, &session, id, TIP, vec![paid]);
    let scanned = assert_counted_once(
        &engine,
        &session,
        id,
        &before,
        &tx,
        "the funding block scanned after a restart",
    );
    assert_eq!(scanned.total, own.total);

    mined(&engine, &session, id, TIP + 1, vec![tx.clone()]);
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "mined");
    assert_eq!(b.total, own.total);
}

/// The same within one process: the funding block is scanned after the
/// wallet's payment from its coins.
#[test]
fn a_coin_spent_unmined_is_not_credited_again_by_its_funding_block() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    mined(&engine, &session, id, TIP, vec![paid]);
    let scanned = assert_counted_once(&engine, &session, id, &before, &tx, "funding scanned");
    assert_eq!(scanned.total, own.total);
}

/// A rescan from below the funding block replays it while the wallet's
/// payment from its coins is unmined.
#[test]
fn a_rescan_does_not_credit_again_a_coin_spent_unmined() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let s = Arc::clone(&session);
    engine
        .block_on(session.on_runtime(async move {
            s.start_rescan(vec![id], FUNDED_AT - 1, |_| FUNDED_AT - 1)
                .await
        }))
        .unwrap();
    assert_counted_once(&engine, &session, id, &before, &tx, "rescan started");
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    let replayed = assert_counted_once(&engine, &session, id, &before, &tx, "funding replayed");
    assert_eq!(replayed.total, own.total);
}

/// A payment without change is no longer the wallet's business once its
/// coin is gone, so only the restored claim keeps the funding block from
/// crediting the coin again after a restart.
#[test]
fn a_coin_spent_unmined_without_change_is_not_credited_again_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let coin = OutPoint::new(paid.txid(), 0);
    let tx = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN,
        true,
    );
    assert_eq!(tx.output.len(), 1, "no change");
    mempool(&engine, &session, id, &tx);
    assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let session = reopen(&engine, session);
    assert_counted_once(&engine, &session, id, &before, &tx, "after a restart");
    mined(&engine, &session, id, TIP, vec![paid]);
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "funding scanned");
    assert_eq!((b.confirmed, b.unconfirmed), (11 * COIN, 0));
}

/// The claim restored for a payment goes when the user abandons it: the
/// coin it spent counts again once its funding block is scanned.
#[test]
fn abandoning_a_stored_payment_after_a_restart_frees_its_coins() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);

    let session = reopen(&engine, session);
    let (s, txid) = (Arc::clone(&session), tx.txid());
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert!(abandoned.contains(&txid));
    mined(&engine, &session, id, TIP, vec![paid]);
    let b = balances(&session, id);
    assert_eq!(
        (b.confirmed, b.unconfirmed, b.total),
        (12 * COIN, 0, 12 * COIN)
    );
}

fn in_history(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    session.hub.history.get(&id, &txid).is_some()
}

/// A wallet paid twelve coins (InstantSend-locked, unmined) that spends
/// two of them, A and B, in a payment never locked; restarted; the
/// funding block scanned after the restart while the payment claims both.
/// Returns A, B, the funding and the payment.
fn payment_claiming_two_coins_after_a_restart() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    (OutPoint, OutPoint),
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN * 3 / 2,
        false,
    );
    let inputs: BTreeSet<OutPoint> = payment.input.iter().map(|i| i.previous_output).collect();
    assert_eq!(inputs, BTreeSet::from([a, b]));
    assert_eq!(payment.output.len(), 2, "with change");
    mempool(&engine, &session, id, &payment);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let held: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(
        !held.contains(&a) && !held.contains(&b),
        "both claimed: {held:?}"
    );
    (dir, engine, session, id, (a, b), paid, payment)
}

/// What the wallet holds once the rival beat the payment: every funding
/// coin but B, and the rival's change; not the payment's change.
fn after_the_rival(paid: &Transaction, b: OutPoint, rival: &Transaction) -> BTreeSet<OutPoint> {
    (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .filter(|o| *o != b)
        .chain([OutPoint::new(rival.txid(), 1)])
        .collect()
}

/// Abandoning a stored payment after its coins' funding block was scanned
/// (the coins it claimed were kept out, not held): the abandon's replay
/// credits them.
#[test]
fn abandoning_a_stored_payment_after_its_funding_block_was_scanned_frees_its_coins() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let (s, txid) = (Arc::clone(&session), tx.txid());
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert!(abandoned.contains(&txid));
    assert_eq!(synced(&engine, &session, id), TIP - 1);
    mined(&engine, &session, id, TIP, vec![paid]);
    let b = balances(&session, id);
    assert_eq!(
        (b.confirmed, b.unconfirmed, b.total),
        (12 * COIN, 0, 12 * COIN)
    );
}

/// A wallet opened later in the session (dash-qt "Open Wallet") gets its
/// claims as soon as it is registered: a block processed for it before its
/// history is loaded does not credit the coins its payment spent.
#[test]
fn a_wallet_opened_later_claims_its_spent_coins_before_its_history_loads() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    wait_stored(&session, id, &[tx.txid()]);

    engine.block_on(session.unload_wallet(id)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_load.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the load's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    mined(&engine, &session, id, TIP, vec![paid]);
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "opened later");
    assert_eq!(b.total, own.total);
}

/// Two payments in a chain, the second spending the first's change, both
/// unmined across a restart: neither the funding block nor the first
/// payment's block credits a coin a later payment spent.
#[test]
fn chained_own_payments_count_each_coin_once_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let first = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &first);
    let own = own_scripts(&engine, &session, id);
    let change = (0..first.output.len() as u32)
        .find(|v| own.contains(&first.output[*v as usize].script_pubkey))
        .map(|v| OutPoint::new(first.txid(), v))
        .expect("the first payment has change");
    let second = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 10,
        false,
    );
    mempool(&engine, &session, id, &second);
    let own = own_scripts(&engine, &session, id);
    let second_change: BTreeSet<OutPoint> = (0..second.output.len() as u32)
        .filter(|v| own.contains(&second.output[*v as usize].script_pubkey))
        .map(|v| OutPoint::new(second.txid(), v))
        .collect();
    assert_eq!(second_change.len(), 1);
    let spent: BTreeSet<OutPoint> = first.input.iter().map(|i| i.previous_output).collect();
    let want: BTreeSet<OutPoint> = (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .filter(|o| !spent.contains(o))
        .chain(second_change)
        .collect();
    let sent = assert_holds(&engine, &session, id, &want, "both broadcast");

    let session = reopen(&engine, session);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "after a restart").total,
        sent.total
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "funding scanned").total,
        sent.total
    );
    mined(&engine, &session, id, TIP + 1, vec![first]);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "first mined").total,
        sent.total
    );
    mined(&engine, &session, id, TIP + 2, vec![second]);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "second mined").total,
        sent.total
    );
}

/// A rescan after a restart replays the funding block while the stored
/// payment is still unmined.
#[test]
fn a_rescan_after_a_restart_does_not_credit_again_a_coin_spent_unmined() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let session = reopen(&engine, session);
    let s = Arc::clone(&session);
    engine
        .block_on(session.on_runtime(async move {
            s.start_rescan(vec![id], FUNDED_AT - 1, |_| FUNDED_AT - 1)
                .await
        }))
        .unwrap();
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    let replayed = assert_counted_once(&engine, &session, id, &before, &tx, "funding replayed");
    assert_eq!(replayed.total, own.total);
}

/// The funding block scanned in a later process stores the coins the
/// payment claims as spent, not unspent: the process after it does not
/// hand them back beside the payment's change.
#[test]
fn a_coin_spent_unmined_stays_spent_two_restarts_later() {
    let (_dir, engine, session, id, (a, b), paid, payment) =
        payment_claiming_two_coins_after_a_restart();
    let own = own_scripts(&engine, &session, id);
    let change = (0..payment.output.len() as u32)
        .filter(|v| own.contains(&payment.output[*v as usize].script_pubkey))
        .map(|v| OutPoint::new(payment.txid(), v));
    let want: BTreeSet<OutPoint> = (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .filter(|o| *o != a && *o != b)
        .chain(change)
        .collect();
    assert_eq!(want.len(), COINS - 1);
    let scanned = assert_holds(&engine, &session, id, &want, "funding scanned");

    let session = reopen(&engine, session);
    let restarted = assert_holds(&engine, &session, id, &want, "a second restart");
    assert_eq!(restarted.total, scanned.total);
    mined(&engine, &session, id, TIP + 1, vec![payment.clone()]);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "mined").total,
        scanned.total
    );
    let session = reopen(&engine, session);
    assert_eq!(
        assert_holds(&engine, &session, id, &want, "a third restart").total,
        scanned.total
    );
}

/// The store holds the coin X a pending payment this app sent spends as
/// unspent beside the payment's change (a block credited it before the
/// claims were set). The load drops X (the payment claims it), stores it
/// spent, and abandoning the payment gives it back.
#[test]
fn a_store_holding_a_claimed_coin_unspent_loads_it_once() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let before = coins(&engine, &session, id);
    let x = OutPoint::new(paid.txid(), 0);
    let x_coin = utxo(&engine, &session, id, x);
    let tx = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![x]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    wait_stored(&session, id, &[tx.txid()]);
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            new_utxos: vec![x_coin],
            ..Default::default()
        },
    );

    let session = reopen(&engine, session);
    let loaded = assert_counted_once(&engine, &session, id, &before, &tx, "loaded");
    assert_eq!(loaded.total, own.total);
    let session = reopen(&engine, session);
    let again = assert_counted_once(&engine, &session, id, &before, &tx, "another restart");
    assert_eq!(again.total, own.total);

    abandon(&engine, &session, id, tx.txid());
    assert_eq!(synced(&engine, &session, id), FUNDED_AT - 1);
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    let all: BTreeSet<OutPoint> = before.iter().map(|c| c.0).collect();
    assert_eq!(
        assert_holds(&engine, &session, id, &all, "abandoned").total,
        12 * COIN
    );
    let session = reopen(&engine, session);
    assert_eq!(
        assert_holds(&engine, &session, id, &all, "abandoned, then a restart").total,
        12 * COIN
    );
}

/// A wallet opened later in the session whose funding block is processed
/// before its claims are set (block writes queued on the wallet manager's
/// lock): the coins the payment spent are dropped again once they are,
/// and stay dropped after a restart.
#[test]
fn a_wallet_opened_later_drops_a_spent_coin_credited_before_its_claims() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");
    wait_stored(&session, id, &[tx.txid()]);

    engine.block_on(session.unload_wallet(id)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_claims.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the load's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    mined(&engine, &session, id, TIP, vec![paid]);
    let credited: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(
        tx.input
            .iter()
            .all(|i| credited.contains(&i.previous_output)),
        "the block credited the spent coins before the claims"
    );
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "opened later");
    assert_eq!(b.total, own.total);

    let session = reopen(&engine, session);
    let b = assert_counted_once(&engine, &session, id, &before, &tx, "after a restart");
    assert_eq!(b.total, own.total);
}

/// Abandoning the stored payment after its funding block was scanned in a
/// later process stores the coins it spent unspent again: the restart
/// after the abandon loads them, before any replay.
#[test]
fn coins_freed_by_an_abandon_after_their_funding_block_load_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let all: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    abandon(&engine, &session, id, tx.txid());

    let session = reopen(&engine, session);
    assert!(
        synced(&engine, &session, id) < TIP,
        "the replay is still due"
    );
    let b = assert_holds(&engine, &session, id, &all, "abandoned, then a restart");
    assert_eq!(b.total, 12 * COIN);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert_eq!(
        assert_holds(&engine, &session, id, &all, "replayed").total,
        12 * COIN
    );
}

/// The store writes no output key-wallet did not credit, whatever the
/// verdict: one a block spent before it was seen (`ObservedSpent`) and one
/// of a transaction a block already beat (`Doomed`) do not come back at
/// the next load; a credited one does.
#[test]
fn outputs_with_any_credit_verdict_are_not_stored() {
    use platform_wallet::changeset::changeset::UtxoCreditVerdict;
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let (_dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..3)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let paid = funding(9, &to, COIN);
    let utxos: Vec<key_wallet::Utxo> = (0..3)
        .map(|v| {
            key_wallet::Utxo::new(
                OutPoint::new(paid.txid(), v as u32),
                paid.output[v].clone(),
                to[v].clone(),
                FUNDED_AT,
                false,
            )
        })
        .collect();
    let store = Arc::clone(&session.live().unwrap().store);
    store
        .store(
            id.0,
            PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    new_utxos: utxos.clone(),
                    utxo_credit_verdicts: [
                        (
                            utxos[1].outpoint,
                            UtxoCreditVerdict::ObservedSpent {
                                height: FUNDED_AT + 1,
                            },
                        ),
                        (utxos[2].outpoint, UtxoCreditVerdict::Doomed),
                    ]
                    .into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    store.flush(id.0).unwrap();
    let credited = BTreeSet::from([utxos[0].outpoint]);
    let stored: BTreeSet<OutPoint> = (store.load().unwrap().wallets[&id.0].wallet_info.accounts)
        .all_funding_accounts()
        .into_iter()
        .flat_map(|a| a.utxos.keys().copied())
        .collect();
    assert_eq!(stored, credited);
    drop(store);
    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &credited, "after a restart");
}

/// #88: a peer relays the payment after a restart, before its coins'
/// funding block is scanned. key-wallet holds none of the coins it spent,
/// so it records the relay with no inputs, and that record replaces the
/// stored one; the load after the next restart still claims them, from
/// the coins the app marked when it sent the payment.
#[test]
fn a_payment_relayed_after_a_restart_keeps_its_claims_across_the_next() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let before = coins(&engine, &session, id);
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    let own = assert_counted_once(&engine, &session, id, &before, &tx, "own broadcast");

    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &tx);
    assert_counted_once(
        &engine,
        &session,
        id,
        &before,
        &tx,
        "relayed after a restart",
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    let b = assert_counted_once(
        &engine,
        &session,
        id,
        &before,
        &tx,
        "the funding block scanned a restart after the relay",
    );
    assert_eq!(b.total, own.total);
}

/// Abandoning a restored payment abandons its restored child, which spent
/// its change and one more coin D: neither change counts, and every coin
/// either spent counts again.
#[test]
fn abandoning_a_restored_payment_abandons_its_restored_child() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let first = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &first);
    let change = change_of(&first, &own_scripts(&engine, &session, id));
    let spent: BTreeSet<OutPoint> = first.input.iter().map(|i| i.previous_output).collect();
    let funding: BTreeSet<OutPoint> = (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .collect();
    let d = *funding.iter().find(|o| !spent.contains(o)).unwrap();
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change, d]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[first.txid(), child.txid()]);

    let session = reopen(&engine, session);
    let s = Arc::clone(&session);
    let txid = first.txid();
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert_eq!(abandoned, BTreeSet::from([first.txid(), child.txid()]));
    mined(&engine, &session, id, TIP, vec![paid]);
    assert_holds(&engine, &session, id, &funding, "funding scanned");
    assert!(spendable(&engine, &session, id, d), "D is spendable");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &funding, "after a restart");
}

/// A pending spend of the wallet's coin this app did not send (another
/// device's, or one a peer relays) gives no claim at the next load: its
/// coin counts when its funding block is scanned, and goes once the spend
/// is mined.
#[test]
fn a_pending_spend_this_app_did_not_send_claims_nothing() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let elsewhere = rival(&engine, &session, id, &paid, 1);
    mempool(&engine, &session, id, &elsewhere);
    wait_stored(&session, id, &[elsewhere.txid()]);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let b = OutPoint::new(paid.txid(), 1);
    let held: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(held.contains(&b), "not claimed: {held:?}");
    mined(&engine, &session, id, TIP + 1, vec![elsewhere.clone()]);
    let want = after_the_rival(&paid, b, &elsewhere);
    assert_holds(&engine, &session, id, &want, "the spend mined");
}

/// A restored payment of A and B without change that a mined spend of B
/// by another device beats keeps A out (key-wallet holds no record of it,
/// and the engine drops only a beaten payment that pays the wallet, #170
/// case 3) until the user abandons it. The abandon gives back A, never B,
/// which the winner spent.
#[test]
fn a_beaten_payment_without_change_keeps_its_other_coin_out_until_it_is_abandoned() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        2 * COIN,
        true,
    );
    assert_eq!(payment.output.len(), 1, "no change");
    mempool(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let rival = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    let held: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(!held.contains(&a), "A stays out: {held:?}");
    assert!(in_history(&session, id, payment.txid()));

    abandon(&engine, &session, id, payment.txid());
    assert_eq!(synced(&engine, &session, id), TIP - 1);

    let session = reopen(&engine, session);
    let held: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(
        held.contains(&a) && !held.contains(&b),
        "restarted before the replay: A loads, B does not: {held:?}"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    let want = after_the_rival(&paid, b, &rival);
    assert_holds(&engine, &session, id, &want, "replayed after a restart");
    assert!(spendable(&engine, &session, id, a), "A is spendable");
    assert!(!spendable(&engine, &session, id, b), "B is not");
}

/// The mark of a payment this app sent, mined in a session that ended
/// before its ChainLock, goes at a load whose header chain holds its block
/// and the block of the wallet's ChainLock. Before that it stays, at a load
/// with no header known too; a pending one's stays.
#[test]
fn a_payment_mark_goes_at_a_load_whose_header_chain_proves_it_chainlocked() {
    let (_dir, engine, session, id) = funded_wallet();
    let mined_one = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &mined_one);
    let pending = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &pending);
    assert_eq!(
        sent_marks(&session, id),
        BTreeSet::from([mined_one.txid(), pending.txid()])
    );
    mined(&engine, &session, id, TIP + 1, vec![mined_one.clone()]);

    let session = reopen(&engine, session);
    assert_eq!(
        sent_marks(&session, id),
        BTreeSet::from([mined_one.txid(), pending.txid()])
    );
    let block = super::block_with(vec![mined_one.clone()]).block_hash();
    super::own_spend_disk_tests::chain_lock(&engine, &session, id, TIP + 1, block);
    let session = reopen(&engine, session);
    assert_eq!(
        sent_marks(&session, id),
        BTreeSet::from([mined_one.txid(), pending.txid()]),
        "no header known at the load"
    );
    engine.block_on(session.unload_wallet(id)).unwrap();
    super::own_spend_disk_tests::header_chain(&session, &[(TIP + 1, block)]);
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(sent_marks(&session, id), BTreeSet::from([pending.txid()]));
}

/// The wallet's own ChainLock at a mined payment's height names another
/// block (a reorg replaced the payment's, and the store keeps no reorg):
/// its mark stays.
#[test]
fn a_payment_mark_stays_when_the_chainlock_names_another_block() {
    let (_dir, engine, session, id) = funded_wallet();
    let payment = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &payment);
    mined(&engine, &session, id, TIP + 1, vec![payment.clone()]);
    let other = super::block_with(vec![foreign()]).block_hash();
    assert_ne!(other, super::block_with(vec![payment.clone()]).block_hash());
    super::own_spend_disk_tests::chain_lock(&engine, &session, id, TIP + 1, other);

    let session = reopen(&engine, session);
    assert!(sent_marks(&session, id).contains(&payment.txid()));
}

/// Another wallet's ChainLock of a mined payment's own block, applied while
/// the payment's wallet was closed: the wallet opened again in the session
/// keeps the payment's mark, as its own ChainLock is the only evidence.
#[test]
fn a_chainlock_another_wallet_applied_keeps_a_payment_mark() {
    let (_dir, engine, session, id) = funded_wallet();
    let payment = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &payment);
    mined(&engine, &session, id, TIP + 1, vec![payment.clone()]);
    let other = second_wallet(&engine, &session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let block = super::block_with(vec![payment.clone()]).block_hash();
    super::own_spend_disk_tests::chain_lock(&engine, &session, other, TIP + 1, block);
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(sent_marks(&session, id).contains(&payment.txid()));
}

/// #170 case 17 guard: a payment's mark names every coin it spends, all of
/// them the wallet's.
#[test]
fn a_payments_mark_names_each_coin_it_spends() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let coins = BTreeSet::from([OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1)]);
    let source = CoinSource::Outpoints(coins.iter().copied().collect());
    let tx = send(&engine, &session, id, source, COIN * 3 / 2, false);
    let (sent, read) = crate::tx_actions::load_sent(&session.live().unwrap().appdb, id);
    assert!(read);
    assert_eq!(sent.get(&tx.txid()).map(|m| &m.coins), Some(&coins));
    assert_eq!(
        sent[&tx.txid()].tx.as_ref(),
        Some(&tx),
        "the mark holds the payment"
    );
}

fn held(engine: &Engine, session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

/// Whether wallet.sqlite holds `outpoint` as an unspent coin of `id`. The
/// store keys a coin by its outpoint in bincode varint: the txid's length,
/// the txid, then the vout (one byte below 251).
fn stored_unspent(session: &NetworkSession, id: WalletId, outpoint: OutPoint) -> bool {
    stored_spent(session, id, outpoint) == Some(false)
}

/// wallet.sqlite's `spent` flag of `outpoint` for `id`; `None` without a row.
fn stored_spent(session: &NetworkSession, id: WalletId, outpoint: OutPoint) -> Option<bool> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    session.live().unwrap().persister.flush(id.0).unwrap();
    stored_spent_at(
        &session.data_dir().join(crate::session::WALLET_DB_FILE),
        id,
        outpoint,
    )
}

/// [`stored_spent`] read from the wallet store at `db`, with no session.
fn stored_spent_at(db: &std::path::Path, id: WalletId, outpoint: OutPoint) -> Option<bool> {
    use dashcore::hashes::Hash;
    use rusqlite::OptionalExtension;
    assert!(outpoint.vout < 251);
    let key: Vec<u8> = (std::iter::once(32))
        .chain(outpoint.txid.to_byte_array())
        .chain([outpoint.vout as u8])
        .collect();
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    conn.query_row(
        "SELECT spent FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![&id.0[..], key],
        |r| r.get::<_, bool>(0),
    )
    .optional()
    .unwrap()
}

/// A payment of A and B without change (one with change goes when it is
/// beaten, #170 case 3) beaten on A by another device's spend mined after
/// a restart, after the funding block; the next restart replays both
/// blocks, which credit B, stored unspent. Returns B, the coins then held
/// and the winner.
fn beaten_payment_with_its_other_coin_back() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    OutPoint,
    BTreeSet<OutPoint>,
    Txid,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        2 * COIN,
        true,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 0);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let held = held(&engine, &session, id);
    assert!(held.contains(&b) && !held.contains(&a), "{held:?}");
    eventually("B stored unspent", || stored_unspent(&session, id, b));
    assert!(sent_marks(&session, id).contains(&payment.txid()));
    (dir, engine, session, id, b, held, winner.txid())
}

/// The payment above, its winner's record unreadable at a wallet load
/// (the history load's read, or the read of the stored spends before the
/// wallet loads): the load claims nothing, so B stays held, counted and
/// stored unspent; the next clean load sees the payment beaten.
fn a_load_missing_the_winner_keeps_the_coin_back(stored_read: bool) {
    let (_dir, engine, session, id, b, held, winner) = beaten_payment_with_its_other_coin_back();
    engine.block_on(session.unload_wallet(id)).unwrap();
    {
        let mut unreadable = session.hub.unreadable_record.lock().unwrap();
        match stored_read {
            false => unreadable.0 = Some(winner),
            true => unreadable.1 = Some(winner),
        }
    }
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_holds(&engine, &session, id, &held, "the winner unread");
    assert!(stored_unspent(&session, id, b), "the winner unread");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &held, "a clean load after it");
    assert!(stored_unspent(&session, id, b), "a clean load after it");
}

#[test]
fn a_history_load_missing_the_winner_keeps_the_coin_a_beaten_payment_spent() {
    a_load_missing_the_winner_keeps_the_coin_back(false);
}

#[test]
fn a_stored_spends_read_missing_the_winner_keeps_the_coin_a_beaten_payment_spent() {
    a_load_missing_the_winner_keeps_the_coin_back(true);
}

/// A load that cannot read a stored record unrelated to the payment claims
/// nothing (as before #88, the funding block credits the coins the payment
/// spent again); the next clean load claims them back.
#[test]
fn a_load_with_an_unreadable_record_claims_nothing_until_a_clean_load() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let other = funding(8, &[fresh_address(&engine, &session, id)], COIN);
    mempool(&engine, &session, id, &other);
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN * 3 / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[other.txid(), payment.txid()]);

    engine.block_on(session.unload_wallet(id)).unwrap();
    *session.hub.unreadable_record.lock().unwrap() = (Some(other.txid()), Some(other.txid()));
    engine.block_on(session.load_wallet(id)).unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    let partial = held(&engine, &session, id);
    assert!(
        partial.contains(&a) && partial.contains(&b),
        "no claim: {partial:?}"
    );

    let session = reopen(&engine, session);
    let clean = held(&engine, &session, id);
    assert!(
        !clean.contains(&a) && !clean.contains(&b),
        "claimed again: {clean:?}"
    );
    assert_holds(&engine, &session, id, &clean, "a clean load");
}

/// A payment of the wallet's coin A, sent and stored unmined; returns
/// it with A and the funding transaction.
fn pending_payment_of_one_coin() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    OutPoint,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    (dir, engine, session, id, payment, a, paid)
}

fn try_abandon(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Result<BTreeSet<Txid>, crate::EngineError> {
    let s = Arc::clone(session);
    engine.block_on(session.on_runtime(async move {
        let manager = s.manager()?;
        s.abandon_inner(&manager, id, txid).await
    }))
}

fn abandoned_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    (crate::tx_actions::load_abandoned(&session.live().unwrap().appdb, id)
        .0
        .copies)
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

/// #170 case 7: an abandon writes its abandoned marks durably before it is
/// applied. When that write fails the abandon is refused with nothing
/// changed: the payment stays stored and pending and keeps its claim on A
/// across a restart, so A is never released while no mark says so.
#[test]
fn a_failed_abandoned_mark_write_refuses_the_abandon() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    session
        .hub
        .fail_abandon_mark
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    assert!(
        !session
            .hub
            .fail_abandon_mark
            .load(std::sync::atomic::Ordering::SeqCst),
        "the fault was hit"
    );
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    assert!(
        persister.get_core_tx_record(id.0, &txid).unwrap().is_some(),
        "the record stays stored"
    );
    drop(persister);
    assert!(
        (session.hub.history.get(&id, &txid)).is_some_and(|e| !e.is_abandoned()),
        "the payment stays pending"
    );
    assert!(abandoned_marks(&session, id).is_empty());

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(sent_marks(&session, id).contains(&txid));
}

/// The marks are durable before the sweep that removes the payment's
/// record: an abandon whose sweep fails after them (as a crash between the
/// two writes leaves it) loads abandoned at the next start, claims nothing,
/// and the funding block scanned again credits A.
#[test]
fn an_abandon_whose_sweep_fails_after_its_mark_loads_abandoned() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    session
        .hub
        .fail_abandon_sweep
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    assert!(
        !session
            .hub
            .fail_abandon_sweep
            .load(std::sync::atomic::Ordering::SeqCst),
        "the fault was hit"
    );
    assert_eq!(abandoned_marks(&session, id), BTreeSet::from([txid]));

    let session = reopen(&engine, session);
    assert!(
        (session.hub.history.get(&id, &txid)).is_some_and(|e| e.is_abandoned()),
        "the payment loads abandoned"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "its change is not credited",
    );
}

/// An abandoned payment relayed again unlocked by a peer is ignored. Its
/// record can still be stored again while its sent mark stands (the abandon
/// sweeps the record, not the mark), as a relay after a restart stores it.
/// A load whose abandoned marks cannot be read claims nothing: its coin
/// stays held and stored unspent, and its sent mark stays.
#[test]
fn a_load_with_an_unreadable_abandoned_mark_claims_nothing() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let record = (session.live().unwrap().persister)
        .get_core_tx_record(id.0, &payment.txid())
        .unwrap()
        .unwrap();
    abandon(&engine, &session, id, payment.txid());
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    assert!(
        (persister.get_core_tx_record(id.0, &payment.txid()).unwrap()).is_none(),
        "the abandon swept the record"
    );
    drop(persister);
    assert!(sent_marks(&session, id).contains(&payment.txid()));
    let relayed = payment.clone();
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            let result = wm.process_mempool_transaction(&relayed, None).await;
            assert!(
                !result.is_relevant,
                "the relay of the abandoned payment is ignored"
            );
        })
    });
    assert!(
        (session.live().unwrap().persister)
            .get_core_tx_record(id.0, &payment.txid())
            .unwrap()
            .is_none(),
        "the ignored relay stores nothing"
    );
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_none_or(|e| e.is_abandoned()),
        "the ignored relay is not live in the history"
    );
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            records: vec![record],
            ..Default::default()
        },
    );
    wait_stored(&session, id, &[payment.txid()]);
    eventually("A stored unspent", || stored_unspent(&session, id, a));
    let (scope, key) = (id.to_string(), format!("abandoned:{}", payment.txid()));
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    let (_, mark) = (appdb.settings_with_prefix(&scope, &key).unwrap().pop()).unwrap();
    appdb.set_setting(&scope, &key, Some("00")).unwrap();
    drop(appdb);

    let session = reopen(&engine, session);
    let partial = held(&engine, &session, id);
    assert!(partial.contains(&a), "not dropped: {partial:?}");
    assert_holds(&engine, &session, id, &partial, "the mark unread");
    assert!(stored_unspent(&session, id, a), "not stored spent");
    assert!(sent_marks(&session, id).contains(&payment.txid()));

    (session.live().unwrap().appdb)
        .set_setting(&scope, &key, Some(&mark))
        .unwrap();
    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&a));
    assert!(stored_unspent(&session, id, a));
    assert!(sent_marks(&session, id).contains(&payment.txid()));
}

fn paid_outputs(paid: &Transaction) -> BTreeSet<OutPoint> {
    (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .collect()
}

fn fail_the_next_sweep(session: &NetworkSession) {
    (session.hub.fail_abandon_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
}

/// An abandon whose sweep is not stored in the session: key-wallet dropped
/// the payment in memory, so its change C is neither held nor offered, and
/// the payment stays listed pending with Abandon offered. After a restart
/// the load stores the sweep: C stays out and A comes back.
#[test]
fn an_abandon_whose_sweep_fails_offers_no_change_in_the_session() {
    let (_dir, engine, session, id, payment, _a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    let c = change_of(&payment, &own_scripts(&engine, &session, id));
    let probe = |session: &Arc<NetworkSession>| {
        super::own_spend_disk_tests::spendable_for(&engine, session, id, c, COIN / 10)
    };
    assert!(probe(&session), "C isn't offered before the Abandon");
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    assert!(!held(&engine, &session, id).contains(&c), "C is held");
    assert!(!probe(&session), "C is offered");
    assert!(
        (session.hub.history.get(&id, &txid)).is_some_and(|e| !e.is_abandoned()),
        "listed pending in the session"
    );
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(extras.can_abandon, "{extras:?}");

    let session = reopen(&engine, session);
    assert!(
        (session.hub.history.get(&id, &txid)).is_some_and(|e| e.is_abandoned()),
        "abandoned after the restart"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "after a restart",
    );
}

/// Abandon again after a sweep that was not stored stores it: the record
/// and C are gone, and A comes back once its funding is scanned.
#[test]
fn abandoning_again_after_a_failed_sweep_stores_it() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, payment, _a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    let c = change_of(&payment, &own_scripts(&engine, &session, id));
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    assert_eq!(
        try_abandon(&engine, &session, id, txid).unwrap(),
        BTreeSet::from([txid])
    );
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    assert!(
        persister.get_core_tx_record(id.0, &txid).unwrap().is_none(),
        "the record is swept"
    );
    drop(persister);
    assert_eq!(stored_spent(&session, id, c), None, "C stays stored");
    assert!(
        (session.hub.history.get(&id, &txid)).is_some_and(|e| e.is_abandoned()),
        "listed abandoned"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "abandoned again, then scanned",
    );
}

/// The load sweeps only payments marked abandoned: a pending payment E
/// beside an abandoned one whose sweep was not stored keeps its record and
/// its change.
#[test]
fn the_load_sweeps_only_payments_marked_abandoned() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let pay = |coin| {
        send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![coin]),
            COIN / 2,
            false,
        )
    };
    let (d, e) = (pay(a), pay(b));
    mempool(&engine, &session, id, &d);
    mempool(&engine, &session, id, &e);
    wait_stored(&session, id, &[d.txid(), e.txid()]);
    let own = own_scripts(&engine, &session, id);
    let (cd, ce) = (change_of(&d, &own), change_of(&e, &own));
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, d.txid()).is_err());

    let session = reopen(&engine, session);
    assert!(
        (session.live().unwrap().persister)
            .get_core_tx_record(id.0, &d.txid())
            .unwrap()
            .is_none(),
        "D's record is swept"
    );
    assert!(
        !held(&engine, &session, id).contains(&cd),
        "D's change is held"
    );
    assert!(
        (session.hub.history.get(&id, &d.txid())).is_some_and(|x| x.is_abandoned()),
        "D loads abandoned"
    );
    assert!(
        (session.hub.history.get(&id, &e.txid())).is_some_and(|x| !x.is_abandoned()),
        "E stays pending"
    );
    let persister = Arc::clone(&session.live().unwrap().persister);
    assert!(
        persister
            .get_core_tx_record(id.0, &e.txid())
            .unwrap()
            .is_some(),
        "E's record stays stored"
    );
    drop(persister);
    assert!(
        held(&engine, &session, id).contains(&ce),
        "E's change stays"
    );
}

/// A payment abandoned and then mined anyway (Abandon is local) counts
/// again: the load leaves it, so its coins stay spent, its change stays and
/// it is listed confirmed after a restart.
#[test]
fn a_payment_abandoned_and_then_mined_is_left_by_the_load() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
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
    wait_stored(&session, id, &[p.txid()]);
    abandon(&engine, &session, id, p.txid());
    mined(
        &engine,
        &session,
        id,
        TIP + 1,
        vec![paid.clone(), p.clone()],
    );
    wait_stored(&session, id, &[p.txid()]);
    let change = change_of(&p, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let coins = held(&engine, &session, id);
    assert!(
        !coins.contains(&a) && !coins.contains(&b),
        "spent coins credited: {coins:?}"
    );
    assert!(coins.contains(&change), "its change is gone: {coins:?}");
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    assert!(
        (session.hub.history.get(&id, &p.txid()))
            .is_some_and(|e| e.context.block_info().is_some() && !e.is_abandoned()),
        "listed confirmed"
    );
}

/// The same for a payment abandoned and then relayed with its InstantSend
/// lock after a reload: the load leaves it.
#[test]
fn a_payment_abandoned_and_then_locked_is_left_by_the_load() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
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
    wait_stored(&session, id, &[p.txid()]);
    abandon(&engine, &session, id, p.txid());
    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &p);
    instant_lock(&engine, &session, id, &p);
    wait_stored_locked(&session, id, p.txid());
    let change = change_of(&p, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let coins = held(&engine, &session, id);
    assert!(!coins.contains(&a), "A credited: {coins:?}");
    assert!(coins.contains(&change), "its change is gone: {coins:?}");
    assert!(
        (session.hub.history.get(&id, &p.txid())).is_some_and(|e| !e.is_abandoned()),
        "listed abandoned"
    );
    assert!(
        (session.hub.history.get(&id, &p.txid())).is_some_and(|e| e.instant_locked),
        "its InstantSend lock is gone"
    );
}

/// A payment abandoned and then locked, whose Abandon mark's saved copy is
/// damaged, keeps the claim of its sent mark while the store holds it: after
/// a restart and a scan of its funding block, A is neither offered nor
/// counted.
#[test]
fn a_locked_payment_whose_abandon_mark_is_damaged_keeps_its_coin_out() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
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
    wait_stored(&session, id, &[p.txid()]);
    abandon(&engine, &session, id, p.txid());
    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &p);
    instant_lock(&engine, &session, id, &p);
    wait_stored_locked(&session, id, p.txid());
    (session.live().unwrap().appdb)
        .set_setting(
            &id.to_string(),
            &format!("abandoned:{}", p.txid()),
            Some("zz"),
        )
        .unwrap();

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    let coins = held(&engine, &session, id);
    assert!(!coins.contains(&a), "A is counted: {coins:?}");
}

/// D spends A and B, stored and pending, its Abandon marked but its sweep
/// not stored; E, stored in `context`, spends B too. Returns D, E, A and B.
fn half_done_beside_a_spender_of_b(
    context: key_wallet::transaction_checking::TransactionContext,
) -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
    OutPoint,
    OutPoint,
) {
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::transaction_router::TransactionType;
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let d = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN * 3 / 2,
        false,
    );
    mempool(&engine, &session, id, &d);
    wait_stored(&session, id, &[d.txid()]);
    let mut e = foreign();
    e.input[0].previous_output = b;
    let record = TransactionRecord::new(
        e.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
        TransactionType::Standard,
        TransactionDirection::Outgoing,
        Vec::new(),
        Vec::new(),
        0,
    );
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            records: vec![record],
            ..Default::default()
        },
    );
    wait_stored(&session, id, &[e.txid()]);
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, d.txid()).is_err());
    (dir, engine, session, id, d, e, a, b)
}

/// The load's step itself, on the store as it is (no open, so no startup
/// repair).
fn finish_now(session: &NetworkSession, id: WalletId) -> Result<(), crate::EngineError> {
    let live = session.live().unwrap();
    let db_path = session.data_dir().join(crate::session::WALLET_DB_FILE);
    crate::tx_actions::finish_marked_abandons(&live.persister, &live.appdb, &db_path, &[id])
        .map(drop)
}

pub(super) fn record_stored(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    (session.live().unwrap().persister)
        .get_core_tx_record(id.0, &txid)
        .unwrap()
        .is_some()
}

/// The load's sweep of D leaves B spent when a stored mined E spends it
/// too, and releases A. The store itself refuses that release (its
/// surviving-claims check).
#[test]
fn the_load_sweep_keeps_a_coin_a_stored_mined_payment_spends() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let mined =
        TransactionContext::InBlock(BlockInfo::new(TIP, dashcore::hashes::Hash::all_zeros(), 0));
    let (_dir, _engine, session, id, d, _e, a, b) = half_done_beside_a_spender_of_b(mined);
    finish_now(&session, id).unwrap();
    assert!(!record_stored(&session, id, d.txid()), "D is not swept");
    assert!(stored_unspent(&session, id, a), "A is not released");
    assert_eq!(stored_spent(&session, id, b), Some(true), "B was released");
}

/// The same with E stored pending: the store does not keep B for a pending
/// spender, the sweep's own check does.
#[test]
fn the_load_sweep_keeps_a_coin_a_stored_pending_payment_spends() {
    use key_wallet::transaction_checking::TransactionContext;
    let (_dir, _engine, session, id, d, _e, a, b) =
        half_done_beside_a_spender_of_b(TransactionContext::Mempool);
    finish_now(&session, id).unwrap();
    assert!(!record_stored(&session, id, d.txid()), "D is not swept");
    assert!(stored_unspent(&session, id, a), "A is not released");
    assert_eq!(stored_spent(&session, id, b), Some(true), "B was released");
}

/// E's record does not read back: the sweep cannot tell whether E spends
/// B, so it fails and changes nothing (the load fails).
#[test]
fn the_load_sweep_fails_when_another_stored_record_does_not_read_back() {
    use key_wallet::transaction_checking::TransactionContext;
    let (_dir, _engine, session, id, d, e, _a, b) =
        half_done_beside_a_spender_of_b(TransactionContext::Mempool);
    corrupt_record(&session, id, e.txid());
    assert!(finish_now(&session, id).is_err(), "the sweep went ahead");
    assert!(record_stored(&session, id, d.txid()), "D is not kept");
    assert_eq!(stored_spent(&session, id, b), Some(true), "B was released");
}

/// A wallet this start leaves closed is swept when it is opened, not at
/// the start: a sweep it cannot finish fails its Open only, the start opens
/// the other wallet, and Open again sweeps it.
#[test]
fn a_closed_wallets_unfinished_sweep_fails_only_its_open() {
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, payment.txid()).is_err());
    let other = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong".to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(super::BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    crate::tx_actions::finish_fault::arm(&session.data_dir().join(crate::session::WALLET_DB_FILE));

    let session = reopen(&engine, session);
    let states = session.wallet_load_states().unwrap();
    assert!(
        (states.iter()).any(|w| w.wallet_id == other && w.loaded),
        "the other wallet is not opened: {states:?}"
    );
    assert!((states.iter()).any(|w| w.wallet_id == id && !w.loaded));
    assert!(record_stored(&session, id, payment.txid()));
    assert!(
        engine.block_on(session.load_wallet(id)).is_err(),
        "opened without its sweep"
    );
    assert!(
        (session.wallet_load_states().unwrap().iter()).any(|w| w.wallet_id == id && !w.loaded),
        "loaded though its Open failed"
    );
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(
        !record_stored(&session, id, payment.txid()),
        "opened again without its sweep"
    );
}

fn corrupt_record(session: &NetworkSession, id: WalletId, txid: Txid) {
    alter_record(
        session,
        id,
        txid,
        "UPDATE core_transactions SET record_blob = X'00' WHERE wallet_id = ?1 AND txid = ?2",
    );
}

fn alter_record(session: &NetworkSession, id: WalletId, txid: Txid, sql: &str) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    session.live().unwrap().persister.flush(id.0).unwrap();
    let conn = rusqlite::Connection::open(session.data_dir().join(crate::session::WALLET_DB_FILE))
        .unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let changed = conn
        .execute(
            sql,
            rusqlite::params![&id.0[..], AsRef::<[u8]>::as_ref(&txid)],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

/// A half-done abandon whose mark's copy does not read back: the mark
/// still names the payment, so the load sweeps it and its change is not
/// credited.
#[test]
fn a_half_done_abandon_whose_mark_copy_does_not_read_back_is_swept() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("abandoned:{txid}"), Some("00"))
        .unwrap();

    let session = reopen(&engine, session);
    assert!(
        !record_stored(&session, id, txid),
        "the payment is not swept"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "its change is not credited",
    );
}

/// A half-done abandon whose payment has no stored record left (its
/// outputs stay) and whose mark's copy does not read back: the load
/// releases the coins its sent mark names.
#[test]
fn a_half_done_abandon_without_a_record_or_a_readable_copy_releases_its_coins() {
    let (_dir, engine, session, id, payment, a, _paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("abandoned:{txid}"), Some("00"))
        .unwrap();
    alter_record(
        &session,
        id,
        txid,
        "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
    );
    assert_eq!(stored_spent(&session, id, a), Some(true));
    finish_now(&session, id).unwrap();
    assert_eq!(
        stored_spent(&session, id, a),
        Some(false),
        "A was not released"
    );
}

/// A half-done abandon whose coins only its sent mark names (no stored
/// record, no readable copy).
fn half_done_named_by_its_sent_mark() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    OutPoint,
) {
    let (dir, engine, session, id, payment, a, _paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("abandoned:{txid}"), Some("00"))
        .unwrap();
    alter_record(
        &session,
        id,
        txid,
        "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
    );
    assert_eq!(stored_spent(&session, id, a), Some(true));
    (dir, engine, session, id, a)
}

/// A read of the sent marks that fails at the start fails the start and
/// changes nothing; the next start, with the read back, finishes the
/// abandon and releases A.
#[test]
fn a_failed_read_of_the_sent_marks_fails_the_start_until_it_reads() {
    let (_dir, engine, session, id, a) = half_done_named_by_its_sent_mark();
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(crate::DashNetwork::Regtest))
            .unwrap()
    );
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, "settings.read_sent:", stage);
    let failed =
        engine.block_on(engine.open_network(crate::DashNetwork::Regtest, super::offline()));
    assert!(failed.is_err(), "the start swept with nothing released");
    dw_appdb::fault::disarm(&path);
    let wallet_db = path.with_file_name(crate::session::WALLET_DB_FILE);
    assert_eq!(
        stored_spent_at(&wallet_db, id, a),
        Some(true),
        "the failed start released A"
    );

    let session = super::open(&engine);
    assert_eq!(
        stored_spent(&session, id, a),
        Some(false),
        "A was not released"
    );
}

/// A sent mark row whose key isn't text, and another whose value doesn't
/// parse, fail neither the start nor the abandon it finishes: A is released
/// from the intact mark.
#[test]
fn a_bad_sent_row_does_not_fail_the_start() {
    use dashcore::hashes::Hash;
    let (_dir, engine, session, id, a) = half_done_named_by_its_sent_mark();
    let conn = rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.execute(
        "INSERT INTO settings_kv (scope, key, value) VALUES (?1, CAST(X'73656e743aff' AS TEXT), '')",
        [id.to_string()],
    )
    .unwrap();
    let other = Txid::from_byte_array([0x42; 32]);
    conn.execute(
        "INSERT INTO settings_kv (scope, key, value) VALUES (?1, ?2, 'not an outpoint')",
        [id.to_string(), format!("sent:{other}")],
    )
    .unwrap();
    drop(conn);

    let session = reopen(&engine, session);
    assert_eq!(
        stored_spent(&session, id, a),
        Some(false),
        "A was not released"
    );
}

/// An abandoned mark row whose key isn't text fails neither the start nor
/// the half-done abandon beside it, which the start finishes.
#[test]
fn a_bad_abandoned_row_does_not_fail_the_start() {
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, payment.txid()).is_err());
    let conn = rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.execute(
        "INSERT INTO settings_kv (scope, key, value) VALUES (?1, CAST(X'6162616e646f6e65643aff' AS TEXT), '')",
        [id.to_string()],
    )
    .unwrap();
    drop(conn);

    let session = reopen(&engine, session);
    assert!(
        !record_stored(&session, id, payment.txid()),
        "the abandon is not finished"
    );
}

/// The way out when a half-done Abandon's own mark has a damaged key: the
/// start skips it, so the payment is pending again with Abandon offered;
/// abandoning it again finishes it as a normal Abandon (A comes back once
/// its funding is scanned, the change goes), also after a restart.
#[test]
fn a_half_done_abandon_whose_own_mark_key_is_damaged_can_be_abandoned_again() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    let c = change_of(&payment, &own_scripts(&engine, &session, id));
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    let conn = rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let key = format!("abandoned:{txid}");
    let damaged: Vec<u8> = (key.as_bytes()[..key.len() - 1].iter().copied())
        .chain([0xff])
        .collect();
    let changed = conn
        .execute(
            "UPDATE settings_kv SET key = CAST(?3 AS TEXT) WHERE scope = ?1 AND key = ?2",
            rusqlite::params![id.to_string(), key, damaged],
        )
        .unwrap();
    assert_eq!(changed, 1);
    drop(conn);

    let session = reopen(&engine, session);
    assert!(record_stored(&session, id, txid), "the start finished it");
    assert!(
        (session.hub.history.get(&id, &txid))
            .is_some_and(|e| e.context.block_info().is_none() && !e.is_abandoned()),
        "not listed pending"
    );
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(extras.can_abandon, "Abandon is not offered: {extras:?}");
    start_offline_spv(&engine, &session);
    engine
        .block_on(session.abandon_transaction(id, txid))
        .unwrap();
    assert!(
        !record_stored(&session, id, txid),
        "the Abandon did not sweep it"
    );
    assert_eq!(
        stored_spent(&session, id, a),
        Some(false),
        "A is not released"
    );
    assert_eq!(
        stored_spent(&session, id, c),
        None,
        "the change stays stored"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "abandoned again, then scanned",
    );

    let session = reopen(&engine, session);
    assert!(!record_stored(&session, id, txid), "back after a restart");
    assert_eq!(
        stored_spent(&session, id, c),
        None,
        "the change is back after a restart"
    );
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "then a restart",
    );
}

/// The way out of a half-done abandon that releases nothing (no stored
/// record, no readable copy, no sent mark), its coin's funding already
/// mined: Tools ▸ Repair's rescan from the wallet's birth gives A back
/// once it passes A's funding block.
#[test]
fn a_rescan_from_the_birth_frees_a_coin_a_half_done_abandon_could_not_name() {
    use super::own_spend_disk_tests::{FUNDED_AT, spendable};
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, FUNDED_AT + 1, vec![paid.clone()]);
    let a = OutPoint::new(paid.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let txid = payment.txid();
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, txid).is_err());
    let scope = id.to_string();
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    appdb
        .set_setting(&scope, &format!("abandoned:{txid}"), Some("00"))
        .unwrap();
    for prefix in ["sent:", "senttx:", "sentin:"] {
        appdb
            .set_setting(&scope, &format!("{prefix}{txid}"), None)
            .unwrap();
    }
    drop(appdb);
    alter_record(
        &session,
        id,
        txid,
        "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
    );

    let session = reopen(&engine, session);
    assert!(
        !record_stored(&session, id, txid),
        "the payment is not swept"
    );
    assert_eq!(stored_spent(&session, id, a), Some(true), "A is released");
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    start_offline_spv(&engine, &session);
    engine
        .block_on(session.rescan(crate::RescanFrom::WalletBirth))
        .unwrap();
    mined(&engine, &session, id, FUNDED_AT + 1, vec![paid.clone()]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "A is not credited"
    );
    eventually("A stored unspent", || {
        stored_spent(&session, id, a) == Some(false)
    });
    assert!(spendable(&engine, &session, id, a), "A is not offered");
    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&a),
        "A is gone after a restart"
    );
    assert!(
        spendable(&engine, &session, id, a),
        "A is not offered after a restart"
    );
}

/// Open Wallet finishes the abandon too: a wallet closed with a half-done
/// abandon and opened again in the session does not credit its change.
#[test]
fn opening_a_wallet_finishes_its_half_done_abandon() {
    let (_dir, engine, session, id, payment, _a, paid) = pending_payment_of_one_coin();
    let c = change_of(&payment, &own_scripts(&engine, &session, id));
    fail_the_next_sweep(&session);
    assert!(try_abandon(&engine, &session, id, payment.txid()).is_err());
    engine.block_on(session.unload_wallet(id)).unwrap();
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(stored_spent(&session, id, c), None, "C stays stored");
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_some_and(|x| x.is_abandoned()),
        "abandoned once opened"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(
        &engine,
        &session,
        id,
        &paid_outputs(&paid),
        "opened, then scanned",
    );
}

/// One payment's damaged abandoned mark doesn't drop the claims of the
/// others: a healthy pending payment's coin stays out of coin selection
/// and the balance after a restart and a scan of its funding block.
#[test]
fn a_bad_abandoned_mark_leaves_a_healthy_payments_coin_out() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let c = change_of(&payment, &own_scripts(&engine, &session, id));
    let conn = rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.execute(
        "INSERT INTO settings_kv (scope, key, value) VALUES (?1, CAST(X'6162616e646f6e65643aff' AS TEXT), '')",
        [id.to_string()],
    )
    .unwrap();
    drop(conn);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    let mut want = paid_outputs(&paid);
    want.remove(&a);
    want.insert(c);
    assert_holds(&engine, &session, id, &want, "A is counted");
}

/// Resend is never offered or done for a payment whose Abandon committed
/// its mark: one asked while the Abandon is between its mark and its sweep
/// waits for the Abandon (it holds the wallet manager through its sweep),
/// and is refused after a sweep that failed, also after a restart.
#[test]
fn a_payment_marked_abandoned_is_never_resent() {
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    let refused = |session: &Arc<NetworkSession>| {
        let extras = engine.block_on(session.tx_detail_extras(id, txid)).ok();
        assert!(extras.as_ref().is_none_or(|x| !x.can_resend), "{extras:?}");
        let resent = engine.block_on(session.resend_transaction(id, txid));
        assert!(
            matches!(
                resent,
                Err(crate::EngineError::TxActionRefused(
                    crate::tx_actions::TxActionRefusal::AlreadyAbandoned
                ))
            ),
            "{resent:?}"
        );
    };
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_abandon_sweep.lock().unwrap() = Some(Arc::clone(&pause));
    let abandoning = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        })
    };
    eventually("the abandon reached its sweep", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let resending = {
        let s = Arc::clone(&session);
        session
            .rt
            .spawn(async move { s.resend_transaction(id, txid).await })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!resending.is_finished(), "a resend ran inside the abandon");
    fail_the_next_sweep(&session);
    pause.release.notify_one();
    assert!(
        engine.block_on(abandoning).unwrap().is_err(),
        "the sweep was stored"
    );
    let resent = engine.block_on(resending).unwrap();
    assert!(
        matches!(
            resent,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::AlreadyAbandoned
            ))
        ),
        "{resent:?}"
    );
    refused(&session);

    let session = reopen(&engine, session);
    refused(&session);
}

/// An Activity Resend that checked a payment just before an Abandon
/// committed its mark hands nothing off: it reads the mark again, held
/// against the Abandon, before its bytes go out.
#[test]
fn an_abandon_during_an_activity_resend_stops_its_hand_off() {
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    let txid = payment.txid();
    start_offline_spv(&engine, &session);
    dash_spv::EventHandler::on_network_event(
        &*session.hub,
        &dash_spv::network::NetworkEvent::PeerConnected {
            address: "127.0.0.1:9".parse().unwrap(),
        },
    );
    let wire = Arc::new(std::sync::Mutex::new(Vec::new()));
    *session.hub.resend_wire.lock().unwrap() = Some(Arc::clone(&wire));
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_resend.lock().unwrap() = Some(Arc::clone(&pause));
    let resending = {
        let s = Arc::clone(&session);
        session
            .rt
            .spawn(async move { s.resend_transaction(id, txid).await })
    };
    eventually("the resend checked the payment", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst) || resending.is_finished()
    });
    assert!(!resending.is_finished(), "{:?}", engine.block_on(resending));
    try_abandon(&engine, &session, id, txid).unwrap();
    assert!(
        abandon_marked(&session, id, txid),
        "the mark was not written"
    );
    pause.release.notify_one();
    let resent = engine.block_on(resending).unwrap();
    assert!(
        matches!(
            resent,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::AlreadyAbandoned
            ))
        ),
        "{resent:?}"
    );
    assert!(wire.lock().unwrap().is_empty(), "handed off after the mark");
}

/// A restart ends dash-spv's rebroadcast of an abandoned payment: its
/// tracked broadcasts live in memory, and the wallet store hands
/// platform-wallet's load no sends to dispatch again, before or after the
/// Abandon.
#[test]
fn the_wallet_store_stages_no_sends_to_dispatch_again_at_load() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    let staged = |session: &Arc<NetworkSession>| {
        let state = session.live().unwrap().persister.load().unwrap();
        let wallet = state.wallets.get(&id.0).expect("the wallet loads");
        wallet.unconfirmed_outgoing_txs.len()
    };
    assert_eq!(staged(&session), 0, "a pending send is staged");
    try_abandon(&engine, &session, id, payment.txid()).unwrap();
    assert_eq!(staged(&session), 0, "an abandoned send is staged");
    let session = reopen(&engine, session);
    assert_eq!(
        staged(&session),
        0,
        "an abandoned send is staged after a restart"
    );
}

/// Remove unconfirmed transactions whose read of the sent marks fails says
/// so, rather than reporting nothing removed, and abandons nothing: also
/// across wallets, when a wallet read after the payment's fails.
#[test]
fn remove_unconfirmed_fails_when_the_sent_marks_do_not_read() {
    let (_dir, engine, session, id, payment, _a, _paid) = pending_payment_of_one_coin();
    start_offline_spv(&engine, &session);
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(&path, "settings.read_sent:", dw_appdb::fault::Stage::Before);
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    dw_appdb::fault::disarm(&path);
    assert!(removed.is_err(), "{removed:?}");
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_some_and(|e| !e.is_abandoned()),
        "the payment was abandoned"
    );
    assert!(
        !abandon_marked(&session, id, payment.txid()),
        "its mark was written"
    );

    let other = super::own_spend_disk_tests::second_wallet(&engine, &session);
    let order: Vec<WalletId> = (session.manager().unwrap().list_wallet_ids_blocking())
        .into_iter()
        .map(WalletId)
        .collect();
    assert_eq!(order, [id, other], "the payment's wallet is read first");
    *session.hub.fail_sent_read.lock().unwrap() = Some(other);
    let removed = engine.block_on(session.drop_unconfirmed(None));
    assert!(removed.is_err(), "all wallets: {removed:?}");
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_some_and(|e| !e.is_abandoned()),
        "all wallets: the payment was abandoned"
    );
    assert!(
        !abandon_marked(&session, id, payment.txid()),
        "all wallets: its mark was written"
    );
}

/// Whether the app database holds `txid`'s abandoned mark, matched by key
/// as `tx_facts` does.
fn abandon_marked(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    let key = format!("abandoned:{txid}");
    (session.live().unwrap().appdb)
        .settings_with_prefix_lossy(&id.to_string(), &key)
        .unwrap()
        .iter()
        .any(|(k, _)| *k == key)
}

/// Runs the wallet's reconcile to its end, as a block event starts it:
/// what can never confirm goes (#170 cases 3 and 4).
fn reconciled(engine: &Engine, session: &NetworkSession, id: WalletId) {
    engine.block_on(session.reconcile_duplicate_coins(id));
}

/// Whether the history shows `txid`, not abandoned.
fn live_in_history(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    (session.hub.history.get(&id, &txid)).is_some_and(|e| !e.is_abandoned())
}

/// Another device of the wallet spends `coins`, outputs of `paid`, paying
/// only someone else.
fn foreign_winner(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    coins: &[OutPoint],
) -> Transaction {
    let owners: Vec<Address> = (coins.iter())
        .map(|c| {
            let script = &paid.output[c.vout as usize].script_pubkey;
            Address::from_script(script, dashcore::Network::Regtest).unwrap()
        })
        .collect();
    let mut tx = super::spend_of(
        coins[0],
        vec![(super::elsewhere(), coins.len() as u64 * COIN - 1_000)],
    );
    tx.input = (coins.iter())
        .map(|c| dashcore::TxIn {
            previous_output: *c,
            ..Default::default()
        })
        .collect();
    super::signed_by_wallet_each(engine, session, id, &owners, tx)
}

/// The funding outputs but `spent`.
fn funding_but(paid: &Transaction, spent: &[OutPoint]) -> BTreeSet<OutPoint> {
    (0..paid.output.len() as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .filter(|o| !spent.contains(o))
        .collect()
}

/// #170 case 3: a payment restored at load, of A and B with change, that a
/// rival mined after the restart beat on A. The rival paying the wallet
/// (`to_wallet`) or only someone else, its inputs are known (the history
/// holds it, or key-wallet observed its block): the payment goes, its
/// change no longer counts and cannot be selected, and B comes back with
/// the rescan of its block, also after a restart. A is the rival's.
fn a_restored_payment_beaten_by_a_mined_rival_drops_its_change(to_wallet: bool) {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
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

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&change), "restored");
    let winner = match to_wallet {
        true => rival(&engine, &session, id, &paid, 0),
        false => foreign_winner(&engine, &session, id, &paid, &[a]),
    };
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "the beaten payment's change goes"
    );
    assert!(!live_in_history(&session, id, p.txid()));
    assert!(!spendable(&engine, &session, id, change));

    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = funding_but(&paid, &[a]);
    if to_wallet {
        want.insert(OutPoint::new(winner.txid(), 1));
    }
    assert_holds(&engine, &session, id, &want, "rescanned");
    assert!(spendable(&engine, &session, id, b), "B is free");
    assert!(!spendable(&engine, &session, id, a), "A is the rival's");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "after a restart");
    assert!(!spendable(&engine, &session, id, change));
    assert!(!live_in_history(&session, id, p.txid()));
}

#[test]
fn a_restored_payment_a_mined_rival_paying_the_wallet_beat_drops_its_change() {
    a_restored_payment_beaten_by_a_mined_rival_drops_its_change(true);
}

#[test]
fn a_restored_payment_a_mined_rival_paying_elsewhere_beat_drops_its_change() {
    a_restored_payment_beaten_by_a_mined_rival_drops_its_change(false);
}

/// How the history lists `txid`: each record's status kind and drop.
fn listed_as(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Vec<(crate::history::TxStatusKind, Option<crate::history::TxDrop>)> {
    let query = crate::history::HistoryQuery {
        filter: crate::history::HistoryFilter::default(),
        sort: crate::history::HistorySort::NewestFirst,
        cursor: None,
        limit: 100,
    };
    let page = engine.block_on(session.history_page(id, query)).unwrap();
    let txid = txid.to_string();
    (page.records.into_iter())
        .filter(|r| r.txid == txid)
        .map(|r| (r.status.kind, r.status.dropped))
        .collect()
}

/// A payment a mined rival beat, dropped by the reconcile, lists as a plain
/// abandoned one: no drop status and no notice, also after a restart.
#[test]
fn a_beaten_payments_drop_lists_as_plain_abandoned_with_no_notice() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
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
    wait_stored(&session, id, &[p.txid()]);
    let session = reopen(&engine, session);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "dropped");
    let plain = vec![(crate::history::TxStatusKind::Abandoned, None)];
    assert_eq!(listed_as(&engine, &session, id, p.txid()), plain);
    let notices = engine.block_on(session.dropped_notices(id)).unwrap();
    assert!(notices.is_empty(), "no notice: {notices:?}");

    let session = reopen(&engine, session);
    assert_eq!(
        listed_as(&engine, &session, id, p.txid()),
        plain,
        "after a restart"
    );
    let notices = engine.block_on(session.dropped_notices(id)).unwrap();
    assert!(notices.is_empty(), "no notice after a restart: {notices:?}");
}

/// #170 case 3 against a resend: a drop of a beaten payment waits for the
/// wallet's abandon gate, which a resend holds for reading through its
/// hand-off, and writes no mark until it is released (MG-10 §1.10). The
/// hand-off starts before the block, so the reconcile its event starts
/// waits too.
#[test]
fn a_drop_of_a_beaten_payment_waits_for_a_resend_hand_off() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
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
    wait_stored(&session, id, &[p.txid()]);
    let session = reopen(&engine, session);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    let gate = session.abandon_gate(id);
    let resend = engine.block_on(Arc::clone(&gate).read_owned());
    mined(&engine, &session, id, TIP + 1, vec![winner]);

    let s = Arc::clone(&session);
    let rt = engine.block_on(async { tokio::runtime::Handle::current() });
    let task = rt.spawn(async move { s.reconcile_duplicate_coins(id).await });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while gate.try_read().is_ok() && !task.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !task.is_finished(),
        "the drop ran through a resend's hand-off"
    );
    assert!(gate.try_read().is_err(), "the drop never reached the gate");
    assert!(live_in_history(&session, id, p.txid()));
    assert!(!abandon_marked(&session, id, p.txid()));

    drop(resend);
    engine.block_on(task).unwrap();
    assert!(!live_in_history(&session, id, p.txid()), "the payment goes");
    assert!(abandon_marked(&session, id, p.txid()));
}

/// Payment P of A with change and its child C of P's change and D, both
/// stored unmined, on twelve mined coins; restarted. Returns A, D, the
/// funding, P, C and C's change.
#[allow(clippy::type_complexity)]
fn payment_and_child_after_a_restart() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    (OutPoint, OutPoint),
    Transaction,
    (Transaction, Transaction, OutPoint),
) {
    let (dir, engine, session, id, paid) = funded_wallet_and_funding();
    let (a, d) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let p_change = change_of(&p, &own_scripts(&engine, &session, id));
    let c = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![p_change, d]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &c);
    let c_change = change_of(&c, &own_scripts(&engine, &session, id));
    wait_stored(&session, id, &[p.txid(), c.txid()]);

    let session = reopen(&engine, session);
    let held = held(&engine, &session, id);
    assert!(held.contains(&c_change) && !held.contains(&d), "{held:?}");
    (dir, engine, session, id, (a, d), paid, (p, c, c_change))
}

/// C is gone, its change neither counts nor can be selected, and D stays
/// out of both: the winner spent it.
fn assert_child_dropped(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    (c, c_change, d): (&Transaction, OutPoint, OutPoint),
    state: &str,
) {
    let held = held(engine, session, id);
    assert!(!held.contains(&c_change), "{state}: C's change: {held:?}");
    assert!(!held.contains(&d), "{state}: D: {held:?}");
    assert!(!live_in_history(session, id, c.txid()), "{state}: C");
    assert!(
        !spendable(engine, session, id, c_change),
        "{state}: C's change"
    );
    assert!(!spendable(engine, session, id, d), "{state}: D");
}

/// #170 case 4: P relayed again after the restart, then a winner paying
/// only someone else, mined, spends A and D: key-wallet sweeps P, and C,
/// which it holds no record of, goes too. D stays out, after a restart and
/// a rescan too.
#[test]
fn a_restored_child_of_a_swept_payment_goes_and_its_coin_the_winner_spent_stays_out() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    echo(&engine, &session, id, &p);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "P is swept");
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "swept");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");
    assert_holds(
        &engine,
        &session,
        id,
        &funding_but(&paid, &[a, d]),
        "rescanned",
    );
}

/// #170 case 4: as above, the winner spending only A. C goes with the
/// sweep's winner, mined, so its inputs are known: D, which nothing else
/// spends, comes back with the rescan of its block.
#[test]
fn a_restored_child_of_a_swept_payment_goes_and_its_other_coin_comes_back() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    echo(&engine, &session, id, &p);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, c.txid()), "C goes");
    assert!(!held(&engine, &session, id).contains(&c_change));

    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    let want = funding_but(&paid, &[a]);
    assert_holds(&engine, &session, id, &want, "rescanned");
    assert!(spendable(&engine, &session, id, d), "D is free");
    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "after a restart");
}

/// #170 case 4, the safety rule: a sweep whose winner is only
/// InstantSend-locked and that the history does not hold frees nothing,
/// as its inputs are unknown: C goes, and D stays out even when its
/// block is scanned again, and after a restart.
#[test]
fn a_child_of_a_payment_an_unknown_locked_winner_swept_keeps_its_coin_out() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    session
        .hub
        .apply_wallet_event(&platform_wallet::events::WalletEvent::TransactionsSwept {
            wallet_id: id.0,
            txids: vec![p.txid()],
            superseded_by: winner.txid(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: Default::default(),
            account_balances: Default::default(),
        });
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "swept");
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned again");
}

/// #170 case 4, S1: key-wallet sweeps P while another wallet is part-way
/// through its open, so the drop of C waits for that open. C goes when the
/// open ends, with no other reconcile, and D stays out, also after a
/// restart.
#[test]
fn a_child_of_a_payment_swept_while_another_wallet_opens_goes_when_the_open_ends() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    let (pause, opening) = another_wallet_opening(&engine, &session);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    session
        .hub
        .apply_wallet_event(&platform_wallet::events::WalletEvent::TransactionsSwept {
            wallet_id: id.0,
            txids: vec![p.txid()],
            superseded_by: winner.txid(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: Default::default(),
            account_balances: Default::default(),
        });
    reconciled(&engine, &session, id);
    assert!(
        live_in_history(&session, id, c.txid()),
        "C waits for the other wallet's open"
    );
    assert!(session.hub.deferred_drop_wallets().contains(&id));
    pause.release.notify_one();
    engine.block_on(opening).unwrap().unwrap();
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "the open ended");
    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
}

/// #170 case 4, S1: as above, but a fork the hub hears truncates the chain
/// while the drop of C waits: the sweep's winner may be gone, so C is not
/// dropped on it when the open ends.
#[test]
fn a_child_of_a_payment_swept_while_another_wallet_opens_stays_after_a_fork() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, _)) = payment_and_child_after_a_restart();
    let (pause, opening) = another_wallet_opening(&engine, &session);
    mined(&engine, &session, id, TIP + 1, vec![foreign()]);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    session
        .hub
        .apply_wallet_event(&platform_wallet::events::WalletEvent::TransactionsSwept {
            wallet_id: id.0,
            txids: vec![p.txid()],
            superseded_by: winner.txid(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: Default::default(),
            account_balances: Default::default(),
        });
    reconciled(&engine, &session, id);
    assert!(session.hub.deferred_drop_wallets().contains(&id));
    let heard = session.hub.truncations();
    super::deliver(&engine, &session, id, |wm| {
        Box::pin(async move {
            wm.truncate_above(TIP).unwrap();
        })
    });
    assert!(session.hub.truncations() > heard, "the hub heard the fork");
    pause.release.notify_one();
    engine.block_on(opening).unwrap().unwrap();
    assert!(
        live_in_history(&session, id, c.txid()),
        "C is not dropped on a sweep a fork may have undone"
    );
}

/// #170 case 4: the app stops after key-wallet's sweep of P is stored and
/// its marks dropped, before C goes. The next load finds C orphaned (no
/// row of P, which spent its coin) and drops it with its winner unknown:
/// D stays out, and so does C's change, also after another restart.
#[test]
fn a_child_orphaned_by_a_sweep_before_a_restart_goes_at_the_load() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            sweeps: vec![platform_wallet::changeset::changeset::SweepBatch {
                txids: vec![p.txid()],
                superseded_by: winner.txid(),
                winner_mined_height: Some(TIP + 1),
                released_outpoints: Vec::new(),
            }],
            ..Default::default()
        },
    );
    crate::tx_actions::forget_sent(&session.live().unwrap().appdb, id, &[p.txid()]).unwrap();

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "loaded");
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned again");
}

/// #170 case 4: C relayed between key-wallet's sweep of P and the
/// reconcile that drops C: key-wallet records C this session, and C goes
/// all the same.
#[test]
fn a_child_relayed_between_the_sweep_and_the_reconcile_goes() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    echo(&engine, &session, id, &p);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    {
        let gate = engine.block_on(session.hub.drop_gate.lock());
        mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
        echo(&engine, &session, id, &c);
        drop(gate);
    }
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "dropped");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
}

/// #170 case 4 with no relay: P and C stored, a winner of A that pays the
/// wallet mined after a restart, after the funding. P and C go, and D,
/// which only C spent, counts once its funding block is scanned again, and
/// after another restart.
#[test]
fn a_child_of_a_beaten_payment_frees_its_other_coin_after_the_funding_scan() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, d) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let p_change = change_of(&p, &own_scripts(&engine, &session, id));
    let c = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![p_change, d]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &c);
    wait_stored(&session, id, &[p.txid(), c.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, a.vout);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "P goes");
    assert!(!live_in_history(&session, id, c.txid()), "C goes");
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = funding_but(&paid, &[a]);
    want.insert(OutPoint::new(winner.txid(), 1));
    assert_holds(&engine, &session, id, &want, "funding scanned");
    assert!(spendable(&engine, &session, id, d), "D is free");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "after a restart");
}

/// A payment without change mined after a restart: key-wallet may hold no
/// record of it, and its block is the spend it observed of its coin. It is
/// not taken for beaten: it stays in the history.
#[test]
fn a_mined_payment_without_change_stays_in_the_history() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN,
        true,
    );
    assert_eq!(p.output.len(), 1, "no change");
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    reconciled(&engine, &session, id);
    assert!(live_in_history(&session, id, p.txid()), "the payment stays");
    assert!(!abandon_marked(&session, id, p.txid()));
}

/// #170 case 4 with the history's load incomplete: the sweep's winner is
/// mined and spends only A, yet nothing proves that no record the load
/// missed spends D, so C goes and D stays out, through a rescan too.
#[test]
fn a_child_dropped_while_the_history_load_is_incomplete_frees_nothing() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    echo(&engine, &session, id, &p);
    (session.hub.history).with_wallet(id, |h| h.load_incomplete = true);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "dropped");
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned again");
}

/// #170 case 4 with a lock key-wallet heard of for a transaction neither
/// it nor the history holds: the sweep's winner is mined and spends only
/// A, yet that lock may spend D, so C goes and D stays out, through a
/// restart and a rescan too.
#[test]
fn a_child_dropped_beside_an_unknown_lock_frees_nothing() {
    use dashcore::hashes::Hash;
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    echo(&engine, &session, id, &p);
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        let unknown = dashcore::Txid::from_byte_array([42; 32]);
        info.core_wallet
            .mark_instant_send_utxos(&unknown, &dashcore::InstantLock::default());
    });
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "dropped");

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");
}

/// #170 case 4: the holds of a drop cannot be read when the wallet opens.
/// The open fails, as for unreadable abandoned marks, rather than load
/// the wallet with D unclaimed; the next open holds D through a rescan.
#[test]
fn an_open_that_cannot_read_a_drops_holds_fails() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    session
        .hub
        .apply_wallet_event(&platform_wallet::events::WalletEvent::TransactionsSwept {
            wallet_id: id.0,
            txids: vec![p.txid()],
            superseded_by: winner.txid(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: Default::default(),
            account_balances: Default::default(),
        });
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "swept");

    engine.block_on(session.unload_wallet(id)).unwrap();
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, crate::tx_actions::DROPPED_HOLDS_FAULT, stage);
    let loaded = engine.block_on(session.load_wallet(id));
    dw_appdb::fault::disarm(&path);
    if loaded.is_ok() {
        mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
        assert!(
            !held(&engine, &session, id).contains(&d),
            "loaded without its holds: D"
        );
    }
    assert!(loaded.is_err(), "the open went ahead");

    engine.block_on(session.load_wallet(id)).unwrap();
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "loaded again");
}

/// #170 case 4: the app stops after a drop of C wrote its marks, before
/// its sweep. The open finishes the sweep without storing D unspent, and
/// every load holds D as the drop did: a rescan of its block does not
/// credit it, after another restart too.
#[test]
fn a_drop_cut_after_its_marks_keeps_its_held_coin_out_across_restarts() {
    let (_dir, engine, session, id, (_, d), paid, (_, c, c_change)) =
        payment_and_child_after_a_restart();
    let scope = id.to_string();
    let rows = [
        (
            format!("abandoned:{}", c.txid()),
            dashcore::consensus::encode::serialize_hex(&c),
        ),
        (format!("drop_holds:{}", c.txid()), d.to_string()),
    ];
    let writes: Vec<(&str, Option<&str>)> = (rows.iter())
        .map(|(k, v)| (k.as_str(), Some(v.as_str())))
        .collect();
    (session.live().unwrap().appdb)
        .set_settings_durable(&scope, &writes)
        .unwrap();

    let session = reopen(&engine, session);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "finished");
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");
    let session = reopen(&engine, session);
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "restarted");
}

/// Two rivals of a restored payment P of A and D with change: another
/// device's spend of D, InstantSend-locked and unmined, then after a
/// restart a mined spend of A paying only someone else. P goes, and D,
/// which the locked spend may still take, stays out: in the session, after
/// a restart, and through a rescan of its block.
#[test]
fn a_payment_two_rivals_beat_keeps_the_coin_a_locked_rival_spends_out() {
    let (_dir, engine, session, id, paid) = funded_wallet_and_funding();
    let (a, d) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, d]),
        COIN * 3 / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    wait_stored(&session, id, &[p.txid()]);

    let session = reopen(&engine, session);
    let locked = rival(&engine, &session, id, &paid, d.vout);
    mempool(&engine, &session, id, &locked);
    instant_lock(&engine, &session, id, &locked);
    wait_stored(&session, id, &[locked.txid()]);

    let session = reopen(&engine, session);
    let winner = foreign_winner(&engine, &session, id, &paid, &[a]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "P goes");
    let held_now = held(&engine, &session, id);
    assert!(!held_now.contains(&change), "P's change: {held_now:?}");
    assert!(!held_now.contains(&d), "D: {held_now:?}");

    let session = reopen(&engine, session);
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    let held_now = held(&engine, &session, id);
    assert!(!held_now.contains(&d), "rescanned: D: {held_now:?}");
    assert!(!spendable(&engine, &session, id, d), "D is not offered");
}

/// #611: what another loaded wallet settled can descend from a payment of
/// this one, so nothing is dropped while another wallet's history loaded
/// incomplete: the beaten payment stays, with its change, until that
/// wallet's history is whole again.
#[test]
fn nothing_is_dropped_while_another_wallet_loaded_incomplete() {
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
    let other = super::own_spend_disk_tests::second_wallet(&engine, &session);

    let session = reopen(&engine, session);
    (session.hub.history).with_wallet(other, |h| h.load_incomplete = true);
    let winner = rival(&engine, &session, id, &paid, 0);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    reconciled(&engine, &session, id);
    assert!(live_in_history(&session, id, p.txid()), "the payment stays");
    assert!(held(&engine, &session, id).contains(&change));

    (session.hub.history).with_wallet(other, |h| h.load_incomplete = false);
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "the payment goes");
    assert!(!held(&engine, &session, id).contains(&change));
}

/// #611 with another wallet part-way through its open: its history is not
/// read yet, so what it settled is unknown and nothing is dropped until
/// the open is done.
#[test]
fn nothing_is_dropped_while_another_wallet_is_opening() {
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
    let other = second_wallet(&engine, &session);

    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(other)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_load.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(other).await })
    };
    eventually("the open's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let winner = rival(&engine, &session, id, &paid, 0);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    reconciled(&engine, &session, id);
    assert!(live_in_history(&session, id, p.txid()), "the payment stays");
    assert!(held(&engine, &session, id).contains(&change));

    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    reconciled(&engine, &session, id);
    assert!(!live_in_history(&session, id, p.txid()), "the payment goes");
    assert!(!held(&engine, &session, id).contains(&change));
}

/// An Abandon in one wallet while another wallet is part-way through its
/// open is not refused as unreadable: only a stored record that did not
/// read is (#611). The drop's own check counts the open (above).
#[test]
fn abandoning_while_another_wallet_is_opening_is_not_refused_as_unreadable() {
    let (_dir, engine, session, id, _paid) = funded_wallet_and_funding();
    let p = send(&engine, &session, id, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let other = second_wallet(&engine, &session);

    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(other)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_load.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(other).await })
    };
    eventually("the open's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let (s, txid) = (Arc::clone(&session), p.txid());
    let abandoned = engine.block_on(session.on_runtime(async move {
        let manager = s.manager()?;
        s.abandon_inner(&manager, id, txid).await
    }));
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    assert!(
        abandoned.as_ref().is_ok_and(|a| a.contains(&txid)),
        "{abandoned:?}"
    );
}

/// A peer's claim T on a coin the scan hasn't found yet, whose input script
/// isn't push-only (its verdict may be wrong, #739), paying change to the
/// wallet; S pays from that change and the wallet's coin b. Returns b's
/// funding, T's funding, T and S.
fn payment_on_a_doubtful_claim(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (Transaction, Transaction, Transaction, Transaction) {
    let b_to = fresh_address(engine, session, id);
    let b_funding = funding(3, &[b_to], super::PAID);
    mined(
        engine,
        session,
        id,
        super::BIRTH - 40,
        vec![b_funding.clone()],
    );
    let (_, claim_funding, mut claim) = super::funding_and_spend(engine, session, id);
    claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
    mempool(engine, session, id, &claim);
    let b = OutPoint::new(b_funding.txid(), 0);
    let s = send(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1), b]),
        super::CHANGE + super::PAID / 2,
        false,
    );
    assert_eq!(s.input.len(), 2, "S spends the change and b");
    mempool(engine, session, id, &s);
    wait_stored(session, id, &[claim.txid(), s.txid()]);
    (b_funding, claim_funding, claim, s)
}

/// S is kept, and b, which it spends, stays held.
fn assert_kept_with_b(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    s: &Transaction,
    b: OutPoint,
    state: &str,
) {
    assert!(live_in_history(session, id, s.txid()), "{state}: S is kept");
    assert!(!spendable(engine, session, id, b), "{state}: b stays held");
}

/// #739's T8 with the drops in: the doubtful claim T, relayed again after a
/// restart, is swept by key-wallet when the scan finds its coin. That
/// sweep's winner is T's funding; what is built on T is #739's to judge,
/// so no drop takes S, and b stays held.
#[test]
fn a_swept_doubtful_claims_payment_is_left_to_the_claim_rule() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, claim_funding, claim, s) = payment_on_a_doubtful_claim(&engine, &session, id);
    let b = OutPoint::new(b_funding.txid(), 0);
    let session = reopen(&engine, session);
    echo(&engine, &session, id, &claim);
    mined(
        &engine,
        &session,
        id,
        super::BIRTH - 20,
        vec![claim_funding],
    );
    reconciled(&engine, &session, id);
    assert_kept_with_b(&engine, &session, id, &s, b, "swept");
    let session = reopen(&engine, session);
    assert_kept_with_b(&engine, &session, id, &s, b, "restarted");
}

/// The same claim dropped by the store's own check at an open: S, whose
/// parent then has no row, is left to #739 too, not dropped as orphaned.
#[test]
fn a_payment_on_a_claim_the_store_dropped_is_not_taken_as_orphaned() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, claim_funding, claim, s) = payment_on_a_doubtful_claim(&engine, &session, id);
    let b = OutPoint::new(b_funding.txid(), 0);
    let session = reopen(&engine, session);
    mined(
        &engine,
        &session,
        id,
        super::BIRTH - 20,
        vec![claim_funding],
    );
    let session = reopen(&engine, session);
    assert!(
        !live_in_history(&session, id, claim.txid()),
        "the store dropped the claim"
    );
    assert_kept_with_b(&engine, &session, id, &s, b, "loaded");
    let session = reopen(&engine, session);
    assert_kept_with_b(&engine, &session, id, &s, b, "restarted");
}

/// C dropped with its winner unknown, so D is held through a note; then
/// the claim rule records C as dropped through its own writer, as a sweep
/// does. PR A's notes are its own: D stays held after a restart and a
/// rescan of its block.
#[test]
fn an_empty_dropped_key_does_not_erase_a_drops_holds() {
    let (_dir, engine, session, id, (a, d), paid, (p, c, c_change)) =
        payment_and_child_after_a_restart();
    let winner = foreign_winner(&engine, &session, id, &paid, &[a, d]);
    session
        .hub
        .apply_wallet_event(&platform_wallet::events::WalletEvent::TransactionsSwept {
            wallet_id: id.0,
            txids: vec![p.txid()],
            superseded_by: winner.txid(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: Default::default(),
            account_balances: Default::default(),
        });
    reconciled(&engine, &session, id);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "dropped");
    let dead =
        crate::tx_actions::Dead::from([(c.txid(), crate::history::DropCause::InvalidSource)]);
    crate::tx_actions::forget_swept(&session.live().unwrap().appdb, id, &[], &dead, &[], &[])
        .unwrap();

    let session = reopen(&engine, session);
    mined(&engine, &session, id, FUNDED_AT, vec![paid]);
    assert_child_dropped(&engine, &session, id, (&c, c_change, d), "rescanned");
}
