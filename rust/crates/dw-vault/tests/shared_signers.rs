//! MG-04b (docs/design/MG-04b-shared-masternodes.md §4, §6.2, §12 "T0,
//! dw-vault"): the shared masternode's owner signers. Each signs only
//! under `MasternodeOwner` at a keychain owner path, hashes the bytes it
//! is given (no digest entry point), and `sign_dissolution` refuses a
//! signature count other than 1 or the share count, and a count of 1 for
//! another share's `actorIndex` (mutant 17).
//!
//! The transactions are dashd's (testdata/shared_registration.json,
//! testdata/shared_maintenance.json, dashd v24.0.0-rc.3). dashd's owner keys
//! can't be derived from a seed, so each test pins the digest twice: dashd's
//! own signature on the vector is `SignCompact` of the digest the vault
//! computes (RFC 6979 makes it comparable byte for byte), and the vault's
//! signature is `SignCompact` of that digest with the wallet's keychain
//! owner key, which `VerifyHashCanonical` accepts for the key's id. Where
//! the vault must own a share, the vector's share table names the wallet's
//! key instead.

mod common;

use std::str::FromStr;

use common::*;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dw_protx::payloads::sign_hash_compact;
use dw_protx::shared::rules::verify_hash_canonical;
use dw_protx::shared::{
    ProDisTx, ProRegTxV3, ProUpShareTx, ProUpSharedRegTx, SpecialTx, payload_bytes,
};
use dw_vault::{
    Credential, GrantPurpose, GrantToken, SignerError, SignerScope, UnlockScope, Vault, VaultError,
    VaultSigner,
};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

const MN_OP: GrantPurpose = GrantPurpose::MasternodeOp { max_duffs: 1_000 };

/// An encrypted vault holding wallet 1, unlocked.
fn encrypted(fx: &Fixture) -> Vault {
    let v = fx.open();
    v.create(Some(PASS)).unwrap();
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

/// A redeemed grant of `purpose` for wallet 1.
fn token(v: &Vault, purpose: GrantPurpose) -> GrantToken {
    let grant = v
        .authorize(purpose, Some(&wallet(1)), Credential::None)
        .unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&wallet(1)))
        .unwrap()
}

fn owner_signer(v: &Vault) -> VaultSigner {
    v.masternode_signer(&wallet(1), &token(v, MN_OP)).unwrap()
}

fn hash160_of(secret: &SecretKey) -> [u8; 20] {
    hash160::Hash::hash(&PublicKey::from_secret_key(secret).serialize()).to_byte_array()
}

/// Wallet 1's keychain owner key `index`, derived here.
fn own_secret(v: &Vault, index: u32) -> SecretKey {
    let master = ExtendedPrivKey::new_master(v.network(), &[1; 64]).unwrap();
    let path = DerivationPath::from_str(&format!("m/9'/1'/3'/2'/{index}")).unwrap();
    master.derive_priv(&path).unwrap().private_key
}

fn own_id(v: &Vault, index: u32) -> [u8; 20] {
    hash160_of(&own_secret(v, index))
}

/// `sig` is `SignCompact(digest)` by `secret`, canonical for its key id.
fn signed_by(sig: &[u8; 65], digest: [u8; 32], secret: &SecretKey) {
    assert_eq!(
        *sig,
        sign_hash_compact(&secret.to_secret_bytes(), &digest).unwrap()
    );
    assert!(verify_hash_canonical(digest, hash160_of(secret), sig));
}

fn invalid<T: std::fmt::Debug>(r: Result<T, SignerError>) -> bool {
    matches!(r, Err(SignerError::Vault(VaultError::InvalidArgument(_))))
}

fn mismatch<T: std::fmt::Debug>(r: Result<T, SignerError>) -> bool {
    matches!(r, Err(SignerError::KeyMismatch(_)))
}

fn not_allowed<T: std::fmt::Debug>(r: Result<T, SignerError>) -> bool {
    matches!(r, Err(SignerError::PathNotAllowed(_)))
}

// MARK: dashd's vectors

