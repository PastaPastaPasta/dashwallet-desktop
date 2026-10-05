//! [`VaultSigner`]: key_wallet's [`Signer`] backed by a wallet seed in the
//! vault, plus Dash message signing ([`WalletSigner`], the signer contract E2
//! codes against for transactions and `signmessage`).
//!
//! platform-wallet registers every wallet external-signable (it never holds
//! keys), so all signing for wallets this app creates goes through here.
//! Each call decrypts the seed record, derives the key for `path`, signs and
//! erases the derived key; nothing secret is cached in the signer.

use async_trait::async_trait;
use dashcore::secp256k1::{Message, PublicKey, Secp256k1, ecdsa};
use key_wallet::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use key_wallet::{ExtendedPubKeySigner, Network, Signer, SignerMethod};
use zeroize::Zeroizing;

use crate::SignerError;
use crate::types::WalletId;
use crate::vault::Vault;

/// Which derivations a signer may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignerScope {
    /// Every path of the wallet.
    Full,
    /// Only the DIP9 CoinJoin account `m/9'/coin'/4'/account'/…` (QT-112).
    CoinJoinOnly,
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
}

impl std::fmt::Debug for VaultSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSigner")
            .field("wallet_id", &hex::encode(self.wallet_id))
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// SLIP-44 coin type: 5 on mainnet, 1 everywhere else.
fn coin_type(network: Network) -> u32 {
    if network == Network::Mainnet { 5 } else { 1 }
}

/// Whether `path` lies inside the CoinJoin account branch of `network`.
pub fn is_coinjoin_path(path: &DerivationPath, network: Network) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    let hardened = |i| ChildNumber::from_hardened_idx(i).ok();
    p.len() >= 4
        && Some(p[0]) == hardened(9)
        && Some(p[1]) == hardened(coin_type(network))
        && Some(p[2]) == hardened(4)
}

impl VaultSigner {
    pub(crate) fn new(vault: Vault, wallet_id: WalletId, scope: SignerScope, epoch: u64) -> Self {
        Self {
            vault,
            wallet_id,
            scope,
            epoch,
        }
    }

    pub fn scope(&self) -> SignerScope {
        self.scope
    }

    /// Derives the extended private key at `path`. The caller erases it.
    fn derive(&self, path: &DerivationPath) -> Result<ExtendedPrivKey, SignerError> {
        let network = self.vault.network();
        if self.scope == SignerScope::CoinJoinOnly && !is_coinjoin_path(path, network) {
            return Err(SignerError::PathNotAllowed(path.to_string()));
        }
        let seed = self.vault.signing_seed(&self.wallet_id, self.epoch)?;
        let secp = Secp256k1::new();
        let mut master = ExtendedPrivKey::new_master(network, &seed[..])
            .map_err(|e| SignerError::Derivation(e.to_string()))?;
        let derived = master
            .derive_priv(&secp, path)
            .map_err(|e| SignerError::Derivation(e.to_string()));
        master.private_key.non_secure_erase();
        derived
    }

    /// Runs `f` on the private key at `path`, then erases the key.
    fn with_key<T>(
        &self,
        path: &DerivationPath,
        f: impl FnOnce(&Secp256k1<dashcore::secp256k1::All>, &ExtendedPrivKey) -> T,
    ) -> Result<T, SignerError> {
        let mut xpriv = self.derive(path)?;
        let secp = Secp256k1::new();
        let out = f(&secp, &xpriv);
        xpriv.private_key.non_secure_erase();
        Ok(out)
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
        self.with_key(path, |secp, x| {
            let sig = secp.sign_ecdsa(&Message::from_digest(sighash), &x.private_key);
            (sig, PublicKey::from_secret_key(secp, &x.private_key))
        })
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
        self.with_key(path, |secp, x| {
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
        self.with_key(path, ExtendedPubKey::from_priv)
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
        self.with_key(path, |_, x| {
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

    #[test]
    fn coinjoin_path_check() {
        let ok = DerivationPath::from_str("m/9'/1'/4'/0'/0/1").unwrap();
        assert!(is_coinjoin_path(&ok, Network::Testnet));
        assert!(!is_coinjoin_path(&ok, Network::Mainnet));
        let main = DerivationPath::from_str("m/9'/5'/4'/0'").unwrap();
        assert!(is_coinjoin_path(&main, Network::Mainnet));
        for bad in [
            "m/44'/1'/0'/0/0",
            "m/9'/1'/5'/0'",
            "m/9'/1'/4'",
            "m/9/1'/4'/0'",
        ] {
            let p = DerivationPath::from_str(bad).unwrap();
            assert!(!is_coinjoin_path(&p, Network::Testnet), "{bad}");
        }
    }
}
