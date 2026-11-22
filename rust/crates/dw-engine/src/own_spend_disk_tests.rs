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
    start_spv(&engine, &session);

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
    headers(
        &session,
        &[(TIP + 1, block_hash(std::slice::from_ref(&winner)))],
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");

    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    assert!(
        held(&engine, &session, id).contains(&change),
        "a plain block does not override the lock: the change stays"
    );
    start_spv(&engine, &session);
    let refused = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::InstantLocked
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        0
    );

    chain_lock(
        &engine,
        &session,
        id,
        TIP + 1,
        block_hash(std::slice::from_ref(&winner)),
    );
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        1
    );
    assert!(abandoned(&engine, &session, id, payment.txid()));
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = outputs(&paid);
    want.remove(&b);
    want.insert(change_of(&winner, &own_scripts(&engine, &session, id)));
    assert_holds(&engine, &session, id, &want, "removed, then scanned again");
    change_is_gone(&engine, &session, id, (&payment, change), "removed");
    assert!(
        !spendable(&engine, &session, id, change),
        "its change is not offered"
    );
    assert!(spendable(&engine, &session, id, a), "A is");
    assert!(!spendable(&engine, &session, id, b), "B is not");

    let session = reopen(&engine, session);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert_holds(&engine, &session, id, &want, "removed, then a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert_holds(&engine, &session, id, &want, "replayed after a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "replayed");
}

/// The wallet's ChainLock of block `hash` at `height`, as dash-spv
/// applies it; waits for the store to hold it.
fn chain_lock(
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

/// [`chain_lock`] in a later session than the spend's block: key-wallet keeps
/// a spend that a lock spared only in memory, so this ChainLock sweeps
/// nothing and the beaten payment stays for Abandon.
fn chain_lock_after_a_restart(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    height: u32,
    hash: dashcore::BlockHash,
) -> Arc<NetworkSession> {
    let session = reopen(engine, session);
    chain_lock(engine, &session, id, height, hash);
    session
}

/// The abandoned `payment`'s change `change` is gone: not counted in the
/// balance, not in coin control's list, and never picked by automatic
/// selection for a payment that only it could fund.
fn change_is_gone(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    (payment, change): (&Transaction, OutPoint),
    state: &str,
) {
    let held = coins(engine, session, id);
    assert!(
        held.iter().all(|(o, _)| *o != change),
        "{state}: the change is held: {held:?}"
    );
    let total: u64 = held.iter().map(|(_, v)| v).sum();
    let b = balances(session, id);
    assert_eq!(b.total, total, "{state}: the change is counted: {b:?}");
    let filter = crate::CoinFilter {
        include_locked: true,
        ..Default::default()
    };
    let listed = engine.block_on(session.utxos(id, filter)).unwrap();
    assert!(
        listed.iter().all(|c| c.outpoint != change),
        "{state}: coin control lists the change"
    );
    let (_draft, prepared) = prepare(
        engine,
        session,
        id,
        CoinSource::Any,
        total + payment.output[change.vout as usize].value / 2,
        false,
        &Arc::new(Accept::default()),
    );
    use crate::send::SendFailure;
    assert!(
        matches!(
            prepared,
            Err(crate::EngineError::Send(
                SendFailure::AmountExceedsBalance { .. }
                    | SendFailure::AmountWithFeeExceedsBalance { .. }
            ))
        ),
        "{state}: automatic selection could fund a payment only the change covers: {:?}",
        prepared.map(|p| p.summary().inputs.clone())
    );
}

/// The SPV header chain holds `chain`'s blocks at their heights, and no
/// other header.
fn headers(session: &NetworkSession, chain: &[(u32, dashcore::BlockHash)]) {
    let chain: std::collections::HashMap<u32, dashcore::BlockHash> =
        chain.iter().copied().collect();
    session.hub.attach_headers(Arc::new(move |h| {
        let hash = chain.get(&h).copied();
        Box::pin(async move { hash })
    }));
}

fn block_hash(txs: &[Transaction]) -> dashcore::BlockHash {
    super::block_with(txs.to_vec()).block_hash()
}

/// Installs the SPV client Abandon and Remove Unconfirmed need for their
/// rescan, as the Platform bring-up would, without its run loop.
fn start_spv(engine: &Engine, session: &Arc<NetworkSession>) {
    let s = Arc::clone(session);
    engine
        .block_on(session.on_runtime(async move {
            let config = s.spv_config()?;
            let dir = std::path::Path::new(crate::session::SPV_DIR);
            crate::fsutil::create_owned_dir(s.data_dir(), dir)?;
            s.manager()?.spv().start(config).await?;
            Ok(())
        }))
        .unwrap();
    assert!(session.manager().unwrap().spv().is_started());
}

/// Abandon refuses `txid` as InstantSend-locked.
fn refused(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid, state: &str) {
    let refused = engine.block_on(session.abandon_transaction(id, txid));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::InstantLocked
            ))
        ),
        "{state}: {refused:?}"
    );
}

