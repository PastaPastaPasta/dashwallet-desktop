//! E0-04 BL-76 end to end: a session whose SPV client syncs from a dashd
//! regtest node hands a leased `TxDraft` First to the network, and the
//! wallet's own sightings settle its dispatch status.
//!
//! - **First → Sent:** the test mines the payment while the hand-off waits
//!   (dash-spv accepts a broadcast from one peer without an InstantSend
//!   quorum only once it is mined), and the hand-off returns `Sent`, or,
//!   when mining and SPV's sighting outrun the permit's 10 s deadline under
//!   load, `broadcast_unknown` (review R1-N1); either way the step settles
//!   `Sent`, announced once.
//! - **MaybeOut → Sent via the wallet's sighting:** nothing is mined during
//!   the hand-off, so the permit's deadline (10 s) cuts it:
//!   `broadcast_unknown`, `MaybeSent`. The wallet holding the transaction
//!   in its mempool view is no evidence; the block that confirms it is
//!   (`SessionHub` → `LeaseTable::note_seen`): `Sent` and one
//!   `DispatchResolved(Sent)`.
//!
//! The deadline stays 10 s; only the test's eventual waits are load
//! tolerant ([`LOAD_WAIT`]). Runs only with `DASHD_PATH` set to a dashd binary with `dash-cli` next
//! to it (e.g. `~/.rust-dashcore-test/dashcore-24.0.0-rc.3/bin/dashd`);
//! skipped otherwise.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dw_engine::platform::{DispatchResolution, DispatchState, FlowKind};
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, Recipient, SendFailure, SessionOptions, SpvState, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const COIN: u64 = 100_000_000;

/// How long an eventual wait (SPV, a scan, a sighting) may take on a loaded
/// host (review R1-N1). The permit's deadline is not one of them.
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

    fn in_mempool(&self, txid: &str) -> bool {
        self.cli(&["getrawmempool"]).contains(txid)
    }
}

impl Drop for Dashd {
    fn drop(&mut self) {
        let _ = self.try_cli(&["stop"]);
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<EngineEvent>>);

impl EventSink for Recorder {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    /// The `DispatchResolved` events of `artifact` under `wallet`.
    fn resolved(&self, wallet: WalletId, artifact: &str) -> Vec<DispatchResolution> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::DispatchResolved { resolved, .. }
                    if resolved.wallet_id == wallet.to_string()
                        && resolved.artifact == artifact =>
                {
                    Some(resolved.resolution)
                }
                _ => None,
            })
            .collect()
    }
}

/// The regtest session's options: SPV from `node` only, no Platform.
fn session_options(node: &Dashd) -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec![format!("127.0.0.1:{}", node.p2p_port)],
        no_platform: true,
        ..Default::default()
    }
}

struct Fixture {
    engine: Engine,
    session: Arc<NetworkSession>,
    rec: Arc<Recorder>,
    wallet: WalletId,
}

impl Fixture {
    fn new(root: &Path, node: &Dashd) -> Self {
        let rec = Arc::new(Recorder::default());
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
            Arc::clone(&rec) as Arc<dyn EventSink>,
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
            rec,
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

    fn receive_address(&self) -> String {
        self.engine
            .block_on(self.session.next_receive_address(self.wallet, None))
            .unwrap()
            .address
    }

    fn grant(&self, purpose: GrantPurpose) -> String {
        self.session
            .vault()
            .authorize(purpose, Some(&self.wallet.0), Credential::None)
            .unwrap()
            .id
    }

    /// An AcceptAndPay lease with a Spend budget, as DP1-02's payment
    /// flows hold.
    fn lease(&self) -> String {
        let grants = vec![
            self.grant(GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: 1_000,
            }),
            self.grant(GrantPurpose::Spend {
                max_duffs: COIN / 2,
            }),
        ];
        self.engine
            .block_on(
                self.session
                    .begin_flow(self.wallet, FlowKind::AcceptAndPay, grants),
            )
            .unwrap()
    }

    fn status(&self, txid: &str) -> Option<DispatchState> {
        self.engine
            .block_on(
                self.session
                    .dashpay(self.wallet)
                    .dispatch_status(txid.into()),
            )
            .unwrap()
    }

