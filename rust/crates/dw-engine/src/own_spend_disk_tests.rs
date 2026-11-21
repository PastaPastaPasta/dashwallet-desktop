//! What the wallet store holds for the coins of the wallet's own unmined
//! payments (#88) across a restart, after an abandon, a beating spend, a
//! late wallet open or a credit verdict, through the production wallet
//! manager and store. Depends on nothing #88 added, so the same tests run
//! against a build without it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{Address, InstantLock, OutPoint, Transaction, TxIn, TxOut, Txid};
use dw_vault::{Credential, GrantPurpose};

use super::{BIRTH, balances, elsewhere, engine_wallet, instant_lock, mempool, mined, reopen};
use crate::send::{CoinSource, Recipient};
use crate::session::NetworkSession;
use crate::{Engine, WalletBalances, WalletId};

pub(super) const COIN: u64 = 100_000_000;
pub(super) const COINS: usize = 12;
pub(super) const FUNDED_AT: u32 = BIRTH + 1;
pub(super) const TIP: u32 = BIRTH + 5;

/// Someone else's regtest address.
pub(super) const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

/// Accepts every broadcast and keeps what it was handed: dash-spv's own
/// injection is replayed by the test through [`mempool`].
#[derive(Default)]
pub(super) struct Accept(pub(super) Mutex<Vec<Transaction>>);

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

