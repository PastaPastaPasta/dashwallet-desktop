//! MG-13 P3, P12 (the vault): the `MasternodeWithdraw` grant, its
//! single-use `MasternodeWithdrawal` signer and
//! `VaultSigner::sign_masternode_withdrawal`, held byte-equal to Platform's
//! own construction (`IdentityCreditWithdrawalTransition::try_from_identity`,
//! which platform-wallet's `masternode_withdraw` calls) and checked with
//! dpp's ECDSA signature verification over the signable bytes, against the
//! identity key's hash160.

mod common;

use std::str::FromStr;

use async_trait::async_trait;
use common::*;
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::signer::Signer as IdentitySigner;
use dpp::identity::{Identity, IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::{BinaryData, Identifier};
use dpp::serialization::{PlatformSerializable, Signable};
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::methods::{
    IdentityCreditWithdrawalTransitionMethodsV0, PreferredKeyPurposeForSigningWithdrawal,
};
use dpp::state_transition::identity_credit_withdrawal_transition::v0::IdentityCreditWithdrawalTransitionV0;
use dpp::state_transition::identity_credit_withdrawal_transition::v1::IdentityCreditWithdrawalTransitionV1;
use dpp::state_transition::identity_credit_withdrawal_transition::{
    IdentityCreditWithdrawalTransition, MIN_CORE_FEE_PER_BYTE,
};
use dpp::version::PlatformVersion;
use dpp::withdrawal::Pooling;
use dw_vault::{
    CREDITS_PER_DUFF, Credential, GrantKind, GrantPurpose, GrantToken, OwnerKey, SignerError,
    SignerScope, Vault, VaultError, VaultSigner, WalletSigner, WithdrawalDestination,
    WithdrawalKey,
};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use key_wallet::{ExtendedPubKeySigner, Signer};

/// A masternode's proTxHash, wire order; its owner identity is the reverse.
const PRO_TX: [u8; 32] = {
    let mut b = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        b[i] = i as u8;
        i += 1;
    }
    b
};
const ATTACHED_OWNER: [u8; 32] = [0x07; 32];
const ATTACHED_PAYOUT: [u8; 32] = [0x09; 32];
const PAYOUT_PATH: &str = "m/44'/1'/0'/0/3";
const MAX: u64 = 5_000_000;
const AMOUNT: u64 = 1_000_000;
const NONCE: u64 = 7;

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

/// The owner identity of `PRO_TX`: the proTxHash in display order.
fn owner_identity() -> [u8; 32] {
    let mut id = PRO_TX;
    id.reverse();
    id
}

/// An encrypted vault holding wallet 1, unlocked, with `PRO_TX`'s owner and
/// payout keys attached.
fn encrypted(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v.store_masternode_key(&PRO_TX, "owner", &ATTACHED_OWNER)
        .unwrap();
    v.store_masternode_key(&PRO_TX, "payout", &ATTACHED_PAYOUT)
        .unwrap();
    v
}

/// A redeemed grant of `purpose` for wallet 1.
fn token(v: &Vault, purpose: GrantPurpose, credential: Credential<'_>) -> GrantToken {
    let grant = v.authorize(purpose, Some(&wallet(1)), credential).unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap()
}

fn hash160_of(secret: &SecretKey) -> [u8; 20] {
    hash160::Hash::hash(&PublicKey::from_secret_key(secret).serialize()).to_byte_array()
}

/// Wallet 1's key at `p`, derived here.
fn derived(v: &Vault, p: &str) -> SecretKey {
    let master = ExtendedPrivKey::new_master(v.network(), &[1; 64]).unwrap();
    master.derive_priv(&path(p)).unwrap().private_key
}

/// The P2PKH script of `secret`.
fn p2pkh(secret: &SecretKey) -> CoreScript {
    CoreScript::new_p2pkh(hash160_of(secret))
}

