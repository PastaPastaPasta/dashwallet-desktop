use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use dash_sdk::SdkBuilder;
use dash_sdk::dapi_grpc::tonic::transport::{Certificate, ClientTlsConfig, Endpoint};
use dash_sdk::sdk::AddressList;
use dash_spv::{ClientConfig, DevnetConfig};
use dpp::version::PlatformVersion;
use dw_appdb::{APP_DB_FILE, APP_DB_LOCK_FILE, AppDb};
use dw_vault::{VAULT_DIR, Vault, VaultConfig};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletManager;
use platform_wallet_storage::SqlitePersister;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::{Error as PemError, PemObject};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::context::LazyTrustedContext;
use crate::events::{SessionHub, SessionPump, WalletName, WalletState};
use crate::fsutil::{create_owned_dir, create_owned_file};
use crate::gate::{OpGate, OpGuard, WalletLifecycle};
use crate::platform::trust::{self, SharedContext, TrustProvider};
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NoticeCode};

pub(crate) type Manager = PlatformWalletManager<crate::store::WalletStore>;

/// File name of the platform-wallet SQLite database inside a network dir.
pub const WALLET_DB_FILE: &str = "wallet.sqlite";
/// dash-spv storage directory inside a network dir.
pub const SPV_DIR: &str = "spv";
/// DashPay avatar thumbnails inside a network dir (DASHPAY §3.4).
pub const AVATARS_DIR: &str = "avatars";

/// Network-scoped wallet id (key-wallet folds the network into the digest,
/// so the same mnemonic has a different id per network).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WalletId(pub [u8; 32]);

impl fmt::Display for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WalletId({self})")
    }
}

impl FromStr for WalletId {
    type Err = EngineError;
    /// Accepts exactly 64 lower-case hex characters (the contract's id form).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(EngineError::InvalidArgument(
                "wallet id must be 64 lower-case hex characters".into(),
            ));
        }
        let bytes = hex::decode(s)
            .map_err(|e| EngineError::InvalidArgument(format!("wallet id is not hex: {e}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EngineError::InvalidArgument("wallet id must be 32 bytes".into()))?;
        Ok(WalletId(arr))
    }
}

/// Core balance buckets in duffs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
    /// Fully mixed CoinJoin balance (QT-043, `CoinJoinBalances.fully_mixed`)
    /// once the wallet's CoinJoin status was computed; before that, the
    /// spendable balance (confirmed + unconfirmed) of the DIP9 CoinJoin
    /// accounts.
    pub coinjoin: u64,
    /// The scan has not reached the wallet's birth block yet: the buckets
    /// hold what the wallet has seen so far (mempool and InstantSend
    /// payments included) and can miss coins in blocks not scanned yet.
    /// `false` says only that the birth block was scanned: a wallet born
    /// long ago is still behind the tip while SPV syncs (the sync snapshot).
    pub catching_up: bool,
}

/// Result of [`NetworkSession::create_wallet`]: the wallet's seed is in the
/// vault; the phrase is returned for headless hosts (dwcli, tests) that have
/// no other way to show it.
pub struct CreatedWallet {
    pub wallet_id: WalletId,
    pub mnemonic: Zeroizing<String>,
}

impl fmt::Debug for CreatedWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedWallet")
            .field("wallet_id", &self.wallet_id)
            .field("mnemonic", &"<redacted>")
            .finish()
    }
}

/// How to reach the network. Empty lists mean "network defaults"; devnet and
/// regtest have no defaults and need explicit DAPI addresses.
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// DAPI endpoints (`https://host:port`). Empty = SDK defaults (mainnet/testnet only).
    pub dapi_addresses: Vec<String>,
    /// Override for the trusted quorum service. Must be https on mainnet/testnet.
    pub quorum_url: Option<String>,
    /// SPV peers (`ip:port`). When non-empty, SPV connects to these only.
    pub spv_peers: Vec<String>,
    /// PEM CA certificate the SDK trusts for DAPI TLS, in addition to the
    /// system roots (a dashmate devnet gateway serves a self-signed
    /// certificate). Every certificate in the file must parse; the file may
    /// be a CA or a self-signed leaf with `CA:FALSE`, and the DAPI host name
    /// must match its subject alternative names. Replacing the system roots
    /// (pinning) needs an SDK change and is not offered.
    pub ca_cert_path: Option<PathBuf>,
    /// Protocol version the SDK starts at, used as given (a seed below the
    /// network's floor is not clamped). Auto-detect stays on and ratchets it
    /// up from what the network reports. `None` = the per-network floor.
    pub initial_protocol_version: Option<u32>,
    /// The chain has no Platform (a plain dashd regtest): `start_spv` runs
    /// no DashPay bring-up and no Platform sync loops, and every wallet's
    /// `dashpay_startup` stays `NotRun`.
    pub no_platform: bool,
    /// Regtest only: dashd's JSON-RPC (`http://user:password@host:port`,
    /// txindex on) in place of DAPI for the ProRegTxs governance resolves
    /// vote outpoints with (MG-02 D80); regtest has no DAPI.
    pub collateral_rpc: Option<String>,
    /// Tests: this session's DAPI attempt timeout. `None` leaves the SDK
    /// default (10 s, rs-dapi-client `request_settings.rs`). `Some(0)`
    /// disables that deadline, so a request waits until the connection
    /// fails. Absent from production builds.
    #[cfg(test)]
    pub dapi_request_timeout: Option<Duration>,
    /// Tests: the session as a release build opens it (the trusted-only
    /// toggle ignored).
    #[cfg(test)]
    pub as_release_build: bool,
    /// Tests: the SDK the session uses in place of one built from
    /// `dapi_addresses`, a mock whose answers the test set. It is used as
    /// given: no context provider, CA, initial version or protocol version
    /// observer is set on it.
    #[cfg(test)]
    pub sdk: Option<dash_sdk::Sdk>,
}

impl std::fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionOptions")
            .field("dapi_addresses", &self.dapi_addresses)
            .field("quorum_url", &self.quorum_url)
            .field("spv_peers", &self.spv_peers)
            .field("ca_cert_path", &self.ca_cert_path)
            .field("initial_protocol_version", &self.initial_protocol_version)
            .field("no_platform", &self.no_platform)
            // Its URL may carry the RPC password.
            .field(
                "collateral_rpc",
                &self.collateral_rpc.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// How long a close waits for references to the wallet database that others
/// still hold once it dropped its own (a drain, a wallet handle, an in-flight
/// read: platform-wallet `manager/mod.rs:1077-1082` at the pin). The
/// persister's in-process claim on its path goes with the last one, and a
/// reopen in this process is refused until then.
pub(crate) const PERSISTER_RELEASE: Duration = Duration::from_secs(3);

/// Waits, within [`PERSISTER_RELEASE`], for the persister's last reference
/// to go; `false` when one is still held.
async fn persister_released(persister: &Weak<SqlitePersister>) -> bool {
    released(persister).await
}

/// Data directories whose failed open left their wallet database held past
/// the release wait, with that database.
static KEPT_AFTER_FAILED_OPEN: Mutex<Vec<(PathBuf, Weak<SqlitePersister>)>> =
    Mutex::new(Vec::new());

/// Why an open of a [`KEPT_AFTER_FAILED_OPEN`] directory is refused.
pub(crate) const KEPT_AFTER_FAILED_OPEN_REFUSAL: &str = "Dash Wallet needs to restart first. Quit Dash Wallet and open it again. Your wallets and their payments are kept, and Try again won't work until you've restarted.";

/// The key of `data_dir` in [`KEPT_AFTER_FAILED_OPEN`], canonical as the
/// store upgrade's holder keys are.
fn kept_key(data_dir: &Path) -> PathBuf {
    data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf())
}

/// Whether a failed open still holds `data_dir`'s wallet database; an entry
/// whose database was released since is dropped.
pub(crate) fn held_after_failed_open(data_dir: &Path) -> bool {
    let key = kept_key(data_dir);
    let mut kept = KEPT_AFTER_FAILED_OPEN
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    kept.retain(|(_, persister)| persister.strong_count() > 0);
    kept.iter().any(|(dir, _)| *dir == key)
}

/// The end of an open that failed once its wallet database was open, after
/// everything holding it was dropped (Q104 104-F1): no wallet is unloaded,
/// as `remove_wallet` can delete a wallet's stored rows. The release is
/// waited for, with wallets left loaded or not, since the pinned manager no
/// longer keeps the database alive once dropped; only a database still held
/// after the wait is recorded, and while it is held the directory's next
/// open in this process is refused with [`KEPT_AFTER_FAILED_OPEN_REFUSAL`].
pub(crate) async fn release_after_failed_open(
    data_dir: &Path,
    persister: Weak<SqlitePersister>,
    #[cfg_attr(not(test), allow(unused_variables))] left_loaded: bool,
) {
    let released = persister_released(&persister).await;
    #[cfg(test)]
    failed_open_probe::record_end(data_dir, left_loaded, released);
    if released {
        return;
    }
    tracing::warn!("the wallet database stays held after a failed open until the app restarts");
    (KEPT_AFTER_FAILED_OPEN
        .lock()
        .unwrap_or_else(|e| e.into_inner()))
    .push((kept_key(data_dir), persister));
}

/// Flushes the wallets a failed open leaves loaded, as a close does for
/// those it keeps (DEC-1046): their writes are committed, not closed.
async fn flush_left_loaded(persister: &Arc<SqlitePersister>, wallets: &[[u8; 32]]) {
    for &id in wallets {
        let persister = Arc::clone(persister);
        let flushed = tokio::task::spawn_blocking(move || {
            platform_wallet::changeset::PlatformWalletPersistence::flush(&*persister, id)
        })
        .await;
        if !matches!(flushed, Ok(Ok(()))) {
            tracing::warn!(wallet_id = %hex::encode(id), ?flushed, "flush after a failed open failed");
        }
    }
}

