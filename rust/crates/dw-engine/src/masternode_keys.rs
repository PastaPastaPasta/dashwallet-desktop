//! The masternode keychain (IOS-083, docs/contracts/m3-engine.md §2.5):
//! a wallet's DIP3 provider keys (owner, voting, operator BLS, platform node
//! ed25519) and the reveal of one private key under a `RevealSecret` grant.
//! Masternode list and ProTx management stay in Dash Core (repo CLAUDE.md
//! "Product scope"; parked on the branches m3/r2-governance, m3/r3-protx);
//! this module holds the keychain only.
//!
//! Keys derive on their DIP3 paths through platform-wallet's
//! `derive_provider_key_at_index`. Public keys need no seed except ed25519
//! platform node keys (SLIP-10 is hardened-only), which are read from the
//! wallet's pool pre-derived at import. A private key leaves the engine only
//! through [`NetworkSession::reveal_masternode_key`]; the seed it needs is
//! decrypted by the vault for that one derivation.
//!
//! | role | path | child |
//! |---|---|---|
//! | voting | `m/9'/coin'/3'/1'/i` | non-hardened, secp256k1 |
//! | owner | `m/9'/coin'/3'/2'/i` | non-hardened, secp256k1 |
//! | operator | `m/9'/coin'/3'/3'/i` | non-hardened, BLS (legacy HD) |
//! | platform node | `m/9'/coin'/3'/4'/i'` | hardened, ed25519 (SLIP-10) |
//!
//! `coin` is 5 on mainnet and 1 elsewhere. Payout keys are ordinary BIP44
//! addresses and are not part of the keychain.

use std::sync::Arc;

use dw_vault::VaultError;
use key_wallet::managed_account::address_pool::PublicKeyType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::provider_key_at_index::ProviderKeyKind;
use zeroize::Zeroizing;

use crate::{EngineError, NetworkSession, WalletId};

/// Most keys one `masternode_keys` call returns.
pub const MAX_KEYS_PER_CALL: u32 = 100;

/// A provider key family of the keychain (DIP3 paths under DIP9 feature
/// 3').
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

impl MasternodeKeyRole {
    /// The DIP3 sub-account (`1'` voting … `4'` platform node).
    fn sub_account(self) -> u32 {
        match self {
            Self::Voting => 1,
            Self::Owner => 2,
            Self::Operator => 3,
            Self::PlatformNode => 4,
        }
    }

    fn kind(self) -> ProviderKeyKind {
        match self {
            Self::Owner => ProviderKeyKind::Owner,
            Self::Voting => ProviderKeyKind::Voting,
            Self::Operator => ProviderKeyKind::Operator,
            Self::PlatformNode => ProviderKeyKind::PlatformNode,
        }
    }

    /// The derivation path of key `index`, as dash-qt and the iOS keychain
    /// print it.
    pub fn path(self, network: dashcore::Network, index: u32) -> String {
        let coin = if network == dashcore::Network::Mainnet {
            5
        } else {
            1
        };
        let hardened = if self == Self::PlatformNode { "'" } else { "" };
        format!("m/9'/{coin}'/3'/{}'/{index}{hardened}", self.sub_account())
    }
}

/// One derived provider key (public data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeKeyInfo {
    pub role: MasternodeKeyRole,
    pub index: u32,
    pub derivation_path: String,
    /// P2PKH address of a secp256k1 key.
    pub address: Option<String>,
    pub public_key_hex: String,
    /// The operator key in Dash's legacy BLS serialization.
    pub legacy_public_key_hex: Option<String>,
    /// Tenderdash node id of a platform node key (40 hex).
    pub platform_node_id: Option<String>,
}

