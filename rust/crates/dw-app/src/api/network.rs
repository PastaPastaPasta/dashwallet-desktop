//! Opening and closing networks (`Engine`), and the networks with data.
//!
//! The window shows one network at a time (E0-13 D-row "one active
//! session"): `open_network` makes a network the active one, closing the
//! one before it, and every session command acts on the active one.
//! `switch_network` moves the window itself to the other network (row 74).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dw_engine::network_choice::{self, NetworkChoice};
use dw_engine::platform::{LeaseStateView, LeaseView};
use dw_engine::{CloseRefused, DashNetwork, NetworkDataInfo, NetworkSession, SessionOptions};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::{LockStatus, NetworkInfo, mirror_enum, parse_network};
use crate::error::{
    AppCode, AppError, CloseRefusalReason, EngineCode, ErrorData, StoreLine, UpdateBusyReason,
};
use crate::ipc::commands;
use crate::launch::LaunchOptions;
use crate::state::AppState;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    ("Engine::new", "internal: the app creates the engine at start-up"),
    ("Engine::config", "internal: the data root is the launch's"),
    ("Engine::network_dir", "internal: a path the engine resolves; `nodeInfo.dataDir` shows the active network's"),
    ("Engine::session", "internal: `open_network` returns the open session"),
    ("Engine::shutdown", "internal: runs when the app exits"),
    ("Engine::block_on", "internal: for synchronous hosts"),
    ("NetworkSession::network", "internal: `open_network` returns it"),
    ("NetworkSession::data_dir", "internal: a path the engine owns"),
    ("NetworkSession::avatars_dir", "internal: avatars cross as bytes"),
    ("NetworkSession::is_open", "internal: the binding drops a session it closes"),
    ("NetworkSession::platform_context_ready", "internal: `warnings` reports it (`platformContextUnavailable`), with the engine's notice"),
];

/// How to reach a network. Every field may be left out of the JSON; the
/// launch flags (`--dapi`, `--spv-peer`, `--ca-cert`, …) fill those the
/// renderer leaves out. On mainnet and testnet the endpoint, quorum and CA
/// fields must be left out (launch flags only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionSettings {
    /// DAPI endpoints (`https://host:port`). Empty: the SDK's defaults
    /// (mainnet and testnet only).
    pub dapi_addresses: Vec<String>,
    /// The trusted quorum service; https on mainnet and testnet.
    pub quorum_url: Option<String>,
    /// SPV peers (`ip:port`). Non-empty: SPV connects to these only.
    pub spv_peers: Vec<String>,
    /// A PEM CA certificate the SDK trusts for DAPI TLS besides the system
    /// roots (a dashmate devnet gateway's self-signed certificate).
    pub ca_cert_path: Option<String>,
    /// The protocol version the SDK starts at; auto-detection ratchets it
    /// up. `null`: the network's floor.
    pub initial_protocol_version: Option<u32>,
    /// The chain has no Platform (a plain dashd regtest).
    pub no_platform: bool,
}

impl SessionSettings {
    /// On mainnet and testnet the trust settings (DAPI and SPV endpoints,
    /// the quorum service, an extra CA) come from the launch flags only: a
    /// compromised renderer must not point the wallet at its own quorum
    /// keys or pin SPV to one peer. Other networks take them from the
    /// renderer (a devnet's settings file, U-01).
    fn check_trust(&self, network: &DashNetwork) -> Result<(), AppError> {
        let public = matches!(network, DashNetwork::Mainnet | DashNetwork::Testnet);
        let SessionSettings {
            dapi_addresses,
            quorum_url,
            spv_peers,
            ca_cert_path,
            initial_protocol_version: _,
            no_platform: _,
        } = self;
        let trust = !dapi_addresses.is_empty()
            || quorum_url.is_some()
            || !spv_peers.is_empty()
            || ca_cert_path.is_some();
        if public && trust {
            return Err(AppError::invalid_argument(
                "on mainnet and testnet, DAPI and SPV endpoints, the quorum service and CA \
                 certificates are set by launch flags only",
            ));
        }
        Ok(())
    }