/// One call of `sign_masternode_withdrawal` and the scope it needs.
#[derive(Clone)]
struct Case {
    key: WithdrawalKey,
    secret: SecretKey,
    identity_key: IdentityPublicKey,
    t: IdentityCreditWithdrawalTransitionV1,
    pro_tx_hash: [u8; 32],
    max_credits: u64,
    destination: Option<WithdrawalDestination>,
}

impl Case {
    fn transition(&self) -> StateTransition {
        IdentityCreditWithdrawalTransition::V1(self.t.clone()).into()
    }

    fn purpose(&self) -> GrantPurpose {
        GrantPurpose::MasternodeWithdraw {
            pro_tx_hash: self.pro_tx_hash,
            max_credits: self.max_credits,
            destination: self.destination,
        }
    }

    fn sign(&self, s: &VaultSigner) -> Result<[u8; 65], SignerError> {
        s.sign_masternode_withdrawal(
            &self.key,
            &self.identity_key,
            &self.transition(),
            &self.pro_tx_hash,
        )
    }

    /// The transition carrying `sig`.
    fn signed(&self, sig: [u8; 65]) -> StateTransition {
        let mut t = self.t.clone();
        t.signature = BinaryData::new(sig.to_vec());
        IdentityCreditWithdrawalTransition::V1(t).into()
    }
}

/// The fields of a V0 identity key, the version Drive gives the owner
/// identity's keys.
fn v0(k: &mut IdentityPublicKey) -> &mut IdentityPublicKeyV0 {
    match k {
        IdentityPublicKey::V0(k) => k,
        other => panic!("{other:?}"),
    }
}

/// The identity key Drive gives the owner identity for `secret`
/// (`get_owner_identity_owner_key_v0`, `…_withdrawal_key_v0`): OWNER id 1
/// or TRANSFER id 0, CRITICAL, read-only, `ECDSA_HASH160`.
fn identity_key(owner: bool, secret: &SecretKey) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: if owner { 1 } else { 0 },
        purpose: if owner {
            Purpose::OWNER
        } else {
            Purpose::TRANSFER
        },
        security_level: SecurityLevel::CRITICAL,
        contract_bounds: None,
        key_type: KeyType::ECDSA_HASH160,
        read_only: true,
        data: hash160_of(secret).to_vec().into(),
        disabled_at: None,
    })
}

/// A valid withdrawal with `key`, whose secret is `secret`; a payout key
/// pays its own address.
fn case(key: WithdrawalKey, secret: SecretKey) -> Case {
    let owner = key.is_owner();
    let script = (!owner).then(|| p2pkh(&secret));
    let destination = script
        .as_ref()
        .map(|s| WithdrawalDestination::of_script(s.as_bytes()));
    let identity_key = identity_key(owner, &secret);
    Case {
        t: IdentityCreditWithdrawalTransitionV1 {
            identity_id: Identifier::from(owner_identity()),
            amount: AMOUNT,
            core_fee_per_byte: MIN_CORE_FEE_PER_BYTE,
            pooling: Pooling::Never,
            output_script: script,
            nonce: NONCE,
            user_fee_increase: 0,
            signature_public_key_id: if owner { 1 } else { 0 },
            signature: BinaryData::default(),
        },
        key,
        secret,
        identity_key,
        pro_tx_hash: PRO_TX,
        max_credits: MAX,
        destination,
    }
}

/// One case per key kind.
fn cases(v: &Vault) -> [Case; 4] {
    [
        case(
            WithdrawalKey::KeychainOwner { index: 2 },
            derived(v, "m/9'/1'/3'/2'/2"),
        ),
        case(
            WithdrawalKey::AttachedOwner {
                pro_tx_hash: PRO_TX,
            },
            SecretKey::from_secret_bytes(ATTACHED_OWNER).unwrap(),
        ),
        case(
            WithdrawalKey::WalletPayout {
                path: path(PAYOUT_PATH),
            },
            derived(v, PAYOUT_PATH),
        ),
        case(
            WithdrawalKey::AttachedPayout {
                pro_tx_hash: PRO_TX,
            },
            SecretKey::from_secret_bytes(ATTACHED_PAYOUT).unwrap(),
        ),
    ]
}

