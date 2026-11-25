//! What an App Connect login asks of Platform, behind a seam (as
//! `requests_net.rs`): [`SdkConnect`] asks the SDK and platform-wallet with
//! proofs; `app_connect_tests.rs` scripts it. Not part of the facade.

use std::future::Future;
use std::sync::Arc;

use dash_sdk::drive::query::{WhereClause, WhereOperator};
use dash_sdk::platform::contract_groups::{
    ContractGroupMembersPage, ContractGroupMembersPageQuery,
};
use dash_sdk::platform::fetch_current_no_parameters::FetchCurrent;
use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dpp::block::extended_epoch_info::ExtendedEpochInfo;
use dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0Getters;
use dpp::contract_group::ContractGroupInfo;
use dpp::data_contract::DataContract;
use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::data_contract::accessors::v1::DataContractV1Getters;
use dpp::data_contract::config::v0::DataContractConfigGettersV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, Purpose};
use dpp::platform_value::Value;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::app_connect_contract;
use platform_wallet::PlatformWallet;

use super::app_connect::ConnectUpdateSigner;
use super::app_connect_plan::{ContractFacts, GroupFacts, MAX_GROUP_MEMBERS, RESPONSE_TYPE};
use super::errors::PlatformError;
use super::signers::VaultIdentitySigner;

/// The response document's three values.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) request_hash: [u8; 20],
    pub(crate) wallet_ephemeral_pub_key: [u8; 33],
    pub(crate) payload: Vec<u8>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("request_hash", &hex::encode(self.request_hash))
            .field("payload_len", &self.payload.len())
            .finish_non_exhaustive()
    }
}