/// A revealed private key, ASCII in zeroing buffers.
pub struct RevealedMasternodeKey {
    pub private_key_hex: Zeroizing<Vec<u8>>,
    /// WIF of a secp256k1 key.
    pub wif: Option<Zeroizing<Vec<u8>>>,
    /// The Tenderdash form of a platform node key (base64 of seed ‖ public
    /// key, `priv_validator_key.json` / `node_key.json`).
    pub tenderdash_key: Option<Zeroizing<Vec<u8>>>,
}

/// Why a keychain call failed (`masternode.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasternodeFailure {
    /// The wallet holds no seed or provider accounts (watch-only), so the
    /// key cannot be derived or revealed.
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

/// Vault refusals of a reveal in the keychain's terms.
fn vault_failure(e: VaultError) -> EngineError {
    match e {
        VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly => {
            MasternodeFailure::VaultLocked.into()
        }
        VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => {
            MasternodeFailure::GrantInvalid.into()
        }
        VaultError::NoSecret => MasternodeFailure::WatchOnly.into(),
        other => EngineError::Vault(other),
    }
}

/// A wallet without the provider account (an xpub watch-only wallet) is
/// `masternode.watch_only`.
fn derive_failure(e: PlatformWalletError) -> EngineError {
    match e {
        PlatformWalletError::AddressNotFound(_) => MasternodeFailure::WatchOnly.into(),
        other => other.into(),
    }
}

/// The Tenderdash node id of an ed25519 public key (first 20 bytes of its
/// SHA-256), lowercase hex.
fn platform_node_id_hex(public_key: &[u8; 32]) -> String {
    hex::encode(dashcore::PlatformNodeId::from_ed25519_public_key(public_key).to_byte_array())
}

/// The Tenderdash form of an ed25519 node key (`priv_validator_key.json` /
/// `node_key.json` `priv_key.value`): base64 of the 32-byte seed followed by
/// the 32-byte public key.
fn tenderdash_private_key(seed: &[u8; 32]) -> Zeroizing<Vec<u8>> {
    use dashcore::eddsa::EddsaSecretKey;
    let key = EddsaSecretKey::from_bytes(seed);
    let mut both = Zeroizing::new([0u8; 64]);
    both[..32].copy_from_slice(seed);
    both[32..].copy_from_slice(&key.public_key().to_bytes());
    base64_encode(&both[..])
}

/// Standard base64 with padding, into a zeroing buffer.
fn base64_encode(bytes: &[u8]) -> Zeroizing<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Zeroizing::new(Vec::with_capacity(bytes.len().div_ceil(3) * 4));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() {
                TABLE[((n >> shift) & 63) as usize]
            } else {
                b'='
            });
        }
    }
    out
}

