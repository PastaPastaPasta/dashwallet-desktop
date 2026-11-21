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
/// someone else is mined. Abandoning the payment frees A only: both stay
/// stored spent, the replay the abandon starts credits A, and B is never
/// counted or offered.
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
    want.remove(&a);
    want.remove(&b);
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    mined(&engine, &session, id, TIP - 1, vec![paid.clone()]);
    mined(&engine, &session, id, TIP, vec![winner]);
    want.insert(a);
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

/// Another device's spend of `coin`, paying only someone else, as dash-spv
/// hands its mempool transactions to the wallet: key-wallet finds it not
/// the wallet's (the coin is claimed), so dash-spv keeps neither it nor its
/// InstantSend lock.
fn unheard_spend(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    funding: &Transaction,
    coin: OutPoint,
) -> Transaction {
    let owner = Address::from_script(
        &funding.output[coin.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let winner = super::signed_by_wallet(
        engine,
        session,
        id,
        &owner,
        super::spend_of(coin, vec![(elsewhere(), COIN - 1_000)]),
    );
    let (manager, tx) = (session.manager().unwrap(), winner.clone());
    let relevant = engine.block_on(async move {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        wm.process_mempool_transaction(&tx, None).await.is_relevant
    });
    assert!(!relevant, "key-wallet finds the spend not the wallet's");
    winner
}

/// #170 case 5, the issue's probe. A restored payment of A and B; after a
/// restart another device's mempool spend of B paying only someone else
/// goes unrecorded and is InstantSend-locked, and the payment is abandoned.
/// B stays stored spent: after the next restart it is neither counted nor
/// offered. A comes back when the scan passes its funding block, and B
/// stays out once the winner is mined.
#[test]
fn abandoning_a_payment_an_unheard_locked_spend_beat_keeps_its_coin_spent_after_a_restart() {
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
    let winner = unheard_spend(&engine, &session, id, &paid, b);
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&a);
    want.remove(&b);
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");

    mined(&engine, &session, id, TIP, vec![paid.clone(), winner]);
    want.insert(a);
    assert_holds(&engine, &session, id, &want, "funding and winner mined");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
    assert!(spendable(&engine, &session, id, a), "A is");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
}

/// As the probe, with the funding block scanned before the payment, and the
/// winner mined in a block ChainLocked before the abandon: key-wallet drops
/// its note of the spend once the scan is past that block, so only the
/// store's own row keeps B out after the next restart.
#[test]
fn abandoning_a_payment_a_chainlocked_foreign_spend_beat_keeps_its_coin_spent_after_a_restart() {
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
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let hash = super::block_with(vec![winner.clone()]).block_hash();
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.apply_chain_lock(dashcore::ChainLock {
                block_height: TIP + 1,
                block_hash: hash,
                signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
            });
        })
    });
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    let manager = session.manager().unwrap();
    let noted = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        info.core_wallet.observed_spent_outpoints().contains_key(&b)
    });
    assert!(!noted, "key-wallet no longer notes the spend of B");
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&a);
    want.remove(&b);
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");

    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    want.insert(a);
    assert_holds(&engine, &session, id, &want, "replayed after a restart");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
    assert!(spendable(&engine, &session, id, a), "A is");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
}

/// An abandon in the session the payment was sent, nothing beating it:
/// both coins come back once the scan passes their funding block, and are
/// stored unspent, so a restart after holds them too.
#[test]
fn abandoning_a_payment_frees_its_coins_once_their_block_is_scanned_and_after_a_restart() {
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
    wait_stored(&session, id, &[payment.txid()]);
    abandon(&engine, &session, id, payment.txid());
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let want = outputs(&paid);
    assert_holds(&engine, &session, id, &want, "abandoned, then scanned");
    assert!(spendable(&engine, &session, id, a), "A is offered");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
}

/// A restored payment a peer relays again after a restart: key-wallet's
/// record of it, stored over the first, names none of the wallet's inputs.
/// Abandoning it after the next restart still schedules a rescan from its
/// coins' funding block, which stores them unspent again.
#[test]
fn abandoning_a_relayed_payment_rescans_the_coins_it_keeps_stored_spent() {
    use platform_wallet::changeset::PlatformWalletPersistence;
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
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &payment);
    let persister = Arc::clone(&session.live().unwrap().persister);
    eventually("the relayed record is stored", || {
        persister.flush(id.0).unwrap();
        (persister.get_core_tx_record(id.0, &payment.txid()).unwrap())
            .is_some_and(|r| r.input_details.is_empty())
    });
    drop(persister);

    let session = reopen(&engine, session);
    abandon(&engine, &session, id, payment.txid());
    assert!(synced(&engine, &session, id) < TIP, "a rescan is due");
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let want = outputs(&paid);
    assert_holds(&engine, &session, id, &want, "rescanned");
    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
}

/// The abandon finds a coin's row in wallet.sqlite under the key the store
/// writes, whatever the width of its vout, and no row for a coin it never
/// stored.
#[test]
fn the_abandon_finds_each_stored_coin_by_its_key() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = fresh_address(&engine, &session, id);
    let txid = Txid::from_byte_array([3; 32]);
    let coin = |vout: u32| {
        key_wallet::Utxo::new(
            OutPoint::new(txid, vout),
            TxOut {
                value: COIN,
                script_pubkey: to.script_pubkey(),
            },
            to.clone(),
            FUNDED_AT,
            false,
        )
    };
    let vouts = [0, 250, 251, 0xFFFF, 0x1_0000, u32::MAX];
    store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            new_utxos: vouts.iter().map(|v| coin(*v)).collect(),
            ..Default::default()
        },
    );
    let stored: BTreeSet<OutPoint> = vouts.iter().map(|v| OutPoint::new(txid, *v)).collect();
    let mut asked = stored.clone();
    asked.insert(OutPoint::new(txid, 7));
    asked.insert(OutPoint::new(Txid::from_byte_array([4; 32]), 0));
    let db = session.data_dir().join(crate::session::WALLET_DB_FILE);
    assert_eq!(
        crate::tx_actions::stored_coins(&db, id, &asked).unwrap(),
        stored
    );
}
