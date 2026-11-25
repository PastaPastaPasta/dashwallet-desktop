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
    spendable_for(engine, session, id, coin, COIN / 2)
}

/// [`spendable`] for a payment of `amount`.
pub(super) fn spendable_for(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
    amount: u64,
) -> bool {
    let (_draft, prepared) = prepare(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![coin]),
        amount,
        false,
        &Arc::new(Accept::default()),
    );
    prepared.is_ok_and(|p| (p.summary().inputs.iter()).any(|i| i.outpoint == coin))
}

/// Waits up to 20 s for `done`.
pub(super) fn eventually(what: &str, done: impl FnMut() -> bool) {
    eventually_within(what, std::time::Duration::from_secs(20), done);
}

/// Waits up to `limit` for `done`.
fn eventually_within(what: &str, limit: std::time::Duration, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("never: {what}");
}

/// The wallet events key-wallet emits from now on, for [`wallet_story`].
pub(super) fn events_from(
    engine: &Engine,
    session: &NetworkSession,
) -> tokio::sync::broadcast::Receiver<platform_wallet::key_wallet_manager::WalletEvent> {
    let manager = session.manager().unwrap();
    engine.block_on(async move { manager.wallet_manager_arc().read().await.subscribe_events() })
}

/// For a failure's message: the history's txids, what the history and
/// key-wallet hold of `txids`, and the events `events` caught.
pub(super) fn wallet_story(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    txids: &[Txid],
    events: Option<
        &mut tokio::sync::broadcast::Receiver<platform_wallet::key_wallet_manager::WalletEvent>,
    >,
) -> String {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let history = session.hub.history.snapshot(&id);
    let shown: Vec<String> = (txids.iter())
        .map(|t| match history.get(t) {
            Some(e) => format!(
                "{t}: {:?} locked={} abandoned={}",
                e.context, e.instant_locked, e.abandoned
            ),
            None => format!("{t}: absent"),
        })
        .collect();
    let manager = session.manager().unwrap();
    let wanted = txids.to_vec();
    let records: Vec<String> = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&id.0) else {
            return vec!["no wallet".into()];
        };
        let held = info.core_wallet.transaction_history();
        (wanted.iter())
            .map(|t| match held.iter().find(|r| r.txid == *t) {
                Some(r) => format!("{t}: {:?}", r.context),
                None => format!("{t}: absent"),
            })
            .collect()
    });
    let caught: Vec<String> = events
        .map(|rx| {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|e| e.to_string().chars().take(300).collect())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "\nhistory txids: {:?}\nhistory: {shown:#?}\nkey-wallet records: {records:#?}\nevents: {caught:#?}",
        history.keys().collect::<Vec<_>>()
    )
}

/// Starts SPV and waits up to 300 s for it to run: `Running` waits for the
/// wallets' DashPay bring-up and then dash-spv's own start, which take
/// minutes on a loaded machine. A start that failed takes its supervisor
/// back, so the state reads `Stopped` again: that fails at once. The
/// shielded store opens after `Running`, as a wallet load (#399), which
/// [`super::reopen`]'s close waits out.
pub(super) fn start_offline_spv(engine: &Engine, session: &Arc<NetworkSession>) {
    engine.block_on(session.start_spv()).unwrap();
    eventually_within(
        "SPV runs",
        std::time::Duration::from_secs(300),
        || match session.spv_state().unwrap() {
            crate::SpvState::Running => true,
            crate::SpvState::Starting => false,
            crate::SpvState::Stopped => panic!("SPV did not start: Notice SpvError says why"),
        },
    );
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
        b.total + b.withheld,
        held.iter().map(|(_, v)| v).sum::<u64>(),
        "{state}: {b:?}"
    );
    b
}

/// [`assert_holds`] while the coins of `waiting` wait for the rescan an
/// abandon or a drop owes them (#818): key-wallet holds those of `want`,
/// but the balance counts none of them.
pub(super) fn assert_holds_waiting(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    want: &BTreeSet<OutPoint>,
    waiting: &BTreeSet<OutPoint>,
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
    assert_eq!(
        &session.hub.rescan_held(&id),
        waiting,
        "{state}: the coins waiting for their rescan"
    );
    let b = balances(session, id);
    let counted: u64 = (held.iter())
        .filter(|(o, _)| !waiting.contains(o))
        .map(|(_, v)| v)
        .sum();
    assert_eq!(b.total, counted, "{state}: {b:?}");
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
    let waiting = BTreeSet::from([a]);
    assert_holds_waiting(
        &engine,
        &session,
        id,
        &want,
        &waiting,
        "abandoned, then a restart",
    );
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

/// A wallet opened later in the session restores its pending payment's
/// claims through the one helper, so platform-wallet's claim mirror sees
/// them before the wallet processes a block.
#[test]
fn a_wallet_opened_later_restores_its_claims_through_the_helper() {
    let (_dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(0x3e, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let a = OutPoint::new(funded.txid(), 0);
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

    engine.block_on(session.unload_wallet(id)).unwrap();
    let restored =
        || (crate::history_ops::claims_restored().lock()).unwrap_or_else(|e| e.into_inner());
    restored().remove(&a);
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(
        restored().contains(&a),
        "the load claims P's coin through the helper"
    );
    assert!(
        !held(&engine, &session, id).contains(&a),
        "P's claim holds A"
    );
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

/// The same locked payment and plain winner (#170 case 14): Abandon and
/// Remove Unconfirmed refuse it while the winner's block is not
/// ChainLocked. Once it is, and the header chain holds it, Abandon asks
/// for a restart and a rescan (the payment's claim, restored at the load,
/// keeps A out); after them Remove Unconfirmed abandons the payment: its
/// change goes, and once the funding block and the winner's are scanned
/// again the wallet holds A and the winner's change, never B, in the
/// session and after a restart.
#[test]
fn a_locked_payment_a_chainlocked_spend_beat_is_abandoned_and_its_change_goes() {
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
    header_chain(
        &session,
        &[(TIP + 1, block_hash(std::slice::from_ref(&winner)))],
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    assert!(
        held(&engine, &session, id).contains(&change),
        "a plain block does not override the lock: the change stays"
    );
    start_spv(&engine, &session);
    refused(&engine, &session, id, payment.txid(), "a plain block");
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
    let first = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            first,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RescanFirst
            ))
        ),
        "{first:?}"
    );
    let session = reopen(&engine, session);
    header_chain(
        &session,
        &[(TIP + 1, block_hash(std::slice::from_ref(&winner)))],
    );
    start_spv(&engine, &session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
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
    chain_lock(&engine, &session, id, TIP + 1, won);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    let forked = dashcore::BlockHash::from_byte_array([1; 32]);
    let next = dashcore::BlockHash::from_byte_array([2; 32]);
    let both = [(TIP + 1, won), (TIP + 2, next)];
    chain_lock(&engine, &session, id, TIP + 2, next);
    header_chain(&session, &both);
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
        header_chain(&session, &chain);
        refused(&engine, &session, id, payment.txid(), state);
        let extras = engine
            .block_on(session.tx_detail_extras(id, payment.txid()))
            .unwrap();
        assert!(!extras.can_abandon, "{state}");
        assert!(held(&engine, &session, id).contains(&change), "{state}");
    }
    header_chain(&session, &both);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    let released = session.hub.last_release.lock().unwrap().take().unwrap();
    assert!(
        released.contains(&a) && !released.contains(&b),
        "the sweep releases A, never B: {released:?}"
    );
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

/// #170 case 14, variant 1: the winner pays only someone else and was seen
/// in an earlier process, so no history holds it. After a restart the scan
/// passes its block again, ChainLocked; the abandon should then go through.
/// It cannot yet: key-wallet does not record a spend of a claimed coin that
/// pays no wallet address, and it prunes the observed spend once its block
/// is ChainLocked and committed, so no evidence is left by the time the
/// scan ends.
#[test]
#[ignore = "#170 case 14 variant 1: needs key-wallet to keep a ChainLocked spend of a claimed coin"]
fn abandoning_a_locked_payment_a_chainlocked_foreign_spend_beat_after_a_rescan() {
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
    chain_lock(
        &engine,
        &session,
        id,
        TIP + 1,
        block_hash(std::slice::from_ref(&winner)),
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    start_spv(&engine, &session);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = outputs(&paid);
    want.remove(&b);
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
    assert_holds(&engine, &session, id, &want, "abandoned, then a restart");
    change_is_gone(&engine, &session, id, (&payment, change), "a restart");
}

/// A payment of A and B, InstantSend-locked, and after a restart another
/// payment of A alone; after the next another device's spend of B is mined and
/// ChainLocked beside a spend of A that pays only someone else, which the
/// wallet does not record. Nothing proves A free, so Abandon refuses the
/// locked payment as without a ChainLock, and abandoning the other payment
/// leaves A spent after a restart: the locked payment's record still
/// claims it.
#[test]
#[ignore = "RC blocker #818: a race in the block reconcile leaves the beaten payment live (phantom credit); the fix PR un-ignores this test"]
fn a_locked_payment_a_chainlock_beat_is_not_abandoned_while_its_other_coin_is_unproven() {
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
    header_chain(&session, &[(TIP + 1, block_hash(&block))]);
    start_spv(&engine, &session);
    let refused = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RescanFirst
            ))
        ),
        "A held by the payment's claim: {refused:?}"
    );
    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        0,
        "the other payment went when its spend was mined (#170 case 3)"
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(abandoned(&engine, &session, id, other.txid()));

    let session = reopen(&engine, session);
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A stays spent after a restart"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert!(held(&engine, &session, id).contains(&change));
}

/// A payment of A and B, InstantSend-locked; after a restart another
/// device's spend of B is mined and ChainLocked beside a spend of A that
/// pays only someone else. The payment's claim, restored at the load,
/// keeps A from the wallet's coins, so Abandon asks for a restart and a
/// rescan; after them the scan records the spend of A in the ChainLocked
/// block and Abandon goes through: once its rescan is done the wallet holds
/// neither A, B nor the payment's change, and offers neither coin.
#[test]
fn a_locked_payment_whose_other_coin_a_chainlocked_block_spent_goes_after_a_restart_and_a_rescan() {
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
    let winner = rival(&engine, &session, id, &paid, 1);
    let elsewhere_spend = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    let block = [winner.clone(), elsewhere_spend];
    let won = block_hash(&block);
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    wait_stored(&session, id, &[winner.txid()]);
    chain_lock(&engine, &session, id, TIP + 1, won);
    header_chain(&session, &[(TIP + 1, won)]);
    start_spv(&engine, &session);
    let refused = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RescanFirst
            ))
        ),
        "A held by the payment's claim: {refused:?}"
    );

    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    start_spv(&engine, &session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    let mut want = outputs(&paid);
    want.remove(&a);
    want.remove(&b);
    want.insert(change_of(&winner, &own_scripts(&engine, &session, id)));
    assert_holds(&engine, &session, id, &want, "abandoned");
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, block.to_vec());
    assert_holds(
        &engine,
        &session,
        id,
        &want,
        "abandoned, then scanned again",
    );
    change_is_gone(&engine, &session, id, (&payment, change), "abandoned");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
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
    mempool(&engine, &session, id, &child);
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
    let manager = session.manager().unwrap();
    let under_mined = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let core = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        (session.hub.history).read(&id, |txs| crate::tx_actions::settled_under(txs, core).1)
    });
    assert!(
        under_mined.contains(&payment.txid()),
        "the finalized txid alone puts the payment under a mined child"
    );
    header_chain(&session, &[(TIP + 1, won), (TIP + 2, hash)]);
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
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(child.txid());
    engine.block_on(session.load_wallet(id)).unwrap();
    header_chain(&session, &[(TIP + 1, won)]);
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
    chain_lock(engine, &session, id, TIP + 1, won);
    let session = reopen(engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
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
    chain_lock(&engine, &session, id, TIP + 1, block_hash(&block));

    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, block_hash(&block))]);
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
    assert!(
        abandoned(&engine, &session, id, other.txid()),
        "the other payment went when its spend was mined (#170 case 3)"
    );
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
    chain_lock(&engine, &session, id, TIP + 2, next);

    let session = reopen(&engine, session);
    header_chain(
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
/// spend of B, W, is mined in a plain block, and the payment stays: a plain
/// block can't overturn a lock, so A and B stay spent. Once a ChainLock
/// covers W's block, key-wallet drops the payment: it leaves the history
/// and key-wallet, with nothing to abandon; B, which W spent, is never
/// held or offered; A comes back, held and offered, and so does W's
/// change. So at once, after SPV starts and after a restart.
#[test]
fn a_locked_payment_a_chainlock_beat_in_its_own_session_leaves_at_the_chainlock() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let funded = balances(&session, id).total;
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
    let winner = rival(&engine, &session, id, &paid, 1);
    let (p, w, change) = (
        payment.txid(),
        winner.txid(),
        OutPoint::new(winner.txid(), 1),
    );
    let offered = |session: &Arc<NetworkSession>, coin: OutPoint| {
        let (_draft, prepared) = prepare(
            &engine,
            session,
            id,
            CoinSource::Outpoints(vec![coin]),
            COIN / 4,
            false,
            &Arc::new(Accept::default()),
        );
        prepared.is_ok_and(|p| (p.summary().inputs.iter()).any(|i| i.outpoint == coin))
    };
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[w]);
    let held_now = held(&engine, &session, id);
    assert!(
        session.hub.history.snapshot(&id).contains_key(&p),
        "a plain block took the payment: {}",
        wallet_story(&engine, &session, id, &[p, w], None)
    );
    assert!(
        !held_now.contains(&a) && !held_now.contains(&b),
        "a plain block gave back the payment's coins: {held_now:?}"
    );

    let won = block_hash(&[winner]);
    let mut events = events_from(&engine, &session);
    chain_lock(&engine, &session, id, TIP + 1, won);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let mut session = session;
    for state in ["the ChainLock", "SPV started", "a restart"] {
        match state {
            "SPV started" => start_spv(&engine, &session),
            "a restart" => {
                session = reopen(&engine, session);
                events = events_from(&engine, &session);
                header_chain(&session, &[(TIP + 1, won)]);
                start_spv(&engine, &session);
            }
            _ => {}
        }
        let mut story = || wallet_story(&engine, &session, id, &[p, w], Some(&mut events));
        assert!(
            !session.hub.history.snapshot(&id).contains_key(&p),
            "{state}: the payment is in the history{}",
            story()
        );
        assert!(
            matches!(
                engine.block_on(session.tx_detail_extras(id, p)),
                Err(crate::EngineError::TxNotFound(_))
            ),
            "{state}: the payment's details read{}",
            story()
        );
        if state != "the ChainLock" {
            assert!(
                matches!(
                    engine.block_on(session.abandon_transaction(id, p)),
                    Err(crate::EngineError::TxNotFound(_))
                ),
                "{state}: the payment can be abandoned{}",
                story()
            );
        }
        let appdb = Arc::clone(&session.live().unwrap().appdb);
        let (marks, read) = crate::tx_actions::load_abandoned(&appdb, id);
        assert!(
            read && !marks.copies.iter().any(|(t, _)| *t == p),
            "{state}: marked abandoned"
        );
        let held_now = held(&engine, &session, id);
        assert!(
            !held_now.contains(&b),
            "{state}: B, which W spent, held{}",
            story()
        );
        assert!(!spendable(&engine, &session, id, b), "{state}: B offered");
        assert!(held_now.contains(&a), "{state}: A not held{}", story());
        assert!(
            spendable(&engine, &session, id, a),
            "{state}: A not offered"
        );
        assert!(
            held_now.contains(&change),
            "{state}: W's change not held{}",
            story()
        );
        assert!(offered(&session, change), "{state}: W's change not offered");
        assert!(
            (0..payment.output.len() as u32).all(|v| !held_now.contains(&OutPoint::new(p, v))),
            "{state}: the payment's outputs held: {held_now:?}"
        );
        assert_eq!(
            balances(&session, id).total,
            funded - COIN + (COIN / 2 - 1_000),
            "{state}: the balance{}",
            story()
        );
    }
}

