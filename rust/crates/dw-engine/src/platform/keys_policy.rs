//! The DashPay identity key policy (DASHPAY §3.1 `keys_policy.rs`, roadmap
//! DP1-01): which keys an identity registers, and which it adds later.
//!
//! Ported from the iOS wallet at dashwallet-ios `37c0e78a2f`:
//! `DWDashPayIdentityKeys.swift` (the DashPay pair, its bounds, recipient
//! eligibility), `DWIdentityRegistrationCoordinator.swift` (`defaultKeyCount`,
//! the pair appended at ids 4 and 5) and `DWIdentityKeyUpgrader.swift` (the
//! IdentityUpdate fallback). iOS takes keys 0–3 from SwiftDashSDK's
//! `prePersistIdentityKeysForRegistration`, whose policy is Rust:
//! `rs-platform-wallet-ffi/src/identity_derive_and_persist.rs` at platform
//! `bc41f1bc23`.
//!
//! | id | purpose        | security level | contract bounds                    |
//! |----|----------------|----------------|------------------------------------|
//! | 0  | AUTHENTICATION | MASTER         | none                               |
//! | 1  | AUTHENTICATION | CRITICAL       | none                               |
//! | 2  | AUTHENTICATION | HIGH           | none                               |
//! | 3  | TRANSFER       | CRITICAL       | none                               |
//! | 4  | ENCRYPTION     | MEDIUM         | DashPay `contactRequest`           |
//! | 5  | DECRYPTION     | MEDIUM         | DashPay `contactRequest`           |
//!
//! Every key is `ECDSA_SECP256K1`, derived at the DIP-13 slot
//! `m/9'/coin'/5'/0'/0'/identity'/key'` with the key index equal to the key
//! id (what [`super::VaultIdentitySigner`] signs with), and is version 0,
//! writable and unlimited, as the FFI's `decode_identity_pubkeys` builds
//! iOS's rows. Keys are derived when needed and never stored (DASHPAY §3.3).

use std::collections::{BTreeMap, BTreeSet};

use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::{IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::system_data_contracts::dashpay_contract;
use dw_vault::VaultSigner;
use key_wallet::Signer;
use key_wallet::bip32::{DerivationPath, KeyDerivationType};
use platform_wallet::wallet::identity::network::identity_auth_derivation_path_for_type;

use crate::EngineError;

/// The keys every identity has before its DashPay pair: AUTHENTICATION
/// MASTER, CRITICAL and HIGH, then TRANSFER CRITICAL (iOS `defaultKeyCount`).
pub const BASE_KEY_COUNT: KeyID = 4;

/// The DashPay document type the encryption and decryption keys are bound to.
pub const CONTACT_REQUEST: &str = "contactRequest";

/// One key the policy asks for. The key type is always `ECDSA_SECP256K1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub id: KeyID,
    pub purpose: Purpose,
    pub security_level: SecurityLevel,
    pub contract_bounds: Option<ContractBounds>,
}

impl KeySpec {
    fn unbounded(id: KeyID, purpose: Purpose, security_level: SecurityLevel) -> Self {
        Self {
            id,
            purpose,
            security_level,
            contract_bounds: None,
        }
    }

    /// A DashPay key (iOS `KeySpecification`): MEDIUM, bound to the DashPay
    /// contract's `contactRequest` documents.
    fn dashpay(id: KeyID, purpose: Purpose) -> Self {
        Self {
            id,
            purpose,
            security_level: SecurityLevel::MEDIUM,
            contract_bounds: Some(ContractBounds::SingleContractDocumentType {
                id: dashpay_contract::ID,
                document_type_name: CONTACT_REQUEST.to_owned(),
            }),
        }
    }
}

/// The DashPay purposes, in the order iOS adds them.
const DASHPAY_PURPOSES: [Purpose; 2] = [Purpose::ENCRYPTION, Purpose::DECRYPTION];