fn abandoned(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid) -> bool {
    engine
        .block_on(session.tx_detail_extras(id, txid))
        .unwrap()
        .abandoned
}

/// A payment of A and B, InstantSend-locked; after a restart another
/// device's spend of B that pays the wallet change is mined and
/// ChainLocked (#170 case 14), and the funding block is scanned again.
/// With a ChainLock above the winner's block, Abandon refuses until the
/// scan reaches it, and while the SPV header chain lacks or differs on
/// either block, each varied alone. Then it goes through: the payment's
/// change leaves at once, and once the funding block and the winner's are scanned
/// again the wallet holds A and the winner's change, never B, in the
/// session and after a restart.
#[test]
fn abandoning_a_locked_payment_a_chainlocked_spend_beat_frees_a_and_drops_its_change() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    assert!(held(&engine, &session, id).contains(&change));
    let won = block_hash(std::slice::from_ref(&winner));
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 1, won);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    let forked = dashcore::BlockHash::from_byte_array([1; 32]);
    let next = dashcore::BlockHash::from_byte_array([2; 32]);
    let both = [(TIP + 1, won), (TIP + 2, next)];
    chain_lock(&engine, &session, id, TIP + 2, next);
    headers(&session, &both);
    let state = "the scan below the ChainLock";
    refused(&engine, &session, id, payment.txid(), state);
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    let cases = [
        (vec![(TIP + 2, next)], "no header holds the winner's block"),
        (
            vec![(TIP + 1, forked), (TIP + 2, next)],
            "a fork replaced the winner's block",
        ),
        (
            vec![(TIP + 1, won)],
            "no header holds the ChainLock's block",
        ),
        (
            vec![(TIP + 1, won), (TIP + 2, forked)],
            "the ChainLock's block is not the header",
        ),
        (vec![], "no header"),
    ];
    for (chain, state) in cases {
        headers(&session, &chain);
        refused(&engine, &session, id, payment.txid(), state);
        let extras = engine
            .block_on(session.tx_detail_extras(id, payment.txid()))
            .unwrap();
        assert!(!extras.can_abandon, "{state}");
        assert!(held(&engine, &session, id).contains(&change), "{state}");
    }
    headers(&session, &both);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert!(
        !held(&engine, &session, id).contains(&change),
        "the change leaves with the abandon"
    );

    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    let mut want = outputs(&paid);
    want.remove(&b);
    want.insert(change_of(&winner, &own_scripts(&engine, &session, id)));
    assert_holds(
        &engine,
        &session,
        id,
        &want,
        "abandoned, then scanned again",
    );
    change_is_gone(&engine, &session, id, (&payment, change), "abandoned");
    assert!(
        !spendable(&engine, &session, id, change),
        "its change is not offered"
    );
    assert!(spendable(&engine, &session, id, a), "A is");
    assert!(!spendable(&engine, &session, id, b), "B is not");

    let session = reopen(&engine, session);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    assert_holds(&engine, &session, id, &want, "replayed after a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "replayed");
    assert!(
        !spendable(&engine, &session, id, change),
        "its change is not offered"
    );
    assert!(spendable(&engine, &session, id, a), "A is");
    assert!(!spendable(&engine, &session, id, b), "B is not");
}

