//! DP5-02 D10: the IdentityCreate transition an invitation claim submits is
//! Platform's own, byte for byte, at the pin.
//!
//! The library's claim (`rs-platform-wallet` `claim_invitation`) puts a
//! placeholder identity of the engine's keys through the SDK's
//! `broadcast_request_for_new_identity_with_private_key`, which builds the
//! transition with `IdentityCreateTransition::
//! try_from_identity_with_signer_and_private_key` (the voucher key signs the
//! outer signature, the identity signer each key's witness; no settings, so
//! no fee increase). Here that construction runs twice for one voucher key
//! and proof: with the engine's vault signer and DP1-01's keys, as the claim
//! passes them, and with keys derived independently from the seed and a
//! plain key signer, as Platform's `simple-signer` signs. The bytes are equal.
//! This holds the engine's signer and keys to Platform's; the library's own
//! call path (its settings, the SDK's version) ran on the devnet (T2).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::secp256k1::{PublicKey, SecretKey};
use dashcore::transaction::special_transaction::TransactionPayload;
use dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxOut};
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::state_transition::asset_lock_proof::InstantAssetLockProof;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::platform_value::BinaryData;
use dpp::prelude::{AssetLockProof, Identifier};
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_create_transition::IdentityCreateTransition;
use dpp::state_transition::identity_create_transition::accessors::IdentityCreateTransitionAccessorsV0;
use dpp::state_transition::identity_create_transition::methods::IdentityCreateTransitionMethodsV0;
use dpp::version::PlatformVersion;
use dw_engine::platform::{VaultIdentitySigner, keys_policy};
use dw_vault::{
    Credential, GrantKind, GrantPurpose, KdfPolicy, MemoryOsStore, SignerScope, SystemClock, Vault,
    VaultConfig,
};
use key_wallet::Network;
use key_wallet::bip32::ExtendedPrivKey;

const WALLET: [u8; 32] = [5; 32];
const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const NETWORK: Network = Network::Regtest;
const INDEX: u32 = 0;
const VOUCHER: [u8; 32] = [0x11; 32];

/// Keys by their public bytes, signing as Platform's `SimpleSigner` does
/// (`dashcore::signer::sign`, a P2PKH witness of the same signature).
#[derive(Debug)]
struct PlainKeys(BTreeMap<Vec<u8>, [u8; 32]>);

#[async_trait]
impl Signer<IdentityPublicKey> for PlainKeys {
    async fn sign(
        &self,
        key: &IdentityPublicKey,
        data: &[u8],
    ) -> Result<BinaryData, ProtocolError> {
        let secret = (self.0.get(key.data().as_slice()))
            .ok_or_else(|| ProtocolError::Generic("not a key of this signer".into()))?;
        Ok(dashcore::signer::sign(data, secret)?.to_vec().into())
    }

    async fn sign_create_witness(
        &self,
        key: &IdentityPublicKey,
        data: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        Ok(AddressWitness::P2pkh {
            signature: self.sign(key, data).await?,
        })
    }

    fn can_sign_with(&self, key: &IdentityPublicKey) -> bool {
        self.0.contains_key(key.data().as_slice())
    }
}

/// The engine's identity signer over an unencrypted vault holding
/// [`PHRASE`], and the vault's directory, as DP1-01's test makes it.
fn vault_signer() -> (tempfile::TempDir, dw_vault::VaultSigner) {
    let dir = dw_testutil::private_tempdir();
    let config = VaultConfig {
        kdf: KdfPolicy::Calibrated,
        os_store: Arc::new(MemoryOsStore::new()),
        clock: Arc::new(SystemClock),
        grant_ttl_secs: 120,
    };
    let v = Vault::open(dir.path().join("vault"), NETWORK, "dp5-02", config).unwrap();
    v.create(None).unwrap();
    let secret = dw_vault::mnemonic::derive_secret(PHRASE.as_bytes(), b"", false).unwrap();
    v.store_wallet_secret(&WALLET, &secret).unwrap();
    let grant = v
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: 1,
            },
            Some(&WALLET),
            Credential::None,
        )
        .unwrap();
    let token = v
        .redeem_grant(&grant.id, GrantKind::PlatformOp, Some(&WALLET))
        .unwrap();
    let signer = v
        .platform_signer(&WALLET, &token, SignerScope::PlatformIdentity)
        .unwrap();
    (dir, signer)
}