/// Ends an open that failed after its manager was built: shuts the manager
/// down, drops it with `held`, and then [`release_after_failed_open`].
async fn end_failed_open(
    data_dir: &Path,
    manager: Arc<Manager>,
    persister: Arc<SqlitePersister>,
    held: impl Send,
) {
    debug_assert!(
        manager.shielded_coordinator().await.is_none(),
        "Q86 F1: no shielded store before the session is handed out"
    );
    let report = manager.shutdown().await;
    if !report.all_clean() {
        tracing::warn!(
            ?report,
            "manager shutdown after a failed open was not clean"
        );
    }
    let loaded = manager.list_wallet_ids_blocking();
    flush_left_loaded(&persister, &loaded).await;
    let released = Arc::downgrade(&persister);
    drop((manager, persister, held));
    release_after_failed_open(data_dir, released, !loaded.is_empty()).await;
}

/// Tests: data directories whose next open fails right after its wallets
/// load, and whose next open fails loading the history.
#[cfg(test)]
pub(crate) static FAIL_LOAD_IN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) static FAIL_HISTORY_LOAD_IN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Tests: data directories whose next open counts none of the wallets the
/// manager loaded among the wallets it knows, as when a registered wallet
/// did not load.
#[cfg(test)]
pub(crate) static KNOWN_WITHOUT_LOADED_IN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Tests: what the failed opens of a data directory left. For each failed
/// journal open, whether its manager had a shielded coordinator and how
/// many wallets it kept loaded (`seen`); for each failed open, whether it
/// ended with a wallet loaded (`ended`).
#[cfg(test)]
pub(crate) mod failed_open_probe {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static SEEN: Mutex<Vec<(PathBuf, bool, usize)>> = Mutex::new(Vec::new());

    pub(crate) fn record(data_dir: &Path, coordinator: bool, loaded: usize) {
        (SEEN.lock().unwrap_or_else(|p| p.into_inner())).push((
            data_dir.to_path_buf(),
            coordinator,
            loaded,
        ));
    }

    pub(crate) fn seen(data_dir: &Path) -> Vec<(bool, usize)> {
        (SEEN.lock().unwrap_or_else(|p| p.into_inner()).iter())
            .filter(|(d, ..)| d == data_dir)
            .map(|(_, c, n)| (*c, *n))
            .collect()
    }

    /// For each failed open of a data directory that ended, whether it
    /// dropped its manager with a wallet still loaded, and whether its
    /// wallet database was released within the wait.
    static ENDED: Mutex<Vec<(PathBuf, bool, bool)>> = Mutex::new(Vec::new());

    pub(crate) fn record_end(data_dir: &Path, left_loaded: bool, released: bool) {
        (ENDED.lock().unwrap_or_else(|p| p.into_inner())).push((
            data_dir.to_path_buf(),
            left_loaded,
            released,
        ));
    }

    pub(crate) fn ended(data_dir: &Path) -> Vec<(bool, bool)> {
        (ENDED.lock().unwrap_or_else(|p| p.into_inner()).iter())
            .filter(|(d, ..)| d == data_dir)
            .map(|(_, loaded, released)| (*loaded, *released))
            .collect()
    }
}

