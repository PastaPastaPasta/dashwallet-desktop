//! M2 PSBT (QT-076…079): create unsigned from a draft, load, analyze, sign,
//! finalize and broadcast. Owner: R2 (compat, `dw-psbt` over key-wallet's
//! `psbt`). Contract: docs/contracts/m2-engine.md §2.8.

use std::sync::Arc;

use crate::api::common::{domain_error_common, ensure_open, not_implemented, parse_wallet_id};
use crate::{NetworkSession, SendError, TxDraft};

/// Largest PSBT accepted from a file (dash-qt: under 100 MiB).
pub const MAX_PSBT_BYTES: u64 = 100 * 1024 * 1024;

/// A partially signed transaction. Immutable: signing returns a new one.
#[derive(Debug, uniffi::Object)]
pub struct Psbt {
    // Filled by R2 with the `dw-psbt` value.
    _private: (),
}

#[uniffi::export]
impl Psbt {
    /// Base64 (clipboard form).
    pub fn to_base64(&self) -> Result<String, PsbtError> {
        not_implemented("Psbt.to_base64")
    }

    /// BIP174 binary (the `.psbt` file dash-qt saves).
    pub fn to_bytes(&self) -> Result<Vec<u8>, PsbtError> {
        not_implemented("Psbt.to_bytes")
    }

    /// Txid of the unsigned transaction.
    pub fn unsigned_txid(&self) -> Result<String, PsbtError> {
        not_implemented("Psbt.unsigned_txid")
    }
}

/// Parses a PSBT from a file (binary or base64) or the clipboard (base64),
/// at most `MAX_PSBT_BYTES`.
#[uniffi::export]
pub fn parse_psbt(data: Vec<u8>) -> Result<Arc<Psbt>, PsbtError> {
    if data.len() as u64 > MAX_PSBT_BYTES {
        return Err(PsbtError::TooLarge {
            size_bytes: data.len() as u64,
        });
    }
    not_implemented("parse_psbt")
}

/// One output line of the PSBT Operations dialog (" * Sends %1 to %2").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PsbtOutput {
    pub address: Option<String>,
    pub amount: u64,
    /// " (own address)".
    pub is_mine: bool,
}

/// dash-qt's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PsbtStatus {
    /// "Transaction is missing some information about inputs."
    MissingInputInfo,
    /// "Transaction still needs signature(s)."
    NeedsSignatures,
    /// "Transaction is fully signed and ready for broadcast."
    Complete,
}

/// The suffix of "needs signature(s)" for the selected wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PsbtSignability {
    /// "(But no wallet is loaded.)"
    NoWallet,
    /// "(But this wallet cannot sign transactions.)" — watch-only.
    WatchOnly,
    /// "(But this wallet does not have the right keys.)"
    NoMatchingKeys,
    CanSign,
}