fn withdrawal_signer(v: &Vault, c: &Case) -> VaultSigner {
    v.masternode_withdrawal_signer(&wallet(1), &token(v, c.purpose(), Credential::None))
        .unwrap()
}

/// A single-key `ECDSA_HASH160` signer, as rs-sdk's simple-signer
/// `SingleKeySigner` signs: `dashcore::signer::sign` of the data.
#[derive(Debug)]
struct OneKey(SecretKey);

#[async_trait]
impl IdentitySigner<IdentityPublicKey> for OneKey {
    async fn sign(&self, _: &IdentityPublicKey, data: &[u8]) -> Result<BinaryData, ProtocolError> {
        let sig = dpp::dashcore::signer::sign(data, &self.0.to_secret_bytes())
            .map_err(|e| ProtocolError::Generic(e.to_string()))?;
        Ok(sig.to_vec().into())
    }

    async fn sign_create_witness(
        &self,
        _: &IdentityPublicKey,
        _: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        unreachable!("a withdrawal has no address witness")
    }

    fn can_sign_with(&self, _: &IdentityPublicKey) -> bool {
        true
    }
}

/// Platform's construction of `c`: `try_from_identity` over the owner
/// identity holding `c`'s identity key, signed with `c`'s secret.
async fn platform(c: &Case) -> StateTransition {
    let version = PlatformVersion::latest();
    let mut identity =
        Identity::create_basic_identity(Identifier::from(owner_identity()), version).unwrap();
    identity.add_public_keys([c.identity_key.clone()]);
    IdentityCreditWithdrawalTransition::try_from_identity(
        &identity,
        c.t.output_script.clone(),
        c.t.amount,
        Pooling::Never,
        MIN_CORE_FEE_PER_BYTE,
        0,
        OneKey(c.secret),
        Some(&c.identity_key),
        PreferredKeyPurposeForSigningWithdrawal::TransferPreferred,
        c.t.nonce,
        version,
        None,
    )
    .await
    .unwrap()
}

// MARK: The grant (P12)

#[test]
fn the_grant_purpose_is_wallet_bound_and_carries_its_target_cap_and_destination() {
    let dest = WithdrawalDestination::of_script(&[0x76, 0xa9, 0x14]);
    let p = GrantPurpose::MasternodeWithdraw {
        pro_tx_hash: PRO_TX,
        max_credits: 2_000_001,
        destination: Some(dest),
    };
    assert_eq!(p.kind(), GrantKind::MasternodeWithdraw);
    assert!(p.wallet_scoped());
    assert!(!p.requires_credential());
    assert_eq!(p.quick_unlock_value(), Some(2_001));
    assert_eq!(2_000_001u64.div_ceil(CREDITS_PER_DUFF), 2_001);
    assert!(dest.matches(&[0x76, 0xa9, 0x14]));
    assert!(!dest.matches(&[0x76, 0xa9]));
    assert!(!dest.matches(&[0x76, 0xa9, 0x14, 0x00]));

    let fx = Fixture::new();
    let v = encrypted(&fx);
    assert!(v.authorize(p, None, Credential::None).is_err());
    let t = token(&v, p, Credential::None);
    assert_eq!(t.max_credits(), Some(2_000_001));
    assert_eq!(t.max_duffs(), None);
    let s = v.masternode_withdrawal_signer(&wallet(1), &t).unwrap();
    assert_eq!(
        s.scope(),
        SignerScope::MasternodeWithdrawal {
            pro_tx_hash: PRO_TX,
            max_credits: 2_000_001,
            destination: Some(dest),
        }
    );
}