/// #170 case 14, variant 1: a payment of A and B, InstantSend-locked, and
/// another device's spend of B that pays only someone else, mined in a plain
/// block and then ChainLocked: in the payment's own session, or after a
/// restart that leaves only its claims and its stored change. The ChainLock
/// sweeps the payment: its change leaves at once and B stays spent, with no
/// Abandon. After a restart the wallet holds A again, never the change or B.
#[test]
fn a_locked_payment_a_chainlocked_foreign_spend_beat_is_swept() {
    for restarted in [false, true] {
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
        let change = change_of(&payment, &own_scripts(&engine, &session, id));
        let owner = Address::from_script(&paid.output[1].script_pubkey, dashcore::Network::Regtest)
            .unwrap();
        let winner = super::signed_by_wallet(
            &engine,
            &session,
            id,
            &owner,
            super::spend_of(b, vec![(elsewhere(), COIN - 1_000)]),
        );
        let session = if restarted {
            let session = reopen(&engine, session);
            mined(&engine, &session, id, TIP, vec![paid.clone()]);
            session
        } else {
            session
        };
        let state = if restarted {
            "after a restart"
        } else {
            "in its own session"
        };

        mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
        assert!(
            in_history(&session, id, payment.txid()),
            "{state}: the lock spares it"
        );
        chain_lock(
            &engine,
            &session,
            id,
            TIP + 1,
            block_hash(std::slice::from_ref(&winner)),
        );

        assert!(
            !in_history(&session, id, payment.txid()),
            "{state}: it is swept"
        );
        change_is_gone(&engine, &session, id, (&payment, change), state);
        assert!(
            !spendable(&engine, &session, id, change),
            "{state}: its change is not offered"
        );
        assert!(!spendable(&engine, &session, id, b), "{state}: B is not");

        let session = reopen(&engine, session);
        let mut want = outputs(&paid);
        want.remove(&b);
        assert_holds(&engine, &session, id, &want, state);
        assert!(
            !in_history(&session, id, payment.txid()),
            "{state}, then a restart"
        );
        change_is_gone(&engine, &session, id, (&payment, change), "a restart");
        assert!(
            spendable(&engine, &session, id, a),
            "{state}, then a restart: A is"
        );
        assert!(
            !spendable(&engine, &session, id, b),
            "{state}, then a restart: B is not"
        );
    }
}

/// #170 case 14, variant 1, with a restart between the winner's block and
/// its ChainLock: a payment of A and B from a mined funding block,
/// InstantSend-locked, and another device's spend of B that pays only
/// someone else, mined in a plain block the lock spares. After a restart the
/// block is ChainLocked and nothing is swept; then the user runs Rescan from
/// the wallet's birth, which replays the funding block and the winner's,
/// ChainLocked: the payment is swept on arrival and its change leaves.
/// Returns the session, the funding transaction, A and B.
fn swept_by_a_rescan_after_a_restart() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    OutPoint,
    OutPoint,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let owner =
        Address::from_script(&paid.output[1].script_pubkey, dashcore::Network::Regtest).unwrap();
    let winner = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(b, vec![(elsewhere(), COIN - 1_000)]),
    );
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    assert!(
        in_history(&session, id, payment.txid()),
        "the lock spares it"
    );

    let won = block_hash(std::slice::from_ref(&winner));
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 1, won);
    assert!(
        in_history(&session, id, payment.txid()),
        "a ChainLock after a restart sweeps nothing"
    );
    assert!(
        !spendable(&engine, &session, id, a),
        "A is held before the rescan"
    );

    let funded = block_hash(std::slice::from_ref(&paid));
    headers(&session, &[(TIP, funded), (TIP + 1, won)]);
    start_spv(&engine, &session);
    engine
        .block_on(session.rescan(crate::RescanFrom::WalletBirth))
        .unwrap();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert!(
        !in_history(&session, id, payment.txid()),
        "the rescan's ChainLocked block sweeps it"
    );
    change_is_gone(&engine, &session, id, (&payment, change), "the rescan");
    (dir, engine, session, id, paid, a, b)
}

