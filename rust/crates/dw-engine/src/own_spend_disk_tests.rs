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
/// from `source` to [`FOREIGN`] through a real draft.
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
    prepare_to(
        engine,
        session,
        id,
        source,
        FOREIGN,
        amount,
        subtract_fee,
        broadcaster,
    )
}

/// [`prepare`] to the address `to`.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_to(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    to: &str,
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
            address: to.into(),
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

/// Prepares and broadcasts a payment of `amount` from `source` to
/// [`FOREIGN`] through a real draft; returns the signed transaction.
pub(super) fn send(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    amount: u64,
    subtract_fee: bool,
) -> Transaction {
    send_to(engine, session, id, source, FOREIGN, amount, subtract_fee)
}

/// [`send`] to the address `to`.
pub(super) fn send_to(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    source: CoinSource,
    to: &str,
    amount: u64,
    subtract_fee: bool,
) -> Transaction {
    let broadcaster = Arc::new(Accept::default());
    let (draft, prepared) = prepare_to(
        engine,
        session,
        id,
        source,
        to,
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
/// device's spend mined after a restart. A plain block does not override
/// the lock (#170 case 11, key-wallet's finality order): after the next
/// restart the payment still claims A and B, so the funding block scanned
/// again credits neither and coin control cannot offer them. Once that
/// block is ChainLocked, the next restart frees A.
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
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let held_now = held(&engine, &session, id);
    assert!(!held_now.contains(&a), "a plain block: A stays claimed");
    assert!(!held_now.contains(&b), "B stays claimed");
    let total: u64 = coins(&engine, &session, id).iter().map(|c| c.1).sum();
    assert_eq!(balances(&session, id).total, total);
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");

    let winner_block = super::block_with(vec![winner.clone()]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, winner_block);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "ChainLocked: A is credited"
    );
}

/// The same payment and winner in a wallet opened later in the session:
/// the claims restored as it is registered follow the same order. The
/// funding block processed before its history loads credits A only once
/// the winner's block is ChainLocked.
#[test]
fn a_wallet_opened_later_keeps_a_locked_payments_claims_until_a_chainlock() {
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

    let (registered, loaded) = opened_later(&engine, &session, id, &paid);
    assert!(!registered.contains(&a), "a plain block: A stays claimed");
    assert!(!loaded.contains(&a), "and after its history loads");

    let winner_block = super::block_with(vec![winner]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, winner_block);
    let (registered, loaded) = opened_later(&engine, &session, id, &paid);
    assert!(registered.contains(&a), "ChainLocked: A is credited");
    assert!(loaded.contains(&a), "and stays after its history loads");
}

