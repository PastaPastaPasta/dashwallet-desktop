//! M3 masternode keychain (IOS-083): a wallet's DIP3 provider keys and the
//! reveal of one private key. Contract: docs/contracts/m3-engine.md §2.5.
//! Masternode list, ProTx, tracked masternodes and evonode tools stay in
//! Dash Core (repo CLAUDE.md "Product scope"; parked on the branches
//! m3/r2-governance and m3/r3-protx).
//!
//! Private keys leave the engine only through `Vault.reveal_masternode_key`
//! (a `RevealSecret` grant), as the mnemonic does through
//! `Vault.reveal_mnemonic`.

use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
use crate::{NetworkSession, Vault};

/// A provider key family (DIP3 paths under DIP9 feature 3').
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MasternodeKeyRole {
    /// secp256k1, `m/9'/coin'/3'/2'/i`.
    Owner,
    /// secp256k1, `m/9'/coin'/3'/1'/i`.
    Voting,
    /// BLS operator key, `m/9'/coin'/3'/3'/i`.
    Operator,
    /// ed25519 Tenderdash node key (evonodes), `m/9'/coin'/3'/4'/i'`.
    PlatformNode,
}

/// One derived provider key (public data only).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeKeyInfo {
    pub role: MasternodeKeyRole,
    pub index: u32,
    /// e.g. `m/9'/5'/3'/1'/0` (DIP3/DIP9 provider key paths).
    pub derivation_path: String,
    /// P2PKH address for secp256k1 roles.
    pub address: Option<String>,
    pub public_key_hex: String,
    /// Legacy BLS serialization of an operator key.
    pub legacy_public_key_hex: Option<String>,
    /// Tenderdash node id of a platform node key (40 hex).
    pub platform_node_id: Option<String>,
}

/// A revealed provider private key. ASCII bytes, held by the host in a
/// zeroing buffer.
#[derive(uniffi::Record)]
pub struct RevealedMasternodeKey {
    pub private_key_hex: Vec<u8>,
    /// WIF for secp256k1 roles.
    pub wif: Option<Vec<u8>>,
    /// The Tenderdash `priv_validator_key`/node key form for a platform
    /// node key (base64).
    pub tenderdash_key: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MasternodeError {
    /// Code `masternode.watch_only`.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `masternode.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `masternode.grant_invalid`.
    #[error("grant invalid")]
    GrantInvalid,
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

crate::api::common::domain_error_common!(@not_implemented MasternodeError);
crate::api::common::export_error_code!(MasternodeError);

impl MasternodeError {
    /// Stable code (docs/contracts/m3-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::WatchOnly => "masternode.watch_only",
            Self::VaultLocked => "masternode.vault_locked",
            Self::GrantInvalid => "masternode.grant_invalid",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<MasternodeKeyRole> for dw_engine::MasternodeKeyRole {
    fn from(r: MasternodeKeyRole) -> Self {
        match r {
            MasternodeKeyRole::Owner => Self::Owner,
            MasternodeKeyRole::Voting => Self::Voting,
            MasternodeKeyRole::Operator => Self::Operator,
            MasternodeKeyRole::PlatformNode => Self::PlatformNode,
        }
    }
}

impl From<dw_engine::MasternodeKeyRole> for MasternodeKeyRole {
    fn from(r: dw_engine::MasternodeKeyRole) -> Self {
        use dw_engine::MasternodeKeyRole as R;
        match r {
            R::Owner => Self::Owner,
            R::Voting => Self::Voting,
            R::Operator => Self::Operator,
            R::PlatformNode => Self::PlatformNode,
        }
    }
}

impl From<dw_engine::EngineError> for MasternodeError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        use dw_engine::MasternodeFailure as F;
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::Masternode(F::WatchOnly) | E::Vault(V::NoSecret) => Self::WatchOnly,
            E::Masternode(F::VaultLocked) | E::Vault(V::NoVault | V::Locked | V::MixingOnly) => {
                Self::VaultLocked
            }
            E::Masternode(F::GrantInvalid)
            | E::Vault(V::GrantInvalid | V::GrantPurposeMismatch) => Self::GrantInvalid,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// Derived provider keys of `role` for indexes `start..start+count`
    /// (count ≤ 100). Public data, no grant.
    pub async fn masternode_keys(
        &self,
        wallet_id: String,
        role: MasternodeKeyRole,
        start: u32,
        count: u32,
    ) -> Result<Vec<MasternodeKeyInfo>, MasternodeError> {
        let _ = (role, start);
        parse_wallet_id(&wallet_id)?;
        if count > 100 {
            return Err(MasternodeError::InvalidArgument {
                detail: "count must be at most 100".to_string(),
            });
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.masternode_keys")
    }
}

#[uniffi::export]
impl Vault {
    /// Reveals one provider private key of a wallet (IOS-083).
    /// `RevealSecret` grant for that wallet.
    pub async fn reveal_masternode_key(
        &self,
        wallet_id: String,
        role: MasternodeKeyRole,
        index: u32,
        grant_id: String,
    ) -> Result<RevealedMasternodeKey, MasternodeError> {
        let _ = (role, index, grant_id);
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.session)?;
        not_implemented("Vault.reveal_masternode_key")
    }
}