/// After [`swept_by_a_rescan_after_a_restart`], a restart, and the next,
/// load A and never B: the balance counts A and not B.
#[test]
fn a_locked_payment_a_foreign_spend_beat_before_a_restart_is_swept_by_a_rescan() {
    let (_dir, engine, mut session, id, paid, a, b) = swept_by_a_rescan_after_a_restart();
    let mut want = outputs(&paid);
    want.remove(&b);
    for round in ["a restart", "a second restart"] {
        session = reopen(&engine, session);
        assert!(held(&engine, &session, id).contains(&a), "{round}: A loads");
        assert!(spendable(&engine, &session, id, a), "{round}: A is offered");
        assert!(!spendable(&engine, &session, id, b), "{round}: B is not");
        assert_holds(&engine, &session, id, &want, round);
        assert_eq!(
            balances(&session, id).total,
            (COINS as u64 - 1) * COIN,
            "{round}: the balance counts A, not B"
        );
    }
}

/// The rescan that sweeps the payment credits A in its own session.
#[test]
#[ignore = "case 11: a rescan replays A's funding block before the block that sweeps its claimant; A returns after a restart"]
fn a_rescan_that_sweeps_a_beaten_payment_credits_a_in_its_session() {
    let (_dir, engine, session, id, _paid, a, b) = swept_by_a_rescan_after_a_restart();
    assert!(
        held(&engine, &session, id).contains(&a),
        "the rescan credits A"
    );
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(!spendable(&engine, &session, id, b), "B is not");
}

/// A payment of A and B from a mined funding block, InstantSend-locked;
/// after a restart another device's payment of A alone; after the next,
/// another device's spend of B is mined and ChainLocked, which sweeps the
/// locked payment. No block spends A: only the other payment holds it. The
/// user drops the other payment. The wallet recorded it with no input of
/// its own, so the drop rescans nothing and A is not credited in that
/// session; the store frees A, and A loads after a restart, never B.
#[test]
fn a_coin_only_a_dropped_payment_held_after_a_sweep_returns_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
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
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let back = fresh_address(&engine, &session, id);
    let other = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(back.script_pubkey(), COIN - 2_000)]),
    );
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[other.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(std::slice::from_ref(&winner));
    chain_lock(&engine, &session, id, TIP + 1, won);
    assert!(
        !in_history(&session, id, payment.txid()),
        "the ChainLock sweeps it"
    );
    let funded = block_hash(std::slice::from_ref(&paid));
    headers(&session, &[(TIP, funded), (TIP + 1, won)]);
    start_spv(&engine, &session);
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        1,
        "only the other payment"
    );
    assert!(abandoned(&engine, &session, id, other.txid()));
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A is not credited"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert_eq!(
        synced(&engine, &session, id),
        TIP + 1,
        "the drop rescans nothing"
    );

    let mut session = session;
    let back = OutPoint::new(other.txid(), 0);
    for round in ["a restart", "a second restart"] {
        session = reopen(&engine, session);
        assert!(held(&engine, &session, id).contains(&a), "{round}: A loads");
        assert!(spendable(&engine, &session, id, a), "{round}: A is offered");
        assert!(!spendable(&engine, &session, id, b), "{round}: B is not");
        assert!(
            !held(&engine, &session, id).contains(&back),
            "{round}: the dropped payment's output is gone"
        );
    }
}

fn in_history(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    session.hub.history.get(&id, &txid).is_some()
}