/// A payment of A and B with no change, stored before its InstantSend
/// lock, and another device's spend of B mined in a plain block. After a
/// restart the lock arrives after the history load copied the wallet's
/// locks and before the claims are installed. The wallet holds no record
/// or coin of the payment, so no lock event reaches its history: the
/// claims read the lock under the guard that installs them, and A stays
/// claimed.
#[test]
fn a_lock_arriving_before_the_claims_are_installed_keeps_a_locked_payments_claims() {
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
    wait_stored(&session, id, &[payment.txid()]);
    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let session = reopen(&engine, session);

    engine.block_on(session.unload_wallet(id)).unwrap();
    let copied = Arc::new(crate::coins::TestPause::default());
    let _copied = crate::held_writes::ReleaseOnDrop(Arc::clone(&copied));
    *session.hub.pause_history.lock().unwrap() = Some(Arc::clone(&copied));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the history load's pause", || {
        copied.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let installing = Arc::new(crate::coins::TestPause::default());
    let _installing = crate::held_writes::ReleaseOnDrop(Arc::clone(&installing));
    *session.hub.pause_spend_claims.lock().unwrap() = Some(Arc::clone(&installing));
    copied.release.notify_one();
    eventually("the claims' pause", || {
        installing.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    instant_lock(&engine, &session, id, &payment);
    installing.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// Two abandoned payments: P of A and B, whose InstantSend lock arrives
/// between the wallet's next load's history copy and its claims' install,
/// and Q of C with no lock. The load restores P's claims and keeps both
/// sent marks. With the lock known at the following load, P still claims
/// A and B, and Q's kept mark claims nothing.
#[test]
fn a_lock_arriving_after_an_abandon_keeps_the_payments_sent_mark() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b, c) = (
        OutPoint::new(paid.txid(), 0),
        OutPoint::new(paid.txid(), 1),
        OutPoint::new(paid.txid(), 2),
    );
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        2 * COIN,
        true,
    );
    mempool(&engine, &session, id, &p);
    let q = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![c]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &q);
    wait_stored(&session, id, &[p.txid(), q.txid()]);
    abandon(&engine, &session, id, p.txid());
    abandon(&engine, &session, id, q.txid());

    engine.block_on(session.unload_wallet(id)).unwrap();
    let copied = Arc::new(crate::coins::TestPause::default());
    let _copied = crate::held_writes::ReleaseOnDrop(Arc::clone(&copied));
    *session.hub.pause_history.lock().unwrap() = Some(Arc::clone(&copied));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the history load's pause", || {
        copied.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let installing = Arc::new(crate::coins::TestPause::default());
    let _installing = crate::held_writes::ReleaseOnDrop(Arc::clone(&installing));
    *session.hub.pause_spend_claims.lock().unwrap() = Some(Arc::clone(&installing));
    copied.release.notify_one();
    eventually("the claims' pause", || {
        installing.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    instant_lock(&engine, &session, id, &p);
    installing.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    let marks = sent_marks(&session, id);
    assert!(marks.contains(&p.txid()), "P's sent mark stays");
    assert!(marks.contains(&q.txid()), "Q's stays too");

    engine.block_on(session.unload_wallet(id)).unwrap();
    let copied = Arc::new(crate::coins::TestPause::default());
    let _copied = crate::held_writes::ReleaseOnDrop(Arc::clone(&copied));
    *session.hub.pause_history.lock().unwrap() = Some(Arc::clone(&copied));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the history load's pause", || {
        copied.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    instant_lock(&engine, &session, id, &p);
    copied.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    let held_now = held(&engine, &session, id);
    assert!(!held_now.contains(&a), "P claims A");
    assert!(!held_now.contains(&b), "and B");
    assert!(held_now.contains(&c), "Q's coin is credited");
}

/// An abandoned payment P of A and B whose mark a load could drop, then
/// relayed again with its InstantSend lock: the wallet stores it locked,
/// and after the next load and the funding block scanned again A and B
/// stay spent and are not offered.
#[test]
fn an_abandoned_payment_relayed_with_its_lock_keeps_its_coins_spent() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        2 * COIN,
        true,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    abandon(&engine, &session, id, p.txid());
    let reload = || {
        engine.block_on(session.unload_wallet(id)).unwrap();
        engine.block_on(session.load_wallet(id)).unwrap();
    };
    reload();

    instant_lock(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    assert!(
        (session.hub.history.get(&id, &p.txid())).is_some_and(|e| e.instant_locked),
        "the relay is taken locked"
    );
    reload();
    mined(&engine, &session, id, TIP, vec![paid]);
    let held_now = held(&engine, &session, id);
    assert!(!held_now.contains(&a), "A stays spent");
    assert!(!held_now.contains(&b), "B stays spent");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// The same payment paid to another loaded wallet, its lock delivered
/// once the claims are installed: the lock reaches only the recipient
/// wallet, so the sender's history never sees it and A is credited and
/// offered until the payment is mined.
#[test]
#[ignore = "a lock after the load reaches only the recipient wallet, see #245"]
fn a_lock_arriving_after_the_load_keeps_a_locked_payments_claims() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let to = fresh_address(&engine, &session, other);
    let payment = send_to(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        &to.to_string(),
        2 * COIN,
        true,
    );
    assert_eq!(payment.output.len(), 1, "no change");
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let session = reopen(&engine, session);

    instant_lock(&engine, &session, id, &payment);
    assert!(
        (session.hub.history.get(&other, &payment.txid())).is_some_and(|e| e.instant_locked),
        "the recipient wallet reads the payment locked"
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// Another device's spend of B that pays only someone else, in a block a
/// wallet opened later sees before its claims are set: the history never
/// holds it, only key-wallet's note of the spent coin. A plain block does
/// not override the payment's lock, so it keeps claiming A; a ChainLocked
/// one does, and the funding block scanned again credits A.
#[test]
fn a_block_seen_before_the_claims_beats_a_locked_payment_only_once_chainlocked() {
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
    let owner =
        Address::from_script(&paid.output[1].script_pubkey, dashcore::Network::Regtest).unwrap();
    let winner = super::spend_of(b, vec![(elsewhere(), COIN - 1_000)]);
    let winner = super::signed_by_wallet(&engine, &session, id, &owner, winner);
    let winner_block = super::block_with(vec![winner.clone()]).block_hash();

    let credited = |chainlock: Option<u32>| {
        engine.block_on(session.unload_wallet(id)).unwrap();
        let pause = Arc::new(crate::coins::TestPause::default());
        let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&pause));
        *session.hub.pause_claims.lock().unwrap() = Some(Arc::clone(&pause));
        let load = {
            let s = Arc::clone(&session);
            session.rt.spawn(async move { s.load_wallet(id).await })
        };
        eventually("the load's pause", || {
            pause.reached.load(std::sync::atomic::Ordering::SeqCst)
        });
        let winner = winner.clone();
        super::deliver(&engine, &session, id, move |wm| {
            Box::pin(async move {
                use platform_wallet::key_wallet_manager::WalletInterface;
                let block = super::block_with(vec![winner]);
                let ids = BTreeSet::from([id.0]);
                (wm.process_block_for_wallets(&block, block.block_hash(), TIP + 1, &ids)).await;
            })
        });
        if let Some(height) = chainlock {
            chain_lock(&engine, &session, id, height, winner_block);
        }
        pause.release.notify_one();
        engine.block_on(load).unwrap().unwrap();
        mined(&engine, &session, id, TIP, vec![paid.clone()]);
        held(&engine, &session, id).contains(&a)
    };
    assert!(!credited(None), "a plain block: A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert!(
        !credited(Some(TIP)),
        "a ChainLock below it: A stays claimed"
    );
    assert!(credited(Some(TIP + 1)), "ChainLocked: A is credited");
}

/// A payment of A and B, InstantSend-locked and stored; after a restart
/// another device's spend of B is mined in a plain block and stored; then a
/// restart. Returns the funding transaction and the winner.
fn locked_payment_with_a_plain_winner() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
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
    mempool(&engine, &session, id, &payment);
    instant_lock(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let session = reopen(&engine, session);
    (dir, engine, session, id, paid, winner)
}

/// The claims are chosen under the write lock that installs them: a
/// ChainLock of the winner's block applied while a wallet opened later
/// waits to install them beats the locked payment, and the funding block
/// scanned again credits A.
#[test]
fn a_chainlock_applied_before_the_claims_are_installed_beats_a_locked_payment() {
    let (_dir, engine, session, id, paid, winner) = locked_payment_with_a_plain_winner();
    let a = OutPoint::new(paid.txid(), 0);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&pause));
    *session.hub.pause_spend_claims.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the claims' pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let winner_block = super::block_with(vec![winner]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, winner_block);
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
}

