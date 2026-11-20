//! The balance after the wallet's own payment, before it is mined (#88): a
//! coin the payment spent counts in neither bucket, and its change counts
//! once. Driven through a real draft (selection, signing, reservation) and
//! the production wallet manager, fed the payment as dash-spv feeds a local
//! broadcast and its echo from a peer; the manager's events reach the hub as
//! SPV's event monitor delivers them.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, Transaction, TxIn, TxOut, Txid};
use dw_vault::{Credential, GrantPurpose};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::{
    BIRTH, balances, elsewhere, engine_wallet, instant_lock, mempool, mined, reopen,
    wait_stored_locked,
};
use crate::send::{CoinSource, Recipient};
use crate::session::NetworkSession;
use crate::{Engine, WalletBalances, WalletId};

const COIN: u64 = 100_000_000;
const COINS: usize = 12;
const FUNDED_AT: u32 = BIRTH + 1;
const TIP: u32 = BIRTH + 5;

/// Someone else's regtest address.
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

/// Accepts every broadcast and keeps what it was handed: dash-spv's own
/// injection is replayed by the test through [`mempool`].
#[derive(Default)]
struct Accept(Mutex<Vec<Transaction>>);

impl crate::send::TestBroadcaster for Accept {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async {})
    }
    fn broadcast(
        &self,
        tx: &Transaction,
        _first: bool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Txid, platform_wallet::PlatformWalletError>>
                + Send,
        >,
    > {
        self.0.lock().unwrap().push(tx.clone());
        let txid = tx.txid();
        Box::pin(async move { Ok(txid) })
    }
}

fn fresh_address(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> Address {
    engine
        .block_on(session.next_receive_address(id, None))
        .unwrap()
        .address
        .parse::<Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked()
}

/// One transaction from a coin the wallet does not own paying `value` to
/// each of `to`.
fn funding(seed: u8, to: &[Address], value: u64) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
            ..Default::default()
        }],
        output: (to.iter())
            .map(|a| TxOut {
                value,
                script_pubkey: a.script_pubkey(),
            })
            .collect(),
        special_transaction_payload: None,
    }
}

/// A transaction that is not the wallet's, so a block can hold it.
fn foreign() -> Transaction {
    Transaction {
        output: vec![TxOut {
            value: COIN,
            script_pubkey: elsewhere(),
        }],
        ..funding(6, &[], 0)
    }
}

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

/// Prepares (signs and reserves, never broadcasts) a payment of `amount`
/// from `source` through a real draft.
fn prepare(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    amount: u64,
    subtract_fee: bool,
    broadcaster: &Arc<Accept>,
) -> (
    Arc<crate::send::TxDraft>,
    Result<Arc<crate::send::PreparedTx>, crate::EngineError>,
) {
    let draft = session.new_tx_draft(id).unwrap();
    *draft.broadcaster.lock().unwrap() = Some(Arc::clone(broadcaster) as _);
    draft
        .set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount,
            subtract_fee_from_amount: subtract_fee,
            label: None,
            message: None,
        }])
        .unwrap();
    draft.set_source(source).unwrap();
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: amount + COIN,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let prepared = engine.block_on(draft.prepare(grant));
    (draft, prepared)
}

/// Prepares and broadcasts a payment of `amount` from `source` through a
/// real draft; returns the signed transaction.
fn send(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    amount: u64,
    subtract_fee: bool,
) -> Transaction {
    let broadcaster = Arc::new(Accept::default());
    let (draft, prepared) = prepare(
        engine,
        session,
        id,
        source,
        amount,
        subtract_fee,
        &broadcaster,
    );
    engine.block_on(draft.broadcast(prepared.unwrap())).unwrap();
    let sent = broadcaster.0.lock().unwrap();
    assert_eq!(sent.len(), 1);
    sent[0].clone()
}

/// The wallet's unspent coins, per funds account, as key-wallet holds them.
fn coins(engine: &Engine, session: &NetworkSession, id: WalletId) -> Vec<(OutPoint, u64)> {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        (info.core_wallet.accounts.all_funding_accounts().into_iter())
            .flat_map(|a| a.utxos.iter().map(|(o, u)| (*o, u.txout.value)))
            .collect()
    })
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

fn own_scripts(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> BTreeSet<dashcore::ScriptBuf> {
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        (info.core_wallet.accounts.all_funding_accounts().into_iter())
            .flat_map(|a| a.managed_account_type().all_addresses())
            .map(|a| a.script_pubkey())
            .collect()
    })
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

