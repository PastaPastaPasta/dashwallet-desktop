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

use crate::dto::{LockStatus, NetworkInfo, parse_network};
use crate::error::{AppCode, AppError, CloseRefusalReason, ErrorData};
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

commands! {
    /// Makes `network` (a directory name; `null`: the window's network,
    /// the launch network until `switchNetwork` opens another) the active
    /// one, opening it if needed and closing the network that was active,
    /// with its draft handles. When it is already active, the open session
    /// is kept and `settings` are ignored. Opening the window's network
    /// starts SPV on mainnet and testnet unless the app was launched with
    /// `--no-sync`, and on another network with `--sync`; a failed start
    /// leaves the network open, readable offline.
    #[facade("Engine::open_network", "Engine::close_network")]
    fn open_network(
        state,
        network: Option<String>,
        settings: Option<SessionSettings>,
    ) -> OpenedNetwork {
        let engine = &state.engine;
        let requested = network.as_deref().map(parse_network).transpose()?;
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
                let s = if network == window {
                    open_window_network(&state, &network, settings).await?
                } else {
                    engine.open_network(network.clone(), settings.over(&state.launch)).await?
                };
                *slot = Some(Arc::clone(&s));
                s
            }
        };
        Ok(OpenedNetwork {
            network: NetworkInfo::from(&network),
            lock: session.vault().status().into(),
        })
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
                    let closed = match state.engine.close_network_unless_busy(old, SWITCH_DRAIN, busy).await {
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
            opened: Some(OpenedNetwork {
                network: NetworkInfo::from(session.network()),
                lock: session.vault().status().into(),
            }),
            choice: NetworkChoiceView::read(&state),
        })
    }

    /// Closes the active network and drops its draft handles; `false` when
    /// none was open.
    #[facade("Engine::close_network")]
    fn close_network(state) -> bool {
        let engine = &state.engine;
        let mut slot = state.session_slot().await;
        let Some(network) = slot.as_ref().map(|s| s.network().clone()) else {
            return Ok(false);
        };
        close_active(&state, &mut slot, engine.close_network(network).await)
    }

    /// The active network, `null` before `openNetwork`.
    #[facade()]
    fn active_network(state) -> Option<NetworkInfo> {
        Ok(state
            .session_slot()
            .await
            .as_ref()
            .map(|s| NetworkInfo::from(s.network())))
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
        let networks = state.engine.existing_networks().await?;
        Ok(networks.into_iter().map(Into::into).collect())
    }
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
    let settings = state.test_endpoints.lock().unwrap().clone().unwrap_or(settings);
    let s = state
        .engine
        .open_network(network.clone(), settings.over(&state.launch))
        .await?;
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
