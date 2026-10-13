//! What the contact-request writes ask of Platform, behind a seam (DP2-02,
//! as `names_net.rs`): [`SdkRequests`] asks the SDK and platform-wallet;
//! `requests_tests.rs` scripts it, to reach every refusal and write
//! boundary without a network. Not part of the facade.

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::prelude::Identifier;
use platform_wallet::{
    AutoAcceptProofSource, ContactInfoPublishOutcome, ContactRequest, PlatformWallet,
};

use super::super::contacts::PrivateDetails;
use super::{Arc, Future, PlatformError, VaultContactCrypto, VaultIdentitySigner};

/// What a contact-request write asks of Platform.
pub(crate) trait RequestNet: Send + Sync {
    /// An identity as Platform has it (its keys); `None` when unknown.
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<Identity>, PlatformError>> + Send;
    /// The identity's credits, proved; `None` when Platform does not know it.
    fn balance(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// Re-fetches the wallet's identity into the library (keys, revision,
    /// balance) and returns it.
    fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> impl Future<Output = Result<Identity, PlatformError>> + Send;
    /// `sender`'s request to `recipient` on Platform, if any: whether a
    /// write that failed landed after all.
    fn sent_to(
        &self,
        wallet: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
    ) -> impl Future<Output = Result<Option<ContactRequest>, PlatformError>> + Send;
    /// Sends a contact request (platform-wallet records it as sent).
    fn send(
        &self,
        wallet: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
        proof: AutoAcceptProofSource,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> impl Future<Output = Result<ContactRequest, PlatformError>> + Send;
    /// Accepts the incoming `request`: sends the reverse one, or adopts it
    /// when it is already on Platform (platform-wallet establishes the
    /// contact). Returns our side's request.
    fn accept(
        &self,
        wallet: &PlatformWallet,
        request: &ContactRequest,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> impl Future<Output = Result<ContactRequest, PlatformError>> + Send;
    /// An IdentityUpdate adding `keys` (platform-wallet applies them to its
    /// copy of the identity once Platform confirms).
    fn add_keys(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        keys: Vec<IdentityPublicKey>,
        signer: &VaultIdentitySigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// Publishes `owner`'s private details of `contact` as its
    /// `contactInfo` (DP2-04). platform-wallet saves them with the contact
    /// first, whatever the publish does, and defers it while the identity
    /// has fewer than two contacts.
    fn set_contact_info(
        &self,
        wallet: &PlatformWallet,
        owner: Identifier,
        contact: Identifier,
        details: &PrivateDetails,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> impl Future<Output = Result<ContactInfoPublishOutcome, PlatformError>> + Send;
}

pub(crate) struct SdkRequests(pub(crate) Arc<dash_sdk::Sdk>);

impl RequestNet for SdkRequests {
    async fn identity(&self, id: Identifier) -> Result<Option<Identity>, PlatformError> {
        Ok(Identity::fetch(&self.0, id).await?)
    }

    async fn balance(&self, id: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(IdentityBalance::fetch(&self.0, id).await?)
    }

    async fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<Identity, PlatformError> {
        Ok(wallet.identity().refresh_identity(&id).await?)
    }

    async fn sent_to(
        &self,
        wallet: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
    ) -> Result<Option<ContactRequest>, PlatformError> {
        let sent = wallet
            .identity()
            .dashpay()
            .sent_contact_requests(&sender)
            .await?;
        Ok(sent.into_iter().find(|r| r.recipient_id == recipient))
    }

    async fn send(
        &self,
        wallet: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
        proof: AutoAcceptProofSource,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> Result<ContactRequest, PlatformError> {
        Ok(wallet
            .identity()
            .dashpay()
            .send_contact_request_with_external_signer(
                &sender, &recipient, None, proof, signer, crypto,
            )
            .await?)
    }

    async fn accept(
        &self,
        wallet: &PlatformWallet,
        request: &ContactRequest,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> Result<ContactRequest, PlatformError> {
        let contact = wallet
            .identity()
            .dashpay()
            .accept_contact_request_with_external_signer(request, signer, crypto)
            .await?;
        Ok(contact.outgoing_request)
    }

    async fn add_keys(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        keys: Vec<IdentityPublicKey>,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        Ok(wallet
            .identity()
            .update_identity_with_external_signer(&id, keys, Vec::new(), signer, None)
            .await?)
    }

    async fn set_contact_info(
        &self,
        wallet: &PlatformWallet,
        owner: Identifier,
        contact: Identifier,
        details: &PrivateDetails,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> Result<ContactInfoPublishOutcome, PlatformError> {
        Ok(wallet
            .identity()
            .dashpay()
            .set_contact_info_with_external_signer(
                &owner,
                &contact,
                details.alias.clone(),
                details.note.clone(),
                details.hidden,
                signer,
                crypto,
            )
            .await?)
    }
}
