//! [`VaultSigner`]: key_wallet's [`Signer`] backed by a wallet seed in the
//! vault, plus Dash message signing ([`WalletSigner`], the signer contract E2
//! codes against for transactions and `signmessage`).
//!
//! platform-wallet registers every wallet external-signable (it never holds
//! keys), so all signing for wallets this app creates goes through here.
//! Each call decrypts the seed record, derives the key for `path`, signs and
//! erases the derived key; nothing secret is cached in the signer.
//!
//! A [`SignerScope`] limits both the paths a signer derives and what it does
//! with the key there ([`KeyUse`]); a refused path or use is
//! [`SignerError::PathNotAllowed`] before the seed is read.
//!
//! Every call is one [`Op`] run by [`VaultSigner::run`] inside the vault's
//! operation gate ([`Vault::gated`]): from the epoch check until its result
//! is released, so [`Vault::lock`] waits for calls already running and every
//! later call is [`SignerError::Locked`]. A result is released only if the
//! signer's epoch is still the vault's, checked under the mutex every epoch
//! change holds, so none is released once `lock()` has returned (vault
//! module doc, "Release").

use async_trait::async_trait;
use dashcore::secp256k1::{Message, PublicKey, SecretKey, ecdsa};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use key_wallet::{ExtendedPubKeySigner, Network, Signer, SignerMethod};
use zeroize::Zeroizing;

use crate::SignerError;
use crate::paths::{self, is_bip44_path, is_coinjoin_path};
use crate::types::{KeySource, WalletId};
use crate::vault::Vault;
use crate::vault::masternode::{ROLE_OWNER, ROLE_VOTING};

/// Which derivations a signer may use, and for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignerScope {
    /// Every path of the wallet.
    Full,
    /// Only the DIP9 CoinJoin account `m/9'/coin'/4'/account'/…` (QT-112).
    CoinJoinOnly,
    /// The CoinJoin account plus the BIP44 accounts `m/44'/coin'/…`: what
    /// mixing needs to turn ordinary coins into denominations and
    /// collaterals of the same wallet (Dash Core lets a mixing-only unlock
    /// create those, `CCoinJoinClientSession::CreateDenominated`,
    /// `src/coinjoin/client.cpp:2194`). The engine uses it only for
    /// transactions whose outputs all pay the wallet itself.
    CoinJoinFunding,
    /// Platform state transitions (DASHPAY §3.3): signatures and public
    /// keys (no chain codes) of the DIP-13 ECDSA identity keys
    /// `m/9'/coin'/5'/0'/0'/identity'/key'` only.
    PlatformIdentity,
    /// DashPay contact crypto (DIP-15); **never signs**. Extended public
    /// keys of the receiving accounts `m/9'/coin'/15'/account'/<user>/<friend>`
    /// (DIP-14 256-bit children), of the auto-accept keys
    /// `m/9'/coin'/16'/expiry'` and of BIP44 account 0 (the seed-binding
    /// check); ECDH and the account-reference mask with identity keys (the
    /// DIP-15 encryption keys are identity keys); the contactInfo keys
    /// `65536'`/`65537'` under an identity key; and the export of an
    /// auto-accept key.
    DashPayCrypto,
    /// Asset-lock funding (registration, top-up): signatures and public keys
    /// (no chain codes) of BIP44, BIP32 and DashPay-receiving addresses and
    /// of the credit keys `m/9'/coin'/5'/{1',2',3'}/…`, plus the extended
    /// public key of an identity's top-up account `m/9'/coin'/5'/2'/i'`
    /// (platform-wallet adds that account through the signer). The cap is
    /// bounded by the token ([`crate::Vault::platform_signer`] refuses a
    /// `max_duffs` above the grant's), but the vault signs sighashes and
    /// cannot see the debit, so the engine checks it before the transaction
    /// leaves the process (E0-04 design §3.4, §4.2).
    PlatformFunding { max_duffs: u64 },
    /// Governance votes (MG-05 §3, D1): the keychain voting keys
    /// `m/9'/coin'/3'/1'/i` and the `voting` key attached to a tracked
    /// masternode, for [`KeyUse::Vote`] only, which no [`VaultSigner`]
    /// method makes. Only a [`crate::VoteSigner`] reaches these keys, and it
    /// signs the digest of a funding vote it builds itself, never one it is
    /// given: no transaction sighash, so coins sent to a voting key's
    /// address stay out of a vote signer's reach.
    MasternodeVoting,
    /// Masternode Core transactions (MG-05 §1): signatures and public keys
    /// of BIP44 and BIP32 addresses (inputs, change), `signmessage` with an
    /// address key (an existing collateral's proof), and a ProUpRegTx's
    /// payload signature ([`KeyUse::OwnerPayload`]) with a keychain owner
    /// key `m/9'/coin'/3'/2'/i` or the `owner` key attached to that
    /// masternode (MG-04 D11, D12), and a shared masternode's owner
    /// signatures (consent, ProUpShareTx, ProUpSharedRegTx, ProDisTx) with
    /// a keychain owner key only (MG-04b D11). An owner key signs no digest
    /// it is given. Like `PlatformFunding`, the vault cannot see the debit of
    /// what it signs; the engine checks it against the grant's cap first
    /// (MG-05 §2). No operator or platform-node key is in reach.
    MasternodeOwner,
}

