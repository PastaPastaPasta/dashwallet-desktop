//! Derivation-path shapes the signer scopes are made of (BIP44, DIP9, DIP-13
//! identities, DIP-14 256-bit children, DIP-15 DashPay).
//!
//! Every predicate checks the whole path: its length, each hardened or
//! normal step, and the coin type of the vault's network. A path that is a
//! prefix or an extension of an allowed shape is refused. 256-bit (DIP-14)
//! children are accepted only where DIP-15 puts them, and only non-hardened.
//!
//! A 31-bit step is valid only with `index < 2^31`. `ChildNumber`'s variants
//! are public, so `Normal { index: 2^31 }` or `Hardened { index: 2^31 | i }`
//! can be built directly; key-wallet would derive them (the second as an
//! alias of `i'`), so the predicates refuse them (review DW-E0-03 M2).

use key_wallet::Network;
use key_wallet::bip32::{ChildNumber, DerivationPath};
use key_wallet::dip9::{
    DASHPAY_CONTACT_INFO_ENC_TO_USER_ID_CHILD, DASHPAY_CONTACT_INFO_PRIVATE_DATA_CHILD,
    FEATURE_PURPOSE, FEATURE_PURPOSE_COINJOIN, FEATURE_PURPOSE_DASHPAY,
    FEATURE_PURPOSE_DASHPAY_AUTO_ACCEPT, FEATURE_PURPOSE_IDENTITIES,
    FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_AUTHENTICATION,
    FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_INVITATIONS,
    FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_REGISTRATION,
    FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_TOPUP,
};

/// BIP44 purpose.
const BIP44: u32 = 44;
/// DIP-13 key type of ECDSA identity keys (`KeyDerivationType::ECDSA`).
const ECDSA_KEY_TYPE: u32 = 0;

/// SLIP-44 coin type: 5 on mainnet, 1 everywhere else.
pub(crate) fn coin_type(network: Network) -> u32 {
    if network == Network::Mainnet { 5 } else { 1 }
}

/// First index past the 31-bit child range (BIP32 `2^31`).
const INDEX_LIMIT: u32 = 1 << 31;

fn is_hardened(c: &ChildNumber, index: u32) -> bool {
    *c == ChildNumber::Hardened { index }
}

/// The index of a valid 31-bit hardened step.
fn hardened_index(c: &ChildNumber) -> Option<u32> {
    match c {
        ChildNumber::Hardened { index } if *index < INDEX_LIMIT => Some(*index),
        _ => None,
    }
}

/// Whether `c` is a valid 31-bit non-hardened step.
fn is_normal(c: &ChildNumber) -> bool {
    matches!(c, ChildNumber::Normal { index } if *index < INDEX_LIMIT)
}

/// `m/9'/coin'/feature'`, the first three steps of every DIP9 path.
fn has_dip9_feature(p: &[ChildNumber], network: Network, feature: u32) -> bool {
    p.len() >= 3
        && is_hardened(&p[0], FEATURE_PURPOSE)
        && is_hardened(&p[1], coin_type(network))
        && is_hardened(&p[2], feature)
}

/// Whether `path` lies inside the CoinJoin account branch of `network`.
pub fn is_coinjoin_path(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() >= 4 && has_dip9_feature(p, network, FEATURE_PURPOSE_COINJOIN)
}

/// Whether `path` lies inside a BIP44 account of `network`
/// (`m/44'/coin'/account'/…`, the account a 31-bit hardened step).
pub fn is_bip44_path(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() >= 4
        && is_hardened(&p[0], BIP44)
        && is_hardened(&p[1], coin_type(network))
        && hardened_index(&p[2]).is_some()
}

/// The DIP-13 ECDSA identity authentication key
/// `m/9'/coin'/5'/0'/0'/identity'/key'`, as `(identity, key)`. The key index
/// is the identity key id (platform-wallet `identity_auth_derivation_path`).
pub fn identity_auth_key(path: &DerivationPath, network: Network) -> Option<(u32, u32)> {
    let p: &[ChildNumber] = path.as_ref();
    let shaped = p.len() == 7
        && has_dip9_feature(p, network, FEATURE_PURPOSE_IDENTITIES)
        && is_hardened(&p[3], FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_AUTHENTICATION)
        && is_hardened(&p[4], ECDSA_KEY_TYPE);
    if !shaped {
        return None;
    }
    Some((hardened_index(&p[5])?, hardened_index(&p[6])?))
}

