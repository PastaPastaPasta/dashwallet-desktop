use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use dash_sdk::sdk::AddressList;
use dash_sdk::SdkBuilder;
use dash_spv::{ClientConfig, DevnetConfig};
use key_wallet::mnemonic::{Language, Mnemonic};
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use platform_wallet::PlatformWalletManager;
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use tokio::runtime::Handle;
use zeroize::Zeroizing;

use crate::context::{LazyTrustedContext, SharedContext};
use crate::events::SessionEventBridge;
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NoticeCode};

type Manager = PlatformWalletManager<SqlitePersister>;

/// File name of the platform-wallet SQLite database inside a network dir.
pub const WALLET_DB_FILE: &str = "wallet.sqlite";
/// dash-spv storage directory inside a network dir.
pub const SPV_DIR: &str = "spv";

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
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s)
            .map_err(|e| EngineError::InvalidArgument(format!("wallet id is not hex: {e}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EngineError::InvalidArgument("wallet id must be 32 bytes".into()))?;
        Ok(WalletId(arr))
    }
}

/// Core balance buckets in duffs, read from platform-wallet's lock-free
/// `WalletBalance` atomics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletSummary {
    pub wallet_id: WalletId,
    pub balances: WalletBalances,
}

/// Result of creating a fresh wallet.
///
/// TODO(vault): the mnemonic is handed back to the caller and stored nowhere.
/// When dw-vault lands, `create_wallet` must write the vault record, fsync, read
/// it back and compare before registering the wallet (DESIGN-opus §1.8 "seed
/// safety ordering"), and the phrase must stop crossing the FFI here.
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
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// DAPI endpoints (`https://host:port`). Empty = SDK defaults (mainnet/testnet only).
    pub dapi_addresses: Vec<String>,
    /// Override for the trusted quorum service. Must be https on mainnet/testnet.
    pub quorum_url: Option<String>,
    /// SPV peers (`ip:port`). When non-empty, SPV connects to these only.
    pub spv_peers: Vec<String>,
}

/// One open network: SDK + PlatformWalletManager<SqlitePersister>.
pub struct NetworkSession {
    network: DashNetwork,
    data_dir: PathBuf,
    rt: Handle,
    sink: Arc<dyn EventSink>,
    context: Arc<LazyTrustedContext>,
    spv_peers: Vec<SocketAddr>,
    /// `None` once closed. Taken out on close so the persister (and its
    /// process-wide open-path claim) is released even while hosts still hold
    /// `Arc<NetworkSession>`.
    manager: RwLock<Option<Arc<Manager>>>,
}