/// What a derived key is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyUse {
    /// A signature over a digest or a state transition.
    Sign,
    /// A Dash Core `signmessage` signature.
    Message,
    /// The public key.
    PublicKey,
    /// The extended public key: the public key and its chain code, which
    /// lets the holder derive every non-hardened descendant.
    ExtendedPublicKey,
    /// DIP-15 key agreement: ECDH, the account-reference mask.
    Agreement,
    /// A DIP-15 contactInfo AES key (`encToUserId`, `privateData`).
    ContactInfo,
    /// The raw key leaves the vault (the DIP-15 auto-accept key).
    Export,
    /// A governance vote's key id, or a signature over the digest of a
    /// funding vote the vault builds ([`crate::VoteSigner`]).
    Vote,
    /// An owner signature over the digest of a payload or transaction the
    /// vault hashes itself: a ProUpRegTx's
    /// ([`VaultSigner::sign_update_registrar`]) or a shared masternode's
    /// ([`VaultSigner::sign_shared_consent`] and its siblings, MG-04b
    /// §6.2).
    OwnerPayload,
}

impl SignerScope {
    /// The funding cap of [`SignerScope::PlatformFunding`].
    pub fn max_duffs(&self) -> Option<u64> {
        match self {
            SignerScope::PlatformFunding { max_duffs } => Some(*max_duffs),
            _ => None,
        }
    }

    /// Whether this scope may use the key at `path` for `key_use`.
    pub(crate) fn allows(&self, key_use: KeyUse, path: &DerivationPath, network: Network) -> bool {
        // What the CoinJoin scopes have always allowed.
        let coinjoin_use = matches!(
            key_use,
            KeyUse::Sign | KeyUse::Message | KeyUse::PublicKey | KeyUse::ExtendedPublicKey
        );
        let sign_or_public_key = matches!(key_use, KeyUse::Sign | KeyUse::PublicKey);
        match self {
            // An owner payload signature comes from a `MasternodeOp` grant
            // only (MG-04 D38), and no other signature at an owner key: a
            // ProUpRegTx's owner signature is a plain ECDSA one over the
            // payload's hash, which `Sign` would make for any digest (D50).
            SignerScope::Full => match key_use {
                KeyUse::OwnerPayload => false,
                KeyUse::Sign | KeyUse::Message => !paths::is_owner_key(path, network),
                _ => true,
            },
            SignerScope::CoinJoinOnly => coinjoin_use && is_coinjoin_path(path, network),
            SignerScope::CoinJoinFunding => {
                coinjoin_use && (is_coinjoin_path(path, network) || is_bip44_path(path, network))
            }
            SignerScope::PlatformIdentity => {
                sign_or_public_key && paths::identity_auth_key(path, network).is_some()
            }
            SignerScope::DashPayCrypto => match key_use {
                KeyUse::Sign | KeyUse::Message | KeyUse::Vote | KeyUse::OwnerPayload => false,
                KeyUse::PublicKey | KeyUse::ExtendedPublicKey => {
                    paths::is_dashpay_receiving_account(path, network)
                        || paths::is_auto_accept_key(path, network)
                        || paths::is_bip44_account_zero(path, network)
                }
                KeyUse::Agreement => paths::identity_auth_key(path, network).is_some(),
                KeyUse::ContactInfo => paths::is_contact_info_key(path, network),
                KeyUse::Export => paths::is_auto_accept_key(path, network),
            },
            SignerScope::PlatformFunding { .. } => match key_use {
                KeyUse::Sign | KeyUse::PublicKey => {
                    paths::is_bip44_address(path, network)
                        || paths::is_bip32_address(path)
                        || paths::is_dashpay_receiving_address(path, network)
                        || paths::is_asset_lock_credit_key(path, network)
                }
                KeyUse::ExtendedPublicKey => paths::is_identity_top_up_account(path, network),
                _ => false,
            },
            SignerScope::MasternodeVoting => {
                key_use == KeyUse::Vote && paths::is_voting_key(path, network)
            }
            SignerScope::MasternodeOwner => {
                let address =
                    paths::is_bip44_address(path, network) || paths::is_bip32_address(path);
                let owner = paths::is_owner_key(path, network);
                match key_use {
                    KeyUse::Sign | KeyUse::Message => address,
                    KeyUse::PublicKey => address || owner,
                    KeyUse::OwnerPayload => owner,
                    _ => false,
                }
            }
        }
    }

