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
    Accept, COIN, COINS, FUNDED_AT, TIP, abandon, assert_holds, change_of, coins, eventually,
    foreign, fresh_address, funding, own_scripts, prepare, rival, send, spendable, store_raw,
    synced, utxo, wait_stored, wallet_funded_unmined,
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

/// A restored payment of A and B that a mined spend of B by another device
/// beats keeps A out (the engine leaves conflicts to key-wallet, which
/// holds no record of it) until the user abandons it. The abandon gives
/// back A, never B, which the winner spent.
#[test]
fn a_beaten_payment_keeps_its_other_coin_out_until_it_is_abandoned() {
    let (_dir, engine, session, id, (a, b), paid, payment) =
        payment_claiming_two_coins_after_a_restart();
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

fn sent_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    crate::tx_actions::load_sent(&session.live().unwrap().appdb, id)
        .0
        .into_keys()
        .collect()
}

/// The mark of a payment this app sent goes at the first load after it is
/// mined; a pending one's stays.
#[test]
fn a_payment_mark_goes_at_the_load_after_it_is_mined() {
    let (_dir, engine, session, id) = funded_wallet();
    let mined_one = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &mined_one);
    let pending = send(&engine, &session, id, CoinSource::Any, COIN, false);
    mempool(&engine, &session, id, &pending);
    assert_eq!(
        sent_marks(&session, id),
        BTreeSet::from([mined_one.txid(), pending.txid()])
    );
    mined(&engine, &session, id, TIP + 1, vec![mined_one]);

    let session = reopen(&engine, session);
    assert_eq!(sent_marks(&session, id), BTreeSet::from([pending.txid()]));
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
    assert_eq!(sent.get(&tx.txid()), Some(&coins));
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
    use dashcore::hashes::Hash;
    use platform_wallet::changeset::PlatformWalletPersistence;
    use rusqlite::OptionalExtension;
    session.live().unwrap().persister.flush(id.0).unwrap();
    assert!(outpoint.vout < 251);
    let key: Vec<u8> = (std::iter::once(32))
        .chain(outpoint.txid.to_byte_array())
        .chain([outpoint.vout as u8])
        .collect();
    let conn = rusqlite::Connection::open_with_flags(
        session.data_dir().join(crate::session::WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT spent FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![&id.0[..], key],
        |r| r.get::<_, bool>(0),
    )
    .optional()
    .unwrap()
        == Some(false)
}

/// A payment of A and B beaten on A by another device's spend mined after
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
        COIN * 3 / 2,
        false,
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

/// How a load misses the winner of [`beaten_payment_with_its_other_coin_back`].
#[derive(Clone, Copy, PartialEq)]
enum WinnerFault {
    /// The history load cannot read its record.
    HistoryRead,
    /// The read of the stored spends before the wallet loads cannot.
    StoredRead,
    /// Its stored row's wallet id has the wrong length, so listing skips
    /// it and no wallet's store load reads it. (A txid of the wrong length
    /// fails the store's own load, and the wallet does not open.)
    SkippedRow,
}

/// Moves the stored transaction row of `txid` from wallet id `from` to
/// wallet id `to`, as damage would: the store's own writes check the key.
fn rekey_stored_row(session: &NetworkSession, txid: Txid, from: &[u8], to: &[u8]) {
    use dashcore::hashes::Hash;
    let conn = rusqlite::Connection::open(session.data_dir().join(crate::session::WALLET_DB_FILE))
        .unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    let changed = conn
        .execute(
            "UPDATE core_transactions SET wallet_id = ?1 WHERE wallet_id = ?2 AND txid = ?3",
            rusqlite::params![to, from, txid.to_byte_array().as_slice()],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

/// The payment above, its winner missed at a wallet load: the load claims
/// nothing, so B stays held, counted and stored unspent; the next clean
/// load sees the payment beaten.
fn a_load_missing_the_winner_keeps_the_coin_back(fault: WinnerFault) {
    let (_dir, engine, session, id, b, held, winner) = beaten_payment_with_its_other_coin_back();
    engine.block_on(session.unload_wallet(id)).unwrap();
    let key = id.0;
    let skipped: Vec<u8> = key.iter().copied().chain([0]).collect();
    {
        let mut unreadable = session.hub.unreadable_record.lock().unwrap();
        match fault {
            WinnerFault::HistoryRead => unreadable.0 = Some(winner),
            WinnerFault::StoredRead => unreadable.1 = Some(winner),
            WinnerFault::SkippedRow => rekey_stored_row(&session, winner, &key, &skipped),
        }
    }
    engine.block_on(session.load_wallet(id)).unwrap();
    let hits = session
        .hub
        .unreadable_hits
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        hits,
        usize::from(fault != WinnerFault::SkippedRow),
        "the fault was hit"
    );
    assert_holds(&engine, &session, id, &held, "the winner unread");
    assert!(stored_unspent(&session, id, b), "the winner unread");
    if fault == WinnerFault::SkippedRow {
        rekey_stored_row(&session, winner, &skipped, &key);
    }

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &held, "a clean load after it");
    assert!(stored_unspent(&session, id, b), "a clean load after it");
}

