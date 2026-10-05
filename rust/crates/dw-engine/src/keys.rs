//! Wallet key material: vault-backed wallet creation and import, vault
//! operations with lock-state events, and message signing through dw-vault's
//! `VaultSigner`.
//!
//! platform-wallet registers every wallet external-signable (it keeps no
//! private keys), so the seed in the vault is the only key material. The seed
//! is therefore written and read back before platform-wallet registers the
//! wallet (DESIGN-opus §1.8 "seed safety ordering"); no path registers a
//! wallet whose seed is not in the vault.

use std::str::FromStr;
use std::sync::Arc;

use dashcore::Address;
use dw_uri::keyio::{Destination, decode_destination};
use dw_vault::MnemonicError;
use dw_vault::mnemonic;
use dw_vault::{GrantKind, LockState, Vault, VaultError, VaultStatus, WalletSigner};
use key_wallet::mnemonic::Language;
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use zeroize::Zeroizing;

use crate::{CreatedWallet, EngineError, EngineEvent, NetworkSession, WalletId};

/// Options of [`NetworkSession::import_wallet`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// First block to scan. `Some(0)` = genesis; `None` = SPV tip, or the
    /// latest checkpoint when SPV is not running (right for a new phrase).
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 quirks (weak checksum, no NFKD,
    /// salt cut at 256 bytes; QT-104).
    pub core_compat: bool,
}

fn mnemonic_error(e: MnemonicError) -> EngineError {
    match e {
        MnemonicError::Invalid(detail) => EngineError::InvalidMnemonic(detail),
        MnemonicError::UnsupportedWordCount(n) => {
            EngineError::InvalidArgument(format!("unsupported word count {n}"))
        }
        MnemonicError::PassphraseNotUtf8 => {
            EngineError::InvalidArgument("BIP39 passphrase is not UTF-8".into())
        }
        MnemonicError::Entropy(detail) => EngineError::Internal(format!("entropy: {detail}")),
    }
}

impl NetworkSession {
    /// The vault of this session's network.
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// Runs a vault operation on the engine's blocking pool (Argon2id, file
    /// writes and OS secret store calls block) and emits `VaultLockState`
    /// when it changed the lock state.
    pub async fn vault_op<T, F>(self: &Arc<Self>, f: F) -> Result<T, EngineError>
    where
        F: FnOnce(&Vault) -> Result<T, VaultError> + Send + 'static,
        T: Send + 'static,
    {
        self.manager()?;
        let this = Arc::clone(self);
        self.rt
            .spawn_blocking(move || {
                let before = this.vault.lock_state();
                let out = f(&this.vault);
                this.emit_lock_state_change(before);
                out.map_err(EngineError::from)
            })
            .await?
    }

    /// Drops the vault's data key and revokes every grant. In-memory; never
    /// blocks.
    pub fn lock_vault(&self) -> Result<VaultStatus, EngineError> {
        self.manager()?;
        let before = self.vault.lock_state();
        let status = self.vault.lock();
        self.emit_lock_state_change(before);
        Ok(status)
    }

    fn emit_lock_state_change(&self, before: LockState) {
        let state = self.vault.lock_state();
        if state != before {
            self.sink.emit(EngineEvent::VaultLockState {
                network: self.network.clone(),
                state,
            });
        }
    }