impl NetworkSession {
    /// Opens the session. Must run on the engine runtime: the manager spawns
    /// its persistence adapter task with `tokio::spawn`.
    pub(crate) async fn open(
        network: DashNetwork,
        data_dir: PathBuf,
        sink: Arc<dyn EventSink>,
        opts: SessionOptions,
    ) -> Result<Arc<Self>, EngineError> {
        network.validate()?;
        let spv_peers = opts
            .spv_peers
            .iter()
            .map(|p| {
                p.parse::<SocketAddr>()
                    .map_err(|e| EngineError::InvalidArgument(format!("bad SPV peer {p:?}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        create_private_dir(&data_dir)?;

        let context = Arc::new(LazyTrustedContext::new(
            network.core_network(),
            network.devnet_name().map(str::to_string),
            opts.quorum_url.clone(),
        ));
        // First attempt off the async workers: construction does a blocking DNS check.
        let ctx = Arc::clone(&context);
        match tokio::task::spawn_blocking(move || ctx.get_or_try_init(true)).await? {
            Ok(provider) => {
                // Prefetch quorums, as the iOS SDK does after building a trusted SDK.
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

        let sdk = build_sdk(&network, &opts, Arc::clone(&context))?;

        let db_path = data_dir.join(WALLET_DB_FILE);
        let persister = tokio::task::spawn_blocking(move || {
            SqlitePersister::open(SqlitePersisterConfig::new(db_path))
        })
        .await??;

        let bridge = Arc::new(SessionEventBridge::new(network.clone(), Arc::clone(&sink)));
        let manager = Arc::new(PlatformWalletManager::new(
            Arc::new(sdk),
            Arc::new(persister),
            bridge,
        ));
        if let Err(e) = manager.load_from_persistor().await {
            let report = manager.shutdown().await;
            if !report.all_clean() {
                tracing::warn!(?report, "manager shutdown after failed load was not clean");
            }
            return Err(e.into());
        }

        Ok(Arc::new(Self {
            network,
            data_dir,
            rt: Handle::current(),
            sink,
            context,
            spv_peers,
            manager: RwLock::new(Some(manager)),
        }))
    }

    pub fn network(&self) -> &DashNetwork {
        &self.network
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn is_open(&self) -> bool {
        self.manager
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Whether the trusted quorum provider has been constructed.
    pub fn platform_context_ready(&self) -> bool {
        self.context.is_ready()
    }

    fn manager(&self) -> Result<Arc<Manager>, EngineError> {
        self.manager
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| EngineError::NetworkNotOpen(self.network.to_string()))
    }

    /// Runs `fut` on the engine runtime; a panic becomes `EngineError::Internal`.
    async fn on_runtime<T, F>(&self, fut: F) -> Result<T, EngineError>
    where
        F: Future<Output = Result<T, EngineError>> + Send + 'static,
        T: Send + 'static,
    {
        self.rt.spawn(fut).await?
    }

    /// Creates a wallet from a fresh BIP39 English mnemonic (12 or 24 words).
    ///
    /// Counterpart: platform-wallet-ffi `platform_wallet_manager_create_wallet_from_mnemonic`
    /// (rs-platform-wallet-ffi/src/manager.rs:599).
    /// The birth height is left to platform-wallet: SPV tip when running, else
    /// the latest checkpoint (a new wallet has no history to scan).
    pub async fn create_wallet(
        self: &Arc<Self>,
        word_count: u8,
    ) -> Result<CreatedWallet, EngineError> {
        if word_count != 12 && word_count != 24 {
            return Err(EngineError::InvalidArgument(format!(
                "word count must be 12 or 24, got {word_count}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let mnemonic = Mnemonic::generate(word_count as usize, Language::English)
                .map_err(|e| EngineError::Wallet(format!("mnemonic generation failed: {e}")))?;
            let phrase = Zeroizing::new(mnemonic.phrase());
            drop(mnemonic);
            let wallet_id = this.register_phrase(&phrase, None).await?;
            Ok(CreatedWallet {
                wallet_id,
                mnemonic: phrase,
            })
        })
        .await
    }

    /// Restores a wallet from an existing mnemonic. `birth_height` = `Some(0)`
    /// scans from genesis; `None` uses platform-wallet's default. Emits
    /// `WalletCreated` like `create_wallet` (the event means "registered").
    ///
    /// Counterpart: `platform_wallet_manager_create_wallet_from_mnemonic_with_birth_height`
    /// (rs-platform-wallet-ffi/src/manager.rs:630).
    ///
    /// TODO(vault): Core's BIP39 quirks (weak checksum, no NFKD) and passphrase
    /// support belong to dw-vault/dw-compat; this accepts only valid BIP39 phrases.
    pub async fn import_wallet(
        self: &Arc<Self>,
        phrase: Zeroizing<String>,
        birth_height: Option<u32>,
    ) -> Result<WalletId, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move { this.register_phrase(&phrase, birth_height).await })
            .await
    }

    async fn register_phrase(
        &self,
        phrase: &str,
        birth_height: Option<u32>,
    ) -> Result<WalletId, EngineError> {
        let manager = self.manager()?;
        let wallet = manager
            .create_wallet_from_mnemonic(
                phrase,
                self.network.core_network(),
                WalletAccountCreationOptions::Default,
                birth_height,
            )
            .await?;
        let wallet_id = WalletId(wallet.wallet_id());
        self.sink.emit(EngineEvent::WalletCreated {
            network: self.network.clone(),
            wallet_id,
        });
        Ok(wallet_id)
    }

    /// Registered wallets with their balance buckets. Wait-free read
    /// (platform-wallet keeps the wallet map in an `ArcSwap`).
    pub fn list_wallets(&self) -> Result<Vec<WalletSummary>, EngineError> {
        let manager = self.manager()?;
        Ok(manager
            .list_wallet_ids_blocking()
            .into_iter()
            .filter_map(|id| {
                manager.get_wallet_blocking(&id).map(|w| WalletSummary {
                    wallet_id: WalletId(id),
                    balances: balances_of(&w),
                })
            })
            .collect())
    }

    pub fn balances(&self, wallet_id: &WalletId) -> Result<WalletBalances, EngineError> {
        let manager = self.manager()?;
        let wallet = manager
            .get_wallet_blocking(&wallet_id.0)
            .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))?;
        Ok(balances_of(&wallet))
    }

    /// Starts dash-spv for this network (storage under `<network dir>/spv`) and
    /// spawns its run loop. Masternode sync stays on: InstantSend/ChainLock
    /// handling depends on it. Counterpart: `platform_wallet_manager_spv_start`
    /// (rs-platform-wallet-ffi/src/spv.rs:408, config built at :519-551).
    pub async fn start_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let manager = this.manager()?;
            let mut config = ClientConfig::new(this.network.core_network())
                .with_storage_path(this.data_dir.join(SPV_DIR));
            config.enable_masternodes = true;
            for peer in &this.spv_peers {
                config.add_peer(*peer);
            }
            if !this.spv_peers.is_empty() {
                config = config.with_restrict_to_configured_peers(true);
            }
            if let Some(name) = this.network.devnet_name() {
                config = config.with_devnet(DevnetConfig::new(name));
            }
            config.validate().map_err(EngineError::InvalidConfig)?;
            let spv = manager.spv_arc();
            spv.start(config).await?;
            spv.spawn_run_loop();
            this.sink.emit(EngineEvent::SpvStateChanged {
                network: this.network.clone(),
                running: true,
            });
            Ok(())
        })
        .await
    }

    /// Stops dash-spv. Idempotent.
    pub async fn stop_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let manager = this.manager()?;
            manager.spv().stop().await?;
            this.sink.emit(EngineEvent::SpvStateChanged {
                network: this.network.clone(),
                running: false,
            });
            Ok(())
        })
        .await
    }