pub(crate) trait ConnectNet: Send + Sync {
    /// The current epoch's protocol version, proved.
    fn protocol_version(&self) -> impl Future<Output = Result<u32, PlatformError>> + Send;
    /// The identity with its disabled keys, and the proved block time (ms).
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(Option<Identity>, u64), PlatformError>> + Send;
    fn contract(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<ContractFacts>, PlatformError>> + Send;
    /// The group's owner, admins and every member.
    fn group(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<GroupFacts>, PlatformError>> + Send;
    /// The identity's current key for `contract` and `purpose`.
    fn current_contract_key(
        &self,
        identity: Identifier,
        contract: Identifier,
        purpose: Purpose,
    ) -> impl Future<Output = Result<Option<KeyID>, PlatformError>> + Send;
    /// Whether `owner` answered the request `request_hash` already.
    fn response_exists(
        &self,
        request_hash: [u8; 20],
        owner: Identifier,
    ) -> impl Future<Output = Result<bool, PlatformError>> + Send;
    fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> impl Future<Output = Result<Identity, PlatformError>> + Send;
    /// The MASTER-signed IdentityUpdate.
    fn update_identity(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        signer: &ConnectUpdateSigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// Creates `owner`'s `loginKeyResponse`.
    fn publish(
        &self,
        wallet: &PlatformWallet,
        owner: Identifier,
        response: &Response,
        signer: &VaultIdentitySigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

pub(crate) struct SdkConnect(pub(crate) Arc<dash_sdk::Sdk>);

impl SdkConnect {
    async fn members(
        &self,
        group: Identifier,
    ) -> Result<
        (
            Vec<Identifier>,
            Vec<(Identifier, String)>,
            Vec<(Identifier, u16)>,
        ),
        PlatformError,
    > {
        let (mut contracts, mut types, mut tokens) = (Vec::new(), Vec::new(), Vec::new());
        for first in [
            ContractGroupMembersPageQuery::contracts(group),
            ContractGroupMembersPageQuery::document_types(group),
            ContractGroupMembersPageQuery::tokens(group),
        ] {
            let mut query = Some(first.with_limit(100));
            while let Some(q) = query.take() {
                let Some(page) = ContractGroupMembersPage::fetch(&self.0, q.clone()).await? else {
                    break;
                };
                if page.is_empty() {
                    break;
                }
                match &page {
                    ContractGroupMembersPage::Contracts(v) => contracts.extend(v.iter().copied()),
                    ContractGroupMembersPage::DocumentTypes(v) => types.extend(v.iter().cloned()),
                    ContractGroupMembersPage::Tokens(v) => {
                        tokens.extend(v.iter().map(|(c, p)| (*c, *p)))
                    }
                }
                if contracts.len() + types.len() + tokens.len() > MAX_GROUP_MEMBERS {
                    return Ok((contracts, types, tokens));
                }
                query = q.after(&page);
            }
        }
        Ok((contracts, types, tokens))
    }
}

impl ConnectNet for SdkConnect {
    async fn protocol_version(&self) -> Result<u32, PlatformError> {
        let epoch = ExtendedEpochInfo::fetch_current(&self.0).await?;
        Ok(epoch.protocol_version())
    }

    async fn identity(&self, id: Identifier) -> Result<(Option<Identity>, u64), PlatformError> {
        let (identity, metadata) = Identity::fetch_with_metadata(&self.0, id, None).await?;
        Ok((identity, metadata.time_ms))
    }

    async fn contract(&self, id: Identifier) -> Result<Option<ContractFacts>, PlatformError> {
        let Some(contract) = DataContract::fetch(&self.0, id).await? else {
            return Ok(None);
        };
        let config = contract.config();
        Ok(Some(ContractFacts {
            owner: contract.owner_id(),
            document_types: contract.document_types().keys().cloned().collect(),
            encryption_requirement: config
                .requires_identity_encryption_bounded_key()
                .map(|r| r as u8),
            decryption_requirement: config
                .requires_identity_decryption_bounded_key()
                .map(|r| r as u8),
            has_tokens: !contract.tokens().is_empty(),
            readonly: config.readonly(),
        }))
    }

    async fn group(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        let Some(info) = ContractGroupInfo::fetch(&self.0, id).await? else {
            return Ok(None);
        };
        let owner = info.owner();
        let (contracts, document_types, tokens) = self.members(id).await?;
        Ok(Some(GroupFacts {
            owner: *owner.owner_id(),
            admins: owner
                .admin_ids()
                .map(|a| a.iter().copied().collect())
                .unwrap_or_default(),
            contracts,
            document_types,
            tokens,
        }))
    }

    async fn current_contract_key(
        &self,
        identity: Identifier,
        contract: Identifier,
        purpose: Purpose,
    ) -> Result<Option<KeyID>, PlatformError> {
        use dash_sdk::platform::identities_contract_keys_query::IdentitiesContractKeysQuery;
        use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
        let query =
            IdentitiesContractKeysQuery::new(vec![identity], contract, None, vec![purpose])?;
        let keys: Option<dpp::identity::identities_contract_keys::IdentitiesContractKeys> =
            Fetch::fetch(&self.0, query).await?;
        Ok(keys
            .and_then(|mut k| k.remove(&identity))
            .and_then(|mut by_purpose| by_purpose.remove(&purpose).flatten())
            .map(|k| k.id()))
    }

    async fn response_exists(
        &self,
        request_hash: [u8; 20],
        owner: Identifier,
    ) -> Result<bool, PlatformError> {
        let contract = DataContract::fetch(&self.0, app_connect_contract::ID)
            .await?
            .ok_or(PlatformError::Unavailable)?;
        let mut query = DocumentQuery::new(Arc::new(contract), RESPONSE_TYPE).map_err(|_| {
            PlatformError::Internal {
                detail: "response query".into(),
            }
        })?;
        query.where_clauses = vec![
            WhereClause {
                field: "appEphemeralPubKeyHash".into(),
                operator: WhereOperator::Equal,
                value: Value::Bytes(request_hash.to_vec()),
            },
            WhereClause {
                field: "$ownerId".into(),
                operator: WhereOperator::Equal,
                value: Value::Identifier(owner.to_buffer()),
            },
        ];
        query.limit = 1;
        let found = dpp::document::Document::fetch_many(&self.0, query).await?;
        Ok(found.into_values().flatten().next().is_some())
    }

    async fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<Identity, PlatformError> {
        Ok(wallet.identity().refresh_identity(&id).await?)
    }

    async fn update_identity(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        signer: &ConnectUpdateSigner,
    ) -> Result<(), PlatformError> {
        Ok(wallet
            .identity()
            .update_identity_with_external_signer(&id, add, disable, signer, None)
            .await?)
    }

    async fn publish(
        &self,
        wallet: &PlatformWallet,
        owner: Identifier,
        response: &Response,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        let properties = serde_json::json!({
            "appEphemeralPubKeyHash": hex::encode(response.request_hash),
            "walletEphemeralPubKey": hex::encode(response.wallet_ephemeral_pub_key),
            "encryptedPayload": hex::encode(&response.payload),
        })
        .to_string();
        wallet
            .identity()
            .create_document_with_signer(
                &owner,
                &app_connect_contract::ID,
                RESPONSE_TYPE,
                &properties,
                None,
                signer,
            )
            .await?;
        Ok(())
    }
}
