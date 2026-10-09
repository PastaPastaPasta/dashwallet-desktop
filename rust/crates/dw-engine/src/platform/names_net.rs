//! What `register_name` asks of Platform, behind a seam (DP1-03 review r1):
//! [`SdkNet`] asks the SDK; `names_tests.rs` scripts it, to reach every
//! refusal and write boundary without a network. Not part of the facade.

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;

use super::{
    Arc, Future, Identifier, Lookup, PlatformError, PlatformVersion, PlatformWallet, UsernameCheck,
    VaultIdentitySigner, contest_end, lookup,
};

/// What a registration asks of Platform.
pub(crate) trait NameNet: Send + Sync {
    fn version(&self) -> &PlatformVersion;
    /// See [`lookup`].
    fn lookup(
        &self,
        check: &UsernameCheck,
    ) -> impl Future<Output = Result<Lookup, PlatformError>> + Send;
    /// See [`contest_end`].
    fn contest_end(&self, normalized: &str) -> impl Future<Output = Option<u64>> + Send;
    /// The identity's credits; `None` when Platform does not know it.
    fn balance(
        &self,
        identity: Identifier,
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// The preorder and the domain, with `fund` for a contested label.
    fn submit(
        &self,
        wallet: &PlatformWallet,
        identity: Identifier,
        label: &str,
        fund: Option<u64>,
        signer: &VaultIdentitySigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

pub(crate) struct SdkNet(pub(super) Arc<dash_sdk::Sdk>);

impl NameNet for SdkNet {
    fn version(&self) -> &PlatformVersion {
        self.0.version()
    }

    async fn lookup(&self, check: &UsernameCheck) -> Result<Lookup, PlatformError> {
        lookup(&self.0, check).await
    }

    async fn contest_end(&self, normalized: &str) -> Option<u64> {
        contest_end(&self.0, normalized).await
    }

    async fn balance(&self, identity: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(IdentityBalance::fetch(&self.0, identity).await?)
    }

    async fn submit(
        &self,
        wallet: &PlatformWallet,
        identity: Identifier,
        label: &str,
        fund: Option<u64>,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        wallet
            .identity()
            .register_name_with_external_signer(&identity, label, fund, signer)
            .await?;
        Ok(())
    }
}