/// Waits, within [`PERSISTER_RELEASE`], for the last reference to `held`
/// to go; `false` when one is still held.
async fn released<T>(held: &Weak<T>) -> bool {
    let deadline = tokio::time::Instant::now() + PERSISTER_RELEASE;
    while held.strong_count() > 0 {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    true
}

/// What a session holds while it is open. Taken out on close so the
/// persister (and its process-wide open-path claim) is released even while
/// hosts still hold `Arc<NetworkSession>`.
#[derive(Clone)]
pub(crate) struct Live {
    pub manager: Arc<Manager>,
    pub persister: Arc<SqlitePersister>,
    /// The store upgrade's proof the persister opened through it.
    pub _persister_opened: Arc<dw_store_upgrade::PersisterOpened>,
    /// The persister platform-wallet uses; knows which wallets are closed.
    pub store: Arc<crate::store::WalletStore>,
    pub appdb: Arc<AppDb>,
    /// The changeset tap the store feeds: DashPay signals and the journal
    /// (DASHPAY §3.5). Here, not on the session, so close releases its
    /// `app.sqlite` handle.
    pub tap: Arc<crate::platform::journal::ChangesetTap>,
    /// Positive provenance (DEC-125): which Platform data counts as verified.
    pub provenance: Arc<crate::platform::provenance::Provenance>,
    /// The wallets' unfinished replays (`crate::replay_record`).
    pub replay: Arc<crate::replay_record::ReplayRecords>,
}

/// The running event pump: its stop signal and task.
struct PumpTask {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// One open network: SDK + PlatformWalletManager<SqlitePersister>, the
/// network's vault and app database, and the in-memory read models.
pub struct NetworkSession {
    pub(crate) network: DashNetwork,
    data_dir: PathBuf,
    /// The avatar cache directory could be created at open; without it
    /// the session keeps avatar thumbnails in memory only (`avatars_dir`).
    avatars_ready: bool,
    pub(crate) rt: Handle,
    pub(crate) sink: Arc<dyn EventSink>,
    context: Arc<LazyTrustedContext>,
    /// The layered quorum provider every proof's quorum key comes from
    /// (DASHPAY §2.2).
    pub(crate) trust: Arc<TrustProvider>,
    /// Holds every wallet's seed (`<network dir>/vault`). platform-wallet
    /// registers wallets external-signable, so this is the only key material.
    pub(crate) vault: Vault,
    pub(crate) spv_peers: Vec<SocketAddr>,
    pub(crate) hub: Arc<SessionHub>,
    /// Admission of operations vs close (review M1).
    pub(crate) gate: OpGate,
    /// Tests: how long an in-app close waits for a wallet
    /// load (`test_support::hold_close_fence`).
    #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
    pub(crate) close_load_wait: std::sync::Mutex<Option<std::time::Duration>>,
    /// Other crates' tests: holds the next Platform send before it starts
    /// (`test_support::hold_platform_send`).
    #[cfg(all(debug_assertions, feature = "test-support"))]
    pub(crate) hold_platform_send: std::sync::Mutex<
        Option<(
            std::sync::mpsc::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// Wallet registrations and removals, one at a time (DEC-210 (3b)).
    pub(crate) wallet_lifecycle: WalletLifecycle,
    live: RwLock<Option<Live>>,
    pump: Mutex<Option<PumpTask>>,
    /// Inputs of prepared, not yet settled transactions (E2 send).
    pub(crate) spends: crate::send::PendingSpends,
    /// One coin fence per wallet (`coins.rs`, E0-04 §2a.6).
    pub(crate) coin_fences: crate::coins::CoinFences,
    /// "Include my username in payment requests" vs the URIs that read it
    /// (`receive.rs`, DP3-03b D20).
    pub(crate) username_gate: crate::receive::UsernameGate,
    /// When this session opened, UNIX seconds ("Startup time").
    pub(crate) opened_at: u64,
    /// The previous session of this network did not close cleanly (its
    /// marker file was still there at open).
    pub(crate) unclean_previous: bool,
    /// This open upgraded the wallet data without a before-upgrade copy it
    /// can restore ([`NetworkSession::upgrade_notice`]).
    upgrade_notice: Option<dw_store_upgrade::UpgradeNotice>,
    /// dash-qt's load-on-startup wallet list; `None` = load every wallet.
    pub(crate) startup_list: Mutex<Option<std::collections::BTreeSet<WalletId>>>,
    /// The console `walletpassphrase` relock timer (review L1): one per
    /// session, replaced by a later one, cancelled by `lock_vault` and
    /// `close`. (generation, task).
    pub(crate) relock: Mutex<Option<(u64, tokio::task::AbortHandle)>>,
    /// Automatic backups (`backup.rs`).
    pub(crate) backups: crate::backup::BackupRuntime,
    /// Orders every write of "ask for my passphrase for every payment"
    /// with what the vault is told of it (`keys.rs`).
    pub(crate) payment_auth: tokio::sync::Mutex<()>,
    /// Per wallet, orders an Abandon's marks with a masternode operation's
    /// resend (MG-10 §1.10): an abandon holds it for writing from its plan
    /// through its marks, a resend for reading from its last mark check
    /// through its hand-off, so no resend hands off bytes an abandon has
    /// marked.
    pub(crate) abandon_gates:
        Mutex<std::collections::HashMap<WalletId, Arc<tokio::sync::RwLock<()>>>>,
    /// Other crates' tests: the next write of that setting fails.
    #[cfg(all(debug_assertions, feature = "test-support"))]
    pub(crate) payment_auth_fault: std::sync::atomic::AtomicBool,
    /// The resends' acceptance waits (QT-091), aborted before SPV stops
    /// (`stop_spv_inner`, and `close` before the manager's shutdown): each
    /// holds platform-wallet's SPV client read lock for dash-spv's acceptance
    /// timeout (60 s), and SPV's stop waits for the write lock
    /// (rs-platform-wallet spv/runtime.rs:321-330, :446).
    pub(crate) resends: Mutex<Vec<tokio::task::AbortHandle>>,
    /// Mixing state, CoinJoin options and queues (M3, `coinjoin.rs`).
    pub(crate) coinjoin: crate::coinjoin::CoinJoinRuntime,
    /// The masternode follower and its event (MG-03, `masternodes/`).
    pub(crate) masternodes: crate::masternodes::follow::MasternodesRuntime,
    /// The bring-up, the Platform sync loops and their status (E0-05).
    pub(crate) platform: crate::platform::runtime::PlatformRuntime,
    /// Leases, the lock coordinator and the dispatch fence (E0-04).
    pub(crate) leases: Arc<crate::platform::lease::LeaseTable>,
    /// The owning handles of the facade's leases, from `begin_flow` until
    /// `end_flow` (E0-04 §4.1): the string id the host holds owns nothing.
    pub(crate) flow_leases: Mutex<
        std::collections::HashMap<crate::platform::lease::LeaseId, crate::platform::lease::Lease>,
    >,
    /// Governance sync and its store (MG-02, `governance/`).
    pub(crate) governance: crate::governance::sync::GovernanceRuntime,
    /// The withdrawals' nonces past Platform's proved ones (DP6-02 D24).
    pub(crate) withdrawal_nonces: crate::platform::withdrawal::WithdrawalNonces,
}

/// Tests: an operation [`NetworkSession::on_runtime`] spawns, boxed so the
/// tests' task-locals can wrap it.
#[cfg(test)]
type TestRun<T> = std::pin::Pin<Box<dyn Future<Output = Result<T, EngineError>> + Send>>;

impl NetworkSession {
    /// The wallet's gate of [`NetworkSession::abandon_gates`].
    pub(crate) fn abandon_gate(&self, wallet: WalletId) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = self.abandon_gates.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(gates.entry(wallet).or_default())
    }

    /// Opens the session. Must run on the engine runtime: the manager spawns
    /// its persistence adapter task with `tokio::spawn`.
    pub(crate) async fn open(
        network: DashNetwork,
        data_dir: PathBuf,
        sink: Arc<dyn EventSink>,
        opts: SessionOptions,
        vault_config: VaultConfig,
    ) -> Result<Arc<Self>, EngineError> {
        let opened = Instant::now();
        if held_after_failed_open(&data_dir) {
            return Err(EngineError::StorageInUse {
                detail: KEPT_AFTER_FAILED_OPEN_REFUSAL.into(),
                retryable: false,
            });
        }
        network.validate()?;
        let spv_peers = opts
            .spv_peers
            .iter()
            .map(|p| {
                p.parse::<SocketAddr>()
                    .map_err(|e| EngineError::InvalidArgument(format!("bad SPV peer {p:?}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Everything the caller can get wrong, before any directory, vault or
        // network I/O exists to clean up.
        let sdk_options = SdkOptions::validate(&opts)?;
        let collateral_rpc = match &opts.collateral_rpc {
            Some(_) if network != DashNetwork::Regtest => {
                return Err(EngineError::InvalidArgument(
                    "a collateral RPC is for regtest only".into(),
                ));
            }
            Some(url) => Some(
                crate::governance::collateral::RpcSource::new(url)
                    .map_err(EngineError::InvalidArgument)?,
            ),
            None => None,
        };
        let persister_config = crate::store_upgrade::persister_config(&data_dir);
        let upgrade_notice = {
            let (dir, config) = (data_dir.clone(), persister_config.clone());
            tokio::task::spawn_blocking(move || {
                crate::fsutil::refuse_insecure_root(&dir)?;
                crate::store_upgrade::prepare_and_reconcile(&dir, &config)
            })
            .await??
        };
        create_owned_dir(&data_dir, Path::new(""))?;
        crate::store_upgrade::stores_opening(&data_dir);
        let marker = data_dir.join(crate::tools::SESSION_MARKER);
        let unclean_previous = marker.exists();

        let (dir, core_network, tag) =
            (data_dir.clone(), network.core_network(), network.dir_name());
        let vault =
            tokio::task::spawn_blocking(move || open_vault(&dir, core_network, &tag, vault_config))
                .await??;

        let dir = data_dir.clone();
        let (appdb, names) = tokio::task::spawn_blocking(move || open_app_db(&dir)).await??;
        let appdb = Arc::new(appdb);
        // Before the persister holds wallet.sqlite and once app.sqlite,
        // where its records go, is open: no other writer can touch the
        // store, and the load reads what was dropped (#739, Q112).
        {
            let (dir, db) = (data_dir.clone(), Arc::clone(&appdb));
            let repaired =
                tokio::task::spawn_blocking(move || crate::store_repair::repair(&dir, &db)).await;
            match repaired {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "store repair: stopped"),
                Err(e) => tracing::warn!(error = %e, "store repair: panicked"),
            }
        }
        {
            let (vault, db) = (vault.clone(), Arc::clone(&appdb));
            tokio::task::spawn_blocking(move || {
                vault.attach_policy_commitments(Arc::new(crate::keys::PolicyCommitmentStore(
                    Arc::downgrade(&db),
                )));
                crate::keys::sync_signing_window(&vault, &db)
            })
            .await?;
        }
        // Owner-only whatever the umask (DASHPAY §3.4). The cache is
        // disposable, so a stray file, a symlink that cannot be resolved, or
        // an entry owned by another user (or a symlink loop) named `avatars`
        // turns avatars off for this session instead of failing the open. A
        // dangling symlink whose target can be created is followed and the
        // target created (mode 0700); avatars stay on.
        let avatars_ready = match create_owned_dir(&data_dir, Path::new(AVATARS_DIR)) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, dir = %data_dir.join(AVATARS_DIR).display(),
                    "avatar cache directory unusable; avatars are disabled for this session");
                false
            }
        };
        let coinjoin_settings = {
            let db = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || crate::coinjoin::load_settings(&db)).await?
        };
        let governance_enabled = {
            let db = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || crate::governance::sync::load_enabled(&db)).await?
        };

        let context = Arc::new(LazyTrustedContext::new(
            network.core_network(),
            network.devnet_name().map(str::to_string),
            opts.quorum_url.clone(),
        ));
        init_quorum_context(&context, &network, sink.as_ref()).await?;

        let trust = TrustProvider::new(
            trust::DEFAULT_MODE,
            Arc::new(Arc::clone(&context)),
            Arc::clone(&sink),
            network.clone(),
        );
        #[cfg(test)]
        let trust = if opts.as_release_build {
            trust.in_release_build()
        } else {
            trust
        };
        let trust = Arc::new(trust);
        let allowed = trust.trusted_only_allowed();
        trust.set_trusted_only({
            let appdb = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || read_trusted_only(&appdb, allowed)).await?
        });
        let protocol = Arc::new(crate::platform::protocol::ProtocolRuntime::default());
        let sdk = build_sdk(
            &network,
            &opts,
            sdk_options,
            SharedContext {
                trusted: Arc::clone(&context),
                trust: Arc::clone(&trust),
            },
            (!opts.no_platform).then(|| protocol.observer()),
        )?;

        // SqlitePersister creates its backup directory with the umask's mode
        // and refuses one below a group-writable directory: `backups` and
        // `backups/auto` are owner-only, legacy 0775 ones included, before
        // it needs a pre-migration backup.
        let auto_backups = (persister_config.auto_backup_dir.as_deref())
            .and_then(|dir| dir.strip_prefix(&data_dir).ok())
            .ok_or_else(|| {
                EngineError::Internal(format!(
                    "{:?} is not inside the network directory",
                    persister_config.auto_backup_dir
                ))
            })?;
        create_owned_dir(&data_dir, auto_backups)?;
        let dir = data_dir.clone();
        let (persister, persister_opened) = tokio::task::spawn_blocking(move || {
            crate::store_upgrade::open_persister(&dir, persister_config)
        })
        .await??;
        let persister_opened = Arc::new(persister_opened);
        // dash-qt loads only the wallets of its settings list; a missing
        // list loads every wallet, and so does one that would load none of
        // them: with no wallet open, closed ones cannot be reached. Such a
        // list is stored again with every wallet on it, so what Settings
        // shows is what this start did.
        let mut startup_list = {
            let appdb = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || crate::multiwallet::read_startup_list(&appdb))
                .await??
        };
        let unloaded: std::collections::HashSet<[u8; 32]> = match &startup_list {
            Some(list) => {
                let db_path = data_dir.join(WALLET_DB_FILE);
                let registered = tokio::task::spawn_blocking(move || {
                    crate::store::registered_wallet_ids(&db_path)
                })
                .await?
                .map_err(|e| EngineError::Storage(format!("wallet list: {e}")))?;
                if registered.iter().any(|id| list.contains(&WalletId(*id))) {
                    registered
                        .into_iter()
                        .filter(|id| !list.contains(&WalletId(*id)))
                        .collect()
                } else {
                    if !registered.is_empty() {
                        let every: std::collections::BTreeSet<WalletId> =
                            registered.iter().copied().map(WalletId).collect();
                        let (appdb, stored) = (Arc::clone(&appdb), every.clone());
                        if let Err(e) = tokio::task::spawn_blocking(move || {
                            crate::multiwallet::write_startup_list(&appdb, &stored)
                        })
                        .await?
                        {
                            tracing::warn!(error = %e, "could not repair the load-on-startup list");
                        }
                        startup_list = Some(every);
                    }
                    Default::default()
                }
            }
            None => Default::default(),
        };
        // Before any wallet loads: an abandon marked but not swept is swept,
        // so its change is never credited. A wallet this start leaves closed
        // is swept when it is opened.
        {
            let (persister, appdb) = (Arc::clone(&persister), Arc::clone(&appdb));
            let db_path = data_dir.join(WALLET_DB_FILE);
            let closed = unloaded.clone();
            tokio::task::spawn_blocking(move || {
                let wallets: Vec<WalletId> = crate::store::registered_wallet_ids(&db_path)
                    .map_err(|e| EngineError::Storage(format!("wallet list: {e}")))?
                    .into_iter()
                    .filter(|id| !closed.contains(id))
                    .map(WalletId)
                    .collect();
                crate::tx_actions::finish_marked_abandons(&persister, &appdb, &db_path, &wallets)
            })
            .await??;
        }
        let hub = Arc::new(SessionHub::new(network.clone(), Arc::clone(&sink)));
        let tap = Arc::new(crate::platform::journal::ChangesetTap::new(
            Arc::clone(&hub),
            Arc::clone(&appdb),
        ));
        let provenance = {
            let appdb = Arc::clone(&appdb);
            let named: std::collections::HashSet<String> =
                names.iter().map(|(id, _, _)| id.clone()).collect();
            let db_path = data_dir.join(WALLET_DB_FILE);
            Arc::new(
                tokio::task::spawn_blocking(move || {
                    let provenance = crate::platform::provenance::Provenance::open(appdb);
                    // H-09: a wallet this installation never named came with
                    // data fetched elsewhere, maybe through the fallback.
                    let unnamed = crate::store::registered_wallet_ids(&db_path).map(|ids| {
                        ids.iter()
                            .any(|id| !named.contains(&WalletId(*id).to_string()))
                    });
                    if unnamed.unwrap_or(true) {
                        provenance.data_fetched_elsewhere();
                    }
                    provenance
                })
                .await?,
            )
        };
        let store = Arc::new(crate::store::WalletStore::new(
            Arc::clone(&persister),
            unloaded,
            Arc::clone(&tap),
        ));
        let replay = crate::replay_record::ReplayRecords::new(Arc::clone(&appdb));
        store.install_replay(&replay);
        hub.attach_replay(&replay);
        let listed: Vec<[u8; 32]> = (names.iter())
            .filter_map(|(id, _, _)| id.parse::<WalletId>().ok().map(|id| id.0))
            .collect();
        for (order, (id, name, created_at)) in (0u64..).zip(names) {
            match id.parse::<WalletId>() {
                Ok(id) => hub.set_name(
                    id,
                    WalletName {
                        name,
                        created_at: Some(created_at),
                        order,
                    },
                ),
                Err(_) => tracing::warn!(wallet_id = %id, "ignoring a malformed wallet name row"),
            }
        }
        let manager = Arc::new(PlatformWalletManager::new(
            Arc::new(sdk),
            Arc::clone(&store),
            Arc::clone(&hub) as Arc<dyn platform_wallet::PlatformEventHandler>,
        ));
        replay.start(Arc::downgrade(&manager), Arc::downgrade(&hub));
        protocol.attach(Arc::downgrade(&manager), Arc::downgrade(&hub));
        trust.attach(
            Arc::new(trust::RuntimeQuorums(Arc::downgrade(&manager.spv_arc()))),
            &provenance,
        );
        if hub.trust.set(Arc::clone(&trust)).is_err() {
            tracing::warn!("the session hub already had a quorum provider");
        }
        let loaded = manager
            .load_from_persistor()
            .await
            .map_err(EngineError::from);
        #[cfg(test)]
        let loaded = match crate::keys::take_test_failure(&FAIL_LOAD_IN, &data_dir) {
            true => loaded.and(Err(EngineError::Storage("test: load failed".into()))),
            false => loaded,
        };
        if let Err(e) = loaded {
            end_failed_open(&data_dir, manager, persister, (store, replay, tap)).await;
            return Err(e);
        }

        let lock_state = vault.lock_state();
        let masternodes = crate::masternodes::follow::MasternodesRuntime::new(
            network.clone(),
            Arc::clone(&sink),
            &Handle::current(),
        );
        // The barriers of payout changes signed before MG-13, once per app
        // database, before anything can quote a withdrawal (P16).
        let barriers = {
            let (db, core) = (Arc::clone(&appdb), network.core_network());
            tokio::task::spawn_blocking(move || {
                crate::masternodes::follow::backfill_payout_barriers(
                    &db,
                    core,
                    crate::events::unix_now(),
                )
            })
            .await
        };
        match barriers {
            Ok(barriers) => masternodes.set_barriers_ready(barriers),
            Err(e) => {
                end_failed_open(&data_dir, manager, persister, (store, replay, tap)).await;
                return Err(e.into());
            }
        }
        // A change of the wallet set re-reads the masternodes (MG-03 D28).
        let sink = masternodes.watch_wallets(sink);
        // The wallets the session knows: loaded, unloaded, listed in the app
        // database, or registered in wallet.sqlite, whatever loaded. A
        // shielded spend imported from the 0.1 journal of any other wallet is
        // archived for good, so a wallet list that does not read stops the
        // open.
        let loaded_ids = manager.wallet_ids().await;
        #[cfg(test)]
        let loaded_ids = match crate::keys::take_test_failure(&KNOWN_WITHOUT_LOADED_IN, &data_dir) {
            true => Vec::new(),
            false => loaded_ids,
        };
        let registered = {
            let db_path = data_dir.join(WALLET_DB_FILE);
            tokio::task::spawn_blocking(move || crate::store::registered_wallet_ids(&db_path)).await
        };
        let registered = match registered {
            Ok(Ok(ids)) => ids,
            Ok(Err(e)) => {
                end_failed_open(&data_dir, manager, persister, (store, replay, tap)).await;
                return Err(EngineError::Storage(format!("wallet list: {e}")));
            }
            Err(e) => {
                end_failed_open(&data_dir, manager, persister, (store, replay, tap)).await;
                return Err(e.into());
            }
        };
        let known: std::collections::HashSet<[u8; 32]> = (loaded_ids.into_iter())
            .chain(store.unloaded())
            .chain(listed)
            .chain(registered)
            .collect();
        let opened_journal = crate::platform::lease::session::open_journal(
            &data_dir,
            known,
            Arc::clone(&persister_opened),
        )
        .await;
        let (journal, shielded_drain) = match opened_journal {
            Ok(opened) => opened,
            Err(e) => {
                #[cfg(test)]
                failed_open_probe::record(
                    &data_dir,
                    manager.shielded_coordinator().await.is_some(),
                    manager.list_wallet_ids_blocking().len(),
                );
                end_failed_open(&data_dir, manager, persister, (store, replay, tap)).await;
                return Err(e);
            }
        };
        let leases = crate::platform::lease::LeaseTable::new(
            network.clone(),
            Arc::clone(&sink),
            Handle::current(),
            Default::default(),
            vault.epoch(),
        );
        let collateral_source: Option<Arc<dyn crate::governance::collateral::CollateralSource>> =
            match collateral_rpc {
                Some(rpc) => Some(Arc::new(rpc)),
                None if opts.no_platform => None,
                None => Some(Arc::new(crate::governance::collateral::SdkSource(
                    manager.sdk_arc(),
                ))),
            };
        let governance = crate::governance::sync::GovernanceRuntime::new(
            network.clone(),
            Arc::clone(&sink),
            &Handle::current(),
            governance_enabled,
            collateral_source,
        );
        let session = Arc::new(Self {
            network,
            data_dir,
            avatars_ready,
            rt: Handle::current(),
            sink,
            context,
            trust,
            vault,
            spv_peers,
            hub: Arc::clone(&hub),
            gate: OpGate::default(),
            #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
            close_load_wait: Default::default(),
            #[cfg(all(debug_assertions, feature = "test-support"))]
            hold_platform_send: Default::default(),
            wallet_lifecycle: WalletLifecycle::default(),
            live: RwLock::new(Some(Live {
                manager: Arc::clone(&manager),
                persister,
                _persister_opened: persister_opened,
                store,
                appdb: Arc::clone(&appdb),
                tap,
                provenance,
                replay,
            })),
            pump: Mutex::new(None),
            spends: Default::default(),
            coin_fences: Default::default(),
            username_gate: Default::default(),
            withdrawal_nonces: Default::default(),
            opened_at: crate::events::unix_now(),
            unclean_previous,
            upgrade_notice,
            startup_list: Mutex::new(startup_list),
            relock: Mutex::new(None),
            backups: Default::default(),
            payment_auth: tokio::sync::Mutex::new(()),
            abandon_gates: Mutex::default(),
            #[cfg(all(debug_assertions, feature = "test-support"))]
            payment_auth_fault: Default::default(),
            resends: Mutex::new(Vec::new()),
            coinjoin: crate::coinjoin::CoinJoinRuntime::new(coinjoin_settings),
            masternodes,
            platform: crate::platform::runtime::PlatformRuntime::new(
                lock_state,
                !opts.no_platform,
                Arc::clone(&hub),
                protocol,
            ),
            leases,
            flow_leases: Mutex::default(),
            governance,
        });
        // Before anything can configure upstream's shielded store.
        let _ = session.platform.shielded_drain.set(shielded_drain);
        session.governance.follow(&session);
        if session.governance.enabled() {
            session.governance.start(&session);
        }
        match journal {
            Some((backend, rows, steps, calls)) => {
                session
                    .leases
                    .load_journal_with(Some(backend), rows, steps, calls)
            }
            None => {
                session
                    .leases
                    .load_journal_with(None, Vec::new(), Vec::new(), Default::default())
            }
        }
        crate::platform::shielded_pool::import_fork_holds(&session).await;
        session.leases.start_reaper();
        session.masternodes.follow(&session);
        // MG-13 P8: undecided masternode withdrawals are rechecked from the
        // start, then periodically.
        session.masternodes.withdrawals.start(&session);
        // Mode B's funding statuses (E0-04 §2a.5, H15): its evidence source,
        // the tap's hook on tracked-row changes, and a first read of every
        // loaded wallet's rows. The same source resumes tracked rows for
        // the catch-up (H6 part 2), which SPV's start and its peers drive.
        let tracked = Arc::new(crate::platform::lease::mode_b::SessionTracked {
            session: Arc::downgrade(&session),
        });
        session.leases.install_tracked(tracked.clone());
        // The persister's tap clears the shielded spend markers (DEC-1057).
        if let Ok(live) = session.live() {
            live.store.install_leases(&session.leases);
        }
        session.leases.install_resumer(tracked);
        session.hub.attach_leases(&session.leases);
        session.attach_reconcile();
        session.attach_dead_claims();
        session.attach_sweep_resend();
        session.attach_balance_reread();
        session.attach_sweep_news();
        // Only a header the SPV store holds is evidence for a sweep record:
        // absent, unreadable or no SPV, its chunk stays pending.
        let spv = Arc::downgrade(&manager.spv_arc());
        session.hub.attach_headers(Arc::new(move |height| {
            let spv = spv.clone();
            Box::pin(async move {
                match spv.upgrade()?.header_hash_at(height).await {
                    platform_wallet::spv::HeaderLookup::Found(hash) => Some(hash),
                    _ => None,
                }
            })
        }));
        if let Ok(live) = session.live() {
            live.tap.watch_locks(&session.leases);
        }
        for id in manager.list_wallet_ids_blocking() {
            session.leases.request_refresh(WalletId(id));
        }
        if let Ok(live) = session.live() {
            let weak = Arc::downgrade(&session);
            let funded = move |wallet, draft| {
                if let Some(session) = weak.upgrade() {
                    crate::platform::registration_flow::backup_once(&session, wallet, draft);
                }
            };
            let _ = live.tap.on_funded.set(Box::new(funded));
            let weak = Arc::downgrade(&session);
            let checked = move |wallet| {
                if let Some(session) = weak.upgrade() {
                    session.expire_contact_profiles_soon(wallet);
                }
            };
            let _ = live.tap.on_contact_profiles.set(Box::new(checked));
            let weak = Arc::downgrade(&session);
            let queued = move |wallet| {
                if let Some(session) = weak.upgrade() {
                    session.platform.contact_accounts_queued(wallet);
                }
            };
            let _ = live.tap.on_contact_accounts_queued.set(Box::new(queued));
            let _ = live
                .tap
                .registrations
                .set(Arc::clone(&session.platform.registrations));
        }
        if let Err(e) =
            create_owned_file(&session.data_dir, Path::new(crate::tools::SESSION_MARKER))
        {
            tracing::warn!(error = %e, "could not write the open-session marker");
        }
        // Avatars (DP4-02): the toggle, the cache directory's descriptor.
        session.open_avatars(Arc::clone(&appdb)).await;
        // Before SPV starts: no block can replay a coin outside its account.
        // A replay owed but not stored: the session does not open.
        if let Err(e) = session.restore_loaded_wallets(&manager).await {
            drop((manager, appdb));
            session.close_after_failed_open().await;
            return Err(e);
        }
        for id in manager.list_wallet_ids_blocking() {
            session.resume_replay(&manager, WalletId(id)).await;
        }
        for id in manager.list_wallet_ids_blocking() {
            session.refresh_wallet_state(&manager, WalletId(id)).await;
            session.load_identity_choices(WalletId(id)).await;
            session.load_payment_locks(WalletId(id)).await;
            // Each pass fetches the contacts' profiles (DP4-01 D12).
            session
                .expire_contact_profiles(&manager, WalletId(id))
                .await;
        }
        let history = session.load_history().await;
        #[cfg(test)]
        let history = match crate::keys::take_test_failure(&FAIL_HISTORY_LOAD_IN, &session.data_dir)
        {
            true => history.and(Err(EngineError::Storage(
                "test: history load failed".into(),
            ))),
            false => history,
        };
        if let Err(e) = history {
            drop((manager, appdb));
            session.close_after_failed_open().await;
            return Err(e);
        }
        session.clean_export_staging().await;
        session.start_pump(&manager, appdb);
        session.back_up_every_wallet_once();
        session.spawn_ensure_background();
        session.spawn_reverify();
        tracing::info!(
            elapsed_ms = opened.elapsed().as_millis() as u64,
            "startup session-open"
        );
        Ok(session)
    }

    fn start_pump(self: &Arc<Self>, manager: &Arc<Manager>, appdb: Arc<AppDb>) {
        let _ = self.hub.appdb.set(Arc::downgrade(&appdb));
        let (stop, stop_rx) = watch::channel(false);
        let target = SessionPump {
            hub: Arc::clone(&self.hub),
            spv: manager.spv_arc(),
            appdb,
            vault: self.vault.clone(),
        };
        let hub = Arc::clone(&self.hub);
        let task = tokio::spawn(async move { hub.pump.run(&target, stop_rx).await });
        *self.pump.lock().unwrap_or_else(|p| p.into_inner()) = Some(PumpTask { stop, task });
    }

    /// Reads a wallet's balance and scan heights into the hub, all from
    /// key-wallet under one read lock. Never from platform-wallet's
    /// event-fed balance mirror: no event follows the engine's own changes
    /// to key-wallet state (an abandon, the coins re-placed after a load,
    /// the duplicate reconcile), so the mirror can keep a total they undid.
    /// A fork truncation no event announced is applied first
    /// (`SessionHub::detect_truncation`): the read would hide it.
    pub(crate) async fn refresh_wallet_state(&self, manager: &Manager, id: WalletId) {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&id.0) else {
            return;
        };
        let core = &info.core_wallet;
        if let Some(cl) = &core.metadata.last_applied_chain_lock {
            self.hub.note_chainlock(cl.block_height, cl.block_hash);
        }
        self.hub.detect_truncation(id, core);
        let read = WalletState {
            birth_height: core.birth_height(),
            synced_height: core.synced_height(),
            processed_height: core.last_processed_height(),
            balance: core.balance(),
            accounts: core.account_balances(),
            ..WalletState::default()
        };
        self.hub.update_wallet_state(id, |s| s.take_core(read));
    }

    pub fn network(&self) -> &DashNetwork {
        &self.network
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// What to tell the user about the store upgrade this session's open
    /// ran, once its persister opened and its wallets loaded: `Some` when it
    /// upgraded the wallet data without a before-upgrade copy it can restore
    /// (Q107 107-1). It never blocks anything and is the same for the whole
    /// session; nothing on disk records that it was shown.
    pub fn upgrade_notice(&self) -> Option<crate::store_upgrade::copy::Line> {
        self.upgrade_notice
            .map(crate::store_upgrade::copy::notice_line)
    }

    /// The avatar thumbnail directory (DASHPAY §3.4), or `None` when it could
    /// not be created at open: thumbnails are then kept in memory only.
    pub fn avatars_dir(&self) -> Option<PathBuf> {
        self.avatars_ready.then(|| self.data_dir.join(AVATARS_DIR))
    }

    /// Runs `fut` on the engine runtime (for callers outside dw-engine whose
    /// work spawns tasks or uses tokio timers, e.g. the console).
    pub async fn run_on_engine<T, F>(&self, fut: F) -> Result<T, EngineError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        Ok(self.rt.spawn(fut).await?)
    }

    pub fn is_open(&self) -> bool {
        !self.gate.is_closed()
            && self
                .live
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
    }

    /// Whether the trusted quorum provider has been constructed.
    pub fn platform_context_ready(&self) -> bool {
        self.context.is_ready()
    }

    fn not_open(&self) -> EngineError {
        EngineError::NetworkNotOpen(self.network.to_string())
    }

    /// Admits an async operation; `NetworkNotOpen` once close has started.
    /// Public entry points take one guard each; helpers they call do not
    /// (a nested admission behind a waiting close would deadlock).
    pub(crate) async fn enter(&self) -> Result<OpGuard<'_>, EngineError> {
        self.gate.enter().await.ok_or_else(|| self.not_open())
    }

    /// The close preflight's read for a call that signs (`None` for one that
    /// does not), taken once no switch pauses the gate
    /// ([`crate::gate::OpGate::unpaused`]): held while the call is admitted
    /// and runs, so an in-app close refuses meanwhile. `network_not_open` if
    /// the gate closed while it waited.
    pub(crate) async fn signing_preflight(
        &self,
        signing: bool,
    ) -> Result<Option<tokio::sync::RwLockReadGuard<'_, ()>>, EngineError> {
        if !signing {
            return Ok(None);
        }
        if !self.gate.unpaused().await {
            return Err(self.not_open());
        }
        Ok(Some(self.platform.close_preflight.read().await))
    }

    /// Admits a synchronous operation without waiting.
    pub(crate) fn try_enter(&self) -> Result<OpGuard<'_>, EngineError> {
        self.gate.try_enter().ok_or_else(|| self.not_open())
    }

    pub(crate) fn live(&self) -> Result<Live, EngineError> {
        self.live
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| self.not_open())
    }

    pub(crate) fn manager(&self) -> Result<Arc<Manager>, EngineError> {
        Ok(self.live()?.manager)
    }

    /// Runs `fut` on the engine runtime; a panic becomes `EngineError::Internal`.
    pub(crate) async fn on_runtime<T, F>(&self, fut: F) -> Result<T, EngineError>
    where
        F: Future<Output = Result<T, EngineError>> + Send + 'static,
        T: Send + 'static,
    {
        // Tests: a held app.sqlite job and the gate's holds stay with the
        // spawned task.
        #[cfg(test)]
        let fut = {
            let mut fut: TestRun<T> = Box::pin(fut);
            if let Ok(pause) = crate::coins::HOLD_APPDB_JOB.try_with(Arc::clone) {
                fut = Box::pin(crate::coins::HOLD_APPDB_JOB.scope(pause, fut));
            }
            if let Ok(holds) = crate::coins::GATE_HOLDS.try_with(Arc::clone) {
                fut = Box::pin(crate::coins::GATE_HOLDS.scope(holds, fut));
            }
            fut
        };
        self.rt.spawn(fut).await?
    }

    /// The configuration dash-spv is started with: storage under
    /// `<network dir>/spv`, masternode sync on (InstantSend/ChainLock
    /// handling depends on it), restricted to the configured peers when any
    /// were given. Counterpart: `platform_wallet_manager_spv_start`
    /// (rs-platform-wallet-ffi/src/spv.rs:408, config built at :519-551).
    pub(crate) fn spv_config(&self) -> Result<ClientConfig, EngineError> {
        let mut config = ClientConfig::new(self.network.core_network())
            .with_storage_path(self.data_dir.join(SPV_DIR))
            .with_user_agent(crate::tools::USER_AGENT);
        config.enable_masternodes = true;
        // A payment and its InstantSend lock reach the wallet while the
        // filters still sync, not only once they are done.
        config = config.with_mempool_during_sync(true);
        for peer in &self.spv_peers {
            config.add_peer(*peer);
        }
        if !self.spv_peers.is_empty() {
            config = config.with_restrict_to_configured_peers(true);
        }
        if let Some(name) = self.network.devnet_name() {
            config = config.with_devnet(DevnetConfig::new(name));
        }
        config.validate().map_err(EngineError::InvalidConfig)?;
        Ok(config)
    }

    /// Starts dash-spv and spawns its run loop. The caller announces the
    /// start ([`Self::emit_spv_running`]) once `spv_state` reads `Running`.
    pub(crate) async fn start_spv_inner(&self, manager: &Manager) -> Result<(), EngineError> {
        let config = self.spv_config()?;
        // Owner-only before dash-spv creates it with the umask's mode.
        create_owned_dir(&self.data_dir, Path::new(SPV_DIR))?;
        let spv = manager.spv_arc();
        spv.start(config).await?;
        spv.spawn_run_loop();
        self.hub.set_spv_running(true);
        // E0-04 H6 part 2: the catch-up's resume pass, spawned.
        self.leases.spv_started();
        Ok(())
    }

    pub(crate) fn emit_spv_running(&self) {
        self.sink.emit(EngineEvent::SpvStateChanged {
            network: self.network.clone(),
            running: true,
        });
    }

    pub(crate) async fn stop_spv_inner(&self, manager: &Manager) -> Result<(), EngineError> {
        self.leases.spv_stopped();
        self.abort_resends();
        let stopped = manager.spv().stop().await;
        self.hub.set_spv_running(false);
        self.sink.emit(EngineEvent::SpvStateChanged {
            network: self.network.clone(),
            running: false,
        });
        Ok(stopped?)
    }

    /// Ends the resends' acceptance waits ([`Self::resends`]). Nothing is
    /// lost: dash-spv sent the bytes before the wait, which only maps its
    /// verdict to a log line, and the wallet keeps the transaction
    /// unconfirmed, to be resent.
    fn abort_resends(&self) {
        let resends = std::mem::take(&mut *self.resends.lock().unwrap_or_else(|p| p.into_inner()));
        for resend in resends {
            resend.abort();
        }
    }

    /// Closes the session: cancels a running bring-up, waits for admitted
    /// operations (review M1), stops the event pump, shuts the manager down
    /// (SPV, coordinators, persistence adapter drain) and releases the
    /// databases: it returns once the last reference to the wallet database
    /// is gone, within [`PERSISTER_RELEASE`]. Must run on the engine runtime.
    pub(crate) async fn close(&self) {
        self.close_with(Closing::Close).await;
    }

    /// Closes a session whose open failed as [`Self::close`] does, without
    /// unloading its wallets (Q104 104-F1): a database left held stays
    /// held, as [`release_after_failed_open`] describes.
    async fn close_after_failed_open(&self) {
        self.close_with(Closing::FailedOpen).await;
    }

    async fn close_with(&self, closing: Closing) {
        // From here the dispatch hook accepts no spend and allows no re-send:
        // a quit is not refused by a signing call in flight; it revokes the
        // leases and drains the passes before its gate waits for that call.
        self.platform.hook_fence.close();
        // A running bring-up ends first (DASHPAY §3.2). It holds no
        // operation guard, so it is cancelled before waiting for the guards,
        // and again after, for a start admitted meanwhile; then its blocking
        // key work is waited for (bounded). The loops drain once, sealed, in
        // the manager's shutdown below (m1-engine §3.2 interpretations).
        self.platform.deactivate(true).await;
        // E0-04 §8.5: revoke, drain and abort the flows before the gate,
        // which waits for the flows' operation guards.
        self.close_leases().await;
        // No withdrawal recheck, `withdrawal_ready` wait or app database
        // call of theirs outlives the session, and none starts after this
        // (MG-13 P4, P8; a quote still running starts no wait).
        self.masternodes.withdrawals.stop().await;
        // A shielded pass admitted under the leases just dropped ends first,
        // as the lock's (0.1 E1, DEC-1032); none is admitted for a wallet
        // holding a stored spend from here.
        self.drain_shielded_pass().await;
        // Registration flows waiting keyless for a proof hold no lease.
        self.platform.registrations.abort_all();
        let _closing = self.gate.close().await;
        // No vault operation is admitted from here, so no unlock can follow:
        // the data key and every grant go now, not with the session's last
        // reference (an in-app network switch keeps the process running).
        // A bare lock, not the vault gate's: no lock event or lease epoch is
        // needed, as the page reboots after a switch and the leases were
        // revoked above.
        self.vault.lock();
        // A picture toggle's write lands before the session ends (DP4-02 D37).
        self.platform.avatars.settled().await;
        self.platform.avatars.close().await;
        self.platform.deactivate(true).await;
        self.platform.contacts.close().await;
        // A retained session handle keeps no scanned key (DP2-03).
        self.platform.users.scans.close();
        // No proof-expiry timer outlives the session (BL-153 D23).
        self.platform.absence_expiries.abort_all();
        self.masternodes.shutdown();
        let keys_idle = self.platform.key_work_idle().await;
        self.cancel_relock();
        self.coinjoin.shutdown();
        self.governance.shutdown().await;
        let pump = self.pump.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(pump) = pump {
            let _ = pump.stop.send(true);
            if let Err(e) = pump.task.await {
                tracing::warn!(error = %e, "event pump task ended abnormally");
            }
        }
        let taken = self.live.write().unwrap_or_else(|p| p.into_inner()).take();
        let Some(live) = taken else { return };
        let manager = live.manager;
        // The gate is closed, so no resend starts after this.
        self.abort_resends();
        let report = manager.shutdown().await;
        self.hub.set_spv_running(false);
        // A fork no trigger found this session is found now: the store
        // keeps the old chain's checkpoint, which hides it after a reopen.
        {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            for id in wm.list_wallets() {
                if let Some(info) = wm.get_wallet_info(id) {
                    self.hub.detect_truncation(WalletId(*id), &info.core_wallet);
                }
            }
        }
        live.replay.apply_truncations().await;
        if report.all_clean() {
            live.replay.close(&manager, &self.hub).await;
        }
        if report.all_clean() && keys_idle {
            let marker = self.data_dir.join(crate::tools::SESSION_MARKER);
            if let Err(e) = std::fs::remove_file(&marker)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, "could not remove the open-session marker");
            }
        } else {
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code: NoticeCode::UncleanShutdown,
                detail: if keys_idle {
                    format!("{:?}", report.per_worker)
                } else {
                    format!(
                        "{:?}; a DashPay bring-up key read did not end",
                        report.per_worker
                    )
                },
            });
        }
        // Each wallet the gate lets go is unloaded before the manager drops
        // (Q86 (c)). Unloaded or kept, none holds the persister once the
        // manager is shut down cleanly and dropped: the pin's balance handler holds the wallets
        // map through a `Weak` (P16, wallet/core/balance_handler.rs:44-57).
        // `remove_wallet` writes nothing through the persister
        // (manager/wallet_lifecycle.rs:676-900, key-wallet-manager accessors.rs:75),
        // but with the shielded coordinator configured it purges the
        // wallet's rows of `shielded.sqlite` (below).
        // Through the wallets' write fence: the close returns after their
        // fenced writes in flight (private details, the bell's `mark_read`).
        //
        // DEC-1046: a wallet holding a shielded spend whose result is not
        // known is not unloaded, as the pin's `remove_wallet` would delete
        // its stored transition and reservations (`remove_from_manager`).
        // It stays in the sealed manager, which runs nothing more (SPV
        // stopped, every loop quiesced sealed and joined, the payment hooks
        // drained, above), and its persister writes are committed before
        // the persister drops: every `store` commits inline (the persister's default
        // `Immediate` mode, WAL), and a flush here retries any left by a
        // transient failure (a failed flush is logged, not returned). The
        // library wrote the spend's record to the shielded store before its
        // first broadcast (`arm_redrive_before_broadcast`,
        // operations.rs:4445-4515: a failed write sends nothing; its
        // reservations live in memory only, file_store.rs:1126-1132, and end
        // with the session). The rows rehydrate as pending at the next
        // open, in this process or the next. App quit takes this path
        // (`Engine::shutdown`); an in-app close is refused beforehand
        // (`Engine::close_network`), and from that check to this close no
        // signing shielded call starts, no wallet loads (an import or a
        // create stores no spend) and the shielded store does not first
        // open (its open rehydrates the stored spends), so an in-app close
        // keeps no wallet (`PlatformRuntime::close_preflight`,
        // `wallet_loads`, E1 r2 R2-3, r3 m1).
        if closing == Closing::FailedOpen {
            debug_assert!(
                manager.shielded_coordinator().await.is_none(),
                "Q86 F1: no shielded store before the session is handed out"
            );
            let loaded = manager.list_wallet_ids_blocking();
            flush_left_loaded(&live.persister, &loaded).await;
            let persister = Arc::downgrade(&live.persister);
            drop((manager, live.store, live.persister, live.appdb, live.tap));
            drop((live.provenance, live.replay));
            release_after_failed_open(&self.data_dir, persister, !loaded.is_empty()).await;
            self.leases.close_journal().await;
            self.sink.emit(EngineEvent::SessionClosed {
                network: self.network.clone(),
            });
            return;
        }
        let mut kept = Vec::new();
        for id in manager.list_wallet_ids_blocking() {
            let unloaded =
                (self.platform).remove_from_manager(&self.leases, &manager, WalletId(id));
            match unloaded.await {
                Ok(()) => {}
                Err(EngineError::ShieldedSpendUnresolved) => kept.push(id),
                Err(e) => {
                    tracing::warn!(wallet_id = %hex::encode(id), error = %e, "unload on close failed");
                }
            }
        }
        for id in kept {
            tracing::info!(
                wallet_id = %hex::encode(id),
                "kept loaded at close: a shielded payment is still being confirmed"
            );
            let persister = Arc::clone(&live.persister);
            let flushed = tokio::task::spawn_blocking(move || {
                platform_wallet::changeset::PlatformWalletPersistence::flush(&*persister, id)
            })
            .await;
            if !matches!(flushed, Ok(Ok(()))) {
                tracing::warn!(wallet_id = %hex::encode(id), ?flushed, "flush on close failed");
            }
        }
        let persister = Arc::downgrade(&live.persister);
        let appdb = Arc::downgrade(&live.appdb);
        drop(manager);
        drop(live.store);
        drop(live.persister);
        drop(live.appdb);
        drop(live.tap);
        drop(live.provenance);
        drop(live.replay);
        // The close returns once the database is released, so the network
        // reopens in this process (CI 41289102488: `StorageInUse` on a
        // reopen right after a close), a kept wallet's included (Q86 (e)).
        // Bounded: a holder that never lets go keeps the database, and its
        // reopen is refused as before.
        if !persister_released(&persister).await {
            tracing::warn!(
                network = %self.network,
                "the wallet database is still held after the close; reopening it in this process is refused until it is released"
            );
        }
        // A write the pump or the store's tap left on a blocking thread
        // (first-seen times, an unread count) still holds the app database
        // for as long as it runs; the close waits for those too.
        if !released(&appdb).await {
            tracing::warn!(
                network = %self.network,
                "the app database is still held after the close: a reopen in this process is not alone on it, and on Windows its file cannot be replaced until it is released"
            );
        }
        // Per network, so the last to close (§8.5).
        self.leases.close_journal().await;
        self.sink.emit(EngineEvent::SessionClosed {
            network: self.network.clone(),
        });
    }
}

