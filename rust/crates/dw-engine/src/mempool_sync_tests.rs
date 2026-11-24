//! A payment seen while the wallet is still catching up (PRODUCT backlog
//! item 1): dash-spv hands the wallet a mempool transaction and its
//! InstantSend lock before the filter scan reaches the wallet's birth
//! block (`ClientConfig::mempool_during_sync`). The wallet records it, the
//! history and the balance show it at once, flagged as catching up, and the
//! block the scan reaches later confirms it without counting it twice,
//! across a restart too. Driven through the production wallet manager, as
//! dash-spv drives it; its events feed the hub, as SPV's event monitor does.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, Block, BlockHash, InstantLock, OutPoint, Transaction, TxIn, TxOut, Txid};
use platform_wallet::PlatformWalletInfo;
use platform_wallet::key_wallet_manager::{WalletEvent, WalletInterface};

use crate::history::{HistoryFilter, HistoryQuery, HistorySort, TxStatusKind};
use crate::session::NetworkSession;
use crate::{DashNetwork, Engine, WalletBalances, WalletId};

#[path = "crash_send_tests.rs"]
mod crash_send_tests;
#[path = "dismissed_spend_disk_tests.rs"]
mod dismissed_spend_disk_tests;
#[path = "h03_kill9_core_tests.rs"]
mod h03_kill9_core_tests;
#[cfg(unix)]
#[path = "kill9_abandon_tests.rs"]
mod kill9_abandon_tests;
#[path = "own_spend_balance_tests.rs"]
mod own_spend_balance_tests;
#[path = "own_spend_disk_tests.rs"]
mod own_spend_disk_tests;
#[path = "removed_payment_way_out_tests.rs"]
mod removed_payment_way_out_tests;
#[path = "signed_coin_pin_tests.rs"]
mod signed_coin_pin_tests;

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const BIRTH: u32 = 500;
/// How long [`reopen`]'s close waits for a wallet load in flight (#399).
const LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(180);
const PAID: u64 = 25 * 100_000_000;
const CHANGE: u64 = 10 * 100_000_000;

/// Keeps every event the engine sends its host.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<crate::EngineEvent>>);

impl crate::EventSink for Recorder {
    fn emit(&self, event: crate::EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    /// Waits up to 10 s (the pump sends at most 4 a second) for an event
    /// `want` accepts, recorded after the first `from`.
    fn wait_for(&self, from: usize, want: impl Fn(&crate::EngineEvent) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if self.0.lock().unwrap().iter().skip(from).any(&want) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// Unreachable SPV peer, DAPI and quorum service: no network.
fn offline() -> crate::SessionOptions {
    crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap()
}

/// A real engine and offline regtest session with one wallet born at
/// [`BIRTH`] whose scan has not started, and what the engine told its host.
fn engine_wallet_recorded() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Arc<Recorder>,
) {
    use crate::EngineConfig;
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::clone(&recorder) as Arc<dyn crate::EventSink>,
    )
    .unwrap();
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    (dir, engine, session, id, recorder)
}

fn engine_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>, WalletId) {
    let (dir, engine, session, id, _) = engine_wallet_recorded();
    (dir, engine, session, id)
}

/// Closes the network and opens it again: the wallet comes back from its
/// store. The close waits out a wallet load in flight as the app's does,
/// and longer: once SPV runs, the Platform start opens the shielded store
/// as a load, which takes up to minutes under load (#399).
fn reopen(engine: &Engine, session: Arc<NetworkSession>) -> Arc<NetworkSession> {
    *session.close_load_wait.lock().unwrap() = Some(LOAD_WAIT);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    open(engine)
}

/// Runs `op` on the session's wallet manager as dash-spv does, then hands
/// the hub the events it emitted and waits for the store to have them.
fn deliver<F>(engine: &Engine, session: &NetworkSession, id: WalletId, op: F)
where
    F: for<'a> FnOnce(
            &'a mut platform_wallet::key_wallet_manager::WalletManager<PlatformWalletInfo>,
        ) -> futures_util::future::BoxFuture<'a, ()>
        + Send
        + 'static,
{
    use dash_spv::EventHandler;
    use platform_wallet::changeset::PlatformWalletPersistence;
    let manager = session.manager().unwrap();
    let events: Vec<WalletEvent> = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut rx = wm.subscribe_events();
        op(&mut wm).await;
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    });
    for event in &events {
        session.hub.on_wallet_event(event);
    }
    session.live().unwrap().persister.flush(id.0).unwrap();
}