/// A ChainLock of the winner's block applied while the wallet was closed,
/// to another wallet that stayed open, beats the locked payment when the
/// wallet opens again and after a restart.
#[test]
fn a_chainlock_applied_while_the_wallet_was_closed_beats_a_locked_payment() {
    let (_dir, engine, session, id, paid, winner) = locked_payment_with_a_plain_winner();
    let a = OutPoint::new(paid.txid(), 0);
    let other = second_wallet(&engine, &session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let winner_block = super::block_with(vec![winner]).block_hash();
    chain_lock(&engine, &session, other, TIP + 1, winner_block);
    engine.block_on(session.load_wallet(id)).unwrap();
    engine
        .block_on(session.set_load_on_startup(id, true))
        .unwrap();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "reopened: A is credited"
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "after a restart");
}

/// The same, with the other wallet closed too before the wallet opens
/// again: the session's ChainLock still beats the locked payment.
#[test]
fn a_chainlock_applied_while_both_wallets_were_closed_beats_a_locked_payment() {
    let (_dir, engine, session, id, paid, winner) = locked_payment_with_a_plain_winner();
    let a = OutPoint::new(paid.txid(), 0);
    let other = second_wallet(&engine, &session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let winner_block = super::block_with(vec![winner]).block_hash();
    chain_lock(&engine, &session, other, TIP + 1, winner_block);
    engine.block_on(session.unload_wallet(other)).unwrap();
    engine.block_on(session.load_wallet(id)).unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "reopened: A is credited"
    );
    assert!(
        spendable(&engine, &session, id, a),
        "and a payment can spend it"
    );
}

