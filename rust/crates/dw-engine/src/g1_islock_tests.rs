//! G1: an InstantSend lock dash-spv has not validated never makes a payment
//! final in the engine: not in the history, the balances or the coin list,
//! not in the wallet store, and not after the engine reopens the network
//! through its real load.
//!
//! dash-spv's `DashSpvClient`, with its InstantSend and mempool managers,
//! runs over `MockNetworkManager` against the session's own wallet manager;
//! the session hub hears the client's events as it does from the SPV
//! runtime, and the session's persister writes through its own adapter. The
//! client holds no masternode list, so no quorum can validate the lock. The
//! production SPV runtime builds the same client over the peer network, which
//! a test cannot feed a lock that fails validation.
//!
//! The client is built as the runtime builds it, with two differences that
//! do not touch the lock path: no compact filters (the mock serves no
//! chain to filter), and the session hub as its only engine handler, not
//! platform-wallet's event manager. The store's writer hears the wallet
//! manager's own events, not the handlers, and the other handlers act only on
//! a validated `InstantLockReceived` or a `TransactionInstantLocked`.
//!
//! A lock stored unverified (a store written before locks were checked)
//! is not final after the reopen either. A control runs the same checks on a
//! lock the wallet takes directly, so each check is shown able to fail.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dash_spv::error::NetworkResult;
use dash_spv::network::{Inbound, MessageType, NetworkEvent, NetworkManager, RequestKey};
use dash_spv::test_utils::{test_socket_address, MockNetworkManager};
use dash_spv::{ClientConfig, DashSpvClient, MempoolStrategy, SyncEvent};
use dashcore::bls_sig_utils::BLSSignature;
use dashcore::network::message::NetworkMessage;
use dashcore::{InstantLock, Transaction, Txid};
use platform_wallet::changeset::PlatformWalletPersistence;
use platform_wallet::key_wallet_manager::WalletManager;
use platform_wallet::PlatformWalletInfo;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{balances, engine_wallet, payment, receive_address, reopen, rows, PAID};
use crate::coins::{CoinFilter, CoinInfo};
use crate::history::TxStatusKind;
use crate::session::{NetworkSession, WALLET_DB_FILE};
use crate::{Engine, WalletId};

const BARRIER: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(3);
/// The positive case waits for a DKG cycle on a loaded machine.
const QUORUM_BARRIER: Duration = Duration::from_secs(300);

/// The client owns its network by value; the test keeps a handle to inject
/// messages through it.
struct SharedNetwork(Arc<MockNetworkManager>);

#[async_trait::async_trait]
impl NetworkManager for SharedNetwork {
    async fn start(&self) {
        self.0.start().await
    }
    async fn stop(&self) {
        self.0.stop().await
    }
    async fn enqueue(&self, height: u32, msg: NetworkMessage) {
        self.0.enqueue(height, msg).await
    }
    async fn send_to(&self, addr: SocketAddr, msg: NetworkMessage) -> bool {
        self.0.send_to(addr, msg).await
    }
    async fn broadcast(&self, msg: NetworkMessage) -> NetworkResult<()> {
        self.0.broadcast(msg).await
    }
    async fn dispatch_local(&self, msg: NetworkMessage) {
        self.0.dispatch_local(msg).await
    }
    async fn release_request(&self, key: RequestKey) {
        self.0.release_request(key).await
    }
    async fn subscribe(&self, kinds: &[MessageType]) -> UnboundedReceiver<Inbound> {
        self.0.subscribe(kinds).await
    }
    fn events(&self) -> broadcast::Receiver<NetworkEvent> {
        self.0.events()
    }
    fn tip(&self) -> u32 {
        self.0.tip()
    }
    async fn connected_count(&self) -> u32 {
        self.0.connected_count().await
    }
    async fn ban_peer(&self, addr: SocketAddr) {
        self.0.ban_peer(addr).await
    }
}

type Client = DashSpvClient<WalletManager<PlatformWalletInfo>, SharedNetwork>;

#[derive(Clone, Copy, Debug)]
enum Order {
    /// The lock right behind the transaction, with no wait.
    Together,
    /// The InstantSend manager has handled the lock before the transaction
    /// arrives.
    LockFirst,
    /// The payment is in the history and the store before the lock arrives.
    TxFirst,
}