/// The abandon of a locked payment a ChainLock beat, with its unlocked
/// child, stops after both abandoned marks are stored and before its sweep,
/// as at an exit. The next open shows the payment InstantSend-locked, as it
/// was (its mark abandons nothing while it is locked), and the open's sweep
/// of marked payments takes the child, its output not counted; it replays
/// from the rewind the abandon stored, and once the replay is done a
/// second abandon of the payment takes it, after a restart too.
#[test]
fn an_override_abandon_cut_before_its_sweep_ends_with_the_next() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(std::slice::from_ref(&winner));
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    (session.hub.quit_before_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
    let cut = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(cut, Err(crate::EngineError::Internal(_))),
        "{cut:?}"
    );

    let session = reopen(&engine, session);
    assert!(
        !abandoned(&engine, &session, id, payment.txid()),
        "the payment"
    );
    let row = (super::rows(&engine, &session, id).into_iter())
        .find(|r| r.0 == payment.txid().to_string())
        .unwrap();
    assert!(row.2, "shown InstantSend-locked: {row:?}");
    assert!(abandoned(&engine, &session, id, child.txid()), "the child");
    assert!(!held(&engine, &session, id).contains(&child_change));
    header_chain(&session, &[(TIP + 1, won)]);
    start_spv(&engine, &session);
    let state = "the cut abandon's replay";
    refused(&engine, &session, id, payment.txid(), state);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    let mut session = session;
    for state in ["the retry", "a restart after it"] {
        if state != "the retry" {
            session = reopen(&engine, session);
        }
        assert!(abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(abandoned(&engine, &session, id, child.txid()), "{state}");
        assert!(
            !held(&engine, &session, id).contains(&child_change),
            "{state}: the child's output is counted"
        );
        assert!(
            !spendable(&engine, &session, id, child_change),
            "{state}: the child's output is offered"
        );
    }
}

/// A payment and its child, the child's first store held in the
/// persistence adapter with what it read before the abandon: an abandon
/// of the payment takes both, and the held store then commits the child's
/// record and change after the sweep. The child is never credited: not in
/// the session, and after a restart the open's sweep of marked payments
/// takes it again before the wallet loads.
#[test]
fn a_child_stored_after_its_abandons_sweep_is_never_credited() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    let child_txid = child.txid();
    let barriers = Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
    *session.live().unwrap().store.hold_store.lock().unwrap() = Some((
        Box::new(
            move |cs: &platform_wallet::changeset::PlatformWalletChangeSet| {
                (cs.core.as_ref()).is_some_and(|c| c.records.iter().any(|r| r.txid == child_txid))
            },
        ),
        Arc::clone(&barriers),
    ));
    mempool(&engine, &session, id, &child);
    barriers.0.wait();
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    start_spv(&engine, &session);
    let abandoned = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(abandoned.is_ok(), "{abandoned:?}");
    barriers.1.wait();
    eventually("the held store commits", || {
        super::own_spend_balance_tests::record_stored(&session, id, child_txid)
    });
    assert!(
        !held(&engine, &session, id).contains(&child_change),
        "the session"
    );
    let session = reopen(&engine, session);
    assert!(
        !super::own_spend_balance_tests::record_stored(&session, id, child_txid),
        "the child's record survives the open"
    );
    assert!(
        !held(&engine, &session, id).contains(&child_change),
        "the child's change is credited after a restart"
    );
    assert!(!spendable(&engine, &session, id, child_change));
    assert!(abandoned_or_gone(&engine, &session, id, child_txid));
    assert!(
        !held(&engine, &session, id).contains(&change),
        "the payment's change is credited after a restart"
    );
    assert!(!spendable(&engine, &session, id, change));
}

/// A payment P of the wallet's coin A, not InstantSend-locked here (the
/// wallet missed its lock), and its child C spending P's change,
/// InstantSend-locked in key-wallet and the history; then a restart.
fn payment_under_a_locked_child() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &child);
    instant_lock(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    super::wait_stored_locked(&session, id, child.txid());
    let session = reopen(&engine, session);
    (dir, engine, session, id, payment, child)
}

/// `payment` and its child are as the network holds them: neither is
/// abandoned, and the payment's coin is neither held nor offered, in the
/// session and after a restart (#446).
fn kept_under_a_settled_child(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    payment: &Transaction,
    child: &Transaction,
) {
    let a = payment.input[0].previous_output;
    let mut session = session;
    for state in ["the session", "a restart"] {
        if state != "the session" {
            session = reopen(engine, session);
        }
        assert!(!abandoned(engine, &session, id, payment.txid()), "{state}");
        assert!(!abandoned(engine, &session, id, child.txid()), "{state}");
        assert!(
            !held(engine, &session, id).contains(&a),
            "{state}: the payment's coin is held"
        );
        assert!(
            !spendable(engine, &session, id, a),
            "{state}: the payment's coin is offered"
        );
    }
}

/// A payment whose child is InstantSend-locked is held by the network too
/// (Dash locks a transaction only once its parents are): Abandon is not
/// offered and refuses, and the coin it spent is never offered (#446).
#[test]
fn abandoning_a_payment_under_a_locked_child_is_refused_and_its_coin_stays_spent() {
    let (_dir, engine, session, id, payment, child) = payment_under_a_locked_child();
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    kept_under_a_settled_child(&engine, session, id, &payment, &child);
}

/// P, its child C spending P's change, and C's child G spending C's
/// change, only G InstantSend-locked, all stored.
#[allow(clippy::type_complexity)]
fn payment_under_a_locked_grandchild() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &child);
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    let grandchild = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![child_change]),
        COIN / 16,
        false,
    );
    mempool(&engine, &session, id, &grandchild);
    instant_lock(&engine, &session, id, &grandchild);
    wait_stored(
        &session,
        id,
        &[payment.txid(), child.txid(), grandchild.txid()],
    );
    super::wait_stored_locked(&session, id, grandchild.txid());
    (dir, engine, session, id, payment, child)
}

/// Three generations: P, its child C spending P's change, and C's child G
/// spending C's change, only G InstantSend-locked; then a restart. G proves
/// C and so P: Abandon of P is not offered and is refused, and P's coin is
/// never offered (#446).
#[test]
fn abandoning_a_payment_under_a_locked_grandchild_is_refused_and_its_coin_stays_spent() {
    let (_dir, engine, session, id, payment, child) = payment_under_a_locked_grandchild();
    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    kept_under_a_settled_child(&engine, session, id, &payment, &child);
}

/// A payment whose InstantSend-locked child, sent from another device of
/// the wallet, has its record gone from the store while its verified lock
/// stays: the lock names the child's inputs, so it still proves the
/// payment. After a restart Abandon of the payment is not
/// offered and refuses, Remove Unconfirmed keeps it, and its coin stays
/// spent, also after another restart (#611).
#[test]
fn abandoning_a_payment_whose_locked_child_kept_only_its_lock_is_refused() {
    use platform_wallet::key_wallet_manager::WalletInterface;
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    // Sent from another device of the wallet: no sent mark keeps a copy of
    // it here. Delivered with its lock, as key-wallet drops an unlocked
    // spend of a held coin whose input isn't signed for it.
    let child = super::spend_of(
        change,
        vec![
            (elsewhere(), COIN / 8),
            (
                fresh_address(&engine, &session, id).script_pubkey(),
                COIN / 16,
            ),
        ],
    );
    let locked = child.clone();
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            let lock = InstantLock {
                txid: locked.txid(),
                inputs: locked.input.iter().map(|i| i.previous_output).collect(),
                ..InstantLock::default()
            };
            wm.process_mempool_instant_send_lock(&locked, lock).await;
        })
    });
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    super::wait_stored_locked(&session, id, child.txid());
    delete_stored_record(&session, id, child.txid());

    let session = reopen(&engine, session);
    assert!(
        !stored_record(&session, id, child.txid()),
        "the child's record came back"
    );
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    engine.block_on(session.drop_unconfirmed(Some(id))).unwrap();
    let mut session = session;
    for state in ["the session", "a restart"] {
        if state != "the session" {
            session = reopen(&engine, session);
        }
        assert!(!abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(
            !held(&engine, &session, id).contains(&a),
            "{state}: the payment's coin is held"
        );
        assert!(
            !spendable(&engine, &session, id, a),
            "{state}: the payment's coin is offered"
        );
    }
}

/// Whether the wallet's store holds `txid`'s record.
fn stored_record(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    session.live().unwrap().persister.flush(id.0).unwrap();
    let conn = rusqlite::Connection::open(session.data_dir().join(crate::session::WALLET_DB_FILE))
        .unwrap();
    conn.query_row(
        "SELECT count(*) FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2 \
         AND record_blob IS NOT NULL",
        rusqlite::params![&id.0[..], AsRef::<[u8]>::as_ref(&txid)],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

/// Deletes `txid`'s row from the wallet's store, as a lost record leaves
/// it; its lock and coin rows stay.
fn delete_stored_record(session: &NetworkSession, id: WalletId, txid: Txid) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    session.live().unwrap().persister.flush(id.0).unwrap();
    let conn = rusqlite::Connection::open(session.data_dir().join(crate::session::WALLET_DB_FILE))
        .unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let deleted = conn
        .execute(
            "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
            rusqlite::params![&id.0[..], AsRef::<[u8]>::as_ref(&txid)],
        )
        .unwrap();
    assert_eq!(deleted, 1);
}

/// As above, but the child's record doesn't read at the wallet's load, so
/// nothing links the grandchild to the payment: Abandon and Remove
/// Unconfirmed both refuse as `RecordUnreadable`, and the payment's coin
/// stays spent, in the session and after a restart that reads the child
/// (#611).
#[test]
fn abandoning_a_payment_after_a_load_missing_its_child_is_refused() {
    let (_dir, engine, session, id, payment, child) = payment_under_a_locked_grandchild();
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(child.txid());
    engine.block_on(session.load_wallet(id)).unwrap();
    start_spv(&engine, &session);
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RecordUnreadable(w)
            )) if w == id
        ),
        "{result:?}"
    );
    let dropped = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert!(
        matches!(
            dropped,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RecordUnreadable(w)
            )) if w == id
        ),
        "{dropped:?}"
    );
    let a = payment.input[0].previous_output;
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(
        !held(&engine, &session, id).contains(&a),
        "the payment's coin is held"
    );
    assert!(
        !spendable(&engine, &session, id, a),
        "the payment's coin is offered"
    );
    let session = reopen(&engine, session);
    kept_under_a_settled_child(&engine, session, id, &payment, &child);
}

/// A payment P from this wallet to a second loaded wallet, whose lock this
/// wallet missed, and the second wallet's InstantSend-locked spend C of
/// what it received, which touches no coin of the first; then a restart.
/// C is only the second wallet's, and still proves P: Abandon of P is not
/// offered and refuses, Remove Unconfirmed for all wallets keeps it, and
/// its coin stays spent (#611).
#[test]
fn abandoning_a_transfer_a_second_wallet_spent_locked_is_refused() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let a = OutPoint::new(paid.txid(), 0);
    let to = fresh_address(&engine, &session, other);
    let payment = send_to(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        &to.to_string(),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    wait_stored(&session, other, &[payment.txid()]);
    let received = (payment.output.iter())
        .position(|o| o.script_pubkey == to.script_pubkey())
        .unwrap();
    let child = super::spend_of(
        OutPoint::new(payment.txid(), received as u32),
        vec![
            (elsewhere(), COIN / 4),
            (
                fresh_address(&engine, &session, other).script_pubkey(),
                COIN / 8,
            ),
        ],
    );
    // Delivered with its lock: key-wallet drops an unlocked spend of a
    // held coin whose input isn't signed for it.
    instant_lock(&engine, &session, other, &child);
    wait_stored(&session, other, &[child.txid()]);
    super::wait_stored_locked(&session, other, child.txid());

    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    engine.block_on(session.drop_unconfirmed(None)).unwrap();
    let mut session = session;
    for state in ["the session", "a restart"] {
        if state != "the session" {
            session = reopen(&engine, session);
        }
        assert!(!abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(
            !held(&engine, &session, id).contains(&a),
            "{state}: A's coin is held"
        );
        assert!(
            !spendable(&engine, &session, id, a),
            "{state}: A's coin is offered"
        );
    }
}

/// A change chain R → P → Q of this wallet, Q paying a second loaded
/// wallet, whose InstantSend-locked spend C of it this wallet never sees;
/// then a restart. C proves Q, P and R across both wallets: Abandon of R is
/// not offered and refuses, Remove Unconfirmed for all wallets keeps R, P
/// and Q, and R's coin stays spent, in the session and after a restart
/// (#611 F1).
#[test]
fn abandoning_the_root_of_a_chain_a_second_wallet_spent_locked_two_hops_on_is_refused() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let a = OutPoint::new(paid.txid(), 0);
    let root = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &root);
    let middle = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change_of(&root, &own_scripts(&engine, &session, id))]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &middle);
    let to = fresh_address(&engine, &session, other);
    let last = send_to(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change_of(
            &middle,
            &own_scripts(&engine, &session, id),
        )]),
        &to.to_string(),
        COIN / 16,
        false,
    );
    mempool(&engine, &session, id, &last);
    let chain = [root.txid(), middle.txid(), last.txid()];
    wait_stored(&session, id, &chain);
    wait_stored(&session, other, &[last.txid()]);
    let received = (last.output.iter())
        .position(|o| o.script_pubkey == to.script_pubkey())
        .unwrap();
    let spend = super::spend_of(
        OutPoint::new(last.txid(), received as u32),
        vec![
            (elsewhere(), COIN / 32),
            (
                fresh_address(&engine, &session, other).script_pubkey(),
                COIN / 64,
            ),
        ],
    );
    instant_lock(&engine, &session, other, &spend);
    wait_stored(&session, other, &[spend.txid()]);
    super::wait_stored_locked(&session, other, spend.txid());

    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, root.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, root.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    engine.block_on(session.drop_unconfirmed(None)).unwrap();
    let mut session = session;
    for state in ["the session", "a restart"] {
        if state != "the session" {
            session = reopen(&engine, session);
        }
        for t in chain {
            assert!(!abandoned(&engine, &session, id, t), "{state}: {t}");
        }
        assert!(
            !held(&engine, &session, id).contains(&a),
            "{state}: R's coin is held"
        );
        assert!(
            !spendable(&engine, &session, id, a),
            "{state}: R's coin is offered"
        );
    }
}