fn load(name: &str) -> serde_json::Value {
    let path = format!("{}/../../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(doc["source"].as_str().unwrap().contains("v24.0.0-rc.3"));
    doc
}

fn tx(hex_str: &serde_json::Value) -> SpecialTx {
    SpecialTx::from_bytes(&hex::decode(hex_str.as_str().unwrap()).unwrap()).unwrap()
}

fn registration_payload(tx: &SpecialTx) -> ProRegTxV3 {
    ProRegTxV3::from_payload(&tx.payload).unwrap()
}

/// `tx` with each `(share, key id)` of `owners` as that share's owner key.
fn owned_by(tx: &SpecialTx, owners: &[(usize, [u8; 20])]) -> SpecialTx {
    let mut payload = registration_payload(tx);
    for (share, key_id) in owners {
        payload.shares[*share].owner_key_id = *key_id;
    }
    let mut tx = tx.clone();
    tx.payload = payload.to_payload().unwrap();
    tx
}

/// The owner secrets of the vector's parties (their WIFs), by key id.
fn dashd_owner(case: &serde_json::Value, key_id: [u8; 20]) -> SecretKey {
    case["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            dashcore::PrivateKey::from_wif(p["owner_wif"].as_str().unwrap())
                .unwrap()
                .inner
        })
        .find(|s| hash160_of(s) == key_id)
        .expect("a party owns the share")
}

/// The registration case of the masternode `pro_tx_hash` and its signed
/// transaction.
fn registration_of(pro_tx_hash: dashcore::Txid) -> (serde_json::Value, SpecialTx) {
    load("shared_registration.json")["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c.clone(), tx(&c["tx_hex"])))
        .find(|(_, tx)| tx.txid() == pro_tx_hash)
        .expect("a registration vector")
}

/// The masternode most maintenance vectors act on.
fn masternode() -> (serde_json::Value, SpecialTx) {
    let doc = load("shared_maintenance.json");
    registration_of(doc["pro_tx_hash"].as_str().unwrap().parse().unwrap())
}

fn maintenance(kind: &str) -> Vec<serde_json::Value> {
    load("shared_maintenance.json")["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == kind)
        .cloned()
        .collect()
}

fn dissolution(tx: &SpecialTx) -> ProDisTx {
    dashcore::consensus::deserialize(&tx.payload).unwrap()
}

/// `MakeSignHash(tx, sig_count)` of the ProDisTx `tx`.
fn dissolve_hash(tx: &SpecialTx, sig_count: u8) -> [u8; 32] {
    dissolution(tx).sign_hash(
        tx.version,
        tx.lock_time,
        &tx.prevouts(),
        &tx.output,
        sig_count,
    )
}

/// The ProDisTx `tx` re-pointed at `registration`.
fn dissolving(tx: &SpecialTx, registration: &SpecialTx) -> SpecialTx {
    let mut payload = dissolution(tx);
    payload.pro_tx_hash = registration.txid();
    let mut tx = tx.clone();
    tx.payload = payload_bytes(&payload);
    tx
}

// MARK: Registration consent

#[test]
fn consent_signs_the_consent_hash_for_a_share_it_owns() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    for case in load("shared_registration.json")["cases"]
        .as_array()
        .unwrap()
    {
        let combined = tx(&case["combined_tx_hex"]);
        let shares = registration_payload(&combined);
        // dashd's join signatures are `SignCompact(MakeSharedRegConsentHash)`.
        let digest = shares.consent_hash(&combined);
        let mut shown = digest;
        shown.reverse();
        assert_eq!(hex::encode(shown), case["prepare"]["consentHash"]);
        for (share, sig) in shares.shares.iter().zip(&shares.join_sigs) {
            signed_by(sig, digest, &dashd_owner(case, share.owner_key_id));
        }

        for (index, share) in [(0, 0), (19, shares.shares.len() - 1)] {
            let prepared = owned_by(&tx(&case["prepare"]["tx"]), &[(share, own_id(&v, index))]);
            let combined = owned_by(&combined, &[(share, own_id(&v, index))]);
            let digest = registration_payload(&combined).consent_hash(&combined);
            let sig = s.sign_shared_consent(index, &combined.to_bytes()).unwrap();
            signed_by(&sig, digest, &own_secret(&v, index));
            // Neither the join signatures nor the scriptSigs are in the
            // digest: the prepared transaction signs the same.
            assert_eq!(
                s.sign_shared_consent(index, &prepared.to_bytes()).unwrap(),
                sig
            );
        }
    }
}

#[test]
fn consent_refuses_a_registration_without_its_share() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let (_, registration) = masternode();
    assert!(mismatch(s.sign_shared_consent(0, &registration.to_bytes())));
    // Share 1 is key 1's, not key 0's.
    let other = owned_by(&registration, &[(1, own_id(&v, 1))]);
    assert!(mismatch(s.sign_shared_consent(0, &other.to_bytes())));
    s.sign_shared_consent(1, &other.to_bytes()).unwrap();
}