/// How [`NetworkSession::close_with`] ends the session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    /// Every wallet that can be is unloaded first.
    Close,
    /// After a failed open: no wallet is unloaded.
    FailedOpen,
}

/// The SDK options that need parsing, validated once at `open`.
struct SdkOptions {
    ca: Option<Certificate>,
    initial_version: Option<&'static PlatformVersion>,
}

impl SdkOptions {
    fn validate(opts: &SessionOptions) -> Result<Self, EngineError> {
        let ca = opts.ca_cert_path.as_deref().map(load_ca).transpose()?;
        let initial_version = opts
            .initial_protocol_version
            .map(|version| {
                PlatformVersion::get(version).map_err(|e| {
                    EngineError::InvalidConfig(format!("initial protocol version {version}: {e}"))
                })
            })
            .transpose()?;
        Ok(Self {
            ca,
            initial_version,
        })
    }
}

/// The network's vault, its open recorded with it for the startup-order pin.
fn open_vault(
    data_dir: &Path,
    network: key_wallet::Network,
    tag: &str,
    config: VaultConfig,
) -> Result<Vault, EngineError> {
    use crate::store_upgrade::order::{Step, record};
    record(data_dir, Step::Vault);
    Ok(Vault::open(data_dir.join(VAULT_DIR), network, tag, config)?)
}