/// A second loaded wallet whose stored record didn't read at its load: what
/// it settled could descend from this wallet's payment, so Abandon of an
/// unlocked payment here and Remove Unconfirmed for all wallets refuse as
/// `RecordUnreadable`. Removing that wallet and restoring it from its
/// recovery phrase clears it, and the Abandon goes through (#611).
#[test]
fn abandoning_while_a_second_wallet_loaded_incomplete_is_refused() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let gift = funding(9, &[fresh_address(&engine, &session, other)], COIN);
    mempool(&engine, &session, other, &gift);
    wait_stored(&session, other, &[gift.txid()]);
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
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(other)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(gift.txid());
    engine.block_on(session.load_wallet(other)).unwrap();
    start_spv(&engine, &session);
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RecordUnreadable(w)
            )) if w == other
        ),
        "{result:?}"
    );
    let dropped = engine.block_on(session.drop_unconfirmed(None));
    assert!(
        matches!(
            dropped,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RecordUnreadable(w)
            )) if w == other
        ),
        "{dropped:?}"
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(
        !spendable(&engine, &session, id, a),
        "the payment's coin is offered"
    );

    let wipe = session
        .vault()
        .authorize(GrantPurpose::Wipe, Some(&other.0), Credential::None)
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(other, wipe)).unwrap();
    let restored = second_wallet(&engine, &session);
    // The record that didn't read is gone with the old store: a load of the
    // restored one, failing that record, reads complete.
    engine.block_on(session.unload_wallet(restored)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(gift.txid());
    engine.block_on(session.load_wallet(restored)).unwrap();
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
}

/// As above, through Remove Unconfirmed: the payment is kept and its coin
/// stays spent (#446).
#[test]
fn removing_unconfirmed_keeps_a_payment_under_a_locked_child() {
    let (_dir, engine, session, id, payment, child) = payment_under_a_locked_child();
    start_spv(&engine, &session);
    let dropped = engine.block_on(session.drop_unconfirmed(Some(id))).unwrap();
    assert_eq!(dropped, 0, "the payment or its funding was dropped");
    kept_under_a_settled_child(&engine, session, id, &payment, &child);
}

/// As above, with the child in a block the history holds: refused the
/// same, not as the child's own confirmation (#446).
#[test]
fn abandoning_a_payment_under_a_mined_child_is_refused_and_its_coin_stays_spent() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &child);
    mined(&engine, &session, id, TIP + 1, vec![child.clone()]);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    kept_under_a_settled_child(&engine, session, id, &payment, &child);
}

fn abandoned_or_gone(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> bool {
    match engine.block_on(session.tx_detail_extras(id, txid)) {
        Ok(x) => x.abandoned,
        Err(crate::EngineError::TxNotFound(_)) => true,
        Err(e) => panic!("{e:?}"),
    }
}

/// An unlocked payment and its child, the child abandoned; after a
/// restart the child comes back InstantSend-locked, stored before the
/// history hears of it. Abandoning the payment then is refused as locked:
/// the child is never swept, and after a restart its record, its lock and
/// its change are as the network left them.
#[test]
fn abandoning_a_payment_under_a_child_locked_before_the_history_heard_is_refused() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    start_spv(&engine, &session);
    abandon(&engine, &session, id, child.txid());

    let session = reopen(&engine, session);
    let manager = session.manager().unwrap();
    let locked = child.clone();
    engine.block_on(async move {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let lock = InstantLock {
            txid: locked.txid(),
            ..InstantLock::default()
        };
        wm.process_mempool_transaction(&locked, Some(lock)).await;
    });
    wait_stored(&session, id, &[child.txid()]);
    assert!(
        (session.hub.history.get(&id, &child.txid())).is_some_and(|e| e.is_abandoned()),
        "the history heard of the lock"
    );
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
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
    let session = reopen(&engine, session);
    assert!(
        super::own_spend_balance_tests::record_stored(&session, id, child.txid()),
        "the child's record is gone"
    );
    assert!(
        held(&engine, &session, id).contains(&child_change),
        "the child's change is not held"
    );
    assert!(
        !abandoned(&engine, &session, id, payment.txid()),
        "the payment"
    );
}

/// As above, but the abandoned child comes back in a block, which
/// key-wallet records before the history hears of it. Abandoning the
/// payment is refused as confirmed, and the child's change stays held.
#[test]
fn abandoning_a_payment_under_a_child_mined_before_the_history_heard_is_refused() {
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
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 8,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    start_spv(&engine, &session);
    abandon(&engine, &session, id, child.txid());

    let session = reopen(&engine, session);
    unheard(&engine, &session, id, &child, Some(TIP + 1));
    let child_change = change_of(&child, &own_scripts(&engine, &session, id));
    start_spv(&engine, &session);
    let refused = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::Confirmed
            ))
        ),
        "{refused:?}"
    );
    assert!(
        held(&engine, &session, id).contains(&child_change),
        "the child's change is not held"
    );
    assert!(
        !abandoned(&engine, &session, id, payment.txid()),
        "the payment"
    );
}

/// A payment of A and B, InstantSend-locked, and its child spending its
/// change, abandoned; after a restart a grandchild spending the child's
/// change pays the wallet. Another device's spend of B is mined and
/// ChainLocked: abandoning the payment takes the grandchild too, through
/// the abandoned child, and the grandchild's output leaves, after a
/// restart too.
#[test]
fn abandoning_a_locked_payment_a_chainlock_beat_takes_a_grandchild_behind_an_abandoned_child() {
    let (_dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    abandon(&engine, &session, id, child.txid());
    let session = reopen(&engine, session);
    let left = change_of(&child, &own_scripts(&engine, &session, id));
    let owner = Address::from_script(
        &child.output[left.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let to = fresh_address(&engine, &session, id);
    let value = child.output[left.vout as usize].value - 1_000;
    let grandchild = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(left, vec![(to.script_pubkey(), value)]),
    );
    mempool(&engine, &session, id, &grandchild);
    wait_stored(&session, id, &[grandchild.txid()]);
    let paid_back = OutPoint::new(grandchild.txid(), 0);

    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    assert!(held(&engine, &session, id).contains(&paid_back));
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    let mut session = session;
    for state in ["the abandon", "a restart after it"] {
        if state != "the abandon" {
            session = reopen(&engine, session);
        }
        assert!(abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(
            abandoned(&engine, &session, id, grandchild.txid()),
            "{state}: the grandchild"
        );
        assert!(
            !held(&engine, &session, id).contains(&paid_back),
            "{state}: the grandchild's output is counted"
        );
    }
}

/// key-wallet's record of `tx` in the mempool, as dash-spv's wallet guard
/// leaves it before the history hears of it: no wallet event is sent.
fn credited_unheard(engine: &Engine, session: &NetworkSession, id: WalletId, tx: &Transaction) {
    use key_wallet::transaction_checking::{TransactionContext, WalletTransactionChecker};
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let manager = session.manager().unwrap();
    engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
        let mut wallet = wallet.clone();
        let info = &mut info.core_wallet;
        info.check_core_transaction(tx, TransactionContext::Mempool, &mut wallet, true, true)
            .await;
        info.update_balance();
    });
}

/// As above, but key-wallet has credited the grandchild and the history
/// has not heard of it (the window between dash-spv's wallet guard and the
/// event's apply) when the abandon runs. It takes the grandchild too: its
/// output is not counted.
#[test]
fn abandoning_a_locked_payment_a_chainlock_beat_takes_a_grandchild_the_history_has_not_heard() {
    let (_dir, engine, session, id, _paid, payment, grandchild) =
        beaten_with_an_undelivered_grandchild();
    let paid_back = OutPoint::new(grandchild.txid(), 0);
    credited_unheard(&engine, &session, id, &grandchild);
    assert!(held(&engine, &session, id).contains(&paid_back));
    assert!((session.hub.history).get(&id, &grandchild.txid()).is_none());
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    match result {
        Ok(()) => assert!(
            !held(&engine, &session, id).contains(&paid_back),
            "the abandon reported done and the grandchild's output is counted"
        ),
        Err(crate::EngineError::TxActionRefused(_)) => {}
        Err(e) => panic!("{e:?}"),
    }
}

/// The beaten payment and its abandoned child as above, and the child's
/// change paid on to the wallet by `grandchild`; returns them with the
/// session after the beat, `grandchild` not yet delivered.
#[allow(clippy::type_complexity)]
fn beaten_with_an_undelivered_grandchild() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid, payment, child) = payment_and_child(false);
    abandon(&engine, &session, id, child.txid());
    let session = reopen(&engine, session);
    let left = change_of(&child, &own_scripts(&engine, &session, id));
    let owner = Address::from_script(
        &child.output[left.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let to = fresh_address(&engine, &session, id);
    let value = child.output[left.vout as usize].value - 1_000;
    let grandchild = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(left, vec![(to.script_pubkey(), value)]),
    );
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    (dir, engine, session, id, paid, payment, grandchild)
}

/// The abandon holds the wallet manager from its plan until its sweep is
/// stored, so key-wallet cannot credit a grandchild in between: one that
/// arrives meanwhile is credited after, shows in Activity, and Abandon
/// takes it.
#[test]
fn a_grandchild_credited_between_the_plan_and_the_sweep_is_never_left_credited() {
    let (_dir, engine, session, id, _paid, payment, grandchild) =
        beaten_with_an_undelivered_grandchild();
    let paid_back = OutPoint::new(grandchild.txid(), 0);
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_sweep.lock().unwrap() = Some(Arc::clone(&pause));
    let abandoning = {
        let s = Arc::clone(&session);
        let txid = payment.txid();
        session
            .rt
            .spawn(async move { s.abandon_transaction(id, txid).await })
    };
    eventually("the abandon's plan, before its sweep", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let manager_held = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        wm.try_read().is_err()
    });
    pause.release.notify_one();
    let result = engine.block_on(abandoning).unwrap();
    assert!(
        manager_held,
        "the abandon let go of the wallet manager between its plan and its sweep"
    );
    match result {
        Ok(()) | Err(crate::EngineError::TxActionRefused(_)) => {}
        Err(e) => panic!("{e:?}"),
    }
    mempool(&engine, &session, id, &grandchild);
    eventually("the grandchild's event lands", || {
        (session.hub.history).get(&id, &grandchild.txid()).is_some()
    });
    if held(&engine, &session, id).contains(&paid_back) {
        abandon(&engine, &session, id, grandchild.txid());
        assert!(!held(&engine, &session, id).contains(&paid_back));
    }
}

/// A transaction key-wallet holds that spends a coin outside the beaten
/// payment's tree is not taken by its abandon.
#[test]
fn abandoning_a_locked_payment_a_chainlock_beat_leaves_an_unrelated_unheard_spend() {
    let (_dir, engine, session, id, paid, payment, _grandchild) =
        beaten_with_an_undelivered_grandchild();
    let d = OutPoint::new(paid.txid(), 5);
    let owner =
        Address::from_script(&paid.output[5].script_pubkey, dashcore::Network::Regtest).unwrap();
    let to = fresh_address(&engine, &session, id);
    let other = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(d, vec![(to.script_pubkey(), COIN - 1_000)]),
    );
    unheard(&engine, &session, id, &other, None);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert!(
        held(&engine, &session, id).contains(&OutPoint::new(other.txid(), 0)),
        "the unrelated spend's output stays"
    );
}

/// What wallet.sqlite holds for `outpoint` of `id`: whether it is spent,
/// and whether it is a sweep's placeholder claim. The store keys a coin by
/// its outpoint in bincode varint: the txid's length, the txid, then the
/// vout (one byte below 251).
fn stored_row(session: &NetworkSession, id: WalletId, outpoint: OutPoint) -> Option<(bool, bool)> {
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
        "SELECT spent, is_sweep_placeholder FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![&id.0[..], key],
        |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?)),
    )
    .optional()
    .unwrap()
}

/// An unlocked payment, its child abandoned, and a grandchild that pays the
/// child's change on to the wallet. Abandoning the payment takes the
/// grandchild through the abandoned child and leaves no claim on the
/// child's change in the store: once the payment and the child are mined
/// without the grandchild, the change is held exactly once, counted and
/// offered, after a restart too.
#[test]
fn abandoning_through_an_abandoned_child_leaves_its_change_unclaimed() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &payment);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 100,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    abandon(&engine, &session, id, child.txid());
    let session = reopen(&engine, session);
    let left = change_of(&child, &own_scripts(&engine, &session, id));
    let owner = Address::from_script(
        &child.output[left.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let to = fresh_address(&engine, &session, id);
    let value = child.output[left.vout as usize].value - 1_000;
    let grandchild = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(left, vec![(to.script_pubkey(), value)]),
    );
    mempool(&engine, &session, id, &grandchild);
    wait_stored(&session, id, &[grandchild.txid()]);
    let session = reopen(&engine, session);

    abandon(&engine, &session, id, payment.txid());
    assert!(abandoned(&engine, &session, id, grandchild.txid()));
    let row = stored_row(&session, id, left);

    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![payment, child]);
    let mut session = session;
    for state in ["mined", "a restart after"] {
        if state != "mined" {
            session = reopen(&engine, session);
        }
        let held = coins(&engine, &session, id);
        assert_eq!(
            held.iter().filter(|(o, _)| *o == left).count(),
            1,
            "{state}: the child's change is not held exactly once: {held:?}"
        );
        assert_eq!(
            balances(&session, id).total,
            held.iter().map(|(_, v)| v).sum::<u64>(),
            "{state}: the balance does not count the coins held"
        );
        assert!(
            spendable(&engine, &session, id, left),
            "{state}: the child's change is not offered"
        );
    }
    assert!(
        row.is_none_or(|(spent, _)| !spent),
        "the abandon claimed the child's change in the store: {row:?}"
    );
}