/// The withdrawal signer comes from a `MasternodeWithdraw` grant only, and
/// a `MasternodeWithdraw` grant gives no other signer.
#[test]
fn only_a_withdraw_grant_issues_the_withdrawal_signer() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let w = wallet(1);
    for purpose in [
        GrantPurpose::MasternodeOp { max_duffs: 1 },
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::Governance,
        GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: MAX,
        },
    ] {
        let t = token(&v, purpose, Credential::None);
        assert_eq!(
            v.masternode_withdrawal_signer(&w, &t).map(drop),
            Err(VaultError::GrantPurposeMismatch),
            "{purpose:?}"
        );
    }
    let p = GrantPurpose::MasternodeWithdraw {
        pro_tx_hash: PRO_TX,
        max_credits: MAX,
        destination: None,
    };
    let t = token(&v, p, Credential::None);
    assert_eq!(
        v.masternode_signer(&w, &t).map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
    assert_eq!(
        v.signer(&w, &t).map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
    assert_eq!(
        v.vote_signer(&w, &t).map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
    assert!(
        v.platform_signer(&w, &t, SignerScope::DashPayCrypto)
            .is_err()
    );
    // Bound to its wallet.
    assert_eq!(
        v.masternode_withdrawal_signer(&wallet(2), &t).map(drop),
        Err(VaultError::GrantPurposeMismatch)
    );
}

/// One grant, one signer (r2 m5): a token that issued one issues no other.
#[test]
fn a_grant_issues_one_withdrawal_signer() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let c = &cases(&v)[0];
    let t = token(&v, c.purpose(), Credential::None);
    let s = v.masternode_withdrawal_signer(&wallet(1), &t).unwrap();
    assert_eq!(
        v.masternode_withdrawal_signer(&wallet(1), &t).map(drop),
        Err(VaultError::GrantInvalid)
    );
    c.sign(&s).unwrap();
}

/// On a locked vault a passphrase grant carries its own key: it issues the
/// signer only through a hold, which stops it when dropped.
#[test]
fn an_own_key_grant_signs_through_a_hold() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let c = cases(&v)[1].clone();
    v.lock();
    let w = wallet(1);
    let mut set = [token(&v, c.purpose(), Credential::Passphrase(PASS))];
    assert!(matches!(
        v.masternode_withdrawal_signer(&w, &set[0]),
        Err(VaultError::InvalidArgument(_))
    ));
    let mut set2 = [token(&v, c.purpose(), Credential::Passphrase(PASS))];
    let hold = v.hold_key(&mut set2).unwrap().unwrap();
    let s = v
        .masternode_withdrawal_signer_held(&w, &hold, &set2[0])
        .unwrap();
    c.sign(&s).unwrap();
    // The refused direct call spent the first token: no hold takes it.
    assert!(v.hold_key(&mut set).is_err());

    let mut set3 = [token(&v, c.purpose(), Credential::Passphrase(PASS))];
    let hold = v.hold_key(&mut set3).unwrap().unwrap();
    let s = v
        .masternode_withdrawal_signer_held(&w, &hold, &set3[0])
        .unwrap();
    drop(hold);
    assert_eq!(c.sign(&s), Err(SignerError::Locked));
}

// MARK: Signing (P3)

/// Every key kind signs a valid withdrawal; the result is Platform's own
/// construction byte for byte (RFC 6979 is deterministic) and verifies
/// over its signable bytes for the key whose hash160 is the identity key's
/// data, as Drive's `ECDSA_HASH160` check does.
#[tokio::test]
async fn withdrawals_equal_platforms_construction_and_verify() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    for c in cases(&v) {
        let s = withdrawal_signer(&v, &c);
        let sig = c.sign(&s).unwrap();
        let ours = c.signed(sig);
        let theirs = platform(&c).await;
        assert_eq!(ours, theirs, "{:?}", c.key);
        assert_eq!(
            ours.serialize_to_bytes().unwrap(),
            theirs.serialize_to_bytes().unwrap()
        );
        let signable = ours.signable_bytes().unwrap();
        let public = PublicKey::from_secret_key(&c.secret).serialize();
        assert_eq!(
            hash160::Hash::hash(&public).to_byte_array().as_slice(),
            c.identity_key.data().as_slice()
        );
        dpp::dashcore::signer::verify_data_signature(&signable, &sig, &public).unwrap();
        // Another key does not verify it.
        let other = SecretKey::from_secret_bytes([3; 32]).unwrap();
        let other = PublicKey::from_secret_key(&other).serialize();
        assert!(dpp::dashcore::signer::verify_data_signature(&signable, &sig, &other).is_err());
    }
}

