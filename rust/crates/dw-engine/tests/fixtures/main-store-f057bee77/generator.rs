//! Store writer for this fixture, run as a dw-engine integration test at desktop main f057bee77. Writes a
//! data root with current desktop main's engine against a dashd regtest node:
//! a balance over several coins, one coin locked by coin control, a pending
//! own spend in dashd's mempool (never mined) and a refused payment that is
//! abandoned with dashd's network off, so its coin is given back under a
//! rescan hold that cannot finish. The data root is closed, reopened offline
//! by main (no SPV peer), observed, closed and shut down cleanly. Writes
//! `$Q5951_OUT/data` and `$Q5951_OUT/main-observed.json`.
//!
//! Needs `DASHD_PATH` (dashd with dash-cli next to it) and `Q5951_OUT`.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::{OutPoint, Txid};
use dw_engine::{
    CoinFilter, CoinSource, DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink,
    ImportOptions, NetworkSession, Recipient, SendFailure, SessionOptions, SpvState, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use serde_json::{Value, json};
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const COIN: u64 = 100_000_000;
const WAIT: Duration = Duration::from_secs(600);

struct Dashd {
    child: Child,
    cli: PathBuf,
    datadir: PathBuf,
    rpc_port: u16,
    p2p_port: u16,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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
        self.try_cli(args).unwrap_or_else(|e| panic!("dash-cli {args:?}: {e}"))
    }

    fn height(&self) -> u32 {
        self.cli(&["getblockcount"]).parse().unwrap()
    }

    fn mine(&self, n: u32) {
        let to = self.cli(&["-rpcwallet=miner", "getnewaddress"]);
        self.cli(&["generatetoaddress", &n.to_string(), &to]);
    }

    fn pay(&self, to: &str, dash: &str) -> String {
        self.cli(&["-rpcwallet=miner", "sendtoaddress", to, dash])
    }

    fn in_mempool(&self, txid: &str) -> bool {
        self.cli(&["getrawmempool"]).contains(txid)
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

fn online(node: &Dashd) -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec![format!("127.0.0.1:{}", node.p2p_port)],
        no_platform: true,
        ..Default::default()
    }
}

pub fn offline() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    }
}