/// A DIP-15 contactInfo key: an identity authentication key path followed
/// by exactly `65536'` (`encToUserId`) or `65537'` (`privateData`) and a
/// hardened derivation index.
pub fn is_contact_info_key(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 9
        && identity_auth_key(&p[..7].into(), network).is_some()
        && (is_hardened(&p[7], DASHPAY_CONTACT_INFO_ENC_TO_USER_ID_CHILD)
            || is_hardened(&p[7], DASHPAY_CONTACT_INFO_PRIVATE_DATA_CHILD))
        && hardened_index(&p[8]).is_some()
}

/// The DIP-15 receiving (friendship) account
/// `m/9'/coin'/15'/account'/<user id>/<friend id>`, the ids being DIP-14
/// non-hardened 256-bit children.
pub fn is_dashpay_receiving_account(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 6
        && has_dip9_feature(p, network, FEATURE_PURPOSE_DASHPAY)
        && hardened_index(&p[3]).is_some()
        && matches!(p[4], ChildNumber::Normal256 { .. })
        && matches!(p[5], ChildNumber::Normal256 { .. })
}

/// An address of a DIP-15 receiving account: the account path plus one
/// non-hardened index.
pub fn is_dashpay_receiving_address(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 7 && is_dashpay_receiving_account(&p[..6].into(), network) && is_normal(&p[6])
}

/// The DIP-15 auto-accept key `m/9'/coin'/16'/expiry'`.
pub fn is_auto_accept_key(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 4
        && has_dip9_feature(p, network, FEATURE_PURPOSE_DASHPAY_AUTO_ACCEPT)
        && hardened_index(&p[3]).is_some()
}

/// BIP44 account 0 itself, `m/44'/coin'/0'`: the account whose xpub
/// platform-wallet's seed-binding check compares (`seed_binding.rs`).
pub fn is_bip44_account_zero(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 3
        && is_hardened(&p[0], BIP44)
        && is_hardened(&p[1], coin_type(network))
        && is_hardened(&p[2], 0)
}

/// A BIP44 address `m/44'/coin'/account'/{0,1}/index`.
pub fn is_bip44_address(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 5 && is_bip44_path(path, network) && is_chain_and_index(&p[3..])
}

/// A BIP32 account address `m/account'/{0,1}/index` (key-wallet's
/// `BIP32Account`). The second step is never hardened, so no such path
/// reaches a DIP9 or BIP44 key.
pub fn is_bip32_address(path: &DerivationPath) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 3 && hardened_index(&p[0]).is_some() && is_chain_and_index(&p[1..])
}

fn is_chain_and_index(p: &[ChildNumber]) -> bool {
    matches!(p[0], ChildNumber::Normal { index: 0 | 1 }) && is_normal(&p[1])
}

/// An asset-lock credit key under `m/9'/coin'/5'`: registration
/// `1'/index`, top-up `2'/index` or `2'/identity'/index`, invitation
/// `3'/index` (key-wallet `AccountType::Identity*`, whose pools use
/// non-hardened indices). A hardened last step is refused, so no credit key
/// is ever the top-up account node itself
/// ([`is_identity_top_up_account`]).
///
/// Before invitation creation (roadmap X3) exports an invitation key:
/// platform-wallet documents it as hardened (`m/9'/coin'/5'/3'/index'`,
/// `contact_requests.rs:66-71`), while this shape signs non-hardened ones.
/// A non-hardened child private key exported next to the `5'/3'` account
/// xpub the wallet holds reveals the account xpriv and every voucher, so X3
/// must use hardened voucher keys or keep that xpub out of the wallet.
pub fn is_asset_lock_credit_key(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    if !has_dip9_feature(p, network, FEATURE_PURPOSE_IDENTITIES) || p.len() < 5 {
        return false;
    }
    let leaf = is_normal(&p[p.len() - 1]);
    match (hardened_index(&p[3]), p.len()) {
        (Some(FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_REGISTRATION), 5)
        | (Some(FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_TOPUP), 5)
        | (Some(FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_INVITATIONS), 5) => leaf,
        (Some(FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_TOPUP), 6) => {
            hardened_index(&p[4]).is_some() && leaf
        }
        _ => false,
    }
}