/// A wallet closed while coin control's summary waits on the app
/// database: its coin read reads the wallet's state only before it waits,
/// so it ends without a panic.
#[test]
fn closing_a_wallet_during_a_coin_read_does_not_panic() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let pause = Arc::new(crate::coins::TestPause::default());
    let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&pause));
    let read = {
        let s = Arc::clone(&session);
        let held = crate::coins::HOLD_APPDB_JOB.scope(Arc::clone(&pause), async move {
            let fee = crate::send::FeeMode::PerKb(1_000);
            (s.coin_selection_summary(id, Vec::new(), Vec::new(), fee, false)).await
        });
        session.rt.spawn(held)
    };
    eventually("the coin read's app database job", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let unload = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move { s.unload_wallet(id).await })
    };
    eventually("the wallet closed", || {
        (session.wallet_load_states().unwrap().iter()).all(|w| w.wallet_id != id || !w.loaded)
    });
    pause.release.notify_one();
    let read = engine.block_on(read).unwrap();
    assert!(!read.as_ref().is_err_and(|e| e.is_task_panic()), "{read:?}");
    engine.block_on(unload).unwrap().unwrap();
}

/// While a load claims nothing (another stored record is unreadable), the
/// history keeps the locked payment's coins from selection against a plain
/// block: A, which only the payment spends, is credited but not offered.
#[test]
fn a_locked_payment_keeps_its_coin_from_selection_without_a_claim() {
    let (_dir, engine, session, id, paid, _) = locked_payment_with_a_plain_winner();
    let a = OutPoint::new(paid.txid(), 0);
    let other = funding(8, &[fresh_address(&engine, &session, id)], COIN);
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[other.txid()]);
    engine.block_on(session.unload_wallet(id)).unwrap();
    *session.hub.unreadable_record.lock().unwrap() = (Some(other.txid()), Some(other.txid()));
    engine.block_on(session.load_wallet(id)).unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "no claim");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// A second wallet of the session, born at [`BIRTH`].
pub(super) fn second_wallet(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(
            session.import_wallet(
                zeroize::Zeroizing::new(
                    b"legal winner thank year wave sausage worth useful legal winner thank yellow"
                        .to_vec(),
                ),
                zeroize::Zeroizing::new(Vec::new()),
                crate::ImportOptions {
                    birth_height: Some(BIRTH),
                    ..Default::default()
                },
            ),
        )
        .unwrap()
}

/// The payments this app sent that `id` keeps a sent mark for.
pub(super) fn sent_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    crate::tx_actions::load_sent(&session.live().unwrap().appdb, id)
        .0
        .into_keys()
        .collect()
}

/// Q, a payment mined at `height` whose block the wallet's ChainLock then
/// names: once its mark is gone, a ChainLock's forgets have run.
fn chain_locked_sentinel(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    height: u32,
) -> Transaction {
    let q = send(engine, session, id, CoinSource::Any, COIN / 3, false);
    mempool(engine, session, id, &q);
    mined(engine, session, id, height, vec![q.clone()]);
    let block = super::block_with(vec![q.clone()]).block_hash();
    chain_lock(engine, session, id, height, block);
    eventually("the sentinel's mark goes", || {
        !sent_marks(session, id).contains(&q.txid())
    });
    q
}

/// A ChainLock of a mined payment's own block while the app runs drops
/// the payment's sent mark in the session.
#[test]
fn a_chainlock_of_a_payments_block_drops_its_mark_in_the_session() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = send(&engine, &session, id, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, id, &p);
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    assert!(sent_marks(&session, id).contains(&p.txid()));
    let block = super::block_with(vec![p.clone()]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, block);
    eventually("the mark goes", || {
        !sent_marks(&session, id).contains(&p.txid())
    });
}