/// A payment of this wallet that another device made, seen by its change
/// before the wallet knows the coins it spent: its debit is unknown, so the
/// Abandon confirmation warns of paying twice. A payment someone else
/// funded to a receiving address keeps a known debit of nothing, with its
/// inputs unresolved: it may be this wallet's, sent from another device.
#[test]
fn a_payment_seen_by_its_change_before_its_inputs_has_an_unknown_debit() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let change: Address = engine.block_on(async {
        let wallet = session.wallet(&id).await.unwrap();
        wallet
            .core()
            .next_change_address_for_account(0)
            .await
            .unwrap()
    });
    let mine = Transaction {
        output: vec![
            TxOut {
                value: COIN,
                script_pubkey: elsewhere(),
            },
            TxOut {
                value: COIN / 2,
                script_pubkey: change.script_pubkey(),
            },
        ],
        ..funding(9, &[], 0)
    };
    mempool(&engine, &session, id, &mine);
    let theirs = funding(10, &[fresh_address(&engine, &session, id)], COIN);
    mempool(&engine, &session, id, &theirs);
    let extras = |txid: Txid| {
        let x = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
        (x.total_debit, x.inputs_unresolved)
    };
    let debit = |txid: Txid| extras(txid).0;
    assert_eq!(debit(mine.txid()), None, "a spend whose inputs are unknown");
    assert_eq!(
        extras(theirs.txid()),
        (Some(0), true),
        "an incoming payment"
    );

    // A payment this app sent to one of its own receiving addresses, with no
    // change, its coins not known yet: its sent mark makes the debit unknown.
    let to_self = Transaction {
        output: vec![TxOut {
            value: COIN,
            script_pubkey: fresh_address(&engine, &session, id).script_pubkey(),
        }],
        ..funding(11, &[], 0)
    };
    mempool(&engine, &session, id, &to_self);
    assert_eq!(
        extras(to_self.txid()),
        (Some(0), true),
        "custom change to a receiving address, no sent mark yet"
    );
    let (scope, key, coin) = (
        id.to_string(),
        format!("{}{}", crate::tx_actions::SENT_PREFIX, to_self.txid()),
        to_self.input[0].previous_output.to_string(),
    );
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    appdb.set_setting(&scope, &key, Some(&coin)).unwrap();
    assert_eq!(debit(to_self.txid()), None, "a self-transfer this app sent");

    // A sent mark that keeps only its copy of the signed transaction.
    let copy_only = Transaction {
        output: vec![TxOut {
            value: COIN,
            script_pubkey: fresh_address(&engine, &session, id).script_pubkey(),
        }],
        ..funding(12, &[], 0)
    };
    mempool(&engine, &session, id, &copy_only);
    assert_eq!(debit(copy_only.txid()), Some(0), "no sent mark yet");
    let key = format!("{}{}", crate::tx_actions::SENT_TX_PREFIX, copy_only.txid());
    appdb.set_setting(&scope, &key, Some("00")).unwrap();
    assert_eq!(
        debit(copy_only.txid()),
        None,
        "a payment whose sent mark keeps only its copy"
    );
}

/// How long an abandon holds the wallet manager, through its sweep's
/// store, in a wallet with `DWD_ABANDON_HOLD_TXS` stored transactions
/// (default 5000). Opt in: `DWD_ABANDON_HOLD=1`.
#[test]
#[ignore = "measurement: DWD_ABANDON_HOLD=1"]
fn abandon_hold_on_a_large_wallet() {
    if std::env::var("DWD_ABANDON_HOLD").is_err() {
        return;
    }
    let count: usize = std::env::var("DWD_ABANDON_HOLD_TXS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let to = fresh_address(&engine, &session, id);
    let many: Vec<Transaction> = (0..count)
        .map(|i| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0xEE; 32]), i as u32),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: 10_000,
                script_pubkey: to.script_pubkey(),
            }],
            ..funding(12, &[], 0)
        })
        .collect();
    mined(&engine, &session, id, TIP - 1, many);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(paid.txid(), 0)]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    start_spv(&engine, &session);
    let started = std::time::Instant::now();
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    let held = session.hub.last_abandon_hold.lock().unwrap().unwrap();
    eprintln!(
        "ABANDON_HOLD txs={} held={held:?} abandon={:?} load={}",
        count + 13,
        started.elapsed(),
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );
}

/// key-wallet credits `tx` from the mempool and its record is stored, and
/// the history has not heard of it; returns the events that tell it, for
/// the caller to hand over.
fn credited_with_late_events(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    tx: &Transaction,
) -> Vec<platform_wallet::key_wallet_manager::WalletEvent> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    use platform_wallet::key_wallet_manager::WalletInterface;
    let manager = session.manager().unwrap();
    let tx = tx.clone();
    let events = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut rx = wm.subscribe_events();
        assert!(wm.process_mempool_transaction(&tx, None).await.is_relevant);
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    });
    session.live().unwrap().persister.flush(id.0).unwrap();
    events
}

/// An unlocked payment, its child abandoned, and after a restart a
/// grandchild spending the child's change and another coin U of the
/// wallet, which key-wallet has credited and stored and the history has
/// not heard of when the payment is abandoned. The abandon goes through
/// and takes the grandchild: Abandoned, its output not counted, U
/// released in the store. Its late event changes none of that, nor does a
/// restart, after which U is offered once its funding is mined.
#[test]
fn abandoning_an_unlocked_payment_takes_a_grandchild_the_history_has_not_heard() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b, u) = (
        OutPoint::new(paid.txid(), 0),
        OutPoint::new(paid.txid(), 1),
        OutPoint::new(paid.txid(), 5),
    );
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &payment);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 100,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    abandon(&engine, &session, id, child.txid());
    let session = reopen(&engine, session);
    let left = change_of(&child, &own_scripts(&engine, &session, id));
    let owner = |o: OutPoint, tx: &Transaction| {
        Address::from_script(
            &tx.output[o.vout as usize].script_pubkey,
            dashcore::Network::Regtest,
        )
        .unwrap()
    };
    let to = fresh_address(&engine, &session, id);
    let mut spend = super::spend_of(left, vec![(to.script_pubkey(), COIN)]);
    spend.input.push(TxIn {
        previous_output: u,
        ..Default::default()
    });
    let grandchild = super::signed_by_wallet_each(
        &engine,
        &session,
        id,
        &[owner(left, &child), owner(u, &paid)],
        spend,
    );
    let paid_back = OutPoint::new(grandchild.txid(), 0);
    start_spv(&engine, &session);
    let late = credited_with_late_events(&engine, &session, id, &grandchild);
    wait_stored(&session, id, &[grandchild.txid()]);
    assert!((session.hub.history).get(&id, &grandchild.txid()).is_none());
    assert!(held(&engine, &session, id).contains(&paid_back));

    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    for event in &late {
        session.hub.apply_wallet_event(event);
    }
    // The late event's balance is from before the sweep: the shown balance
    // is the manager's, without the grandchild's output.
    eventually("the shown balance is the manager's", || {
        let held_sum: u64 = coins(&engine, &session, id).iter().map(|(_, v)| v).sum();
        balances(&session, id).total == held_sum
    });
    let mut session = session;
    for state in ["the abandon and the late event", "a restart after"] {
        if state != "the abandon and the late event" {
            session = reopen(&engine, session);
        }
        assert!(abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(
            abandoned(&engine, &session, id, grandchild.txid()),
            "{state}: the grandchild"
        );
        assert!(
            !held(&engine, &session, id).contains(&paid_back),
            "{state}: the grandchild's output is counted"
        );
        let row = stored_row(&session, id, u);
        assert!(
            row.is_some_and(|(spent, _)| !spent),
            "{state}: U is not released in the store: {row:?}"
        );
    }
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(spendable(&engine, &session, id, u), "U is not offered");
}

/// The key of a DashPay contact's account paid by this wallet (a test key).
fn contact_key() -> key_wallet::bip32::ExtendedPrivKey {
    key_wallet::bip32::ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[101; 32]).unwrap()
}

/// The contact's address at `index`, of the account [`contact_key`] holds.
fn contact_address(index: u32) -> Address {
    let xpub = key_wallet::bip32::ExtendedPubKey::from_priv(&contact_key());
    let child = xpub
        .derive_pub(&[key_wallet::bip32::ChildNumber::Normal { index }])
        .unwrap();
    Address::p2pkh(
        &dashcore::PublicKey::new(child.public_key),
        dashcore::Network::Regtest,
    )
}

/// Adds to wallet `id` its account for the contact of [`contact_key`], as
/// an accepted contact request leaves it.
fn add_contact_account(engine: &Engine, session: &NetworkSession, id: WalletId) {
    use key_wallet::account::AccountType;
    use key_wallet::managed_account::ManagedCoreFundsAccount;
    let xpub = key_wallet::bip32::ExtendedPubKey::from_priv(&contact_key());
    let account_type = AccountType::DashpayExternalAccount {
        index: 0,
        user_identity_id: [1; 32],
        friend_identity_id: [2; 32],
    };
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (w, info) = wm.get_wallet_mut_and_info_mut(&id.0).unwrap();
        w.add_account(account_type, Some(xpub)).unwrap();
        let managed = ManagedCoreFundsAccount::from_account(&key_wallet::Account {
            parent_wallet_id: Some(id.0),
            account_type,
            network: dashcore::Network::Regtest,
            account_xpub: xpub,
            is_watch_only: true,
        });
        (info.core_wallet.accounts)
            .insert_funds_bearing_account(managed)
            .unwrap();
    });
}

/// `tx` with input `input` signed by the contact's key for their address
/// at `index`, the address the coin it spends pays.
fn signed_by_contact(mut tx: Transaction, input: usize, index: u32) -> Transaction {
    let sighash = dashcore::sighash::SighashCache::new(&tx)
        .legacy_signature_hash(
            input,
            &contact_address(index).script_pubkey(),
            dashcore::sighash::EcdsaSighashType::All.to_u32(),
        )
        .unwrap();
    let key = (contact_key().derive_priv(&[key_wallet::bip32::ChildNumber::Normal { index }]))
        .unwrap()
        .private_key;
    let sig = key.sign_ecdsa(dashcore::secp256k1::Message::from_digest(
        *sighash.as_byte_array(),
    ));
    tx.input[input].script_sig = dashcore::blockdata::script::Builder::new()
        .push_slice(dashcore::ecdsa::Signature::sighash_all(sig).serialize())
        .push_key(&dashcore::PublicKey::new(
            dashcore::secp256k1::PublicKey::from_secret_key(&key),
        ))
        .into_script();
    tx
}

/// Whether key-wallet holds `txid` in a contact's account, or in one of
/// the wallet's own.
fn held_in(engine: &Engine, session: &NetworkSession, id: WalletId, txid: Txid) -> (bool, bool) {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        let (contacts, own): (Vec<_>, Vec<_>) = (info.core_wallet.accounts.all_accounts())
            .into_iter()
            .partition(|a| a.managed_account_type().is_contact_owned());
        let holds = |accounts: &[_]| {
            accounts
                .iter()
                .any(|a: &key_wallet::managed_account::ManagedAccountRef<'_>| {
                    a.transactions().contains_key(&txid)
                })
        };
        (holds(&contacts), holds(&own))
    })
}

/// The wallet's payment of coin 0 of `paid` to the contact's address 0,
/// in the mempool and stored; and the contact's spend of it to their
/// address 1, signed, which key-wallet holds in the contact's account
/// only.
fn paid_to_a_contact_who_spends_it(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
) -> (Transaction, Transaction) {
    add_contact_account(engine, session, id);
    let contact = contact_address(0);
    let payment = send_to(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(paid.txid(), 0)]),
        &contact.to_string(),
        COIN / 2,
        false,
    );
    mempool(engine, session, id, &payment);
    wait_stored(session, id, &[payment.txid()]);
    let to_contact = (payment.output.iter())
        .position(|o| o.script_pubkey == contact.script_pubkey())
        .unwrap();
    let theirs = signed_by_contact(
        super::spend_of(
            OutPoint::new(payment.txid(), to_contact as u32),
            vec![(contact_address(1).script_pubkey(), COIN / 2 - 1_000)],
        ),
        0,
        0,
    );
    credited_unheard(engine, session, id, &theirs);
    assert_eq!(
        held_in(engine, session, id, theirs.txid()),
        (true, false),
        "key-wallet holds the contact's spend in their account only"
    );
    (payment, theirs)
}

/// A payment to a contact, and the contact's spend of it to another of
/// their addresses, which key-wallet holds in the wallet's account for
/// that contact. Abandoning the payment writes no abandoned mark for the
/// contact's spend and leaves it out of Activity, after a restart too.
/// (key-wallet and the store's sweep still drop it with the payment.)
#[test]
fn abandoning_a_payment_to_a_contact_marks_nothing_of_the_contacts_spend() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (payment, theirs) = paid_to_a_contact_who_spends_it(&engine, &session, id, &paid);
    start_spv(&engine, &session);

    abandon(&engine, &session, id, payment.txid());
    let mark = |session: &Arc<NetworkSession>| {
        let (scope, key) = (id.to_string(), format!("abandoned:{}", theirs.txid()));
        (engine.block_on(session.appdb_op(move |db| db.setting(&scope, &key)))).unwrap()
    };
    assert_eq!(
        mark(&session),
        None,
        "the contact's spend is marked abandoned"
    );
    assert!((session.hub.history).get(&id, &theirs.txid()).is_none());
    let session = reopen(&engine, session);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    assert_eq!(mark(&session), None);
    assert!(
        (session.hub.history).get(&id, &theirs.txid()).is_none(),
        "the contact's spend is in Activity after a restart"
    );
}

/// The shape above, and a spend E of the contact's output and the
/// wallet's coin U (signed by both), which key-wallet credits and stores
/// in the wallet's own account and the history has not heard of.
/// key-wallet's walk reaches E only through the contact's spend, so the
/// abandon of the payment takes E: E is Abandoned and U is released in
/// the store at once and after a restart, then offered once its funding
/// is mined.
#[test]
fn abandoning_a_payment_to_a_contact_releases_a_coin_of_a_spend_behind_the_contacts() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (payment, theirs) = paid_to_a_contact_who_spends_it(&engine, &session, id, &paid);
    let u = OutPoint::new(paid.txid(), 5);
    let to = fresh_address(&engine, &session, id);
    let mut spend = super::spend_of(u, vec![(to.script_pubkey(), COIN)]);
    spend.input.push(TxIn {
        previous_output: OutPoint::new(theirs.txid(), 0),
        ..Default::default()
    });
    let owner = Address::from_script(
        &paid.output[u.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let mut both = super::signed_by_wallet(&engine, &session, id, &owner, spend);
    both = signed_by_contact(both, 1, 1);
    let late = credited_with_late_events(&engine, &session, id, &both);
    wait_stored(&session, id, &[both.txid()]);
    assert!(held_in(&engine, &session, id, both.txid()).1);
    assert!((session.hub.history).get(&id, &both.txid()).is_none());
    assert!((session.hub.history).get(&id, &theirs.txid()).is_none());
    start_spv(&engine, &session);

    abandon(&engine, &session, id, payment.txid());
    for event in &late {
        session.hub.apply_wallet_event(event);
    }
    let mut session = session;
    for state in ["the abandon", "a restart after"] {
        if state != "the abandon" {
            session = reopen(&engine, session);
        }
        let row = stored_row(&session, id, u);
        assert!(
            row.is_some_and(|(spent, _)| !spent),
            "{state}: U is not released in the store: {row:?}"
        );
        assert!(
            abandoned(&engine, &session, id, both.txid()),
            "{state}: the spend behind the contact's"
        );
    }
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(spendable(&engine, &session, id, u), "U is not offered");
}

/// The history learns that `txid` is InstantSend-locked; key-wallet does
/// not (its record stays pending), as when the history holds a lock it
/// rehydrated.
fn locked_in_history_only(engine: &Engine, session: &NetworkSession, id: WalletId, txid: Txid) {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let manager = session.manager().unwrap();
    let (balance, account_balances) = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        (info.balance(), info.account_balances())
    });
    (session.hub).apply_wallet_event(
        &platform_wallet::key_wallet_manager::WalletEvent::TransactionInstantLocked {
            wallet_id: id.0,
            txid,
            instant_lock: InstantLock {
                txid,
                ..InstantLock::default()
            },
            balance,
            account_balances,
        },
    );
    assert!(session.hub.history.get(&id, &txid).unwrap().instant_locked);
}