/// Waits up to 10 s for the wallet store to hold `txid`'s coins
/// InstantSend-locked: the persister hears wallet events through its own
/// adapter task.
fn wait_stored_locked(session: &NetworkSession, id: WalletId, txid: Txid) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let store = Arc::clone(&session.live().unwrap().store);
    while std::time::Instant::now() < deadline {
        let stored = store.load().unwrap();
        let locked = stored.wallets.get(&id.0).is_some_and(|w| {
            (w.wallet_info.accounts.all_funding_accounts().into_iter())
                .flat_map(|a| a.utxos.iter())
                .any(|(o, u)| o.txid == txid && u.is_instantlocked)
        });
        if locked {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the store never held {txid}'s coins InstantSend-locked");
}

fn receive_address(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> Address {
    let info = engine
        .block_on(session.current_receive_address(id))
        .unwrap();
    info.address
        .parse::<Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked()
}

/// A payment of [`PAID`] to `to` from a coin the wallet does not own.
fn payment(to: &Address) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: PAID,
            script_pubkey: to.script_pubkey(),
        }],
        special_transaction_payload: None,
    }
}

fn block_with(txs: Vec<Transaction>) -> Block {
    let mut block = Block {
        header: dashcore::blockdata::block::Header {
            version: dashcore::blockdata::block::Version::from_consensus(1),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: dashcore::TxMerkleNode::all_zeros(),
            time: 1_793_598_712,
            bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
            nonce: 0,
        },
        txdata: txs,
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    block
}

fn mempool(engine: &Engine, session: &NetworkSession, id: WalletId, tx: &Transaction) {
    let tx = tx.clone();
    deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            let result = wm.process_mempool_transaction(&tx, None).await;
            assert!(result.is_relevant, "the payment is the wallet's");
        })
    });
}

fn instant_lock(engine: &Engine, session: &NetworkSession, id: WalletId, tx: &Transaction) {
    let tx = tx.clone();
    deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            let lock = InstantLock {
                txid: tx.txid(),
                ..InstantLock::default()
            };
            wm.process_mempool_instant_send_lock(&tx, lock).await;
        })
    });
}

/// The scan reaches `height`, whose block holds `txs`, and commits its
/// checkpoint there.
fn mined(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    height: u32,
    txs: Vec<Transaction>,
) {
    deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            let block = block_with(txs);
            wm.process_block_for_wallets(
                &block,
                block.block_hash(),
                height,
                &BTreeSet::from([id.0]),
            )
            .await;
            wm.update_wallet_synced_height(&id.0, height);
        })
    });
}

fn balances(session: &NetworkSession, id: WalletId) -> WalletBalances {
    session
        .balances(&id)
        .unwrap()
        .expect("the wallet's state is read at its load")
}