/// A payment of A and B, InstantSend-locked, and after a restart another
/// payment of A alone; after the next, another device's spend of B is mined
/// beside a spend of A that pays only someone else, which the wallet does not
/// record, and ChainLocked in that session. The ChainLock sweeps the locked
/// payment and its change. Its claim on A was disputed by the other payment's,
/// so the sweep releases nothing: A stays spent, by the block's spend of it.
/// Dropping the other payment and a restart leave A spent too.
#[test]
fn a_locked_payment_a_chainlock_beat_beside_a_spend_of_its_other_coin_leaves_a_spent() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let owner =
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let back = fresh_address(&engine, &session, id);
    let other = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(back.script_pubkey(), COIN - 2_000)]),
    );
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[other.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    let elsewhere_spend = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    let block = [winner.clone(), elsewhere_spend.clone()];
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    wait_stored(&session, id, &[winner.txid()]);
    chain_lock(&engine, &session, id, TIP + 1, block_hash(&block));

    assert!(
        !in_history(&session, id, payment.txid()),
        "the ChainLock sweeps it"
    );
    change_is_gone(&engine, &session, id, (&payment, change), "swept");
    change_is_gone(&engine, &session, id, (&paid, a), "A, spent in the block");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    headers(&session, &[(TIP + 1, block_hash(&block))]);
    start_spv(&engine, &session);
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        1,
        "only the other payment"
    );
    assert!(abandoned(&engine, &session, id, other.txid()));

    let session = reopen(&engine, session);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A stays spent after a restart"
    );
    assert!(!in_history(&session, id, payment.txid()), "a restart");
    change_is_gone(&engine, &session, id, (&paid, a), "A after a restart");
    assert!(
        !spendable(&engine, &session, id, a),
        "A is not offered after a restart"
    );
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
}

/// A payment of A and B, InstantSend-locked, and its child spending its
/// change, both InstantSend-locked; after a restart another device's spend
/// of B is mined and ChainLocked, and after the next the funding block is
/// scanned again.
/// The child's lock outlives an abandon of the payment, so Abandon and
/// Remove Unconfirmed refuse both and nothing is removed.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_under_a_locked_child() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(true);
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    refused(&engine, &session, id, payment.txid(), "a locked child");
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        0
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(!abandoned(&engine, &session, id, child.txid()));
    assert!(held(&engine, &session, id).contains(&child_change));
}

/// As above, but the child is abandoned first and comes back with its
/// lock after the restarts, still marked abandoned in the history.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_under_a_child_abandoned_then_locked() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    abandon(&engine, &session, id, child.txid());
    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    instant_lock(&engine, &session, id, &child);
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "a child locked again",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(held(&engine, &session, id).contains(&child_change));
}

/// As above, but the child, not locked, comes back after the restarts and
/// its lock arrives while the abandon reads the headers: after it has read
/// the history, before it applies.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_when_its_child_locks_meanwhile() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    let session = reopen(&engine, session);
    let (session, won) = beaten_on_b(&engine, session, id, &paid);
    mempool(&engine, &session, id, &child);
    let pause = Arc::new(crate::coins::TestPause::default());
    {
        let pause = Arc::clone(&pause);
        session.hub.attach_headers(Arc::new(move |h| {
            let pause = Arc::clone(&pause);
            Box::pin(async move {
                let first = !pause
                    .reached
                    .swap(true, std::sync::atomic::Ordering::SeqCst);
                if first {
                    pause.release.notified().await;
                }
                (h == TIP + 1).then_some(won)
            })
        }));
    }
    let abandoning = {
        let s = Arc::clone(&session);
        let txid = payment.txid();
        session.rt.spawn(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        })
    };
    eventually("the abandon's header read", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    instant_lock(&engine, &session, id, &child);
    pause.release.notify_one();
    let result = engine.block_on(abandoning).unwrap();
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::InstantLocked
            ))
        ),
        "{result:?}"
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(!abandoned(&engine, &session, id, child.txid()));
    assert!(held(&engine, &session, id).contains(&child_change));
}

/// As above, but a block holding the child reaches key-wallet before the
/// history hears of it, which still has the child out of a block.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_under_a_child_mined_unheard() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    unheard(&engine, &session, id, &child, Some(TIP + 2));
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "a child mined unheard",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
}

/// As above, but key-wallet finalized the child in a ChainLocked block and
/// kept only its txid before the history heard of it.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_under_a_child_finalized_unheard() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    let session = reopen(&engine, session);
    let (session, won) = beaten_on_b(&engine, session, id, &paid);
    let block = super::block_with(vec![child.clone()]);
    let (manager, hash, txid) = (session.manager().unwrap(), block.block_hash(), child.txid());
    let finalized_only = engine.block_on(async move {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        (wm.process_block_for_wallets(&block, hash, TIP + 2, &BTreeSet::from([id.0]))).await;
        wm.apply_chain_lock(dashcore::ChainLock {
            block_height: TIP + 2,
            block_hash: hash,
            signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
        });
        wm.update_wallet_synced_height(&id.0, TIP + 2);
        let accounts = wm
            .get_wallet_info(&id.0)
            .unwrap()
            .core_wallet
            .accounts
            .all_accounts();
        (accounts.iter()).any(|a| a.transaction_is_finalized(&txid))
            && (accounts.iter()).all(|a| !a.transactions().contains_key(&txid))
    });
    assert!(finalized_only, "key-wallet keeps the child's txid alone");
    headers(&session, &[(TIP + 1, won), (TIP + 2, hash)]);
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "a child finalized unheard",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
}

