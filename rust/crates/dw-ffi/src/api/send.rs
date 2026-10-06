//! Sending: transaction drafts, prepare (sign, never broadcast), broadcast.
//! Owner: E2 (engine-send). Shape follows DESIGN-opus §1.5.
//! Contract: docs/contracts/m1-engine.md §2.7. Behaviour lives in
//! `dw_engine::send`; this module maps types.

use std::sync::Arc;

use crate::NetworkSession;
use crate::api::common::{NotImplementedError, OutPoint, parse_wallet_id};

/// One payment line (QT-052/053).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Recipient {
    pub address: String,
    /// Duffs; must be at least the output's dust threshold (546 for P2PKH).
    pub amount: u64,
    /// dash-qt "Subtract fee from amount".
    pub subtract_fee_from_amount: bool,
    /// Saved to the address book after a broadcast (QT-063).
    pub label: Option<String>,
    /// `message` from a `dash:` URI; stored with the transaction.
    pub message: Option<String>,
}

/// Which coins a draft may spend.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CoinSource {
    /// Any spendable coin of the standard accounts, except user-locked,
    /// reserved, immature and untrusted unconfirmed coins.
    Any,
    /// Only fully mixed CoinJoin coins (CoinJoin send page, QT-051).
    /// `NotImplemented` until CoinJoin rounds are tracked.
    FullyMixedOnly,
    /// Exactly these outpoints, all of them (coin control, QT-068..074).
    Outpoints { outpoints: Vec<OutPoint> },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FeeMode {
    /// Engine-recommended rate for a confirmation target (QT-057; on SPV this
    /// is the minimum relay fee, 1000 duff/kB, for every target 1..=1008).
    Recommended { target_blocks: u32 },
    /// Custom rate in duffs per 1000 bytes, 1000..=10,000,000.
    PerKb { duffs_per_kb: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChangePolicy {
    /// Fresh internal-chain address.
    Auto,
    /// Custom change address (QT-073). A foreign address counts against the
    /// spend cap.
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
    /// Sum the recipients receive (after any subtract-fee share).
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
    /// Pays one of this wallet's addresses (review H-3: a change output
    /// with `is_mine == false` is a foreign custom change address).
    pub is_mine: bool,
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
    /// Inputs minus outputs back to the wallet: what leaves the wallet,
    /// fee included.
    pub total_debit: u64,
    /// Paid to scripts the wallet does not own (recipients and a foreign
    /// change address), fee excluded: the figure a `Spend` grant caps.
    pub external_sent: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BroadcastOutcome {
    pub txid: String,
    /// Peers the transaction was announced to; `None`: dash-spv does not
    /// report it.
    pub peers_announced: Option<u32>,
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
    /// Code `send.outpoint_unavailable`: spent, locked, reserved, immature or
    /// not the wallet's.
    #[error("outpoint unavailable")]
    OutpointUnavailable { outpoint: OutPoint },
    /// Code `send.absurd_fee` (QT-058: above 0.1 DASH).
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
    /// Code `send.vault_locked` (also a mixing-only unlock).
    #[error("vault locked")]
    VaultLocked,
    /// Code `send.grant_invalid`: missing, expired, used or not a Spend grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `send.grant_exceeded`: `external_sent` is above the grant's
    /// `max_duffs`.
    #[error("grant allows {max_duffs}")]
    GrantExceeded { max_duffs: u64 },
    /// Code `send.prepared_tx_spent`: the prepared transaction was already
    /// broadcast, abandoned or released after a failed broadcast.
    #[error("prepared transaction no longer pending")]
    PreparedTxSpent,
    /// Code `send.no_peers`: SPV is not running or has no peer; nothing was
    /// sent and the inputs were released.
    #[error("no peers")]
    NoPeers,
    /// Code `send.broadcast_rejected`: provably not sent; inputs released.
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
    /// Code `send.amount_too_small_after_fee` (review M-5): a subtract-fee
    /// recipient would be left with dust.
    #[error("recipient {index}: too small after the fee")]
    AmountTooSmallAfterFee { index: u32 },
    /// Code `send.broadcast_unknown` (review M-7): handed to the network
    /// without an acceptance verdict. The inputs stay reserved; `broadcast`
    /// may be retried, `abandon` is refused.
    #[error("broadcast outcome unknown: {reason}")]
    BroadcastUnknown { reason: String },
}

impl NotImplementedError for SendError {
    fn not_implemented(call: &'static str) -> Self {
        Self::NotImplemented {
            call: call.to_string(),
        }
    }
}

impl From<dw_engine::EngineError> for SendError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        use dw_engine::SendFailure as F;
        let detail = e.to_string();
        match e {
            E::Send(f) => match f {
                F::NoRecipients => Self::NoRecipients,
                F::InvalidAddress { index } => Self::InvalidAddress { index },
                F::PlatformAddress { index } => Self::PlatformAddress { index },
                F::InvalidAmount { index } => Self::InvalidAmount { index },
                F::DustAmount { index } => Self::DustAmount { index },
                F::DuplicateAddress { index } => Self::DuplicateAddress { index },
                F::AmountExceedsBalance { available } => Self::AmountExceedsBalance { available },
                F::AmountWithFeeExceedsBalance { fee, available } => {
                    Self::AmountWithFeeExceedsBalance { fee, available }
                }
                F::AmountTooSmallAfterFee { index } => Self::AmountTooSmallAfterFee { index },
                F::InsufficientMixedFunds { available } => {
                    Self::InsufficientMixedFunds { available }
                }
                F::OutpointUnavailable(o) => Self::OutpointUnavailable { outpoint: o.into() },
                F::AbsurdFee { fee } => Self::AbsurdFee { fee },
                F::TxTooLarge => Self::TxTooLarge,
                F::InvalidChangeAddress => Self::InvalidChangeAddress,
                F::WatchOnly => Self::WatchOnly,
                F::VaultLocked => Self::VaultLocked,
                F::GrantInvalid => Self::GrantInvalid,
                F::GrantExceeded { max_duffs, .. } => Self::GrantExceeded { max_duffs },
                F::PreparedTxSpent => Self::PreparedTxSpent,
                F::NoPeers => Self::NoPeers,
                F::BroadcastRejected { reason } => Self::BroadcastRejected { reason },
                F::BroadcastUnknown { reason } => Self::BroadcastUnknown { reason },
            },
            E::SpvNotRunning => Self::NoPeers,
            E::InvalidConfig(_) | E::InvalidArgument(_) | E::InvalidAddress(_) => {
                Self::InvalidArgument { detail }
            }
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(_) => Self::NotImplemented { call: detail },
            _ => Self::Internal { detail },
        }
    }
}

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
            Self::AmountTooSmallAfterFee { .. } => "send.amount_too_small_after_fee",
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
            Self::BroadcastUnknown { .. } => "send.broadcast_unknown",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl CoinSource {
    fn to_engine(&self) -> Result<dw_engine::CoinSource, SendError> {
        Ok(match self {
            CoinSource::Any => dw_engine::CoinSource::Any,
            CoinSource::FullyMixedOnly => dw_engine::CoinSource::FullyMixedOnly,
            CoinSource::Outpoints { outpoints } => dw_engine::CoinSource::Outpoints(
                outpoints
                    .iter()
                    .map(OutPoint::to_core)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

impl From<FeeMode> for dw_engine::FeeMode {
    fn from(f: FeeMode) -> Self {
        match f {
            FeeMode::Recommended { target_blocks } => Self::Recommended { target_blocks },
            FeeMode::PerKb { duffs_per_kb } => Self::PerKb(duffs_per_kb),
        }
    }
}

impl From<dw_engine::PreparedSummary> for PreparedTxSummary {
    fn from(s: dw_engine::PreparedSummary) -> Self {
        Self {
            txid: s.txid,
            fee: s.fee,
            fee_rate_per_kb: s.fee_rate_per_kb,
            size_bytes: s.size_bytes,
            inputs: s
                .inputs
                .into_iter()
                .map(|i| PreparedInput {
                    outpoint: i.outpoint.into(),
                    address: i.address,
                    amount: i.amount,
                })
                .collect(),
            outputs: s
                .outputs
                .into_iter()
                .map(|o| PreparedOutput {
                    address: o.address,
                    amount: o.amount,
                    is_change: o.is_change,
                    label: o.label,
                    is_mine: o.is_mine,
                })
                .collect(),
            total_sent: s.total_sent,
            total_debit: s.total_debit,
            external_sent: s.external_sent,
        }
    }
}

/// A signed transaction held by the engine with its inputs reserved. Only
/// `TxDraft::broadcast` sends it; `TxDraft::abandon`, or releasing the last
/// reference while it is pending, releases the inputs.
#[derive(Debug, uniffi::Object)]
pub struct PreparedTx {
    inner: Arc<dw_engine::PreparedTx>,
}

#[uniffi::export]
impl PreparedTx {
    pub fn summary(&self) -> PreparedTxSummary {
        self.inner.summary().clone().into()
    }
}

/// An editable payment for one wallet. Setters validate what they can
/// without the network; `estimate` and `prepare` read the wallet.
#[derive(Debug, uniffi::Object)]
pub struct TxDraft {
    inner: Arc<dw_engine::TxDraft>,
}

#[uniffi::export]
impl NetworkSession {
    /// A new, empty draft for `wallet_id` (source `Any`, fee
    /// `Recommended{target_blocks: 6}`, change `Auto`).
    pub fn new_tx_draft(&self, wallet_id: String) -> Result<Arc<TxDraft>, SendError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(Arc::new(TxDraft {
            inner: self.inner.new_tx_draft(id)?,
        }))
    }

    /// The spendable amount of `source` for dash-qt's "Use available
    /// balance" (review M-4): put this minus the other recipients' amounts
    /// in the entry and set `subtract_fee_from_amount`. Excludes user-locked,
    /// reserved, immature and untrusted unconfirmed coins. `fee` is
    /// validated only; with subtract-fee the fee comes out of the amount.
    pub async fn max_spendable(
        &self,
        wallet_id: String,
        source: CoinSource,
        fee: FeeMode,
    ) -> Result<u64, SendError> {
        let id = parse_wallet_id(&wallet_id)?;
        let source = source.to_engine()?;
        Ok(self.inner.max_spendable(id, source, fee.into()).await?)
    }
}

#[uniffi::export]
impl TxDraft {
    pub fn wallet_id(&self) -> String {
        self.inner.wallet_id().to_string()
    }

    /// Replaces the recipient list. Validates addresses (network, Platform
    /// rejection), amounts and duplicates; errors carry the recipient index.
    pub fn set_recipients(&self, recipients: Vec<Recipient>) -> Result<(), SendError> {
        let recipients = recipients
            .into_iter()
            .map(|r| dw_engine::Recipient {
                address: r.address,
                amount: r.amount,
                subtract_fee_from_amount: r.subtract_fee_from_amount,
                label: r.label,
                message: r.message,
            })
            .collect();
        Ok(self.inner.set_recipients(recipients)?)
    }

    /// Coin source. Outpoints must be distinct; whether they are spendable
    /// is checked by `estimate`/`prepare` (`send.outpoint_unavailable`).
    pub fn set_source(&self, source: CoinSource) -> Result<(), SendError> {
        Ok(self.inner.set_source(source.to_engine()?)?)
    }

    pub fn set_fee(&self, fee: FeeMode) -> Result<(), SendError> {
        Ok(self.inner.set_fee(fee.into())?)
    }

    pub fn set_change(&self, change: ChangePolicy) -> Result<(), SendError> {
        let change = match change {
            ChangePolicy::Auto => dw_engine::ChangePolicy::Auto,
            ChangePolicy::Address { address } => dw_engine::ChangePolicy::Address(address),
        };
        Ok(self.inner.set_change(change)?)
    }

    /// Coin selection and fee for the current draft without signing.
    pub async fn estimate(&self) -> Result<TxEstimate, SendError> {
        let e = self.inner.estimate().await?;
        Ok(TxEstimate {
            fee: e.fee,
            size_bytes: e.size_bytes,
            input_count: e.input_count,
            change: e.change,
            total_sent: e.total_sent,
        })
    }

    /// Selects coins, builds, signs (through the vault, with a `Spend` grant
    /// that caps `external_sent`) and reserves the inputs. Never broadcasts.
    pub async fn prepare(&self, grant_id: String) -> Result<Arc<PreparedTx>, SendError> {
        Ok(Arc::new(PreparedTx {
            inner: self.inner.prepare(grant_id).await?,
        }))
    }

    /// Announces `prepared` and waits for the network's acceptance (up to
    /// about a minute on SPV). See `send.broadcast_unknown`.
    pub async fn broadcast(
        &self,
        prepared: Arc<PreparedTx>,
    ) -> Result<BroadcastOutcome, SendError> {
        let outcome = self.inner.broadcast(Arc::clone(&prepared.inner)).await?;
        Ok(BroadcastOutcome {
            txid: outcome.txid,
            peers_announced: None,
        })
    }

    /// Discards `prepared` and releases its reserved inputs. Idempotent;
    /// `send.prepared_tx_spent` once it was handed to the network.
    pub async fn abandon(&self, prepared: Arc<PreparedTx>) -> Result<(), SendError> {
        Ok(self.inner.abandon(Arc::clone(&prepared.inner)).await?)
    }
}