impl NetworkSession {
    /// Derived provider keys of `role` for `start..start+count` (≤ 100,
    /// IOS-083). Platform node keys come from the wallet's pre-derived pool;
    /// indexes past it are left out.
    pub async fn masternode_keys(
        self: &Arc<Self>,
        wallet_id: WalletId,
        role: MasternodeKeyRole,
        start: u32,
        count: u32,
    ) -> Result<Vec<MasternodeKeyInfo>, EngineError> {
        if count > MAX_KEYS_PER_CALL {
            return Err(EngineError::InvalidArgument(format!(
                "count must be at most {MAX_KEYS_PER_CALL}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let network = this.network.core_network();
            let wallet = this.wallet(&wallet_id).await?;
            let end = start.saturating_add(count);
            if role == MasternodeKeyRole::PlatformNode {
                let state = wallet.state().await;
                let Some(account) = state.core_wallet.accounts.provider_platform_keys.as_ref()
                else {
                    return Err(MasternodeFailure::WatchOnly.into());
                };
                let mut keys = Vec::new();
                for pool in account.managed_account_type().address_pools() {
                    for entry in pool.addresses.values() {
                        if !(start..end).contains(&entry.index) {
                            continue;
                        }
                        if let Some(PublicKeyType::EdDSA(pk)) = &entry.public_key
                            && let Ok(pk32) = <[u8; 32]>::try_from(pk.as_slice())
                        {
                            keys.push(MasternodeKeyInfo {
                                role,
                                index: entry.index,
                                derivation_path: role.path(network, entry.index),
                                address: None,
                                public_key_hex: hex::encode(pk32),
                                legacy_public_key_hex: None,
                                platform_node_id: Some(platform_node_id_hex(&pk32)),
                            });
                        }
                    }
                }
                keys.sort_by_key(|k| k.index);
                return Ok(keys);
            }
            tokio::task::spawn_blocking(move || {
                (start..end)
                    .map(|index| {
                        let key = wallet
                            .derive_provider_key_at_index(role.kind(), index, None, false)
                            .map_err(derive_failure)?;
                        Ok(MasternodeKeyInfo {
                            role,
                            index,
                            derivation_path: role.path(network, index),
                            address: key.address.clone(),
                            public_key_hex: hex::encode(&key.public_key_bytes),
                            legacy_public_key_hex: key
                                .legacy_public_key_bytes
                                .as_ref()
                                .map(hex::encode),
                            platform_node_id: None,
                        })
                    })
                    .collect::<Result<Vec<_>, EngineError>>()
            })
            .await?
        })
        .await
    }

    /// One provider private key of a wallet (IOS-083) under a `RevealSecret`
    /// grant for that wallet. The vault decrypts the seed for this one
    /// derivation; the grant is consumed.
    pub async fn reveal_masternode_key(
        self: &Arc<Self>,
        wallet_id: WalletId,
        role: MasternodeKeyRole,
        index: u32,
        grant_id: String,
    ) -> Result<RevealedMasternodeKey, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let wallet = this.wallet(&wallet_id).await?;
            let vault = this.vault.clone();
            let derived = tokio::task::spawn_blocking(move || {
                vault.with_revealed_seed(&wallet_id.0, &grant_id, |seed| {
                    wallet
                        .derive_provider_key_at_index(role.kind(), index, Some(&seed[..]), true)
                        .map_err(derive_failure)
                })
            })
            .await?
            .map_err(vault_failure)??;
            let private = derived.private_key.as_ref().ok_or_else(|| {
                EngineError::Internal("the derivation returned no private key".into())
            })?;
            let tenderdash_key = (role == MasternodeKeyRole::PlatformNode && private.len() >= 32)
                .then(|| {
                    let mut seed = Zeroizing::new([0u8; 32]);
                    seed.copy_from_slice(&private[..32]);
                    tenderdash_private_key(&seed)
                });
            Ok(RevealedMasternodeKey {
                private_key_hex: Zeroizing::new(hex::encode(&private[..]).into_bytes()),
                wif: derived
                    .private_key_wif
                    .as_ref()
                    .map(|w| Zeroizing::new(w.as_bytes().to_vec())),
                tenderdash_key,
            })
        })
        .await
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::Network;

    #[test]
    fn test_IOS_083_paths_follow_dip3() {
        use MasternodeKeyRole::*;
        assert_eq!(Voting.path(Network::Mainnet, 0), "m/9'/5'/3'/1'/0");
        assert_eq!(Owner.path(Network::Testnet, 7), "m/9'/1'/3'/2'/7");
        assert_eq!(Operator.path(Network::Regtest, 2), "m/9'/1'/3'/3'/2");
        assert_eq!(PlatformNode.path(Network::Mainnet, 3), "m/9'/5'/3'/4'/3'");
    }

    #[test]
    fn test_IOS_083_tenderdash_key_is_seed_then_public_key() {
        assert_eq!(&base64_encode(b"Man")[..], b"TWFu");
        assert_eq!(&base64_encode(b"Ma")[..], b"TWE=");
        assert_eq!(&base64_encode(b"M")[..], b"TQ==");
        let text = tenderdash_private_key(&[1u8; 32]);
        assert_eq!(text.len(), 88);
        assert_eq!(text.last(), Some(&b'='));
    }
}