/// As above, with the child locked, but its record cannot be read at the
/// wallet's load.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_after_a_load_missing_its_child() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(true);
    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(child.txid());
    engine.block_on(session.load_wallet(id)).unwrap();
    headers(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "a load missing the child",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
}

/// A payment of A and B, InstantSend-locked; after the restarts a child
/// spending its change reaches key-wallet before the history hears of it.
/// The abandon would remove a transaction whose inputs it cannot check, so
/// it refuses.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_under_a_child_the_history_lacks() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let owner = Address::from_script(
        &payment.output[change.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();

    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    let back = fresh_address(&engine, &session, id);
    let child = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(change, vec![(back.script_pubkey(), COIN / 4)]),
    );
    unheard(&engine, &session, id, &child, None);
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "a child the history lacks",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
}

/// `tx` reaches key-wallet, from the mempool or in a block at `height`,
/// and no event of it reaches the hub: the history does not hear of it.
fn unheard(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    tx: &Transaction,
    height: Option<u32>,
) {
    use platform_wallet::key_wallet_manager::WalletInterface;
    let manager = session.manager().unwrap();
    let tx = tx.clone();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        match height {
            None => assert!(wm.process_mempool_transaction(&tx, None).await.is_relevant),
            Some(height) => {
                let block = super::block_with(vec![tx]);
                (wm.process_block_for_wallets(
                    &block,
                    block.block_hash(),
                    height,
                    &BTreeSet::from([id.0]),
                ))
                .await;
            }
        }
    });
}

/// A payment of A and B, InstantSend-locked, and its child spending its
/// change, InstantSend-locked when `locked`; both stored.
#[allow(clippy::type_complexity)]
fn payment_and_child(
    locked: bool,
) -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 4,
        false,
    );
    mempool(&engine, &session, id, &child);
    if locked {
        instant_lock(&engine, &session, id, &child);
    }
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    (dir, engine, session, id, paid, payment, child)
}

/// Another device's spend of `paid`'s B is mined at `TIP + 1` and
/// ChainLocked; after a restart the SPV header chain holds its block, the
/// funding block is scanned again and SPV is started. Returns the session
/// and the block's hash.
fn beaten_on_b(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
) -> (Arc<NetworkSession>, dashcore::BlockHash) {
    let winner = rival(engine, &session, id, paid, 1);
    mined(engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    let session = chain_lock_after_a_restart(engine, session, id, TIP + 1, won);
    let session = reopen(engine, session);
    headers(&session, &[(TIP + 1, won)]);
    mined(engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(engine, &session);
    (session, won)
}

/// A payment of A and B, InstantSend-locked, and after a restart another
/// payment of A alone; after the next, another device's spend of B is
/// mined and ChainLocked beside a spend of A that pays only someone else.
/// After a restart a replay passes the funding block but not the
/// ChainLocked one: Abandon refuses until it does, and then A stays spent,
/// after an abandon of the other payment and a restart too.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_while_the_scan_replays() {
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
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let back = fresh_address(&engine, &session, id);
    let other = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(back.script_pubkey(), COIN - 2_000)]),
    );
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[other.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    let elsewhere_spend = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    let block = [winner.clone(), elsewhere_spend];
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    wait_stored(&session, id, &[winner.txid()]);
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 1, block_hash(&block));

    let session = reopen(&engine, session);
    headers(&session, &[(TIP + 1, block_hash(&block))]);
    start_spv(&engine, &session);
    let s = Arc::clone(&session);
    engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.rewind_recorded(&manager, id, TIP - 1).await
        }))
        .unwrap();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&a), "replayed to A");
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    refused(&engine, &session, id, payment.txid(), "the replay below it");
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    engine
        .block_on(session.abandon_transaction(id, other.txid()))
        .unwrap();
    assert!(!held(&engine, &session, id).contains(&a), "A is spent");

    let session = reopen(&engine, session);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A stays spent after a restart"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// A payment of A and B, InstantSend-locked; after a restart another