/// A wallet past its birth block holding [`COINS`] coins of one DASH each
/// paid by a transaction InstantSend-locked but not mined yet; returns it.
fn wallet_funded_unmined() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
) {
    let (dir, engine, session, id) = engine_wallet();
    mined(&engine, &session, id, FUNDED_AT, vec![foreign()]);
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let paid = funding(7, &to, COIN);
    mempool(&engine, &session, id, &paid);
    instant_lock(&engine, &session, id, &paid);
    let b = balances(&session, id);
    assert!(!b.catching_up);
    assert_eq!((b.confirmed, b.total), (12 * COIN, 12 * COIN));
    (dir, engine, session, id, paid)
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

/// Whether a payment from `coin` alone can be prepared (signed and
/// reserved); the reservation is dropped again.
fn spendable(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, coin: OutPoint) -> bool {
    let (_draft, prepared) = prepare(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN / 2,
        false,
        &Arc::new(Accept::default()),
    );
    prepared.is_ok_and(|p| (p.summary().inputs.iter()).any(|i| i.outpoint == coin))
}

/// Waits up to 20 s for `done`.
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("never: {what}");
}

fn synced(engine: &Engine, session: &NetworkSession, id: WalletId) -> u32 {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        wm.get_wallet_info(&id.0)
            .unwrap()
            .core_wallet
            .synced_height()
    })
}

fn in_history(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    session.hub.history.get(&id, &txid).is_some()
}

/// The wallet holds exactly `want`, each once, and its balance is their sum.
fn assert_holds(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    want: &BTreeSet<OutPoint>,
    state: &str,
) -> WalletBalances {
    let held = coins(engine, session, id);
    let got: BTreeSet<OutPoint> = held.iter().map(|(o, _)| *o).collect();
    assert_eq!(
        got.len(),
        held.len(),
        "{state}: an outpoint held twice: {held:?}"
    );
    assert_eq!(&got, want, "{state}: {held:?}");
    let b = balances(session, id);
    assert_eq!(
        b.total,
        held.iter().map(|(_, v)| v).sum::<u64>(),
        "{state}: {b:?}"
    );
    b
}

/// Another device of the wallet spends `paid`'s output `vout`, paying half
/// to someone else and the rest less a fee back to the wallet.
fn rival(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    vout: u32,
) -> Transaction {
    let coin = OutPoint::new(paid.txid(), vout);
    let owner = Address::from_script(
        &paid.output[vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let change = fresh_address(engine, session, id);
    let tx = super::spend_of(
        coin,
        vec![
            (elsewhere(), COIN / 2),
            (change.script_pubkey(), COIN / 2 - 1_000),
        ],
    );
    super::signed_by_wallet(engine, session, id, &owner, tx)
}

/// The scan reaches `height`, whose block holds `txs`, but the hub never
/// hears of it (the process ends first); the store does.
fn mined_unheard(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    height: u32,
    txs: Vec<Transaction>,
) {
    let txids: Vec<Txid> = txs.iter().map(|t| t.txid()).collect();
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let block = super::block_with(txs);
        wm.process_block_for_wallets(&block, block.block_hash(), height, &BTreeSet::from([id.0]))
            .await;
        wm.update_wallet_synced_height(&id.0, height);
    });
    wait_stored(session, id, &txids);
}

/// Waits for the wallet store to hold the records of `txids`: the persister
/// hears wallet events through its own adapter task.
fn wait_stored(session: &NetworkSession, id: WalletId, txids: &[Txid]) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    eventually("the store holds the transactions", || {
        persister.flush(id.0).unwrap();
        (txids.iter()).all(|t| {
            persister
                .get_core_tx_record(id.0, t)
                .is_ok_and(|r| r.is_some())
        })
    });
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

/// #88: a payment restored as the claimant of A and B loses to a
/// mined spend of B by another device of the wallet. key-wallet holds no
/// record of the payment, so its conflict sweep drops nothing: the engine
/// drops it, releases A and replays A's funding block, which the scan had
/// passed. A counts and can be spent, in the session and after a restart.
#[test]
fn a_coin_claimed_by_a_payment_a_mined_spend_beats_counts_again() {
    let (_dir, engine, session, id, (a, b), paid, payment) =
        payment_claiming_two_coins_after_a_restart();
    let rival = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    eventually("the payment dropped and the scan rewound", || {
        !in_history(&session, id, payment.txid()) && synced(&engine, &session, id) < TIP
    });
    assert_eq!(synced(&engine, &session, id), TIP - 1);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    let want = after_the_rival(&paid, b, &rival);
    let replayed = assert_holds(&engine, &session, id, &want, "replayed");
    assert!(spendable(&engine, &session, id, a), "A is spendable");

    let session = reopen(&engine, session);
    assert!(!in_history(&session, id, payment.txid()));
    let restarted = assert_holds(&engine, &session, id, &want, "after a restart");
    assert_eq!(restarted.total, replayed.total);
    assert_eq!(synced(&engine, &session, id), TIP + 1, "no replay at load");
    assert!(
        spendable(&engine, &session, id, a),
        "A is spendable after a restart"
    );
}