/// PSBT Operations dialog content (QT-079).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PsbtAnalysis {
    pub outputs: Vec<PsbtOutput>,
    /// `None` while input values are missing.
    pub fee: Option<u64>,
    /// Sum of outputs not paying the wallet plus fee; `None` like `fee`.
    pub total: Option<u64>,
    /// "Transaction has %1 unsigned inputs."
    pub unsigned_inputs: u32,
    pub status: PsbtStatus,
    pub signability: PsbtSignability,
    /// Value paid to scripts the wallet does not own; a signing `Spend`
    /// grant must cover it (same cap as `TxDraft.prepare`).
    pub external_sent: Option<u64>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PsbtError {
    /// Code `psbt.invalid`: not a PSBT (binary or base64).
    #[error("invalid psbt: {detail}")]
    Invalid { detail: String },
    /// Code `psbt.too_large`: over `MAX_PSBT_BYTES`.
    #[error("psbt too large: {size_bytes} bytes")]
    TooLarge { size_bytes: u64 },
    /// Code `psbt.network_mismatch`: outputs or keys of another network.
    #[error("psbt for another network")]
    NetworkMismatch,
    /// Code `psbt.not_complete`: broadcast before every input is signed.
    #[error("psbt not complete")]
    NotComplete,
    /// Code `psbt.fee_rate_too_high`: above 0.1 DASH/kB (dash-qt's broadcast cap).
    #[error("fee rate {duffs_per_kb} duff/kB too high")]
    FeeRateTooHigh { duffs_per_kb: u64 },
    /// Code `psbt.watch_only`: the wallet has no keys.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `psbt.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `psbt.grant_invalid`: missing, expired, other wallet or purpose.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `psbt.grant_exceeded`: `external_sent` above the grant's cap.
    #[error("grant cap {max_duffs} exceeded")]
    GrantExceeded { max_duffs: u64 },
    /// Code `psbt.no_peers`: not sent; nothing reached the network.
    #[error("no peers")]
    NoPeers,
    /// Code `psbt.broadcast_rejected`: a peer rejected it.
    #[error("rejected: {reason}")]
    BroadcastRejected { reason: String },
    /// Code `psbt.broadcast_unknown`: announced without a verdict; it may be
    /// on the network.
    #[error("outcome unknown: {reason}")]
    BroadcastUnknown { reason: String },
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

domain_error_common!(PsbtError);
crate::api::common::export_error_code!(PsbtError);

impl PsbtError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "psbt.invalid",
            Self::TooLarge { .. } => "psbt.too_large",
            Self::NetworkMismatch => "psbt.network_mismatch",
            Self::NotComplete => "psbt.not_complete",
            Self::FeeRateTooHigh { .. } => "psbt.fee_rate_too_high",
            Self::WatchOnly => "psbt.watch_only",
            Self::VaultLocked => "psbt.vault_locked",
            Self::GrantInvalid => "psbt.grant_invalid",
            Self::GrantExceeded { .. } => "psbt.grant_exceeded",
            Self::NoPeers => "psbt.no_peers",
            Self::BroadcastRejected { .. } => "psbt.broadcast_rejected",
            Self::BroadcastUnknown { .. } => "psbt.broadcast_unknown",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

#[uniffi::export]
impl TxDraft {
    /// dash-qt "Create Unsigned" (QT-076/077): plans the draft as `estimate`
    /// does and returns it as a PSBT with the wallet's input data (UTXOs,
    /// derivation paths) filled in. Signs and reserves nothing, needs no
    /// grant, and works for watch-only wallets.
    pub async fn create_unsigned(&self) -> Result<Arc<Psbt>, SendError> {
        not_implemented("TxDraft.create_unsigned")
    }
}

#[uniffi::export]
impl NetworkSession {
    /// The Operations dialog content for `psbt` seen from `wallet_id`
    /// (`None` = "no wallet is loaded").
    pub async fn analyze_psbt(
        &self,
        wallet_id: Option<String>,
        psbt: Arc<Psbt>,
    ) -> Result<PsbtAnalysis, PsbtError> {
        let _ = psbt;
        if let Some(id) = &wallet_id {
            parse_wallet_id(id)?;
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.analyze_psbt")
    }

    /// "Sign Tx": signs every input the wallet owns through `VaultSigner`
    /// and returns the new PSBT. Needs a `Spend{max_duffs}` grant for the
    /// wallet covering `external_sent`; redeemed after the checks.
    pub async fn sign_psbt(
        &self,
        wallet_id: String,
        psbt: Arc<Psbt>,
        grant_id: String,
    ) -> Result<Arc<Psbt>, PsbtError> {
        let _ = (psbt, grant_id);
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.sign_psbt")
    }

    /// "Broadcast Tx": finalizes a complete PSBT, checks the fee rate cap and
    /// broadcasts with `TxDraft.broadcast`'s verdict rules (accepted, never
    /// sent, or unknown). Returns the txid.
    pub async fn broadcast_psbt(&self, psbt: Arc<Psbt>) -> Result<String, PsbtError> {
        let _ = psbt;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.broadcast_psbt")
    }
}