/// The `InstantLockReceived` events the client emits, as `(txid, validated)`.
#[derive(Default)]
struct LockEvents(std::sync::Mutex<Vec<(Txid, bool)>>);

impl dash_spv::EventHandler for LockEvents {
    fn on_sync_event(&self, event: &SyncEvent) {
        if let SyncEvent::InstantLockReceived {
            instant_lock,
            validated,
        } = event
        {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((instant_lock.txid, *validated));
        }
    }
}

impl LockEvents {
    fn of(&self, txid: &Txid) -> Vec<bool> {
        (self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
            .iter()
            .filter(|(t, _)| t == txid)
            .map(|(_, validated)| *validated)
            .collect()
    }
}

/// A well-formed lock for `tx` that no quorum the client holds can
/// validate.
fn unvalidatable_lock(tx: &Transaction) -> InstantLock {
    InstantLock {
        inputs: vec![tx.input[0].previous_output],
        txid: tx.txid(),
        signature: BLSSignature::from([0x11; 96]),
        ..InstantLock::default()
    }
}

/// The `verified` column of each of `wallet`'s lock rows for `txid`.
fn lock_rows(session: &NetworkSession, wallet: WalletId, txid: &Txid) -> Vec<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        session.data_dir().join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT verified FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2")
        .unwrap();
    stmt.query_map(
        rusqlite::params![wallet.0.as_slice(), AsRef::<[u8]>::as_ref(txid)],
        |r| r.get(0),
    )
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn record_stored(session: &NetworkSession, txid: &Txid) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        session.data_dir().join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM core_transactions WHERE txid = ?1 AND record_blob IS NOT NULL",
        [AsRef::<[u8]>::as_ref(txid)],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

/// One coin of the payment as the engine holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Coin {
    /// The coin list's InstantSend flag.
    listed_locked: bool,
    /// Confirmed, InstantSend-locked or own change (Dash Core `IsTrusted`):
    /// what lets automatic selection spend it. `None`: not in the snapshot
    /// the spends select from.
    trusted: Option<bool>,
    /// Neither user-locked, reserved nor immature: whether it is held back for
    /// another reason.
    selectable: Option<bool>,
}

/// What the engine shows of the payment: its history rows, the balances,
/// and its coins.
#[derive(Debug)]
struct View {
    rows: Vec<(TxStatusKind, bool, i64)>,
    confirmed: u64,
    unconfirmed: u64,
    coins: Vec<Coin>,
}

fn view(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: &Txid) -> View {
    let b = balances(session, id);
    let listed = engine
        .block_on(session.utxos(
            id,
            CoinFilter {
                include_locked: true,
                ..Default::default()
            },
        ))
        .unwrap();
    let fence = engine.block_on(session.coin_fence(id)).unwrap();
    let wallet = engine.block_on(session.wallet(&id)).unwrap();
    let snap = engine
        .block_on(session.coin_snapshot(&fence, &wallet))
        .unwrap();
    let coin = |c: &CoinInfo| {
        let held = snap.coins.iter().find(|w| w.utxo.outpoint == c.outpoint);
        Coin {
            listed_locked: c.instant_locked,
            trusted: held.map(|w| w.utxo.is_confirmed || w.utxo.is_instantlocked || w.utxo.is_trusted),
            selectable: held.map(|w| w.chosen_selectable(snap.height)),
        }
    };
    View {
        rows: (rows(engine, session, id).into_iter())
            .filter(|(t, ..)| *t == txid.to_string())
            .map(|(_, kind, locked, amount)| (kind, locked, amount))
            .collect(),
        confirmed: b.confirmed,
        unconfirmed: b.unconfirmed,
        coins: listed
            .iter()
            .filter(|c| c.outpoint.txid == *txid)
            .map(coin)
            .collect(),
    }
}

const NOT_FINAL_COIN: Coin = Coin {
    listed_locked: false,
    trusted: Some(false),
    selectable: Some(true),
};

/// The payment is in the engine and not final: one unconfirmed row without
/// a lock, its amount unconfirmed and none confirmed, and one coin that is
/// not InstantSend-locked or trusted and is held back for nothing else.
fn not_final_violation(v: &View) -> Option<String> {
    let amount = i64::try_from(PAID).unwrap();
    let ok = v.rows == [(TxStatusKind::Unconfirmed, false, amount)]
        && v.confirmed == 0
        && v.unconfirmed == PAID
        && v.coins == [NOT_FINAL_COIN];
    (!ok).then(|| {
        format!(
            "want one Unconfirmed row without a lock for {PAID}, confirmed 0, \
             unconfirmed {PAID} and one selectable coin neither locked nor trusted; got {v:?}"
        )
    })
}