    /// The engine's options: these settings over the launch's.
    fn over(self, launch: &LaunchOptions) -> SessionOptions {
        let SessionSettings {
            dapi_addresses,
            quorum_url,
            spv_peers,
            ca_cert_path,
            initial_protocol_version,
            no_platform,
        } = self;
        let or = |v: Vec<String>, d: &Vec<String>| if v.is_empty() { d.clone() } else { v };
        SessionOptions {
            dapi_addresses: or(dapi_addresses, &launch.dapi_addresses),
            quorum_url: quorum_url.or_else(|| launch.quorum_url.clone()),
            spv_peers: or(spv_peers, &launch.spv_peers),
            ca_cert_path: ca_cert_path
                .map(PathBuf::from)
                .or_else(|| launch.ca_cert_path.clone()),
            initial_protocol_version: initial_protocol_version
                .or(launch.initial_protocol_version),
            no_platform: no_platform || launch.no_platform,
            // Launch flag only, regtest only (MG-02 D80, MG-09 D28); elsewhere
            // governance resolves collaterals through DAPI.
            collateral_rpc: launch.collateral_rpc.clone(),
        }
    }
}

/// A network the user chooses between (the developer networks are launch
/// flags only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ChosenNetwork {
    Mainnet,
    Testnet,
}

impl From<NetworkChoice> for ChosenNetwork {
    fn from(c: NetworkChoice) -> Self {
        match c {
            NetworkChoice::Mainnet => ChosenNetwork::Mainnet,
            NetworkChoice::Testnet => ChosenNetwork::Testnet,
        }
    }
}

impl From<ChosenNetwork> for NetworkChoice {
    fn from(c: ChosenNetwork) -> Self {
        match c {
            ChosenNetwork::Mainnet => NetworkChoice::Mainnet,
            ChosenNetwork::Testnet => NetworkChoice::Testnet,
        }
    }
}

/// The network choice of onboarding and Settings (release 0.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkChoiceView {
    /// The saved choice; mainnet when none was saved.
    pub chosen: ChosenNetwork,
    /// A choice was saved (`false`: first run, onboarding may ask).
    pub saved: bool,
    /// The network the window shows (`launchInfo.network`).
    pub active: NetworkInfo,
    /// The window stays on its launch network (`--network`, or a flag of
    /// one network: [`LaunchOptions::pinned`]): a choice is saved for a
    /// later start without those flags, and the window does not switch.
    pub forced_by_flag: bool,
}

impl NetworkChoiceView {
    fn read(state: &AppState) -> Self {
        let saved = network_choice::read_network_choice(&state.data_root);
        NetworkChoiceView {
            chosen: saved.unwrap_or_default().into(),
            saved: saved.is_some(),
            active: NetworkInfo::from(&state.window_network()),
            forced_by_flag: state.launch.pinned(),
        }
    }
}

/// What `switch_network` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSwitch {
    /// The window's network, open; `null` when the window is pinned and the
    /// choice was only saved.
    pub opened: Option<OpenedNetwork>,
    pub choice: NetworkChoiceView,
}

/// The active network after `openNetwork`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenedNetwork {
    pub network: NetworkInfo,
    pub lock: LockStatus,
    /// The store upgrade's notice for this session, shown once over the
    /// wallet and dismissed there: `Some` when its open upgraded the wallet
    /// data without a before-upgrade copy it can restore
    /// ([`NetworkSession::upgrade_notice`]). Every call while the session is
    /// open returns it.
    pub notice: Option<StoreLine>,
}

impl OpenedNetwork {
    fn of(session: &NetworkSession, network: NetworkInfo) -> Self {
        Self {
            network,
            lock: session.vault().status().into(),
            notice: session.upgrade_notice().map(|l| StoreLine {
                key: l.key.into(),
                text: l.text.into(),
            }),
        }
    }
}

/// A network with data under the data root (IOS-009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkData {
    pub network: NetworkInfo,
    pub directory: String,
    pub has_wallet_state: bool,
    pub has_vault: bool,
    pub has_os_store_key: bool,
}

impl From<NetworkDataInfo> for NetworkData {
    fn from(n: NetworkDataInfo) -> Self {
        let NetworkDataInfo {
            network,
            directory,
            has_wallet_state,
            has_vault,
            has_os_store_key,
        } = n;
        NetworkData {
            network: NetworkInfo::from(&network),
            directory,
            has_wallet_state,
            has_vault,
            has_os_store_key,
        }
    }
}