/// `(wallet_id, name, created_at)` of every named wallet in `app.sqlite`.
type NamedWallets = Vec<(String, String, u64)>;

/// The network's `app.sqlite` (created owner-only first) and its named
/// wallets, the open recorded with it for the startup-order pin.
fn open_app_db(data_dir: &Path) -> Result<(AppDb, NamedWallets), EngineError> {
    use crate::store_upgrade::order::{Step, record};
    record(data_dir, Step::AppDb);
    create_owned_file(data_dir, Path::new(APP_DB_FILE))?;
    create_owned_file(data_dir, Path::new(APP_DB_LOCK_FILE))?;
    let open = || {
        let db = AppDb::open(&data_dir.join(APP_DB_FILE))?;
        let names = db.wallets()?;
        Ok::<_, dw_appdb::AppDbError>((db, names))
    };
    open().map_err(|e| EngineError::Storage(format!("app database: {e}")))
}

/// Reads and checks a PEM CA file the way the SDK will use it. The SDK parses
/// it at its first DAPI call and panics the calling task on a bad one
/// (`rs-dapi-client` `create_channel`: `expect("Failed to set TLS config")`),
/// so a damaged file has to be rejected here.
fn load_ca(path: &Path) -> Result<Certificate, EngineError> {
    let bad_ca = |e: &dyn fmt::Display| {
        EngineError::InvalidConfig(format!("CA certificate {}: {e}", path.display()))
    };
    let pem = std::fs::read(path).map_err(|e| bad_ca(&e))?;
    // Every PEM block must decode (truncated and bad-base64 ones fail here)...
    let certs = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| match e {
            PemError::MissingSectionEnd { .. } => {
                bad_ca(&"truncated: a CERTIFICATE block has no END line")
            }
            PemError::Base64Decode(detail) => {
                bad_ca(&format_args!("a CERTIFICATE block is not base64: {detail}"))
            }
            other => bad_ca(&format_args!("not a PEM certificate: {other:?}")),
        })?;
    if certs.is_empty() {
        return Err(bad_ca(&"no -----BEGIN CERTIFICATE----- block"));
    }
    // ...and be a certificate: tonic silently drops a block whose DER does not
    // parse, which would only show up later as an unknown issuer.
    for (index, cert) in certs.iter().enumerate() {
        webpki::anchor_from_trusted_cert(cert)
            .map_err(|e| bad_ca(&format_args!("certificate {} of the file: {e}", index + 1)))?;
    }
    let cert = Certificate::from_pem(pem);
    // Last, the exact call the SDK makes with it. Cheap, and it holds even if
    // tonic's parsing changes.
    Endpoint::from_static("https://localhost")
        .tls_config(ClientTlsConfig::new().ca_certificate(cert.clone()))
        .map_err(|e| bad_ca(&e))?;
    Ok(cert)
}