fn wait_for(what: &str, barrier: Duration, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < barrier, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_until(what: &str, ready: impl FnMut() -> bool) {
    wait_for(what, BARRIER, ready);
}

fn instantsend_pending(engine: &Engine, client: &Client) -> usize {
    engine.block_on(async {
        (client.progress().await.instantsend())
            .map(|p| p.pending())
            .unwrap_or(0)
    })
}

/// A client over a mock network on the session's wallet manager, its events
/// to the session hub and to `locks`, running.
struct Spv {
    client: Client,
    net: Arc<MockNetworkManager>,
    locks: Arc<LockEvents>,
    _dir: tempfile::TempDir,
}

impl Spv {
    fn start(engine: &Engine, session: &Arc<NetworkSession>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let wm = session.manager().unwrap().wallet_manager_arc();
        let net = Arc::new(MockNetworkManager::new());
        let locks = Arc::new(LockEvents::default());
        let config = ClientConfig::regtest()
            .without_filters()
            .with_mempool_tracking(MempoolStrategy::FetchAll)
            .with_storage_path(dir.path());
        assert!(
            config.enable_masternodes,
            "the InstantSend manager needs masternodes on"
        );
        let handlers: Vec<Arc<dyn dash_spv::EventHandler>> = vec![
            Arc::clone(&session.hub) as Arc<dyn dash_spv::EventHandler>,
            Arc::clone(&locks) as Arc<dyn dash_spv::EventHandler>,
        ];
        let client = engine.block_on(async {
            let client =
                DashSpvClient::new(config, SharedNetwork(Arc::clone(&net)), wm, handlers)
                    .await
                    .expect("spv client");
            client.run().await.expect("run spv client");
            client
        });
        Spv {
            client,
            net,
            locks,
            _dir: dir,
        }
    }

    /// The violation if the client did not hold `txid`'s lock back: it must
    /// have emitted nothing for it and hold exactly one lock pending.
    fn forwarded(&self, engine: &Engine, txid: &Txid) -> Option<String> {
        let events = self.locks.of(txid);
        let pending = instantsend_pending(engine, &self.client);
        (!events.is_empty() || pending != 1).then(|| {
            format!("InstantLockReceived events (validated) {events:?}, {pending} lock(s) pending")
        })
    }

    fn stop(self, engine: &Engine) {
        engine.block_on(self.client.stop());
    }
}

/// Delivers a payment and an unverifiable lock for it in `order`, waiting
/// until the payment is in the history and the store and the lock reached
/// the InstantSend manager. The client keeps running.
fn deliver_unvalidated(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    order: Order,
) -> (Transaction, Spv) {
    let spv = Spv::start(engine, session);
    let tx = payment(&receive_address(engine, session, id));
    let txid = tx.txid();
    let lock = unvalidatable_lock(&tx);
    let peer = test_socket_address(1);
    let send = |msg: NetworkMessage| engine.block_on(spv.net.inject(peer, msg));
    let tx_msg = || NetworkMessage::Tx(Box::new(tx.clone()));
    let lock_msg = || NetworkMessage::ISLock(lock.clone());
    let tx_seen = || {
        session.live().unwrap().persister.flush(id.0).unwrap();
        record_stored(session, &txid) && rows(engine, session, id).len() == 1
    };
    let lock_seen =
        || instantsend_pending(engine, &spv.client) >= 1 || !spv.locks.of(&txid).is_empty();
    match order {
        Order::Together => {
            send(tx_msg());
            send(lock_msg());
            wait_until("the payment in the history and the store", tx_seen);
            wait_until("the lock at the InstantSend manager", lock_seen);
        }
        Order::LockFirst => {
            send(lock_msg());
            wait_until("the lock at the InstantSend manager", lock_seen);
            send(tx_msg());
            wait_until("the payment in the history and the store", tx_seen);
        }
        Order::TxFirst => {
            send(tx_msg());
            wait_until("the payment in the history and the store", tx_seen);
            send(lock_msg());
            wait_until("the lock at the InstantSend manager", lock_seen);
        }
    }
    (tx, spv)
}

/// Polls the store and the engine for [`SETTLE`] and keeps the first
/// violation of each: a forwarded lock reaches both within milliseconds.
fn settled_violations(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: &Txid,
) -> (Option<String>, Option<String>) {
    let (mut store, mut shown) = (None, None);
    let start = Instant::now();
    while start.elapsed() < SETTLE && (store.is_none() || shown.is_none()) {
        session.live().unwrap().persister.flush(id.0).unwrap();
        let locks = lock_rows(session, id, txid);
        if store.is_none() && locks.contains(&1) {
            store = Some(format!("lock rows (verified column) {locks:?}"));
        }
        if shown.is_none() {
            shown = not_final_violation(&view(engine, session, id, txid));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (store, shown)
}

fn assert_none(order: impl std::fmt::Debug, checks: Vec<(&str, Option<String>)>) {
    let violations: Vec<String> = (checks.into_iter())
        .filter_map(|(check, v)| v.map(|v| format!("{check}: {v}")))
        .collect();
    assert!(
        violations.is_empty(),
        "{order:?}: an unvalidated lock made the payment final:\n{}",
        violations.join("\n")
    );
}

fn run(order: Order) {
    let (_dir, engine, session, id) = engine_wallet();
    let (tx, spv) = deliver_unvalidated(&engine, &session, id, order);
    let txid = tx.txid();
    let (store, shown) = settled_violations(&engine, &session, id, &txid);
    let forwarded = spv.forwarded(&engine, &txid);
    spv.stop(&engine);
    let session = reopen(&engine, session);
    let (reopened_store, reopened) = settled_violations(&engine, &session, id, &txid);
    assert_none(
        order,
        vec![
            ("SPV held the lock back", forwarded),
            ("store, live", store),
            ("engine, live", shown),
            ("store, after reopen", reopened_store),
            ("engine, after reopen", reopened),
        ],
    );
}

#[test]
fn g1_engine_unvalidated_lock_with_its_transaction_is_never_final() {
    run(Order::Together);
}

#[test]
fn g1_engine_unvalidated_lock_before_its_transaction_is_never_final() {
    run(Order::LockFirst);
}

#[test]
fn g1_engine_unvalidated_lock_after_its_transaction_is_never_final() {
    run(Order::TxFirst);
}

/// A lock row a store holds unverified, beside the payment's record, is not
/// final once the engine loads the store.
#[test]
fn g1_engine_stored_unverified_lock_is_not_final_after_a_reopen() {
    let (_dir, engine, session, id) = engine_wallet();
    let (tx, spv) = deliver_unvalidated(&engine, &session, id, Order::TxFirst);
    let txid = tx.txid();
    let forwarded = spv.forwarded(&engine, &txid);
    spv.stop(&engine);
    assert_eq!(forwarded, None, "the seeded row is the only lock");
    let db = session.data_dir().join(WALLET_DB_FILE);
    *session.close_load_wait.lock().unwrap() = Some(super::LOAD_WAIT);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(crate::DashNetwork::Regtest))
            .unwrap()
    );
    let blob =
        bincode::serde::encode_to_vec(unvalidatable_lock(&tx), bincode::config::standard())
            .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO core_instant_locks (wallet_id, txid, islock_blob, verified) \
         VALUES (?1, ?2, ?3, 0)",
        rusqlite::params![id.0.as_slice(), AsRef::<[u8]>::as_ref(&txid), blob],
    )
    .unwrap();
    drop(conn);
    let session = super::open(&engine);
    assert_eq!(lock_rows(&session, id, &txid), [0], "the unverified row stays");
    let (_, reopened) = settled_violations(&engine, &session, id, &txid);
    assert_none("stored unverified", vec![("engine, after reopen", reopened)]);
}