/// An amount equal to the grant's cap is within it.
#[test]
fn the_cap_itself_is_allowed() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let mut c = cases(&v)[2].clone();
    c.t.amount = MAX;
    c.sign(&withdrawal_signer(&v, &c)).unwrap();
}

// MARK: Refusals before the secret (P3)

/// `mutate` makes the case wrong; `is_refusal` recognizes its error. The
/// refusal comes before the secret is read: the same signer then signs the
/// valid case (it was not spent), and on a locked vault the call gives the
/// same refusal rather than `Locked` (no gated operation began).
fn refused_before_the_secret(
    pick: usize,
    mutate: impl Fn(&mut Case),
    is_refusal: impl Fn(&SignerError) -> bool,
) {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let valid = cases(&v)[pick].clone();
    let mut bad = valid.clone();
    mutate(&mut bad);
    // The scope follows the valid case: the mutation is the call's.
    let s = withdrawal_signer(&v, &valid);
    let err = bad.sign(&s).unwrap_err();
    assert!(is_refusal(&err), "{:?}: {err:?}", valid.key);
    valid.sign(&s).unwrap();

    let s = withdrawal_signer(&v, &valid);
    v.lock();
    let err = bad.sign(&s).unwrap_err();
    assert!(is_refusal(&err), "{:?} locked: {err:?}", valid.key);
    assert_eq!(valid.sign(&s), Err(SignerError::Locked));
}

fn path_not_allowed(word: &'static str) -> impl Fn(&SignerError) -> bool {
    move |e| matches!(e, SignerError::PathNotAllowed(m) if m.contains(word))
}

fn key_mismatch(word: &'static str) -> impl Fn(&SignerError) -> bool {
    move |e| matches!(e, SignerError::KeyMismatch(m) if m.contains(word))
}

const ALL: [usize; 4] = [0, 1, 2, 3];

#[test]
fn a_v0_transition_is_refused() {
    for pick in ALL {
        let fx = Fixture::new();
        let v = encrypted(&fx);
        let c = cases(&v)[pick].clone();
        let s = withdrawal_signer(&v, &c);
        let t = &c.t;
        let v0: StateTransition =
            IdentityCreditWithdrawalTransition::V0(IdentityCreditWithdrawalTransitionV0 {
                identity_id: t.identity_id,
                amount: t.amount,
                core_fee_per_byte: t.core_fee_per_byte,
                pooling: t.pooling,
                output_script: t.output_script.clone().unwrap_or_else(|| p2pkh(&c.secret)),
                nonce: t.nonce,
                user_fee_increase: 0,
                signature_public_key_id: t.signature_public_key_id,
                signature: BinaryData::default(),
            })
            .into();
        let err = s
            .sign_masternode_withdrawal(&c.key, &c.identity_key, &v0, &PRO_TX)
            .unwrap_err();
        assert!(path_not_allowed("V1")(&err), "{err:?}");
        c.sign(&s).unwrap();
    }
}

#[test]
fn another_identity_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            // The proTxHash itself, not reversed: the wrong byte order.
            |c| c.t.identity_id = Identifier::from(PRO_TX),
            key_mismatch("another identity"),
        );
    }
}

