//! Opening and closing networks (`Engine`), and the networks with data.
//!
//! The window shows one network at a time (E0-13 D-row "one active
//! session"): `open_network` makes a network the active one, closing the
//! one before it, and every session command acts on the active one.

use std::path::PathBuf;
use std::sync::Arc;

use dw_engine::{NetworkDataInfo, SessionOptions};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::{LockStatus, NetworkInfo, parse_network};
use crate::ipc::commands;
use crate::launch::LaunchOptions;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    ("Engine::new", "internal: the app creates the engine at start-up"),
    ("Engine::config", "internal: the data root is the launch's"),
    ("Engine::network_dir", "internal: a path the engine resolves"),
    ("Engine::session", "internal: `open_network` returns the open session"),
    ("Engine::shutdown", "internal: runs when the app exits"),
    ("Engine::block_on", "internal: for synchronous hosts"),
    ("NetworkSession::network", "internal: `open_network` returns it"),
    ("NetworkSession::data_dir", "internal: a path the engine owns"),
    ("NetworkSession::avatars_dir", "internal: avatars cross as bytes"),
    ("NetworkSession::is_open", "internal: the binding drops a session it closes"),
    ("NetworkSession::platform_context_ready", "internal: the `platform` events and `platformStatus` report it"),
];

/// How to reach a network. Every field is optional; the launch flags
/// (`--dapi`, `--spv-peer`, `--ca-cert`, …) fill those the renderer leaves out.
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
        }
    }
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
    /// Makes `network` (a directory name; `null`: the launch network) the
    /// active one, opening it if needed and closing the network that was
    /// active, with its draft handles. When it is already active, the open
    /// session is kept and `settings` are ignored. Opening the launch
    /// network of a `--sync` launch starts SPV; a failed start leaves the
    /// network open, readable offline.
    #[facade("Engine::open_network", "Engine::close_network")]
    fn open_network(
        state,
        network: Option<String>,
        settings: Option<SessionSettings>,
    ) -> OpenedNetwork {
        let engine = state.engine()?;
        let network = match network {
            Some(name) => parse_network(&name)?,
            None => state.launch.network.clone(),
        };
        let mut slot = state.session_slot().await;
        let session = match &*slot {
            Some(s) if *s.network() == network => Arc::clone(s),
            active => {
                if let Some(old) = active {
                    let old = old.network().clone();
                    *slot = None;
                    state.clear_handles();
                    engine.close_network(old).await?;
                }
                let options = settings.unwrap_or_default().over(&state.launch);
                let s = engine.open_network(network.clone(), options).await?;
                *slot = Some(Arc::clone(&s));
                if state.launch.sync
                    && network == state.launch.network
                    && let Err(e) = s.start_spv().await
                {
                    eprintln!("dw-app: SPV did not start: {e}");
                }
                s
            }
        };
        Ok(OpenedNetwork {
            network: NetworkInfo::from(&network),
            lock: session.vault().status().into(),
        })
    }

    /// Closes the active network and drops its draft handles; `false` when
    /// none was open.
    #[facade("Engine::close_network")]
    fn close_network(state) -> bool {
        let engine = state.engine()?;
        let mut slot = state.session_slot().await;
        let Some(session) = slot.take() else {
            return Ok(false);
        };
        state.clear_handles();
        Ok(engine.close_network(session.network().clone()).await?)
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

    /// Every network with data under the data root, in network order.
    #[facade("Engine::existing_networks")]
    fn existing_networks(state) -> Vec<NetworkData> {
        let networks = state.engine()?.existing_networks().await?;
        Ok(networks.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_fall_back_to_the_launch_flags() {
        let launch = LaunchOptions {
            dapi_addresses: vec!["https://127.0.0.1:2443".into()],
            ca_cert_path: Some("/etc/dash/ca.pem".into()),
            initial_protocol_version: Some(9),
            no_platform: true,
            ..LaunchOptions::default()
        };
        let o = SessionSettings::default().over(&launch);
        assert_eq!(o.dapi_addresses, launch.dapi_addresses);
        assert_eq!(o.ca_cert_path, launch.ca_cert_path);
        assert_eq!(o.initial_protocol_version, Some(9));
        assert!(o.no_platform);
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