/// `(txid, status kind, instant locked, amount)` of each history row.
fn rows(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> Vec<(String, TxStatusKind, bool, i64)> {
    let page = engine
        .block_on(session.history_page(
            id,
            HistoryQuery {
                filter: HistoryFilter::default(),
                sort: HistorySort::NewestFirst,
                cursor: None,
                limit: 100,
            },
        ))
        .unwrap();
    page.records
        .into_iter()
        .map(|r| (r.txid, r.status.kind, r.status.instant_locked, r.amount))
        .collect()
}

#[test]
fn the_engine_asks_dash_spv_for_mempool_relay_while_syncing() {
    let (_dir, _engine, session, _id) = engine_wallet();
    assert!(session.spv_config().unwrap().mempool_during_sync);
}

/// Before the scan starts the balance is what the wallet knows, flagged as
/// catching up, not unknown.
#[test]
fn a_wallet_catching_up_has_a_balance() {
    let (_dir, _engine, session, id) = engine_wallet();
    let b = balances(&session, id);
    assert!(b.catching_up);
    assert_eq!(b.total, 0);
    let info = session.wallet_info(&id).unwrap();
    assert_eq!(info.balances, Some(b));
}

/// A payment seen in the mempool mid-sync shows in the history and the
/// balance at once; its InstantSend lock marks it; the block the scan
/// reaches later confirms the same row and counts it once.
#[test]
fn a_payment_seen_mid_sync_shows_at_once_and_counts_once() {
    let (_dir, engine, session, id, recorder) = engine_wallet_recorded();
    let to = receive_address(&engine, &session, id);
    let tx = payment(&to);
    let txid = tx.txid().to_string();
    let from = recorder.len();

    mempool(&engine, &session, id, &tx);
    let b = balances(&session, id);
    assert!(b.catching_up, "the scan has not reached the birth block");
    assert_eq!(b.total, PAID);
    assert_eq!(
        rows(&engine, &session, id),
        [(txid.clone(), TxStatusKind::Unconfirmed, false, PAID as i64)]
    );
    assert!(
        recorder.wait_for(from, |e| matches!(
            e,
            crate::EngineEvent::Balances { wallet_id, balances: Some(b), .. }
                if *wallet_id == id && b.catching_up && b.total == PAID
        )),
        "Balances is sent with the payment, catching up"
    );
    assert!(
        recorder.wait_for(from, |e| matches!(
            e,
            crate::EngineEvent::HistoryChanged { wallet_id, txids, .. }
                if *wallet_id == id && txids.contains(&tx.txid())
        )),
        "HistoryChanged names the payment"
    );

    instant_lock(&engine, &session, id, &tx);
    assert_eq!(
        rows(&engine, &session, id),
        [(txid.clone(), TxStatusKind::Unconfirmed, true, PAID as i64)]
    );
    let b = balances(&session, id);
    assert_eq!((b.total, b.confirmed), (PAID, PAID), "InstantSend-locked");

    // The same payment seen again (a second peer, or the relay turned on
    // again after a reconnect) changes nothing.
    mempool(&engine, &session, id, &tx);
    assert_eq!(rows(&engine, &session, id).len(), 1);
    assert_eq!(balances(&session, id).total, PAID);

    let flipped_from = recorder.len();
    mined(&engine, &session, id, BIRTH + 10, vec![tx.clone()]);
    assert!(
        recorder.wait_for(flipped_from, |e| matches!(
            e,
            crate::EngineEvent::Balances { wallet_id, balances: Some(b), .. }
                if *wallet_id == id && !b.catching_up && b.total == PAID
        )),
        "Balances is sent when the scan passes the birth block"
    );
    let b = balances(&session, id);
    assert!(!b.catching_up, "the scan passed the birth block");
    assert_eq!((b.total, b.confirmed), (PAID, PAID), "counted once");
    let after = rows(&engine, &session, id);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0].0, txid);
    assert_ne!(after[0].1, TxStatusKind::Unconfirmed);
    assert_eq!(after[0].3, PAID as i64);
}

/// A restart mid-sync keeps the payment once, and so does the relay seeing
/// it again and the block confirming it after the restart.
#[test]
fn a_restart_mid_sync_keeps_the_payment_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = receive_address(&engine, &session, id);
    let tx = payment(&to);
    let txid = tx.txid().to_string();
    mempool(&engine, &session, id, &tx);
    instant_lock(&engine, &session, id, &tx);
    wait_stored_locked(&session, id, tx.txid());

    let session = reopen(&engine, session);
    let b = balances(&session, id);
    assert!(b.catching_up);
    assert_eq!(b.total, PAID);
    let kept: Vec<_> = (rows(&engine, &session, id).into_iter())
        .map(|(txid, kind, _, amount)| (txid, kind, amount))
        .collect();
    assert_eq!(
        kept,
        [(txid.clone(), TxStatusKind::Unconfirmed, PAID as i64)]
    );

    mempool(&engine, &session, id, &tx);
    assert_eq!(rows(&engine, &session, id).len(), 1);
    assert_eq!(balances(&session, id).total, PAID);

    mined(&engine, &session, id, BIRTH + 10, vec![tx]);
    let b = balances(&session, id);
    assert!(!b.catching_up);
    assert_eq!((b.total, b.confirmed), (PAID, PAID));
    assert_eq!(rows(&engine, &session, id).len(), 1);
}

/// A transaction spending `coin` to `outputs`.
fn spend_of(coin: OutPoint, outputs: Vec<(dashcore::ScriptBuf, u64)>) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: coin,
            ..Default::default()
        }],
        output: (outputs.into_iter())
            .map(|(script_pubkey, value)| TxOut {
                value,
                script_pubkey,
            })
            .collect(),
        special_transaction_payload: None,
    }
}