/// What a restored network waits for before its funds actions run again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RestoreStatus {
    /// Empty: the gate opens at the engine's next look.
    pub waiting: Vec<RestoreWait>,
    /// The loaded wallets whose history has not loaded (`historyLoad`).
    pub history_wallets: Vec<String>,
    /// The last attempt to open the gate failed reading or writing its
    /// mark; it is tried again after a growing delay, at once on
    /// `restoreRetry`.
    pub clear_failed: bool,
}

/// One thing a restored network waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RestoreWait {
    /// Core has not caught up with the network.
    CoreSync,
    /// Platform's first check since the network opened has not completed;
    /// a network opened with `noPlatform` never waits for it.
    PlatformPass,
    /// A wallet's history has not loaded.
    HistoryLoad,
}

mirror_enum!(dw_engine::RestoreWait => RestoreWait { CoreSync, PlatformPass, HistoryLoad });

impl From<dw_engine::RestoreStatus> for RestoreStatus {
    fn from(s: dw_engine::RestoreStatus) -> Self {
        RestoreStatus {
            waiting: s.waiting.into_iter().map(Into::into).collect(),
            history_wallets: s.history_wallets.iter().map(ToString::to_string).collect(),
            clear_failed: s.clear_failed,
        }
    }
}

commands! {
    /// Makes `network` (a directory name; `null`: the window's network,
    /// the launch network until `switchNetwork` opens another) the active
    /// one, opening it if needed and closing the network that was active,
    /// with its draft handles. When it is already active, the open session
    /// is kept and `settings` are ignored. Opening the window's network
    /// starts SPV on mainnet and testnet unless the app was launched with
    /// `--no-sync`, and on another network with `--sync`; a failed start
    /// leaves the network open, readable offline.
    #[facade(
        "Engine::open_network",
        "Engine::close_network",
        "NetworkSession::upgrade_notice"
    )]
    fn open_network(
        state,
        network: Option<String>,
        settings: Option<SessionSettings>,
    ) -> OpenedNetwork {
        // Before the wait and before a retry: "moon" is `invalid_argument`
        // even while the engine is still starting, and it must not start
        // another build after a failure.
        let requested = network.as_deref().map(parse_network).transpose()?;
        state.retry_engine_if_failed();
        let engines = state.engine.clone();
        let engine = engines.wait().await?;
        let settings = settings.unwrap_or_default();
        let mut slot = state.session_slot().await;
        // Read under the slot: a switch moves the window network while it
        // holds it.
        let window = state.window_network();
        let network = requested.unwrap_or_else(|| window.clone());
        settings.check_trust(&network)?;
        let session = match &*slot {
            Some(s) if *s.network() == network => Arc::clone(s),
            active => {
                if let Some(old) = active {
                    let old = old.network().clone();
                    close_active(&state, &mut slot, engine.close_network(old).await)?;
                }
                let s = open_any_network(&state, &network, settings).await?;
                *slot = Some(Arc::clone(&s));
                s
            }
        };
        Ok(OpenedNetwork::of(&session, NetworkInfo::opened(&session)))
    }

    /// Moves the window to `network` (onboarding, Settings), saving the
    /// choice for the next start once it is open. A pinned window
    /// ([`NetworkChoiceView::forced_by_flag`]) only saves it. Otherwise,
    /// unless it is already the window's, the network is closed as
    /// [`Engine::close_network_unless_busy`] closes it: its operations
    /// paused, it is refused with `app.network_busy` while a Platform flow
    /// holds a lease, a wallet mixes or a broadcast has not returned,
    /// checked before and after the operations already running finish
    /// (`draining` when they do not within 2 s); then the engine's own
    /// refusals (`wallet` while a wallet is still opening,
    /// `shielded_spend_unresolved`, both with
    /// [`ErrorData::NetworkCloseRefused`]). A refusal changes nothing:
    /// the session, its drafts, the window and the choice stay, and the
    /// paused operations run. Once closed, its vault is locked and its
    /// draft handles dropped, and the chosen one opens with SPV started as
    /// at launch. A failed open reopens the network it left and is
    /// `app.network_switch_failed`, the choice unsaved. The renderer boots
    /// again on success: every read of the old network is gone.
    ///
    /// [`Engine::close_network_unless_busy`]: dw_engine::Engine::close_network_unless_busy
    #[facade(
        "Engine::close_network_unless_busy",
        "NetworkSession::leases",
        "NetworkSession::mixing_busy"
    )]
    fn switch_network(state, network: ChosenNetwork) -> NetworkSwitch {
        let target = NetworkChoice::from(network).network();
        if state.launch.pinned() {
            network_choice::write_network_choice(&state.data_root, network.into())?;
            return Ok(NetworkSwitch {
                opened: None,
                choice: NetworkChoiceView::read(&state),
            });
        }
        state.retry_engine_if_failed();
        let engines = state.engine.clone();
        let engine = engines.wait().await?;
        let mut slot = state.session_slot().await;
        let left = state.window_network();
        let active = slot.clone();
        let session = match active {
            Some(s) if *s.network() == target => s,
            active => {
                if let Some(s) = active {
                    let old = s.network().clone();
                    drop(s);
                    let busy = busy_check(state.broadcasts());
                    let closed = match engine.close_network_unless_busy(old, SWITCH_DRAIN, busy).await {
                        Ok(closed) => Ok(closed),
                        Err(CloseRefused::Busy(e)) => return Err(e),
                        Err(CloseRefused::Draining) => {
                            return Err(busy_error(Busy {
                                draining: true,
                                ..Busy::default()
                            }));
                        }
                        Err(CloseRefused::Engine(e)) => Err(e),
                    };
                    // A refusal keeps the session active: nothing changed. A
                    // close that failed once the session was gone goes on
                    // like one that went through.
                    if let Err(e) = close_active(&state, &mut slot, closed) {
                        if slot.is_some() {
                            return Err(close_refusal(e));
                        }
                        eprintln!("dw-app: the close of {} failed: {e}", left.dir_name());
                    }
                }
                match open_window_network(&state, &target, SessionSettings::default()).await {
                    Ok(s) => {
                        *slot = Some(Arc::clone(&s));
                        s
                    }
                    Err(e) => {
                        eprintln!("dw-app: {} did not open: {e}", target.dir_name());
                        #[cfg(test)]
                        if let Some(seam) = &*state.before_rollback.lock().unwrap() {
                            seam();
                        }
                        let back = open_window_network(&state, &left, SessionSettings::default())
                            .await
                            .inspect_err(|e| eprintln!("dw-app: {} did not reopen: {e}", left.dir_name()));
                        let rolled_back = back.is_ok();
                        *slot = back.ok();
                        return Err(AppError::app(
                            AppCode::NetworkSwitchFailed,
                            format!("{} did not open: {e}", target.dir_name()),
                        )
                        .with(ErrorData::NetworkSwitch {
                            network: NetworkInfo::from(&target),
                            rolled_back,
                        }));
                    }
                }
            }
        };
        state.set_window_network(target);
        // Last: a switch that did not happen never changes what the next
        // start opens.
        if let Err(e) = network_choice::write_network_choice(&state.data_root, network.into()) {
            eprintln!("dw-app: the network choice was not saved: {e}");
        }
        Ok(NetworkSwitch {
            opened: Some(OpenedNetwork::of(
                &session,
                NetworkInfo::from(session.network()),
            )),
            choice: NetworkChoiceView::read(&state),
        })
    }

    /// The way out of a restore that waits for Platform (`restoreStatus`):
    /// closes the active network as `switchNetwork` closes it (refused with
    /// `app.network_busy` while a Platform flow holds a lease, a wallet
    /// mixes or a broadcast has not returned, `draining` when the running
    /// operations do not finish within 2 s, then the engine's own
    /// refusals with [`ErrorData::NetworkCloseRefused`]; a refusal changes
    /// nothing) and opens it again with Platform off, for this session
    /// only: the next start, or `openNetwork` after a close, opens it with
    /// Platform. Without Platform the restore's gate opens once Core has
    /// caught up and every wallet's history has loaded. While the gate is
    /// open, or does not wait for Platform (the network already runs
    /// without it, or only Core or a history load is waited for), it does
    /// nothing and returns the active network. A
    /// failed open reopens the network with Platform and is
    /// `app.network_switch_failed` ([`ErrorData::NetworkSwitch`],
    /// `rolledBack` false when that failed too, the window then without a
    /// network). `network_not_open` before `openNetwork`. The renderer
    /// boots again on success.
    #[facade("NetworkSession::restore_status", "Engine::close_network_unless_busy")]
    fn finish_without_platform(state) -> OpenedNetwork {
        state.retry_engine_if_failed();
        let engines = state.engine.clone();
        let engine = engines.wait().await?;
        let mut slot = state.session_slot().await;
        let Some(session) = slot.clone() else {
            return Err(AppError::new(EngineCode::NetworkNotOpen, "call open_network first"));
        };
        let network = session.network().clone();
        let waits_for_platform = (session.restore_status().await?)
            .is_some_and(|s| s.waiting.contains(&dw_engine::RestoreWait::PlatformPass));
        if !waits_for_platform {
            return Ok(OpenedNetwork::of(&session, NetworkInfo::opened(&session)));
        }
        drop(session);
        let busy = busy_check(state.broadcasts());
        let closed = match engine.close_network_unless_busy(network.clone(), SWITCH_DRAIN, busy).await {
            Ok(closed) => Ok(closed),
            Err(CloseRefused::Busy(e)) => return Err(e),
            Err(CloseRefused::Draining) => {
                return Err(busy_error(Busy {
                    draining: true,
                    ..Busy::default()
                }));
            }
            Err(CloseRefused::Engine(e)) => Err(e),
        };
        if let Err(e) = close_active(&state, &mut slot, closed) {
            if slot.is_some() {
                return Err(close_refusal(e));
            }
            eprintln!("dw-app: the close of {} failed: {e}", network.dir_name());
        }
        #[cfg(test)]
        if let Some(seam) = &*state.before_platform_off_open.lock().unwrap() {
            seam();
        }
        let platform_off = SessionSettings {
            no_platform: true,
            ..SessionSettings::default()
        };
        let session = match open_any_network(&state, &network, platform_off).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("dw-app: {} did not open without Platform: {e}", network.dir_name());
                #[cfg(test)]
                if let Some(seam) = &*state.before_rollback.lock().unwrap() {
                    seam();
                }
                let back = open_any_network(&state, &network, SessionSettings::default())
                    .await
                    .inspect_err(|e| eprintln!("dw-app: {} did not reopen: {e}", network.dir_name()));
                let rolled_back = back.is_ok();
                *slot = back.ok();
                return Err(AppError::app(
                    AppCode::NetworkSwitchFailed,
                    format!("{} did not open without Platform: {e}", network.dir_name()),
                )
                .with(ErrorData::NetworkSwitch {
                    network: NetworkInfo::from(&network),
                    rolled_back,
                }));
            }
        };
        *slot = Some(Arc::clone(&session));
        Ok(OpenedNetwork::of(&session, NetworkInfo::opened(&session)))
    }

    /// Closes the active network and drops its draft handles; `false` when
    /// none was open.
    #[facade("Engine::close_network")]
    fn close_network(state) -> bool {
        // No session yet, including while the engine is still starting:
        // `false`, without waiting out the build.
        let mut slot = state.session_slot().await;
        let Some(network) = slot.as_ref().map(|s| s.network().clone()) else {
            return Ok(false);
        };
        let engines = state.engine.clone();
        let engine = engines.wait().await?;
        close_active(&state, &mut slot, engine.close_network(network).await)
    }

    /// The active network, `null` before `openNetwork`, with whether its
    /// wallet data was restored (read again at each call).
    #[facade()]
    fn active_network(state) -> Option<NetworkInfo> {
        Ok(state
            .session_slot()
            .await
            .as_ref()
            .map(|s| NetworkInfo::opened(s)))
    }

    /// What the active network's restore waits for before funds actions
    /// run again; `null` when they run. A change is a `sync` event.
    #[facade("NetworkSession::restore_status")]
    fn restore_status(state) -> Option<RestoreStatus> {
        Ok(state.session().await?.restore_status().await?.map(Into::into))
    }

    /// Asks Platform for a fresh check and the restore to try opening the
    /// gate at once, a failed attempt's delay cut short. Nothing while the
    /// gate is open.
    #[facade("NetworkSession::restore_retry")]
    fn restore_retry(state) -> () {
        Ok(state.session().await?.restore_retry().await?)
    }

    /// The saved network choice (mainnet or testnet), the window's network
    /// and whether the window is pinned to it.
    #[facade()]
    fn get_network_choice(state) -> NetworkChoiceView {
        Ok(NetworkChoiceView::read(&state))
    }

    /// Every network with data under the data root, in network order.
    #[facade("Engine::existing_networks")]
    fn existing_networks(state) -> Vec<NetworkData> {
        let engines = state.engine.clone();
        let networks = engines.wait().await?.existing_networks().await?;
        Ok(networks.into_iter().map(Into::into).collect())
    }

    /// Whether `network` (`null`: the window's) has a verified
    /// before-upgrade snapshot to restore: "Restore earlier data" shows
    /// only then.
    #[facade("Engine::restore_available")]
    fn restore_available(state, network: Option<String>) -> bool {
        let network = restore_network(&state, network)?;
        let engines = state.engine.clone();
        Ok(engines.wait().await?.restore_available(&network).await?)
    }

    /// Restore earlier data: asks for the restore of `network`'s
    /// (`null`: the window's) verified before-upgrade snapshot at the next
    /// start; the renderer then calls `restartApp`. Nothing is restored in
    /// this process. Without a verified snapshot, `storage_restore`
    /// (refused, missingBackup).
    #[facade("Engine::request_restore")]
    fn request_restore(state, network: Option<String>) -> () {
        let network = restore_network(&state, network)?;
        let engines = state.engine.clone();
        Ok(engines.wait().await?.request_restore(&network).await?)
    }

    /// The foreign-lineage screen's Keep the current files: ends
    /// `network`'s (`null`: the window's) part-done restore keeping its
    /// files as they are; the renderer then opens the network again.
    #[facade("Engine::keep_current_files")]
    fn keep_current_files(state, network: Option<String>) -> () {
        let network = restore_network(&state, network)?;
        let engines = state.engine.clone();
        Ok(engines.wait().await?.keep_current_files(&network).await?)
    }

    /// Cancel restore: drops `network`'s (`null`: the window's) restore
    /// request while it hasn't begun replacing files; once it has,
    /// `storage_restore` (incomplete).
    #[facade("Engine::cancel_restore")]
    fn cancel_restore(state, network: Option<String>) -> () {
        let network = restore_network(&state, network)?;
        let engines = state.engine.clone();
        Ok(engines.wait().await?.cancel_restore(&network).await?)
    }
}