/// A payment's block reorged out (truncated in memory), then a ChainLock
/// of the block that replaced it at that height: the payment, back out of
/// a block, keeps its mark.
#[test]
fn a_chainlock_after_a_reorg_keeps_the_reorged_payments_mark() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = send(&engine, &session, id, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, id, &p);
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.truncate_above(TIP).unwrap();
        })
    });
    let other = super::block_with(vec![foreign()]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, other);
    chain_locked_sentinel(&engine, &session, id, TIP + 2);
    assert!(sent_marks(&session, id).contains(&p.txid()));
}

/// A ChainLock of another block at a payment's height, applied before any
/// reorg: key-wallet promotes the payment's record by height, but the lock
/// names another block, so the mark stays.
#[test]
fn a_chainlock_of_another_block_at_its_height_keeps_a_payments_mark() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = send(&engine, &session, id, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, id, &p);
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    let other = super::block_with(vec![foreign()]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, other);
    chain_locked_sentinel(&engine, &session, id, TIP + 2);
    assert!(sent_marks(&session, id).contains(&p.txid()));
}

/// Another loaded wallet, paid by the payment P, applies the ChainLock of
/// P's block while the sender is closed. The block also holds R, a payment
/// of that wallet: R's mark going shows its forgets ran, in the same write
/// as any for P, and they touch only its own marks. The sender opened again
/// keeps P's.
#[test]
fn another_wallets_chainlock_drops_no_mark_of_the_sender() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let other = second_wallet(&engine, &session);
    let funded = funding(9, &[fresh_address(&engine, &session, other)], COIN);
    mined(&engine, &session, other, TIP, vec![funded]);
    let r = send(&engine, &session, other, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, other, &r);
    assert!(sent_marks(&session, other).contains(&r.txid()));
    let to = fresh_address(&engine, &session, other);
    let p = send_to(
        &engine,
        &session,
        id,
        CoinSource::Any,
        &to.to_string(),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    let txs = vec![p.clone(), r.clone()];
    mined(&engine, &session, id, TIP + 1, txs.clone());
    mined(&engine, &session, other, TIP + 1, txs.clone());
    engine.block_on(session.unload_wallet(id)).unwrap();
    let block = super::block_with(txs).block_hash();
    chain_lock(&engine, &session, other, TIP + 1, block);
    eventually("the other wallet's own mark goes", || {
        !sent_marks(&session, other).contains(&r.txid())
    });
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(sent_marks(&session, id).contains(&p.txid()));
}

/// Unloads wallet `id` and opens it again; `paid`'s block is processed for
/// it once it is registered, before its history loads. The coins it holds
/// then, and once the load is done.
fn opened_later(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
) -> (BTreeSet<OutPoint>, BTreeSet<OutPoint>) {
    engine.block_on(session.unload_wallet(id)).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    let _release = crate::held_writes::ReleaseOnDrop(Arc::clone(&pause));
    *session.hub.pause_load.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let s = Arc::clone(session);
        session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the load's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    mined(engine, session, id, TIP, vec![paid.clone()]);
    let registered = held(engine, session, id);
    pause.release.notify_one();
    engine.block_on(load).unwrap().unwrap();
    (registered, held(engine, session, id))
}

/// The wallet's ChainLock of block `hash` at `height`, as dash-spv
/// applies it; waits for the store to hold it.
pub(super) fn chain_lock(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    height: u32,
    hash: dashcore::BlockHash,
) {
    super::deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.apply_chain_lock(dashcore::ChainLock {
                block_height: height,
                block_hash: hash,
                signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
            });
        })
    });
    use platform_wallet::changeset::PlatformWalletPersistence;
    let store = Arc::clone(&session.live().unwrap().store);
    eventually("the store holds the ChainLock", || {
        let stored = store.load().unwrap();
        stored.wallets.get(&id.0).is_some_and(|w| {
            (w.wallet_info.metadata.last_applied_chain_lock.as_ref())
                .is_some_and(|cl| cl.block_height >= height)
        })
    });
}