/// `tx` with its one input signed by the wallet's key for `owner`, the
/// address the coin it spends pays, as another device of the wallet signs.
fn signed_by_wallet(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    owner: &Address,
    tx: Transaction,
) -> Transaction {
    signed_by_wallet_each(engine, session, id, std::slice::from_ref(owner), tx)
}

/// `tx` with each input signed by the wallet's key for the address of
/// `owners` at its index, as another device of the wallet signs.
fn signed_by_wallet_each(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    owners: &[Address],
    tx: Transaction,
) -> Transaction {
    let phrase = std::str::from_utf8(PHRASE).unwrap();
    signed_by_phrase_each(engine, session, id, phrase, owners, tx)
}

/// [`signed_by_wallet_each`] with the keys of `phrase`, the device that
/// holds wallet `id`'s secret (a watch-only wallet's own is elsewhere).
fn signed_by_phrase_each(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    phrase: &str,
    owners: &[Address],
    tx: Transaction,
) -> Transaction {
    use key_wallet::wallet::initialization::WalletAccountCreationOptions;
    use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionSigner;
    let manager = session.manager().unwrap();
    let owners = owners.to_vec();
    let phrase = phrase.to_owned();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let paths: std::collections::HashMap<Address, _> = {
            let wm = wm.read().await;
            let info = wm.get_wallet_info(&id.0).unwrap();
            (owners.iter())
                .map(|o| {
                    let path = crate::coins::own_address_info(&info.core_wallet, o)
                        .unwrap()
                        .path;
                    (o.clone(), path)
                })
                .collect()
        };
        let device = key_wallet::Wallet::from_mnemonic(
            key_wallet::Mnemonic::from_phrase(&phrase).unwrap(),
            key_wallet::Network::Regtest,
            WalletAccountCreationOptions::Default,
        )
        .unwrap();
        let coins = (tx.input.iter().zip(owners))
            .map(|(input, owner)| {
                key_wallet::Utxo::new(
                    input.previous_output,
                    TxOut {
                        value: PAID,
                        script_pubkey: owner.script_pubkey(),
                    },
                    owner,
                    0,
                    false,
                )
            })
            .collect();
        device
            .sign_tx(tx, coins, move |a| paths.get(&a).cloned())
            .await
            .unwrap()
    })
}

/// A script no address of the wallet's pays.
fn elsewhere() -> dashcore::ScriptBuf {
    dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array([3; 20]))
}

/// Mid-sync, an unconfirmed spend of the wallet's coin is beaten by a
/// rival the scan then finds in a block: the loser's row goes and the
/// coins it made do not count. (key-wallet records no row for the winner,
/// which pays no address of the wallet.)
#[test]
fn a_conflicting_spend_seen_mid_sync_is_dropped_when_its_rival_is_mined() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = receive_address(&engine, &session, id);
    let funding = payment(&to);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let coin = OutPoint::new(funding.txid(), 0);
    let next = receive_address(&engine, &session, id);
    let loser = signed_by_wallet(
        &engine,
        &session,
        id,
        &to,
        spend_of(coin, vec![(next.script_pubkey(), PAID - 1_000)]),
    );
    mempool(&engine, &session, id, &loser);
    assert!(balances(&session, id).catching_up);
    assert_eq!(rows(&engine, &session, id).len(), 2);

    let winner = spend_of(coin, vec![(elsewhere(), PAID - 2_000)]);
    mined(&engine, &session, id, BIRTH - 10, vec![winner]);

    let ids: Vec<String> = rows(&engine, &session, id)
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert!(
        !ids.contains(&loser.txid().to_string()),
        "the beaten spend is dropped: {ids:?}"
    );
    let b = balances(&session, id);
    assert!(b.catching_up);
    assert_eq!(b.total, 0, "the coin is spent once");
}