#[test]
fn consent_refuses_what_is_no_shared_registration() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let (_, registration) = masternode();
    let mine = owned_by(&registration, &[(0, own_id(&v, 0))]);
    let bytes = mine.to_bytes();
    s.sign_shared_consent(0, &bytes).unwrap();

    let mut trailing = bytes.clone();
    trailing.push(0);
    let mut other_type = mine.clone();
    other_type.tx_type = 10;
    let mut not_shared = registration_payload(&mine);
    not_shared.shares.clear();
    not_shared.join_sigs.clear();
    let mut unshared = mine.clone();
    unshared.payload = not_shared.to_payload().unwrap();
    let mut payload_trailing = mine.clone();
    payload_trailing.payload.push(0);
    for refused in [
        Vec::new(),
        vec![0; 10],
        bytes[..bytes.len() - 1].to_vec(),
        trailing,
        other_type.to_bytes(),
        unshared.to_bytes(),
        payload_trailing.to_bytes(),
        // A dissolution is no registration.
        tx(&maintenance("dissolve_unilateral")[0]["tx_hex"]).to_bytes(),
    ] {
        assert!(invalid(s.sign_shared_consent(0, &refused)));
    }
}

// MARK: ProUpShareTx and ProUpSharedRegTx

#[test]
fn update_share_signs_the_payload_hash() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let (case, registration) = masternode();
    // The attached owner key stays ProUpRegTx-only: the shared signers
    // reach keychain owner keys alone, whatever is attached.
    let owner = dashd_owner(
        &case,
        registration_payload(&registration).shares[0].owner_key_id,
    );
    v.store_masternode_key(
        &registration.txid().to_byte_array(),
        "owner",
        &owner.to_secret_bytes(),
    )
    .unwrap();
    let s = owner_signer(&v);
    let cases = maintenance("update_share");
    assert_eq!(cases.len(), 2);
    for c in cases {
        let payload: ProUpShareTx =
            dashcore::consensus::deserialize(&tx(&c["tx_hex"]).payload).unwrap();
        assert_eq!(payload.pro_tx_hash, registration.txid());
        let digest = payload.sign_hash();
        let share = &registration_payload(&registration).shares[payload.share_index as usize];
        signed_by(
            &payload.sig.clone().try_into().unwrap(),
            digest,
            &dashd_owner(&case, share.owner_key_id),
        );
        for index in [0, 19] {
            let sig = s
                .sign_update_share(index, &payload_bytes(&payload))
                .unwrap();
            signed_by(&sig, digest, &own_secret(&v, index));
            // The hash leaves the signature out.
            let mut unsigned = payload.clone();
            unsigned.sig.clear();
            assert_eq!(
                s.sign_update_share(index, &payload_bytes(&unsigned))
                    .unwrap(),
                sig
            );
            let mut other = payload.clone();
            other.inputs_hash[0] ^= 1;
            assert_ne!(
                s.sign_update_share(index, &payload_bytes(&other)).unwrap(),
                sig
            );
        }
    }
}

