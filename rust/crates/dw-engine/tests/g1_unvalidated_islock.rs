//! An InstantSend lock the wallet never validated never makes a payment
//! final once the engine reopens the wallet: its activity row, its balance
//! bucket and its coin read as an unconfirmed mempool payment, and only a
//! lock stored as verified reads as InstantSend.
//!
//! The payment, its coin and its lock are written to the network's
//! `wallet.sqlite` through the SQLite persister's own write API while the
//! network is closed. That writer stores every lock as verified: a lock
//! stored before the store kept the flag reads back unverified (`verified =
//! 0`), so the row is set to that by hand. Each case then reopens the network
//! through `Engine::open_network`, which loads the wallet from the store as
//! the app does, and reads what the engine shows: the transaction's status
//! and activity row, the wallet's confirmed and pending balance, the coin
//! control row, and what a send without coin control may spend.
//!
//! The positive case (`#[ignore]`) runs against a regtest masternode network
//! (dashd plus rust-dashcore's pre-generated `regtest-mn` data): a payment
//! whose lock the engine's SPV validates against a live quorum is stored with
//! `verified = 1` and is still InstantSend-final after the reopen. The store
//! writes every lock it is handed as verified, so that row means validated
//! only while dash-spv hands the wallet validated locks alone.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::bls_sig_utils::BLSSignature;
use dashcore::{
    Address, InstantLock, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, Witness,
};
use dw_engine::{
    CoinFilter, CoinInfo, CoinSource, DashNetwork, Engine, EngineConfig, EngineEvent, EventSink,
    FeeMode, HistoryFilter, HistoryQuery, HistorySort, ImportOptions, NetworkSession,
    SessionOptions, TxRecord, TxStatus, TxStatusKind, WalletBalances, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::managed_account::transaction_record::{
    OutputDetail, OutputRole, TransactionDirection, TransactionRecord,
};
use key_wallet::transaction_checking::{TransactionContext, TransactionType};
use platform_wallet::changeset::{
    CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
};
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
/// A wallet the masternode network's pre-generated chain never paid.
const FRESH_PHRASE: &str =
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage above";
const AMOUNT: u64 = 25_000_000;

struct Silent;

impl EventSink for Silent {
    fn emit(&self, _: EngineEvent) {}
}

fn offline_options() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    }
}

fn new_engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(4),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap()
}

/// An engine with a phrase imported on regtest, and the wallet's first
/// receive address.
struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    options: SessionOptions,
    wallet: WalletId,
    address: Address,
}

impl Fixture {
    fn new(options: SessionOptions, phrase: &str) -> Self {
        let dir = dw_testutil::private_tempdir();
        let engine = new_engine(&dir.path().join("data"));
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, options.clone()))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        let wallet = engine
            .block_on(session.import_wallet(
                Zeroizing::new(phrase.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    ..ImportOptions::default()
                },
            ))
            .unwrap();
        let address = engine
            .block_on(session.next_receive_address(wallet, None))
            .unwrap()
            .address;
        let address = Address::from_str(&address)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        Self {
            _dir: dir,
            engine,
            session,
            options,
            wallet,
            address,
        }
    }

    fn db(&self) -> PathBuf {
        self.session.data_dir().join("wallet.sqlite")
    }

    /// Closes the network, runs `edit` on its `wallet.sqlite`, and opens it
    /// again: the engine loads the wallet from the store.
    fn reopen_after(&mut self, edit: impl FnOnce(&Path)) {
        let db = self.db();
        assert!(
            self.engine
                .block_on(self.engine.close_network(DashNetwork::Regtest))
                .unwrap(),
            "the network closes"
        );
        edit(&db);
        self.session = self
            .engine
            .block_on(
                self.engine
                    .open_network(DashNetwork::Regtest, self.options.clone()),
            )
            .unwrap();
    }

    fn view(&self, txid: &Txid) -> View {
        let wallet = self.wallet;
        let status = self
            .engine
            .block_on(self.session.tx_detail(wallet, txid.to_string()))
            .unwrap()
            .status;
        let query = HistoryQuery {
            filter: HistoryFilter::default(),
            sort: HistorySort::NewestFirst,
            cursor: None,
            limit: 50,
        };
        let activity: Vec<TxRecord> = (self.engine)
            .block_on(self.session.history_page(wallet, query))
            .unwrap()
            .records
            .into_iter()
            .filter(|r| r.txid == txid.to_string())
            .collect();
        let coins: Vec<CoinInfo> = (self.engine)
            .block_on(self.session.utxos(
                wallet,
                CoinFilter {
                    include_locked: true,
                    fully_mixed_only: false,
                    min_confirmations: None,
                },
            ))
            .unwrap()
            .into_iter()
            .filter(|c| c.outpoint.txid == *txid)
            .collect();
        let auto_spendable = (self.engine)
            .block_on(self.session.max_spendable(
                wallet,
                CoinSource::Any,
                FeeMode::Recommended { target_blocks: 6 },
            ))
            .unwrap();
        View {
            status,
            activity: activity
                .iter()
                .map(|r| (r.status.clone(), r.counts_toward_balance))
                .collect(),
            balances: self.session.balances(&wallet).unwrap().expect("balances"),
            coins: coins
                .iter()
                .map(|c| (c.instant_locked, c.confirmations))
                .collect(),
            auto_spendable,
        }
    }
}