/// The six keys of a new identity (the table above).
pub fn registration_specs() -> Vec<KeySpec> {
    let mut specs = vec![
        KeySpec::unbounded(0, Purpose::AUTHENTICATION, SecurityLevel::MASTER),
        KeySpec::unbounded(1, Purpose::AUTHENTICATION, SecurityLevel::CRITICAL),
        KeySpec::unbounded(2, Purpose::AUTHENTICATION, SecurityLevel::HIGH),
        KeySpec::unbounded(3, Purpose::TRANSFER, SecurityLevel::CRITICAL),
    ];
    specs.extend(
        (BASE_KEY_COUNT..)
            .zip(DASHPAY_PURPOSES)
            .map(|(id, purpose)| KeySpec::dashpay(id, purpose)),
    );
    specs
}

fn is_enabled_ecdsa(key: &IdentityPublicKey) -> bool {
    key.key_type() == KeyType::ECDSA_SECP256K1 && key.disabled_at().is_none()
}

/// The DashPay purposes `keys` (an identity's key set, from Platform) has no
/// enabled `ECDSA_SECP256K1` key for, ENCRYPTION first (iOS
/// `DWIdentityKeyUpgrader.missingDashPayPurposes`). Empty means the identity
/// has its DashPay keys. Bounds are not checked, as on iOS.
pub fn missing_dashpay_purposes<'a>(
    keys: impl IntoIterator<Item = &'a IdentityPublicKey>,
) -> Vec<Purpose> {
    let present: Vec<Purpose> = keys
        .into_iter()
        .filter(|k| is_enabled_ecdsa(k))
        .map(|k| k.purpose())
        .collect();
    DASHPAY_PURPOSES
        .into_iter()
        .filter(|p| !present.contains(p))
        .collect()
}

/// The keys an IdentityUpdate adds for "Enable DashPay keys" (DP2-02), as
/// iOS `ensureDashPayKeys` picks them: one per purpose missing from
/// `platform_keys` (the identity's key set as Platform returns it, never
/// local rows, which can be stale), at the ids after the highest of
/// `platform_keys` (disabled keys included) and `local_max_id` (the highest
/// id the wallet stores). Empty when nothing is missing.
pub fn upgrade_specs<'a>(
    platform_keys: impl IntoIterator<Item = &'a IdentityPublicKey>,
    local_max_id: Option<KeyID>,
) -> Vec<KeySpec> {
    let platform_keys: Vec<&IdentityPublicKey> = platform_keys.into_iter().collect();
    let platform_max_id = platform_keys.iter().map(|k| k.id()).max();
    // iOS: `max(networkMaxId ?? 0, localMaxId ?? 0) + 1`. Saturating: an id
    // that large has no DIP-13 slot, so `derive_keys` refuses it.
    let next = platform_max_id
        .unwrap_or(0)
        .max(local_max_id.unwrap_or(0))
        .saturating_add(1);
    missing_dashpay_purposes(platform_keys)
        .into_iter()
        .zip(0..)
        .map(|(purpose, i)| KeySpec::dashpay(next.saturating_add(i), purpose))
        .collect()
}

/// Whether a contact request can be sent to an identity with key set `keys`
/// (iOS `recipientEligibility`): it has an enabled `ECDSA_SECP256K1` key
/// whose purpose the SDK accepts as the recipient key, DECRYPTION or
/// ENCRYPTION (platform-wallet `select_recipient_key_index`). A key set that
/// could not be fetched is the caller's "unknown", not `false`.
pub fn is_dashpay_recipient<'a>(keys: impl IntoIterator<Item = &'a IdentityPublicKey>) -> bool {
    keys.into_iter().any(|k| {
        is_enabled_ecdsa(k)
            && dash_sdk::platform::dashpay::recipient_key_purpose_is_valid(k.purpose())
    })
}

/// The DIP-13 path of key `id` of identity `identity_index`.
pub fn key_path(
    network: key_wallet::Network,
    identity_index: u32,
    id: KeyID,
) -> Result<DerivationPath, EngineError> {
    identity_auth_derivation_path_for_type(network, KeyDerivationType::ECDSA, identity_index, id)
        .map_err(|e| EngineError::InvalidArgument(e.to_string()))
}