/// The network a restore command names; `null`: the window's.
fn restore_network(state: &AppState, network: Option<String>) -> Result<DashNetwork, AppError> {
    Ok(match network.as_deref().map(parse_network).transpose()? {
        Some(n) => n,
        None => state.window_network(),
    })
}

/// Opens `network`: as the window's ([`open_window_network`]) when it is
/// the window's network, otherwise with `settings` over the launch flags
/// and SPV not started.
async fn open_any_network(
    state: &AppState,
    network: &DashNetwork,
    settings: SessionSettings,
) -> Result<Arc<NetworkSession>, AppError> {
    if *network == state.window_network() {
        return open_window_network(state, network, settings).await;
    }
    let engines = state.engine.clone();
    let s = (engines.wait().await?)
        .open_network(network.clone(), settings.over(&state.launch))
        .await?;
    crate::shell::sleep::require_beats(&s);
    Ok(s)
}

/// Opens `network` as the window's: with the launch flags (or, on a
/// developer network, `settings` over them), and SPV started as the launch
/// starts it for that network.
async fn open_window_network(
    state: &AppState,
    network: &DashNetwork,
    settings: SessionSettings,
) -> Result<Arc<NetworkSession>, AppError> {
    #[cfg(test)]
    let settings = (state.test_endpoints.lock().unwrap().clone())
        .map(|t| SessionSettings {
            no_platform: t.no_platform || settings.no_platform,
            ..t
        })
        .unwrap_or(settings);
    state.retry_engine_if_failed();
    let engines = state.engine.clone();
    let s = engines
        .wait()
        .await?
        .open_network(network.clone(), settings.over(&state.launch))
        .await?;
    crate::shell::sleep::require_beats(&s);
    if state.launch.starts_spv_on(network) {
        #[cfg(test)]
        state.spv_started.lock().unwrap().push(network.clone());
        if let Err(e) = s.start_spv().await {
            eprintln!("dw-app: SPV did not start: {e}");
        }
    }
    Ok(s)
}