    /// Whether this scope may use the key attached for `role` to a tracked
    /// masternode for `key_use` (MG-01 D7): the `voting` key, for
    /// [`KeyUse::Vote`] under [`SignerScope::MasternodeVoting`], and the
    /// `owner` key, for [`KeyUse::OwnerPayload`] under
    /// [`SignerScope::MasternodeOwner`] (MG-04 D12). Nothing else.
    pub(crate) fn allows_attached(&self, key_use: KeyUse, role: &str) -> bool {
        match self {
            SignerScope::MasternodeVoting => key_use == KeyUse::Vote && role == ROLE_VOTING,
            SignerScope::MasternodeOwner => key_use == KeyUse::OwnerPayload && role == ROLE_OWNER,
            _ => false,
        }
    }
}

/// The signer contract for transaction and message signing.
#[async_trait]
pub trait WalletSigner: ExtendedPubKeySigner + Signer<Error = SignerError> {
    /// The wallet whose seed signs.
    fn wallet_id(&self) -> WalletId;

    /// Dash Core `signmessage` for the key at `path`: base64 of the 65-byte
    /// compact signature over `"DarkCoin Signed Message:\n" ‖ message`,
    /// compressed public key.
    async fn sign_message(
        &self,
        path: &DerivationPath,
        message: &[u8],
    ) -> Result<String, SignerError>;
}

const METHODS: &[SignerMethod] = &[SignerMethod::Digest];

/// Core's `CKey::SignCompact` over `digest` for a compressed key:
/// `27 + 4 + recid ‖ r ‖ s`, deterministic (RFC 6979).
/// Also what `dashcore::signer::sign` makes for a Platform
/// `ECDSA_HASH160` key (MG-07's name votes).
pub(crate) fn sign_compact(digest: [u8; 32], secret: &SecretKey) -> [u8; 65] {
    let sig =
        ecdsa::RecoverableSignature::sign_ecdsa_recoverable(Message::from_digest(digest), secret);
    let (recid, rs) = sig.serialize_compact();
    let mut out = [0u8; 65];
    out[0] = 27 + recid.to_u8() + 4;
    out[1..].copy_from_slice(&rs);
    out
}

/// A signer for one wallet, valid until the vault locks or changes unlock
/// scope. Obtain it from [`Vault::signer`] (after redeeming a grant) or
/// [`Vault::mixing_signer`].
#[derive(Clone)]
pub struct VaultSigner {
    vault: Vault,
    wallet_id: WalletId,
    scope: SignerScope,
    epoch: u64,
    /// The data key it signs with ([`KeySource`]).
    key: KeySource,
}