#[test]
fn update_shared_registrar_signs_the_payload_hash() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let (case, registration) = masternode();
    let cases = maintenance("update_registrar");
    assert_eq!(cases.len(), 1);
    let payload: ProUpSharedRegTx =
        dashcore::consensus::deserialize(&tx(&cases[0]["tx_hex"]).payload).unwrap();
    assert_eq!(payload.pro_tx_hash, registration.txid());
    let digest = payload.sign_hash();
    let shares = registration_payload(&registration).shares;
    assert_eq!(payload.sigs.len(), shares.len());
    for (share, sig) in shares.iter().zip(&payload.sigs) {
        signed_by(sig, digest, &dashd_owner(&case, share.owner_key_id));
    }
    for index in [0, 19] {
        let sig = s
            .sign_update_shared_registrar(index, &payload_bytes(&payload))
            .unwrap();
        signed_by(&sig, digest, &own_secret(&v, index));
        // The count and signatures are not in the hash.
        let mut unsigned = payload.clone();
        unsigned.sigs.clear();
        assert_eq!(
            s.sign_update_shared_registrar(index, &payload_bytes(&unsigned))
                .unwrap(),
            sig
        );
        let mut other = payload.clone();
        other.voting_key_id[0] ^= 1;
        assert_ne!(
            s.sign_update_shared_registrar(index, &payload_bytes(&other))
                .unwrap(),
            sig
        );
    }
}

#[test]
fn update_signers_refuse_malformed_payloads() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let share = tx(&maintenance("update_share")[0]["tx_hex"]);
    let registrar = tx(&maintenance("update_registrar")[0]["tx_hex"]);
    s.sign_update_share(0, &share.payload).unwrap();
    s.sign_update_shared_registrar(0, &registrar.payload)
        .unwrap();
    for (payload, sign) in [
        (
            &share.payload,
            VaultSigner::sign_update_share as fn(&VaultSigner, u32, &[u8]) -> _,
        ),
        (
            &registrar.payload,
            VaultSigner::sign_update_shared_registrar,
        ),
    ] {
        let mut trailing = payload.clone();
        trailing.push(0);
        for refused in [
            Vec::new(),
            payload[..payload.len() - 1].to_vec(),
            trailing,
            // The whole transaction is no payload.
            share.to_bytes(),
        ] {
            assert!(invalid(sign(&s, 0, &refused)));
        }
    }
}

// MARK: ProDisTx (mutant 17)

#[test]
fn a_unilateral_dissolution_is_signed_by_the_actors_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let cases = maintenance("dissolve_unilateral");
    assert!(cases.len() >= 2);
    for c in cases {
        let dashd = tx(&c["tx_hex"]);
        let (case, registration) = registration_of(dissolution(&dashd).pro_tx_hash);
        let shares = registration_payload(&registration).shares;
        let actor = usize::from(dissolution(&dashd).actor_index);
        assert_eq!(c["actor_index"], actor);
        // dashd's signature: `SignCompact(MakeSignHash(tx, 1))` by the
        // actor share's key.
        signed_by(
            &dissolution(&dashd).sigs[0],
            dissolve_hash(&dashd, 1),
            &dashd_owner(&case, shares[actor].owner_key_id),
        );

        let mine = owned_by(&registration, &[(actor, own_id(&v, 3))]);
        let ours = dissolving(&dashd, &mine);
        let sig = s
            .sign_dissolution(3, &ours.to_bytes(), &mine.to_bytes(), 1)
            .unwrap();
        signed_by(&sig, dissolve_hash(&ours, 1), &own_secret(&v, 3));

        // Mutant 17: a count of 1 for another share's `actorIndex`.
        let other = (actor + 1) % shares.len();
        let not_actor = owned_by(&registration, &[(other, own_id(&v, 3))]);
        let ours = dissolving(&dashd, &not_actor);
        assert!(mismatch(s.sign_dissolution(
            3,
            &ours.to_bytes(),
            &not_actor.to_bytes(),
            1
        )));
    }
}

