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
//! A wallet the change only pays records it too, with nothing of its own
//! in it. When the network forgets such a change, Tools ▸ Repair ▸ Rescan
//! keeps it, and a restore from the phrase drops it, in a new folder or
//! after removing the wallet; a block spending its input drops it. Remove
//! unconfirmed transactions abandons a live one, and it stays abandoned
//! while the network still has it.
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
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, RescanFrom, SessionOptions, SpvState, TxStatusKind, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::DerivationPath;
use key_wallet::bip32::ExtendedPrivKey;
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A second wallet, the one a payout change pays.
const PAYOUT_PHRASE: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

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
        Self::start_on(dashd, datadir, (free_port(), free_port()), &[])
    }

    fn start_on(
        dashd: &Path,
        datadir: &Path,
        (p2p_port, rpc_port): (u16, u16),
        extra: &[&str],
    ) -> Self {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(datadir.join("dashd.stdout"))
            .unwrap();
        let child = Command::new(dashd)
            .args(extra.iter().map(|a| a.to_string()))
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

    /// Restarts dashd on the same ports with an empty mempool, its wallet
    /// broadcasting nothing, and abandons `txid` there: the network no
    /// longer has it.
    fn forget(&mut self, dashd: &Path, txid: &str) {
        self.stop();
        let _ = std::fs::remove_file(self.datadir.join("regtest").join("mempool.dat"));
        let ports = (self.p2p_port, self.rpc_port);
        let flags = ["-persistmempool=0", "-walletbroadcast=0"];
        let restarted = Self::start_on(dashd, &self.datadir.clone(), ports, &flags);
        // Dropping the stopped one would stop the restarted one on its ports.
        std::mem::forget(std::mem::replace(self, restarted));
        self.cli(&["loadwallet", "miner"]);
        assert!(!self.in_mempool(txid));
        self.cli(&["abandontransaction", txid]);
    }

    /// A transaction of the miner spending `outpoint`, mined.
    fn spend_and_mine(&self, outpoint: &dashcore::OutPoint) {
        let (txid, vout) = (outpoint.txid.to_string(), outpoint.vout);
        let out: serde_json::Value =
            serde_json::from_str(&self.cli(&["gettxout", &txid, &vout.to_string()])).unwrap();
        let value = out["value"].as_f64().unwrap() - 0.001;
        let inputs = format!(r#"[{{"txid":"{txid}","vout":{vout}}}]"#);
        let outputs = format!(r#"{{"{}":{value:.8}}}"#, self.cli(&["getnewaddress"]));
        let raw = self.cli(&["createrawtransaction", &inputs, &outputs]);
        let signed: serde_json::Value =
            serde_json::from_str(&self.cli(&["signrawtransactionwithwallet", &raw])).unwrap();
        self.cli(&["sendrawtransaction", signed["hex"].as_str().unwrap()]);
        self.mine(1);
    }

    fn stop(&mut self) {
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
        self.change_payout_to(pro_tx_hash, &self.cli(&["getnewaddress"]), fee_source)
    }

    /// A ProUpRegTx of `pro_tx_hash` changing its payout to `payout`, its
    /// fee paid from `fee_source`, left in the mempool.
    fn change_payout_to(
        &self,
        pro_tx_hash: &str,
        payout: &str,
        fee_source: &str,
    ) -> (String, Address) {
        let txid = self.cli(&[
            "protx",
            "update_registrar",
            pro_tx_hash,
            "",
            "",
            payout,
            fee_source,
        ]);
        assert!(self.in_mempool(&txid));
        let payout = Address::from_str(payout)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        (txid, payout)
    }
}

impl Drop for Dashd {
    fn drop(&mut self) {
        self.stop();
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

fn import(engine: &Engine, session: &Arc<NetworkSession>, phrase: &str) -> WalletId {
    engine
        .block_on(session.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap()
}

/// One installation of a wallet: its own engine and data root, the wallet
/// imported from a phrase.
struct Installation {
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

impl Installation {
    fn new(root: &Path, node: &Dashd) -> Self {
        Self::of(PHRASE, root, node)
    }

    fn of(phrase: &str, root: &Path, node: &Dashd) -> Self {
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
        let wallet = import(&engine, &session, phrase);
        Self {
            engine,
            session,
            wallet,
        }
    }

    /// Removes the wallet and imports `phrase` again in this installation's
    /// data folder.
    fn remove_and_restore(&mut self, phrase: &str) {
        let id = self.wallet;
        let wipe =
            (self.engine)
                .block_on(self.session.vault_op(move |v| {
                    v.authorize(GrantPurpose::Wipe, Some(&id.0), Credential::None)
                }))
                .unwrap();
        (self.engine)
            .block_on(self.session.remove_wallet(id, wipe.id))
            .unwrap();
        self.wallet = import(&self.engine, &self.session, phrase);
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
            match detail {
                Ok(detail) => {
                    let tx = deserialize(&hex::decode(&detail.raw_hex).unwrap()).unwrap();
                    return (detail.status.kind, tx);
                }
                Err(EngineError::TxNotFound(_)) => {}
                Err(e) => panic!("{txid}: {e:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "{txid} never reached the history"
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// The status of `txid` in the history now, `None` when it has none.
    fn status_now(&self, txid: &str) -> Option<TxStatusKind> {
        match (self.engine).block_on(self.session.tx_detail(self.wallet, txid.to_owned())) {
            Ok(d) => Some(d.status.kind),
            Err(EngineError::TxNotFound(_)) => None,
            Err(e) => panic!("{txid}: {e:?}"),
        }
    }

    /// The status of `txid` once it is other than unconfirmed, or after
    /// `wait`.
    fn status_after(&self, txid: &str, wait: Duration) -> Option<TxStatusKind> {
        let deadline = Instant::now() + wait;
        loop {
            let status = self.status_now(txid);
            if Instant::now() >= deadline
                || !matches!(status, None | Some(TxStatusKind::Unconfirmed))
            {
                return status;
            }
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
    let mut status = b.wait_in_history(&txid).0;
    while status == TxStatusKind::Unconfirmed {
        assert!(Instant::now() < deadline, "{txid} never confirmed");
        std::thread::sleep(Duration::from_millis(250));
        status = b.wait_in_history(&txid).0;
    }
    assert!(
        matches!(status, TxStatusKind::Confirming | TxStatusKind::Confirmed),
        "{txid} after mining: {status:?}"
    );

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

/// A wallet the change only pays: what a restore, a rescan, a block and
/// Remove unconfirmed transactions do to the change, live and forgotten.
#[test]
fn a_change_only_the_payout_wallet_records_and_its_ways_out() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let mut node = Dashd::start(&dashd, &node_dir);
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
    node.mine(600);
    let pro_tx_hash = node.register_masternode();
    let fee_source = node.cli(&["getnewaddress"]);
    for _ in 0..2 {
        node.cli(&["sendtoaddress", &fee_source, "1"]);
    }
    node.mine(1);

    // The payout wallet: none of the change's coins are its own.
    let mut p = Installation::of(PAYOUT_PHRASE, &dir.path().join("p"), &node);
    let to = (p.engine)
        .block_on(p.session.next_receive_address(p.wallet, None))
        .unwrap()
        .address;
    p.start_spv();
    p.wait_scanned(node.height());

    // A live change, then mined.
    let (txid, payout) = node.change_payout_to(&pro_tx_hash, &to, &fee_source);
    p.holds_unmined_change(&node, &txid, &payout, "the payout wallet");
    node.mine(1);
    p.wait_scanned(node.height());
    let mined = p.status_after(&txid, Duration::from_secs(60));
    eprintln!("mined: the payout wallet reads {mined:?}");
    assert!(
        matches!(
            mined,
            Some(TxStatusKind::Confirming | TxStatusKind::Confirmed)
        ),
        "the payout wallet sees the change mined: {mined:?}"
    );

    // A live change: a restore sees it again; Remove unconfirmed abandons
    // it while the network still has it.
    let (txid, payout) = node.change_payout_to(&pro_tx_hash, &to, &fee_source);
    p.holds_unmined_change(&node, &txid, &payout, "the payout wallet, again");
    let q = Installation::of(PAYOUT_PHRASE, &dir.path().join("q"), &node);
    q.start_spv();
    q.holds_unmined_change(&node, &txid, &payout, "restored while live");
    let held = Installation::of(PAYOUT_PHRASE, &dir.path().join("held"), &node);
    held.start_spv();
    held.holds_unmined_change(&node, &txid, &payout, "another installation");
    held.stop_spv();
    let dropped = (q.engine)
        .block_on(q.session.drop_unconfirmed(Some(q.wallet)))
        .unwrap();
    assert!(node.in_mempool(&txid));
    let live_dropped = q.status_now(&txid);
    q.stop_spv();
    q.start_spv();
    q.wait_scanned(node.height());
    let live_reconnected = q.status_after(&txid, Duration::from_secs(30));
    eprintln!(
        "remove unconfirmed while live: {dropped} dropped, {live_dropped:?}, after a reconnect {live_reconnected:?}"
    );
    q.stop_spv();

    // The change never mines: the network forgets it.
    p.stop_spv();
    let (_, change) = p.wait_in_history(&txid);
    node.forget(&dashd, &txid);
    node.mine(1);
    p.start_spv();
    p.wait_scanned(node.height());
    std::thread::sleep(Duration::from_secs(30));
    let forgotten = p.status_now(&txid);

    // Tools ▸ Repair ▸ Rescan from the wallet's birth.
    (p.engine)
        .block_on(p.session.rescan(RescanFrom::WalletBirth))
        .unwrap();
    let deadline = Instant::now() + LOAD_WAIT;
    while p.session.rescan_progress().unwrap().is_some() {
        assert!(Instant::now() < deadline, "the rescan never finished");
        std::thread::sleep(Duration::from_millis(250));
    }
    p.wait_scanned(node.height());
    let rescanned = p.status_now(&txid);

    // A restore from the phrase.
    let r = Installation::of(PAYOUT_PHRASE, &dir.path().join("r"), &node);
    r.start_spv();
    r.wait_scanned(node.height());
    let restored = r.status_after(&txid, Duration::from_secs(30));

    // The wallet removed and restored from the phrase in the same folder.
    p.remove_and_restore(PAYOUT_PHRASE);
    p.wait_scanned(node.height());
    let restored_in_place = p.status_after(&txid, Duration::from_secs(30));

    // A block spends its input; the payout wallet's filters match no part
    // of it.
    held.start_spv();
    held.wait_scanned(node.height());
    let held_forgotten = held.status_now(&txid);
    node.spend_and_mine(&change.input[0].previous_output);
    held.wait_scanned(node.height());
    let spent = held.status_after(&txid, Duration::from_secs(30));
    eprintln!(
        "forgotten: {forgotten:?}; rescan {rescanned:?}; restore {restored:?}; removed and restored in place {restored_in_place:?}; another installation {held_forgotten:?}, then its input spent in a block {spent:?}"
    );
    assert_eq!(live_dropped, Some(TxStatusKind::Abandoned));
    assert_eq!(live_reconnected, Some(TxStatusKind::Abandoned));
    assert_eq!(forgotten, Some(TxStatusKind::Unconfirmed));
    assert_eq!(rescanned, Some(TxStatusKind::Unconfirmed));
    assert_eq!(restored, None);
    assert_eq!(restored_in_place, None);
    assert_eq!(held_forgotten, Some(TxStatusKind::Unconfirmed));
    assert_eq!(spent, None);
}
