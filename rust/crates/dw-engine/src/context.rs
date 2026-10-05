//! Platform proof context: the trusted HTTPS quorum service (iOS parity),
//! constructed lazily.
//!
//! `TrustedHttpContextProvider::new_with_url` resolves the quorum host's DNS
//! name at construction on desktop targets
//! (rs-sdk-trusted-context-provider/src/provider.rs:196-200, `verify_domain_resolves`).
//! Constructing it eagerly would make opening a wallet fail while offline, so
//! this wrapper builds it lazily and retries at most every
//! [`RETRY_INTERVAL`]. Until it exists every lookup returns a
//! `ContextProviderError`: proofs fail verification, they are never skipped.
//!
//! The SDK calls the `ContextProvider` methods synchronously from tokio
//! workers. Those calls never build the provider themselves: when it is
//! missing they start one background build (on its own OS thread, so the
//! blocking DNS lookup never occupies a worker) and fail at once.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
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

type Builder = dyn Fn() -> Result<TrustedHttpContextProvider, String> + Send + Sync;

pub(crate) struct LazyTrustedContext {
    builder: Box<Builder>,
    provider: RwLock<Option<Arc<TrustedHttpContextProvider>>>,
    last_attempt: Mutex<Option<Instant>>,
    /// A background build started by a lookup is running.
    retry_in_flight: AtomicBool,
}

impl LazyTrustedContext {
    pub(crate) fn new(
        network: Network,
        devnet_name: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        let url = resolved_url(network, base_url);
        Self::with_builder(Box::new(move || {
            let cache = NonZeroUsize::new(CACHE_SIZE).expect("CACHE_SIZE is non-zero");
            let result = match &url {
                Some(url) => TrustedHttpContextProvider::new_with_url(network, url.clone(), cache),
                None => TrustedHttpContextProvider::new(network, devnet_name.clone(), cache),
            };
            result.map_err(|e| e.to_string())
        }))
    }

    fn with_builder(builder: Box<Builder>) -> Self {
        Self {
            builder,
            provider: RwLock::new(None),
            last_attempt: Mutex::new(None),
            retry_in_flight: AtomicBool::new(false),
        }
    }

    fn current(&self) -> Option<Arc<TrustedHttpContextProvider>> {
        self.provider
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
    }

    /// Records an attempt now unless one was made within [`RETRY_INTERVAL`]
    /// (`force` ignores the interval). Returns whether to attempt.
    fn claim_attempt(&self, force: bool) -> bool {
        let mut last = self.last_attempt.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = *last
            && !force
            && t.elapsed() < RETRY_INTERVAL
        {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    fn build_and_store(&self) -> Result<Arc<TrustedHttpContextProvider>, String> {
        let built = Arc::new((self.builder)()?);
        let mut slot = self.provider.write().unwrap_or_else(|p| p.into_inner());
        Ok(Arc::clone(slot.get_or_insert(built)))
    }

    /// Returns the provider, building it on this thread if no attempt was
    /// made within [`RETRY_INTERVAL`]; `force` ignores the interval.
    /// Blocking (DNS): call it from a blocking thread only.
    pub(crate) fn get_or_try_init(
        &self,
        force: bool,
    ) -> Result<Arc<TrustedHttpContextProvider>, String> {
        if let Some(p) = self.current() {
            return Ok(p);
        }
        if !self.claim_attempt(force) {
            return Err("trusted quorum provider unavailable; retry pending".to_string());
        }
        self.build_and_store()
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.current().is_some()
    }

    /// Lookup path used by the SDK. Never blocks: when the provider is
    /// missing it starts at most one background build (rate-limited by
    /// [`RETRY_INTERVAL`]) and returns an error immediately.
    fn provider(self: &Arc<Self>) -> Result<Arc<TrustedHttpContextProvider>, ContextProviderError> {
        if let Some(p) = self.current() {
            return Ok(p);
        }
        self.schedule_retry();
        Err(ContextProviderError::Generic(
            "trusted quorum provider unavailable; retry scheduled".to_string(),
        ))
    }

    fn schedule_retry(self: &Arc<Self>) {
        if self.retry_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        if !self.claim_attempt(false) {
            self.retry_in_flight.store(false, Ordering::Release);
            return;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("dw-quorum-context".into())
            .spawn(move || {
                if let Err(e) = this.build_and_store() {
                    tracing::debug!(error = %e, "trusted quorum provider retry failed");
                }
                this.retry_in_flight.store(false, Ordering::Release);
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the quorum provider retry thread");
            self.retry_in_flight.store(false, Ordering::Release);
        }
    }
}

/// The quorum URL used when the caller gives none. Regtest uses the local
/// dashmate quorum sidecar, as the iOS SDK does.
fn resolved_url(network: Network, base_url: Option<String>) -> Option<String> {
    base_url.or_else(|| match network {
        Network::Regtest => Some("http://127.0.0.1:22444".to_string()),
        _ => None,
    })
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
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn regtest_defaults_to_local_sidecar_and_builds_offline() {
        assert_eq!(
            resolved_url(Network::Regtest, None).as_deref(),
            Some("http://127.0.0.1:22444")
        );
        let ctx = LazyTrustedContext::new(Network::Regtest, None, None);
        // An IP-literal URL needs no DNS, so this works without a network.
        assert!(ctx.get_or_try_init(true).is_ok());
        assert!(ctx.is_ready());
    }

    #[test]
    fn mainnet_rejects_plaintext_override() {
        let ctx = Arc::new(LazyTrustedContext::new(
            Network::Mainnet,
            None,
            Some("http://127.0.0.1:1".into()),
        ));
        assert!(ctx.get_or_try_init(true).is_err());
        assert!(!ctx.is_ready());
        // Within the retry interval a lookup schedules nothing and fails.
        assert!(ctx.provider().is_err());
    }

    /// Review M3: a lookup on a missing provider must not run the (blocking)
    /// build on the caller's thread, and concurrent lookups start one build.
    #[test]
    fn lookups_never_block_and_start_one_background_build() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&builds);
        let ctx = Arc::new(LazyTrustedContext::with_builder(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            // Stands in for a DNS lookup that hangs.
            std::thread::sleep(Duration::from_millis(400));
            Err("dns timeout".into())
        })));

        let started = Instant::now();
        for _ in 0..20 {
            assert!(ctx.provider().is_err());
        }
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "lookups waited for the build: {:?}",
            started.elapsed()
        );
        assert!(ctx.retry_in_flight.load(Ordering::Acquire));

        // Let the background build finish; still exactly one attempt, and the
        // retry interval stops further builds.
        let deadline = Instant::now() + Duration::from_secs(5);
        while ctx.retry_in_flight.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!ctx.retry_in_flight.load(Ordering::Acquire));
        assert!(ctx.provider().is_err());
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }
}
