//! Masternode keychain paths (DIP3 provider keys under DIP9 feature 3',
//! key-wallet `AccountType::Provider*Keys`, `account_type.rs:493-540`):
//!
//! | role | path | child |
//! |---|---|---|
//! | voting | `m/9'/coin'/3'/1'/i` | non-hardened, secp256k1 |
//! | owner | `m/9'/coin'/3'/2'/i` | non-hardened, secp256k1 |
//! | operator | `m/9'/coin'/3'/3'/i` | non-hardened, BLS (legacy HD) |
//! | platform node | `m/9'/coin'/3'/4'/i'` | hardened, ed25519 (SLIP-10) |
//!
//! `coin` is 5 on mainnet and 1 elsewhere. Payout keys are ordinary BIP44
//! addresses.

use dashcore::Network;

/// Provider key families with a DIP3 path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderRole {
    Voting,
    Owner,
    Operator,
    PlatformNode,
}

impl ProviderRole {
    /// The DIP3 sub-account (`1'` voting … `4'` platform).
    pub fn sub_account(self) -> u32 {
        match self {
            Self::Voting => 1,
            Self::Owner => 2,
            Self::Operator => 3,
            Self::PlatformNode => 4,
        }
    }

    /// Whether the child index is hardened (ed25519 has no public
    /// derivation).
    pub fn hardened_child(self) -> bool {
        self == Self::PlatformNode
    }
}

/// SLIP-44 coin type of the provider paths.
pub fn coin_type(network: Network) -> u32 {
    if network == Network::Mainnet { 5 } else { 1 }
}

/// The derivation path of key `index` of `role`, as dash-qt and the iOS
/// keychain print it.
pub fn path(role: ProviderRole, network: Network, index: u32) -> String {
    format!(
        "m/9'/{}'/3'/{}'/{}{}",
        coin_type(network),
        role.sub_account(),
        index,
        if role.hardened_child() { "'" } else { "" }
    )
}

/// The Tenderdash form of an ed25519 node key (`priv_validator_key.json` /
/// `node_key.json` `priv_key.value`): base64 of the 32-byte seed followed by
/// the 32-byte public key.
pub fn tenderdash_private_key(seed: &[u8; 32]) -> zeroize::Zeroizing<String> {
    use dashcore::ed25519_dalek::SigningKey;
    let key = SigningKey::from_bytes(seed);
    let mut both = zeroize::Zeroizing::new([0u8; 64]);
    both[..32].copy_from_slice(seed);
    both[32..].copy_from_slice(key.verifying_key().as_bytes());
    zeroize::Zeroizing::new(base64_encode(&both[..]))
}

/// The Tenderdash node id of an ed25519 public key (first 20 bytes of its
/// SHA-256), lowercase hex.
pub fn platform_node_id_hex(public_key: &[u8; 32]) -> String {
    hex::encode(dashcore::PlatformNodeId::from_ed25519_public_key(public_key).to_byte_array())
}

/// Standard base64 with padding (no extra dependency for one use).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_IOS_083_paths_follow_dip3() {
        assert_eq!(
            path(ProviderRole::Voting, Network::Mainnet, 0),
            "m/9'/5'/3'/1'/0"
        );
        assert_eq!(
            path(ProviderRole::Owner, Network::Testnet, 7),
            "m/9'/1'/3'/2'/7"
        );
        assert_eq!(
            path(ProviderRole::Operator, Network::Regtest, 2),
            "m/9'/1'/3'/3'/2"
        );
        assert_eq!(
            path(ProviderRole::PlatformNode, Network::Mainnet, 3),
            "m/9'/5'/3'/4'/3'"
        );
    }

    #[test]
    fn test_IOS_083_tenderdash_key_is_seed_then_public_key() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        let text = tenderdash_private_key(&[1u8; 32]);
        assert_eq!(text.len(), 88);
        assert!(text.ends_with('='));
    }
}