#[test]
fn a_unanimous_dissolution_is_signed_by_any_owners_key() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let cases = maintenance("dissolve_unanimous");
    assert_eq!(cases.len(), 1);
    let dashd = tx(&cases[0]["tx_hex"]);
    let (case, registration) = registration_of(dissolution(&dashd).pro_tx_hash);
    let shares = registration_payload(&registration).shares;
    let n = u8::try_from(shares.len()).unwrap();
    let sigs = dissolution(&dashd).sigs;
    assert_eq!(sigs.len(), shares.len());
    for (share, sig) in shares.iter().zip(&sigs) {
        signed_by(
            sig,
            dissolve_hash(&dashd, n),
            &dashd_owner(&case, share.owner_key_id),
        );
    }
    let actor = usize::from(dissolution(&dashd).actor_index);
    // Any share, the actor's or not.
    for share in 0..shares.len() {
        let mine = owned_by(&registration, &[(share, own_id(&v, 0))]);
        let ours = dissolving(&dashd, &mine);
        let sig = s
            .sign_dissolution(0, &ours.to_bytes(), &mine.to_bytes(), n)
            .unwrap();
        signed_by(&sig, dissolve_hash(&ours, n), &own_secret(&v, 0));
        if share != actor {
            assert!(mismatch(s.sign_dissolution(
                0,
                &ours.to_bytes(),
                &mine.to_bytes(),
                1
            )));
        }
    }
    // A key of no share signs neither.
    let ours = dissolving(&dashd, &registration);
    for count in [1, n] {
        assert!(mismatch(s.sign_dissolution(
            0,
            &ours.to_bytes(),
            &registration.to_bytes(),
            count
        )));
    }
}

/// Mutant 17: `sig_count` is 1 or the share count, nothing else, whichever
/// share the key owns.
#[test]
fn a_dissolution_refuses_other_signature_counts() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let dashd = tx(&maintenance("dissolve_unanimous")[0]["tx_hex"]);
    let (_, registration) = registration_of(dissolution(&dashd).pro_tx_hash);
    let n = registration_payload(&registration).shares.len();
    assert_eq!(n, 3);
    let actor = usize::from(dissolution(&dashd).actor_index);
    let mine = owned_by(&registration, &[(actor, own_id(&v, 0))]);
    let ours = dissolving(&dashd, &mine).to_bytes();
    let mine = mine.to_bytes();
    s.sign_dissolution(0, &ours, &mine, 1).unwrap();
    s.sign_dissolution(0, &ours, &mine, 3).unwrap();
    for count in [0, 2, 4, 8, 255] {
        assert!(
            invalid(s.sign_dissolution(0, &ours, &mine, count)),
            "{count}"
        );
    }
}

#[test]
fn a_dissolution_refuses_malformed_bytes_and_another_registration() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let dashd = tx(&maintenance("dissolve_unilateral")[0]["tx_hex"]);
    let (_, registration) = registration_of(dissolution(&dashd).pro_tx_hash);
    let actor = usize::from(dissolution(&dashd).actor_index);
    let mine = owned_by(&registration, &[(actor, own_id(&v, 0))]);
    let ours = dissolving(&dashd, &mine);
    let (ours_bytes, mine_bytes) = (ours.to_bytes(), mine.to_bytes());
    s.sign_dissolution(0, &ours_bytes, &mine_bytes, 1).unwrap();

    let mut trailing = ours_bytes.clone();
    trailing.push(0);
    let mut other_type = ours.clone();
    other_type.tx_type = 11;
    let mut payload_trailing = ours.clone();
    payload_trailing.payload.push(0);
    let mut past_table = dissolution(&ours);
    past_table.actor_index = 3;
    let mut past = ours.clone();
    past.payload = payload_bytes(&past_table);
    for refused in [
        Vec::new(),
        ours_bytes[..ours_bytes.len() - 1].to_vec(),
        trailing,
        other_type.to_bytes(),
        payload_trailing.to_bytes(),
        past.to_bytes(),
        // dashd's own transaction names the unaltered registration.
        dashd.to_bytes(),
        mine_bytes.clone(),
    ] {
        assert!(invalid(s.sign_dissolution(0, &refused, &mine_bytes, 1)));
    }
    // The registration: none, not a shared one, or another masternode's.
    let mut unshared = registration_payload(&mine);
    unshared.shares.clear();
    unshared.join_sigs.clear();
    let mut not_shared = mine.clone();
    not_shared.payload = unshared.to_payload().unwrap();
    let other = owned_by(&mine, &[(actor, own_id(&v, 0)), ((actor + 1) % 3, [9; 20])]);
    for refused in [
        Vec::new(),
        mine_bytes[..mine_bytes.len() - 1].to_vec(),
        not_shared.to_bytes(),
        ours_bytes.clone(),
        other.to_bytes(),
    ] {
        assert!(invalid(s.sign_dissolution(0, &ours_bytes, &refused, 1)));
    }
}