/// A grant is bound to the masternode the user acted on (MG-13 Sol M1): a
/// withdrawal of another one, consistent in itself (its identity and
/// attached key follow the other proTxHash), signs nothing under it.
#[test]
fn another_masternode_than_the_grants_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| {
                c.pro_tx_hash[0] ^= 1;
                let mut identity = c.pro_tx_hash;
                identity.reverse();
                c.t.identity_id = Identifier::from(identity);
                if let WithdrawalKey::AttachedOwner { pro_tx_hash }
                | WithdrawalKey::AttachedPayout { pro_tx_hash } = &mut c.key
                {
                    *pro_tx_hash = c.pro_tx_hash;
                }
            },
            path_not_allowed("than the grant's"),
        );
    }
}

#[test]
fn an_attached_key_of_another_masternode_is_refused() {
    for pick in [1, 3] {
        refused_before_the_secret(
            pick,
            |c| match &mut c.key {
                WithdrawalKey::AttachedOwner { pro_tx_hash }
                | WithdrawalKey::AttachedPayout { pro_tx_hash } => pro_tx_hash[31] ^= 1,
                _ => unreachable!(),
            },
            key_mismatch("another masternode"),
        );
    }
}

#[test]
fn an_identity_key_of_the_other_purpose_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| {
                let k = v0(&mut c.identity_key);
                k.purpose = if c.key.is_owner() {
                    Purpose::TRANSFER
                } else {
                    Purpose::OWNER
                };
            },
            key_mismatch("purpose"),
        );
        refused_before_the_secret(
            pick,
            |c| {
                let k = v0(&mut c.identity_key);
                k.purpose = Purpose::AUTHENTICATION;
            },
            key_mismatch("purpose"),
        );
    }
}

#[test]
fn an_identity_key_of_another_type_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| {
                let k = v0(&mut c.identity_key);
                k.key_type = KeyType::ECDSA_SECP256K1;
                k.data = PublicKey::from_secret_key(&c.secret)
                    .serialize()
                    .to_vec()
                    .into();
            },
            key_mismatch("ECDSA_HASH160"),
        );
    }
}

#[test]
fn a_disabled_identity_key_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| {
                let k = v0(&mut c.identity_key);
                k.disabled_at = Some(1);
            },
            key_mismatch("disabled"),
        );
    }
}

#[test]
fn an_identity_key_other_than_the_signing_key_id_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| c.t.signature_public_key_id += 1,
            key_mismatch("signing key"),
        );
        refused_before_the_secret(
            pick,
            |c| {
                let k = v0(&mut c.identity_key);
                k.id += 2;
            },
            key_mismatch("signing key"),
        );
    }
}

#[test]
fn a_user_fee_increase_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| c.t.user_fee_increase = 1,
            path_not_allowed("user fee"),
        );
    }
}

#[test]
fn a_core_fee_above_the_minimum_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| c.t.core_fee_per_byte = MIN_CORE_FEE_PER_BYTE + 1,
            path_not_allowed("core fee"),
        );
        refused_before_the_secret(
            pick,
            |c| c.t.core_fee_per_byte = 0,
            path_not_allowed("core fee"),
        );
    }
}

#[test]
fn pooling_is_refused() {
    for pick in ALL {
        for pooling in [Pooling::IfAvailable, Pooling::Standard] {
            refused_before_the_secret(
                pick,
                move |c| c.t.pooling = pooling,
                path_not_allowed("pooled"),
            );
        }
    }
}

#[test]
fn an_amount_above_the_grant_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| c.t.amount = MAX + 1,
            path_not_allowed("above the grant"),
        );
    }
}

/// Owner key: no script (Drive pays the registered payout address).
#[test]
fn an_owner_key_withdrawal_with_a_script_is_refused() {
    for pick in [0, 1] {
        refused_before_the_secret(
            pick,
            |c| c.t.output_script = Some(p2pkh(&c.secret)),
            path_not_allowed("destination"),
        );
    }
}