/// The top-up account of one identity, `m/9'/coin'/5'/2'/identity'`
/// (key-wallet `AccountType::IdentityTopUp`). platform-wallet asks the
/// signer for its extended public key when it first tops that identity up
/// (`asset_lock/build.rs:600-620`).
pub fn is_identity_top_up_account(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() == 5
        && has_dip9_feature(p, network, FEATURE_PURPOSE_IDENTITIES)
        && is_hardened(&p[3], FEATURE_PURPOSE_IDENTITIES_SUBFEATURE_TOPUP)
        && hardened_index(&p[4]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn path(s: &str) -> DerivationPath {
        DerivationPath::from_str(s).unwrap()
    }

    /// `m/9'/1'/15'/0'/<0x11…>/<0x22…>` built with 256-bit children.
    fn receiving(coin: u32, extra: Option<ChildNumber>) -> DerivationPath {
        let mut p = vec![
            ChildNumber::Hardened { index: 9 },
            ChildNumber::Hardened { index: coin },
            ChildNumber::Hardened { index: 15 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Normal256 { index: [0x11; 32] },
            ChildNumber::Normal256 { index: [0x22; 32] },
        ];
        p.extend(extra);
        p.into()
    }

    #[test]
    fn coinjoin_path_check() {
        let ok = path("m/9'/1'/4'/0'/0/1");
        assert!(is_coinjoin_path(&ok, Network::Testnet));
        assert!(!is_coinjoin_path(&ok, Network::Mainnet));
        let main = path("m/9'/5'/4'/0'");
        assert!(is_coinjoin_path(&main, Network::Mainnet));
        for bad in [
            "m/44'/1'/0'/0/0",
            "m/9'/1'/5'/0'",
            "m/9'/1'/4'",
            "m/9/1'/4'/0'",
        ] {
            assert!(!is_coinjoin_path(&path(bad), Network::Testnet), "{bad}");
        }
    }

    #[test]
    fn bip44_path_check() {
        let ok = path("m/44'/1'/0'/1/7");
        assert!(is_bip44_path(&ok, Network::Testnet));
        assert!(!is_bip44_path(&ok, Network::Mainnet));
        for bad in [
            "m/44'/1'/0",
            "m/44'/1'",
            "m/9'/1'/4'/0'/0/1",
            "m/45'/1'/0'/0/0",
        ] {
            assert!(!is_bip44_path(&path(bad), Network::Testnet), "{bad}");
        }
    }

    #[test]
    fn identity_auth_key_shape() {
        let t = Network::Testnet;
        assert_eq!(
            identity_auth_key(&path("m/9'/1'/5'/0'/0'/3'/5'"), t),
            Some((3, 5))
        );
        assert_eq!(
            identity_auth_key(&path("m/9'/5'/5'/0'/0'/0'/0'"), Network::Mainnet),
            Some((0, 0))
        );
        for bad in [
            "m/9'/5'/5'/0'/0'/0'/0'", // mainnet coin on testnet
            "m/9'/1'/5'/0'/0'/0'",    // the identity, not a key
            "m/9'/1'/5'/0'/0'/0'/0'/0'",
            "m/9'/1'/5'/0'/0'/0'/0",  // non-hardened key
            "m/9'/1'/5'/0'/0'/0/0'",  // non-hardened identity
            "m/9'/1'/5'/0'/1'/0'/0'", // BLS key type
            "m/9'/1'/5'/1'/0'/0'/0'", // registration funding
            "m/9'/1'/15'/0'/0'/0'/0'",
            "m/44'/1'/5'/0'/0'/0'/0'",
        ] {
            assert_eq!(identity_auth_key(&path(bad), t), None, "{bad}");
        }
    }

    #[test]
    fn contact_info_key_shape() {
        let t = Network::Testnet;
        assert!(is_contact_info_key(
            &path("m/9'/1'/5'/0'/0'/0'/2'/65536'/0'"),
            t
        ));
        assert!(is_contact_info_key(
            &path("m/9'/1'/5'/0'/0'/1'/0'/65537'/9'"),
            t
        ));
        for bad in [
            "m/9'/1'/5'/0'/0'/0'/2'/65538'/0'",
            "m/9'/1'/5'/0'/0'/0'/2'/65536'/0",
            "m/9'/1'/5'/0'/0'/0'/2'/65536'",
            "m/9'/1'/5'/0'/0'/0'/2'/65536'/0'/0'",
            "m/9'/1'/5'/0'/0'/0'/2'/0'/0'",
            "m/9'/1'/5'/0'/0'/0'/2'",
            "m/9'/1'/5'/1'/0'/0'/2'/65536'/0'",
            "m/9'/5'/5'/0'/0'/0'/2'/65536'/0'",
        ] {
            assert!(!is_contact_info_key(&path(bad), t), "{bad}");
        }
    }

    #[test]
    fn dashpay_receiving_shapes() {
        let t = Network::Testnet;
        assert!(is_dashpay_receiving_account(&receiving(1, None), t));
        assert!(!is_dashpay_receiving_account(
            &receiving(1, None),
            Network::Mainnet
        ));
        assert!(is_dashpay_receiving_account(
            &receiving(5, None),
            Network::Mainnet
        ));
        let address = receiving(1, Some(ChildNumber::Normal { index: 7 }));
        assert!(is_dashpay_receiving_address(&address, t));
        assert!(!is_dashpay_receiving_account(&address, t));
        assert!(!is_dashpay_receiving_address(&receiving(1, None), t));
        let hardened_leaf = receiving(1, Some(ChildNumber::Hardened { index: 7 }));
        assert!(!is_dashpay_receiving_address(&hardened_leaf, t));
        // 31-bit or hardened 256-bit ids are not DIP-15 receiving accounts.
        let mut p: Vec<ChildNumber> = receiving(1, None).into();
        p[5] = ChildNumber::Hardened256 { index: [0x22; 32] };
        assert!(!is_dashpay_receiving_account(&p.clone().into(), t));
        p[5] = ChildNumber::Normal { index: 1 };
        assert!(!is_dashpay_receiving_account(&p.into(), t));
        assert!(!is_dashpay_receiving_account(
            &path("m/9'/1'/15'/0'/1/2"),
            t
        ));
    }

    #[test]
    fn auto_accept_and_bip44_account_zero() {
        let t = Network::Testnet;
        assert!(is_auto_accept_key(&path("m/9'/1'/16'/1900000000'"), t));
        for bad in [
            "m/9'/1'/16'/1900000000",
            "m/9'/1'/16'",
            "m/9'/1'/16'/1'/0'",
            "m/9'/5'/16'/1'",
            "m/9'/1'/15'/1'",
        ] {
            assert!(!is_auto_accept_key(&path(bad), t), "{bad}");
        }
        assert!(is_bip44_account_zero(&path("m/44'/1'/0'"), t));
        for bad in ["m/44'/1'/1'", "m/44'/1'/0'/0", "m/44'/1'", "m/44'/5'/0'"] {
            assert!(!is_bip44_account_zero(&path(bad), t), "{bad}");
        }
    }

    #[test]
    fn funding_input_shapes() {
        let t = Network::Testnet;
        assert!(is_bip44_address(&path("m/44'/1'/0'/0/3"), t));
        assert!(is_bip44_address(&path("m/44'/1'/2'/1/0"), t));
        for bad in [
            "m/44'/1'/0'/2/3",
            "m/44'/1'/0'/0'/3",
            "m/44'/1'/0'/0/3'",
            "m/44'/1'/0'/0",
            "m/44'/1'/0'/0/3/1",
        ] {
            assert!(!is_bip44_address(&path(bad), t), "{bad}");
        }
        assert!(is_bip32_address(&path("m/0'/0/4")));
        assert!(is_bip32_address(&path("m/3'/1/0")));
        for bad in [
            "m/0/0/4",
            "m/0'/2/4",
            "m/0'/0'/4",
            "m/0'/0/4/0",
            "m/9'/1'/5'",
        ] {
            assert!(!is_bip32_address(&path(bad)), "{bad}");
        }
    }

    /// Every dynamic step of every shape, as a raw `ChildNumber`: the last
    /// 31-bit index is accepted; `2^31`, `u32::MAX` and 256-bit children are
    /// not (review DW-E0-03 M2).
    #[test]
    fn dynamic_steps_take_31_bit_indices_only() {
        let t = Network::Testnet;
        type Shape = fn(&DerivationPath, Network) -> bool;
        let bip32: Shape = |p, _| is_bip32_address(p);
        let identity: Shape = |p, n| identity_auth_key(p, n).is_some();
        // (shape, a matching path, the dynamic steps, hardened?)
        let cases: [(&str, Shape, DerivationPath, &[usize], bool); 13] = [
            (
                "identity",
                identity,
                path("m/9'/1'/5'/0'/0'/3'/5'"),
                &[5, 6],
                true,
            ),
            (
                "contactInfo",
                is_contact_info_key,
                path("m/9'/1'/5'/0'/0'/3'/5'/65536'/2'"),
                &[5, 6, 8],
                true,
            ),
            (
                "receiving account",
                is_dashpay_receiving_account,
                receiving(1, None),
                &[3],
                true,
            ),
            (
                "receiving address",
                is_dashpay_receiving_address,
                receiving(1, Some(ChildNumber::Normal { index: 4 })),
                &[6],
                false,
            ),
            (
                "auto-accept",
                is_auto_accept_key,
                path("m/9'/1'/16'/7'"),
                &[3],
                true,
            ),
            (
                "bip44 path",
                is_bip44_path,
                path("m/44'/1'/2'/0"),
                &[2],
                true,
            ),
            (
                "bip44 address account",
                is_bip44_address,
                path("m/44'/1'/2'/1/4"),
                &[2],
                true,
            ),
            (
                "bip44 address leaf",
                is_bip44_address,
                path("m/44'/1'/2'/1/4"),
                &[4],
                false,
            ),
            ("bip32 account", bip32, path("m/3'/1/4"), &[0], true),
            ("bip32 leaf", bip32, path("m/3'/1/4"), &[2], false),
            (
                "credit key leaf",
                is_asset_lock_credit_key,
                path("m/9'/1'/5'/1'/4"),
                &[4],
                false,
            ),
            (
                "top-up credit key",
                is_asset_lock_credit_key,
                path("m/9'/1'/5'/2'/3'/4"),
                &[4],
                true,
            ),
            (
                "top-up account",
                is_identity_top_up_account,
                path("m/9'/1'/5'/2'/3'"),
                &[4],
                true,
            ),
        ];
        for (name, shape, base, steps, hardened) in cases {
            assert!(shape(&base, t), "{name}: base");
            let base: Vec<ChildNumber> = base.into();
            let step = |index| {
                if hardened {
                    ChildNumber::Hardened { index }
                } else {
                    ChildNumber::Normal { index }
                }
            };
            for &at in steps {
                let with = |c: ChildNumber| {
                    let mut p = base.clone();
                    p[at] = c;
                    shape(&p.into(), t)
                };
                assert!(with(step(INDEX_LIMIT - 1)), "{name}[{at}]: 2^31-1");
                for bad in [
                    step(INDEX_LIMIT),
                    step(INDEX_LIMIT | 3),
                    step(u32::MAX),
                    ChildNumber::Hardened256 { index: [0x42; 32] },
                    ChildNumber::Normal256 { index: [0x42; 32] },
                ] {
                    assert!(!with(bad), "{name}[{at}]: {bad:?}");
                }
            }
        }
    }

    #[test]
    fn asset_lock_credit_key_shapes() {
        let t = Network::Testnet;
        for ok in [
            "m/9'/1'/5'/1'/0",
            "m/9'/1'/5'/2'/3",
            "m/9'/1'/5'/2'/0'/3",
            "m/9'/1'/5'/3'/2",
        ] {
            assert!(is_asset_lock_credit_key(&path(ok), t), "{ok}");
        }
        for bad in [
            "m/9'/1'/5'/0'/0'",       // identity authentication
            "m/9'/1'/5'/0'/0'/0'/0'", // an identity key
            "m/9'/1'/5'/4'/0",        // address top-up (not in scope)
            "m/9'/1'/5'/1'",
            "m/9'/1'/5'/1'/4'", // hardened leaf
            "m/9'/1'/5'/2'/0'", // the top-up account node
            "m/9'/1'/5'/1'/0/0",
            "m/9'/1'/5'/2'/0/3",
            "m/9'/1'/5'/2'/0'/3'",
            "m/9'/1'/5'/3'/0'/1",
            "m/9'/5'/5'/1'/0",
            "m/9'/1'/4'/1'/0",
        ] {
            assert!(!is_asset_lock_credit_key(&path(bad), t), "{bad}");
        }
        assert!(is_identity_top_up_account(&path("m/9'/1'/5'/2'/0'"), t));
        for bad in [
            "m/9'/1'/5'/2'/0",
            "m/9'/1'/5'/2'",
            "m/9'/1'/5'/2'/0'/0",
            "m/9'/1'/5'/1'/0'",
            "m/9'/5'/5'/2'/0'",
        ] {
            assert!(!is_identity_top_up_account(&path(bad), t), "{bad}");
        }
    }
}