/// What the engine shows of the payment.
#[derive(Debug)]
struct View {
    status: TxStatus,
    /// Its activity rows: status and whether each counts toward the balance.
    activity: Vec<(TxStatus, bool)>,
    balances: WalletBalances,
    /// Its coins: `(instant_locked, confirmations)`.
    coins: Vec<(bool, u32)>,
    /// What a send without coin control may spend.
    auto_spendable: u64,
}

/// The payment is in the wallet and pending: unconfirmed, not
/// InstantSend-locked, its coin neither locked nor confirmed, counted as
/// pending balance only, and not spendable without coin control.
fn assert_not_final(view: &View, what: &str) {
    let pending = TxStatus {
        kind: TxStatusKind::Unconfirmed,
        confirmations: 0,
        instant_locked: false,
        chain_locked: false,
        matures_in: None,
    };
    let violations: Vec<&str> = [
        (view.status != pending, "the transaction status"),
        (
            view.activity != [(pending.clone(), false)],
            "the activity row",
        ),
        (
            (view.balances.confirmed, view.balances.unconfirmed) != (0, AMOUNT),
            "the balance buckets",
        ),
        (view.coins != [(false, 0)], "the coin"),
        (view.auto_spendable != 0, "what a send may spend"),
    ]
    .into_iter()
    .filter_map(|(bad, check)| bad.then_some(check))
    .collect();
    assert!(
        violations.is_empty(),
        "{what}: an unvalidated lock made the payment final ({}): {view:#?}",
        violations.join(", ")
    );
}

/// The payment is InstantSend-final: locked, its coin locked, counted as
/// confirmed balance and spendable without coin control.
fn assert_final(view: &View, what: &str) {
    let locked = TxStatus {
        kind: TxStatusKind::Unconfirmed,
        confirmations: 0,
        instant_locked: true,
        chain_locked: false,
        matures_in: None,
    };
    assert_eq!(view.status, locked, "{what}: the transaction status");
    assert_eq!(view.activity, [(locked, true)], "{what}: the activity row");
    assert_eq!(
        (view.balances.confirmed, view.balances.unconfirmed),
        (AMOUNT, 0),
        "{what}: the balance buckets"
    );
    assert_eq!(view.coins, [(true, 0)], "{what}: the coin");
    assert_eq!(view.auto_spendable, AMOUNT, "{what}: what a send may spend");
}

/// A payment of [`AMOUNT`] to `address` from an outpoint the wallet does not
/// own, and a well-formed lock for it no quorum signed.
fn payment_to(address: &Address) -> (Transaction, InstantLock) {
    let tx = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0x42; 32]), 3),
            script_sig: ScriptBuf::new(),
            sequence: 0xffff_ffff,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: AMOUNT,
            script_pubkey: address.script_pubkey(),
        }],
        special_transaction_payload: None,
    };
    let lock = InstantLock {
        inputs: vec![tx.input[0].previous_output],
        txid: tx.txid(),
        signature: BLSSignature::from([0x11; 96]),
        ..InstantLock::default()
    };
    (tx, lock)
}

