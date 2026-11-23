//! The ProTx owner flows' records (MG-04 §3, mn-engine §5) as the renderer
//! sees them (MG-10 §3.1): registration and registrar-update requests and
//! previews, the keychain picks, and the wallet's ProTx operations. Shared
//! masternode calls take and return the same choices and operations. Enum
//! values are snake_case and data enums are tagged by `kind`, as the engine
//! writes them; field names are camelCase. Duffs are numbers.

use dw_engine::masternodes as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::governance::MasternodeTypeName;
use super::mirror_enum;

impl From<MasternodeTypeName> for engine::MasternodeType {
    fn from(x: MasternodeTypeName) -> Self {
        match x {
            MasternodeTypeName::Regular => Self::Regular,
            MasternodeTypeName::Evonode => Self::Evonode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralChoice {
    FundNew,
    ExistingUtxo { outpoint: String },
    External { outpoint: String, address: String },
}

impl From<CollateralChoice> for engine::CollateralChoice {
    fn from(x: CollateralChoice) -> Self {
        match x {
            CollateralChoice::FundNew => Self::FundNew,
            CollateralChoice::ExistingUtxo { outpoint } => Self::ExistingUtxo { outpoint },
            CollateralChoice::External { outpoint, address } => Self::External { outpoint, address },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CollateralKindName {
    FundNew,
    ExistingUtxo,
    External,
}

mirror_enum!(engine::CollateralKind => CollateralKindName { FundNew, ExistingUtxo, External });

/// The operator's BLS public key: given, the keychain's, or the next unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperatorChoice {
    /// 96 hex characters, basic serialization.
    Given { public_key: String },
    Keychain { index: u32 },
    NextUnused,
}

impl From<OperatorChoice> for engine::OperatorChoice {
    fn from(c: OperatorChoice) -> Self {
        match c {
            OperatorChoice::Given { public_key } => Self::Given { public_key },
            OperatorChoice::Keychain { index } => Self::Keychain { index },
            OperatorChoice::NextUnused => Self::NextUnused,
        }
    }
}

/// The voting key: the keychain's, another wallet's address, or the next
/// unused keychain key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum VotingChoice {
    Keychain { index: u32 },
    Address { address: String },
    NextUnused,
}

impl From<VotingChoice> for engine::VotingChoice {
    fn from(c: VotingChoice) -> Self {
        match c {
            VotingChoice::Keychain { index } => Self::Keychain { index },
            VotingChoice::Address { address } => Self::Address { address },
            VotingChoice::NextUnused => Self::NextUnused,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayoutChoice {
    NewAddress,
    Address { address: String },
}

impl From<PayoutChoice> for engine::PayoutChoice {
    fn from(x: PayoutChoice) -> Self {
        match x {
            PayoutChoice::NewAddress => Self::NewAddress,
            PayoutChoice::Address { address } => Self::Address { address },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PlatformNodeChoice {
    Given {
        node_id: String,
    },
    Keychain {
        index: u32,
    },
    NextUnused,
}

impl From<PlatformNodeChoice> for engine::PlatformNodeChoice {
    fn from(x: PlatformNodeChoice) -> Self {
        match x {
            PlatformNodeChoice::Given { node_id } => Self::Given { node_id },
            PlatformNodeChoice::Keychain { index } => Self::Keychain { index },
            PlatformNodeChoice::NextUnused => Self::NextUnused,
        }
    }
}

/// An evonode's Platform fields; `null` ports take the network's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EvonodeTerms {
    pub node: PlatformNodeChoice,
    pub p2p_port: Option<u16>,
    pub http_port: Option<u16>,
}

impl From<EvonodeTerms> for engine::EvonodeTerms {
    fn from(x: EvonodeTerms) -> Self {
        let EvonodeTerms {
            node,
            p2p_port,
            http_port,
        } = x;
        Self {
            node: node.into(),
            p2p_port,
            http_port,
        }
    }
}

/// A ProRegTx to build (MG-04 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MnRegistrationRequest {
    pub kind: MasternodeTypeName,
    pub collateral: CollateralChoice,
    /// `a.b.c.d[:port]`.
    pub service: String,
    pub operator: OperatorChoice,
    /// Basis points, 0..=10000.
    pub operator_reward: u16,
    /// `null`: the lowest unused owner key.
    pub owner_index: Option<u32>,
    pub voting: VotingChoice,
    pub payout: PayoutChoice,
    /// Set exactly for an evonode.
    pub evonode: Option<EvonodeTerms>,
}

impl From<MnRegistrationRequest> for engine::RegistrationRequest {
    fn from(x: MnRegistrationRequest) -> Self {
        let MnRegistrationRequest {
            kind,
            collateral,
            service,
            operator,
            operator_reward,
            owner_index,
            voting,
            payout,
            evonode,
        } = x;
        Self {
            kind: kind.into(),
            collateral: collateral.into(),
            service,
            operator: operator.into(),
            operator_reward,
            owner_index,
            voting: voting.into(),
            payout: payout.into(),
            evonode: evonode.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformFacts {
    pub node_id: String,
    pub p2p_port: u16,
    pub http_port: u16,
}

impl From<engine::PlatformFacts> for PlatformFacts {
    fn from(x: engine::PlatformFacts) -> Self {
        let engine::PlatformFacts {
            node_id,
            p2p_port,
            http_port,
        } = x;
        Self {
            node_id,
            p2p_port,
            http_port,
        }
    }
}

/// What a ProTx carries, for display. No secret, no signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProTxTerms {
    pub masternode_type: MasternodeTypeName,
    pub version: u16,
    pub service: String,
    pub owner_address: String,
    pub owner_index: Option<u32>,
    pub voting_address: String,
    pub voting_index: Option<u32>,
    pub operator_public_key: String,
    pub operator_index: Option<u32>,
    /// Basis points.
    pub operator_reward: u16,
    /// Empty for a `new_address` payout before registration.
    pub payout_address: String,
    pub collateral: Option<CollateralKindName>,
    pub collateral_outpoint: Option<String>,
    pub collateral_address: Option<String>,
    pub platform: Option<PlatformFacts>,
    pub platform_node_index: Option<u32>,
}

impl From<engine::ProTxTerms> for ProTxTerms {
    fn from(x: engine::ProTxTerms) -> Self {
        let engine::ProTxTerms {
            masternode_type,
            version,
            service,
            owner_address,
            owner_index,
            voting_address,
            voting_index,
            operator_public_key,
            operator_index,
            operator_reward,
            payout_address,
            collateral,
            collateral_outpoint,
            collateral_address,
            platform,
            platform_node_index,
        } = x;
        Self {
            masternode_type: masternode_type.into(),
            version,
            service,
            owner_address,
            owner_index,
            voting_address,
            voting_index,
            operator_public_key,
            operator_index,
            operator_reward,
            payout_address,
            collateral: collateral.map(Into::into),
            collateral_outpoint,
            collateral_address,
            platform: platform.map(Into::into),
            platform_node_index,
        }
    }
}

/// A registration's plan before anything is locked or signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MnRegistrationPreview {
    pub terms: ProTxTerms,
    pub fee: u64,
    /// What the `masternodeOp` grant must cover; a fund-new collateral stays
    /// in the wallet.
    pub total_debit: u64,
    pub collateral_amount: u64,
}

impl From<engine::RegistrationPreview> for MnRegistrationPreview {
    fn from(x: engine::RegistrationPreview) -> Self {
        let engine::RegistrationPreview {
            terms,
            fee,
            total_debit,
            collateral_amount,
        } = x;
        Self {
            terms: terms.into(),
            fee,
            total_debit,
            collateral_amount,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum KeepOrOperator {
    Keep,
    Set(OperatorChoice),
}

impl From<KeepOrOperator> for engine::KeepOr<engine::OperatorChoice> {
    fn from(x: KeepOrOperator) -> Self {
        match x {
            KeepOrOperator::Keep => Self::Keep,
            KeepOrOperator::Set(c) => Self::Set(c.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum KeepOrVoting {
    Keep,
    Set(VotingChoice),
}

impl From<KeepOrVoting> for engine::KeepOr<engine::VotingChoice> {
    fn from(x: KeepOrVoting) -> Self {
        match x {
            KeepOrVoting::Keep => Self::Keep,
            KeepOrVoting::Set(c) => Self::Set(c.into()),
        }
    }
}

/// A ProUpRegTx to build: operator and voting kept or set; the payout
/// always named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOrOperator,
    pub voting: KeepOrVoting,
    pub payout: PayoutChoice,
}

impl From<UpdateRegistrarRequest> for engine::UpdateRegistrarRequest {
    fn from(x: UpdateRegistrarRequest) -> Self {
        let UpdateRegistrarRequest {
            pro_tx_hash,
            operator,
            voting,
            payout,
        } = x;
        Self {
            pro_tx_hash,
            operator: operator.into(),
            voting: voting.into(),
            payout: payout.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerKeySource {
    Keychain { index: u32 },
    Attached,
}

impl From<engine::OwnerKeySource> for OwnerKeySource {
    fn from(x: engine::OwnerKeySource) -> Self {
        match x {
            engine::OwnerKeySource::Keychain { index } => Self::Keychain { index },
            engine::OwnerKeySource::Attached => Self::Attached,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRegistrarPreview {
    pub terms: ProTxTerms,
    pub version: u16,
    pub fee: u64,
    pub total_debit: u64,
    pub owner_key: OwnerKeySource,
    pub operator_changes: bool,
    pub voting_changes: bool,
    /// `null` when the wallet holds no payout of the masternode.
    pub payout_changes: Option<bool>,
    /// A new operator key bans the masternode until the operator's service
    /// update.
    pub pose_bans: bool,
}

impl From<engine::UpdateRegistrarPreview> for UpdateRegistrarPreview {
    fn from(x: engine::UpdateRegistrarPreview) -> Self {
        let engine::UpdateRegistrarPreview {
            terms,
            version,
            fee,
            total_debit,
            owner_key,
            operator_changes,
            voting_changes,
            payout_changes,
            pose_bans,
        } = x;
        Self {
            terms: terms.into(),
            version,
            fee,
            total_debit,
            owner_key: owner_key.into(),
            operator_changes,
            voting_changes,
            payout_changes,
            pose_bans,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyPick {
    pub index: u32,
    pub address: Option<String>,
    /// Operator keys, basic BLS hex.
    pub public_key: Option<String>,
    pub node_id: Option<String>,
}

impl From<engine::KeyPick> for KeyPick {
    fn from(x: engine::KeyPick) -> Self {
        let engine::KeyPick {
            index,
            address,
            public_key,
            node_id,
        } = x;
        Self {
            index,
            address,
            public_key,
            node_id,
        }
    }
}

/// The keychain keys a registration would use by default. Public only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProTxKeys {
    pub owner: KeyPick,
    pub voting: KeyPick,
    pub operator: KeyPick,
    pub platform_node: Option<KeyPick>,
}

impl From<engine::ProTxKeys> for ProTxKeys {
    fn from(x: engine::ProTxKeys) -> Self {
        let engine::ProTxKeys {
            owner,
            voting,
            operator,
            platform_node,
        } = x;
        Self {
            owner: owner.into(),
            voting: voting.into(),
            operator: operator.into(),
            platform_node: platform_node.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProTxOperationKindName {
    Registration,
    UpdateRegistrar,
    UpdateShare,
    Dissolve,
}

mirror_enum!(engine::ProTxOperationKind => ProTxOperationKindName {
    Registration, UpdateRegistrar, UpdateShare, Dissolve,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProTxOperationStateName {
    NeedsSignature,
    InputsSpent,
    Signed,
    NotSent,
    Sent,
    Confirmed { height: u32 },
    Conflicted,
}

impl From<engine::ProTxOperationState> for ProTxOperationStateName {
    fn from(x: engine::ProTxOperationState) -> Self {
        use engine::ProTxOperationState as S;
        match x {
            S::NeedsSignature => Self::NeedsSignature,
            S::InputsSpent => Self::InputsSpent,
            S::Signed => Self::Signed,
            S::NotSent => Self::NotSent,
            S::Sent => Self::Sent,
            S::Confirmed { height } => Self::Confirmed { height },
            S::Conflicted => Self::Conflicted,
        }
    }
}

/// What the collateral key signs (External): Core's `signmessage
/// <address> <message>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignRequest {
    pub address: String,
    pub message: String,
}

impl From<engine::SignRequest> for SignRequest {
    fn from(x: engine::SignRequest) -> Self {
        let engine::SignRequest { address, message } = x;
        Self { address, message }
    }
}

/// One of the wallet's ProTx operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProTxOperation {
    pub id: String,
    pub kind: ProTxOperationKindName,
    pub pro_tx_hash: Option<String>,
    pub txid: Option<String>,
    pub state: ProTxOperationStateName,
    pub sign_request: Option<SignRequest>,
    pub fee: u64,
    /// The cap a resend's `masternodeOp` grant takes.
    pub debit: u64,
    pub overdue: bool,
    pub created_at: u64,
    pub signed_at: Option<u64>,
    pub sent_at: Option<u64>,
    /// The last attempt's failure code.
    pub last_error: Option<String>,
    pub terms: ProTxTerms,
}

impl From<engine::ProTxOperation> for ProTxOperation {
    fn from(x: engine::ProTxOperation) -> Self {
        let engine::ProTxOperation {
            id,
            kind,
            pro_tx_hash,
            txid,
            state,
            sign_request,
            fee,
            debit,
            overdue,
            created_at,
            signed_at,
            sent_at,
            last_error,
            terms,
        } = x;
        Self {
            id,
            kind: kind.into(),
            pro_tx_hash,
            txid,
            state: state.into(),
            sign_request: sign_request.map(Into::into),
            fee,
            debit,
            overdue,
            created_at,
            signed_at,
            sent_at,
            last_error,
            terms: terms.into(),
        }
    }
}