/// The payment is InstantSend-final in the engine: its row is locked, its
/// one coin locked, trusted and selectable, and its amount confirmed balance
/// (key-wallet counts an InstantSend-locked coin as confirmed).
fn assert_final(v: &View, amount: u64, what: &str) {
    let signed = i64::try_from(amount).unwrap();
    assert!(
        matches!(v.rows.as_slice(), [(_, true, a)] if *a == signed),
        "{what}: one InstantSend-locked row for {amount}; got {v:?}"
    );
    let final_coin = Coin {
        listed_locked: true,
        trusted: Some(true),
        selectable: Some(true),
    };
    assert_eq!(v.coins, [final_coin], "{what}: one locked, trusted coin");
    assert_eq!(v.confirmed, amount, "{what}: the payment is confirmed balance");
}

/// Control: the same harness and checks report a lock the wallet takes
/// (handed to it directly, past dash-spv) as final, stored verified, live and
/// after a reopen; so each check above can fail.
#[test]
fn g1_engine_control_a_lock_the_wallet_takes_is_final() {
    let (_dir, engine, session, id) = engine_wallet();
    let (tx, spv) = deliver_unvalidated(&engine, &session, id, Order::TxFirst);
    let txid = tx.txid();
    spv.stop(&engine);
    super::instant_lock(&engine, &session, id, &tx);
    super::wait_stored_locked(&session, id, txid);
    wait_until("a verified lock row", || {
        session.live().unwrap().persister.flush(id.0).unwrap();
        lock_rows(&session, id, &txid) == [1]
    });
    assert_final(&view(&engine, &session, id, &txid), PAID, "live");
    let session = reopen(&engine, session);
    assert_eq!(lock_rows(&session, id, &txid), [1], "after reopen: the verified row");
    assert_final(&view(&engine, &session, id, &txid), PAID, "after reopen");
}