fn open_persister(db: &Path) -> SqlitePersister {
    SqlitePersister::open(SqlitePersisterConfig::new(db)).expect("open the store")
}

fn store(db: &Path, wallet: WalletId, core: CoreChangeSet) {
    let persister = open_persister(db);
    let changeset = PlatformWalletChangeSet {
        core: Some(core),
        ..Default::default()
    };
    persister.store(wallet.0, changeset).expect("store");
    persister.flush(wallet.0).expect("flush");
}

/// The payment as the wallet-event adapter stores one that arrived with its
/// lock: the record in InstantSend context, its coin locked, and the lock.
fn store_locked_payment(
    db: &Path,
    wallet: WalletId,
    address: &Address,
    tx: &Transaction,
    lock: &InstantLock,
) {
    let record = TransactionRecord::new(
        tx.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        TransactionContext::InstantSend(lock.clone()),
        TransactionType::Standard,
        TransactionDirection::Incoming,
        Vec::new(),
        vec![OutputDetail {
            index: 0,
            role: OutputRole::Received,
            address: Some(address.clone()),
            value: AMOUNT,
        }],
        AMOUNT as i64,
    );
    let utxo = Utxo {
        outpoint: OutPoint::new(tx.txid(), 0),
        txout: tx.output[0].clone(),
        address: address.clone(),
        height: 0,
        is_coinbase: false,
        is_confirmed: false,
        is_instantlocked: true,
        is_locked: false,
        is_trusted: false,
    };
    store(
        db,
        wallet,
        CoreChangeSet {
            records: vec![record],
            new_utxos: vec![utxo],
            instant_locks_for_non_final_records: [(tx.txid(), lock.clone())].into(),
            ..Default::default()
        },
    );
}