/// The developer toggle (`trust.trusted_only`). A read error leaves it off:
/// SPV keeps being compared. A release build ignores it (H-09): a data
/// root a developer build turned it on in does not carry it over.
fn read_trusted_only(appdb: &AppDb, allowed: bool) -> bool {
    match appdb.setting(dw_appdb::GLOBAL_SCOPE, trust::TRUSTED_ONLY_KEY) {
        Ok(v) if !allowed => {
            if v.as_deref() == Some("1") {
                tracing::warn!("the trusted-only toggle is set but ignored in a release build");
            }
            false
        }
        Ok(v) => v.as_deref() == Some("1"),
        Err(e) => {
            tracing::warn!(error = %e, "could not read the trusted-only toggle; it stays off");
            false
        }
    }
}

fn build_sdk(
    network: &DashNetwork,
    opts: &SessionOptions,
    sdk_options: SdkOptions,
    context: SharedContext,
    observer: Option<dash_sdk::sdk::ProtocolVersionObserver>,
) -> Result<dash_sdk::Sdk, EngineError> {
    #[cfg(test)]
    if let Some(sdk) = &opts.sdk {
        return Ok(sdk.clone());
    }
    let builder = if opts.dapi_addresses.is_empty() {
        match network {
            DashNetwork::Mainnet => SdkBuilder::new_mainnet(),
            DashNetwork::Testnet => SdkBuilder::new_testnet(),
            DashNetwork::Devnet { .. } | DashNetwork::Regtest => {
                return Err(EngineError::InvalidConfig(format!(
                    "{network} has no default DAPI addresses; pass dapi_addresses"
                )));
            }
        }
    } else {
        let list = AddressList::from_str(&opts.dapi_addresses.join(","))
            .map_err(|e| EngineError::InvalidArgument(format!("bad DAPI address list: {e}")))?;
        SdkBuilder::new(list).with_network(network.core_network())
    };
    let mut builder = builder.with_context_provider(context);
    if let Some(ca) = sdk_options.ca {
        builder = builder.with_ca_certificate(ca);
    }
    if let Some(version) = sdk_options.initial_version {
        builder = builder.with_initial_version(version);
    }
    if let Some(observer) = observer {
        builder = builder.with_protocol_version_observer(observer);
    }
    // Tests only: a session can hold a DAPI attempt open (`Some(0)` is
    // "no deadline" in rs-dapi-client) or cut it sooner than the default.
    #[cfg(test)]
    if let Some(timeout) = opts.dapi_request_timeout {
        builder = builder.with_settings(dash_sdk::RequestSettings {
            connect_timeout: None,
            timeout: Some(timeout),
            retries: None,
            ban_failed_address: None,
            ban_busy_address: None,
            max_decoding_message_size: None,
        });
    }
    Ok(builder.build()?)
}