/// The same when the process ends before the hub hears of the rival's
/// block: the next load finds the payment beaten in the stored history,
/// drops it and replays A's funding block.
#[test]
fn a_coin_claimed_by_a_payment_beaten_before_a_restart_counts_again_after_it() {
    let (_dir, engine, session, id, (a, b), paid, payment) =
        payment_claiming_two_coins_after_a_restart();
    let rival = rival(&engine, &session, id, &paid, 1);
    mined_unheard(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    assert!(in_history(&session, id, payment.txid()));

    let session = reopen(&engine, session);
    assert!(!in_history(&session, id, payment.txid()));
    assert_eq!(synced(&engine, &session, id), TIP - 1);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    let want = after_the_rival(&paid, b, &rival);
    assert_holds(&engine, &session, id, &want, "replayed");
    assert!(spendable(&engine, &session, id, a), "A is spendable");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "after a second restart");
    assert_eq!(synced(&engine, &session, id), TIP + 1, "no replay at load");
    assert!(
        spendable(&engine, &session, id, a),
        "A is spendable after a second restart"
    );
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

/// The wallet's own output of `tx` (its change).
fn change_of(tx: &Transaction, own: &BTreeSet<dashcore::ScriptBuf>) -> OutPoint {
    let vout = (0..tx.output.len() as u32)
        .find(|v| own.contains(&tx.output[*v as usize].script_pubkey))
        .expect("with change");
    OutPoint::new(tx.txid(), vout)
}

/// key-wallet's copy of the wallet's coin `outpoint`.
fn utxo(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    outpoint: OutPoint,
) -> key_wallet::Utxo {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        (info.core_wallet.accounts.all_funding_accounts().into_iter())
            .find_map(|a| a.utxos.get(&outpoint).cloned())
            .expect("held")
    })
}

/// Writes `changeset`'s core part to wallet.sqlite as the SQLite persister
/// alone writes it (no engine wrapper).
fn store_raw(
    session: &NetworkSession,
    id: WalletId,
    core: platform_wallet::changeset::CoreChangeSet,
) {
    use platform_wallet::changeset::{PlatformWalletChangeSet, PlatformWalletPersistence};
    let persister = Arc::clone(&session.live().unwrap().persister);
    let changeset = PlatformWalletChangeSet {
        core: Some(core),
        ..Default::default()
    };
    persister.store(id.0, changeset).unwrap();
    persister.flush(id.0).unwrap();
}

fn abandon(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid) {
    let s = Arc::clone(session);
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert!(abandoned.contains(&txid));
}

/// A wallet that hit #88 on an earlier build has its spent coin X stored
/// unspent beside the payment's change. The load drops X (the payment
/// claims it), stores it spent, and abandoning the payment gives it back.
#[test]
fn a_store_written_before_the_fix_loads_the_spent_coin_once() {
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

/// The store keeps an output key-wallet did not credit as spent whatever
/// the verdict: one a block spent before it was seen (`ObservedSpent`) and
/// one of a transaction a block already beat (`Doomed`) do not come back
/// at the next load; a credited one does.
#[test]
fn outputs_with_any_credit_verdict_are_stored_spent() {
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

/// A beaten payment is dropped with its unmined child, which spent its
/// change: neither change counts, and the coin only the payment spent
/// does.
#[test]
fn a_beaten_payment_is_dropped_with_its_child() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
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
    let own = own_scripts(&engine, &session, id);
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change_of(&payment, &own)]),
        COIN / 10,
        false,
    );
    mempool(&engine, &session, id, &child);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let rival = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    eventually(
        "the payment and its child dropped, the scan rewound",
        || {
            !in_history(&session, id, payment.txid())
                && !in_history(&session, id, child.txid())
                && synced(&engine, &session, id) < TIP
        },
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![rival.clone()]);
    let want = after_the_rival(&paid, b, &rival);
    assert_holds(&engine, &session, id, &want, "replayed");

    let session = reopen(&engine, session);
    assert!(!in_history(&session, id, child.txid()));
    assert_holds(&engine, &session, id, &want, "after a restart");
}
