use std::sync::Arc;

use crate::{DashNetwork, EngineError};

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SessionOptions {
    /// DAPI endpoints; empty = network defaults (mainnet/testnet only).
    pub dapi_addresses: Vec<String>,
    /// Trusted quorum service override (https on mainnet/testnet).
    pub quorum_url: Option<String>,
    /// SPV peers `ip:port`; non-empty restricts SPV to them.
    pub spv_peers: Vec<String>,
}

impl From<SessionOptions> for dw_engine::SessionOptions {
    fn from(o: SessionOptions) -> Self {
        Self {
            dapi_addresses: o.dapi_addresses,
            quorum_url: o.quorum_url,
            spv_peers: o.spv_peers,
            // Not on the UniFFI record: the checked-in Swift bindings are
            // frozen with the old UIs; the next binding (E0-13) adds them.
            ..Default::default()
        }
    }
}

/// One open network. Wallet operations are in `wallet.rs`.
#[derive(uniffi::Object)]
pub struct NetworkSession {
    pub(crate) inner: Arc<dw_engine::NetworkSession>,
}

impl NetworkSession {
    pub(crate) fn wrap(inner: Arc<dw_engine::NetworkSession>) -> Self {
        Self { inner }
    }
}

#[uniffi::export]
impl NetworkSession {
    pub fn network(&self) -> DashNetwork {
        self.inner.network().clone().into()
    }

    /// `false` after the engine closed this network.
    pub fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    /// Whether the trusted quorum provider is available for Platform proofs.
    pub fn platform_context_ready(&self) -> bool {
        self.inner.platform_context_ready()
    }

    pub async fn start_spv(&self) -> Result<(), EngineError> {
        Ok(self.inner.start_spv().await?)
    }

    pub async fn stop_spv(&self) -> Result<(), EngineError> {
        Ok(self.inner.stop_spv().await?)
    }

    pub fn spv_running(&self) -> Result<bool, EngineError> {
        Ok(self.inner.spv_running()?)
    }
}
