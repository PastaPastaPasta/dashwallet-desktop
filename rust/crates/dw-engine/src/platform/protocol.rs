//! The network's Platform protocol version, for the features and prices
//! that follow it (release 0.1: the contested-name lock, the shielded pool's
//! gate). Not part of the facade.
//!
//! The SDK starts at the network's floor (`dash_sdk::sdk::min_protocol_version`,
//! 13 for mainnet and testnet at the pin) or the session's
//! `initial_protocol_version`, and every proof-verified response's metadata
//! ratchets it up, never down. [`NetworkSession::protocol_version`] reads
//! that value as it stands, offline; [`NetworkSession::refresh_protocol_version`]
//! first asks Platform once (a proven `getEpochsInfo`), so a call that needs
//! the network's version before any other read gets it. A failed refresh
//! keeps the version learned so far: a feature gated on a version reads as
//! absent until a proof shows it, and a price that falls at a version is
//! the higher one until then.

use std::sync::Arc;

use dpp::version::PlatformVersion;

use super::errors::PlatformError;
use crate::NetworkSession;

impl NetworkSession {
    /// The network's protocol version as the SDK last proved it (the floor
    /// before any proof). In-memory read.
    #[allow(dead_code, reason = "lane E1's shielded gate reads it")]
    pub(crate) fn protocol_version(&self) -> Result<u32, PlatformError> {
        let _op = self.try_enter()?;
        Ok(self.manager()?.sdk_arc().protocol_version_number())
    }

    /// The network's protocol version after one proven read of Platform's
    /// current epoch; the version learned so far when that read fails.
    pub(crate) async fn refresh_protocol_version(self: &Arc<Self>) -> Result<u32, PlatformError> {
        let this = Arc::clone(self);
        Ok(self
            .on_runtime(async move {
                #[cfg(test)]
                crate::coins::pause_at(&this.platform.hold_refresh).await;
                let _op = this.enter().await?;
                let sdk = this.manager()?.sdk_arc();
                Ok(refreshed(&sdk).await)
            })
            .await?)
    }
}

/// `sdk`'s protocol version after one proven refresh; the one it holds when
/// the refresh fails (the SDK logs why).
pub(crate) async fn refreshed(sdk: &dash_sdk::Sdk) -> u32 {
    sdk.refresh_protocol_version()
        .await
        .unwrap_or_else(|_| sdk.protocol_version_number())
}

/// The rules at `protocol_version`; a version this build does not know is
/// `internal` (the SDK never ratchets past the latest it knows).
pub(crate) fn platform_version(
    protocol_version: u32,
) -> Result<&'static PlatformVersion, PlatformError> {
    PlatformVersion::get(protocol_version).map_err(|e| PlatformError::Internal {
        detail: format!("protocol version {protocol_version}: {e}"),
    })
}