/// How long a switch waits for the operations admitted before it to
/// finish before it checks the network again.
const SWITCH_DRAIN: Duration = Duration::from_secs(2);

/// What runs on the network a switch would leave.
#[derive(Default)]
struct Busy {
    flows: Vec<crate::dto::platform::FlowKindName>,
    stalled: bool,
    mixing: bool,
    broadcast: bool,
    draining: bool,
}

/// The switch's check of the network it leaves: the error that refuses
/// it, `app.network_busy`, when closing would cut a Platform flow's
/// lease, mixing, or a broadcast not yet returned (`broadcasts`). A lease
/// that cannot be read refuses too.
fn busy_check(
    broadcasts: Arc<AtomicUsize>,
) -> impl Fn(&NetworkSession) -> Option<AppError> + Send + Sync + 'static {
    move |session| {
        let leases = match session.leases() {
            Ok(leases) => leases,
            Err(e) => return Some(e.into()),
        };
        let broadcast = broadcasts.load(Ordering::SeqCst) > 0;
        busy_of(&leases, session.mixing_busy(), broadcast).map(busy_error)
    }
}

/// What `leases`, mixing and a broadcast not yet returned make busy.
/// `stalled`: every flow's lease is parked or waits for a grant, so
/// locking the wallet ends them.
fn busy_of(leases: &[LeaseView], mixing: bool, broadcast: bool) -> Option<Busy> {
    let mut busy = Busy::default();
    let mut stalled = true;
    for lease in leases {
        let flow = lease.flow.into();
        match lease.state {
            LeaseStateView::Revoked { .. } | LeaseStateView::Ended => continue,
            LeaseStateView::Parked { .. } | LeaseStateView::NeedsGrant => {}
            _ => stalled = false,
        }
        if !busy.flows.contains(&flow) {
            busy.flows.push(flow);
        }
    }
    busy.stalled = stalled && !busy.flows.is_empty();
    busy.mixing = mixing;
    busy.broadcast = broadcast;
    (!busy.flows.is_empty() || busy.mixing || busy.broadcast).then_some(busy)
}