/// Payout key: exactly the approved script.
#[test]
fn a_payout_key_withdrawal_elsewhere_is_refused() {
    let other = SecretKey::from_secret_bytes([3; 32]).unwrap();
    for pick in [2, 3] {
        refused_before_the_secret(
            pick,
            |c| c.t.output_script = Some(p2pkh(&other)),
            path_not_allowed("destination"),
        );
        refused_before_the_secret(
            pick,
            |c| c.t.output_script = None,
            path_not_allowed("destination"),
        );
        // One byte more or less is another script.
        refused_before_the_secret(
            pick,
            |c| {
                let mut b = c.t.output_script.as_ref().unwrap().as_bytes().to_vec();
                b.push(0);
                c.t.output_script = Some(CoreScript::from_bytes(b));
            },
            path_not_allowed("destination"),
        );
    }
}

/// The scope's destination names the key kind: an owner key under a
/// payout-key grant, or a payout key under an owner-key grant, is refused.
#[test]
fn a_key_kind_other_than_the_grants_is_refused() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let [owner, _, payout, _] = cases(&v);
    // The owner key under the payout grant (it names a destination).
    let s = withdrawal_signer(&v, &payout);
    let err = owner.sign(&s).unwrap_err();
    assert!(path_not_allowed("destination")(&err), "{err:?}");
    payout.sign(&s).unwrap();
    // The payout key under the owner grant (it names none).
    let s = withdrawal_signer(&v, &owner);
    let err = payout.sign(&s).unwrap_err();
    assert!(path_not_allowed("destination")(&err), "{err:?}");
    owner.sign(&s).unwrap();
}

#[test]
fn a_signed_transition_is_refused() {
    for pick in ALL {
        refused_before_the_secret(
            pick,
            |c| c.t.signature = BinaryData::new(vec![0; 65]),
            path_not_allowed("already signed"),
        );
    }
}

/// A key path the scope refuses (an operator key, a CoinJoin key) is
/// refused before the signer is spent.
#[test]
fn a_payout_key_outside_the_address_paths_is_refused() {
    for p in ["m/9'/1'/3'/3'/0", "m/9'/1'/4'/0'/0/1", "m/9'/1'/3'/1'/0"] {
        refused_before_the_secret(
            2,
            |c| {
                c.key = WithdrawalKey::WalletPayout { path: path(p) };
            },
            path_not_allowed(""),
        );
    }
}

// MARK: The key check with the secret (P3)

/// A key whose hash160 is not the identity key's data signs nothing, and
/// the signer stays spent.
#[test]
fn a_key_that_is_not_the_identity_key_signs_nothing_and_spends_the_signer() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    for valid in cases(&v) {
        let mut bad = valid.clone();
        let k = v0(&mut bad.identity_key);
        k.data = hash160_of(&SecretKey::from_secret_bytes([3; 32]).unwrap())
            .to_vec()
            .into();
        let s = withdrawal_signer(&v, &valid);
        let err = bad.sign(&s).unwrap_err();
        assert!(key_mismatch("not the identity key")(&err), "{err:?}");
        assert_eq!(
            valid.sign(&s),
            Err(SignerError::Vault(VaultError::GrantInvalid))
        );
    }
    // A keychain owner key at another index is another key.
    let mut c = cases(&v)[0].clone();
    c.key = WithdrawalKey::KeychainOwner { index: 3 };
    let err = c.sign(&withdrawal_signer(&v, &c)).unwrap_err();
    assert!(key_mismatch("not the identity key")(&err), "{err:?}");
}

/// A masternode with no key attached for the role: `NoSecret`, after the
/// checks (the signer is spent).
#[test]
fn a_missing_attached_key_is_no_secret() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    v.delete_masternode_keys(&PRO_TX, Some("payout")).unwrap();
    let c = cases(&v)[3].clone();
    let s = withdrawal_signer(&v, &c);
    assert_eq!(c.sign(&s), Err(SignerError::NoSecret));
    assert_eq!(
        c.sign(&s),
        Err(SignerError::Vault(VaultError::GrantInvalid))
    );
}

// MARK: Single use (P3, P12, r2 m5)