    fn wait_status(&self, txid: &str, want: DispatchState, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.status(txid) != Some(want) {
            assert!(
                Instant::now() < deadline,
                "{txid} reads {:?}, waiting for {want:?}",
                self.status(txid)
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Whether the wallet holds `txid` (in any state).
    fn wallet_has(&self, txid: &str) -> bool {
        self.engine
            .block_on(self.session.tx_detail(self.wallet, txid.into()))
            .is_ok()
    }
}

#[test]
fn a_leased_first_settles_by_the_wallets_sightings_against_dashd() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    node.cli(&["createwallet", "miner"]);
    node.mine(101);
    let f = Fixture::new(dir.path(), &node);

    // Two coins of 1 DASH, one for each payment.
    for _ in 0..2 {
        let to = f.receive_address();
        node.cli(&["-rpcwallet=miner", "sendtoaddress", &to, "1.0"]);
    }
    node.mine(1);
    f.start_spv();
    f.wait_scanned(node.height());
    let payee = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let pay = |lease: &str| {
        let draft = f.session.new_tx_draft(f.wallet).unwrap();
        draft
            .set_recipients(vec![Recipient {
                address: payee.clone(),
                amount: COIN / 10,
                subtract_fee_from_amount: false,
                label: None,
                message: None,
            }])
            .unwrap();
        let prepared = f.engine.block_on(draft.prepare(lease.into())).unwrap();
        (draft, prepared)
    };

    // The signed bytes, for dashd when the hand-off left them unsent.
    let push = |prepared: &dw_engine::PreparedTx| {
        let raw: String = prepared
            .raw()
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if let Err(e) = node.try_cli(&["sendrawtransaction", &raw]) {
            assert!(e.contains("already"), "{e}");
        }
    };

    // First → Sent: mined while the hand-off waits. Under load the mining
    // and the sighting may outrun the 10 s deadline: then the hand-off
    // reads `broadcast_unknown` and the block settles it (R1-N1).
    let lease = f.lease();
    let (draft, prepared) = pay(&lease);
    let txid = prepared.summary().txid.clone();
    let sent = std::thread::scope(|s| {
        let send = s.spawn(|| f.engine.block_on(draft.broadcast(Arc::clone(&prepared))));
        let deadline = Instant::now() + LOAD_WAIT;
        while !node.in_mempool(&txid) && !send.is_finished() {
            assert!(Instant::now() < deadline, "{txid} never reached dashd");
            std::thread::sleep(Duration::from_millis(100));
        }
        if !node.in_mempool(&txid) {
            push(&prepared);
        }
        node.mine(1);
        send.join().unwrap()
    });
    match sent {
        Ok(out) => assert_eq!(out.txid, txid, "accepted once mined"),
        // The permit's deadline or SPV's own acceptance wait, whichever
        // a loaded host hits first.
        Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => {}
        Err(other) => panic!("{other:?}"),
    }
    f.wait_status(&txid, DispatchState::Sent, LOAD_WAIT);
    f.wait_scanned(node.height());
    // Once, whichever came first: the hand-off's acceptance or the block.
    assert_eq!(
        f.rec.resolved(f.wallet, &txid),
        vec![DispatchResolution::Sent]
    );

    // MaybeOut → Sent through the wallet's sighting.
    let (draft, prepared) = pay(&lease);
    let txid = prepared.summary().txid.clone();
    let started = Instant::now();
    let out = f.engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    match &out {
        Err(EngineError::Send(SendFailure::BroadcastUnknown { reason })) => {
            assert!(reason.contains("deadline"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    // Cut at the permit's 10 s deadline, never before it; a loaded host
    // may poll the overdue timer later (R1-N1).
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(9),
        "cut at the permit's 10 s deadline: {took:?}"
    );
    assert_eq!(f.status(&txid), Some(DispatchState::MaybeSent));
    // Whatever the cut left on the wire, dashd holds it now.
    push(&prepared);
    assert!(node.in_mempool(&txid));
    // The wallet's own mempool view is no evidence.
    let deadline = Instant::now() + LOAD_WAIT;
    while !f.wallet_has(&txid) {
        assert!(Instant::now() < deadline, "the wallet never held {txid}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(f.status(&txid), Some(DispatchState::MaybeSent));
    assert!(f.rec.resolved(f.wallet, &txid).is_empty());
    node.mine(1);
    f.wait_status(&txid, DispatchState::Sent, LOAD_WAIT);
    assert_eq!(
        f.rec.resolved(f.wallet, &txid),
        vec![DispatchResolution::Sent],
        "announced once, through the block"
    );
}

/// The fee delta that makes dashd refuse a payment: its modified fee falls
/// below the minimum relay fee (Core v24.0.0-rc.2 validation.cpp:1027-1060).
const REFUSING_DELTA: i64 = -10 * COIN as i64;

/// A wallet with one confirmed coin and an unconfirmed payment the engine
/// broadcast itself, its txid resent, the resend still in its acceptance
/// wait. With one peer dash-spv withholds the payment from no one, so no
/// echo can count, and a resend of a txid it already tracks emits no event:
/// the wait runs dash-spv's full 60 s, holding platform-wallet's SPV client
/// read lock, which SPV's stop waits for. dashd refuses the payment, so a
/// later session can only find it in the wallet's own store, never in
/// dashd's mempool.
fn resent_payment(dir: &Path, node: &Dashd) -> (Fixture, String) {
    node.cli(&["createwallet", "miner"]);
    node.mine(101);
    let f = Fixture::new(dir, node);
    let to = f.receive_address();
    node.cli(&["-rpcwallet=miner", "sendtoaddress", &to, "1.0"]);
    node.mine(1);
    f.start_spv();
    f.wait_scanned(node.height());

    let payee = node.cli(&["-rpcwallet=miner", "getnewaddress"]);
    let draft = f.session.new_tx_draft(f.wallet).unwrap();
    draft
        .set_recipients(vec![Recipient {
            address: payee,
            amount: COIN / 10,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    let grant = f.grant(GrantPurpose::Spend {
        max_duffs: COIN / 2,
    });
    let prepared = f.engine.block_on(draft.prepare(grant)).unwrap();
    let txid = prepared.summary().txid.clone();
    node.cli(&["prioritisetransaction", &txid, &REFUSING_DELTA.to_string()]);
    match f.engine.block_on(draft.broadcast(prepared)) {
        Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => {}
        other => panic!("dashd refuses it, so no acceptance is seen ({other:?})"),
    }
    assert!(!node.in_mempool(&txid));
    assert!(f.wallet_has(&txid));
    let id = std::str::FromStr::from_str(&txid).unwrap();
    let started = Instant::now();
    f.engine
        .block_on(f.session.resend_transaction(f.wallet, id))
        .unwrap();
    // The call returns early only when the wait ends within the hand-off.
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the wait ended at once"
    );
    (f, txid)
}

/// QT-091 against dashd: a resend's acceptance wait never holds the
/// session's close. Aborting the wait loses nothing: the reopened wallet
/// still holds the payment unconfirmed, and a resend from it gets it mined.
#[test]
fn a_resends_acceptance_wait_does_not_hold_the_close() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    let (f, txid) = resent_payment(dir.path(), &node);
    let started = Instant::now();
    assert!(
        f.engine
            .block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(30),
        "the close waited for the resend's acceptance wait: {took:?}"
    );

    let session = f
        .engine
        .block_on(
            f.engine
                .open_network(DashNetwork::Regtest, session_options(&node)),
        )
        .unwrap();
    let detail = |session: &Arc<NetworkSession>| {
        f.engine.block_on(session.tx_detail(f.wallet, txid.clone()))
    };
    let reopened = detail(&session).unwrap();
    assert!(
        reopened.block_height.is_none() && reopened.status.confirmations == 0,
        "{:?}",
        reopened.status
    );
    f.engine.block_on(session.start_spv()).unwrap();
    let deadline = Instant::now() + LOAD_WAIT;
    // dashd ignores a txid it refused until its tip moves (v24.0.0-rc.2
    // net_processing.cpp:2359-2366, :5016).
    node.cli(&[
        "prioritisetransaction",
        &txid,
        &(-REFUSING_DELTA).to_string(),
    ]);
    node.mine(1);
    let id = std::str::FromStr::from_str(&txid).unwrap();
    while let Err(e) = f.engine.block_on(session.resend_transaction(f.wallet, id)) {
        assert!(
            matches!(e, EngineError::NoPeers | EngineError::SpvNotRunning)
                && Instant::now() < deadline,
            "{e:?}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    while !node.in_mempool(&txid) {
        assert!(Instant::now() < deadline, "{txid} never reached dashd");
        std::thread::sleep(Duration::from_millis(250));
    }
    node.mine(1);
    let height = node.height();
    while detail(&session).unwrap().block_height != Some(height) {
        assert!(
            Instant::now() < deadline,
            "{txid} never confirmed in the wallet"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// QT-091 against dashd: nor does it hold SPV's stop (Stop, Change peers).
#[test]
fn a_resends_acceptance_wait_does_not_hold_the_spv_stop() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    let (f, _) = resent_payment(dir.path(), &node);
    let started = Instant::now();
    f.engine.block_on(f.session.stop_spv()).unwrap();
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(30),
        "SPV's stop waited for the resend's acceptance wait: {took:?}"
    );
}

/// QT-091 against dashd: nor the engine's shutdown, which the app's exit
/// runs unbounded (dw-app lib.rs, `RunEvent::Exit`).
#[test]
fn a_resends_acceptance_wait_does_not_hold_the_engine_shutdown() {
    let Some(dashd) = std::env::var_os("DASHD_PATH").map(PathBuf::from) else {
        eprintln!("skipped: DASHD_PATH is not set");
        return;
    };
    let dir = dw_testutil::private_tempdir();
    let node_dir = dir.path().join("dashd");
    std::fs::create_dir(&node_dir).unwrap();
    let node = Dashd::start(&dashd, &node_dir);
    let (f, _) = resent_payment(dir.path(), &node);
    let started = Instant::now();
    f.engine.block_on(f.engine.shutdown()).unwrap();
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(30),
        "the engine's shutdown waited for the resend's acceptance wait: {took:?}"
    );
}