/// The history learns that `txid` is in a block at `height`; key-wallet
/// does not (its record stays pending).
fn mined_in_history_only(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    txid: Txid,
    height: u32,
) {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let manager = session.manager().unwrap();
    let (mut record, balance, account_balances) = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let record = (info.accounts.all_accounts().into_iter())
            .filter(|a| !a.managed_account_type().is_contact_owned())
            .find_map(|a| a.transactions().get(&txid).cloned())
            .expect("key-wallet records the transaction");
        (record, info.balance(), info.account_balances())
    });
    record.context =
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 0));
    (session.hub).apply_wallet_event(
        &platform_wallet::key_wallet_manager::WalletEvent::TransactionDetected {
            wallet_id: id.0,
            record: Box::new(record),
            balance,
            account_balances,
            addresses_derived: Vec::new(),
        },
    );
    let entry = session.hub.history.get(&id, &txid).unwrap();
    assert!(entry.context.block_info().is_some());
}

/// An unlocked payment, its child T, which `settle` makes the history hold
/// as settled while key-wallet's record of it stays pending, and a spend E
/// of T's change and the wallet's coin U that key-wallet credits and
/// stores and the history has not heard of. key-wallet's walk would take T
/// and E; the abandon refuses as `refusal`, releases nothing and marks
/// nothing, after a restart too.
fn abandon_under_a_child_settled_in_the_history_only(
    settle: impl FnOnce(&Engine, &NetworkSession, WalletId, Txid),
    refusal: crate::tx_actions::TxActionRefusal,
) {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let u = OutPoint::new(paid.txid(), 5);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(paid.txid(), 0)]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change]),
        COIN / 100,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    settle(&engine, &session, id, child.txid());
    let left = change_of(&child, &own_scripts(&engine, &session, id));
    let owner = |o: OutPoint, tx: &Transaction| {
        Address::from_script(
            &tx.output[o.vout as usize].script_pubkey,
            dashcore::Network::Regtest,
        )
        .unwrap()
    };
    let to = fresh_address(&engine, &session, id);
    let mut spend = super::spend_of(left, vec![(to.script_pubkey(), COIN)]);
    spend.input.push(TxIn {
        previous_output: u,
        ..Default::default()
    });
    let e = super::signed_by_wallet_each(
        &engine,
        &session,
        id,
        &[owner(left, &child), owner(u, &paid)],
        spend,
    );
    let _late = credited_with_late_events(&engine, &session, id, &e);
    wait_stored(&session, id, &[e.txid()]);
    assert!((session.hub.history).get(&id, &e.txid()).is_none());
    start_spv(&engine, &session);

    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(&result, Err(crate::EngineError::TxActionRefused(r)) if *r == refusal),
        "{result:?}"
    );
    let mark = |session: &Arc<NetworkSession>| {
        let (scope, key) = (id.to_string(), format!("abandoned:{}", e.txid()));
        (engine.block_on(session.appdb_op(move |db| db.setting(&scope, &key)))).unwrap()
    };
    let mut session = session;
    for state in ["the abandon", "a restart after"] {
        if state != "the abandon" {
            session = reopen(&engine, session);
        }
        let row = stored_row(&session, id, u);
        assert!(
            row.is_some_and(|(spent, _)| spent),
            "{state}: U is released in the store: {row:?}"
        );
        assert!(!abandoned(&engine, &session, id, payment.txid()), "{state}");
        assert!(!abandoned(&engine, &session, id, child.txid()), "{state}");
        assert_eq!(mark(&session), None, "{state}: E is marked abandoned");
    }
}

#[test]
fn abandoning_a_payment_under_a_child_locked_in_the_history_only_is_refused() {
    abandon_under_a_child_settled_in_the_history_only(
        locked_in_history_only,
        crate::tx_actions::TxActionRefusal::InstantLocked,
    );
}

/// A child the history holds both locked and mined: `Confirmed` wins.
#[test]
fn abandoning_a_payment_under_a_child_locked_and_mined_in_the_history_only_is_refused_as_confirmed()
{
    abandon_under_a_child_settled_in_the_history_only(
        |engine, session, id, txid| {
            locked_in_history_only(engine, session, id, txid);
            mined_in_history_only(engine, session, id, txid, TIP);
        },
        crate::tx_actions::TxActionRefusal::Confirmed,
    );
}

#[test]
fn abandoning_a_payment_under_a_child_mined_in_the_history_only_is_refused_as_confirmed() {
    abandon_under_a_child_settled_in_the_history_only(
        |engine, session, id, txid| mined_in_history_only(engine, session, id, txid, TIP),
        crate::tx_actions::TxActionRefusal::Confirmed,
    );
}

/// Two pending payments of the wallet's coin U, the wallet's own and one
/// it sees with change to it (key-wallet records both), U's funding mined.
fn two_spends_of_a_coin() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    OutPoint,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let u = OutPoint::new(paid.txid(), 0);
    let first = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![u]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &first);
    let second = second_spend_of(&engine, &session, id, &paid, u);
    mempool(&engine, &session, id, &second);
    wait_stored(&session, id, &[first.txid(), second.txid()]);
    (dir, engine, session, id, paid, u, first, second)
}

/// A spend of `u`, an output of `paid`, signed by the wallet, with change
/// to it.
fn second_spend_of(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    u: OutPoint,
) -> Transaction {
    let owner = Address::from_script(
        &paid.output[u.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    super::signed_by_wallet(
        engine,
        session,
        id,
        &owner,
        super::spend_of(
            u,
            vec![
                (fresh_address(engine, session, id).script_pubkey(), COIN / 4),
                (elsewhere(), COIN * 3 / 4 - 2_000),
            ],
        ),
    )
}

/// Waits until the store has applied every wallet event sent before: the
/// persistence adapter applies them in order, so once a marker payment to
/// the wallet is stored, everything before it is.
fn store_settled(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) {
    static SEED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(200);
    let seed = SEED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let marker = funding(seed, &[fresh_address(engine, session, id)], 1_000);
    mempool(engine, session, id, &marker);
    wait_stored(session, id, &[marker.txid()]);
}

/// The scan an abandon asked for, if one is pending: the funding block of
/// `paid` again.
fn scan_if_pending(engine: &Engine, session: &NetworkSession, id: WalletId, paid: &Transaction) {
    let replay = Arc::clone(&session.live().unwrap().replay);
    if engine.block_on(replay.get(id)).unwrap().is_some() {
        mined(engine, session, id, TIP, vec![paid.clone()]);
    }
}

/// Asserts that U is stored spent, not counted and not offered (`held`), or
/// stored unspent: once any scan the app asked for has run and the store
/// has applied it, then after a restart the same. A released U is counted
/// and offered from the restart on (as any abandoned payment's coin is).
fn coin_state(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    u: OutPoint,
    held: bool,
    state: &str,
) -> Arc<NetworkSession> {
    let mut session = session;
    for after in ["", " and a restart"] {
        let mut events = events_from(engine, &session);
        if !after.is_empty() {
            session = reopen(engine, session);
            events = events_from(engine, &session);
            start_spv(engine, &session);
        }
        scan_if_pending(engine, &session, id, paid);
        store_settled(engine, &session, id);
        let mut story = || {
            let spenders: Vec<Txid> = (session.hub.history.snapshot(&id).into_iter())
                .filter(|(_, e)| e.tx.input.iter().any(|i| i.previous_output == u))
                .map(|(t, _)| t)
                .collect();
            let replay = Arc::clone(&session.live().unwrap().replay);
            let record = engine.block_on(replay.get(id));
            format!(
                "\nrewind record: {record:?}{}",
                wallet_story(engine, &session, id, &spenders, Some(&mut events))
            )
        };
        let row = stored_row(&session, id, u);
        assert_eq!(
            row.map(|(spent, _)| spent),
            Some(held),
            "{state}{after}: U stored spent: {row:?}{}",
            story()
        );
        if held || !after.is_empty() {
            assert_eq!(
                coins(engine, &session, id).iter().any(|(o, _)| *o == u),
                !held,
                "{state}{after}: U counted{}",
                story()
            );
            assert_eq!(
                spendable(engine, &session, id, u),
                !held,
                "{state}{after}: U offered{}",
                story()
            );
        }
    }
    session
}

/// Two pending spends of the wallet's coin U: abandoning one leaves U spent
/// by the other, in the store and in what is offered, at once and after a
/// restart. Abandoning the other too, after that restart, releases U.
#[test]
fn abandoning_one_of_two_spends_of_a_coin_keeps_it_until_the_other_goes() {
    let (_dir, engine, session, id, paid, u, first, second) = two_spends_of_a_coin();
    start_spv(&engine, &session);
    abandon(&engine, &session, id, first.txid());
    let session = coin_state(&engine, session, id, &paid, u, true, "one abandoned");
    abandon(&engine, &session, id, second.txid());
    coin_state(&engine, session, id, &paid, u, false, "both abandoned");
}

/// As above, both abandoned in one session, the other spend first: that
/// abandon releases nothing and asks for no scan; the wallet's own payment's
/// then releases U and asks for the scan of U's funding block. U is stored
/// unspent at once, and counted and offered after a restart.
#[test]
fn abandoning_both_spends_of_a_coin_in_one_session_releases_it() {
    let (_dir, engine, session, id, paid, u, first, second) = two_spends_of_a_coin();
    start_spv(&engine, &session);
    let replay = Arc::clone(&session.live().unwrap().replay);
    let record = || engine.block_on(replay.get(id)).unwrap();
    abandon(&engine, &session, id, second.txid());
    assert!(
        record().is_none(),
        "the abandon that released nothing asked for a scan"
    );
    abandon(&engine, &session, id, first.txid());
    assert!(
        record().is_some(),
        "the abandon that released U asked for no scan"
    );
    coin_state(&engine, session, id, &paid, u, false, "both abandoned");
}

/// An abandon of one of two spends of U releases nothing, so it asks for no
/// scan: a restart before any scan ran resumes none, and U stays stored
/// spent, not counted and not offered (after a restart key-wallet holds no
/// record of the other spend, so a scan would credit U).
#[test]
fn an_abandon_that_releases_nothing_leaves_no_scan_for_a_restart() {
    let (_dir, engine, session, id, paid, u, first, _second) = two_spends_of_a_coin();
    start_spv(&engine, &session);
    abandon(&engine, &session, id, first.txid());
    let replay = Arc::clone(&session.live().unwrap().replay);
    let record = engine.block_on(replay.get(id)).unwrap();
    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    scan_if_pending(&engine, &session, id, &paid);
    store_settled(&engine, &session, id);
    let row = stored_row(&session, id, u);
    assert_eq!(
        row.map(|(spent, _)| spent),
        Some(true),
        "U stored spent: {row:?}"
    );
    assert!(
        !coins(&engine, &session, id).iter().any(|(o, _)| *o == u),
        "U is counted"
    );
    assert!(record.is_none(), "the abandon asked for a scan: {record:?}");
}

/// As above, with a restart before the first abandon: key-wallet then holds
/// no record of the other spend, and the history's keeps U spent.
#[test]
fn abandoning_one_of_two_spends_of_a_coin_after_a_restart_keeps_it_until_the_other_goes() {
    let (_dir, engine, session, id, paid, u, first, second) = two_spends_of_a_coin();
    let session = reopen(&engine, session);
    assert_eq!(
        held_in(&engine, &session, id, second.txid()),
        (false, false)
    );
    start_spv(&engine, &session);
    abandon(&engine, &session, id, first.txid());
    let session = coin_state(&engine, session, id, &paid, u, true, "one abandoned");
    abandon(&engine, &session, id, second.txid());
    coin_state(&engine, session, id, &paid, u, false, "both abandoned");
}

/// The wallet's pending payment of U, and another spend of U key-wallet
/// credited and neither the history nor the store got (a lost flush). The
/// abandon of the payment keeps U stored spent and asks for no rescan (the
/// other spend still spends U). The lost flush ends at the restart after
/// the abandon: nothing spends U then, but it stays stored spent, not
/// counted and not offered, until Repair ▸ Rescan from the wallet's birth;
/// then it is stored unspent, held once and offered, and after another
/// restart too.
#[test]
fn a_coin_kept_for_a_spend_the_store_never_got_comes_back_with_a_rescan() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let u = OutPoint::new(paid.txid(), 0);
    let first = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![u]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &first);
    wait_stored(&session, id, &[first.txid()]);
    let lost = second_spend_of(&engine, &session, id, &paid, u);
    credited_unheard(&engine, &session, id, &lost);
    assert!(held_in(&engine, &session, id, lost.txid()).1);
    start_spv(&engine, &session);
    abandon(&engine, &session, id, first.txid());
    let row = stored_row(&session, id, u);
    assert!(row.is_some_and(|(spent, _)| spent), "{row:?}");

    let mut session = reopen(&engine, session);
    for state in ["a restart", "a rescan from the birth", "a restart after"] {
        match state {
            "a rescan from the birth" => {
                engine
                    .block_on(session.rescan(crate::RescanFrom::WalletBirth))
                    .unwrap();
                mined(&engine, &session, id, TIP, vec![paid.clone()]);
            }
            _ => {
                if state == "a restart after" {
                    session = reopen(&engine, session);
                }
                start_spv(&engine, &session);
                scan_if_pending(&engine, &session, id, &paid);
            }
        }
        store_settled(&engine, &session, id);
        let back = state != "a restart";
        let row = stored_row(&session, id, u);
        assert_eq!(
            row.map(|(spent, _)| spent),
            Some(!back),
            "{state}: U stored spent: {row:?}"
        );
        let coins_held = coins(&engine, &session, id);
        assert_eq!(
            coins_held.iter().filter(|(o, _)| *o == u).count(),
            usize::from(back),
            "{state}: U held: {coins_held:?}"
        );
        assert_eq!(
            spendable(&engine, &session, id, u),
            back,
            "{state}: U offered"
        );
    }
}