/// What runs on `session` that closing the engine for an update would cut
/// (PK-03): a broadcast not yet returned, mixing or a Platform flow.
pub(crate) fn running(
    session: &NetworkSession,
    broadcasts: &AtomicUsize,
) -> Result<Option<UpdateBusyReason>, AppError> {
    Ok(update_busy(
        &session.leases()?,
        session.mixing_busy(),
        broadcasts.load(Ordering::SeqCst) > 0,
    ))
}

/// The one reason an update names: a payment first, then mixing, then a
/// Platform flow.
fn update_busy(leases: &[LeaseView], mixing: bool, broadcast: bool) -> Option<UpdateBusyReason> {
    busy_of(leases, mixing, broadcast).map(|b| {
        if b.broadcast {
            UpdateBusyReason::Payment
        } else if b.mixing {
            UpdateBusyReason::Mixing
        } else {
            UpdateBusyReason::Platform
        }
    })
}

/// The switch's refusal while mixing, for a reset of chain data too: a
/// mixing session's signed inputs stay held while the reset blinds the
/// wallet to its transaction.
pub(crate) fn mixing_busy_error() -> AppError {
    busy_error(Busy {
        mixing: true,
        ..Busy::default()
    })
}

fn busy_error(busy: Busy) -> AppError {
    let Busy {
        flows,
        stalled,
        mixing,
        broadcast,
        draining,
    } = busy;
    AppError::app(
        AppCode::NetworkBusy,
        format!("running: {flows:?}, mixing {mixing}, broadcast {broadcast}, draining {draining}"),
    )
    .with(ErrorData::NetworkBusy {
        flows,
        stalled,
        mixing,
        broadcast,
        draining,
    })
}

