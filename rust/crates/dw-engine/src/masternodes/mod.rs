//! Masternodes in the engine (M3, owner R3; docs/contracts/m3-engine.md
//! §2.3–2.5): the list model over the SPV masternode list and
//! platform-wallet's masternode records, owned detection, ProTx flows over
//! `dw-protx`, the masternode keychain, tracked masternodes, and the
//! `Masternodes` event.
//!
//! - [`list`]: list state, rows, details, owned detection.
//! - [`protx`]: registration with the operator-secret gate, Update Service,
//!   Update Registrar, Revoke.
//! - [`keys`]: keychain, reveal, list search, tracked masternodes and
//!   attached keys.
//!
//! Not here yet (the FFI answers `NotImplemented`): v24 shared-masternode
//! sessions and their maintenance transactions (the payload codecs are in
//! `dw_protx::shared`), registration with an external collateral, and the
//! evonode Platform calls (M4).

pub mod keys;
pub mod list;
pub mod protx;

pub use keys::{
    MasternodeKeyInfo, MasternodeKeyUsage, RevealedMasternodeKey, TrackedCapabilities,
    TrackedMasternodeInfo,
};
pub use list::{
    MasternodeDetail, MasternodeListState, MasternodeListStatus, MasternodeQuery, MasternodeRow,
    MasternodeShare, MasternodeType, MasternodeTypeFilter, OperatorReward, OwnedRole,
};
pub use protx::{
    CollateralCandidate, CollateralChoice, FeeSourceCandidate, FeeSourceChoice, OperatorKeyChoice,
    PlatformFields, PreparedProviderTx, PreparedRegistration, ProviderTxKind, ProviderTxSummary,
    RegistrationRequest, RegistrationSummary, RevocationReason, RevokeRequest,
    UpdateRegistrarRequest, UpdateServiceRequest,
};

use crate::EngineError;

/// A key's job on a masternode (platform-wallet `MasternodeKeyRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MasternodeKeyRole {
    /// secp256k1; signs ProUpRegTx.
    Owner,
    /// secp256k1; governance and contested-name votes.
    Voting,
    /// BLS12-381; signs ProUpServTx and ProUpRevTx.
    Operator,
    /// ed25519 Tenderdash node key (evonodes).
    PlatformNode,
    /// secp256k1 key of the owner payout address.
    OwnerPayout,
    /// secp256k1 key of the operator payout address.
    OperatorPayout,
}

/// Why an outpoint cannot be a collateral (Register wizard, existing UTXO).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollateralRefusal {
    /// Not exactly 1000 DASH (4000 for an EvoNode).
    WrongAmount,
    /// No confirmation yet.
    Unconfirmed,
    /// Not a P2PKH output.
    NotP2pkh,
    /// Locked by the user or reserved by another operation.
    Locked,
    /// Already the collateral of a registered masternode.
    AlreadyCollateral,
    /// Not an unspent output of the wallet (or unknown for external).
    NotFound,
}

