//! The records of the ProTx owner flows (MG-04 §3, mn-engine §5). Same
//! conventions as `records.rs`: display-order hex hashes, `"<txid>-<n>"`
//! outpoints, duffs, Unix seconds. No record holds a secret, and none an
//! operator key other than its public key.

use serde::{Deserialize, Serialize};

use super::records::MasternodeType;

/// Where a registration's collateral comes from (UX-SPEC §4.25.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralChoice {
    /// The registration pays the collateral to a fresh address of this
    /// wallet (`protx register_fund`).
    FundNew,
    /// A coin of this wallet of exactly the type's amount (`protx register`).
    ExistingUtxo { outpoint: String },
    /// Another wallet's coin and its P2PKH address; the user signs the
    /// message there (`register_prepare` / `register_submit`).
    External { outpoint: String, address: String },
}

/// What a row's collateral was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollateralKind {
    FundNew,
    ExistingUtxo,
    External,
}

impl CollateralChoice {
    pub fn kind(&self) -> CollateralKind {
        match self {
            Self::FundNew => CollateralKind::FundNew,
            Self::ExistingUtxo { .. } => CollateralKind::ExistingUtxo,
            Self::External { .. } => CollateralKind::External,
        }
    }
}

/// The operator key: a BLS **public** key, given or the keychain's
/// (MG-04 D14). The wallet never holds the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperatorChoice {
    /// 96 hex characters, basic serialization.
    Given { public_key: String },
    /// The keychain's operator public key at `index`.
    Keychain { index: u32 },
    /// The lowest keychain index whose key no masternode uses (D13).
    NextUnused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VotingChoice {
    Keychain {
        index: u32,
    },
    /// Another wallet's P2PKH address (a voting key held elsewhere).
    Address {
        address: String,
    },
    /// The lowest unused keychain voting index (D13).
    NextUnused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayoutChoice {
    /// A fresh receive address of this wallet, issued at registration.
    NewAddress,
    /// P2PKH or P2SH, this wallet's or another's (MG-00 D28).
    Address { address: String },
}

/// An evonode's platform node id (40 hex) or the keychain's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlatformNodeChoice {
    Given { node_id: String },
    Keychain { index: u32 },
    NextUnused,
}

/// An evonode's version-2 Platform fields (D18). `None` ports: the
/// network's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvonodeTerms {
    pub node: PlatformNodeChoice,
    pub p2p_port: Option<u16>,
    pub http_port: Option<u16>,
}

/// A ProRegTx to build (MG-04 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub kind: MasternodeType,
    pub collateral: CollateralChoice,
    /// `a.b.c.d[:port]`, required (D17).
    pub service: String,
    pub operator: OperatorChoice,
    /// Basis points, 0..=10000 (D16).
    pub operator_reward: u16,
    /// `None`: the lowest unused owner key (D13).
    pub owner_index: Option<u32>,
    pub voting: VotingChoice,
    pub payout: PayoutChoice,
    /// `Some` exactly for an evonode.
    pub evonode: Option<EvonodeTerms>,
}

/// What an evonode registers on Platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformFacts {
    pub node_id: String,
    pub p2p_port: u16,
    pub http_port: u16,
}

/// What a ProTx carries, for display: addresses, keys and fields, as
/// strings. No secret, no signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxTerms {
    pub masternode_type: MasternodeType,
    /// Payload version (2 at this pin, D1).
    pub version: u16,
    pub service: String,
    pub owner_address: String,
    /// The keychain owner index, when the owner key is the keychain's.
    pub owner_index: Option<u32>,
    pub voting_address: String,
    pub voting_index: Option<u32>,
    pub operator_public_key: String,
    pub operator_index: Option<u32>,
    pub operator_reward: u16,
    pub payout_address: String,
    pub collateral: Option<CollateralKind>,
    /// The collateral outpoint: an existing coin's, or (once signed) the
    /// fund-new output of the ProRegTx.
    pub collateral_outpoint: Option<String>,
    /// The fund-new collateral address, or an existing coin's address.
    pub collateral_address: Option<String>,
    pub platform: Option<PlatformFacts>,
    pub platform_node_index: Option<u32>,
}