// MARK: Scopes and paths (D11)

/// Valid inputs for each of the four signers, owned by keychain owner
/// key 0.
struct Inputs {
    consent: Vec<u8>,
    share: Vec<u8>,
    registrar: Vec<u8>,
    dissolve: Vec<u8>,
    registration: Vec<u8>,
}

impl Inputs {
    fn new(v: &Vault) -> Self {
        let dashd = tx(&maintenance("dissolve_unilateral")[0]["tx_hex"]);
        let (_, registration) = registration_of(dissolution(&dashd).pro_tx_hash);
        let actor = usize::from(dissolution(&dashd).actor_index);
        let mine = owned_by(&registration, &[(actor, own_id(v, 0))]);
        Self {
            consent: mine.to_bytes(),
            share: tx(&maintenance("update_share")[0]["tx_hex"]).payload,
            registrar: tx(&maintenance("update_registrar")[0]["tx_hex"]).payload,
            dissolve: dissolving(&dashd, &mine).to_bytes(),
            registration: mine.to_bytes(),
        }
    }

    /// Every signer's result at keychain owner key `index`.
    fn sign(&self, s: &VaultSigner, index: u32) -> [Result<[u8; 65], SignerError>; 4] {
        [
            s.sign_shared_consent(index, &self.consent),
            s.sign_update_share(index, &self.share),
            s.sign_update_shared_registrar(index, &self.registrar),
            s.sign_dissolution(index, &self.dissolve, &self.registration, 1),
        ]
    }
}

/// D11: only a `MasternodeOp` grant's signer makes a shared owner
/// signature; every other scope a test can obtain is refused before a key
/// is read (`MasternodeVoting`'s, which only a `VoteSigner` holds, is
/// `SignerScope::allows`' unit test).
#[test]
fn only_the_masternode_owner_scope_signs_shared_payloads() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let inputs = Inputs::new(&v);
    for r in inputs.sign(&owner_signer(&v), 0) {
        r.unwrap();
    }
    let w = wallet(1);
    let platform = GrantPurpose::PlatformOp {
        max_duffs: 10,
        max_credits: 10,
    };
    let mut others = vec![
        v.signer(&w, &token(&v, GrantPurpose::Spend { max_duffs: 1 }))
            .unwrap(),
        v.signer(&w, &token(&v, GrantPurpose::SignMessage)).unwrap(),
        v.mixing_signer(&w).unwrap(),
        v.mixing_funding_signer(&w).unwrap(),
        v.dashpay_crypto_signer(&w).unwrap(),
    ];
    for scope in [
        SignerScope::PlatformIdentity,
        SignerScope::DashPayCrypto,
        SignerScope::PlatformFunding { max_duffs: 1 },
    ] {
        others.push(v.platform_signer(&w, &token(&v, platform), scope).unwrap());
    }
    for s in others {
        assert_ne!(s.scope(), SignerScope::MasternodeOwner);
        for r in inputs.sign(&s, 0) {
            assert!(not_allowed(r), "{:?}", s.scope());
        }
    }
}

/// A hardened index is no owner key path; and the signers take a keychain
/// index only, so the attached owner key is out of their reach.
#[test]
fn the_shared_signers_refuse_a_non_owner_path() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    for r in Inputs::new(&v).sign(&s, 1 << 31) {
        assert!(not_allowed(r));
    }
}

#[test]
fn a_lock_ends_the_shared_signers() {
    let fx = Fixture::new();
    let v = encrypted(&fx);
    let s = owner_signer(&v);
    let inputs = Inputs::new(&v);
    for r in inputs.sign(&s, 0) {
        r.unwrap();
    }
    v.lock();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    for r in inputs.sign(&s, 0) {
        assert_eq!(r, Err(SignerError::Locked));
    }
}