    /// Creates a wallet from a fresh English phrase of `word_count` words
    /// (12, 15, 18, 21 or 24). The seed is stored in the vault exactly as
    /// [`Self::import_wallet`] stores it; the phrase is also returned so a
    /// headless host can show it. Needs a vault that holds its data key.
    pub async fn create_wallet(
        self: &Arc<Self>,
        word_count: u8,
    ) -> Result<CreatedWallet, EngineError> {
        let phrase = mnemonic::generate(word_count.into(), Language::English)
            .map_err(mnemonic_error)?;
        let wallet_id = self
            .import_wallet(
                Zeroizing::new(phrase.to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions::default(),
            )
            .await?;
        let text = std::str::from_utf8(&phrase)
            .map_err(|_| EngineError::Internal("generated phrase is not UTF-8".into()))?;
        Ok(CreatedWallet {
            wallet_id,
            mnemonic: Zeroizing::new(text.to_owned()),
        })
    }

    /// Adds the wallet of `phrase` + `bip39_passphrase` on this network.
    ///
    /// Order (DESIGN-opus §1.8): derive the seed, store phrase, passphrase
    /// and seed in the vault and read them back, then register the wallet
    /// with platform-wallet from the stored seed. A failed registration
    /// deletes the records it wrote.
    ///
    /// When the wallet is already registered:
    /// - and the vault holds its seed: `WalletAlreadyExists`;
    /// - without a seed (keys lost or never stored): the seed is stored,
    ///   which attaches the keys, and the call succeeds (`WalletChanged`).
    ///
    /// Errors: `InvalidMnemonic`, `InvalidArgument` (passphrase not UTF-8 in
    /// strict mode), `Vault(NoVault | Locked | MixingOnly | …)`.
    ///
    /// Counterpart: `platform_wallet_manager_create_wallet_from_seed_bytes`.
    pub async fn import_wallet(
        self: &Arc<Self>,
        phrase: Zeroizing<Vec<u8>>,
        bip39_passphrase: Zeroizing<Vec<u8>>,
        options: ImportOptions,
    ) -> Result<WalletId, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let manager = this.manager()?;
            let network = this.network.core_network();
            let vault = this.vault.clone();
            // PBKDF2, vault file writes and fsync stay off the async workers.
            let (wallet_id, seed, had_secret) = tokio::task::spawn_blocking(
                move || -> Result<(WalletId, Zeroizing<[u8; 64]>, bool), EngineError> {
                    let secret = mnemonic::derive_secret(
                        &phrase,
                        &bip39_passphrase,
                        options.core_compat,
                    )
                    .map_err(mnemonic_error)?;
                    let id = mnemonic::wallet_id_for_seed(&secret.seed, network)
                        .map_err(mnemonic_error)?;
                    let had_secret = vault.has_wallet_secret(&id);
                    if !had_secret {
                        vault.store_wallet_secret(&id, &secret)?;
                    }
                    Ok((WalletId(id), Zeroizing::new(*secret.seed), had_secret))
                },
            )
            .await??;

            if manager.get_wallet(&wallet_id.0).await.is_some() {
                if had_secret {
                    return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
                }
                this.sink.emit(EngineEvent::WalletChanged {
                    network: this.network.clone(),
                    wallet_id,
                });
                return Ok(wallet_id);
            }

            let registered = manager
                .create_wallet_from_seed_bytes(
                    network,
                    &seed,
                    WalletAccountCreationOptions::Default,
                    options.birth_height,
                )
                .await;
            drop(seed);
            match registered {
                Ok(wallet) if wallet.wallet_id() == wallet_id.0 => {}
                Ok(wallet) => {
                    return Err(EngineError::Internal(format!(
                        "platform-wallet registered {} for the seed of {wallet_id}",
                        hex::encode(wallet.wallet_id())
                    )));
                }
                Err(e) => {
                    let e = EngineError::from(e);
                    // A concurrent registration of the same wallet now uses
                    // the stored seed, so only roll back our own failure.
                    if !had_secret && !matches!(e, EngineError::WalletAlreadyExists(_)) {
                        this.forget_secret(wallet_id).await;
                    }
                    return Err(e);
                }
            }
            this.sink.emit(EngineEvent::WalletCreated {
                network: this.network.clone(),
                wallet_id,
            });
            Ok(wallet_id)
        })
        .await
    }

    /// Deletes the vault records of a wallet whose registration failed.
    async fn forget_secret(&self, wallet_id: WalletId) {
        let vault = self.vault.clone();
        let deleted = tokio::task::spawn_blocking(move || vault.delete_wallet_secret(&wallet_id.0))
            .await
            .map_err(EngineError::from)
            .and_then(|r| r.map_err(EngineError::from));
        if let Err(e) = deleted {
            tracing::warn!(%wallet_id, error = %e, "could not delete the seed of a wallet that failed to register");
        }
    }

    /// Dash Core `signmessage` (QT-099): signs `message` with the key of
    /// `address`, a P2PKH address of `wallet_id` on this network, and returns
    /// the base64 compact signature.
    ///
    /// Needs a `SignMessage` grant. The grant is redeemed only after the
    /// address checks pass, so a mistyped address does not consume it. The
    /// key is derived from the vault seed by `VaultSigner` and erased after
    /// signing.
    pub async fn sign_message(
        self: &Arc<Self>,
        wallet_id: WalletId,
        address: String,
        message: Vec<u8>,
        grant_id: String,
    ) -> Result<String, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let manager = this.manager()?;
            let network = this.network.core_network();
            match decode_destination(&address, network) {
                Ok(Destination::PubKeyHash(_)) => {}
                Ok(Destination::ScriptHash(_)) => return Err(EngineError::AddressNoKey(address)),
                Err(_) => return Err(EngineError::InvalidAddress(address)),
            }
            let parsed = Address::from_str(&address)
                .ok()
                .and_then(|a| a.require_network(network).ok())
                .ok_or_else(|| EngineError::InvalidAddress(address.clone()))?;
            let wallet = manager
                .get_wallet(&wallet_id.0)
                .await
                .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))?;
            let path = {
                let state = wallet.state().await;
                state
                    .core_wallet
                    .all_managed_accounts()
                    .into_iter()
                    .find_map(|account| account.get_address_info(&parsed))
                    .map(|info| info.path)
            }
            .ok_or_else(|| EngineError::AddressNotMine(address.clone()))?;

            let vault = this.vault.clone();
            let signer = tokio::task::spawn_blocking(move || {
                let token = vault.redeem_grant(&grant_id, GrantKind::SignMessage)?;
                vault.signer(&wallet_id.0, &token)
            })
            .await??;
            Ok(signer.sign_message(&path, &message).await?)
        })
        .await
    }
}