    pub fn spv_running(&self) -> Result<bool, EngineError> {
        Ok(self.manager()?.spv().is_started())
    }

    /// Shuts the manager down (SPV, coordinators, persistence adapter drain)
    /// and releases the persister. Must run on the engine runtime.
    pub(crate) async fn close(&self) {
        let taken = self
            .manager
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(manager) = taken else { return };
        let report = manager.shutdown().await;
        if !report.all_clean() {
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code: NoticeCode::UncleanShutdown,
                detail: format!("{:?}", report.per_worker),
            });
        }
        // Unload every wallet before dropping the manager. Upstream has a
        // reference cycle that otherwise leaks each registered wallet — and
        // with it the persister and its process-wide open-path claim, so the
        // database could never be reopened in this process:
        //   PlatformWallet -> SpvBroadcaster -> Arc<SpvRuntime>
        //     (rs-platform-wallet/src/broadcaster.rs:219-226, manager/wallet_lifecycle.rs:557)
        //   SpvRuntime -> PlatformEventManager -> BalanceUpdateHandler
        //     -> Arc<ArcSwap<wallets map>> -> Arc<PlatformWallet>
        //     (manager/mod.rs:522, wallet/core/balance_handler.rs:43-45)
        // `remove_wallet` is an in-memory unload (no persister writes:
        // manager/wallet_lifecycle.rs:676-900, key-wallet-manager accessors.rs:75).
        for id in manager.list_wallet_ids_blocking() {
            if let Err(e) = manager.remove_wallet(&id).await {
                tracing::warn!(wallet_id = %hex::encode(id), error = %e, "unload on close failed");
            }
        }
        drop(manager);
        self.sink.emit(EngineEvent::SessionClosed {
            network: self.network.clone(),
        });
    }
}

fn balances_of(wallet: &platform_wallet::wallet::PlatformWallet) -> WalletBalances {
    let b = wallet.balance();
    WalletBalances {
        confirmed: b.confirmed(),
        unconfirmed: b.unconfirmed(),
        immature: b.immature(),
        locked: b.locked(),
        total: b.total(),
    }
}

fn build_sdk(
    network: &DashNetwork,
    opts: &SessionOptions,
    context: Arc<LazyTrustedContext>,
) -> Result<dash_sdk::Sdk, EngineError> {
    let builder = if opts.dapi_addresses.is_empty() {
        match network {
            DashNetwork::Mainnet => SdkBuilder::new_mainnet(),
            DashNetwork::Testnet => SdkBuilder::new_testnet(),
            DashNetwork::Devnet { .. } | DashNetwork::Regtest => {
                return Err(EngineError::InvalidConfig(format!(
                    "{network} has no default DAPI addresses; pass dapi_addresses"
                )))
            }
        }
    } else {
        let list = AddressList::from_str(&opts.dapi_addresses.join(","))
            .map_err(|e| EngineError::InvalidArgument(format!("bad DAPI address list: {e}")))?;
        SdkBuilder::new(list).with_network(network.core_network())
    };
    Ok(builder
        .with_context_provider(SharedContext(context))
        .build()?)
}

/// Creates `dir` (and parents) and restricts it to the current user, because
/// SqlitePersister refuses databases under group/world-writable directories.
pub(crate) fn create_private_dir(dir: &Path) -> Result<(), EngineError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
    }

    #[test]
    fn created_wallet_debug_redacts_mnemonic() {
        let w = CreatedWallet {
            wallet_id: WalletId([1; 32]),
            mnemonic: Zeroizing::new("secret words".into()),
        };
        assert!(!format!("{w:?}").contains("secret"));
    }
}
