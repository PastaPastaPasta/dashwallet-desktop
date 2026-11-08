//! The asset-lock input bound (release 0.1, review r2 R2-m3): a lock the
//! engine funds spends at most [`MAX_INPUTS`] coins, on every route that
//! funds one (registration and top-up, `registration_flow.rs`'s
//! `PwLib::fund_lock`; the test-only `platform-addr fund`). The pinned
//! builder bounds a transaction at 500 inputs (key-wallet
//! `transaction_builder.rs:29`) and takes no tighter bound, so the engine
//! applies it twice:
//!
//! - **Before anything is signed** ([`check`]): the fewest of the wallet's
//!   coins, largest first, that reach the lock. No selection takes fewer
//!   (the library's sources are among the wallet's funding accounts), so
//!   past the bound the lock is refused with nothing selected or signed.
//! - **Before the first signature** ([`Capped`]): key-wallet's builder
//!   shows the lock's signer the selected inputs before it signs any
//!   (`Signer::approve_inputs`, `transaction_builder.rs:827`), and a
//!   selection past the bound is refused there: nothing is signed or
//!   released, the builder releases its reservation and nothing reaches
//!   the wire. Behind that, the signer signs at most [`MAX_INPUTS`]
//!   distinct sighashes (one per input), so a selection past the bound
//!   that it was not shown still fails at its next signature, before the
//!   transaction is complete.
//!
//! Either refusal is `invalid_argument` (no code names an input bound on
//! these routes).

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use dashcore::Transaction;
use dashcore::secp256k1::{PublicKey, ecdsa};
use dw_vault::SignerError;
use key_wallet::bip32::{DerivationPath, ExtendedPubKey};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::{ExtendedPubKeySigner, Signer, SignerMethod, Utxo};
use platform_wallet::PlatformWallet;

use super::errors::PlatformError;

/// The most coins one asset lock spends.
pub(crate) const MAX_INPUTS: usize = 50;

/// The refusal: `needs` coins, or more than the bound when unknown.
pub(crate) fn too_many(needs: Option<usize>) -> PlatformError {
    let needs = needs.map_or_else(|| format!("more than {MAX_INPUTS}"), |n| n.to_string());
    // Shown as it is (the top-up's presenter): the advice is the remedy.
    PlatformError::InvalidArgument {
        detail: format!(
            "this payment would use {needs} coins and at most {MAX_INPUTS} can be used: \
             send Dash to your own wallet first to combine them, then try again"
        ),
    }
}

/// Refuses, before anything is selected or signed, a lock of `duffs` that
/// no selection of the wallet's coins funds within the bound.
pub(crate) async fn check(wallet: &PlatformWallet, duffs: u64) -> Result<(), PlatformError> {
    let values: Vec<u64> = {
        let state = wallet.state().await;
        state
            .core_wallet
            .utxos()
            .iter()
            .map(|u| u.value())
            .collect()
    };
    match least(&values, duffs) {
        Some(n) if n > MAX_INPUTS => Err(too_many(Some(n))),
        // Short or within the bound: the library's own answer stands.
        _ => Ok(()),
    }
}

/// The fewest of `values` that reach `duffs`, largest first; `None` when
/// all of them do not.
pub(crate) fn least(values: &[u64], duffs: u64) -> Option<usize> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut sum = 0u64;
    for (n, v) in sorted.into_iter().enumerate() {
        sum = sum.saturating_add(v);
        if sum >= duffs {
            return Some(n + 1);
        }
    }
    None
}

/// A lock's signer that refuses a build of more than [`MAX_INPUTS`]
/// inputs when the builder shows it, and signs at most [`MAX_INPUTS`]
/// distinct sighashes (module doc); a sighash it signed already may be
/// signed again. Once it refused, it signs nothing more.
#[derive(Debug)]
pub(crate) struct Capped<S> {
    inner: S,
    signed: Mutex<BTreeSet<[u8; 32]>>,
    refused: AtomicBool,
    /// The inputs of the build it refused before signing; 0 if none.
    shown: AtomicUsize,
}

impl<S> Capped<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            signed: Mutex::default(),
            refused: AtomicBool::new(false),
            shown: AtomicUsize::new(0),
        }
    }

    /// Whether it refused a build or a signature past the bound.
    pub(crate) fn refused(&self) -> bool {
        self.refused.load(Ordering::SeqCst)
    }

    /// The refusal: the inputs of the build it refused before signing,
    /// when it did.
    pub(crate) fn refusal(&self) -> PlatformError {
        too_many(Some(self.shown.load(Ordering::SeqCst)).filter(|&n| n > 0))
    }
}

#[async_trait]
impl<S: Signer<Error = SignerError>> Signer for Capped<S> {
    type Error = SignerError;

    fn supported_methods(&self) -> &[SignerMethod] {
        self.inner.supported_methods()
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<(ecdsa::Signature, PublicKey), SignerError> {
        if self.refused() {
            return Err(SignerError::PathNotAllowed(format!(
                "{path}: the asset lock's build was refused for its inputs"
            )));
        }
        {
            let mut signed = self.signed.lock().unwrap_or_else(|p| p.into_inner());
            if !signed.contains(&sighash) {
                if signed.len() >= MAX_INPUTS {
                    self.refused.store(true, Ordering::SeqCst);
                    return Err(SignerError::PathNotAllowed(format!(
                        "{path}: an asset lock spends at most {MAX_INPUTS} coins"
                    )));
                }
                signed.insert(sighash);
            }
        }
        self.inner.sign_ecdsa(path, sighash).await
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
        self.inner.public_key(path).await
    }

    async fn approve_inputs(&self, tx: &Transaction, inputs: &[Utxo]) -> Result<(), SignerError> {
        if inputs.len() > MAX_INPUTS {
            self.shown.store(inputs.len(), Ordering::SeqCst);
            self.refused.store(true, Ordering::SeqCst);
            return Err(SignerError::PathNotAllowed(format!(
                "{} inputs: an asset lock spends at most {MAX_INPUTS} coins",
                inputs.len()
            )));
        }
        self.inner.approve_inputs(tx, inputs).await
    }
}

#[async_trait]
impl<S: ExtendedPubKeySigner + Signer<Error = SignerError>> ExtendedPubKeySigner for Capped<S> {
    async fn extended_public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, SignerError> {
        self.inner.extended_public_key(path).await
    }
}

#[cfg(test)]
#[path = "lock_inputs_tests.rs"]
mod tests;