/// The private keys of `keys`, derived from the phrase's seed at DP1-01's
/// paths without the vault, checked against the keys' public bytes.
fn plain_keys(keys: &BTreeMap<u32, IdentityPublicKey>) -> PlainKeys {
    let seed = dw_vault::mnemonic::derive_secret(PHRASE.as_bytes(), b"", false)
        .unwrap()
        .seed;
    let master = ExtendedPrivKey::new_master(NETWORK, &seed[..]).unwrap();
    let mut plain = BTreeMap::new();
    for (id, key) in keys {
        let path = keys_policy::key_path(NETWORK, INDEX, *id).unwrap();
        let secret = master.derive_priv(&path).unwrap().private_key;
        let public = PublicKey::from_secret_key(&secret).serialize();
        assert_eq!(key.data().as_slice(), public.as_slice(), "key {id}");
        plain.insert(public.to_vec(), secret.to_secret_bytes());
    }
    PlainKeys(plain)
}

fn funding_tx() -> Transaction {
    let voucher = SecretKey::from_secret_bytes(VOUCHER).unwrap();
    let public = dashcore::PublicKey::new(PublicKey::from_secret_key(&voucher));
    Transaction {
        version: 3,
        lock_time: 0,
        input: vec![],
        output: vec![TxOut {
            value: 3_000_000,
            script_pubkey: ScriptBuf::new_op_return(&[]),
        }],
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: vec![TxOut {
                    value: 3_000_000,
                    script_pubkey: ScriptBuf::new_p2pkh(&public.pubkey_hash()),
                }],
            },
        )),
    }
}

/// The two proofs a claim submits: the link's InstantSend proof, and the
/// ChainLock one it falls back to.
fn proofs() -> [AssetLockProof; 2] {
    let tx = funding_tx();
    let txid = tx.txid();
    [
        AssetLockProof::Instant(InstantAssetLockProof::new(
            InstantLock {
                txid,
                ..InstantLock::default()
            },
            tx,
            0,
        )),
        AssetLockProof::Chain(ChainAssetLockProof::new(
            1_500,
            OutPoint::new(txid, 0).into(),
        )),
    ]
}

/// The SDK's construction (`broadcast_identity.rs`) for the claim's
/// placeholder identity.
async fn transition<S: Signer<IdentityPublicKey>>(
    keys: &BTreeMap<u32, IdentityPublicKey>,
    proof: AssetLockProof,
    signer: &S,
) -> StateTransition {
    let placeholder = Identity::V0(IdentityV0 {
        id: Identifier::default(),
        public_keys: keys.clone(),
        balance: 0,
        revision: 0,
    });
    IdentityCreateTransition::try_from_identity_with_signer_and_private_key(
        &placeholder,
        proof,
        &VOUCHER,
        signer,
        &dpp::native_bls::NativeBlsModule,
        0,
        PlatformVersion::latest(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn the_claims_transition_is_platforms_own() {
    let (_dir, vault) = vault_signer();
    let keys = keys_policy::derive_keys(&vault, INDEX, &keys_policy::registration_specs())
        .await
        .unwrap();
    assert_eq!(keys.len(), 6, "DP1-01's registration keys");
    let engine = VaultIdentitySigner::new(vault, [INDEX]).unwrap();
    let plain = plain_keys(&keys);
    for proof in proofs() {
        let id = proof.create_identifier().unwrap();
        let ours = transition(&keys, proof.clone(), &engine).await;
        let platforms = transition(&keys, proof, &plain).await;
        assert_eq!(
            ours.serialize_to_bytes().unwrap(),
            platforms.serialize_to_bytes().unwrap()
        );
        let StateTransition::IdentityCreate(create) = &ours else {
            panic!("{ours:?}");
        };
        // The id the claim's status and adoption derive.
        assert_eq!(create.identity_id(), id);
        assert_eq!(create.public_keys().len(), keys.len());
    }
}
