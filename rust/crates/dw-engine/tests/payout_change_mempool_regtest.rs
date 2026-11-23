//! A payout change one installation of a wallet broadcast reaches another
//! installation of the same wallet before it is mined (#361): a ProUpRegTx
//! whose fee the wallet's coins pay is in the second installation's history,
//! unconfirmed, whether that installation was already connected when it was
//! broadcast or connected only after, with its store or restored fresh from
//! the phrase.
//!
//! dashd plays the first installation: it holds the masternode's owner key
//! and the private key of the wallet's first receive address, and builds the
//! ProUpRegTx with `protx update_registrar` funded from that address, as the
//! app funds one from the wallet's coins.
//!
//! Runs only with `DASHD_PATH` set to a dashd binary with `dash-cli` next to
//! it (e.g. `~/.rust-dashcore-test/dashcore-24.0.2/bin/dashd`); skipped
//! otherwise.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::consensus::deserialize;
use dashcore::{Address, Network, PrivateKey, Transaction};
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, NetworkSession,
    SessionOptions, SpvState, TxStatusKind, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::DerivationPath;
use key_wallet::bip32::ExtendedPrivKey;
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// How long an eventual wait (SPV, a scan, a relay) may take on a loaded
/// host.
const LOAD_WAIT: Duration = Duration::from_secs(600);

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
            .args(["-rpcuser=dw", "-rpcpassword=dw", "-rpcwallet=miner"])
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
        let to = self.cli(&["getnewaddress"]);
        self.cli(&["generatetoaddress", &n.to_string(), &to]);
    }

    fn in_mempool(&self, txid: &str) -> bool {
        self.cli(&["getrawmempool"]).contains(txid)
    }

    /// A regular masternode the miner wallet registers and holds every key
    /// of, mined: its ProTx hash.
    fn register_masternode(&self) -> String {
        let new = || self.cli(&["getnewaddress"]);
        let (collateral, owner, voting, payout, fund) = (new(), new(), new(), new(), new());
        self.cli(&["sendtoaddress", &fund, "1001"]);
        self.mine(1);
        let bls: serde_json::Value = serde_json::from_str(&self.cli(&["bls", "generate"])).unwrap();
        let operator = bls["public"].as_str().unwrap();
        let pro_tx_hash = self.cli(&[
            "protx",
            "register_fund",
            &collateral,
            "127.0.0.1:19999",
            &owner,
            operator,
            &voting,
            "0",
            &payout,
            &fund,
        ]);
        self.mine(1);
        pro_tx_hash
    }

    /// A ProUpRegTx of `pro_tx_hash` changing its payout to a fresh address
    /// of the miner, its fee paid from `fee_source`, left in the mempool:
    /// its txid and the new payout.
    fn change_payout(&self, pro_tx_hash: &str, fee_source: &str) -> (String, Address) {
        let payout = self.cli(&["getnewaddress"]);
        let txid = self.cli(&[
            "protx",
            "update_registrar",
            pro_tx_hash,
            "",
            "",
            &payout,
            fee_source,
        ]);
        assert!(self.in_mempool(&txid));
        let payout = Address::from_str(&payout)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        (txid, payout)
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

/// The WIF of the phrase's BIP44 account 0 receive address 0, derived here
/// from the phrase, and that address.
fn first_receive_key() -> (String, String) {
    let seed = dw_vault::mnemonic::derive_secret(PHRASE.as_bytes(), b"", false)
        .unwrap()
        .seed;
    let master = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
    let path = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    let secret = master.derive_priv(&path).unwrap().private_key;
    let key = PrivateKey::new(secret, Network::Regtest);
    let address = Address::p2pkh(&key.public_key(), Network::Regtest);
    (key.to_wif(), address.to_string())
}

/// One installation of the wallet: its own engine and data root, the wallet
/// imported from [`PHRASE`].
struct Installation {
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

impl Installation {
    fn new(root: &Path, node: &Dashd) -> Self {
        let engine = Engine::new(
            EngineConfig {
                data_root: root.to_owned(),
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

    fn start_spv(&self) {
        self.engine.block_on(self.session.start_spv()).unwrap();
        let deadline = Instant::now() + LOAD_WAIT;
        while self.session.spv_state().unwrap() != SpvState::Running {
            assert!(Instant::now() < deadline, "SPV did not start");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn stop_spv(&self) {
        self.engine.block_on(self.session.stop_spv()).unwrap();
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
                "the wallet never scanned to {height}"
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Waits for `txid` in the wallet's history: its status and its
    /// transaction.
    fn wait_in_history(&self, txid: &str) -> (TxStatusKind, Transaction) {
        let deadline = Instant::now() + LOAD_WAIT;
        loop {
            let detail = self
                .engine
                .block_on(self.session.tx_detail(self.wallet, txid.to_owned()));
            if let Ok(detail) = detail {
                let tx = deserialize(&hex::decode(&detail.raw_hex).unwrap()).unwrap();
                return (detail.status.kind, tx);
            }
            assert!(
                Instant::now() < deadline,
                "{txid} never reached the history"
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// `txid` is in the history, unconfirmed, as the ProUpRegTx changing the
    /// payout to `payout`, while dashd still holds it in its mempool.
    fn holds_unmined_change(&self, node: &Dashd, txid: &str, payout: &Address, case: &str) {
        let (status, tx) = self.wait_in_history(txid);
        assert!(node.in_mempool(txid), "{case}: still unmined");
        assert_eq!(status, TxStatusKind::Unconfirmed, "{case}");
        let Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(p)) =
            tx.special_transaction_payload
        else {
            panic!("{case}: {txid} is not a ProUpRegTx");
        };
        assert_eq!(p.script_payout, payout.script_pubkey(), "{case}");
        eprintln!("{case}: {txid} in the history, {status:?}, payout {payout}, unmined");
    }
}

#[test]
fn another_installations_unmined_payout_change_is_in_the_history() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    node.cli(&[
        "createwallet",
        "miner",
        "false",
        "false",
        "",
        "false",
        "false",
        "false",
    ]);
    // DIP3 is active past regtest's height 432.
    node.mine(600);
    let pro_tx_hash = node.register_masternode();

    // The first installation's coins: two on the wallet's first receive
    // address, whose key dashd holds too.
    let b = Installation::new(&dir.path().join("b"), &node);
    let (wif, fee_source) = first_receive_key();
    assert_eq!(
        b.engine
            .block_on(b.session.next_receive_address(b.wallet, None))
            .unwrap()
            .address,
        fee_source
    );
    node.cli(&["importprivkey", &wif, "", "false"]);
    for _ in 0..3 {
        node.cli(&["sendtoaddress", &fee_source, "1"]);
    }
    node.mine(1);
    b.start_spv();
    b.wait_scanned(node.height());

    // B connected when A broadcasts.
    let (txid, payout) = node.change_payout(&pro_tx_hash, &fee_source);
    b.holds_unmined_change(&node, &txid, &payout, "connected at the broadcast");
    node.mine(1);
    let deadline = Instant::now() + LOAD_WAIT;
    while b.wait_in_history(&txid).0 == TxStatusKind::Unconfirmed {
        assert!(Instant::now() < deadline, "{txid} never confirmed");
        std::thread::sleep(Duration::from_millis(250));
    }

    // B connects after A broadcast, with its store.
    b.stop_spv();
    let (txid, payout) = node.change_payout(&pro_tx_hash, &fee_source);
    b.start_spv();
    b.holds_unmined_change(&node, &txid, &payout, "reconnected after the broadcast");
    node.mine(1);
    b.wait_scanned(node.height());

    // A fresh restore from the phrase connects after A broadcast.
    let (txid, payout) = node.change_payout(&pro_tx_hash, &fee_source);
    let c = Installation::new(&dir.path().join("c"), &node);
    c.start_spv();
    c.holds_unmined_change(&node, &txid, &payout, "restored after the broadcast");
}