/// The public keys of `specs` for identity `identity_index`, keyed by id, for
/// IdentityCreate or IdentityUpdate. `signer` derives them (a
/// `PlatformIdentity` signer may); no private key leaves the vault.
pub async fn derive_keys(
    signer: &VaultSigner,
    identity_index: u32,
    specs: &[KeySpec],
) -> Result<BTreeMap<KeyID, IdentityPublicKey>, EngineError> {
    let mut ids = BTreeSet::new();
    if let Some(dup) = specs.iter().find(|s| !ids.insert(s.id)) {
        return Err(EngineError::InvalidArgument(format!(
            "duplicate identity key id {}",
            dup.id
        )));
    }
    let mut keys = BTreeMap::new();
    for spec in specs {
        let path = key_path(signer.network(), identity_index, spec.id)?;
        let public_key = signer.public_key(&path).await?;
        let key = IdentityPublicKey::V0(IdentityPublicKeyV0 {
            id: spec.id,
            purpose: spec.purpose,
            security_level: spec.security_level,
            contract_bounds: spec.contract_bounds.clone(),
            key_type: KeyType::ECDSA_SECP256K1,
            read_only: false,
            data: public_key.serialize().to_vec().into(),
            disabled_at: None,
        });
        keys.insert(spec.id, key);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(
        id: KeyID,
        purpose: Purpose,
        key_type: KeyType,
        disabled_at: Option<u64>,
    ) -> IdentityPublicKey {
        IdentityPublicKey::V0(IdentityPublicKeyV0 {
            id,
            purpose,
            security_level: SecurityLevel::MEDIUM,
            contract_bounds: None,
            key_type,
            read_only: false,
            data: vec![2; 33].into(),
            disabled_at,
        })
    }

    fn ecdsa(id: KeyID, purpose: Purpose) -> IdentityPublicKey {
        key(id, purpose, KeyType::ECDSA_SECP256K1, None)
    }

    fn base_keys() -> Vec<IdentityPublicKey> {
        vec![
            ecdsa(0, Purpose::AUTHENTICATION),
            ecdsa(1, Purpose::AUTHENTICATION),
            ecdsa(2, Purpose::AUTHENTICATION),
            ecdsa(3, Purpose::TRANSFER),
        ]
    }

    fn levels(specs: &[KeySpec]) -> Vec<(KeyID, Purpose, SecurityLevel)> {
        specs
            .iter()
            .map(|s| (s.id, s.purpose, s.security_level))
            .collect()
    }

    #[test]
    fn registration_set_is_the_ios_set() {
        let specs = registration_specs();
        assert_eq!(
            levels(&specs),
            vec![
                (0, Purpose::AUTHENTICATION, SecurityLevel::MASTER),
                (1, Purpose::AUTHENTICATION, SecurityLevel::CRITICAL),
                (2, Purpose::AUTHENTICATION, SecurityLevel::HIGH),
                (3, Purpose::TRANSFER, SecurityLevel::CRITICAL),
                (4, Purpose::ENCRYPTION, SecurityLevel::MEDIUM),
                (5, Purpose::DECRYPTION, SecurityLevel::MEDIUM),
            ]
        );
        assert!(specs[..4].iter().all(|s| s.contract_bounds.is_none()));
    }

    /// iOS `testRegistrationSpecificationsMatchDashPayContractPolicy`.
    #[test]
    fn dashpay_pair_is_bound_to_contact_requests() {
        let pair = &registration_specs()[BASE_KEY_COUNT as usize..];
        assert_eq!(pair.len(), 2);
        // DWDashPayIdentityKeys.dashPayContractId.
        let ios_contract_id: [u8; 32] = [
            162, 161, 180, 172, 111, 239, 34, 234, 42, 26, 104, 232, 18, 54, 68, 179, 87, 135, 95,
            107, 65, 44, 24, 16, 146, 129, 193, 70, 231, 178, 113, 188,
        ];
        for spec in pair {
            assert_eq!(
                spec.contract_bounds,
                Some(ContractBounds::SingleContractDocumentType {
                    id: ios_contract_id.into(),
                    document_type_name: "contactRequest".into(),
                })
            );
        }
    }

    /// iOS `DashPayIdentityKeysTests` recipient cases.
    #[test]
    fn recipient_eligibility() {
        let enc = ecdsa(4, Purpose::ENCRYPTION);
        let dec = ecdsa(5, Purpose::DECRYPTION);
        assert!(is_dashpay_recipient([&enc, &dec]));
        assert!(is_dashpay_recipient([&enc]));
        assert!(is_dashpay_recipient([&dec]));
        let auth = ecdsa(0, Purpose::AUTHENTICATION);
        let transfer = ecdsa(3, Purpose::TRANSFER);
        assert!(!is_dashpay_recipient([&auth, &transfer]));
        let disabled = key(4, Purpose::ENCRYPTION, KeyType::ECDSA_SECP256K1, Some(42));
        assert!(!is_dashpay_recipient([&disabled]));
        let bls = key(5, Purpose::DECRYPTION, KeyType::BLS12_381, None);
        assert!(!is_dashpay_recipient([&bls]));
        assert!(!is_dashpay_recipient([]));
    }

    #[test]
    fn complete_identity_needs_no_upgrade() {
        let mut keys = base_keys();
        keys.extend([ecdsa(4, Purpose::ENCRYPTION), ecdsa(5, Purpose::DECRYPTION)]);
        assert!(missing_dashpay_purposes(&keys).is_empty());
        assert!(upgrade_specs(&keys, Some(5)).is_empty());
    }

    #[test]
    fn identity_without_the_pair_gets_both_after_its_last_key() {
        let keys = base_keys();
        assert_eq!(
            missing_dashpay_purposes(&keys),
            vec![Purpose::ENCRYPTION, Purpose::DECRYPTION]
        );
        let specs = upgrade_specs(&keys, None);
        assert_eq!(
            levels(&specs),
            vec![
                (4, Purpose::ENCRYPTION, SecurityLevel::MEDIUM),
                (5, Purpose::DECRYPTION, SecurityLevel::MEDIUM),
            ]
        );
        assert_eq!(specs, registration_specs()[BASE_KEY_COUNT as usize..]);
    }

    /// The mobile cohort with only an ENCRYPTION key gets a DECRYPTION key;
    /// disabled keys count for the next id but not as present, and so do
    /// keys of another type.
    #[test]
    fn upgrade_skips_present_purposes_and_counts_every_id() {
        let mut keys = base_keys();
        keys.extend([
            ecdsa(4, Purpose::ENCRYPTION),
            key(9, Purpose::DECRYPTION, KeyType::ECDSA_SECP256K1, Some(1)),
            key(7, Purpose::DECRYPTION, KeyType::BLS12_381, None),
        ]);
        assert_eq!(missing_dashpay_purposes(&keys), vec![Purpose::DECRYPTION]);
        assert_eq!(
            levels(&upgrade_specs(&keys, Some(3))),
            vec![(10, Purpose::DECRYPTION, SecurityLevel::MEDIUM)]
        );
    }

    /// Local rows move the next id (a key the wallet derived but Platform
    /// does not show yet keeps its slot) but never count as present: only
    /// Platform says which purposes the identity has.
    #[test]
    fn local_rows_only_move_the_next_id() {
        let keys = base_keys();
        assert_eq!(
            levels(&upgrade_specs(&keys, Some(11))),
            vec![
                (12, Purpose::ENCRYPTION, SecurityLevel::MEDIUM),
                (13, Purpose::DECRYPTION, SecurityLevel::MEDIUM),
            ]
        );
        assert_eq!(upgrade_specs(&keys, Some(1))[0].id, 4);
    }

    /// No overflow on an absurd id: the specs saturate, and no DIP-13 slot
    /// exists for them.
    #[test]
    fn upgrade_ids_saturate() {
        let keys = [ecdsa(u32::MAX, Purpose::AUTHENTICATION)];
        let specs = upgrade_specs(&keys, None);
        assert!(specs.iter().all(|s| s.id == u32::MAX));
        assert!(key_path(key_wallet::Network::Testnet, 0, u32::MAX).is_err());
    }

    #[test]
    fn key_path_is_the_dip13_slot() {
        let path = |network, i, id| key_path(network, i, id).unwrap().to_string();
        assert_eq!(
            path(key_wallet::Network::Mainnet, 0, 5),
            "m/9'/5'/5'/0'/0'/0'/5'"
        );
        assert_eq!(
            path(key_wallet::Network::Testnet, 3, 0),
            "m/9'/1'/5'/0'/0'/3'/0'"
        );
        assert_eq!(
            path(key_wallet::Network::Regtest, 1, 4),
            "m/9'/1'/5'/0'/0'/1'/4'"
        );
        assert!(key_path(key_wallet::Network::Testnet, 1 << 31, 0).is_err());
    }
}