/// The `verified` column of the payment's lock rows.
fn lock_rows(db: &Path, txid: &Txid) -> Vec<i64> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut stmt = conn
        .prepare("SELECT verified FROM core_instant_locks WHERE txid = ?1")
        .unwrap();
    stmt.query_map([txid.as_byte_array().as_slice()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Sets the lock row to what a store written before the verified flag holds
/// after its upgrade: the persister has no write that stores a lock
/// unverified.
fn mark_unverified(db: &Path, txid: &Txid) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let changed = conn
        .execute(
            "UPDATE core_instant_locks SET verified = 0 WHERE txid = ?1",
            [txid.as_byte_array().as_slice()],
        )
        .unwrap();
    assert_eq!(changed, 1, "one lock row");
}

#[test]
fn g1_an_unverified_lock_reads_as_mempool_after_a_reopen() {
    let mut f = Fixture::new(offline_options(), PHRASE);
    let (tx, lock) = payment_to(&f.address);
    let txid = tx.txid();
    let (wallet, address) = (f.wallet, f.address.clone());
    f.reopen_after(|db| {
        store_locked_payment(db, wallet, &address, &tx, &lock);
        assert_eq!(
            lock_rows(db, &txid),
            [1],
            "the writer stores the lock verified"
        );
        mark_unverified(db, &txid);
        assert_eq!(lock_rows(db, &txid), [0]);
    });
    assert_not_final(&f.view(&txid), "after the reopen");

    // The load wrote nothing that makes it final on the next one.
    f.reopen_after(|db| assert_eq!(lock_rows(db, &txid), [0], "the row stays unverified"));
    assert_not_final(&f.view(&txid), "after a second reopen");
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

#[test]
fn g1_a_verified_lock_stays_instantsend_after_a_reopen() {
    let mut f = Fixture::new(offline_options(), PHRASE);
    let (tx, lock) = payment_to(&f.address);
    let txid = tx.txid();
    let (wallet, address) = (f.wallet, f.address.clone());
    f.reopen_after(|db| {
        store_locked_payment(db, wallet, &address, &tx, &lock);
        assert_eq!(lock_rows(db, &txid), [1]);
    });
    assert_final(&f.view(&txid), "after the reopen");

    f.reopen_after(|db| assert_eq!(lock_rows(db, &txid), [1]));
    assert_final(&f.view(&txid), "after a second reopen");
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

#[test]
fn g1_an_unverified_lock_stored_again_as_verified_is_final_after_a_reopen() {
    let mut f = Fixture::new(offline_options(), PHRASE);
    let (tx, lock) = payment_to(&f.address);
    let txid = tx.txid();
    let (wallet, address) = (f.wallet, f.address.clone());
    f.reopen_after(|db| {
        store_locked_payment(db, wallet, &address, &tx, &lock);
        mark_unverified(db, &txid);
    });
    assert_not_final(&f.view(&txid), "before the validation");

    // The lock arrives validated: the adapter stores it, and the record
    // the wallet re-emits with it, again.
    f.reopen_after(|db| {
        store_locked_payment(db, wallet, &address, &tx, &lock);
        assert_eq!(lock_rows(db, &txid), [1], "a lock stored again is verified");
    });
    assert_final(&f.view(&txid), "after the validation and a reopen");
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// How long the masternode network, SPV and a lock may take on a loaded
/// host.
const MN_WAIT: Duration = Duration::from_secs(600);

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < MN_WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A lock the engine's SPV validates against a live regtest quorum.
#[test]
#[ignore = "needs dashd and rust-dashcore's regtest-mn data (DASHD_PATH, DASHD_MN_DATADIR)"]
fn g1_a_validated_lock_is_final_and_survives_a_reopen() {
    use dash_spv::test_utils::MasternodeTestContext;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut mn = runtime
        .block_on(MasternodeTestContext::new(false))
        .expect("a dashd masternode network (unset SKIP_DASHD_TESTS)");
    let options = SessionOptions {
        spv_peers: vec![mn.controller_addr.to_string()],
        ..offline_options()
    };
    let mut f = Fixture::new(options, FRESH_PHRASE);
    let controller_height = |mn: &MasternodeTestContext| {
        mn.controller
            .try_rpc_call("getblockcount", &[])
            .and_then(|v| v.as_u64())
            .expect("getblockcount") as u32
    };
    let caught_up_to = |f: &Fixture, height: u32| {
        let snapshot = f.session.sync_snapshot().unwrap();
        snapshot.caught_up
            && snapshot.tip_height.is_some_and(|h| h >= height)
            && f.session
                .wallet_scan_height(&f.wallet)
                .is_some_and(|h| h >= height)
    };

    f.engine.block_on(f.session.start_spv()).unwrap();
    let height = controller_height(&mn);
    wait_until("SPV caught up", || caught_up_to(&f, height));

    // The pre-generated quorums are in the past; a DKG cycle forms one that
    // signs locks now.
    mn.mine_dkg_cycle().expect("DKG cycle");
    let height = controller_height(&mn);
    wait_until("SPV caught up after the DKG cycle", || {
        caught_up_to(&f, height)
    });

    let txid = mn
        .controller
        .send_to_address(&f.address, dashcore::Amount::from_sat(AMOUNT));
    // One mocktime bump starts the masternodes' signing.
    mn.bump_mocktime(30);
    wait_until("the controller's lock", || {
        mn.controller
            .try_rpc_call("getrawtransaction", &[txid.to_string().into(), 1.into()])
            .and_then(|raw| raw.get("instantlock").and_then(|v| v.as_bool()))
            .unwrap_or(false)
    });
    let db = f.db();
    wait_until("the payment InstantSend-locked in the engine", || {
        (f.engine)
            .block_on(f.session.tx_detail(f.wallet, txid.to_string()))
            .is_ok_and(|d| d.status.instant_locked)
    });
    wait_until("a verified lock row", || lock_rows(&db, &txid) == [1]);
    assert_eq!(
        mn.controller
            .try_rpc_call("getrawtransaction", &[txid.to_string().into(), 1.into()])
            .and_then(|raw| raw.get("confirmations").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        0,
        "the payment is still unmined"
    );
    assert_final(&f.view(&txid), "live");

    f.engine.block_on(f.session.stop_spv()).unwrap();
    f.reopen_after(|db| assert_eq!(lock_rows(db, &txid), [1]));
    assert_final(&f.view(&txid), "after the reopen");
    f.engine.block_on(f.engine.shutdown()).unwrap();
    drop(mn);
}