pub(super) fn fresh_address(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> Address {
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
pub(super) fn funding(seed: u8, to: &[Address], value: u64) -> Transaction {
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
pub(super) fn foreign() -> Transaction {
    Transaction {
        output: vec![TxOut {
            value: COIN,
            script_pubkey: elsewhere(),
        }],
        ..funding(6, &[], 0)
    }
}

/// Prepares (signs and reserves, never broadcasts) a payment of `amount`
/// from `source` through a real draft.
pub(super) fn prepare(
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
    prepare_with(
        engine,
        session,
        id,
        source,
        amount,
        subtract_fee,
        broadcaster,
        |_| {},
    )
}

/// [`prepare`], with `configure` applied to the draft before it is signed.
#[allow(clippy::too_many_arguments)]
fn prepare_with(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    amount: u64,
    subtract_fee: bool,
    broadcaster: &Arc<Accept>,
    configure: impl FnOnce(&crate::send::TxDraft),
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
    configure(&draft);
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
pub(super) fn send(
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
pub(super) fn coins(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> Vec<(OutPoint, u64)> {
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

pub(super) fn own_scripts(
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

/// A wallet past its birth block holding [`COINS`] coins of one DASH each
/// paid by a transaction InstantSend-locked but not mined yet; returns it.
pub(super) fn wallet_funded_unmined() -> (
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

/// Whether a payment from `coin` alone can be prepared (signed and
/// reserved); the reservation is dropped again.
pub(super) fn spendable(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
) -> bool {
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
pub(super) fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("never: {what}");
}

pub(super) fn synced(engine: &Engine, session: &NetworkSession, id: WalletId) -> u32 {
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

/// The wallet holds exactly `want`, each once, and its balance is their sum.
pub(super) fn assert_holds(
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
pub(super) fn rival(
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

/// Waits for the wallet store to hold the records of `txids`: the persister
/// hears wallet events through its own adapter task.
pub(super) fn wait_stored(session: &NetworkSession, id: WalletId, txids: &[Txid]) {
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

/// The wallet's own output of `tx` (its change).
pub(super) fn change_of(tx: &Transaction, own: &BTreeSet<dashcore::ScriptBuf>) -> OutPoint {
    let vout = (0..tx.output.len() as u32)
        .find(|v| own.contains(&tx.output[*v as usize].script_pubkey))
        .expect("with change");
    OutPoint::new(tx.txid(), vout)
}

/// key-wallet's copy of the wallet's coin `outpoint`.
pub(super) fn utxo(
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
pub(super) fn store_raw(
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

pub(super) fn abandon(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid) {
    let s = Arc::clone(session);
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert!(abandoned.contains(&txid));
}

fn held(engine: &Engine, session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

fn outputs(tx: &Transaction) -> BTreeSet<OutPoint> {
    (0..tx.output.len() as u32)
        .map(|v| OutPoint::new(tx.txid(), v))
        .collect()
}

/// The wallet's payment, InstantSend-locked: the lock is stored apart from
/// its record, which loads as locked. After a restart, Abandon and Remove
/// Unconfirmed both refuse it, and the coins it spent stay spent.
#[test]
fn a_payment_locked_apart_from_its_record_is_not_abandoned_after_a_restart() {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    wait_stored(&session, id, &[tx.txid()]);
    let lock = InstantLock {
        txid: tx.txid(),
        ..InstantLock::default()
    };
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            instant_locks_for_non_final_records: [(tx.txid(), lock)].into(),
            ..Default::default()
        },
    );

    let session = reopen(&engine, session);
    let record = (session.live().unwrap().persister)
        .get_core_tx_record(id.0, &tx.txid())
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            record.context,
            key_wallet::transaction_checking::TransactionContext::InstantSend(_)
        ),
        "the record loads as locked: {:?}",
        record.context
    );
    let spent: BTreeSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    assert!(held(&engine, &session, id).is_disjoint(&spent));
    engine.block_on(session.start_spv()).unwrap();
    eventually("SPV runs", || {
        session.spv_state().unwrap() == crate::SpvState::Running
    });

    let abandoned = engine.block_on(session.abandon_transaction(id, tx.txid()));
    assert!(
        matches!(
            abandoned,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::InstantLocked
            ))
        ),
        "{abandoned:?}"
    );
    let dropped = engine.block_on(session.drop_unconfirmed(Some(id))).unwrap();
    assert_eq!(dropped, 0);
    assert!(held(&engine, &session, id).is_disjoint(&spent));

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).is_disjoint(&spent),
        "the coins it spent stay spent after a restart"
    );
}

/// A restored payment of A and B; after a restart, before the block
/// funding them is scanned, another device's spend of B paying only
/// someone else is mined. Abandoning the payment frees A only: after the
/// next restart B is neither counted nor offered.
#[test]
fn abandoning_a_payment_a_foreign_spend_beat_keeps_its_coin_spent_after_a_restart() {
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
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    let owner =
        Address::from_script(&paid.output[1].script_pubkey, dashcore::Network::Regtest).unwrap();
    let winner = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(b, vec![(elsewhere(), COIN - 1_000)]),
    );
    mined(&engine, &session, id, TIP, vec![winner.clone()]);
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&b);
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    mined(&engine, &session, id, TIP, vec![winner]);
    assert_holds(&engine, &session, id, &want, "replayed after a restart");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
    assert!(spendable(&engine, &session, id, a), "A is");
}

/// A restored payment whose change two restored children spend (another
/// device's beside this app's): abandoning the payment abandons both, and
/// once the funding block is scanned the wallet holds its funding coins
/// alone, after a restart too.
#[test]
fn abandoning_a_restored_payment_abandons_both_children_of_its_change() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let first = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &first);
    let change = change_of(&first, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 4,
        false,
    );
    mempool(&engine, &session, id, &child);
    let value = first.output[change.vout as usize].value;
    let owner = Address::from_script(
        &first.output[change.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let back = fresh_address(&engine, &session, id);
    let other = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(
            change,
            vec![
                (elsewhere(), value / 2),
                (back.script_pubkey(), value / 2 - 1_000),
            ],
        ),
    );
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[first.txid(), child.txid(), other.txid()]);

    let session = reopen(&engine, session);
    let s = Arc::clone(&session);
    let txid = first.txid();
    let abandoned = engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
        .unwrap();
    assert_eq!(
        abandoned,
        BTreeSet::from([first.txid(), child.txid(), other.txid()])
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let funding = outputs(&paid);
    assert_holds(&engine, &session, id, &funding, "funding scanned");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &funding, "after a restart");
}

/// A wallet opened later in the session, whose funding block is scanned
/// while it opens: the coins its pending payment spent are not stored
/// unspent, so the restart after loads neither.
#[test]
fn a_wallet_opened_later_stores_no_coin_its_payment_spent_unspent() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let tx = send(&engine, &session, id, CoinSource::Any, COIN * 5 / 2, false);
    mempool(&engine, &session, id, &tx);
    wait_stored(&session, id, &[tx.txid()]);
    let spent: BTreeSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();

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
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    assert!(held(&engine, &session, id).is_disjoint(&spent));

    let session = reopen(&engine, session);
    let mut want: BTreeSet<OutPoint> = outputs(&paid).difference(&spent).copied().collect();
    want.insert(change_of(&tx, &own_scripts(&engine, &session, id)));
    assert_holds(&engine, &session, id, &want, "after a restart");
}

/// A coin stored unspent stays so when a later batch hands it to the
/// store again with a verdict that the wallet does not hold it: the store
/// only declines to write it (a known gap: an existing row is not marked
/// spent).
#[test]
fn a_coin_stored_unspent_stays_so_whatever_a_later_verdict_says() {
    use platform_wallet::changeset::changeset::UtxoCreditVerdict;
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let (_dir, engine, session, id) = engine_wallet();
    let to = fresh_address(&engine, &session, id);
    let paid = funding(9, std::slice::from_ref(&to), COIN);
    let x = key_wallet::Utxo::new(
        OutPoint::new(paid.txid(), 0),
        paid.output[0].clone(),
        to,
        FUNDED_AT,
        false,
    );
    store_raw(
        &session,
        id,
        CoreChangeSet {
            new_utxos: vec![x.clone()],
            ..Default::default()
        },
    );
    let store = Arc::clone(&session.live().unwrap().store);
    store
        .store(
            id.0,
            PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    new_utxos: vec![x.clone()],
                    utxo_credit_verdicts: [(x.outpoint, UtxoCreditVerdict::Doomed)].into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    store.flush(id.0).unwrap();
    drop(store);

    let session = reopen(&engine, session);
    assert_holds(
        &engine,
        &session,
        id,
        &BTreeSet::from([x.outpoint]),
        "after a restart",
    );
}

/// A payment of A and B, InstantSend-locked, then beaten on B by another
/// device's spend mined after a restart (a ChainLock outranks the lock).
/// After the next restart the funding block scanned again credits A: the
/// beaten payment no longer claims it.
#[test]
fn a_locked_payment_a_mined_spend_beat_frees_its_other_coin_after_a_restart() {
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
    instant_lock(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
}

/// A wallet holding one confirmed coin of one DASH, mined at [`FUNDED_AT`]
/// and scanned to [`TIP`]; returns the transaction paying it.
fn single_coin_wallet() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
) {
    let (dir, engine, session, id) = engine_wallet();
    let to = fresh_address(&engine, &session, id);
    let paid = funding(7, std::slice::from_ref(&to), COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let b = balances(&session, id);
    assert_eq!((b.confirmed, b.total), (COIN, COIN), "{b:?}");
    (dir, engine, session, id, paid)
}

/// Pays a quarter DASH from `source`, hands the payment to the wallet as
/// dash-spv does after a broadcast and waits for the store to hold it;
/// returns it with the coins it spent and its change.
fn pay_stored(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
) -> (Transaction, Vec<OutPoint>, OutPoint) {
    let tx = send(engine, session, id, source, COIN / 4, false);
    mempool(engine, session, id, &tx);
    wait_stored(session, id, &[tx.txid()]);
    let spent = tx.input.iter().map(|i| i.previous_output).collect();
    let change = change_of(&tx, &own_scripts(engine, session, id));
    (tx, spent, change)
}

fn buckets(b: &WalletBalances) -> (u64, u64, u64) {
    (b.confirmed, b.unconfirmed, b.total)
}

/// #86: a wallet holding one coin pays; after a restart, a second payment
/// made before any block spends the first one's change, and after another
/// restart a third spends the second one's. The change counts as it did
/// before each restart.
#[test]
fn change_of_an_unmined_own_payment_is_spendable_after_a_restart() {
    let (_dir, engine, session, id, paid) = single_coin_wallet();
    let (_, spent, change) = pay_stored(&engine, &session, id, CoinSource::Any);
    assert_eq!(spent, vec![OutPoint::new(paid.txid(), 0)]);
    assert!(utxo(&engine, &session, id, change).is_trusted);
    let before = buckets(&balances(&session, id));

    let session = reopen(&engine, session);
    let after = assert_holds(&engine, &session, id, &BTreeSet::from([change]), "restart");
    let (_, spent, second_change) = pay_stored(&engine, &session, id, CoinSource::Any);
    assert_eq!(spent, vec![change], "the second payment spends the change");
    assert_eq!(buckets(&after), before, "as before the restart");
    let before = buckets(&balances(&session, id));

    let session = reopen(&engine, session);
    let after = assert_holds(
        &engine,
        &session,
        id,
        &BTreeSet::from([second_change]),
        "second restart",
    );
    let (_, spent, _) = pay_stored(&engine, &session, id, CoinSource::Any);
    assert_eq!(spent, vec![second_change]);
    assert_eq!(buckets(&after), before, "as before the second restart");
}

/// A payment restored at load whose coin another device then spends in a
/// block, paying only someone else: the payment can never confirm, so
/// automatic selection does not take its change, nor after the block is
/// ChainLocked and key-wallet forgets the spend, nor after a restart.
#[test]
fn change_of_a_restored_payment_a_block_beat_is_not_selected() {
    let (_dir, engine, session, id, paid) = single_coin_wallet();
    let (_, _, change) = pay_stored(&engine, &session, id, CoinSource::Any);

    let session = reopen(&engine, session);
    assert!(utxo(&engine, &session, id, change).is_trusted);
    let winner = spent_elsewhere(&engine, &session, id, &paid);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert_eq!(available(&engine, &session, id, COIN / 8), Some(0));

    chain_lock(&engine, &session, TIP + 1);
    let coin = OutPoint::new(paid.txid(), 0);
    assert!(
        !observed_spent(&engine, &session, id).contains(&coin),
        "key-wallet forgot the spend"
    );
    assert_eq!(available(&engine, &session, id, COIN / 8), Some(0));

    let session = reopen(&engine, session);
    assert!(!utxo(&engine, &session, id, change).is_trusted);
    assert_eq!(available(&engine, &session, id, COIN / 8), Some(0));
}

/// key-wallet applies a ChainLock at `height`.
fn chain_lock(engine: &Engine, session: &NetworkSession, height: u32) {
    use platform_wallet::key_wallet_manager::WalletInterface;
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        wm.write().await.apply_chain_lock(dashcore::ChainLock {
            block_height: height,
            block_hash: dashcore::BlockHash::all_zeros(),
            signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
        });
    });
}

/// The outpoints key-wallet holds as spent by a block it scanned.
fn observed_spent(engine: &Engine, session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        info.core_wallet
            .observed_spent_outpoints()
            .keys()
            .copied()
            .collect()
    })
}

/// Another device of the wallet spends `paid`'s output 0, paying only
/// someone else.
fn spent_elsewhere(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
) -> Transaction {
    let owner =
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let winner = super::spend_of(
        OutPoint::new(paid.txid(), 0),
        vec![(elsewhere(), COIN - 1_000)],
    );
    super::signed_by_wallet(engine, session, id, &owner, winner)
}

/// What automatic selection finds for a payment of `amount`: `None` when
/// it is enough, else what it reports available.
fn available(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    amount: u64,
) -> Option<u64> {
    let (_draft, prepared) = prepare(
        engine,
        session,
        id,
        CoinSource::Any,
        amount,
        false,
        &Arc::new(Accept::default()),
    );
    match prepared {
        Ok(_) => None,
        Err(crate::EngineError::Send(crate::send::SendFailure::AmountExceedsBalance {
            available,
        })) => Some(available),
        Err(e) => panic!("{e:?}"),
    }
}

/// A restored payment P and its restored child C, which spends P's change.
/// After a restart a peer relays P, so key-wallet records it; then another
/// device's spend of P's coin, paying only someone else, is InstantSend
/// locked and key-wallet sweeps P. C can never confirm: automatic
/// selection takes nothing of its change, before and after a peer relays C,
/// a relay key-wallet ignores as a spend of a swept transaction.
#[test]
fn change_of_a_restored_child_of_a_swept_payment_is_not_selected() {
    let (_dir, engine, session, id, paid) = single_coin_wallet();
    let (parent, _, change) = pay_stored(&engine, &session, id, CoinSource::Any);
    let (child, spent, child_change) = pay_stored(&engine, &session, id, CoinSource::Any);
    assert_eq!(spent, vec![change]);

    let session = reopen(&engine, session);
    assert!(utxo(&engine, &session, id, child_change).is_trusted);
    mempool(&engine, &session, id, &parent);
    let winner = spent_elsewhere(&engine, &session, id, &paid);
    instant_lock(&engine, &session, id, &winner);
    assert!(
        session.hub.history.get(&id, &parent.txid()).is_none(),
        "key-wallet swept P"
    );
    assert_eq!(
        available(&engine, &session, id, COIN / 8),
        Some(0),
        "C's change is not selected"
    );
    super::deliver(&engine, &session, id, move |wm| {
        use platform_wallet::key_wallet_manager::WalletInterface;
        Box::pin(async move {
            let result = wm.process_mempool_transaction(&child, None).await;
            assert!(!result.is_relevant, "the relay of C is ignored");
        })
    });
    assert_eq!(
        available(&engine, &session, id, COIN / 8),
        Some(0),
        "nor after a peer relays C"
    );
}

/// A restored payment beaten by another device's spend of its coin that
/// pays the wallet too and is InstantSend-locked, not mined: the history
/// holds the winner, and automatic selection takes only the winner's
/// change, never the payment's.
#[test]
fn change_of_a_restored_payment_a_locked_spend_beat_is_not_selected() {
    let (_dir, engine, session, id, paid) = single_coin_wallet();
    let (_, _, change) = pay_stored(&engine, &session, id, CoinSource::Any);

    let session = reopen(&engine, session);
    assert!(utxo(&engine, &session, id, change).is_trusted);
    let winner = rival(&engine, &session, id, &paid, 0);
    mempool(&engine, &session, id, &winner);
    instant_lock(&engine, &session, id, &winner);
    let won = change_of(&winner, &own_scripts(&engine, &session, id));
    let won = winner.output[won.vout as usize].value;
    assert_eq!(
        available(&engine, &session, id, COIN * 6 / 10),
        Some(won),
        "only the winner's change"
    );
}

/// A wallet load that cannot read every stored record trusts no change: a
/// payment's change stays untrusted until a load reads everything.
#[test]
fn a_load_with_an_unreadable_record_trusts_no_change() {
    let (_dir, engine, session, id, paid) = single_coin_wallet();
    let other = funding(8, &[fresh_address(&engine, &session, id)], COIN);
    mempool(&engine, &session, id, &other);
    let coin = OutPoint::new(paid.txid(), 0);
    let (payment, _, change) = pay_stored(&engine, &session, id, CoinSource::Outpoints(vec![coin]));
    wait_stored(&session, id, &[other.txid(), payment.txid()]);

    engine.block_on(session.unload_wallet(id)).unwrap();
    *session.hub.unreadable_record.lock().unwrap() = (Some(other.txid()), Some(other.txid()));
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(!utxo(&engine, &session, id, change).is_trusted);

    let session = reopen(&engine, session);
    assert!(
        utxo(&engine, &session, id, change).is_trusted,
        "a clean load"
    );
}

/// The change of a payment of a coin neither in a block nor InstantSend
/// locked is not trusted in the session that sent it, and stays so after a
/// restart: it counts as unconfirmed, and automatic selection does not
/// take it.
#[test]
fn change_of_a_payment_of_an_unconfirmed_coin_stays_untrusted_after_a_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let to = fresh_address(&engine, &session, id);
    let paid = funding(7, std::slice::from_ref(&to), COIN);
    mempool(&engine, &session, id, &paid);
    let coin = OutPoint::new(paid.txid(), 0);
    let (_, _, change) = pay_stored(&engine, &session, id, CoinSource::Outpoints(vec![coin]));
    assert!(!utxo(&engine, &session, id, change).is_trusted);
    let before = buckets(&balances(&session, id));
    assert_eq!(before.0, 0, "nothing is confirmed: {before:?}");

    let session = reopen(&engine, session);
    let after = assert_holds(&engine, &session, id, &BTreeSet::from([change]), "restart");
    assert_eq!(buckets(&after), before, "as before the restart");
    assert!(!utxo(&engine, &session, id, change).is_trusted);
    let (_draft, prepared) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Any,
        COIN / 4,
        false,
        &Arc::new(Accept::default()),
    );
    assert!(
        matches!(
            prepared,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::AmountExceedsBalance { available: 0 }
            ))
        ),
        "automatic selection takes no coin"
    );
}

