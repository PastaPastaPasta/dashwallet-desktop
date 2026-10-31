//! A masternode's Platform credit withdrawal (MG-13 P3).
//!
//! A masternode's Platform rewards accrue on its owner identity, whose id is
//! the proTxHash in Platform's order: Core's display order, the reverse of
//! the transaction's wire bytes (Drive `get_owner_identifier`,
//! `rs-drive-abci/.../update_masternode_identities/create_owner_identity/v0/mod.rs:35-39`,
//! takes `MasternodeListItem::pro_tx_hash`, a `ProTxHash` that rust-dashcore
//! parses and displays forward, `dash/src/hash_types.rs:177`; platform-wallet
//! `execute_masternode_withdrawal` reverses the wire bytes, platform
//! `dc0c9463`). Two keys can withdraw them (Drive
//! `create_owner_identity/v2`): the owner key (identity key purpose
//! `OWNER`, key id 1), to the registered payout address only, with no
//! output script; and the payout address's key (purpose `TRANSFER`, key
//! id 0), to any script.
//!
//! As with an owner payload ([`crate::OwnerKey`]) or a vote
//! ([`crate::VoteSigner`]), neither key signs a digest it is given: coins
//! sent to an owner or payout key's address would be spendable by any digest
//! signer at that path. [`VaultSigner::sign_masternode_withdrawal`] takes the
//! transition and the identity key, checks every field against the grant's
//! scope ([`SignerScope::MasternodeWithdrawal`]) before it reads the secret,
//! checks the key against the identity key with the secret, and signs
//! `StateTransition::signable_bytes()` itself, as an `ECDSA_HASH160` signer
//! does for dpp's `sign_external`: the compact recoverable signature over
//! their double SHA-256 (`dashcore::signer::sign`'s encoding). The signer
//! signs once (r2 m5).

use dashcore::hashes::{Hash, hash160, sha256, sha256d};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::{IdentityPublicKey, KeyType, Purpose};
use dpp::serialization::Signable;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::{
    IdentityCreditWithdrawalTransition, MIN_CORE_FEE_PER_BYTE,
};
use dpp::withdrawal::Pooling;
use key_wallet::bip32::DerivationPath;

use crate::SignerError;
use crate::paths::owner_key_path;
use crate::signer::{KeyUse, SignerScope, VaultSigner, sign_compact};
use crate::vault::masternode::{ROLE_OWNER, ROLE_PAYOUT};

/// The output script a payout-key withdrawal may pay, bound by its SHA-256
/// so that [`SignerScope`] and [`crate::GrantPurpose`] stay `Copy`. The
/// transition's script must hash to it exactly: the same bytes, the same
/// length.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WithdrawalDestination([u8; 32]);

impl WithdrawalDestination {
    /// The binding of the output script `script` (its raw bytes).
    pub fn of_script(script: &[u8]) -> Self {
        Self(sha256::Hash::hash(script).to_byte_array())
    }

    /// Whether `script` is the bound script.
    pub fn matches(&self, script: &[u8]) -> bool {
        *self == Self::of_script(script)
    }

    /// The SHA-256 of the bound script.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl std::fmt::Debug for WithdrawalDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WithdrawalDestination(sha256 {})", hex::encode(self.0))
    }
}

/// Which key signs a masternode withdrawal (MG-13 P2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WithdrawalKey {
    /// The wallet's keychain owner key `m/9'/coin'/3'/2'/index`.
    KeychainOwner { index: u32 },
    /// The `owner` key attached to the tracked masternode `pro_tx_hash`
    /// (wire order).
    AttachedOwner { pro_tx_hash: [u8; 32] },
    /// The wallet's key of the payout address, at the BIP44 or BIP32
    /// address `path`.
    WalletPayout { path: DerivationPath },
    /// The `payout` key attached to the tracked masternode `pro_tx_hash`
    /// (wire order).
    AttachedPayout { pro_tx_hash: [u8; 32] },
}

impl WithdrawalKey {
    /// Whether it is an owner key (identity key purpose `OWNER`, no output
    /// script) rather than a payout key (`TRANSFER`, the approved script).
    pub fn is_owner(&self) -> bool {
        matches!(
            self,
            WithdrawalKey::KeychainOwner { .. } | WithdrawalKey::AttachedOwner { .. }
        )
    }
}