/// A lock the engine's own SPV client validates against a live regtest
/// quorum is final, stored verified, and final again after a reopen.
#[test]
#[ignore = "needs dashd and rust-dashcore's regtest-mn data (DASHD_PATH, DASHD_MN_DATADIR)"]
fn g1_engine_validated_lock_is_final_and_survives_a_reopen() {
    use dash_spv::test_utils::MasternodeTestContext;
    const AMOUNT: u64 = 25_000_000;
    let probe = tokio::runtime::Runtime::new().unwrap();
    let mut mn = probe
        .block_on(MasternodeTestContext::new(false))
        .expect("a dashd masternode network (unset SKIP_DASHD_TESTS)");
    let options = crate::SessionOptions {
        spv_peers: vec![mn.controller_addr.to_string()],
        ..super::offline()
    };
    let (_dir, engine, session, id, _) = super::engine_wallet_on(options);
    engine.block_on(session.start_spv()).unwrap();
    let masternodes_synced = || {
        session.sync_snapshot().is_ok_and(|s| {
            s.running
                && (s.phases.iter())
                    .any(|p| p.phase == crate::sync::SyncPhase::Masternodes && p.done)
        })
    };
    wait_for("masternode sync", QUORUM_BARRIER, masternodes_synced);
    // The pre-generated quorums are in the past; a DKG cycle forms one that
    // signs locks now. A lock that arrives before the client has the new
    // quorum waits pending and is validated once it does.
    mn.mine_dkg_cycle().expect("DKG cycle");
    let address = receive_address(&engine, &session, id);
    let txid = mn
        .controller
        .send_to_address(&address, dashcore::Amount::from_sat(AMOUNT));
    mn.bump_mocktime(30);
    wait_for("the controller's lock", QUORUM_BARRIER, || {
        let locked = (mn.controller)
            .try_rpc_call("getrawtransaction", &[txid.to_string().into(), 1.into()])
            .and_then(|raw| raw.get("instantlock").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        if !locked {
            std::thread::sleep(Duration::from_millis(225));
        }
        locked
    });
    let final_in_engine = || {
        let v = view(&engine, &session, id, &txid);
        v.rows.iter().any(|(_, locked, _)| *locked) && v.coins.iter().all(|c| c.listed_locked)
    };
    wait_for("the payment InstantSend-final in the engine", QUORUM_BARRIER, final_in_engine);
    wait_until("a verified lock row", || {
        session.live().unwrap().persister.flush(id.0).unwrap();
        lock_rows(&session, id, &txid) == [1]
    });
    assert_final(&view(&engine, &session, id, &txid), AMOUNT, "live");
    engine.block_on(session.stop_spv()).unwrap();
    let session = reopen(&engine, session);
    assert_eq!(lock_rows(&session, id, &txid), [1], "after reopen: the verified row");
    assert_final(&view(&engine, &session, id, &txid), AMOUNT, "after reopen");
    drop(mn);
}