/// A payment whose change the user set to one of the wallet's CoinJoin
/// addresses: key-wallet does not trust that output, and neither does the
/// load after a restart.
#[test]
fn change_to_a_coinjoin_address_is_not_trusted_after_a_restart() {
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    let (_dir, engine, session, id, _) = single_coin_wallet();
    let manager = session.manager().unwrap();
    let mix = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        let account = info.core_wallet.accounts.coinjoin_accounts.get(&0).unwrap();
        let key_wallet::managed_account::managed_account_type::ManagedAccountType::CoinJoin {
            internal_addresses,
            ..
        } = account.managed_account_type()
        else {
            panic!("a CoinJoin account");
        };
        internal_addresses
            .addresses
            .values()
            .next()
            .unwrap()
            .address
            .clone()
    });
    let broadcaster = Arc::new(Accept::default());
    let (draft, prepared) = prepare_with(
        &engine,
        &session,
        id,
        CoinSource::Any,
        COIN / 4,
        false,
        &broadcaster,
        |d| {
            d.set_change(crate::send::ChangePolicy::Address(mix.to_string()))
                .unwrap()
        },
    );
    engine.block_on(draft.broadcast(prepared.unwrap())).unwrap();
    let tx = broadcaster.0.lock().unwrap()[0].clone();
    mempool(&engine, &session, id, &tx);
    wait_stored(&session, id, &[tx.txid()]);
    let vout = (tx.output.iter()).position(|o| o.script_pubkey == mix.script_pubkey());
    let change = OutPoint::new(tx.txid(), vout.unwrap() as u32);
    assert!(!utxo(&engine, &session, id, change).is_trusted);

    let session = reopen(&engine, session);
    assert!(!utxo(&engine, &session, id, change).is_trusted);
}