/// A registration's plan before anything is locked or signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationPreview {
    pub terms: ProTxTerms,
    pub fee: u64,
    /// Fee plus what pays other wallets: what the `MasternodeOp` grant must
    /// cover (MG-05 §2). The collateral of `FundNew` stays in the wallet.
    pub total_debit: u64,
    pub collateral_amount: u64,
}

/// Keep the masternode's current value, or set another (D10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum KeepOr<T> {
    Keep,
    Set(T),
}

/// A ProUpRegTx to build (MG-04 §3): the operator and voting keys kept from
/// the synced list or set; the payout always named (D10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOr<OperatorChoice>,
    pub voting: KeepOr<VotingChoice>,
    pub payout: PayoutChoice,
}

/// Which owner key signs a ProUpRegTx (D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerKeySource {
    Keychain {
        index: u32,
    },
    /// The `owner` key attached to the tracked masternode.
    Attached,
}

/// A ProUpRegTx's plan before anything is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRegistrarPreview {
    /// The masternode as it would read after the update (a `NewAddress`
    /// payout empty, D29).
    pub terms: ProTxTerms,
    pub version: u16,
    pub fee: u64,
    /// The fee: a ProUpRegTx pays no other wallet.
    pub total_debit: u64,
    pub owner_key: OwnerKeySource,
    pub operator_changes: bool,
    pub voting_changes: bool,
    /// `None` when the wallet holds no payout of the masternode (D10).
    pub payout_changes: Option<bool>,
    /// A new operator key bans the masternode until the operator's service
    /// update (Core resets its state).
    pub pose_bans: bool,
}

/// The keychain keys a registration would use by default (D13): public
/// keys and addresses only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxKeys {
    pub owner: KeyPick,
    pub voting: KeyPick,
    pub operator: KeyPick,
    pub platform_node: Option<KeyPick>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPick {
    pub index: u32,
    /// Owner and voting keys.
    pub address: Option<String>,
    /// Operator keys (basic BLS hex).
    pub public_key: Option<String>,
    /// Platform node keys.
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProTxOperationKind {
    Registration,
    UpdateRegistrar,
    /// A shared masternode's reward change, a ProUpShareTx (MG-04b §5.1).
    UpdateShare,
    /// A shared masternode's dissolution, a ProDisTx (MG-04b §5).
    Dissolve,
}

/// An operation's state, derived on every read (MG-04 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProTxOperationState {
    /// External, prepared: waiting for the collateral key's signature.
    NeedsSignature,
    /// External, prepared, and a fee input was spent since: refresh it.
    InputsSpent,
    /// Signed and saved; no hand-off started (a crash, or a lock first).
    Signed,
    /// Signed; its one attempt was definitely not sent.
    NotSent,
    /// Handed to the network, or may have been; not in a block.
    Sent,
    /// In a block of the wallet's history.
    Confirmed { height: u32 },
    /// An input was spent by another transaction in a block.
    Conflicted,
}

/// What the collateral key must sign (External).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignRequest {
    pub address: String,
    /// Core's `MakeSignString`, verbatim (`signmessage <address> <message>`).
    pub message: String,
}

/// One of the wallet's ProTx operations (`masternode_ops`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxOperation {
    pub id: String,
    pub kind: ProTxOperationKind,
    /// A registration's once signed (its txid); an update's target.
    pub pro_tx_hash: Option<String>,
    pub txid: Option<String>,
    pub state: ProTxOperationState,
    pub sign_request: Option<SignRequest>,
    pub fee: u64,
    pub created_at: u64,
    pub signed_at: Option<u64>,
    pub sent_at: Option<u64>,
    /// The code of the last attempt's failure (§6), never input.
    pub last_error: Option<String>,
    pub terms: ProTxTerms,
}