impl std::fmt::Debug for VaultSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSigner")
            .field("wallet_id", &hex::encode(self.wallet_id))
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl VaultSigner {
    pub(crate) fn new(
        vault: Vault,
        wallet_id: WalletId,
        scope: SignerScope,
        epoch: u64,
        key: KeySource,
    ) -> Self {
        Self {
            vault,
            wallet_id,
            scope,
            epoch,
            key,
        }
    }

    pub fn scope(&self) -> SignerScope {
        self.scope
    }

    /// The network of the vault this signer derives for.
    pub fn network(&self) -> Network {
        self.vault.network()
    }

    /// The scope check of one use of the key at `path`.
    fn check(&self, key_use: KeyUse, path: &DerivationPath) -> Result<(), SignerError> {
        if self.scope.allows(key_use, path, self.vault.network()) {
            Ok(())
        } else {
            Err(SignerError::PathNotAllowed(path.to_string()))
        }
    }

    /// Runs `body` as one operation on the keys `uses`, the only way a
    /// signer reaches a key: every use passes the scope check before the
    /// seed is read; then, inside the vault's gate, the signer's epoch must
    /// be the one the operation started under, the seed is decrypted and
    /// `body` runs on the [`Op`]. The master key is erased when `body`
    /// returns, and its result is released only if that epoch is still
    /// current ([`Vault::gated`]), else [`SignerError::Locked`].
    pub(crate) fn run<T>(
        &self,
        uses: &[(KeyUse, &DerivationPath)],
        body: impl FnOnce(&Op<'_>) -> Result<T, SignerError>,
    ) -> Result<T, SignerError> {
        for (key_use, path) in uses {
            self.check(*key_use, path)?;
        }
        self.vault.gated(SignerError::Locked, |gate| {
            let seed = self
                .vault
                .signing_seed(gate, &self.wallet_id, self.epoch, &self.key)?;
            let master = ExtendedPrivKey::new_master(self.vault.network(), &seed[..])
                .map_err(|e| SignerError::Derivation(e.to_string()))?;
            #[cfg(test)]
            test_hook::opened();
            body(&Op {
                signer: self,
                master,
            })
        })
    }

    /// Runs `f` on the key attached for `role` to the tracked masternode
    /// `pro_tx_hash`, in an operation of its own, then erases the key. The
    /// scope check ([`SignerScope::allows_attached`]) comes before the
    /// record is read; inside the gate the signer's epoch must still be the
    /// vault's, as for a seed ([`Self::run`]). `NoSecret` when no key is
    /// attached for `role`, or the record is no secp256k1 secret.
    pub(crate) fn with_attached_key<T>(
        &self,
        pro_tx_hash: &[u8; 32],
        role: &str,
        key_use: KeyUse,
        f: impl FnOnce(&SecretKey) -> T,
    ) -> Result<T, SignerError> {
        if !self.scope.allows_attached(key_use, role) {
            return Err(SignerError::PathNotAllowed(format!(
                "the attached {role} key"
            )));
        }
        self.vault.gated(SignerError::Locked, |gate| {
            /// Erases the secret when dropped, so a panicking `f` erases it
            /// too.
            struct Erased(SecretKey);
            impl Drop for Erased {
                fn drop(&mut self) {
                    self.0.non_secure_erase();
                }
            }
            let secret = Erased(self.vault.attached_secret(
                gate,
                self.epoch,
                &self.key,
                pro_tx_hash,
                role,
            )?);
            #[cfg(test)]
            test_hook::opened();
            Ok(f(&secret.0))
        })
    }

    /// Runs `f` on the private key at `path` in an operation of its own,
    /// then erases the key.
    pub(crate) fn with_key<T>(
        &self,
        path: &DerivationPath,
        key_use: KeyUse,
        f: impl FnOnce(&ExtendedPrivKey) -> T,
    ) -> Result<T, SignerError> {
        self.run(&[(key_use, path)], |op| op.with_key(path, key_use, f))
    }
}

/// One signer call inside the gate ([`VaultSigner::run`]): the wallet's
/// master key, erased on drop, before the call's result is released.
pub(crate) struct Op<'a> {
    signer: &'a VaultSigner,
    master: ExtendedPrivKey,
}

impl Op<'_> {
    /// Runs `f` on the private key at `path` (scope-checked again), then
    /// erases the key.
    pub(crate) fn with_key<T>(
        &self,
        path: &DerivationPath,
        key_use: KeyUse,
        f: impl FnOnce(&ExtendedPrivKey) -> T,
    ) -> Result<T, SignerError> {
        self.signer.check(key_use, path)?;
        let mut xpriv = self
            .master
            .derive_priv(path)
            .map_err(|e| SignerError::Derivation(e.to_string()))?;
        let out = f(&xpriv);
        xpriv.private_key.non_secure_erase();
        Ok(out)
    }
}

impl Drop for Op<'_> {
    fn drop(&mut self) {
        self.master.private_key.non_secure_erase();
    }
}