/// device's spend of B is mined and ChainLocked, and a spend of A that
/// pays the wallet is recorded in a block the SPV header chain does not
/// hold at its height. Nothing proves A spent, so Abandon refuses.
#[test]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_while_a_replaced_block_spends_its_coin() {
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
    let owner =
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let back = fresh_address(&engine, &session, id);
    let forked = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(back.script_pubkey(), COIN - 2_000)]),
    );
    mined(&engine, &session, id, TIP + 2, vec![forked.clone()]);
    wait_stored(&session, id, &[winner.txid(), forked.txid()]);
    let next = dashcore::BlockHash::from_byte_array([2; 32]);
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 2, next);

    let session = reopen(&engine, session);
    headers(
        &session,
        &[(TIP + 1, block_hash(&[winner])), (TIP + 2, next)],
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    refused(
        &engine,
        &session,
        id,
        payment.txid(),
        "A's spender replaced",
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
}

/// The payment InstantSend-locked; in the same session another device's
/// spend of B, paying the wallet change, is mined in a plain block and
/// ChainLocked after. The ChainLock sweeps the payment: it leaves the
/// history, so there is nothing to abandon, and its change leaves the
/// balance. A is not back until a restart (case 11); then the wallet holds
/// A and the winner's change, never B.
#[test]
fn a_locked_payment_a_chainlock_beat_in_its_own_session_is_swept() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    assert!(
        in_history(&session, id, payment.txid()),
        "the lock spares it"
    );
    chain_lock(
        &engine,
        &session,
        id,
        TIP + 1,
        block_hash(std::slice::from_ref(&winner)),
    );

    assert!(
        !in_history(&session, id, payment.txid()),
        "the ChainLock sweeps it"
    );
    let gone = engine.block_on(session.tx_detail_extras(id, payment.txid()));
    assert!(
        matches!(gone, Err(crate::EngineError::TxNotFound(_))),
        "nothing to abandon: {gone:?}"
    );
    change_is_gone(&engine, &session, id, (&payment, change), "its own session");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
    // Case 11: A is not back until a restart. Nothing counts or offers it.
    change_is_gone(&engine, &session, id, (&paid, a), "A in its own session");
    assert!(!spendable(&engine, &session, id, a), "no draft takes A");

    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&b);
    want.insert(change_of(&winner, &own_scripts(&engine, &session, id)));
    assert_holds(&engine, &session, id, &want, "swept, then a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(!spendable(&engine, &session, id, b), "B is not");
}

/// The abandon of a locked payment a ChainLock beat stops after its
/// abandoned mark is stored and before its sweep, as at an exit. The next
/// open shows the payment InstantSend-locked, as before the abandon, its
/// change counted, and replays from the rewind the abandon stored; once
/// the replay is done a second abandon ends it, after a restart too.
#[test]
fn an_override_abandon_cut_before_its_sweep_ends_with_the_next() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(std::slice::from_ref(&winner));
    let session = chain_lock_after_a_restart(&engine, session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    headers(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    (session.hub.quit_before_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
    let cut = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(cut, Err(crate::EngineError::Internal(_))),
        "{cut:?}"
    );

    let session = reopen(&engine, session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.abandoned, "shown abandoned");
    let row = (super::rows(&engine, &session, id).into_iter())
        .find(|r| r.0 == payment.txid().to_string())
        .unwrap();
    assert!(row.2, "shown InstantSend-locked: {row:?}");
    assert!(held(&engine, &session, id).contains(&change));
    headers(&session, &[(TIP + 1, won)]);
    start_spv(&engine, &session);
    let state = "the cut abandon's replay";
    refused(&engine, &session, id, payment.txid(), state);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert!(!held(&engine, &session, id).contains(&change));

    let session = reopen(&engine, session);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert!(!held(&engine, &session, id).contains(&change));
}

/// #170 case 14 as it most often comes: the payment InstantSend-locked, a
/// restart, then another device's spend of B mined and ChainLocked in that
/// session. The ChainLock sweeps the payment, known only by its claims and
/// its stored change: it leaves the history, its change leaves the balance,
/// and once the funding block is scanned again in that session A is
/// credited, with no Abandon. After a restart A loads, never B.
#[test]
fn a_locked_payment_beaten_after_a_restart_is_swept_and_a_rescan_credits_a() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    chain_lock(&engine, &session, id, TIP + 1, won);
    assert!(
        !in_history(&session, id, payment.txid()),
        "the ChainLock sweeps it"
    );
    change_is_gone(&engine, &session, id, (&payment, change), "swept");
    change_is_gone(&engine, &session, id, (&paid, a), "A before the rescan");

    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "the rescan credits A"
    );
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(!spendable(&engine, &session, id, b), "B is not");

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&a),
        "A loads after a restart"
    );
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
}