/// A payment P of A, abandoned and then relayed again with its InstantSend
/// lock (the history keeps its abandoned flag), and after it another
/// pending spend Q of A the wallet sees, then a reload (key-wallet then
/// holds neither). Abandoning Q leaves A stored spent: P's lock spends it.
/// (A Q made before P's lock is swept by it.)
#[test]
fn abandoning_a_spend_of_a_coin_an_abandoned_payment_locked_later_spends_keeps_it() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
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
    let reload = || {
        engine.block_on(session.unload_wallet(id)).unwrap();
        engine.block_on(session.load_wallet(id)).unwrap();
    };
    reload();
    instant_lock(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let q = second_spend_of(&engine, &session, id, &paid, a);
    mempool(&engine, &session, id, &q);
    wait_stored(&session, id, &[q.txid()]);
    reload();
    let entry =
        |t: Txid| (session.hub.history.get(&id, &t)).map(|e| (e.abandoned, e.instant_locked));
    assert_eq!(
        entry(p.txid()),
        Some((true, true)),
        "P is flagged abandoned and locked"
    );
    assert_eq!(entry(q.txid()), Some((false, false)), "Q is pending");
    start_spv(&engine, &session);

    abandon(&engine, &session, id, q.txid());
    let row = stored_row(&session, id, a);
    assert!(
        row.is_some_and(|(spent, _)| spent),
        "A is released although P's lock spends it: {row:?}"
    );
}

/// A payment of the wallet's coins A and U, and another device's pending
/// spend of U, both pending across a restart. Abandoning the payment
/// releases A and rescans from their funding block; the replay of that
/// block credits U too, which the other device's spend still holds: U is
/// counted in the balance (#378), but never offered, and A is held and
/// offered.
#[test]
fn abandons_rescan_counts_but_never_offers_a_coin_a_pending_payment_from_elsewhere_spends() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let (u, a) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, u]),
        COIN * 3 / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let owner =
        Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest).unwrap();
    let spend = super::spend_of(
        u,
        vec![
            (
                fresh_address(&engine, &session, id).script_pubkey(),
                COIN / 4,
            ),
            (elsewhere(), COIN * 3 / 4 - 2_000),
        ],
    );
    let other = super::signed_by_wallet(&engine, &session, id, &owner, spend);
    mempool(&engine, &session, id, &other);
    wait_stored(&session, id, &[other.txid()]);

    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    abandon(&engine, &session, id, payment.txid());
    scan_if_pending(&engine, &session, id, &paid);
    store_settled(&engine, &session, id);
    let counted =
        |s: &Arc<NetworkSession>, o: OutPoint| coins(&engine, s, id).iter().any(|(c, _)| *c == o);
    let mut session = session;
    for state in ["the abandon's rescan", "a restart after it"] {
        if state != "the abandon's rescan" {
            session = reopen(&engine, session);
            start_spv(&engine, &session);
            store_settled(&engine, &session, id);
        }
        assert!(counted(&session, a), "{state}: A is not counted");
        assert!(
            spendable(&engine, &session, id, a),
            "{state}: A is not offered"
        );
        // #378: the balance counts U while the other payment is pending;
        // once it no longer does, flip this to "not counted".
        assert!(counted(&session, u), "{state}: U is not counted");
        assert!(
            !spendable(&engine, &session, id, u),
            "{state}: U is offered"
        );
        let (_draft, any) = prepare(
            &engine,
            &session,
            id,
            CoinSource::Any,
            COIN * 9,
            false,
            &Arc::new(Accept::default()),
        );
        let any = any.unwrap_or_else(|e| panic!("{state}: the automatic choice fails: {e:?}"));
        assert!(
            !(any.summary().inputs.iter()).any(|i| i.outpoint == u),
            "{state}: the automatic choice takes U"
        );
    }
}

/// A prepared payment from A, dropped: key-wallet's reservation goes in a
/// task the drop spawns, held here before it runs. Until it runs, A stays
/// reserved to the engine too, so no plan takes it for key-wallet's build to
/// drop (#745): coin control from A is refused by name, and the automatic
/// choice of everything else spends exactly the other coins. Once it runs, A
/// pays again.
#[test]
fn a_dropped_payments_coin_stays_reserved_until_key_wallet_releases_it() {
    use crate::send::SendFailure;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let inputs = |source: CoinSource, amount: u64, subtract_fee: bool| {
        let (_draft, prepared) = prepare(
            &engine,
            &session,
            id,
            source,
            amount,
            subtract_fee,
            &Arc::new(Accept::default()),
        );
        prepared.map(|p| {
            (p.summary().inputs.iter())
                .map(|i| i.outpoint)
                .collect::<BTreeSet<_>>()
        })
    };
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_release.lock().unwrap() = Some(Arc::clone(&pause));
    let release = crate::held_writes::ReleaseOnDrop(Arc::clone(&pause));
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(
        pause.wait_reached(std::time::Duration::from_secs(20)),
        "the dropped payment's release is held"
    );

    let from_a = inputs(CoinSource::Outpoints(vec![a, b]), COIN * 3 / 2, false);
    assert!(
        matches!(
            from_a,
            Err(crate::EngineError::Send(SendFailure::OutpointUnavailable(o))) if o == a
        ),
        "coin control from A and B while the release is held: {from_a:?}"
    );
    let others: BTreeSet<OutPoint> = (1..COINS as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .collect();
    let any = inputs(CoinSource::Any, (COINS as u64 - 1) * COIN, true);
    assert_eq!(
        any.as_ref().ok(),
        Some(&others),
        "the automatic choice while the release is held: {any:?}"
    );

    drop(release);
    eventually("A and B pay once the releases ran", || {
        inputs(CoinSource::Outpoints(vec![a, b]), COIN * 3 / 2, false).ok()
            == Some(BTreeSet::from([a, b]))
    });
}

/// A coin U of the wallet that only a pending payment from elsewhere
/// spends: another device of the wallet's, or a payment with another
/// party's input too (as a CoinJoin mix or a co-signed one is). key-wallet
/// keeps no record of it across a restart; the history does. After the
/// restart U stays stored spent. A rescan from the wallet's birth credits U
/// again: it is stored unspent and counted in the balance, but the
/// history's pending spend keeps it reserved, after a restart too: never
/// offered, coin control's send of it refuses and the automatic choice
/// passes it over. Once that payment is in a block, U is gone.
#[test]
fn a_coin_a_pending_payment_from_elsewhere_spends_is_never_offered_after_a_rescan() {
    for multi_party in [false, true] {
        let tag = if multi_party {
            "multi-party"
        } else {
            "other device"
        };
        let (_dir, engine, session, id, paid) = wallet_funded_unmined();
        mined(&engine, &session, id, TIP, vec![paid.clone()]);
        let u = OutPoint::new(paid.txid(), 0);
        let owner = Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest)
            .unwrap();
        let mut spend = super::spend_of(
            u,
            vec![
                (
                    fresh_address(&engine, &session, id).script_pubkey(),
                    COIN / 4,
                ),
                (elsewhere(), COIN * 3 / 4 - 2_000),
            ],
        );
        if multi_party {
            spend.input.push(TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([77; 32]), 3),
                ..Default::default()
            });
            spend.output[1].value += COIN;
        }
        let other = super::signed_by_wallet(&engine, &session, id, &owner, spend);
        mempool(&engine, &session, id, &other);
        wait_stored(&session, id, &[other.txid()]);

        let session = reopen(&engine, session);
        assert_eq!(
            held_in(&engine, &session, id, other.txid()),
            (false, false),
            "{tag}"
        );
        start_spv(&engine, &session);
        store_settled(&engine, &session, id);
        let counted = |s: &Arc<NetworkSession>| coins(&engine, s, id).iter().any(|(o, _)| *o == u);
        let row = stored_row(&session, id, u);
        assert_eq!(
            row.map(|(spent, _)| spent),
            Some(true),
            "{tag}, a restart: {row:?}"
        );
        assert!(!counted(&session), "{tag}, a restart: U is counted");

        engine
            .block_on(session.rescan(crate::RescanFrom::WalletBirth))
            .unwrap();
        mined(&engine, &session, id, TIP, vec![paid.clone()]);
        store_settled(&engine, &session, id);
        let mut session = session;
        for state in ["a rescan", "a rescan and a restart"] {
            if state != "a rescan" {
                session = reopen(&engine, session);
                start_spv(&engine, &session);
                store_settled(&engine, &session, id);
            }
            // #378: the balance counts U while that payment is pending;
            // once it no longer does, flip this to "not counted".
            let row = stored_row(&session, id, u);
            assert_eq!(
                row.map(|(spent, _)| spent),
                Some(false),
                "{tag}, {state}: {row:?}"
            );
            assert!(counted(&session), "{tag}, {state}: U is not counted");
            let b = balances(&session, id);
            let sum: u64 = coins(&engine, &session, id).iter().map(|(_, v)| v).sum();
            assert_eq!(b.total, sum, "{tag}, {state}");
            assert!(
                !spendable(&engine, &session, id, u),
                "{tag}, {state}: U is offered"
            );
            let (_draft, by_outpoint) = prepare(
                &engine,
                &session,
                id,
                CoinSource::Outpoints(vec![u]),
                COIN / 2,
                false,
                &Arc::new(Accept::default()),
            );
            assert!(
                matches!(
                    &by_outpoint,
                    Err(crate::EngineError::Send(crate::send::SendFailure::OutpointUnavailable(o)))
                        if *o == u
                ),
                "{tag}, {state}: {:?}",
                by_outpoint.map(|_| ())
            );
            let (_draft, by_any) = prepare(
                &engine,
                &session,
                id,
                CoinSource::Any,
                11 * COIN + COIN / 2,
                false,
                &Arc::new(Accept::default()),
            );
            assert!(
                matches!(
                    &by_any,
                    Err(crate::EngineError::Send(
                        crate::send::SendFailure::AmountExceedsBalance { .. }
                    ))
                ),
                "{tag}, {state}: the automatic choice takes U: {:?}",
                by_any.map(|_| ())
            );
        }

        mined(&engine, &session, id, TIP + 1, vec![other.clone()]);
        store_settled(&engine, &session, id);
        let row = stored_row(&session, id, u);
        assert_eq!(
            row.map(|(spent, _)| spent),
            Some(true),
            "{tag}, in a block: {row:?}"
        );
        assert!(!counted(&session), "{tag}, in a block: U is counted");
    }
}

/// A sync commit above a single Abandon's rewind target, landing after its
/// sweep and before its rescan starts, leaves its rewind record stored, as
/// it does for Remove Unconfirmed: a stop there still rescans at the next
/// start.
#[test]
fn a_sync_commit_during_a_single_abandon_keeps_its_rewind_record() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
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
    start_spv(&engine, &session);
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_before_rescan.lock().unwrap() = Some(Arc::clone(&pause));
    let abandoning = {
        let s = Arc::clone(&session);
        let txid = payment.txid();
        session
            .rt
            .spawn(async move { s.abandon_transaction(id, txid).await })
    };
    if !pause.wait_reached(std::time::Duration::from_secs(60)) {
        let ended = abandoning
            .is_finished()
            .then(|| engine.block_on(abandoning));
        panic!("the abandon never reached its rescan: {ended:?}");
    }
    let replay = Arc::clone(&session.live().unwrap().replay);
    let height = synced(&engine, &session, id) + 1;
    let (before, after) = engine.block_on(async {
        let before = replay.get(id).await.unwrap();
        replay.note(crate::replay_record::ReplayNote::Committed { wallet: id, height });
        replay.settled().await;
        (before, replay.get(id).await.unwrap())
    });
    pause.release.notify_one();
    engine.block_on(abandoning).unwrap().unwrap();
    assert!(before.is_some(), "the abandon stored no rewind record");
    assert_eq!(
        after, before,
        "a sync commit during the abandon deleted its rewind record before its rescan"
    );
}

/// A late event for a transaction no abandon took writes its balance as
/// before: the shown balance follows it at once.
#[test]
fn a_late_event_for_a_live_transaction_writes_its_balance() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid]);
    let incoming = funding(91, &[fresh_address(&engine, &session, id)], COIN);
    let late = credited_with_late_events(&engine, &session, id, &incoming);
    wait_stored(&session, id, &[incoming.txid()]);
    let held_sum: u64 = coins(&engine, &session, id).iter().map(|(_, v)| v).sum();
    assert_ne!(
        balances(&session, id).total,
        held_sum,
        "the history heard it already"
    );
    for event in &late {
        session.hub.apply_wallet_event(event);
    }
    assert_eq!(balances(&session, id).total, held_sum);
}

/// A single Abandon of a locked payment a ChainLock beat, stopped after
/// its sweep and before its rescan, its rewind record lost some other way
/// (an older build's, or a failed rewind write). After the restart, and
/// after Repair ▸ Rescan from the wallet's birth, the payment's other coin
/// is held once and offered, never the coin the winner spent.
#[test]
fn a_rescan_restores_the_coin_of_an_abandon_stopped_without_its_rewind_record() {
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
    let won = block_hash(std::slice::from_ref(&winner));
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);

    (session.hub.quit_before_rescan).store(true, std::sync::atomic::Ordering::SeqCst);
    let stopped = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(stopped.is_err(), "{stopped:?}");
    let replay = Arc::clone(&session.live().unwrap().replay);
    let height = synced(&engine, &session, id) + 1;
    let (before, after) = engine.block_on(async {
        let before = replay.get(id).await.unwrap();
        replay.note(crate::replay_record::ReplayNote::Committed { wallet: id, height });
        replay.settled().await;
        (before, replay.get(id).await.unwrap())
    });
    assert!(before.is_some(), "the abandon stored no rewind record");
    assert!(
        after.is_none(),
        "the rewind record is still stored: {after:?}"
    );

    let session = reopen(&engine, session);
    start_spv(&engine, &session);
    assert!(abandoned(&engine, &session, id, payment.txid()));
    let check = |session: &Arc<NetworkSession>, state: &str| {
        let coins_held = coins(&engine, session, id);
        assert_eq!(
            coins_held.iter().filter(|(o, _)| *o == a).count(),
            1,
            "{state}: the coin is not held exactly once: {coins_held:?}"
        );
        assert_eq!(
            balances(session, id).total,
            coins_held.iter().map(|(_, v)| v).sum::<u64>(),
            "{state}"
        );
        assert!(
            spendable(&engine, session, id, a),
            "{state}: the coin is not offered"
        );
        assert!(
            !spendable(&engine, session, id, b),
            "{state}: the winner's coin is"
        );
    };
    check(&session, "the restart");
    engine
        .block_on(session.rescan(crate::RescanFrom::WalletBirth))
        .unwrap();
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    check(&session, "the rescan");
}