/// The engine's own refusal of a switch's close, named for the renderer.
fn close_refusal(e: AppError) -> AppError {
    let reason = match e.code_str() {
        "wallet" => CloseRefusalReason::WalletOpening,
        "shielded_spend_unresolved" => CloseRefusalReason::ShieldedSpendUnresolved,
        _ => return e,
    };
    e.with(ErrorData::NetworkCloseRefused { reason })
}

/// The active session's close, as `close_network` returned it: the slot and
/// the session's handles are dropped once it closed, or failed to. A
/// refusal, which leaves the engine's session open
/// (`shielded_spend_unresolved`, DEC-1046; `wallet` while a wallet is
/// still opening, E1 r2), leaves both as they were: the session stays
/// active, its drafts kept.
pub(crate) fn close_active(
    state: &crate::state::AppState,
    slot: &mut Option<Arc<dw_engine::NetworkSession>>,
    closed: Result<bool, dw_engine::EngineError>,
) -> Result<bool, AppError> {
    if closed.is_err() && slot.as_ref().is_some_and(|s| s.is_open()) {
        return closed.map_err(Into::into);
    }
    *slot = None;
    state.clear_handles();
    Ok(closed?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(flow: dw_engine::platform::FlowKind, state: LeaseStateView) -> LeaseView {
        LeaseView {
            id: "00000000".into(),
            wallet_id: "00".repeat(32),
            flow,
            state,
            own_key: false,
            key_expires_in_secs: None,
            funds_committed: false,
            budgets: Vec::new(),
            in_flight: 0,
            call_running: false,
        }
    }

    #[test]
    fn flows_that_only_wait_for_a_grant_or_a_proof_are_stalled() {
        use dw_engine::platform::{FlowKind, ParkReason};
        let waiting = lease(FlowKind::ContactRequest, LeaseStateView::NeedsGrant);
        let parked = lease(
            FlowKind::Registration,
            LeaseStateView::Parked {
                reason: ParkReason::ProofWaiting,
            },
        );
        let active = lease(FlowKind::TopUp, LeaseStateView::Active);
        let ended = lease(FlowKind::TopUp, LeaseStateView::Ended);
        let busy = busy_of(&[waiting.clone(), parked.clone(), ended.clone()], false, false).unwrap();
        assert_eq!(busy.flows.len(), 2);
        assert!(busy.stalled);
        let busy = busy_of(&[waiting, active], false, false).unwrap();
        assert!(!busy.stalled, "a flow still running is not stalled");
        let busy = busy_of(std::slice::from_ref(&ended), true, false).unwrap();
        assert!(!busy.stalled, "no flow, nothing to stall");
        assert!(busy_of(&[ended], false, false).is_none());
    }

    #[test]
    fn an_update_names_a_payment_then_mixing_then_a_platform_flow() {
        use dw_engine::platform::FlowKind;
        let active = lease(FlowKind::TopUp, LeaseStateView::Active);
        let waiting = lease(FlowKind::ContactRequest, LeaseStateView::NeedsGrant);
        let ended = lease(FlowKind::TopUp, LeaseStateView::Ended);
        let flows = [active.clone(), waiting.clone()];
        assert_eq!(update_busy(&flows, true, true), Some(UpdateBusyReason::Payment));
        assert_eq!(update_busy(&flows, true, false), Some(UpdateBusyReason::Mixing));
        assert_eq!(update_busy(&[], true, false), Some(UpdateBusyReason::Mixing));
        assert_eq!(update_busy(&[waiting], false, false), Some(UpdateBusyReason::Platform));
        assert_eq!(update_busy(&[active], false, false), Some(UpdateBusyReason::Platform));
        assert_eq!(update_busy(&[ended], false, false), None);
        assert_eq!(update_busy(&[], false, false), None);
    }

    #[test]
    fn settings_fall_back_to_the_launch_flags() {
        let launch = LaunchOptions {
            dapi_addresses: vec!["https://127.0.0.1:2443".into()],
            ca_cert_path: Some("/etc/dash/ca.pem".into()),
            initial_protocol_version: Some(9),
            no_platform: true,
            collateral_rpc: Some("http://dw:dw@127.0.0.1:19898".into()),
            ..LaunchOptions::default()
        };
        let o = SessionSettings::default().over(&launch);
        assert_eq!(o.dapi_addresses, launch.dapi_addresses);
        assert_eq!(o.ca_cert_path, launch.ca_cert_path);
        assert_eq!(o.initial_protocol_version, Some(9));
        assert!(o.no_platform);
        assert_eq!(o.collateral_rpc, launch.collateral_rpc);
        let o = SessionSettings {
            dapi_addresses: vec!["https://10.0.0.1:443".into()],
            ca_cert_path: Some("/tmp/gw.pem".into()),
            initial_protocol_version: Some(11),
            ..SessionSettings::default()
        }
        .over(&launch);
        assert_eq!(o.dapi_addresses, ["https://10.0.0.1:443"]);
        assert_eq!(o.ca_cert_path, Some(PathBuf::from("/tmp/gw.pem")));
        assert_eq!(o.initial_protocol_version, Some(11));
    }
}
