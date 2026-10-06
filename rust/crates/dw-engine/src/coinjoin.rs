//! CoinJoin in the engine (M3, owner R1; docs/contracts/m3-engine.md §2.1):
//! per-wallet mixing state over `dw-coinjoin`, the CoinJoin options of the
//! session, the `CoinJoin` event, the IOS-057 recovery scan and "move mixed
//! coins". Today this module holds the failure type of the contract; the
//! calls return `NotImplemented` from the FFI until R1 lands them.

use crate::EngineError;

/// Why a CoinJoin call failed (`coinjoin.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoinJoinFailure {
    /// CoinJoin features are turned off (`enablecoinjoin` false).
    Disabled,
    /// The wallet has no private keys.
    WatchOnly,
    /// The balance is below the mixing minimum (0.00140001 DASH);
    /// `min_duffs` is that minimum ("CoinJoin requires at least %2 to use").
    InsufficientFunds {
        min_duffs: u64,
    },
    /// The vault is locked: unlock it for mixing only (or fully) first.
    VaultLocked,
    /// The grant is unknown, expired or of another purpose.
    GrantInvalid,
    /// "Move mixed coins": nothing above the 1000-duff threshold to move.
    NothingToMove,
    /// The recovery scan needs a running SPV client.
    SpvNotRunning,
    NoPeers,
    /// A peer rejected a sweep transaction; the reason is Core's.
    BroadcastRejected(String),
}

impl std::fmt::Display for CoinJoinFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("coinjoin is disabled"),
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::InsufficientFunds { min_duffs } => {
                write!(f, "balance below the mixing minimum of {min_duffs} duffs")
            }
            Self::VaultLocked => f.write_str("vault locked"),
            Self::GrantInvalid => f.write_str("grant invalid"),
            Self::NothingToMove => f.write_str("no mixed coins to move"),
            Self::SpvNotRunning => f.write_str("spv is not running"),
            Self::NoPeers => f.write_str("no connected peers"),
            Self::BroadcastRejected(r) => write!(f, "broadcast rejected: {r}"),
        }
    }
}

impl From<CoinJoinFailure> for EngineError {
    fn from(f: CoinJoinFailure) -> Self {
        EngineError::CoinJoin(f)
    }
}