#[test]
fn a_signer_signs_once_its_clones_included() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    for c in cases(&v) {
        let s = withdrawal_signer(&v, &c);
        let clone = s.clone();
        c.sign(&s).unwrap();
        for signer in [&s, &clone] {
            assert_eq!(
                c.sign(signer),
                Err(SignerError::Vault(VaultError::GrantInvalid))
            );
        }
        // Through a clone first: the original is spent too.
        let s = withdrawal_signer(&v, &c);
        let clone = s.clone();
        c.sign(&clone).unwrap();
        assert_eq!(
            c.sign(&s),
            Err(SignerError::Vault(VaultError::GrantInvalid))
        );
        // Another key kind under the spent signer is refused as well.
        let other = cases(&v)[0].clone();
        assert!(other.sign(&s).is_err());
    }
}

/// Two threads racing one signer: exactly one signs.
#[test]
fn racing_clones_sign_once() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let c = cases(&v)[0].clone();
    for _ in 0..8 {
        let s = withdrawal_signer(&v, &c);
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let s = s.clone();
                    let c = c.clone();
                    scope.spawn(move || c.sign(&s))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    }
}

// MARK: Nothing else under the scope (P3)

/// Under the withdrawal scope, no digest, message, public key, extended
/// public key or owner payload, with either key kind; none of them spends
/// the signer.
#[tokio::test]
async fn nothing_else_signs_under_the_withdrawal_scope() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let not_allowed = |r: Result<(), SignerError>| {
        assert!(matches!(r, Err(SignerError::PathNotAllowed(_))), "{r:?}");
    };
    for c in cases(&v) {
        let s = withdrawal_signer(&v, &c);
        for p in ["m/9'/1'/3'/2'/2", PAYOUT_PATH, "m/0'/1/3"] {
            let p = path(p);
            not_allowed(s.sign_ecdsa(&p, [3; 32]).await.map(drop));
            not_allowed(s.sign_message(&p, b"hello").await.map(drop));
            not_allowed(s.public_key(&p).await.map(drop));
            not_allowed(s.extended_public_key(&p).await.map(drop));
        }
        let payload = ProviderUpdateRegistrarPayload {
            version: 2,
            pro_tx_hash: dashcore::Txid::from_byte_array(PRO_TX),
            provider_mode: 0,
            operator_public_key: [0x8b; 48].into(),
            voting_key_hash: dashcore::PubkeyHash::from_byte_array([0x33; 20]),
            script_payout: dashcore::ScriptBuf::new(),
            inputs_hash: dashcore::hash_types::InputsHash::from_byte_array([0x55; 32]),
            payload_sig: Vec::new(),
            payouts: None,
        };
        for key in [
            OwnerKey::Keychain { index: 2 },
            OwnerKey::Attached {
                pro_tx_hash: PRO_TX,
            },
        ] {
            not_allowed(s.sign_update_registrar(&key, &payload).map(drop));
        }
        // Still unspent.
        c.sign(&s).unwrap();
    }
}

/// Every other scope refuses the withdrawal, `Full` and `MasternodeOwner`
/// included, with every key kind.
#[test]
fn other_scopes_refuse_the_withdrawal() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let w = wallet(1);
    let signers = [
        v.signer(
            &w,
            &token(&v, GrantPurpose::Spend { max_duffs: 1 }, Credential::None),
        )
        .unwrap(),
        v.masternode_signer(
            &w,
            &token(
                &v,
                GrantPurpose::MasternodeOp { max_duffs: 1 },
                Credential::None,
            ),
        )
        .unwrap(),
        v.platform_signer(
            &w,
            &token(
                &v,
                GrantPurpose::PlatformOp {
                    max_duffs: 1,
                    max_credits: MAX,
                },
                Credential::None,
            ),
            SignerScope::PlatformIdentity,
        )
        .unwrap(),
        v.mixing_signer(&w).unwrap(),
    ];
    for s in &signers {
        for c in cases(&v) {
            let err = c.sign(s).unwrap_err();
            assert!(path_not_allowed("signer")(&err), "{:?} {err:?}", s.scope());
        }
    }
}
