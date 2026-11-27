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
use dash_sdk::platform::transition::broadcast::BroadcastStateTransition;
use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dash_sdk::query_types::IdentityNonceFetcher;
use dpp::block::extended_epoch_info::ExtendedEpochInfo;
use dpp::block::extended_epoch_info::v0::ExtendedEpochInfoV0Getters;
use dpp::contract_group::ContractGroupInfo;
use dpp::data_contract::DataContract;
use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::data_contract::accessors::v1::DataContractV1Getters;
use dpp::data_contract::config::v0::DataContractConfigGettersV0;
use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::{Identity, IdentityPublicKey, KeyID, Purpose};
use dpp::platform_value::Value;
use dpp::prelude::Identifier;
use dpp::serialization::{
    PlatformDeserializableTrusted, PlatformSerializable, PlatformSerializableWithPlatformVersion,
};
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_update_transition::IdentityUpdateTransition;
use dpp::state_transition::identity_update_transition::methods::IdentityUpdateTransitionMethodsV0;
use dpp::state_transition::proof_result::StateTransitionProofResult;
use dpp::system_data_contracts::app_connect_contract;
use platform_wallet::PlatformWallet;

use super::app_connect::{ConnectUpdateSigner, DisconnectKeyFeeSpec};
use super::app_connect_plan::{
    ContractFacts, GroupFacts, GroupInfoFacts, MAX_GROUP_MEMBERS, RESPONSE_TYPE,
};
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
    /// The identity with its disabled keys, the proved block time (ms), and
    /// the proved block height.
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(Option<Identity>, u64, u64), PlatformError>> + Send;
    fn contract(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<ContractFacts>, PlatformError>> + Send;
    /// The group's owner, admins and serialized `ContractGroupInfo` metadata
    /// alone (no member enumeration — used to price group-bound revocation).
    fn group_info(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<GroupInfoFacts>, PlatformError>> + Send;
    /// The group's owner, admins and members up to `MAX_GROUP_MEMBERS + 1`
    /// (approval refuses any larger group).
    fn group(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<GroupFacts>, PlatformError>> + Send;
    /// The group's owner, admins and every member, paging through all
    /// categories without truncating at `MAX_GROUP_MEMBERS` (used to show the
    /// complete scope of an already-approved key).
    fn group_all(
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
    /// Refreshes the SDK wallet's cached identity from Platform, propagating
    /// any refresh error. Mandatory before `update_identity`, which signs at
    /// the returned identity's next revision, and before `publish`, which is
    /// SDK cache-backed.
    fn refresh_cache(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> impl Future<Output = Result<Identity, PlatformError>> + Send;
    /// Fetches the identity with its proved block height (used by Disconnect,
    /// which signs directly from the returned identity).
    fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> impl Future<Output = Result<(Identity, u64), PlatformError>> + Send;
    /// Signs and broadcasts the MASTER-signed IdentityUpdate at
    /// `identity.revision() + 1` and exactly `nonce`.
    #[expect(clippy::too_many_arguments, reason = "one IdentityUpdate's fields")]
    fn update_identity(
        &self,
        wallet: &PlatformWallet,
        identity: &Identity,
        master_key_id: KeyID,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &ConnectUpdateSigner,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// The identity's proved nonce state (low 40 bits: tip; upper 24 bits:
    /// past-window bitset; `0` if none stored yet) and the proved block height
    /// at which it was read.
    fn identity_nonce(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(u64, u64), PlatformError>> + Send;
    /// Signs a MASTER-authorized disable-only `IdentityUpdateTransition` at
    /// `identity.revision() + 1` and `nonce`, returning its serialized bytes
    /// so it can be persisted before broadcast and re-broadcast idempotently.
    fn sign_disable_keys(
        &self,
        identity: &Identity,
        master_key_id: KeyID,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &VaultIdentitySigner,
    ) -> impl Future<Output = Result<Vec<u8>, PlatformError>> + Send;
    /// Broadcasts a previously signed disable-only `IdentityUpdateTransition`.
    fn broadcast_disable_keys(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        transition_bytes: &[u8],
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// The live Platform credit fee upper bound to disable concrete bound
    /// keys at the current proved protocol version.
    fn disconnect_fee(
        &self,
        specs: &[DisconnectKeyFeeSpec],
    ) -> impl Future<Output = Result<u64, PlatformError>> + Send;
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
        limit: Option<usize>,
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
                if limit.is_some_and(|max| contracts.len() + types.len() + tokens.len() > max) {
                    return Ok((contracts, types, tokens));
                }
                query = q.after(&page);
            }
        }
        Ok((contracts, types, tokens))
    }

    async fn fetch_group(
        &self,
        id: Identifier,
        limit: Option<usize>,
    ) -> Result<Option<GroupFacts>, PlatformError> {
        let Some(info) = ContractGroupInfo::fetch(&self.0, id).await? else {
            return Ok(None);
        };
        let owner = info.owner();
        let (contracts, document_types, tokens) = self.members(id, limit).await?;
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
}

impl ConnectNet for SdkConnect {
    async fn protocol_version(&self) -> Result<u32, PlatformError> {
        let epoch = ExtendedEpochInfo::fetch_current(&self.0).await?;
        Ok(epoch.protocol_version())
    }

    async fn identity(
        &self,
        id: Identifier,
    ) -> Result<(Option<Identity>, u64, u64), PlatformError> {
        let (identity, metadata) = Identity::fetch_with_metadata(&self.0, id, None).await?;
        Ok((identity, metadata.time_ms, metadata.height))
    }

    async fn contract(&self, id: Identifier) -> Result<Option<ContractFacts>, PlatformError> {
        let Some(contract) = DataContract::fetch(&self.0, id).await? else {
            return Ok(None);
        };
        let config = contract.config();
        let serialized_bytes = contract
            .serialize_to_bytes_with_platform_version(self.0.version())
            .map(|b| b.len())
            .map_err(|e| PlatformError::Internal {
                detail: format!("serialize contract for fee bound: {e}"),
            })?;
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
            serialized_bytes,
        }))
    }

    async fn group_info(&self, id: Identifier) -> Result<Option<GroupInfoFacts>, PlatformError> {
        let Some(info) = ContractGroupInfo::fetch(&self.0, id).await? else {
            return Ok(None);
        };
        let serialized_bytes =
            info.serialize_to_bytes()
                .map(|b| b.len())
                .map_err(|e| PlatformError::Internal {
                    detail: format!("serialize contract group info for fee bound: {e}"),
                })?;
        let owner = info.owner();
        Ok(Some(GroupInfoFacts {
            owner: *owner.owner_id(),
            admins: owner
                .admin_ids()
                .map(|a| a.iter().copied().collect())
                .unwrap_or_default(),
            serialized_bytes,
        }))
    }

    async fn group(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        self.fetch_group(id, Some(MAX_GROUP_MEMBERS)).await
    }

    async fn group_all(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        self.fetch_group(id, None).await
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

    async fn refresh_cache(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<Identity, PlatformError> {
        Ok(wallet.identity().refresh_identity(&id).await?)
    }

    async fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<(Identity, u64), PlatformError> {
        let _ = wallet.identity().refresh_identity(&id).await;
        let (identity, metadata) = Identity::fetch_with_metadata(&self.0, id, None).await?;
        let identity =
            identity.ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound))?;
        Ok((identity, metadata.height))
    }

    async fn update_identity(
        &self,
        wallet: &PlatformWallet,
        identity: &Identity,
        master_key_id: KeyID,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &ConnectUpdateSigner,
    ) -> Result<(), PlatformError> {
        let id = identity.id();
        let mut updated = identity.clone();
        let revision =
            identity
                .revision()
                .checked_add(1)
                .ok_or_else(|| PlatformError::Internal {
                    detail: "identity is at the last revision".into(),
                })?;
        updated.set_revision(revision);
        let transition = IdentityUpdateTransition::try_from_identity_with_signer(
            &updated,
            &master_key_id,
            add,
            disable,
            nonce,
            0,
            signer,
            self.0.version(),
            None,
        )
        .await
        .map_err(|e| PlatformError::Internal {
            detail: format!("sign identity update: {e}"),
        })?;
        let result = transition
            .broadcast_and_wait::<StateTransitionProofResult>(&self.0, None)
            .await;
        self.0.refresh_identity_nonce(&id).await;
        let _ = self.0.get_identity_nonce(id, false, None).await;
        if result.is_ok() {
            let _ = wallet.identity().refresh_identity(&id).await;
        }
        result.map(|_| ()).map_err(PlatformError::from)
    }

    async fn identity_nonce(&self, id: Identifier) -> Result<(u64, u64), PlatformError> {
        let (state, metadata) =
            IdentityNonceFetcher::fetch_with_metadata(&self.0, id, None).await?;
        Ok((
            state.map_or(0, |IdentityNonceFetcher(n)| n),
            metadata.height,
        ))
    }

    async fn sign_disable_keys(
        &self,
        identity: &Identity,
        master_key_id: KeyID,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &VaultIdentitySigner,
    ) -> Result<Vec<u8>, PlatformError> {
        let mut updated = identity.clone();
        updated.set_revision(identity.revision() + 1);
        let transition = IdentityUpdateTransition::try_from_identity_with_signer(
            &updated,
            &master_key_id,
            Vec::new(),
            disable,
            nonce,
            0,
            signer,
            self.0.version(),
            None,
        )
        .await
        .map_err(|e| PlatformError::Internal {
            detail: format!("sign identity update: {e}"),
        })?;
        transition
            .serialize_to_bytes()
            .map_err(|e| PlatformError::Internal {
                detail: format!("serialize identity update: {e}"),
            })
    }

    async fn broadcast_disable_keys(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
        transition_bytes: &[u8],
    ) -> Result<(), PlatformError> {
        let transition = StateTransition::deserialize_from_bytes_trusted(transition_bytes)
            .map_err(|e| PlatformError::Internal {
                detail: format!("deserialize identity update: {e}"),
            })?;
        let result = transition
            .broadcast_and_wait::<StateTransitionProofResult>(&self.0, None)
            .await;
        // Sync the SDK's cached identity nonce from Platform without bumping
        // (`bump_first = false`) so each disconnect broadcast does not reserve
        // and discard an extra unused nonce gap.
        self.0.refresh_identity_nonce(&id).await;
        let _ = self.0.get_identity_nonce(id, false, None).await;
        if result.is_ok() {
            let _ = wallet.identity().refresh_identity(&id).await;
        }
        result.map(|_| ()).map_err(PlatformError::from)
    }

    async fn disconnect_fee(&self, specs: &[DisconnectKeyFeeSpec]) -> Result<u64, PlatformError> {
        if specs.is_empty() {
            return Ok(0);
        }
        let version = self.protocol_version().await?;
        super::app_connect::disconnect_fee_at(version, specs)
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
