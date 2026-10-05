//! Sending: transaction drafts, prepare (sign, never broadcast), broadcast.
//! Owner: E2 (engine-send). Shape follows DESIGN-opus §1.5.
//! Contract: docs/contracts/m1-engine.md §send.

use std::sync::Arc;

use crate::NetworkSession;
use crate::api::common::{OutPoint, domain_error_common, not_implemented, parse_wallet_id};

/// One payment line (QT-052/053).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Recipient {
    pub address: String,
    /// Duffs; must be above the dust threshold.
    pub amount: u64,
    /// dash-qt "Subtract fee from amount".
    pub subtract_fee_from_amount: bool,
    /// Saved to the address book when set (QT-053 "Add to address book").
    pub label: Option<String>,
    /// `message` from a `dash:` URI; stored with the transaction.
    pub message: Option<String>,
}

/// Which coins a draft may spend.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CoinSource {
    /// Any spendable coin of the standard account.
    Any,
    /// Only fully mixed CoinJoin coins (CoinJoin send page, QT-051).
    FullyMixedOnly,
    /// Exactly these outpoints (coin control, QT-068..074).
    Outpoints { outpoints: Vec<OutPoint> },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FeeMode {
    /// Engine-recommended rate for a confirmation target (QT-057; on SPV this
    /// is the minimum relay fee for every target, DESIGN-opus §1.14).
    Recommended { target_blocks: u32 },
    /// Custom rate in duffs per 1000 bytes.
    PerKb { duffs_per_kb: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChangePolicy {
    /// Fresh internal-chain address.
    Auto,
    /// Custom change address (QT-073).
    Address { address: String },
}

/// Fee and size preview of the current draft; nothing is signed or reserved.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxEstimate {
    pub fee: u64,
    /// Estimated serialized size in bytes (QT-072).
    pub size_bytes: u32,
    pub input_count: u32,
    /// Change output value; `None` when the change is dropped into the fee.
    pub change: Option<u64>,
    /// Sum the recipients receive.
    pub total_sent: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PreparedInput {
    pub outpoint: OutPoint,
    pub address: Option<String>,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PreparedOutput {
    pub address: Option<String>,
    pub amount: u64,
    pub is_change: bool,
    pub label: Option<String>,
}

/// What the confirm dialog shows (QT-059, IOS-046).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PreparedTxSummary {
    pub txid: String,
    pub fee: u64,
    pub fee_rate_per_kb: u64,
    pub size_bytes: u32,
    pub inputs: Vec<PreparedInput>,
    pub outputs: Vec<PreparedOutput>,
    /// Sum the recipients receive.
    pub total_sent: u64,
    /// `total_sent + fee`: what leaves the wallet.
    pub total_debit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BroadcastOutcome {
    pub txid: String,
    /// Peers the transaction was announced to.
    pub peers_announced: u32,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SendError {
    /// Code `send.no_recipients`.
    #[error("no recipients")]
    NoRecipients,
    /// Code `send.invalid_address` (QT-055).
    #[error("recipient {index}: invalid address")]
    InvalidAddress { index: u32 },
    /// Code `send.platform_address` (QT-067: Platform addresses are rejected on L1).
    #[error("recipient {index}: platform address")]
    PlatformAddress { index: u32 },
    /// Code `send.invalid_amount`: zero or above the maximum supply.
    #[error("recipient {index}: invalid amount")]
    InvalidAmount { index: u32 },
    /// Code `send.dust_amount`.
    #[error("recipient {index}: dust amount")]
    DustAmount { index: u32 },
    /// Code `send.duplicate_address` (QT-060).
    #[error("recipient {index}: duplicate address")]
    DuplicateAddress { index: u32 },
    /// Code `send.amount_exceeds_balance`.
    #[error("amount exceeds balance {available}")]
    AmountExceedsBalance { available: u64 },
    /// Code `send.amount_with_fee_exceeds_balance`.
    #[error("amount with fee {fee} exceeds balance {available}")]
    AmountWithFeeExceedsBalance { fee: u64, available: u64 },
    /// Code `send.insufficient_mixed_funds`: `FullyMixedOnly` cannot cover it.
    #[error("insufficient mixed funds")]
    InsufficientMixedFunds { available: u64 },
    /// Code `send.outpoint_unavailable`: spent, locked or not the wallet's.
    #[error("outpoint unavailable")]
    OutpointUnavailable { outpoint: OutPoint },
    /// Code `send.absurd_fee` (QT-058 fee cap).
    #[error("absurd fee {fee}")]
    AbsurdFee { fee: u64 },
    /// Code `send.tx_too_large`.
    #[error("transaction too large")]
    TxTooLarge,
    /// Code `send.invalid_change_address`.
    #[error("invalid change address")]
    InvalidChangeAddress,
    /// Code `send.watch_only`: the wallet has no signing keys.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `send.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `send.grant_invalid`: missing, expired or not a Spend grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `send.grant_exceeded`: the debit is above the grant's `max_duffs`.
    #[error("grant allows {max_duffs}")]
    GrantExceeded { max_duffs: u64 },
    /// Code `send.prepared_tx_spent`: the prepared transaction was already
    /// broadcast or abandoned.
    #[error("prepared transaction no longer pending")]
    PreparedTxSpent,
    /// Code `send.no_peers`: nothing to broadcast to.
    #[error("no peers")]
    NoPeers,
    /// Code `send.broadcast_rejected`: a peer rejected the transaction.
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(SendError);

impl SendError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRecipients => "send.no_recipients",
            Self::InvalidAddress { .. } => "send.invalid_address",
            Self::PlatformAddress { .. } => "send.platform_address",
            Self::InvalidAmount { .. } => "send.invalid_amount",
            Self::DustAmount { .. } => "send.dust_amount",
            Self::DuplicateAddress { .. } => "send.duplicate_address",
            Self::AmountExceedsBalance { .. } => "send.amount_exceeds_balance",
            Self::AmountWithFeeExceedsBalance { .. } => "send.amount_with_fee_exceeds_balance",
            Self::InsufficientMixedFunds { .. } => "send.insufficient_mixed_funds",
            Self::OutpointUnavailable { .. } => "send.outpoint_unavailable",
            Self::AbsurdFee { .. } => "send.absurd_fee",
            Self::TxTooLarge => "send.tx_too_large",
            Self::InvalidChangeAddress => "send.invalid_change_address",
            Self::WatchOnly => "send.watch_only",
            Self::VaultLocked => "send.vault_locked",
            Self::GrantInvalid => "send.grant_invalid",
            Self::GrantExceeded { .. } => "send.grant_exceeded",
            Self::PreparedTxSpent => "send.prepared_tx_spent",
            Self::NoPeers => "send.no_peers",
            Self::BroadcastRejected { .. } => "send.broadcast_rejected",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

/// A signed transaction held by the engine with its inputs reserved. Only
/// `TxDraft::broadcast` sends it; `TxDraft::abandon` releases the inputs.
#[derive(uniffi::Object)]
pub struct PreparedTx {
    summary: PreparedTxSummary,
}

#[uniffi::export]
impl PreparedTx {
    pub fn summary(&self) -> PreparedTxSummary {
        self.summary.clone()
    }
}

/// An editable payment for one wallet. Setters validate what they can
/// without the network; `prepare` does the rest.
#[derive(uniffi::Object)]
pub struct TxDraft {
    wallet_id: String,
}

#[uniffi::export]
impl NetworkSession {
    /// A new, empty draft for `wallet_id` (source `Any`, fee
    /// `Recommended{target_blocks: 6}`, change `Auto`).
    pub fn new_tx_draft(&self, wallet_id: String) -> Result<Arc<TxDraft>, SendError> {
        let _ = parse_wallet_id(&wallet_id)?;
        not_implemented("NetworkSession.new_tx_draft")
    }

    /// The largest single-recipient amount `source` can pay at `fee`, with
    /// the fee subtracted (iOS "Max", dash-qt "Use available balance").
    pub async fn max_spendable(
        &self,
        wallet_id: String,
        source: CoinSource,
        fee: FeeMode,
    ) -> Result<u64, SendError> {
        let _ = (parse_wallet_id(&wallet_id)?, source, fee);
        not_implemented("NetworkSession.max_spendable")
    }
}

#[uniffi::export]
impl TxDraft {
    pub fn wallet_id(&self) -> String {
        self.wallet_id.clone()
    }

    /// Replaces the recipient list. Validates addresses (network, Platform
    /// rejection), amounts and duplicates; errors carry the recipient index.
    pub fn set_recipients(&self, recipients: Vec<Recipient>) -> Result<(), SendError> {
        let _ = recipients;
        not_implemented("TxDraft.set_recipients")
    }

    pub fn set_source(&self, source: CoinSource) -> Result<(), SendError> {
        let _ = source;
        not_implemented("TxDraft.set_source")
    }

    pub fn set_fee(&self, fee: FeeMode) -> Result<(), SendError> {
        let _ = fee;
        not_implemented("TxDraft.set_fee")
    }

    pub fn set_change(&self, change: ChangePolicy) -> Result<(), SendError> {
        let _ = change;
        not_implemented("TxDraft.set_change")
    }

    /// Coin selection and fee for the current draft without signing.
    pub async fn estimate(&self) -> Result<TxEstimate, SendError> {
        not_implemented("TxDraft.estimate")
    }

    /// Selects coins, builds, signs (through the vault, with a `Spend` grant)
    /// and reserves the inputs. Never broadcasts.
    pub async fn prepare(&self, grant_id: String) -> Result<Arc<PreparedTx>, SendError> {
        let _ = grant_id;
        not_implemented("TxDraft.prepare")
    }

    /// Announces `prepared` to the network and records it in history.
    pub async fn broadcast(
        &self,
        prepared: Arc<PreparedTx>,
    ) -> Result<BroadcastOutcome, SendError> {
        let _ = prepared;
        not_implemented("TxDraft.broadcast")
    }

    /// Discards `prepared` and releases its reserved inputs. Idempotent.
    pub async fn abandon(&self, prepared: Arc<PreparedTx>) -> Result<(), SendError> {
        let _ = prepared;
        not_implemented("TxDraft.abandon")
    }
}