/// Lets the lock-race tests pause a gated operation of the current thread
/// once it has passed its epoch check ([`opened`]).
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;

    type Hook = Box<dyn FnMut()>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
        static CHECKED: RefCell<Option<Hook>> = const { RefCell::new(None) };
        static KEY_TAKEN: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Calls `f` whenever an operation of this thread is opened.
    pub(crate) fn set(f: impl FnMut() + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    /// Calls `f` whenever an operation of this thread has taken its copy
    /// of a grant's own key inside the gate, before it reads the seed.
    pub(crate) fn set_key_taken(f: impl FnMut() + 'static) {
        KEY_TAKEN.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(crate) fn key_taken() {
        KEY_TAKEN.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f();
            }
        });
    }

    /// Calls `f` whenever an operation of this thread has checked (or
    /// redeemed) its grant token, or loaded the vault's key, and not yet
    /// entered the gate.
    pub(crate) fn set_checked(f: impl FnMut() + 'static) {
        CHECKED.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(crate) fn opened() {
        HOOK.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f();
            }
        });
    }

    pub(crate) fn checked() {
        CHECKED.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f();
            }
        });
    }
}

#[async_trait]
impl Signer for VaultSigner {
    type Error = SignerError;

    fn supported_methods(&self) -> &[SignerMethod] {
        METHODS
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<(ecdsa::Signature, PublicKey), SignerError> {
        self.with_key(path, KeyUse::Sign, |x| {
            // Low-R grinding, as Dash Core's `CKey::Sign`: one byte smaller on
            // average, and the same signature dashd makes for the sighash.
            let sig = ecdsa::sign_low_r(Message::from_digest(sighash), &x.private_key);
            (sig, PublicKey::from_secret_key(&x.private_key))
        })
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
        self.with_key(path, KeyUse::PublicKey, |x| {
            PublicKey::from_secret_key(&x.private_key)
        })
    }
}

#[async_trait]
impl ExtendedPubKeySigner for VaultSigner {
    async fn extended_public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, SignerError> {
        self.with_key(path, KeyUse::ExtendedPublicKey, ExtendedPubKey::from_priv)
    }
}

#[async_trait]
impl WalletSigner for VaultSigner {
    fn wallet_id(&self) -> WalletId {
        self.wallet_id
    }