fn engine(root: &Path) -> Engine {
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

fn wait_scanned(s: &NetworkSession, w: &WalletId, height: u32) {
    let deadline = Instant::now() + WAIT;
    while s.wallet_scan_height(w).is_none_or(|h| h < height) {
        assert!(Instant::now() < deadline, "never scanned to {height}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// What the engine shows for `w`, in the shape the line's test reads back.
pub fn observe(engine: &Engine, s: &Arc<NetworkSession>, w: WalletId, txids: &[Txid]) -> Value {
    let b = s.balances(&w).unwrap().expect("balances known");
    let mut coins: Vec<Value> = engine
        .block_on(s.utxos(
            w,
            CoinFilter { include_locked: true, fully_mixed_only: false, min_confirmations: None },
        ))
        .unwrap()
        .into_iter()
        .map(|c| {
            json!({
                "outpoint": c.outpoint.to_string(),
                "address": c.address,
                "amount": c.amount,
                "confirmations": c.confirmations,
                "block_height": c.block_height,
                "instant_locked": c.instant_locked,
                "chain_locked": c.chain_locked,
                "user_locked": c.user_locked,
                "reserved": c.reserved,
                "is_change": c.is_change,
                "spendable": c.spendable,
                "excluded": c.excluded.map(|e| format!("{e:?}")),
            })
        })
        .collect();
    coins.sort_by_key(|c| c["outpoint"].as_str().unwrap().to_owned());
    let mut locked: Vec<String> = engine
        .block_on(s.locked_outpoints(w))
        .unwrap()
        .into_iter()
        .map(|o| o.to_string())
        .collect();
    locked.sort();
    let txs: Vec<Value> = txids
        .iter()
        .map(|t| {
            let x = engine.block_on(s.tx_detail_extras(w, *t)).unwrap();
            let mut withheld: Vec<String> = x.withheld_outputs.iter().map(|o| o.to_string()).collect();
            withheld.sort();
            json!({
                "txid": x.txid,
                "abandoned": x.abandoned,
                "can_abandon": x.can_abandon,
                "withheld_outputs": withheld,
                "checking_spent_coins": x.checking_spent_coins,
                "total_credit": x.total_credit,
                "total_debit": x.total_debit,
            })
        })
        .collect();
    json!({
        "wallet_id": w.to_string(),
        "balances": {
            "confirmed": b.confirmed, "unconfirmed": b.unconfirmed, "immature": b.immature,
            "locked": b.locked, "total": b.total, "coinjoin": b.coinjoin, "withheld": b.withheld,
            "catching_up": b.catching_up, "held_unfinished": b.held_unfinished,
            "held_unvalued": b.held_unvalued, "held_awaiting": b.held_awaiting,
        },
        "coins": coins,
        "locked_outpoints": locked,
        "txs": txs,
    })
}

/// The wallet's stored rescan-hold marks (`settings_kv`, wallet scope), read
/// from a copy so the store itself is untouched.
pub fn hold_marks(network_dir: &Path, w: &WalletId) -> Vec<Value> {
    let tmp = dw_testutil::private_tempdir();
    for f in ["app.sqlite", "app.sqlite-wal", "app.sqlite-shm"] {
        let from = network_dir.join(f);
        if from.exists() {
            std::fs::copy(&from, tmp.path().join(f)).unwrap();
        }
    }
    let db = rusqlite::Connection::open(tmp.path().join("app.sqlite")).unwrap();
    let mut st = db
        .prepare("SELECT key, value FROM settings_kv WHERE scope = ?1 AND key LIKE 'rescan_hold:%' ORDER BY key")
        .unwrap();
    st.query_map([w.to_string()], |r| {
        Ok(json!({"key": r.get::<_, String>(0)?, "value": r.get::<_, Option<String>>(1)?}))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn q5951_write_main_store() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let out = PathBuf::from(std::env::var_os("Q5951_OUT").expect("Q5951_OUT"));
    let root = out.join("data");
    std::fs::create_dir_all(&root).unwrap();
    let node_dir = dw_testutil::private_tempdir();
    let node = Dashd::start(&dashd, node_dir.path());
    node.cli(&["createwallet", "miner"]);
    node.mine(101);

    let engine = engine(&root);
    let s = engine.block_on(engine.open_network(DashNetwork::Regtest, online(&node))).unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let w = engine
        .block_on(s.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions { birth_height: Some(0), core_compat: false, ..Default::default() },
        ))
        .unwrap();
    let mut addrs = Vec::new();
    for _ in 0..4 {
        addrs.push(engine.block_on(s.next_receive_address(w, None)).unwrap().address);
    }
    for (a, amount) in addrs.iter().zip(["10.0", "5.0", "2.5", "1.0"]) {
        node.pay(a, amount);
        node.mine(1);
    }
    node.mine(1);
    engine.block_on(s.start_spv()).unwrap();
    let deadline = Instant::now() + WAIT;
    while s.spv_state().unwrap() != SpvState::Running {
        assert!(Instant::now() < deadline, "SPV did not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    wait_scanned(&s, &w, node.height());
    let coin_at = |a: &str| -> OutPoint {
        let cs = engine
            .block_on(s.utxos(w, CoinFilter { include_locked: true, fully_mixed_only: false, min_confirmations: None }))
            .unwrap();
        let v: Vec<_> = cs.into_iter().filter(|c| c.address == a).collect();
        assert_eq!(v.len(), 1, "one coin on {a}");
        v[0].outpoint
    };
    let (c10, c5, c25, c1) = (coin_at(&addrs[0]), coin_at(&addrs[1]), coin_at(&addrs[2]), coin_at(&addrs[3]));
    eprintln!("coins: 10={c10} 5={c5} 2.5={c25} 1={c1}");

    // Coin control lock.
    engine.block_on(s.lock_outpoints(w, vec![c25])).unwrap();

    let grant = || {
        s.vault()
            .authorize(GrantPurpose::Spend { max_duffs: 2 * COIN }, Some(&w.0), Credential::None)
            .unwrap()
            .id
    };
    let prepare = |coin: OutPoint, payee: &str| {
        let draft = s.new_tx_draft(w).unwrap();
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
        let prepared = engine.block_on(draft.prepare(grant())).unwrap();
        (draft, prepared)
    };

    // A pending own spend: in dashd's mempool, never mined.
    let payee = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let (draft, prepared) = prepare(c5, &payee);
    let pending: Txid = prepared.summary().txid.parse().unwrap();
    let sent = engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    eprintln!("pending own spend {pending}: {:?}", sent.as_ref().map(|_| ()));
    if !node.in_mempool(&pending.to_string()) {
        let raw = prepared.raw().expect("signed");
        if let Err(e) = node.try_cli(&["sendrawtransaction", &hex::encode(raw)]) {
            assert!(e.contains("already"), "{e}");
        }
    }
    let deadline = Instant::now() + WAIT;
    while !node.in_mempool(&pending.to_string()) {
        assert!(Instant::now() < deadline, "the pending spend never reached dashd");
        std::thread::sleep(Duration::from_millis(200));
    }

    // A refused payment from the 1 DASH coin, abandoned with dashd's network
    // off, so the abandon's rescan cannot reach the tip and the given-back
    // coin stays under its rescan hold.
    let payee2 = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let (draft2, prepared2) = prepare(c1, &payee2);
    let refused: Txid = prepared2.summary().txid.parse().unwrap();
    node.cli(&["prioritisetransaction", &refused.to_string(), &(-10 * COIN as i64).to_string()]);
    let out2 = engine.block_on(draft2.broadcast(Arc::clone(&prepared2)));
    assert!(
        matches!(&out2, Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))),
        "{out2:?}"
    );
    assert!(!node.in_mempool(&refused.to_string()));
    node.cli(&["setnetworkactive", "false"]);
    std::thread::sleep(Duration::from_secs(3));
    eprintln!("SPV state with dashd's network off: {:?}", s.spv_state());
    let abandon_mode = match engine.block_on(s.abandon_transaction(w, refused)) {
        Ok(()) => "dashd network off",
        Err(e) => {
            eprintln!("abandon with dashd's network off refused: {e:?}; retrying with it on");
            node.cli(&["setnetworkactive", "true"]);
            let deadline = Instant::now() + WAIT;
            while s.spv_state().unwrap() != SpvState::Running {
                assert!(Instant::now() < deadline, "SPV did not come back");
                std::thread::sleep(Duration::from_millis(100));
            }
            engine.block_on(s.abandon_transaction(w, refused)).unwrap();
            "dashd network on"
        }
    };
    eprintln!("abandoned {refused} ({abandon_mode})");

    let live = observe(&engine, &s, w, &[pending, refused]);
    eprintln!("live: {live:#}");
    engine.block_on(s.stop_spv()).unwrap();
    let net_dir = s.data_dir().to_path_buf();
    drop(s);
    assert!(engine.block_on(engine.close_network(DashNetwork::Regtest)).unwrap());
    let marks_after_close = hold_marks(&net_dir, &w);

    // Reopened offline by main: the state the line must read back.
    let s = engine.block_on(engine.open_network(DashNetwork::Regtest, offline())).unwrap();
    let reopened = observe(&engine, &s, w, &[pending, refused]);
    drop(s);
    assert!(engine.block_on(engine.close_network(DashNetwork::Regtest)).unwrap());
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
    let marks_final = hold_marks(&net_dir, &w);

    let doc = json!({
        "writer": "origin/main engine (dw-engine tests/q5951_store_writer_main.rs)",
        "phrase": "abandon x11 about, no BIP39 passphrase, regtest, birth height 0",
        "dashd_height": node.height(),
        "coins_paid": {"10": c10.to_string(), "5": c5.to_string(), "2.5": c25.to_string(), "1": c1.to_string()},
        "user_locked": c25.to_string(),
        "pending_own_spend": pending.to_string(),
        "abandoned_refused_payment": refused.to_string(),
        "abandon_mode": abandon_mode,
        "network_dir": net_dir.strip_prefix(&root).unwrap().display().to_string(),
        "live": live,
        "reopened_offline": reopened,
        "rescan_hold_marks_after_close": marks_after_close,
        "rescan_hold_marks_final": marks_final,
    });
    std::fs::write(out.join("main-observed.json"), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    eprintln!("{doc:#}");
    assert_eq!(live, reopened, "main's own offline reopen differs from its live view");
}
