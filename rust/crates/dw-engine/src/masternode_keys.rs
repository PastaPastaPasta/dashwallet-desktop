//! The masternode keychain (IOS-083, docs/contracts/m3-engine.md §2.5):
//! a wallet's DIP3 provider keys (owner, voting, operator BLS, platform node
//! ed25519) and the reveal of one private key under a `RevealSecret` grant.
//! Masternode list and ProTx management stay in Dash Core (repo CLAUDE.md
//! "Product scope"); this module holds the keychain only.

use crate::EngineError;

/// A provider key family of the keychain (DIP3 paths under DIP9 feature
/// 3'). Payout keys are ordinary BIP44 addresses and are not listed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasternodeKeyRole {
    /// secp256k1, `m/9'/coin'/3'/2'/i`.
    Owner,
    /// secp256k1, `m/9'/coin'/3'/1'/i`.
    Voting,
    /// BLS12-381, `m/9'/coin'/3'/3'/i`.
    Operator,
    /// ed25519 Tenderdash node key (evonodes), `m/9'/coin'/3'/4'/i'`.
    PlatformNode,
}

/// Why a keychain call failed (`masternode.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasternodeFailure {
    /// The wallet holds no seed (watch-only), so the key cannot be derived
    /// or revealed.
    WatchOnly,
    VaultLocked,
    GrantInvalid,
}

impl std::fmt::Display for MasternodeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::VaultLocked => f.write_str("vault locked"),
            Self::GrantInvalid => f.write_str("grant invalid"),
        }
    }
}

impl From<MasternodeFailure> for EngineError {
    fn from(f: MasternodeFailure) -> Self {
        EngineError::Masternode(f)
    }
}