/// Two locked payments a ChainLock beat, of coins funded in different
/// blocks; Remove Unconfirmed takes both, and the abandon of the one whose
/// coins were funded lower fails once its sweep is stored.
/// The checkpoint still goes below that funding block, so the rescan
/// brings its free coin back, whichever ran first.
#[test]
fn a_remove_unconfirmed_that_fails_after_a_sweep_still_rescans_its_coins() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let to = [
        fresh_address(&engine, &session, id),
        fresh_address(&engine, &session, id),
    ];
    let low = funding(8, &to, COIN);
    mempool(&engine, &session, id, &low);
    instant_lock(&engine, &session, id, &low);
    let pay = |coins: Vec<OutPoint>| {
        let payment = send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(coins),
            COIN * 3 / 2,
            false,
        );
        mempool(&engine, &session, id, &payment);
        instant_lock(&engine, &session, id, &payment);
        payment
    };
    let high_payment = pay(vec![
        OutPoint::new(paid.txid(), 0),
        OutPoint::new(paid.txid(), 1),
    ]);
    let low_payment = pay(vec![
        OutPoint::new(low.txid(), 0),
        OutPoint::new(low.txid(), 1),
    ]);
    wait_stored(&session, id, &[high_payment.txid(), low_payment.txid()]);

    let session = reopen(&engine, session);
    let winners = vec![
        rival(&engine, &session, id, &paid, 1),
        rival(&engine, &session, id, &low, 1),
    ];
    mined(&engine, &session, id, TIP + 1, winners.clone());
    let won_txids: Vec<Txid> = winners.iter().map(|w| w.txid()).collect();
    wait_stored(&session, id, &won_txids);
    let won = block_hash(&winners);
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP - 1, vec![low.clone()]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);

    *session.hub.fail_after_sweep_of.lock().unwrap() = Some(low_payment.txid());
    let failed = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert!(failed.is_err(), "{failed:?}");
    let checkpoint = synced(&engine, &session, id);
    assert!(
        checkpoint < TIP - 1,
        "the rescan starts below the lower funding block: {checkpoint}"
    );
    mined(&engine, &session, id, TIP - 1, vec![low.clone()]);
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, winners);
    let freed = OutPoint::new(low.txid(), 0);
    assert!(held(&engine, &session, id).contains(&freed));
    assert!(spendable(&engine, &session, id, freed));
}

/// Remove Unconfirmed of two beaten payments: the first one's abandon has
/// stored its rewind record, and a sync commit above that record lands
/// before the second one starts. The record stays stored until the batch
/// rewinds, so a stop there still replays the first one's coins at the
/// next start.
#[test]
fn a_sync_commit_during_remove_unconfirmed_keeps_its_rewind_record() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let coin = |vout| OutPoint::new(paid.txid(), vout);
    let payments: Vec<Transaction> = [(0, 1), (2, 3)]
        .into_iter()
        .map(|(a, b)| {
            let payment = send(
                &engine,
                &session,
                id,
                CoinSource::Outpoints(vec![coin(a), coin(b)]),
                COIN * 3 / 2,
                false,
            );
            mempool(&engine, &session, id, &payment);
            instant_lock(&engine, &session, id, &payment);
            payment
        })
        .collect();
    let txids: Vec<Txid> = payments.iter().map(|p| p.txid()).collect();
    wait_stored(&session, id, &txids);
    let session = reopen(&engine, session);
    let winners: Vec<Transaction> = [1, 3]
        .into_iter()
        .map(|vout| rival(&engine, &session, id, &paid, vout))
        .collect();
    mined(&engine, &session, id, TIP + 1, winners.clone());
    let won_txids: Vec<Txid> = winners.iter().map(|w| w.txid()).collect();
    wait_stored(&session, id, &won_txids);
    let won = block_hash(&winners);
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);

    let pause = Arc::new(crate::coins::TestPause::default());
    (session.hub.pause_abandon_skip).store(1, std::sync::atomic::Ordering::SeqCst);
    *session.hub.pause_abandon.lock().unwrap() = Some(Arc::clone(&pause));
    let dropping = {
        let s = Arc::clone(&session);
        session
            .rt
            .spawn(async move { s.drop_unconfirmed(Some(id)).await })
    };
    eventually("the second abandon's start", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let replay = Arc::clone(&session.live().unwrap().replay);
    let height = synced(&engine, &session, id) + 1;
    let record = engine.block_on(async {
        replay.note(crate::replay_record::ReplayNote::Committed { wallet: id, height });
        replay.settled().await;
        replay.get(id).await.unwrap()
    });
    pause.release.notify_one();
    assert_eq!(engine.block_on(dropping).unwrap().unwrap(), 2);
    assert!(
        record.is_some_and(|r| r.reached < TIP),
        "the first abandon's rewind record went before the batch rewound: {record:?}"
    );
}

/// Two payments of two coins each, InstantSend-locked; after a restart
/// another device's spends of one coin of each are mined in one block
/// and ChainLocked. One Remove Unconfirmed drops both: the rescan the
/// first one's abandon needs does not stop the second, and once the
/// funding block and the winners' are scanned again each payment's other
/// coin is spendable, never the one its winner spent.
#[test]
fn removing_unconfirmed_drops_every_locked_payment_a_chainlock_beat() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let coin = |vout| OutPoint::new(paid.txid(), vout);
    let payments: Vec<Transaction> = [(0, 1), (2, 3)]
        .into_iter()
        .map(|(a, b)| {
            let payment = send(
                &engine,
                &session,
                id,
                CoinSource::Outpoints(vec![coin(a), coin(b)]),
                COIN * 3 / 2,
                false,
            );
            mempool(&engine, &session, id, &payment);
            instant_lock(&engine, &session, id, &payment);
            payment
        })
        .collect();
    let txids: Vec<Txid> = payments.iter().map(|p| p.txid()).collect();
    wait_stored(&session, id, &txids);

    let session = reopen(&engine, session);
    let winners: Vec<Transaction> = [1, 3]
        .into_iter()
        .map(|vout| rival(&engine, &session, id, &paid, vout))
        .collect();
    mined(&engine, &session, id, TIP + 1, winners.clone());
    let won_txids: Vec<Txid> = winners.iter().map(|w| w.txid()).collect();
    wait_stored(&session, id, &won_txids);
    let won = block_hash(&winners);
    chain_lock(&engine, &session, id, TIP + 1, won);
    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);

    assert_eq!(
        engine.block_on(session.drop_unconfirmed(Some(id))).unwrap(),
        2
    );
    for txid in &txids {
        assert!(abandoned(&engine, &session, id, *txid));
    }
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, winners);
    for (free, spent) in [(0, 1), (2, 3)] {
        assert!(spendable(&engine, &session, id, coin(free)), "{free} is");
        assert!(
            !spendable(&engine, &session, id, coin(spent)),
            "{spent} is not"
        );
    }
}

/// #170 case 14 as it most often comes: the payment InstantSend-locked, a
/// restart, then another device's spend of B mined and ChainLocked. In
/// that session the payment's claim, restored at the load, keeps A out of
/// key-wallet's coins through a rescan, so Abandon refuses; after the next
/// restart the rescan credits A and Abandon goes through: once its rescan
/// is done the wallet holds A once, never B nor the payment's change.
#[test]
fn a_locked_payment_beaten_after_a_restart_is_abandoned_only_after_the_next() {
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
    chain_lock(&engine, &session, id, TIP + 1, won);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    start_spv(&engine, &session);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A is not credited"
    );
    let refused = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            refused,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RescanFirst
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again."
    );

    let session = reopen(&engine, session);
    header_chain(&session, &[(TIP + 1, won)]);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    start_spv(&engine, &session);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    assert!(abandoned(&engine, &session, id, payment.txid()));
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = outputs(&paid);
    want.remove(&b);
    want.insert(change_of(&winner, &own_scripts(&engine, &session, id)));
    assert_holds(&engine, &session, id, &want, "abandoned: A once, B spent");
    change_is_gone(&engine, &session, id, (&payment, change), "abandoned");
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(!spendable(&engine, &session, id, b), "B is not offered");
}

/// A payment of A and B, InstantSend-locked, and its unlocked child that
/// spends its change and another coin D of the wallet; after a restart
/// another device's spend of B is mined and ChainLocked, and after the
/// next the funding block is scanned again. That load restores the child's
/// claim on D, as every later one would: a restart and a rescan would not
/// help, so Abandon refuses as InstantSend-locked, not with `RescanFirst`.
#[test]
fn a_locked_payment_a_chainlock_beat_under_a_child_claiming_its_own_coin_is_refused_as_locked() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b, d) = (
        OutPoint::new(paid.txid(), 0),
        OutPoint::new(paid.txid(), 1),
        OutPoint::new(paid.txid(), 2),
    );
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
        CoinSource::Outpoints(vec![change, d]),
        COIN,
        false,
    );
    mempool(&engine, &session, id, &child);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);

    let session = reopen(&engine, session);
    let (session, _) = beaten_on_b(&engine, session, id, &paid);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
    assert!(
        !held(&engine, &session, id).contains(&d),
        "the child claims D"
    );
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    refused(&engine, &session, id, payment.txid(), "the child claims D");
    assert!(!abandoned(&engine, &session, id, payment.txid()));
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
    header_chain(&session, &[(TIP + 1, won)]);
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

/// As above, but the child is in a block rather than InstantSend-locked: a
/// mined descendant refuses the abandon whatever beat the payment, and its
/// other output stays held (#446).
#[test]
fn abandoning_an_unlocked_payment_a_chainlock_beat_under_a_mined_child_is_refused() {
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
    mined(&engine, &session, id, TIP + 1, vec![child.clone()]);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    let left = OutPoint::new(payment.txid(), 1);

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&left));
    let winner = rival(&engine, &session, id, &paid, 1);
    assert!(
        (session.hub.history.get(&id, &child.txid()))
            .is_some_and(|e| e.context.block_info().is_some()),
        "the history holds the child in its block"
    );
    mined(&engine, &session, id, TIP + 2, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    chain_lock(&engine, &session, id, TIP + 2, won);
    header_chain(&session, &[(TIP + 2, won)]);
    start_spv(&engine, &session);
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(!extras.can_abandon, "Abandon is offered");
    let result = engine.block_on(session.abandon_transaction(id, payment.txid()));
    assert!(
        matches!(
            result,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::SettledDescendant
            ))
        ),
        "{result:?}"
    );
    assert!(!abandoned(&engine, &session, id, payment.txid()));
    assert!(
        held(&engine, &session, id).contains(&left),
        "its other output is gone"
    );
}

