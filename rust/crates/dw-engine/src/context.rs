//! Platform proof context: the trusted HTTPS quorum service (iOS parity),
//! constructed lazily.
//!
//! `TrustedHttpContextProvider::new_with_url` resolves the quorum host's DNS
//! name at construction on desktop targets
//! (rs-sdk-trusted-context-provider/src/provider.rs:196-200, `verify_domain_resolves`).
//! Constructing it eagerly would make opening a wallet fail while offline, so
//! this wrapper builds it on first use and retries at most every
//! [`RETRY_INTERVAL`]. Until it exists every lookup returns a
//! `ContextProviderError` — proofs fail verification, they are never skipped.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use dash_context_provider::{ContextProvider, ContextProviderError};
use dpp::data_contract::TokenConfiguration;
use dpp::prelude::{CoreBlockHeight, DataContract, Identifier};
use dpp::version::PlatformVersion;
use key_wallet::Network;
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;

const RETRY_INTERVAL: Duration = Duration::from_secs(30);
/// Same quorum cache size the iOS SDK uses (rs-sdk-ffi/src/sdk.rs `dash_sdk_create_trusted`).
const CACHE_SIZE: usize = 100;

pub(crate) struct LazyTrustedContext {
    network: Network,
    devnet_name: Option<String>,
    /// `None` = the network's default quorum service URL.
    base_url: Option<String>,
    provider: RwLock<Option<Arc<TrustedHttpContextProvider>>>,
    last_attempt: Mutex<Option<Instant>>,
}

impl LazyTrustedContext {
    pub(crate) fn new(
        network: Network,
        devnet_name: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        Self {
            network,
            devnet_name,
            base_url,
            provider: RwLock::new(None),
            last_attempt: Mutex::new(None),
        }
    }

    /// The quorum URL used when the caller gives none. Regtest uses the local
    /// dashmate quorum sidecar, as the iOS SDK does.
    fn resolved_url(&self) -> Option<String> {
        self.base_url.clone().or_else(|| match self.network {
            Network::Regtest => Some("http://127.0.0.1:22444".to_string()),
            _ => None,
        })
    }

    fn build(&self) -> Result<TrustedHttpContextProvider, String> {
        let cache = NonZeroUsize::new(CACHE_SIZE).expect("CACHE_SIZE is non-zero");
        let result = match self.resolved_url() {
            Some(url) => TrustedHttpContextProvider::new_with_url(self.network, url, cache),
            None => TrustedHttpContextProvider::new(self.network, self.devnet_name.clone(), cache),
        };
        result.map_err(|e| e.to_string())
    }

    /// Returns the provider, constructing it if no attempt was made within
    /// [`RETRY_INTERVAL`]. `force` ignores the retry interval.
    pub(crate) fn get_or_try_init(
        &self,
        force: bool,
    ) -> Result<Arc<TrustedHttpContextProvider>, String> {
        if let Some(p) = self
            .provider
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            return Ok(Arc::clone(p));
        }
        {
            let mut last = self.last_attempt.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(t) = *last {
                if !force && t.elapsed() < RETRY_INTERVAL {
                    return Err("trusted quorum provider unavailable; retry pending".to_string());
                }
            }
            *last = Some(Instant::now());
        }
        let built = Arc::new(self.build()?);
        let mut slot = self.provider.write().unwrap_or_else(|p| p.into_inner());
        Ok(Arc::clone(slot.get_or_insert(built)))
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.provider
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    fn provider(&self) -> Result<Arc<TrustedHttpContextProvider>, ContextProviderError> {
        self.get_or_try_init(false)
            .map_err(ContextProviderError::Generic)
    }
}

/// Handle given to `SdkBuilder::with_context_provider` (which takes the
/// provider by value), sharing the lazy state with the session.
pub(crate) struct SharedContext(pub(crate) Arc<LazyTrustedContext>);

impl ContextProvider for SharedContext {
    fn get_data_contract(
        &self,
        id: &Identifier,
        platform_version: &PlatformVersion,
    ) -> Result<Option<Arc<DataContract>>, ContextProviderError> {
        self.0.provider()?.get_data_contract(id, platform_version)
    }

    fn register_data_contract(&self, contract: Arc<DataContract>) {
        if let Ok(p) = self.0.provider() {
            p.register_data_contract(contract);
        }
    }

    fn get_token_configuration(
        &self,
        token_id: &Identifier,
    ) -> Result<Option<TokenConfiguration>, ContextProviderError> {
        self.0.provider()?.get_token_configuration(token_id)
    }

    fn get_quorum_public_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        core_chain_locked_height: u32,
    ) -> Result<[u8; 48], ContextProviderError> {
        self.0
            .provider()?
            .get_quorum_public_key(quorum_type, quorum_hash, core_chain_locked_height)
    }

    fn get_platform_activation_height(&self) -> Result<CoreBlockHeight, ContextProviderError> {
        self.0.provider()?.get_platform_activation_height()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regtest_defaults_to_local_sidecar_and_builds_offline() {
        let ctx = LazyTrustedContext::new(Network::Regtest, None, None);
        assert_eq!(
            ctx.resolved_url().as_deref(),
            Some("http://127.0.0.1:22444")
        );
        // An IP-literal URL needs no DNS, so this works without a network.
        assert!(ctx.get_or_try_init(true).is_ok());
        assert!(ctx.is_ready());
    }

    #[test]
    fn mainnet_rejects_plaintext_override() {
        let ctx =
            LazyTrustedContext::new(Network::Mainnet, None, Some("http://127.0.0.1:1".into()));
        assert!(ctx.get_or_try_init(true).is_err());
        assert!(!ctx.is_ready());
        // Within the retry interval a non-forced lookup does not retry.
        assert!(ctx.provider().is_err());
    }
}