#[test]
fn a_history_load_missing_the_winner_keeps_the_coin_a_beaten_payment_spent() {
    a_load_missing_the_winner_keeps_the_coin_back(WinnerFault::HistoryRead);
}

#[test]
fn a_stored_spends_read_missing_the_winner_keeps_the_coin_a_beaten_payment_spent() {
    a_load_missing_the_winner_keeps_the_coin_back(WinnerFault::StoredRead);
}

/// #170 case 7: a stored row whose key has the wrong length makes the
/// listing incomplete, not a load that misses the winner unnoticed.
#[test]
fn a_load_listing_a_skipped_row_keeps_the_coin_a_beaten_payment_spent() {
    a_load_missing_the_winner_keeps_the_coin_back(WinnerFault::SkippedRow);
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
    (crate::tx_actions::load_abandoned(&session.live().unwrap().appdb, id).0)
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
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
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
    assert!(!sent_marks(&session, id).contains(&payment.txid()));
}

/// The coins of `id`'s coin control list that are reserved.
fn reserved_coins(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> BTreeSet<OutPoint> {
    let rows = engine
        .block_on(session.utxos(id, crate::coins::CoinFilter::default()))
        .unwrap();
    rows.iter()
        .filter(|r| r.reserved)
        .map(|r| r.outpoint)
        .collect()
}

/// #170 case 7: a load that cannot read a payment's record, while it reads
/// the marks, holds the coins the payment's sent mark names. The funding
/// block credits A (no claim is made from incomplete evidence) and the
/// balance counts it, but coin control lists it reserved and no payment
/// can spend it. The next full read, here the next coin read once the
/// record reads again, restores the claim and lifts the hold, with no
/// restart.
#[test]
fn an_unread_payment_record_holds_its_coins_until_a_full_read() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    engine.block_on(session.unload_wallet(id)).unwrap();
    *session.hub.unreadable_always.lock().unwrap() = Some(payment.txid());
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(
        session.hub.evidence_missed(&id),
        Some(crate::history_ops::Evidence::Held([a].into()))
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "credited");
    assert!(
        reserved_coins(&engine, &session, id).contains(&a),
        "listed reserved"
    );
    assert!(!spendable(&engine, &session, id, a), "not spent");
    let all = assert_holds(&engine, &session, id, &held(&engine, &session, id), "held");
    assert!(all.total > 0, "the balance answers");

    *session.hub.unreadable_always.lock().unwrap() = None;
    let coins = held(&engine, &session, id);
    reserved_coins(&engine, &session, id);
    assert_eq!(
        session.hub.evidence_missed(&id),
        None,
        "a full read lifts it"
    );
    assert!(!held(&engine, &session, id).contains(&a), "claimed again");
    assert!(coins.contains(&a));
}

/// #170 case 7: a load that cannot read the payment marks refuses every
/// spend of the wallet (`evidence_unreadable`) and lists every coin
/// reserved; its balance and history still answer, and the automatic
/// CoinJoin pass waits without a status. The next spend once the marks
/// read again restores the claims and goes ahead, with no restart.
#[test]
fn unreadable_payment_marks_refuse_spends_until_a_full_read() {
    let (_dir, engine, session, id, payment, a, paid) = pending_payment_of_one_coin();
    let (scope, bad) = (id.to_string(), format!("sent:{}", paid.txid()));
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    appdb.set_setting(&scope, &bad, Some("zz")).unwrap();
    drop(appdb);
    let session = reopen(&engine, session);
    assert_eq!(
        session.hub.evidence_missed(&id),
        Some(crate::history_ops::Evidence::Unreadable)
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let coins = held(&engine, &session, id);
    assert!(coins.contains(&a), "no claim from incomplete evidence");
    assert_holds(&engine, &session, id, &coins, "the balance answers");
    assert!(
        engine
            .block_on(session.history_page(
                id,
                crate::HistoryQuery {
                    filter: Default::default(),
                    sort: crate::HistorySort::NewestFirst,
                    cursor: None,
                    limit: 10,
                }
            ))
            .is_ok(),
        "the history answers"
    );
    assert_eq!(
        reserved_coins(&engine, &session, id),
        coins,
        "every coin listed reserved"
    );
    let free = OutPoint::new(paid.txid(), 1);
    let (_draft, refused) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![free]),
        COIN / 2,
        false,
        &Arc::new(Accept::default()),
    );
    assert_eq!(refused.err().map(|e| e.code()), Some("evidence_unreadable"));
    let quiet = {
        let s = Arc::clone(&session);
        engine.block_on(session.on_runtime(async move {
            let fence = s.coin_fence(id).await?;
            s.pass_view(&fence).await.map(|v| v.is_none())
        }))
    };
    assert!(quiet.unwrap(), "the automatic pass waits");

    (session.live().unwrap().appdb)
        .set_setting(&scope, &bad, None)
        .unwrap();
    assert!(
        spendable(&engine, &session, id, free),
        "a full read lifts it"
    );
    assert_eq!(session.hub.evidence_missed(&id), None);
    assert!(!held(&engine, &session, id).contains(&a), "claimed again");
    assert!(sent_marks(&session, id).contains(&payment.txid()));
}
