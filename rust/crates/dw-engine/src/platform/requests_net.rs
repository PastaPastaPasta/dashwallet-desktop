//! What the contact-request writes (and DP4-01's profile write) ask of
//! Platform, behind a seam (DP2-02, as `names_net.rs`): [`SdkRequests`] asks
//! the SDK and platform-wallet; `requests_tests.rs` scripts it, to reach
//! every refusal and write boundary without a network. Not part of the
//! facade.

use dash_sdk::drive::query::{WhereClause, WhereOperator};
use dash_sdk::platform::transition::put_document::PutDocument;
use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dash_sdk::query_types::IdentityBalance;
use dpp::data_contract::DataContract;
use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::data_contract::document_type::accessors::DocumentTypeV0Getters;
use dpp::document::{Document, DocumentV0, DocumentV0Getters, INITIAL_REVISION};
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::platform_value::platform_value;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use platform_wallet::{
    AutoAcceptProofSource, ContactInfoPublishOutcome, ContactRequest, PlatformWallet,
};

use super::super::contacts::PrivateDetails;
use super::super::profiles::{
    PROFILE, ProfileDocument, ProfileProperties, ProfileSchema, schema_of,
};
use super::super::registration_flow::dashpay_contract;
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
    /// `owner`'s DashPay `profile` document on Platform, if any (DP4-01;
    /// one per owner, the contract's unique `$ownerId` index).
    fn profile_document(
        &self,
        owner: Identifier,
    ) -> impl Future<Output = Result<Option<ProfileDocument>, PlatformError>> + Send;
    /// The `profile` document type of the DashPay contract Platform runs,
    /// proved; `None` when it has none (D21).
    fn profile_schema(
        &self,
    ) -> impl Future<Output = Result<Option<ProfileSchema>, PlatformError>> + Send;
    /// Puts `owner`'s profile with exactly `properties`, signed with `key`:
    /// a replacement of `current` (its id and revision) at the next
    /// revision, else a creation.
    fn put_profile(
        &self,
        owner: Identifier,
        current: Option<(Identifier, u64)>,
        properties: ProfileProperties,
        key: &IdentityPublicKey,
        signer: &VaultIdentitySigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
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

    async fn profile_document(
        &self,
        owner: Identifier,
    ) -> Result<Option<ProfileDocument>, PlatformError> {
        let query = DocumentQuery::new(dashpay_contract()?, PROFILE)
            .map_err(|e| PlatformError::Internal {
                detail: format!("the profile query: {e}"),
            })?
            .with_where(WhereClause {
                field: "$ownerId".into(),
                operator: WhereOperator::Equal,
                value: platform_value!(owner),
            })
            .with_limit(1);
        let documents = Document::fetch_many(&self.0, query).await?;
        Ok(documents
            .into_values()
            .flatten()
            .next()
            .map(|d| ProfileDocument {
                id: d.id(),
                revision: d.revision().unwrap_or(INITIAL_REVISION),
                properties: d.properties().clone(),
            }))
    }

    async fn profile_schema(&self) -> Result<Option<ProfileSchema>, PlatformError> {
        let contract = DataContract::fetch(&self.0, dashpay_contract::ID).await?;
        Ok(contract.and_then(|c| {
            let profile = c.document_type_for_name(PROFILE).ok()?;
            Some(schema_of(profile.properties()))
        }))
    }

    async fn put_profile(
        &self,
        owner: Identifier,
        current: Option<(Identifier, u64)>,
        properties: ProfileProperties,
        key: &IdentityPublicKey,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        let contract = dashpay_contract()?;
        let document_type = (contract.document_type_for_name(PROFILE))
            .map_err(|e| PlatformError::Internal {
                detail: format!("the profile document type: {e}"),
            })?
            .to_owned_document_type();
        // A creation's id is drawn by the SDK with its entropy.
        let (id, revision) =
            current.map_or((Identifier::default(), None), |(id, r)| (id, Some(r + 1)));
        let document = Document::V0(DocumentV0 {
            id,
            owner_id: owner,
            properties,
            revision,
            ..DocumentV0::default()
        });
        document
            .put_to_platform_and_wait_for_response(
                &self.0,
                document_type,
                None,
                key.clone(),
                None,
                signer,
                None,
            )
            .await?;
        Ok(())
    }
}