/// A payment of A and B without change (one with change goes by itself,
/// #170 case 3), not InstantSend-locked; after a restart another device's
/// spend of B is mined and ChainLocked. Abandoning the payment frees A on
/// disk at once, as without a ChainLock: a restart before any scan holds
/// A, never B.
#[test]
fn abandoning_an_unlocked_payment_a_chainlock_beat_frees_its_other_coin_after_a_restart() {
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
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    let winner = rival(&engine, &session, id, &paid, 1);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    let won = block_hash(&[winner]);
    chain_lock(&engine, &session, id, TIP + 1, won);
    header_chain(&session, &[(TIP + 1, won)]);
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
/// once the claims are installed: key-wallet tells only the recipient
/// wallet, and the sender's history learns of it from that (#828), so A
/// stays claimed and is not offered.
#[test]
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
    lock_reruns_done(&engine, &session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// Waits for the claim choices a lock no event of the wallet named started.
fn lock_reruns_done(engine: &Engine, session: &NetworkSession) {
    let runs = session.hub.take_lock_reruns();
    engine.block_on(async {
        for r in runs {
            r.await.unwrap();
        }
    });
}

/// A payment P of A and B with no change to `to`, stored before its
/// InstantSend lock; after a restart another device's spend of B is mined
/// in a plain block, and after the next the session is open. Returns the
/// session, P and the winner.
fn unlocked_payment_beaten_on_b(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    to: &str,
) -> (Arc<NetworkSession>, Transaction, Transaction) {
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let payment = send_to(
        engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        to,
        2 * COIN,
        true,
    );
    assert_eq!(payment.output.len(), 1, "no change");
    mempool(engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);
    let session = reopen(engine, session);
    let winner = rival(engine, &session, id, paid, 1);
    mined(engine, &session, id, TIP + 1, vec![winner.clone()]);
    wait_stored(&session, id, &[winner.txid()]);
    (reopen(engine, session), payment, winner)
}

/// A second wallet loaded at startup: a fresh address of it.
fn startup_recipient(engine: &Engine, session: &Arc<NetworkSession>) -> String {
    let other = second_wallet(engine, session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    fresh_address(engine, session, other).to_string()
}

/// Whether `id`'s history shows `txid` InstantSend-locked.
fn history_shows_locked(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    (session.hub.history.get(&id, &txid)).is_some_and(|e| crate::history::instant_locked(&e))
}

/// The same payment paid to an address of no loaded wallet: no wallet event
/// names its lock, only the SPV's (#828). An unverified one changes
/// nothing; the verified one, heard before key-wallet takes it, keeps A
/// claimed.
#[test]
fn a_lock_no_wallet_event_names_keeps_a_locked_payments_claims() {
    use dash_spv::EventHandler;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, FOREIGN);
    let lock = InstantLock {
        txid: payment.txid(),
        ..InstantLock::default()
    };
    let heard = |validated| dash_spv::sync::SyncEvent::InstantLockReceived {
        instant_lock: lock.clone(),
        validated,
    };

    session.hub.on_sync_event(&heard(false));
    lock_reruns_done(&engine, &session);
    assert!(
        !history_shows_locked(&session, id, payment.txid()),
        "unverified: no lock"
    );
    session.hub.on_sync_event(&heard(true));
    lock_reruns_done(&engine, &session);
    assert!(
        history_shows_locked(&session, id, payment.txid()),
        "the history shows the lock"
    );
    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// The lock of a payment to another loaded wallet arriving after the load,
/// then a restart: the sender's store keeps the lock, so the next load
/// keeps A claimed when the funding block is scanned again.
#[test]
fn a_lock_arriving_after_the_load_keeps_a_locked_payments_claims_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    let session = reopen(&engine, session);
    assert!(
        history_shows_locked(&session, id, payment.txid()),
        "the load reads the lock"
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// Whether the wallet's store holds a verified InstantSend lock of `txid`.
fn stored_lock(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    session.live().unwrap().persister.flush(id.0).unwrap();
    let conn = rusqlite::Connection::open(session.data_dir().join(crate::session::WALLET_DB_FILE))
        .unwrap();
    conn.query_row(
        "SELECT count(*) FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2 \
         AND verified = 1",
        rusqlite::params![&id.0[..], AsRef::<[u8]>::as_ref(&txid)],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

/// The same payment with its record lost before the load, so the history
/// holds it from its sent mark alone, its lock arriving after the load,
/// then a restart: the sender's store keeps the lock with no record, so the
/// next load reads the payment locked and keeps A claimed when the funding
/// block is scanned again.
#[test]
fn a_lock_arriving_after_the_load_of_a_payment_with_no_record_keeps_its_claims_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    delete_stored_record(&session, id, payment.txid());
    let session = reopen(&engine, session);
    assert!(
        session.hub.history.get(&id, &payment.txid()).is_some(),
        "the history holds the payment from its mark"
    );

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        !stored_record(&session, id, payment.txid()),
        "the sender stores no record"
    );
    assert!(
        stored_lock(&session, id, payment.txid()),
        "the sender stores the lock"
    );
    let session = reopen(&engine, session);
    assert!(
        history_shows_locked(&session, id, payment.txid()),
        "the load reads the lock"
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// The same payment with no record, the winner's block ChainLocked before
/// its lock arrives after the load: the lock claims nothing, so no lock is
/// stored without a record, and after a restart the funding block credits
/// A.
#[test]
fn a_lock_of_a_payment_with_no_record_a_chainlock_beat_is_not_stored() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, winner) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    delete_stored_record(&session, id, payment.txid());
    let session = reopen(&engine, session);
    chain_lock(&engine, &session, id, TIP + 1, block_hash(&[winner]));

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        !stored_lock(&session, id, payment.txid()),
        "no lock stored without a record"
    );
    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
}

/// The lock of a payment to an address of no loaded wallet heard after the
/// load while the sender's store fails, then another transaction's lock
/// once it works, then a restart: the lock is kept until it is stored, so
/// the next load keeps A claimed when the funding block is scanned again.
#[test]
fn a_lock_whose_store_failed_is_stored_at_the_next_lock_heard() {
    use dash_spv::EventHandler;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, FOREIGN);
    let txid = payment.txid();
    let heard = |txid| dash_spv::sync::SyncEvent::InstantLockReceived {
        instant_lock: InstantLock {
            txid,
            ..InstantLock::default()
        },
        validated: true,
    };
    let fail = |fails: bool| {
        *session.live().unwrap().store.fail_store.lock().unwrap() = fails.then(|| {
            Box::new(
                move |cs: &platform_wallet::changeset::PlatformWalletChangeSet| {
                    (cs.core.as_ref())
                        .is_some_and(|c| c.instant_locks_for_non_final_records.contains_key(&txid))
                },
            ) as crate::store::StoreFailure
        });
    };

    fail(true);
    session.hub.on_sync_event(&heard(txid));
    lock_reruns_done(&engine, &session);
    assert!(
        history_shows_locked(&session, id, txid),
        "the history shows the lock"
    );
    assert!(!stored_lock(&session, id, txid), "the store failed");
    fail(false);
    session
        .hub
        .on_sync_event(&heard(Txid::from_byte_array([9; 32])));
    lock_reruns_done(&engine, &session);
    assert!(
        stored_lock(&session, id, txid),
        "stored once a lock is heard again"
    );
    let session = reopen(&engine, session);
    assert!(
        history_shows_locked(&session, id, txid),
        "the load reads the lock"
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(!held(&engine, &session, id).contains(&a), "A stays claimed");
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// The winner's block ChainLocked before the payment's lock arrives after
/// the load: only a ChainLocked block beats a locked payment, and this one
/// does, so the lock claims nothing and the funding block credits A.
#[test]
fn a_lock_arriving_after_the_load_claims_nothing_once_a_chainlock_beat_the_payment() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, winner) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    chain_lock(&engine, &session, id, TIP + 1, block_hash(&[winner]));

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
}

/// The funding block scanned again before the lock of a payment to another
/// loaded wallet arrives: A is credited first, so the lock reaches the
/// sender through A and key-wallet spends it; the claim choice run again
/// beside it leaves A spent, after a restart too.
#[test]
fn a_lock_arriving_after_the_funding_block_leaves_the_coin_spent() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(
        held(&engine, &session, id).contains(&a),
        "unlocked and beaten: A is credited"
    );

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "the lock claims A"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
    assert_eq!(
        stored_row(&session, id, a).map(|r| r.0),
        Some(true),
        "A is stored spent"
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A stays spent after a restart"
    );
}

/// A load that could not read a stored record restores no claim: a lock
/// arriving after it shows in the history and still claims nothing.
#[test]
fn a_lock_arriving_after_an_incomplete_load_claims_nothing() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let to = startup_recipient(&engine, &session);
    let (session, payment, winner) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    engine.block_on(session.unload_wallet(id)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(winner.txid());
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(
        session
            .hub
            .history
            .read_wallet(&id, |h| h.claim_marks.is_none()),
        "the load restored no claim"
    );

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        history_shows_locked(&session, id, payment.txid()),
        "the history shows the lock"
    );
    assert!(
        session
            .hub
            .history
            .read_wallet(&id, |h| h.claimed.values().all(|t| *t != payment.txid())),
        "and the payment claims nothing"
    );
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

/// Answers the session's header lookups from `chain` alone: a height it
/// does not name has no header yet.
pub(super) fn header_chain(session: &NetworkSession, chain: &[(u32, dashcore::BlockHash)]) {
    let chain: std::collections::HashMap<u32, dashcore::BlockHash> =
        chain.iter().copied().collect();
    session.hub.attach_headers(Arc::new(move |height| {
        let hash = chain.get(&height).copied();
        Box::pin(async move { hash })
    }));
}

/// P, a payment of `id` whose block the scan reaches after the wallet
/// applied a ChainLock at its height naming `locked(P)`: the record is
/// stored ChainLocked by height (key-wallet's catch-up), and no ChainLock
/// of the session names it.
fn mined_below_a_chainlock(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    locked: impl FnOnce(&Transaction) -> dashcore::BlockHash,
) -> Transaction {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let p = send(engine, session, id, CoinSource::Any, COIN / 2, false);
    mempool(engine, session, id, &p);
    chain_lock(engine, session, id, TIP + 1, locked(&p));
    mined(engine, session, id, TIP + 1, vec![p.clone()]);
    let persister = Arc::clone(&session.live().unwrap().persister);
    eventually("the record stored ChainLocked", || {
        persister.flush(id.0).unwrap();
        (persister.get_core_tx_record(id.0, &p.txid()).unwrap()).is_some_and(|r| {
            matches!(
                r.context,
                key_wallet::transaction_checking::TransactionContext::InChainLockedBlock(_)
            )
        })
    });
    p
}

/// Wallet `id` closed and opened again with `chain` as the header chain.
fn reload_with_headers(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    chain: &[(u32, dashcore::BlockHash)],
) {
    engine.block_on(session.unload_wallet(id)).unwrap();
    header_chain(session, chain);
    engine.block_on(session.load_wallet(id)).unwrap();
}

/// A record stored ChainLocked in block X at H, while the header chain
/// holds Y at H, the block the wallet's ChainLock names: a stored context
/// proves nothing, and the mark stays.
#[test]
fn a_record_stored_chainlocked_in_a_block_the_header_chain_replaced_keeps_its_mark() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let y = super::block_with(vec![foreign()]).block_hash();
    let p = mined_below_a_chainlock(&engine, &session, id, |_| y);
    reload_with_headers(&engine, &session, id, &[(TIP + 1, y)]);
    assert!(sent_marks(&session, id).contains(&p.txid()));
}

/// A payment mined after its ChainLock was applied (stored ChainLocked by
/// height at catch-up), whose block the session's locks never named: its
/// mark goes at the next load whose header chain holds its block, which is
/// the block the wallet's ChainLock names.
#[test]
fn a_record_chainlocked_at_catch_up_loses_its_mark_once_the_header_chain_proves_it() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = mined_below_a_chainlock(&engine, &session, id, |p| {
        super::block_with(vec![p.clone()]).block_hash()
    });
    let x = super::block_with(vec![p.clone()]).block_hash();
    reload_with_headers(&engine, &session, id, &[(TIP + 1, x)]);
    assert!(!sent_marks(&session, id).contains(&p.txid()));
}

/// The same payment with the header chain behind its height: no proof,
/// and the mark stays.
#[test]
fn a_header_chain_behind_a_payments_block_keeps_its_mark() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = mined_below_a_chainlock(&engine, &session, id, |p| {
        super::block_with(vec![p.clone()]).block_hash()
    });
    let x = super::block_with(vec![p.clone()]).block_hash();
    reload_with_headers(&engine, &session, id, &[(TIP, x)]);
    assert!(sent_marks(&session, id).contains(&p.txid()));
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

/// A ChainLock of a later block (the next one, at another height): it
/// covers the payment's block, and key-wallet promotes its record by
/// height, but it names another block, so the payment's mark stays.
#[test]
fn a_chainlock_of_a_later_block_keeps_an_earlier_payments_mark() {
    let (_dir, engine, session, id, _) = wallet_funded_unmined();
    let p = send(&engine, &session, id, CoinSource::Any, COIN / 2, false);
    mempool(&engine, &session, id, &p);
    mined(&engine, &session, id, TIP + 1, vec![p.clone()]);
    chain_locked_sentinel(&engine, &session, id, TIP + 2);
    assert!(
        (session.hub.history.get(&id, &p.txid())).is_some_and(|e| matches!(
            e.context,
            key_wallet::transaction_checking::TransactionContext::InChainLockedBlock(_)
        )),
        "the later lock promoted it"
    );
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

/// #172: Reset chain data with a wallet closed. Its next open rescans it
/// from its birth height, as the reset does for an open wallet, so what the
/// old block data's start hid from it comes in; its stored checkpoint alone
/// would resume above that start.
#[test]
fn a_wallet_closed_through_a_reset_of_chain_data_rescans_from_its_birth_when_opened() {
    let (_dir, engine, session, id) = engine_wallet();
    let open = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong".to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    for wallet in [id, open] {
        mined(&engine, &session, wallet, TIP, vec![foreign()]);
        assert_eq!(synced(&engine, &session, wallet), TIP);
    }
    engine.block_on(session.unload_wallet(id)).unwrap();
    engine.block_on(session.reset_chain_data()).unwrap();
    assert_eq!(
        synced(&engine, &session, open),
        BIRTH - 1,
        "the open wallet"
    );
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(
        synced(&engine, &session, id),
        BIRTH - 1,
        "the closed wallet"
    );
}

/// A payment to another loaded wallet abandoned after the load, its lock
/// arriving afterwards: the lock outranks the abandon, as at a load that
/// reads it, so the payment claims A again and keeps its sent mark.
#[test]
fn a_lock_arriving_after_an_abandon_this_session_claims_the_coins_again() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    abandon(&engine, &session, id, payment.txid());

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        !abandoned(&engine, &session, id, payment.txid()),
        "the lock outranks the abandon"
    );
    assert!(
        sent_marks(&session, id).contains(&payment.txid()),
        "its sent mark stays"
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "A is claimed again"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");
}

/// A payment D of C whose saved copy is damaged, retired by Remove
/// Unconfirmed after the load, and then the lock of a payment to another
/// loaded wallet that the load found beaten: the lock chooses again only
/// what it revives, so the payment claims A while nothing claims C again,
/// and C stays counted and stored unspent, after a restart too.
#[test]
fn a_lock_after_remove_unconfirmed_leaves_a_retired_damaged_payments_coin_free() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, c) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 2));
    let damaged = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![c]),
        COIN / 2,
        false,
    );
    {
        let (appdb, scope) = (&session.live().unwrap().appdb, id.to_string());
        let key = format!("{}{}", crate::tx_actions::SENT_TX_PREFIX, damaged.txid());
        appdb.set_setting(&scope, &key, Some("zz")).unwrap();
    }
    let to = startup_recipient(&engine, &session);
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    start_offline_spv(&engine, &session);
    engine.block_on(session.drop_unconfirmed(Some(id))).unwrap();
    assert!(
        !sent_marks(&session, id).contains(&damaged.txid()),
        "the damaged payment is retired"
    );

    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&c), "C is counted");

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    assert!(
        !held(&engine, &session, id).contains(&a),
        "the lock claims A"
    );
    assert!(
        held(&engine, &session, id).contains(&c),
        "nothing claims C again"
    );
    assert_eq!(
        stored_row(&session, id, c).map(|r| r.0),
        Some(false),
        "C is stored unspent"
    );
    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(
        held(&engine, &session, id).contains(&c),
        "C after a restart"
    );
}

/// A payment P to another loaded wallet, beaten on B by a plain block and
/// abandoned after the load; once the rescan has credited A, P2 spends A
/// with change, and then P's lock arrives: the locked P beats P2 on A, so
/// P2 and its change go, and the change is never counted, offered or
/// stored, after a restart too.
#[test]
fn a_lock_after_an_abandon_and_a_new_spend_of_its_coin_never_counts_the_new_spends_change() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let to = startup_recipient(&engine, &session);
    let (session, payment, winner) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    abandon(&engine, &session, id, payment.txid());
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![winner.clone()]);
    let mut want = held(&engine, &session, id);
    assert!(want.remove(&a), "the rescan credits A");
    let p2 = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p2);
    wait_stored(&session, id, &[p2.txid()]);
    let change = change_of(&p2, &own_scripts(&engine, &session, id));
    assert!(
        held(&engine, &session, id).contains(&change),
        "P2's change is counted before the lock"
    );

    instant_lock(&engine, &session, id, &payment);
    lock_reruns_done(&engine, &session);
    let gone = |session: &Arc<NetworkSession>, state: &str| {
        assert_holds(&engine, session, id, &want, state);
        assert!(
            !spendable_for(&engine, session, id, change, COIN / 10),
            "{state}: P2's change is not offered"
        );
        assert_eq!(
            stored_row(session, id, change),
            None,
            "{state}: P2's change is not stored"
        );
        assert!(
            session.hub.history.get(&id, &p2.txid()).is_none(),
            "{state}: the history drops P2"
        );
        assert!(
            !stored_record(session, id, p2.txid()),
            "{state}: the store drops P2"
        );
    };
    gone(&session, "the lock");
    assert!(
        !abandoned(&engine, &session, id, payment.txid()),
        "the lock outranks the abandon"
    );
    assert!(!spendable(&engine, &session, id, a), "A is not offered");

    let session = reopen(&engine, session);
    gone(&session, "a restart");
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "the funding block scanned again leaves P2's change out"
    );
}

/// The payment to another loaded wallet, beaten on B, then that wallet's
/// record of it detected with a lock naming another transaction: the lock
/// isn't the payment's, so the sender's history doesn't show it locked, no
/// claim choice runs again and no lock is stored, and the funding block
/// credits A.
#[test]
fn a_detected_record_whose_lock_names_another_transaction_changes_nothing() {
    use key_wallet::transaction_checking::TransactionContext;
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let to = fresh_address(&engine, &session, other).to_string();
    let (session, payment, _) = unlocked_payment_beaten_on_b(&engine, session, id, &paid, &to);
    let txid = payment.txid();
    let mut record = {
        use platform_wallet::changeset::PlatformWalletPersistence;
        (session.live().unwrap().persister)
            .get_core_tx_record(other.0, &txid)
            .unwrap()
            .expect("the recipient stores the payment")
    };
    let manager = session.manager().unwrap();
    let (balance, account_balances) = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&other.0).unwrap().core_wallet;
        (info.balance(), info.account_balances())
    });
    record.context = TransactionContext::InstantSend(InstantLock {
        txid: Txid::from_byte_array([9; 32]),
        ..InstantLock::default()
    });

    (session.hub).apply_wallet_event(
        &platform_wallet::key_wallet_manager::WalletEvent::TransactionDetected {
            wallet_id: other.0,
            record: Box::new(record),
            balance,
            account_balances,
            addresses_derived: Vec::new(),
        },
    );
    assert!(
        session.hub.take_lock_reruns().is_empty(),
        "no claim choice runs again"
    );
    assert!(
        !history_shows_locked(&session, id, txid),
        "the sender's history doesn't show it locked"
    );
    assert!(!stored_lock(&session, id, txid), "no lock is stored");
    mined(&engine, &session, id, TIP, vec![paid]);
    assert!(held(&engine, &session, id).contains(&a), "A is credited");
}