async fn init_quorum_context(
    context: &Arc<LazyTrustedContext>,
    network: &DashNetwork,
    sink: &dyn EventSink,
) -> Result<u64, EngineError> {
    let ctx = Arc::clone(context);
    let started = Instant::now();
    let init = tokio::task::spawn_blocking(move || ctx.get_or_try_init(true)).await?;
    let quorum_init_ms = started.elapsed().as_millis() as u64;
    match init {
        Ok(provider) => {
            tokio::spawn(async move {
                if let Err(e) = provider.update_quorum_caches().await {
                    tracing::warn!(error = %e, "quorum prefetch failed; continuing");
                }
            });
        }
        Err(detail) => sink.emit(EngineEvent::Notice {
            network: Some(network.clone()),
            code: NoticeCode::PlatformContextUnavailable,
            detail,
        }),
    }
    tracing::info!(elapsed_ms = quorum_init_ms, "startup quorum-context-init");
    Ok(quorum_init_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_id_hex_round_trip() {
        let id = WalletId([0xab; 32]);
        let s = id.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(s.parse::<WalletId>().unwrap(), id);
        assert!("abcd".parse::<WalletId>().is_err());
        assert!("zz".repeat(32).parse::<WalletId>().is_err());
        // The contract's form is lower case only.
        assert!("AB".repeat(32).parse::<WalletId>().is_err());
    }

    struct NoEvents;

    impl EventSink for NoEvents {
        fn emit(&self, _: EngineEvent) {}
    }

    fn sdk_with(opts: SessionOptions) -> Result<dash_sdk::Sdk, EngineError> {
        let trusted = Arc::new(LazyTrustedContext::new(
            dashcore::Network::Regtest,
            None,
            Some("http://127.0.0.1:1".into()),
        ));
        let trust = Arc::new(TrustProvider::new(
            trust::DEFAULT_MODE,
            Arc::new(Arc::clone(&trusted)),
            Arc::new(NoEvents),
            DashNetwork::Regtest,
        ));
        let context = SharedContext { trusted, trust };
        let opts = SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            ..opts
        };
        let sdk_options = SdkOptions::validate(&opts)?;
        build_sdk(&DashNetwork::Regtest, &opts, sdk_options, context, None)
    }

    #[test]
    fn initial_protocol_version_seeds_the_sdk() {
        let sdk = sdk_with(SessionOptions {
            initial_protocol_version: Some(PlatformVersion::first().protocol_version),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            sdk.protocol_version_number(),
            PlatformVersion::first().protocol_version
        );
        let err = sdk_with(SessionOptions {
            initial_protocol_version: Some(u32::MAX),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, EngineError::InvalidConfig(_)), "{err:?}");
    }

    #[test]
    fn missing_ca_certificate_is_a_config_error() {
        let err = sdk_with(SessionOptions {
            ca_cert_path: Some("/nonexistent/dwd-ca.pem".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, EngineError::InvalidConfig(_)), "{err:?}");
    }

    /// A self-signed `CA:FALSE` certificate (CN=localhost), as a dashmate
    /// gateway serves; validity is not checked at load.
    const GOOD_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDHzCCAgegAwIBAgIUC0UrfHVX7RAdqsyyadadvEUJ9ZMwDQYJKoZIhvcNAQEL\n\
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MTAwODE1NTU0MloXDTI2MTAx\n\
MDE1NTU0MlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\n\
AAOCAQ8AMIIBCgKCAQEAzLIVPB7Jw8N3Hl0OwSMiZA9r1gdBGJ7rKFTNtCwZkLU5\n\
uMm1w9f1YHQcYAWCnLeKBxgvgQrHVGifhrXlVttyJnhiZiE15huWyE1a7AON/7Nx\n\
vOpWy1j2cidZMou6r2UiBLJ33fjpWyXEWbshgZk3SNL7NSJ+2iANqCcKL2TaInfZ\n\
v081VzcHMyheMQpxvncR36n0GjvtV9HH7ApX/Mih/DXnkhG6LTvGqiTRKyPcPsHb\n\
7gLUcJYbQbd4aDdlkoQSJfc+zs3N6Fb2CsZM1zTHZBZ21brxWVS5yuqVVk7EEd+A\n\
KHm8kRNZMnu2NPAg8ATcpdZ7e3Ns5yTZx1Jm+efCcwIDAQABo2kwZzAdBgNVHQ4E\n\
FgQUfJ8EmVR6rAgSGiRcGql6Qn8lUyUwHwYDVR0jBBgwFoAUfJ8EmVR6rAgSGiRc\n\
Gql6Qn8lUyUwGgYDVR0RBBMwEYIJbG9jYWxob3N0hwR/AAABMAkGA1UdEwQCMAAw\n\
DQYJKoZIhvcNAQELBQADggEBAJWftMJtNa+9Xn29ESCmkq7D7mpDUemmPi1A6QTS\n\
HyKU1JvXWyiqbwe+TK+LmTR5vyzgWY29pMFkuZ44hTVF8XlIIMt7OU9u8R9Msq8/\n\
M3DLDx5xFLWpl53Chu0EkZOrq3Zo/hC/OLGbLSQIElqa/t/+CByg2dT4OtmG2KMb\n\
IYyg/zrxLd6ckSamTS/peM81T5R92Qa9PnDGusYDGWurZEEm6ZQtCVmtu7joC+NE\n\
J62k8Vm0RL24O48uZ1hN5XHIBBu8bVYm3zm3k+GS2dLwZna3ciWpEjqwXMZyUiBC\n\
PRihiNYRunWtfhIwgcXNS8k8Aw/lDZNOwwt4ykNeDyMuBUM=\n\
-----END CERTIFICATE-----\n\
";

    fn ca_error(contents: &[u8]) -> EngineError {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, contents).unwrap();
        sdk_with(SessionOptions {
            ca_cert_path: Some(path),
            ..Default::default()
        })
        .unwrap_err()
    }

    #[test]
    fn a_valid_ca_certificate_is_accepted() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("ca.pem");
        // A chain: the certificate twice, with a comment line between.
        std::fs::write(&path, format!("{GOOD_CA_PEM}# again\n{GOOD_CA_PEM}")).unwrap();
        sdk_with(SessionOptions {
            ca_cert_path: Some(path),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn a_damaged_ca_file_is_a_config_error() {
        // Each of these used to pass the open-time check and panic an engine
        // task at the first DAPI call (or, for the last, trust nothing).
        let cases: [(&str, Vec<u8>); 7] = [
            ("empty", Vec::new()),
            (
                "key, not a certificate",
                b"-----BEGIN PRIVATE KEY-----\n".to_vec(),
            ),
            ("DER bytes", vec![0x30, 0x82, 0x03, 0x1f, 0x00, 0xff]),
            (
                "truncated mid-block",
                GOOD_CA_PEM.as_bytes()[..300].to_vec(),
            ),
            (
                "no END line",
                b"-----BEGIN CERTIFICATE-----\nZm9vYmFy\n".to_vec(),
            ),
            (
                "bad base64",
                b"-----BEGIN CERTIFICATE-----\n!!!!not base64!!!!\n-----END CERTIFICATE-----\n"
                    .to_vec(),
            ),
            (
                "base64 that is not a certificate",
                b"-----BEGIN CERTIFICATE-----\nZm9vYmFyZm9vYmFy\n-----END CERTIFICATE-----\n"
                    .to_vec(),
            ),
        ];
        for (name, contents) in cases {
            let err = ca_error(&contents);
            assert!(
                matches!(err, EngineError::InvalidConfig(_)),
                "{name}: {err:?}"
            );
        }
        // A good certificate followed by a truncated one is damaged too.
        let mut chain = GOOD_CA_PEM.as_bytes().to_vec();
        chain.extend_from_slice(&GOOD_CA_PEM.as_bytes()[..300]);
        let err = ca_error(&chain);
        assert!(
            matches!(err, EngineError::InvalidConfig(_)),
            "chain: {err:?}"
        );
    }

    #[test]
    fn created_wallet_debug_redacts_mnemonic() {
        let w = CreatedWallet {
            wallet_id: WalletId([1; 32]),
            mnemonic: Zeroizing::new("secret words".into()),
        };
        assert!(!format!("{w:?}").contains("secret"));
    }

    #[tokio::test]
    async fn quorum_context_init_excludes_notice_delivery_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct SlowNoticeSink(AtomicUsize);
        impl EventSink for SlowNoticeSink {
            fn emit(&self, event: EngineEvent) {
                if matches!(event, EngineEvent::Notice { .. }) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        }

        let context = Arc::new(LazyTrustedContext::new(
            dashcore::Network::Regtest,
            None,
            Some("not a valid url".into()),
        ));
        let sink = SlowNoticeSink(AtomicUsize::new(0));
        let total = Instant::now();
        let init_ms = init_quorum_context(&context, &DashNetwork::Regtest, &sink)
            .await
            .unwrap();
        let total_ms = total.elapsed().as_millis() as u64;
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
        assert!(
            total_ms >= init_ms + 300,
            "quorum init duration included slow notice delivery: {init_ms} of {total_ms} ms"
        );
    }
}
