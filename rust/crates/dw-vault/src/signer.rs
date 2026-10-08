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
//! Every call is one [`Op`]: it holds the vault's operation gate from the
//! epoch check until its result exists, so [`Vault::lock`] waits for calls
//! already running and every later call is [`SignerError::Locked`].

use std::sync::Arc;

use async_trait::async_trait;
use dashcore::secp256k1::{All, Message, PublicKey, Secp256k1, ecdsa};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use key_wallet::{ExtendedPubKeySigner, Network, Signer, SignerMethod};
use zeroize::Zeroizing;

use crate::SignerError;
use crate::crypto::Key32;
use crate::paths::{self, is_bip44_path, is_coinjoin_path};
use crate::types::WalletId;
use crate::vault::{OpGuard, Vault};

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
    /// (platform-wallet adds that account through the signer). The engine
    /// refuses a transaction whose wallet debit exceeds `max_duffs` before
    /// it asks this signer, as it does for a `Spend` grant; until roadmap
    /// E0-04 puts caps on `PlatformOp` grants, the engine also picks the cap.
    PlatformFunding { max_duffs: u64 },
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
            SignerScope::Full => true,
            SignerScope::CoinJoinOnly => coinjoin_use && is_coinjoin_path(path, network),
            SignerScope::CoinJoinFunding => {
                coinjoin_use && (is_coinjoin_path(path, network) || is_bip44_path(path, network))
            }
            SignerScope::PlatformIdentity => {
                sign_or_public_key && paths::identity_auth_key(path, network).is_some()
            }
            SignerScope::DashPayCrypto => match key_use {
                KeyUse::Sign | KeyUse::Message => false,
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

/// A signer for one wallet, valid until the vault locks or changes unlock
/// scope. Obtain it from [`Vault::signer`] (after redeeming a grant) or
/// [`Vault::mixing_signer`].
#[derive(Clone)]
pub struct VaultSigner {
    vault: Vault,
    wallet_id: WalletId,
    scope: SignerScope,
    epoch: u64,
    /// The data key of the grant that issued this signer, when a passphrase
    /// authorized it on a locked or mixing-only vault; otherwise the vault's
    /// key is used. Zeroized when the last clone drops.
    own_key: Option<Arc<Key32>>,
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
        own_key: Option<Arc<Key32>>,
    ) -> Self {
        Self {
            vault,
            wallet_id,
            scope,
            epoch,
            own_key,
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

    /// Opens one operation that will use the keys `uses`: every use passes
    /// the scope check before the seed is read, then the operation gate is
    /// taken and the epoch checked.
    pub(crate) fn op(&self, uses: &[(KeyUse, &DerivationPath)]) -> Result<Op<'_>, SignerError> {
        for (key_use, path) in uses {
            self.check(*key_use, path)?;
        }
        let gate = self.vault.op_guard();
        let seed =
            self.vault
                .signing_seed(&gate, &self.wallet_id, self.epoch, self.own_key.as_deref())?;
        let master = ExtendedPrivKey::new_master(self.vault.network(), &seed[..])
            .map_err(|e| SignerError::Derivation(e.to_string()))?;
        #[cfg(test)]
        test_hook::fire(test_hook::OpPoint::Opened);
        Ok(Op {
            signer: self,
            master,
            secp: Secp256k1::new(),
            _gate: gate,
        })
    }

    /// Runs `f` on the private key at `path` in an operation of its own,
    /// then erases the key.
    pub(crate) fn with_key<T>(
        &self,
        path: &DerivationPath,
        key_use: KeyUse,
        f: impl FnOnce(&Secp256k1<All>, &ExtendedPrivKey) -> T,
    ) -> Result<T, SignerError> {
        self.op(&[(key_use, path)])?.with_key(path, key_use, f)
    }
}

/// One signer call ([`VaultSigner::op`]): the wallet's master key and the
/// vault's operation gate, held until the call's result exists. On drop the
/// master key is erased, then the gate is released.
pub(crate) struct Op<'a> {
    signer: &'a VaultSigner,
    master: ExtendedPrivKey,
    secp: Secp256k1<All>,
    _gate: OpGuard<'a>,
}

impl Op<'_> {
    /// Runs `f` on the private key at `path` (scope-checked again), then
    /// erases the key.
    pub(crate) fn with_key<T>(
        &self,
        path: &DerivationPath,
        key_use: KeyUse,
        f: impl FnOnce(&Secp256k1<All>, &ExtendedPrivKey) -> T,
    ) -> Result<T, SignerError> {
        self.signer.check(key_use, path)?;
        let mut xpriv = self
            .master
            .derive_priv(&self.secp, path)
            .map_err(|e| SignerError::Derivation(e.to_string()))?;
        let out = f(&self.secp, &xpriv);
        xpriv.private_key.non_secure_erase();
        Ok(out)
    }
}

impl Drop for Op<'_> {
    fn drop(&mut self) {
        self.master.private_key.non_secure_erase();
        #[cfg(test)]
        test_hook::fire(test_hook::OpPoint::Closing);
    }
}

/// Observes [`Op`]s on the current thread, for the lock-race tests: `Opened`
/// once the epoch check passed, `Closing` just before the gate is released.
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum OpPoint {
        Opened,
        Closing,
    }

    type Hook = Box<dyn FnMut(OpPoint)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Calls `f` at every [`OpPoint`] of the operations this thread runs.
    pub(crate) fn set(f: impl FnMut(OpPoint) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(crate) fn fire(point: OpPoint) {
        HOOK.with(|h| {
            if let Some(f) = h.borrow_mut().as_mut() {
                f(point);
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
        self.with_key(path, KeyUse::Sign, |secp, x| {
            // Low-R grinding, as Dash Core's `CKey::Sign`: one byte smaller on
            // average, and the same signature dashd makes for the sighash.
            let sig = secp.sign_ecdsa_low_r(&Message::from_digest(sighash), &x.private_key);
            (sig, PublicKey::from_secret_key(secp, &x.private_key))
        })
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
        self.with_key(path, KeyUse::PublicKey, |secp, x| {
            PublicKey::from_secret_key(secp, &x.private_key)
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
        self.with_key(path, KeyUse::Message, |_, x| {
            let secret = dw_uri::keyio::Secret {
                key: Zeroizing::new(x.private_key.secret_bytes()),
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
    const ALL_USES: [KeyUse; 7] = [
        KeyUse::Sign,
        KeyUse::Message,
        KeyUse::PublicKey,
        KeyUse::ExtendedPublicKey,
        KeyUse::Agreement,
        KeyUse::ContactInfo,
        KeyUse::Export,
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

    #[test]
    fn full_scope_allows_everything() {
        assert_eq!(uses(SignerScope::Full, "m/0"), ALL_USES.to_vec());
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
}