    async fn sign_message(
        &self,
        path: &DerivationPath,
        message: &[u8],
    ) -> Result<String, SignerError> {
        self.with_key(path, KeyUse::Message, |x| {
            let secret = dw_uri::keyio::Secret {
                key: Zeroizing::new(x.private_key.to_secret_bytes()),
                compressed: true,
            };
            dw_message::sign_message(&secret, message)
                .map_err(|e| SignerError::Derivation(e.to_string()))
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn path(s: &str) -> DerivationPath {
        DerivationPath::from_str(s).unwrap()
    }

    const T: Network = Network::Testnet;
    const ALL_USES: [KeyUse; 9] = [
        KeyUse::Sign,
        KeyUse::Message,
        KeyUse::PublicKey,
        KeyUse::ExtendedPublicKey,
        KeyUse::Agreement,
        KeyUse::ContactInfo,
        KeyUse::Export,
        KeyUse::Vote,
        KeyUse::OwnerPayload,
    ];

    fn uses(scope: SignerScope, p: &str) -> Vec<KeyUse> {
        ALL_USES
            .into_iter()
            .filter(|u| scope.allows(*u, &path(p), T))
            .collect()
    }

    /// Every derived key and the per-call master key rely on key-wallet's
    /// erasing `Drop` for `ExtendedPrivKey` (rust-dashcore `e4208c9`,
    /// `bip32.rs:373-395`); a pin without it would leave `Op::master`'s
    /// chain code and `ScanKey::master_key`'s clone unerased.
    #[test]
    fn extended_private_keys_erase_on_drop() {
        assert!(std::mem::needs_drop::<ExtendedPrivKey>());
    }

    /// Everything but an owner payload signature, which only a
    /// `MasternodeOp` grant's signer makes (MG-04 D38), and no signature or
    /// message signature at an owner key (D50). A voting key keeps every
    /// use but the owner payload.
    #[test]
    fn full_scope_allows_everything_but_owner_signatures() {
        for p in ["m/0", "m/44'/1'/0'/0/0", "m/9'/1'/3'/1'/0"] {
            assert_eq!(uses(SignerScope::Full, p), ALL_USES[..8].to_vec(), "{p}");
        }
        let owner: Vec<KeyUse> = ALL_USES[..8]
            .iter()
            .copied()
            .filter(|u| !matches!(u, KeyUse::Sign | KeyUse::Message))
            .collect();
        for p in ["m/9'/1'/3'/2'/0", "m/9'/1'/3'/2'/7"] {
            assert_eq!(uses(SignerScope::Full, p), owner, "{p}");
        }
    }

    #[test]
    fn coinjoin_scopes_keep_their_paths_and_never_touch_platform_uses() {
        let cj = "m/9'/1'/4'/0'/0/1";
        let bip44 = "m/44'/1'/0'/0/0";
        let before = vec![
            KeyUse::Sign,
            KeyUse::Message,
            KeyUse::PublicKey,
            KeyUse::ExtendedPublicKey,
        ];
        assert_eq!(uses(SignerScope::CoinJoinOnly, cj), before);
        assert!(uses(SignerScope::CoinJoinOnly, bip44).is_empty());
        assert_eq!(uses(SignerScope::CoinJoinFunding, bip44), before);
        assert!(uses(SignerScope::CoinJoinFunding, "m/9'/1'/5'/0'/0'/0'/0'").is_empty());
    }

    #[test]
    fn platform_identity_signs_identity_keys_only() {
        let s = SignerScope::PlatformIdentity;
        assert_eq!(
            uses(s, "m/9'/1'/5'/0'/0'/0'/3'"),
            vec![KeyUse::Sign, KeyUse::PublicKey]
        );
        for other in [
            "m/44'/1'/0'/0/0",
            "m/9'/1'/4'/0'/0/1",
            "m/9'/1'/5'/1'/0",
            "m/9'/1'/5'/0'/0'/0'/3'/65536'/0'",
            "m/9'/1'/16'/1'",
            "m/9'/1'/5'/0'/0'/0'",
            "m",
        ] {
            assert!(uses(s, other).is_empty(), "{other}");
        }
    }

    #[test]
    fn dashpay_crypto_never_signs() {
        let s = SignerScope::DashPayCrypto;
        let public = vec![KeyUse::PublicKey, KeyUse::ExtendedPublicKey];
        assert_eq!(
            uses(s, "m/9'/1'/16'/7'"),
            [public.clone(), vec![KeyUse::Export]].concat()
        );
        assert_eq!(uses(s, "m/44'/1'/0'"), public);
        assert_eq!(uses(s, "m/9'/1'/5'/0'/0'/0'/4'"), vec![KeyUse::Agreement]);
        assert_eq!(
            uses(s, "m/9'/1'/5'/0'/0'/0'/2'/65537'/1'"),
            vec![KeyUse::ContactInfo]
        );
        for other in [
            "m/44'/1'/0'/0/0",
            "m/44'/1'/1'",
            "m/9'/1'/4'/0'/0/1",
            "m/9'/1'/5'/1'/0",
            "m/9'/1'/5'/0'/0'/0'/2'/65538'/1'",
            "m/9'/1'/15'/0'",
            "m",
        ] {
            assert!(uses(s, other).is_empty(), "{other}");
        }
    }

    #[test]
    fn platform_funding_signs_funding_inputs_and_credit_keys() {
        let s = SignerScope::PlatformFunding { max_duffs: 1 };
        assert_eq!(s.max_duffs(), Some(1));
        for ok in [
            "m/44'/1'/0'/0/0",
            "m/0'/1/3",
            "m/9'/1'/5'/1'/0",
            "m/9'/1'/5'/2'/0'/1",
            "m/9'/1'/5'/3'/2",
        ] {
            assert_eq!(uses(s, ok), vec![KeyUse::Sign, KeyUse::PublicKey], "{ok}");
        }
        assert_eq!(uses(s, "m/9'/1'/5'/2'/0'"), vec![KeyUse::ExtendedPublicKey]);
        for other in [
            "m/9'/1'/4'/0'/0/1",
            "m/9'/1'/5'/0'/0'/0'/0'",
            "m/9'/1'/16'/1'",
            "m/44'/1'/0'",
            "m/44'/5'/0'/0/0",
            "m",
        ] {
            assert!(uses(s, other).is_empty(), "{other}");
        }
    }

    /// A voting key is reached for votes only (MG-05 D1), and no other scope
    /// but `Full` reaches it at all.
    #[test]
    fn masternode_voting_votes_with_voting_keys_only() {
        let s = SignerScope::MasternodeVoting;
        assert_eq!(uses(s, "m/9'/1'/3'/1'/0"), vec![KeyUse::Vote]);
        assert_eq!(uses(s, "m/9'/1'/3'/1'/19"), vec![KeyUse::Vote]);
        for other in [
            "m/9'/1'/3'/2'/0",
            "m/9'/1'/3'/3'/0",
            "m/9'/1'/3'/4'/0'",
            "m/9'/1'/3'/1'/0'",
            "m/9'/5'/3'/1'/0",
            "m/44'/1'/0'/0/0",
            "m/0'/1/3",
            "m",
        ] {
            assert!(uses(s, other).is_empty(), "{other}");
        }
        for other in [
            SignerScope::CoinJoinOnly,
            SignerScope::CoinJoinFunding,
            SignerScope::PlatformIdentity,
            SignerScope::DashPayCrypto,
            SignerScope::PlatformFunding { max_duffs: 1 },
            SignerScope::MasternodeOwner,
        ] {
            assert!(uses(other, "m/9'/1'/3'/1'/0").is_empty(), "{other:?}");
            for path in ["m/44'/1'/0'/0/0", "m/9'/1'/3'/2'/0"] {
                assert!(
                    !other.allows(KeyUse::Vote, &self::path(path), T),
                    "{other:?}"
                );
            }
        }
        assert!(s.allows_attached(KeyUse::Vote, "voting"));
        for (scope, key_use, role) in [
            (s, KeyUse::Sign, "voting"),
            (s, KeyUse::PublicKey, "voting"),
            (s, KeyUse::Vote, "owner"),
            (s, KeyUse::Vote, "payout"),
            (SignerScope::Full, KeyUse::Vote, "voting"),
            (SignerScope::MasternodeOwner, KeyUse::Sign, "owner"),
            (SignerScope::MasternodeOwner, KeyUse::Vote, "voting"),
            (SignerScope::MasternodeOwner, KeyUse::OwnerPayload, "voting"),
            (SignerScope::MasternodeOwner, KeyUse::OwnerPayload, "payout"),
            (SignerScope::Full, KeyUse::OwnerPayload, "owner"),
            (s, KeyUse::OwnerPayload, "owner"),
        ] {
            assert!(
                !scope.allows_attached(key_use, role),
                "{scope:?} {key_use:?} {role}"
            );
        }
    }

    /// MG-04 D11: an owner key signs owner payloads only, no digest it is
    /// given; the attached owner key likewise (D12).
    #[test]
    fn masternode_owner_signs_addresses_and_owner_payloads() {
        let s = SignerScope::MasternodeOwner;
        let address = vec![KeyUse::Sign, KeyUse::Message, KeyUse::PublicKey];
        assert_eq!(uses(s, "m/44'/1'/0'/0/0"), address);
        assert_eq!(uses(s, "m/44'/1'/3'/1/9"), address);
        assert_eq!(uses(s, "m/0'/1/3"), address);
        assert_eq!(
            uses(s, "m/9'/1'/3'/2'/4"),
            vec![KeyUse::PublicKey, KeyUse::OwnerPayload]
        );
        assert!(s.allows_attached(KeyUse::OwnerPayload, "owner"));
        for other in [
            SignerScope::CoinJoinOnly,
            SignerScope::CoinJoinFunding,
            SignerScope::PlatformIdentity,
            SignerScope::DashPayCrypto,
            SignerScope::PlatformFunding { max_duffs: 1 },
            SignerScope::MasternodeVoting,
            SignerScope::Full,
        ] {
            assert!(
                !other.allows(KeyUse::OwnerPayload, &path("m/9'/1'/3'/2'/0"), T),
                "{other:?}"
            );
        }
        for other in [
            "m/9'/1'/3'/1'/0",  // voting
            "m/9'/1'/3'/3'/0",  // operator
            "m/9'/1'/3'/4'/0'", // platform node
            "m/9'/1'/3'/2'/0'",
            "m/9'/1'/4'/0'/0/1",
            "m/9'/1'/5'/1'/0",
            "m/9'/1'/5'/0'/0'/0'/0'",
            "m/44'/1'/0'",
            "m/44'/5'/0'/0/0",
            "m",
        ] {
            assert!(uses(s, other).is_empty(), "{other}");
        }
    }
}