/// Why a masternode call failed (`masternode.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasternodeFailure {
    /// The SPV masternode list has not synced yet.
    ListUnavailable,
    /// No masternode with this proTxHash.
    NotFound(String),
    /// The operation needs a key no wallet holds and no tracked masternode
    /// has attached.
    KeyNotInWallet(MasternodeKeyRole),
    /// A service address that is not `IP:port`, uses a non-routable IP or a
    /// port the network forbids.
    InvalidService(String),
    /// A key, public key or node id that does not parse or does not match.
    InvalidKey {
        role: MasternodeKeyRole,
        detail: String,
    },
    /// A payout address that is not P2PKH/P2SH of this network.
    InvalidPayout(String),
    /// Owner, voting, payout and collateral addresses must differ as dash-qt
    /// requires; the detail names the pair.
    DuplicateAddress(String),
    CollateralUnavailable(CollateralRefusal),
    /// The fee source cannot pay; `needed` includes a fund-new collateral.
    InsufficientFunds {
        needed: u64,
        available: u64,
    },
    /// The typed operator secret does not match the registered public key.
    OperatorSecretMismatch,
    /// The generated operator secret has not been confirmed as saved
    /// (the "type the last 4 characters" gate, QT-124).
    OperatorSecretUnconfirmed,
    /// The pasted collateral signature does not verify.
    CollateralSignatureInvalid,
    /// The list entry needs a payload version this wallet cannot build
    /// (for example an extended network-info entry and a version-2
    /// ProUpServTx).
    UnsupportedEntry(String),
    WatchOnly,
    VaultLocked,
    GrantInvalid,
    NoPeers,
    /// A peer rejected the provider transaction; Core's reason
    /// (`bad-protx-dup-key`, …).
    BroadcastRejected(String),
    /// A shared-session message that does not parse or fails a check.
    SharedEnvelopeInvalid(String),
    /// A shared-session message above 2 MiB.
    SharedEnvelopeTooLarge {
        size_bytes: u64,
    },
    /// A shared-session message from another network.
    SharedNetworkMismatch,
    SharedSessionNotFound(String),
    /// The session asks to sign foreign or short-changed inputs.
    SharedInputsRefused(String),
    /// A coin reserved for the session was spent elsewhere.
    SharedCoinSpent(dashcore::OutPoint),
    /// The masternode is already tracked.
    AlreadyTracked(String),
    /// The call needs Platform (evonode credits, epoch blocks) and the
    /// trusted quorum context is not available.
    PlatformUnavailable,
}

impl std::fmt::Display for MasternodeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListUnavailable => f.write_str("masternode list unavailable"),
            Self::NotFound(h) => write!(f, "masternode {h} not found"),
            Self::KeyNotInWallet(role) => write!(f, "{role:?} key not in wallet"),
            Self::InvalidService(d) => write!(f, "invalid service: {d}"),
            Self::InvalidKey { role, detail } => write!(f, "invalid {role:?} key: {detail}"),
            Self::InvalidPayout(d) => write!(f, "invalid payout: {d}"),
            Self::DuplicateAddress(d) => write!(f, "duplicate address: {d}"),
            Self::CollateralUnavailable(r) => write!(f, "collateral unavailable: {r:?}"),
            Self::InsufficientFunds { needed, available } => {
                write!(f, "needs {needed} duffs, {available} available")
            }
            Self::OperatorSecretMismatch => f.write_str("operator secret does not match"),
            Self::OperatorSecretUnconfirmed => f.write_str("operator secret not confirmed"),
            Self::CollateralSignatureInvalid => f.write_str("collateral signature invalid"),
            Self::UnsupportedEntry(d) => write!(f, "unsupported entry: {d}"),
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::VaultLocked => f.write_str("vault locked"),
            Self::GrantInvalid => f.write_str("grant invalid"),
            Self::NoPeers => f.write_str("no connected peers"),
            Self::BroadcastRejected(r) => write!(f, "broadcast rejected: {r}"),
            Self::SharedEnvelopeInvalid(d) => write!(f, "shared message invalid: {d}"),
            Self::SharedEnvelopeTooLarge { size_bytes } => {
                write!(f, "shared message of {size_bytes} bytes is too large")
            }
            Self::SharedNetworkMismatch => f.write_str("shared message is for another network"),
            Self::SharedSessionNotFound(id) => write!(f, "shared session {id} not found"),
            Self::SharedInputsRefused(d) => write!(f, "shared inputs refused: {d}"),
            Self::SharedCoinSpent(o) => write!(f, "reserved coin {o} was spent"),
            Self::AlreadyTracked(h) => write!(f, "masternode {h} already tracked"),
            Self::PlatformUnavailable => f.write_str("platform unavailable"),
        }
    }
}

impl From<MasternodeFailure> for EngineError {
    fn from(f: MasternodeFailure) -> Self {
        EngineError::Masternode(f)
    }
}