impl VaultSigner {
    /// The signature of the masternode `pro_tx_hash`'s (wire order) credit
    /// withdrawal `transition` with `key`, which must be the identity key
    /// `identity_key` (MG-13 P3): 65 bytes, the compact recoverable
    /// signature with a compressed key over the double SHA-256 of
    /// `transition.signable_bytes()`, which the caller puts in the
    /// transition's signature. Only a
    /// [`SignerScope::MasternodeWithdrawal`] signer makes it, once.
    ///
    /// Refused before the secret is read, the signer still unspent:
    /// - another scope, or a key whose path or attached role the scope
    ///   refuses: [`SignerError::PathNotAllowed`];
    /// - a transition other than an `IdentityCreditWithdrawal` V1;
    ///   `user_fee_increase` not 0, `core_fee_per_byte` not
    ///   `MIN_CORE_FEE_PER_BYTE`, `pooling` not `Never`; an amount above the
    ///   scope's `max_credits`; an owner key with an output script or under a
    ///   scope that names a destination; a payout key under a scope with
    ///   none, or whose output script is not the scope's destination; a
    ///   signature already present: [`SignerError::PathNotAllowed`];
    /// - an identity id other than the reversed `pro_tx_hash`; an attached
    ///   key of another masternode; an identity key whose purpose is not the
    ///   key's (`OWNER` or `TRANSFER`), whose type is not `ECDSA_HASH160`,
    ///   that is disabled, or whose id is not the transition's
    ///   `signature_public_key_id`: [`SignerError::KeyMismatch`].
    ///
    /// Then the signer is spent (a second call, on it or a clone, is
    /// `vault.grant_invalid`), and with the secret, a key whose compressed
    /// public key's hash160 is not the identity key's data is
    /// [`SignerError::KeyMismatch`] with nothing signed.
    pub fn sign_masternode_withdrawal(
        &self,
        key: &WithdrawalKey,
        identity_key: &IdentityPublicKey,
        transition: &StateTransition,
        pro_tx_hash: &[u8; 32],
    ) -> Result<[u8; 65], SignerError> {
        let refused = |why: &str| Err(SignerError::PathNotAllowed(why.into()));
        let mismatch = |why: &str| Err(SignerError::KeyMismatch(why.into()));

        let scope = self.scope();
        let SignerScope::MasternodeWithdrawal {
            max_credits,
            destination,
        } = scope
        else {
            return Err(SignerError::PathNotAllowed(format!(
                "a masternode withdrawal under a {scope:?} signer"
            )));
        };
        let StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(t)) =
            transition
        else {
            return refused("a transition other than an identity credit withdrawal V1");
        };

        // The masternode's owner identity (module doc).
        let mut owner_identity = *pro_tx_hash;
        owner_identity.reverse();
        if t.identity_id.to_buffer() != owner_identity {
            return mismatch("a withdrawal from another identity than the masternode's");
        }
        if let WithdrawalKey::AttachedOwner { pro_tx_hash: mn }
        | WithdrawalKey::AttachedPayout { pro_tx_hash: mn } = key
            && mn != pro_tx_hash
        {
            return mismatch("the attached key of another masternode");
        }

        let owner = key.is_owner();
        let purpose = if owner {
            Purpose::OWNER
        } else {
            Purpose::TRANSFER
        };
        if identity_key.purpose() != purpose {
            return mismatch("an identity key of another purpose than the key's");
        }
        if identity_key.key_type() != KeyType::ECDSA_HASH160 {
            return mismatch("an identity key that is not ECDSA_HASH160");
        }
        if identity_key.disabled_at().is_some() {
            return mismatch("a disabled identity key");
        }
        if identity_key.id() != t.signature_public_key_id {
            return mismatch("an identity key other than the transition's signing key");
        }
        let Ok(key_hash) = <[u8; 20]>::try_from(identity_key.data().as_slice()) else {
            return mismatch("an identity key whose data is not a hash160");
        };

        if t.user_fee_increase != 0 {
            return refused("a withdrawal with a user fee increase");
        }
        if t.core_fee_per_byte != MIN_CORE_FEE_PER_BYTE {
            return refused("a withdrawal with another core fee per byte than the minimum");
        }
        if t.pooling != Pooling::Never {
            return refused("a pooled withdrawal");
        }
        if t.amount > max_credits {
            return refused("a withdrawal above the grant's credits");
        }
        // Owner key: Drive pays the registered payout address, so the
        // transition has no script and the grant named none. Payout key:
        // exactly the script the user approved.
        let destination_ok = match (owner, destination, &t.output_script) {
            (true, None, None) => true,
            (false, Some(approved), Some(script)) => approved.matches(script.as_bytes()),
            _ => false,
        };
        if !destination_ok {
            return refused("a withdrawal to another destination than the approved one");
        }
        if !t.signature.is_empty() {
            return refused("a transition that is already signed");
        }
        let data = transition
            .signable_bytes()
            .map_err(|e| SignerError::Derivation(format!("withdrawal transition: {e}")))?;
        let digest = sha256d::Hash::hash(&data).to_byte_array();

        let sign = |secret: &SecretKey| {
            let public = PublicKey::from_secret_key(secret);
            if hash160::Hash::hash(&public.serialize()).to_byte_array() != key_hash {
                return mismatch("the key is not the identity key");
            }
            Ok(sign_compact(digest, secret))
        };
        let use_ = KeyUse::MasternodeWithdrawal;
        match key {
            WithdrawalKey::KeychainOwner { index } => {
                let path = owner_key_path(self.network(), *index);
                self.signed_once_at(&path, sign)
            }
            WithdrawalKey::WalletPayout { path } => self.signed_once_at(path, sign),
            WithdrawalKey::AttachedOwner { pro_tx_hash }
            | WithdrawalKey::AttachedPayout { pro_tx_hash } => {
                let role = if owner { ROLE_OWNER } else { ROLE_PAYOUT };
                if !scope.allows_attached(use_, role) {
                    return Err(SignerError::PathNotAllowed(format!(
                        "the attached {role} key"
                    )));
                }
                self.consume_single_use()?;
                self.with_attached_key(pro_tx_hash, role, use_, sign)?
            }
        }
    }

    /// Spends the signer once the key at `path` passed the scope check,
    /// then runs `sign` on it.
    fn signed_once_at(
        &self,
        path: &DerivationPath,
        sign: impl FnOnce(&SecretKey) -> Result<[u8; 65], SignerError>,
    ) -> Result<[u8; 65], SignerError> {
        let use_ = KeyUse::MasternodeWithdrawal;
        self.check(use_, path)?;
        self.consume_single_use()?;
        self.with_key(path, use_, |x| sign(&x.private_key))?
    }
}
