//! Coins on CoinJoin addresses past index 99 against a dashd regtest node:
//! a reload restores each into CoinJoin from the store, and a replay of
//! their blocks (a rescan, an abandon's rescan) counts each once. A coin
//! there is spendable after the reload.
//!
//! dashd pays the addresses directly (regtest has no masternodes to mix
//! with); the store and the restore don't depend on how a coin got there.
//! CoinJoin pools hand out addresses up to their gap of 100 past the last
//! used one, so the coins sit at indexes 60, 150 and 240, in that order.
//!
//! One transaction paying a pool's addresses past its gap (a mixing
//! wallet's denomination transaction, restored from its seed) is credited
//! over several passes of its block; every coin of it reloads. A store that
//! kept only the first gap of them recovers with one rescan on this chain,
//! which has no ChainLocks. On a chainlocked chain one rescan is not enough:
//! a block at or below the wallet's last ChainLock finalizes the transaction
//! on its first pass, and key-wallet credits nothing on a later pass of a
//! finalized transaction (rust-dashcore `key-wallet/src/managed_account/
//! managed_core_funds_account.rs:938-942` @55663e43).
//!
//! Runs only with `DASHD_PATH` set to a dashd binary with `dash-cli` next
//! to it (e.g. `~/.rust-dashcore-test/dashcore-24.0.0-rc.3/bin/dashd`);
//! skipped otherwise.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::{Network, OutPoint};
use dw_engine::{
    CoinFilter, CoinInfo, CoinSource, DashNetwork, Engine, EngineConfig, EngineError, EngineEvent,
    EventSink, ImportOptions, NetworkSession, Recipient, RescanFrom, SendFailure, SessionOptions,
    SpvState, WalletBalances, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Wallet;
use key_wallet::account::AccountType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::mnemonic::Mnemonic;
use key_wallet::wallet::ManagedWalletInfo;
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const COIN: u64 = 100_000_000;
const COINJOIN_INDEXES: [u32; 3] = [60, 150, 240];

/// How long an eventual wait (SPV, a scan, a rescan) may take on a loaded
/// host.
const LOAD_WAIT: Duration = Duration::from_secs(600);

/// A regtest dashd with its RPC through `dash-cli`; stopped on drop.
struct Dashd {
    child: Child,
    cli: PathBuf,
    datadir: PathBuf,
    rpc_port: u16,
    p2p_port: u16,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Dashd {
    fn start(dashd: &Path, datadir: &Path) -> Self {
        let (p2p_port, rpc_port) = (free_port(), free_port());
        let log = std::fs::File::create(datadir.join("dashd.stdout")).unwrap();
        let child = Command::new(dashd)
            .args([
                "-regtest".to_owned(),
                format!("-datadir={}", datadir.display()),
                "-server".into(),
                "-printtoconsole".into(),
                "-debug=mempool".into(),
                "-txindex=1".into(),
                "-blockfilterindex=1".into(),
                "-peerblockfilters=1".into(),
                "-peerbloomfilters=1".into(),
                "-listen=1".into(),
                "-listenonion=0".into(),
                format!("-port={p2p_port}"),
                format!("-bind=127.0.0.1:{p2p_port}"),
                format!("-rpcport={rpc_port}"),
                format!("-rpcbind=127.0.0.1:{rpc_port}"),
                "-rpcallowip=127.0.0.1".into(),
                "-rpcuser=dw".into(),
                "-rpcpassword=dw".into(),
                "-fallbackfee=0.00001".into(),
            ])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("dashd starts");
        let node = Self {
            child,
            cli: dashd.with_file_name("dash-cli"),
            datadir: datadir.to_owned(),
            rpc_port,
            p2p_port,
        };
        let deadline = Instant::now() + Duration::from_secs(300);
        while node.try_cli(&["getblockchaininfo"]).is_err() {
            assert!(Instant::now() < deadline, "dashd RPC never answered");
            std::thread::sleep(Duration::from_millis(250));
        }
        node
    }

    fn try_cli(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.cli)
            .arg("-regtest")
            .arg(format!("-datadir={}", self.datadir.display()))
            .arg(format!("-rpcport={}", self.rpc_port))
            .args(["-rpcuser=dw", "-rpcpassword=dw"])
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }

    fn cli(&self, args: &[&str]) -> String {
        self.try_cli(args)
            .unwrap_or_else(|e| panic!("dash-cli {args:?}: {e}"))
    }

    fn height(&self) -> u32 {
        self.cli(&["getblockcount"]).parse().unwrap()
    }

    fn mine(&self, n: u32) {
        let to = self.cli(&["-rpcwallet=miner", "getnewaddress"]);
        self.cli(&["generatetoaddress", &n.to_string(), &to]);
    }

    fn pay(&self, to: &str) {
        self.cli(&["-rpcwallet=miner", "sendtoaddress", to, "1.0"]);
        self.mine(1);
    }

    /// One transaction paying `each` DASH to every address of `to`, mined.
    fn pay_each(&self, to: &[String], each: &str) {
        let outputs: serde_json::Map<String, serde_json::Value> = (to.iter())
            .map(|a| (a.clone(), serde_json::Value::String(each.to_owned())))
            .collect();
        let outputs = serde_json::Value::Object(outputs).to_string();
        self.cli(&["-rpcwallet=miner", "sendmany", "", &outputs]);
        self.mine(1);
    }

    fn in_mempool(&self, txid: &str) -> bool {
        self.cli(&["getrawmempool"]).contains(txid)
    }

    fn confirmations(&self, txid: &str) -> u64 {
        let tx: serde_json::Value =
            serde_json::from_str(&self.cli(&["getrawtransaction", txid, "true"])).unwrap();
        tx["confirmations"].as_u64().unwrap_or(0)
    }
}

impl Drop for Dashd {
    fn drop(&mut self) {
        let _ = self.try_cli(&["stop"]);
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Silent;

impl EventSink for Silent {
    fn emit(&self, _: EngineEvent) {}
}

fn session_options(node: &Dashd) -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec![format!("127.0.0.1:{}", node.p2p_port)],
        no_platform: true,
        ..Default::default()
    }
}

/// The CoinJoin account 0 addresses of [`PHRASE`] at `indexes`, derived
/// here from the phrase, not read from the engine.
fn coinjoin_addresses(indexes: &[u32]) -> Vec<String> {
    receive_addresses(AccountType::CoinJoin { index: 0 }, indexes)
}

/// The external-chain addresses of [`PHRASE`]'s funds account `account` at
/// `indexes`, derived here from the phrase.
fn receive_addresses(account: AccountType, indexes: &[u32]) -> Vec<String> {
    let mnemonic = Mnemonic::from_phrase(PHRASE).unwrap();
    let wallet = Wallet::from_mnemonic(
        mnemonic,
        Network::Regtest,
        WalletAccountCreationOptions::Default,
    )
    .unwrap();
    let info = ManagedWalletInfo::from_wallet(&wallet, 0);
    let mut pool = (info.accounts.funds_account(&account).unwrap())
        .managed_account_type()
        .address_pools()[0]
        .clone();
    let top = *indexes.iter().max().unwrap();
    let have = pool.highest_generated.unwrap_or(0);
    if top > have {
        pool.generate_addresses(top - have, &wallet.key_source_for_account(account), true)
            .unwrap();
    }
    (indexes.iter())
        .map(|i| pool.address_at_index(*i).unwrap().to_string())
        .collect()
}

struct Fixture {
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

impl Fixture {
    fn new(root: &Path, node: &Dashd) -> Self {
        let engine = Engine::new(
            EngineConfig {
                data_root: root.join("data"),
                worker_threads: Some(4),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(Silent),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, session_options(node)))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        let wallet = engine
            .block_on(session.import_wallet(
                Zeroizing::new(PHRASE.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap();
        Self {
            engine,
            session,
            wallet,
        }
    }

    /// Closes the network and opens it again: the wallet reloads from its
    /// store. SPV stays stopped.
    fn reload(&mut self, node: &Dashd) {
        self.engine.block_on(self.session.stop_spv()).unwrap();
        assert!(
            self.engine
                .block_on(self.engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        self.session = self
            .engine
            .block_on(
                self.engine
                    .open_network(DashNetwork::Regtest, session_options(node)),
            )
            .unwrap();
    }

    /// As [`Self::reload`], running `edit` on the wallet's store while the
    /// network is closed.
    fn reload_editing_store(&mut self, node: &Dashd, edit: impl FnOnce(&rusqlite::Connection)) {
        self.engine.block_on(self.session.stop_spv()).unwrap();
        let db = self.session.data_dir().join("wallet.sqlite");
        assert!(
            self.engine
                .block_on(self.engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        edit(&rusqlite::Connection::open(db).unwrap());
        self.session = self
            .engine
            .block_on(
                self.engine
                    .open_network(DashNetwork::Regtest, session_options(node)),
            )
            .unwrap();
    }

    /// The wallet's coin count, total and CoinJoin balance.
    fn holdings(&self) -> (usize, u64, u64) {
        let b = self.balances();
        (self.coins().len(), b.total, b.coinjoin)
    }

    fn start_spv(&self) {
        self.engine.block_on(self.session.start_spv()).unwrap();
        let deadline = Instant::now() + LOAD_WAIT;
        while self.session.spv_state().unwrap() != SpvState::Running {
            assert!(Instant::now() < deadline, "SPV did not start");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_scanned(&self, height: u32) {
        let deadline = Instant::now() + LOAD_WAIT;
        while self
            .session
            .wallet_scan_height(&self.wallet)
            .is_none_or(|h| h < height)
        {
            assert!(
                Instant::now() < deadline,
                "the wallet never scanned to {height} ({:?})",
                self.session.wallet_scan_height(&self.wallet)
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Waits for the running rescan to finish and the wallet to scan back
    /// to `height`.
    fn wait_rescanned(&self, height: u32) {
        let deadline = Instant::now() + LOAD_WAIT;
        while self.session.rescan_progress().unwrap().is_some() {
            assert!(Instant::now() < deadline, "the rescan never finished");
            std::thread::sleep(Duration::from_millis(250));
        }
        self.wait_scanned(height);
    }

    fn balances(&self) -> WalletBalances {
        self.session
            .balances(&self.wallet)
            .unwrap()
            .expect("balances known")
    }

    fn coins(&self) -> Vec<CoinInfo> {
        self.engine
            .block_on(self.session.utxos(
                self.wallet,
                CoinFilter {
                    include_locked: true,
                    fully_mixed_only: false,
                    min_confirmations: None,
                },
            ))
            .unwrap()
    }

    fn coin_at(&self, address: &str) -> OutPoint {
        let coins: Vec<_> = (self.coins().into_iter())
            .filter(|c| c.address == address)
            .collect();
        assert_eq!(coins.len(), 1, "one coin on {address}");
        coins[0].outpoint
    }

    fn receive_address(&self) -> String {
        self.engine
            .block_on(self.session.next_receive_address(self.wallet, None))
            .unwrap()
            .address
    }

    fn spend_grant(&self) -> String {
        self.session
            .vault()
            .authorize(
                GrantPurpose::Spend {
                    max_duffs: 2 * COIN,
                },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    /// A payment of half a DASH to `payee` from exactly `coin`.
    fn prepare_from(
        &self,
        coin: OutPoint,
        payee: &str,
    ) -> (Arc<dw_engine::TxDraft>, Arc<dw_engine::PreparedTx>) {
        let draft = self.session.new_tx_draft(self.wallet).unwrap();
        draft
            .set_recipients(vec![Recipient {
                address: payee.to_owned(),
                amount: COIN / 2,
                subtract_fee_from_amount: false,
                label: None,
                message: None,
            }])
            .unwrap();
        draft.set_source(CoinSource::Outpoints(vec![coin])).unwrap();
        let prepared = self
            .engine
            .block_on(draft.prepare(self.spend_grant()))
            .unwrap();
        (draft, prepared)
    }

    /// The public CoinJoin recovery scan (it raises the lookahead, then
    /// rescans and waits for it), with a deadline. Polls `rescan_progress`
    /// meanwhile, as the app's UI does: only that clears a finished rescan,
    /// which the scan's wait looks for.
    fn recovery_scan(&self) {
        std::thread::scope(|s| {
            let scan = s.spawn(|| {
                self.engine
                    .block_on(self.session.coinjoin_recovery_scan(self.wallet))
            });
            let deadline = Instant::now() + LOAD_WAIT;
            let mut next = Instant::now();
            while !scan.is_finished() {
                let state = format!(
                    "rescan {:?}, scanned to {:?}, SPV {:?}",
                    self.session.rescan_progress().ok().flatten(),
                    self.session.wallet_scan_height(&self.wallet),
                    self.session.spv_state().ok(),
                );
                let now = Instant::now();
                assert!(now < deadline, "the recovery scan never finished: {state}");
                if now >= next {
                    eprintln!("recovery scan running: {state}");
                    next = now + Duration::from_secs(30);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            scan.join().unwrap().unwrap();
        });
    }

    fn assert_counted(&self, total: u64, coinjoin: u64, when: &str) {
        let b = self.balances();
        assert_eq!(
            (b.total, b.coinjoin),
            (total, coinjoin),
            "{when}: total and CoinJoin balance"
        );
    }
}

/// Pushes `prepared` through `send` and mines it, whether dash-spv's
/// broadcast or dashd's own RPC delivers it first.
fn send_and_mine<T: Send>(
    node: &Dashd,
    txid: &str,
    raw: &[u8],
    send: impl FnOnce() -> Result<T, EngineError> + Send,
) {
    let sent = std::thread::scope(|s| {
        let send = s.spawn(send);
        let deadline = Instant::now() + LOAD_WAIT;
        while !node.in_mempool(txid) && !send.is_finished() {
            assert!(Instant::now() < deadline, "{txid} never reached dashd");
            std::thread::sleep(Duration::from_millis(100));
        }
        if !node.in_mempool(txid)
            && let Err(e) = node.try_cli(&["sendrawtransaction", &hex::encode(raw)])
        {
            assert!(e.contains("already"), "{e}");
        }
        node.mine(1);
        send.join().unwrap()
    });
    match sent {
        Ok(_) | Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => {}
        Err(other) => panic!("{other:?}"),
    }
    assert!(node.confirmations(txid) >= 1, "dashd mined {txid}");
}

/// The wallet signs a PSBT spending `coin` whole (less a fee) to dashd's
/// miner wallet, and it is mined. A PSBT, as coin control's CoinJoin rules
/// don't take a coin dashd paid directly.
fn spend_by_psbt(f: &Fixture, node: &Dashd, coin: OutPoint) {
    let fee = 10_000;
    let prev: dashcore::Transaction = dashcore::consensus::encode::deserialize(
        &hex::decode(node.cli(&["getrawtransaction", &coin.txid.to_string()])).unwrap(),
    )
    .unwrap();
    let value = prev.output[coin.vout as usize].value;
    let payee = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let payee_info: serde_json::Value =
        serde_json::from_str(&node.cli(&["-rpcwallet=miner", "getaddressinfo", &payee])).unwrap();
    let tx = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: coin,
            sequence: u32::MAX,
            ..Default::default()
        }],
        output: vec![dashcore::TxOut {
            value: value - fee,
            script_pubkey: dashcore::ScriptBuf::from_bytes(
                hex::decode(payee_info["scriptPubKey"].as_str().unwrap()).unwrap(),
            ),
        }],
        special_transaction_payload: None,
    };
    let unsigned = dw_psbt::create_unsigned(
        tx,
        vec![dw_psbt::InputData {
            prev_tx: prev,
            derivation: None,
        }],
        vec![None],
    )
    .unwrap();
    let signed = f
        .engine
        .block_on(f.session.sign_psbt(f.wallet, unsigned, f.spend_grant()))
        .unwrap();
    let mut complete = signed.clone();
    assert!(
        dw_psbt::finalize(&mut complete),
        "the wallet signed its input"
    );
    let spend = dw_psbt::extract(&complete).unwrap();
    let (raw, txid) = (
        dashcore::consensus::encode::serialize(&spend),
        spend.txid().to_string(),
    );
    send_and_mine(node, &txid, &raw, || {
        f.engine.block_on(f.session.broadcast_psbt(signed))
    });
}

/// A regtest dashd under `dir` with a `miner` wallet holding mature coins,
/// or `None` when `DASHD_PATH` is not set.
fn funded_node(dir: &Path) -> Option<Dashd> {
    let dashd = PathBuf::from(std::env::var_os("DASHD_PATH")?);
    let node_dir = dir.join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    node.cli(&["createwallet", "miner"]);
    node.mine(101);
    Some(node)
}

#[test]
fn coinjoin_coins_past_99_reload_replay_once_and_spend_against_dashd() {
    let dir = dw_testutil::private_tempdir();
    let Some(node) = funded_node(dir.path()) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let mut f = Fixture::new(dir.path(), &node);

    // The BIP44 coin first: the abandon below rescans from its block, so
    // the replay covers every CoinJoin coin's block too.
    let bip44 = f.receive_address();
    node.pay(&bip44);
    let coinjoin = coinjoin_addresses(&COINJOIN_INDEXES);
    for to in &coinjoin {
        node.pay(to);
    }
    f.start_spv();
    f.wait_scanned(node.height());
    f.assert_counted(4 * COIN, 3 * COIN, "first sync");
    let held: Vec<OutPoint> = coinjoin.iter().map(|a| f.coin_at(a)).collect();
    let bip44_coin = f.coin_at(&bip44);

    // The store alone, before SPV runs: every CoinJoin coin restores into
    // CoinJoin.
    f.reload(&node);
    f.assert_counted(4 * COIN, 3 * COIN, "reloaded from the store");
    f.start_spv();
    f.wait_scanned(node.height());
    f.assert_counted(4 * COIN, 3 * COIN, "after the catch-up sync");

    // A replay of every block from genesis.
    f.engine
        .block_on(f.session.rescan(RescanFrom::Genesis))
        .unwrap();
    f.wait_rescanned(node.height());
    f.assert_counted(4 * COIN, 3 * COIN, "after a rescan from genesis");
    for (address, outpoint) in coinjoin.iter().zip(&held) {
        assert_eq!(f.coin_at(address), *outpoint);
    }

    // An abandon's rescan: a payment from the BIP44 coin that dashd refuses
    // (its fee lowered in dashd's eyes), abandoned.
    let payee = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let (draft, prepared) = f.prepare_from(bip44_coin, &payee);
    let txid = prepared.summary().txid.clone();
    node.cli(&[
        "prioritisetransaction",
        &txid,
        &(-10 * COIN as i64).to_string(),
    ]);
    let out = f.engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    assert!(
        matches!(
            &out,
            Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))
        ),
        "{out:?}"
    );
    assert!(!node.in_mempool(&txid));
    f.engine
        .block_on(
            f.session
                .abandon_transaction(f.wallet, txid.parse().unwrap()),
        )
        .unwrap();
    f.wait_rescanned(node.height());
    assert_eq!(f.coin_at(&bip44), bip44_coin, "the abandon released it");
    f.assert_counted(4 * COIN, 3 * COIN, "after the abandon's rescan");

    // Spendable from CoinJoin: the wallet signs the coin at index 150 (a
    // PSBT, as coin control's CoinJoin rules don't take a coin dashd paid
    // directly) and it pays dashd.
    spend_by_psbt(&f, &node, held[1]);
    f.wait_scanned(node.height());
    f.assert_counted(3 * COIN, 2 * COIN, "after spending the coin at 150");

    // And once more from the store.
    f.reload(&node);
    f.assert_counted(3 * COIN, 2 * COIN, "reloaded after the spend");
}

/// Sol r1 M1 through the public CoinJoin recovery scan: coins at CoinJoin
/// 600 and 1,500, which only the recovery scan's lookahead (1,000) finds,
/// the one at 1,500 only through the one at 600. The wallet spends the coin
/// at 600. Reloaded from the store (pool rows 0..99, the scan's lookahead
/// stored), the coin at 1,500 comes back in CoinJoin; a second recovery scan
/// replays the whole history, the spent coin's included, and counts it once.
#[test]
fn a_coin_past_a_spent_bridge_counts_once_through_the_recovery_scan_against_dashd() {
    let dir = dw_testutil::private_tempdir();
    let Some(node) = funded_node(dir.path()) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let mut f = Fixture::new(dir.path(), &node);
    let addrs = coinjoin_addresses(&[600, 1_500]);
    for to in &addrs {
        node.pay(to);
    }
    f.start_spv();
    f.wait_scanned(node.height());
    f.assert_counted(0, 0, "past the default gap");

    f.recovery_scan();
    f.wait_scanned(node.height());
    f.assert_counted(2 * COIN, 2 * COIN, "after the recovery scan");
    let deep = f.coin_at(&addrs[1]);

    spend_by_psbt(&f, &node, f.coin_at(&addrs[0]));
    f.wait_scanned(node.height());
    f.assert_counted(COIN, COIN, "after spending the coin at 600");

    f.reload(&node);
    f.assert_counted(COIN, COIN, "reloaded from the store");
    assert_eq!(f.coin_at(&addrs[1]), deep);

    f.start_spv();
    f.wait_scanned(node.height());
    f.recovery_scan();
    f.wait_scanned(node.height());
    f.assert_counted(COIN, COIN, "after a second recovery scan's replay");
    assert_eq!(f.coin_at(&addrs[1]), deep);
}

/// A mixing wallet's denomination transaction, restored from its seed:
/// one transaction paying CoinJoin indexes 0..489, contiguous. The first
/// pass of its block matches the first 100; the rest are credited as the
/// block is scanned again for the addresses each pass derives. A reload
/// holds every coin, before and after SPV catches up.
#[test]
fn one_transaction_past_the_coinjoin_gap_reloads_whole_against_dashd() {
    one_transaction_past_the_gap_reloads_whole(AccountType::CoinJoin { index: 0 }, 490, 49 * COIN);
}

/// As above on the BIP44 receive chain (gap 30): one payment to 45 of the
/// wallet's addresses.
#[test]
fn one_transaction_past_the_bip44_gap_reloads_whole_against_dashd() {
    one_transaction_past_the_gap_reloads_whole(
        AccountType::Standard {
            index: 0,
            standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
        },
        45,
        0,
    );
}

fn one_transaction_past_the_gap_reloads_whole(account: AccountType, count: u32, coinjoin: u64) {
    let dir = dw_testutil::private_tempdir();
    let Some(node) = funded_node(dir.path()) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let mut f = Fixture::new(dir.path(), &node);
    let indexes: Vec<u32> = (0..count).collect();
    node.pay_each(&receive_addresses(account, &indexes), "0.1");
    f.start_spv();
    f.wait_scanned(node.height());
    let first_sync = f.holdings();
    assert_eq!(
        first_sync,
        (count as usize, u64::from(count) * COIN / 10, coinjoin)
    );

    f.reload(&node);
    let reloaded = f.holdings();
    assert_eq!(reloaded, first_sync, "reloaded from the store");
    f.start_spv();
    f.wait_scanned(node.height());
    assert_eq!(f.holdings(), first_sync, "after the catch-up sync");
}

/// A wallet whose store kept only the first 100 coins of such a transaction
/// (what a build without the fix wrote: every other row is the same) gets
/// all 490 back with one rescan, and a reload keeps them, on a chain without
/// ChainLocks. With the funding block at or below the wallet's last
/// ChainLock, the rescan's first pass finalizes the transaction and the
/// passes after it credit nothing (rust-dashcore `key-wallet/src/
/// managed_account/managed_core_funds_account.rs:938-942` @55663e43).
#[test]
fn a_store_missing_coins_past_the_gap_recovers_with_one_rescan_without_chainlocks_against_dashd() {
    let dir = dw_testutil::private_tempdir();
    let Some(node) = funded_node(dir.path()) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let mut f = Fixture::new(dir.path(), &node);
    let indexes: Vec<u32> = (0..490).collect();
    let addresses = receive_addresses(AccountType::CoinJoin { index: 0 }, &indexes);
    node.pay_each(&addresses, "0.1");
    f.start_spv();
    f.wait_scanned(node.height());
    let whole = (490, 49 * COIN, 49 * COIN);
    assert_eq!(f.holdings(), whole);

    f.reload_editing_store(&node, |db| {
        for address in &addresses[100..] {
            let script = dashcore::Address::from_str(address)
                .unwrap()
                .require_network(Network::Regtest)
                .unwrap()
                .script_pubkey();
            let removed = db
                .execute(
                    "DELETE FROM core_utxos WHERE script = ?1",
                    [script.as_bytes()],
                )
                .unwrap();
            assert_eq!(removed, 1, "one stored coin on {address}");
        }
    });
    assert_eq!(
        f.holdings(),
        (100, 10 * COIN, 10 * COIN),
        "the store keeps 100"
    );

    f.start_spv();
    f.wait_scanned(node.height());
    f.engine
        .block_on(f.session.rescan(RescanFrom::Genesis))
        .unwrap();
    f.wait_rescanned(node.height());
    assert_eq!(f.holdings(), whole, "after one rescan");
    f.reload(&node);
    assert_eq!(f.holdings(), whole, "reloaded after the rescan");
}
