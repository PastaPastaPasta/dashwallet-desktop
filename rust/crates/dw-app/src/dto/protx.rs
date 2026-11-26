//! A masternode registration's records (MG-04 §3, mn-engine §5) as the
//! renderer sees them (MG-10 §3.1): the request and its choices, the
//! preview, and the keychain picks. Shared masternode calls take the same
//! operator and voting choices. Enum
//! values are snake_case and data enums are tagged by `kind`, as the engine
//! writes them; field names are camelCase. Duffs are numbers.

use dw_engine::masternodes as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::governance::MasternodeTypeName;
use super::masternodes::{PayoutChoice, ProTxTerms};

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
            CollateralChoice::External { outpoint, address } => {
                Self::External { outpoint, address }
            }
        }
    }
}

/// The operator's BLS public key: given, the keychain's, or the next unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperatorChoice {
    /// 96 hex characters, basic serialization.
    Given {
        public_key: String,
    },
    Keychain {
        index: u32,
    },
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
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PlatformNodeChoice {
    Given { node_id: String },
    Keychain { index: u32 },
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

/// A keychain key: its index and its address, public key or node id.
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