/// A payment of B with two outputs to the wallet, not InstantSend-locked,
/// and a locked child spending one of them; after a restart another
/// device's spend of B is mined and ChainLocked. Abandon removes the
/// payment and its other output, as without a ChainLock, and leaves the
/// child: the lock exception's rules are only for a locked payment.
#[test]
fn abandoning_an_unlocked_payment_a_chainlock_beat_under_a_locked_child_drops_its_other_output() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let b = OutPoint::new(paid.txid(), 1);
    let owner =
        Address::from_script(&paid.output[1].script_pubkey, dashcore::Network::Regtest).unwrap();
    let (to_child, other) = (
        fresh_address(&engine, &session, id),
        fresh_address(&engine, &session, id),
    );
    let payment = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(
            b,
            vec![
                (to_child.script_pubkey(), COIN / 2),
                (other.script_pubkey(), COIN / 2 - 1_000),
            ],
        ),
    );
    mempool(&engine, &session, id, &payment);
    let child = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &to_child,
        super::spend_of(
            OutPoint::new(payment.txid(), 0),
            vec![(elsewhere(), COIN / 2 - 1_000)],
        ),
    );
    mempool(&engine, &session, id, &child);
    instant_lock(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    let left = OutPoint::new(payment.txid(), 1);

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&left));
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    chain_lock(&engine, &session, id, TIP + 1, won);
    headers(&session, &[(TIP + 1, won)]);
    start_spv(&engine, &session);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert!(!abandoned(&engine, &session, id, child.txid()));
    assert!(
        !held(&engine, &session, id).contains(&left),
        "its other output leaves"
    );

    let session = reopen(&engine, session);
    assert!(
        !held(&engine, &session, id).contains(&left),
        "its other output stays gone after a restart"
    );
}

/// A payment of A and B, not InstantSend-locked; after a restart another
/// device's spend of B is mined and ChainLocked. Abandoning the payment
/// frees A on disk at once, as without a ChainLock: a restart before any
/// scan holds A, never B.
#[test]
fn abandoning_an_unlocked_payment_a_chainlock_beat_frees_its_other_coin_after_a_restart() {
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
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    chain_lock(&engine, &session, id, TIP + 1, won);
    headers(&session, &[(TIP + 1, won)]);
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&a), "A is free");
    assert!(!held(&engine, &session, id).contains(&b), "B is not");
}

/// #170 case 14: the beaten payment, abandoned, comes back from a peer
/// after a restart, first without its InstantSend lock, then with it. It
/// should stay abandoned and its change uncounted; intake takes it as live
/// again, as without the abandon.
#[test]
#[ignore = "#170 case 14: intake does not honour a ChainLock defeat"]
fn an_abandoned_payment_a_chainlock_beat_stays_abandoned_when_redelivered() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();

    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &payment);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    change_is_gone(&engine, &session, id, (&payment, change), "redelivered");
    instant_lock(&engine, &session, id, &payment);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    change_is_gone(
        &engine,
        &session,
        id,
        (&payment, change),
        "redelivered locked",
    );
}