/// A coin the wallet's scan has not found yet, paying [`PAID`] to its
/// receive address, and a spend of it to someone else that pays [`CHANGE`]
/// back to the wallet. Returns the coin's address, its funding and the spend.
fn funding_and_spend(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (Address, Transaction, Transaction) {
    let to = receive_address(engine, session, id);
    let funding = payment(&to);
    let coin = OutPoint::new(funding.txid(), 0);
    let change_address = engine
        .block_on(session.next_receive_address(id, None))
        .unwrap()
        .address
        .parse::<Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked();
    let spend = spend_of(
        coin,
        vec![
            (elsewhere(), PAID - CHANGE - 1_000),
            (change_address.script_pubkey(), CHANGE),
        ],
    );
    (to, funding, spend)
}

/// Mid-sync, the wallet sees an unconfirmed spend of a coin its scan has
/// not found yet, paying change to it (another device spent it). When the
/// scan reaches the coin's block the coin is not credited as unspent, and
/// when it reaches the spend's block only the change counts.
#[test]
fn a_spend_of_a_coin_not_scanned_yet_never_overstates_the_balance() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to, funding, spend) = funding_and_spend(&engine, &session, id);
    let spend = signed_by_wallet(&engine, &session, id, &to, spend);

    mempool(&engine, &session, id, &spend);
    assert_eq!(balances(&session, id).total, CHANGE);

    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    let b = balances(&session, id);
    assert!(b.catching_up);
    assert_eq!(b.total, CHANGE, "the spent coin is not credited");

    mined(&engine, &session, id, BIRTH + 1, vec![spend]);
    let b = balances(&session, id);
    assert!(!b.catching_up);
    assert_eq!((b.total, b.confirmed), (CHANGE, CHANGE));
    assert_eq!(rows(&engine, &session, id).len(), 2);
}

/// Mid-sync, a peer announces a spend of a coin the scan has not found yet
/// that its owner never signed. When the scan finds the coin, the claim is
/// dropped with its row and the coin counts, across a restart too.
#[test]
fn an_unsigned_spend_of_a_coin_not_scanned_yet_does_not_hide_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (_, funding, spend) = funding_and_spend(&engine, &session, id);

    mempool(&engine, &session, id, &spend);
    assert_eq!(balances(&session, id).total, CHANGE);

    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let b = balances(&session, id);
    assert!(b.catching_up);
    assert_eq!(b.total, PAID, "the coin is credited");
    let ids: Vec<String> = rows(&engine, &session, id)
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(ids, [funding.txid().to_string()], "the claim's row is gone");

    let session = reopen(&engine, session);
    assert_eq!(balances(&session, id).total, PAID);
    assert_eq!(rows(&engine, &session, id).len(), 1);
}

/// A rescan from the birth height takes the wallet back to catching up at
/// once: the balance read says so and a `Balances` event marks it, before
/// the scan resumes.
#[test]
fn a_rescan_from_the_birth_height_marks_the_balance_catching_up() {
    let (_dir, engine, session, id, recorder) = engine_wallet_recorded();
    let to = receive_address(&engine, &session, id);
    mined(&engine, &session, id, BIRTH + 10, vec![payment(&to)]);
    assert!(!balances(&session, id).catching_up);

    let from = recorder.len();
    let s = Arc::clone(&session);
    engine
        .block_on(
            session.on_runtime(async move { s.start_rescan(vec![id], BIRTH, |_| BIRTH).await }),
        )
        .unwrap();
    let b = balances(&session, id);
    assert!(b.catching_up, "the checkpoint is below the birth block");
    assert_eq!(b.total, PAID);
    assert!(
        recorder.wait_for(from, |e| matches!(
            e,
            crate::EngineEvent::Balances { wallet_id, balances: Some(b), .. }
                if *wallet_id == id && b.catching_up
        )),
        "Balances is sent when the rescan rewinds below the birth block"
    );
}

/// A chain view (`history_page`, `tx_detail`, the notices) reads the history
/// store's own entries, not a copy of the wallet's whole history per read
/// (H-06: on a 40k wallet the copy cost more than the page).
#[test]
fn a_chain_view_reads_the_history_store_in_place() {
    let (_dir, engine, session, id) = engine_wallet();
    let manager = session.manager().unwrap();
    session.hub.history.with_wallet(id, |_| ());
    let store = session
        .hub
        .history
        .read(&id, |txs| std::ptr::from_ref(txs) as usize);
    let viewed = engine
        .block_on(session.with_view(&manager, id, |view| {
            std::ptr::from_ref(view.history) as usize
        }))
        .unwrap();
    assert_eq!(viewed, store);
}
