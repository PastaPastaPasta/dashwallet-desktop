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
use dw_vault::{GrantKind, LockState, Vault, VaultError, WalletSecret, WalletSigner};
use key_wallet::mnemonic::Language;
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use key_wallet::wallet::managed_wallet_info::transaction_building::AccountTypePreference;
use zeroize::Zeroizing;

use crate::platform::runtime::PlatformSignal;

/// Tests: the next wallet registration of the session on this data
/// directory fails.
#[cfg(test)]
pub(crate) static FAIL_REGISTRATION_IN: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Tests: whether [`FAIL_REGISTRATION_IN`] refuses this registration.
fn registration_refused(data_dir: &std::path::Path) -> bool {
    #[cfg(test)]
    return FAIL_REGISTRATION_IN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take_if(|dir| dir == data_dir)
        .is_some();
    #[cfg(not(test))]
    {
        let _ = data_dir;
        false
    }
}
use crate::wallets::validate_name;
use crate::{CreatedWallet, EngineError, EngineEvent, NetworkSession, WalletId};

/// Address lookahead of restores in Dash Core compatibility mode: dash-qt
/// restores scan 1000 keys ahead (QT-105), which also finds funds Core mixed
/// on its BIP44 chains (DESIGN.md R2).
pub const CORE_COMPAT_LOOKAHEAD: u32 = 1000;
/// Largest lookahead key-wallet supports (`MAX_GAP_LIMIT`).
pub const MAX_LOOKAHEAD: u32 = key_wallet::gap_limit::MAX_GAP_LIMIT;
/// dw-appdb setting (wallet scope) holding a raised lookahead, applied again
/// whenever the session opens (key-wallet keeps the gap limit in memory).
const LOOKAHEAD_SETTING: &str = "bip44.lookahead";

/// app.sqlite setting of "require authentication for every payment", one
/// per vault: this prefix and the vault id in hex. `"0"` is off; absent
/// (the default) or anything else is on.
const PAYMENT_AUTH_KEY: &str = "auth.every_payment/";

fn payment_auth_key(vault_id: &[u8]) -> String {
    format!("{PAYMENT_AUTH_KEY}{}", hex::encode(vault_id))
}

/// "Require authentication for every payment" as read for one vault
/// ([`NetworkSession::payment_authentication`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentAuthentication {
    pub on: bool,
    /// The vault it was read for ([`Vault::vault_id`]); `None` without a
    /// vault (the default, on). A host that issues a grant later checks the
    /// vault is still this one.
    pub vault_id: Option<Vec<u8>>,
}

/// Who schedules the automatic backup of an imported wallet and signals its
/// DashPay bring-up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AutoBackup {
    /// The import does both once the wallet is registered.
    Schedule,
    /// The caller does both (or neither) when its whole operation
    /// succeeded: a restore that rolls the wallet back must not have started
    /// a bring-up for it (review DW-E0-05-r1 M4).
    Caller,
}

/// Where an imported seed comes from: a wallet created here gets the short
/// DashPay bring-up budget (`platform::CREATED_HERE_BUDGET`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Origin {
    CreatedHere,
    Imported,
}

/// Options of [`NetworkSession::import_wallet`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// First block to scan. `Some(0)` = genesis; `None` = SPV tip, or the
    /// latest checkpoint when SPV is not running (right for a new phrase).
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 quirks (weak checksum, no NFKD,
    /// salt cut at 256 bytes; QT-104).
    pub core_compat: bool,
    /// Display name (1–64 characters after trimming); `None` = "Wallet N".
    pub name: Option<String>,
    /// Address lookahead (gap limit) of the BIP44 account's receive and
    /// change chains, 1..=1000. `None` = the default (30), or
    /// [`CORE_COMPAT_LOOKAHEAD`] with `core_compat`, as dash-qt keeps a
    /// 1000-key pool. A raised gap is stored with the wallet and applied
    /// again each time the session opens.
    pub lookahead: Option<u32>,
}

impl ImportOptions {
    fn effective_lookahead(&self) -> Option<u32> {
        self.lookahead
            .or(self.core_compat.then_some(CORE_COMPAT_LOOKAHEAD))
    }
}

/// What the blocking part of an import stored.
struct Stored {
    wallet_id: WalletId,
    seed: Zeroizing<[u8; 64]>,
    /// The vault already held this wallet's seed.
    had_secret: bool,
    /// Hex BIP32 fingerprint of the master key.
    fingerprint: String,
}

pub(crate) fn mnemonic_error(e: MnemonicError) -> EngineError {
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

/// Raises the receive/change lookahead of BIP44 account 0 and of the DIP9
/// CoinJoin account 0 to `gap` (DESIGN.md R2: Core-mixed funds sit on the
/// BIP44 chains, dashj-mixed funds on the CoinJoin account; both need the
/// restore gap).
pub(crate) async fn apply_lookahead(
    wallet: &platform_wallet::wallet::platform_wallet::PlatformWallet,
    gap: u32,
) -> Result<(), EngineError> {
    for account in [
        AccountTypePreference::BIP44,
        AccountTypePreference::CoinJoin,
    ] {
        wallet.core().set_gap_limit(account, 0, gap).await?;
    }
    Ok(())
}

impl NetworkSession {
    /// The vault of this session's network.
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// Runs a vault operation on the engine's blocking pool (Argon2id, file
    /// writes and OS secret store calls block) and emits `VaultLockState`
    /// when it changed the lock state.
    ///
    /// E0-04: it first waits while a lock's vault gate is pending, so an
    /// unlock issued after a lock's call is ordered after that lock (H14).
    /// It compares the vault's epoch before and after (H9): a call that
    /// ended the epoch moves every lease to `NeedsGrant` and re-creates
    /// the background leases where the vault stays prompt-free. Calls that
    /// end the epoch on purpose (encrypt, passphrase change, recover,
    /// destroy) go through [`Self::revoking_vault_op`], or for a passphrase
    /// change [`Self::change_passphrase`], instead.
    pub async fn vault_op<T, F>(self: &Arc<Self>, f: F) -> Result<T, EngineError>
    where
        F: FnOnce(&Vault) -> Result<T, VaultError> + Send + 'static,
        T: Send + 'static,
    {
        let this = Arc::clone(self);
        let _op = self.enter().await?;
        self.manager()?;
        self.leases.wait_gates().await;
        let (out, epoch_changed) = self
            .rt
            .spawn_blocking(move || {
                let before = this.vault.lock_state();
                let epoch = this.vault.epoch();
                let out = f(&this.vault);
                this.emit_lock_state_change(before);
                let after = this.vault.epoch();
                if after != epoch {
                    this.leases.epoch_changed(after);
                }
                (out.map_err(EngineError::from), after != epoch)
            })
            .await?;
        if epoch_changed {
            // Inline: the call returns with no task still holding the
            // session, and an unlock returns with its background leases.
            self.ensure_background().await;
        }
        out
    }

    /// "Require authentication for every payment" (DESIGN-opus §1.8; default
    /// on). While on, a host issues `Spend` and `PlatformOp` grants on an
    /// encrypted vault only against a credential, never `Credential::None`
    /// (dw-app enforces it at its IPC boundary). Stored in this network's
    /// app.sqlite under the vault's id, so script in a webview cannot change
    /// it and every new vault (after `destroy`, or a recovery) starts on.
    pub async fn payment_authentication(
        self: &Arc<Self>,
    ) -> Result<PaymentAuthentication, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let Some(vault_id) = this.vault().vault_id() else {
                return Ok(PaymentAuthentication {
                    on: true,
                    vault_id: None,
                });
            };
            let key = payment_auth_key(&vault_id);
            let on = this
                .appdb_op(move |db| {
                    Ok(db.setting(dw_appdb::GLOBAL_SCOPE, &key)?.as_deref() != Some("0"))
                })
                .await?;
            Ok(PaymentAuthentication {
                on,
                vault_id: Some(vault_id),
            })
        })
        .await
    }

    /// Turns "require authentication for every payment" on or off. Either
    /// way needs a `ChangeCredential` grant, redeemed before the setting is
    /// written, on an encrypted vault, so the passphrase:
    /// `VaultError::NotEncrypted` without one. A vault with no passphrase
    /// has no credential to ask for, and a value stored then would carry
    /// over, unasked, to the vault once encrypted. The value is stored for
    /// the vault that redeemed the grant.
    pub async fn set_payment_authentication(
        self: &Arc<Self>,
        on: bool,
        grant_id: String,
    ) -> Result<(), EngineError> {
        let vault_id = self
            .vault_op(move |v| {
                if !v.status().encrypted {
                    return Err(VaultError::NotEncrypted);
                }
                let vault_id = v.vault_id().ok_or(VaultError::NoVault)?;
                v.redeem_grant(&grant_id, GrantKind::ChangeCredential, None)?;
                Ok(vault_id)
            })
            .await?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let key = payment_auth_key(&vault_id);
            this.appdb_op(move |db| {
                db.set_setting(dw_appdb::GLOBAL_SCOPE, &key, (!on).then_some("0"))
            })
            .await
        })
        .await
    }

    /// Dash Core's `walletpassphrase` timer (console, QT-145): locks the
    /// vault `after` from now. One timer per session: a later call replaces
    /// the pending one (as Core's named RPC timer), `lock_vault` and closing
    /// the session cancel it. The timer holds only a weak reference to the
    /// session (review L1).
    pub fn relock_after(self: &Arc<Self>, after: std::time::Duration) {
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let session = Arc::downgrade(self);
        let mut slot = self.relock.lock().unwrap_or_else(|p| p.into_inner());
        let task = self.rt.spawn(async move {
            tokio::time::sleep(after).await;
            let Some(s) = session.upgrade() else { return };
            let mine = {
                let mut slot = s.relock.lock().unwrap_or_else(|p| p.into_inner());
                match &*slot {
                    Some((g, _)) if *g == generation => slot.take().is_some(),
                    _ => false,
                }
            };
            if mine && let Ok(_op) = s.try_enter() {
                s.lock_now();
            }
        });
        if let Some((_, old)) = slot.replace((generation, task.abort_handle())) {
            old.abort();
        }
    }

    /// Cancels a pending [`Self::relock_after`] timer.
    pub(crate) fn cancel_relock(&self) {
        let pending = self.relock.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some((_, task)) = pending {
            task.abort();
        }
    }

    /// Whether a [`Self::relock_after`] timer is pending.
    pub fn relock_pending(&self) -> bool {
        self.relock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    pub(crate) fn emit_lock_state_change(&self, before: LockState) {
        let state = self.vault.lock_state();
        if state != before {
            self.platform.note_lock_state(before, state);
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
        let phrase =
            mnemonic::generate(word_count.into(), Language::English).map_err(mnemonic_error)?;
        let secret = Zeroizing::new(phrase.to_vec());
        let wallet_id = self
            .import_secret_inner(
                ImportOptions::default(),
                move || mnemonic::derive_secret(&secret, b"", false).map_err(mnemonic_error),
                AutoBackup::Schedule,
                Origin::CreatedHere,
            )
            .await?;
        // The import took and released the operation guard.
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
    ///   which attaches the keys, and the call succeeds (`WalletCreated`).
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
        let core_compat = options.core_compat;
        self.import_secret_with(options, move || {
            mnemonic::derive_secret(&phrase, &bip39_passphrase, core_compat).map_err(mnemonic_error)
        })
        .await
    }

    /// [`Self::import_wallet`] for a secret built by `make_secret` (a phrase,
    /// a raw seed from a Dash Core file, a restored backup). `make_secret`
    /// runs on the blocking pool; everything after it follows the same
    /// seed-safety order and the same rules for registered wallets.
    pub(crate) async fn import_secret_with<F>(
        self: &Arc<Self>,
        options: ImportOptions,
        make_secret: F,
    ) -> Result<WalletId, EngineError>
    where
        F: FnOnce() -> Result<WalletSecret, EngineError> + Send + 'static,
    {
        self.import_secret_inner(options, make_secret, AutoBackup::Schedule, Origin::Imported)
            .await
    }

    /// [`Self::import_secret_with`] with the automatic backup left to the
    /// caller when `auto_backup` is [`AutoBackup::Caller`] (a restore
    /// schedules it only once every bundle has succeeded), for a seed of
    /// `origin`.
    pub(crate) async fn import_secret_inner<F>(
        self: &Arc<Self>,
        options: ImportOptions,
        make_secret: F,
        auto_backup: AutoBackup,
        origin: Origin,
    ) -> Result<WalletId, EngineError>
    where
        F: FnOnce() -> Result<WalletSecret, EngineError> + Send + 'static,
    {
        let name = options.name.as_deref().map(validate_name).transpose()?;
        let lookahead = options.effective_lookahead();
        if let Some(n) = lookahead
            && !(1..=MAX_LOOKAHEAD).contains(&n)
        {
            return Err(EngineError::InvalidArgument(format!(
                "lookahead {n} outside 1..={MAX_LOOKAHEAD}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let network = this.network.core_network();
            let vault = this.vault.clone();
            // PBKDF2, vault file writes and fsync stay off the async workers.
            let Stored {
                wallet_id,
                seed,
                had_secret,
                fingerprint,
            } = tokio::task::spawn_blocking(move || -> Result<Stored, EngineError> {
                let secret = make_secret()?;
                let id =
                    mnemonic::wallet_id_for_seed(&secret.seed, network).map_err(mnemonic_error)?;
                // The vault keeps a seed it holds: an import of the same seed
                // (the id derives from it) stored meanwhile, perhaps with
                // another derivation flag, counts as one already there.
                let had_secret = vault.has_wallet_secret(&id)
                    || match vault.store_wallet_secret(&id, &secret) {
                        Err(VaultError::WalletSecretExists) => true,
                        stored => {
                            stored?;
                            false
                        }
                    };
                // Kept for PSBT derivation records (watch-only use, no key needed).
                let fingerprint =
                    key_wallet::bip32::ExtendedPrivKey::new_master(network, &secret.seed[..])
                        .map(|m| m.fingerprint().to_string())
                        .map_err(|e| EngineError::Internal(format!("master key: {e}")))?;
                Ok(Stored {
                    wallet_id: WalletId(id),
                    seed: Zeroizing::new(*secret.seed),
                    had_secret,
                    fingerprint,
                })
            })
            .await??;
            // Rows of a removed wallet are skipped until it is readmitted
            // (DEC-210), before its first write.
            let readmit = wallet_id.to_string();
            let readmitted = match this.appdb_op(move |db| db.readmit_wallet(&readmit)).await {
                Ok(readmitted) => readmitted,
                Err(e) => {
                    if !had_secret {
                        this.forget_secret(wallet_id).await;
                    }
                    return Err(e);
                }
            };
            this.store_fingerprint(wallet_id, fingerprint).await;

            // A closed (unloaded) wallet is registered: open it instead.
            if this.live()?.store.is_unloaded(&wallet_id.0) {
                if !had_secret {
                    this.forget_secret(wallet_id).await;
                }
                return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
            }
            if manager.get_wallet(&wallet_id.0).await.is_some() {
                if had_secret {
                    return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
                }
                // Keys attached: watch-only becomes false; hosts reload.
                this.sink.emit(EngineEvent::WalletCreated {
                    network: this.network.clone(),
                    wallet_id,
                });
                if auto_backup == AutoBackup::Schedule {
                    this.platform.signal(PlatformSignal::WalletAdded(wallet_id));
                    // E0-04 §8.6 (review L-4): as `load_wallet`.
                    this.leases.wallet_opened(wallet_id);
                    this.schedule_automatic_backup(wallet_id);
                }
                return Ok(wallet_id);
            }

            let default_name = this.next_default_name();
            let registered = if registration_refused(this.data_dir()) {
                Err(EngineError::Internal("test: registration refused".into()))
            } else {
                manager
                    .create_wallet_from_seed_bytes(
                        network,
                        &seed,
                        WalletAccountCreationOptions::Default,
                        options.birth_height,
                    )
                    .await
                    .map_err(EngineError::from)
            };
            drop(seed);
            let wallet = match registered {
                Ok(wallet) if wallet.wallet_id() == wallet_id.0 => wallet,
                Ok(wallet) => {
                    return Err(EngineError::Internal(format!(
                        "platform-wallet registered {} for the seed of {wallet_id}",
                        hex::encode(wallet.wallet_id())
                    )));
                }
                Err(e) => {
                    // A concurrent registration of the same wallet now uses
                    // the stored seed, so only roll back our own failure.
                    if !matches!(e, EngineError::WalletAlreadyExists(_)) {
                        if !had_secret {
                            this.forget_secret(wallet_id).await;
                        }
                        if readmitted {
                            this.record_removed_again(wallet_id).await;
                        }
                    }
                    return Err(e);
                }
            };
            if let Some(gap) = lookahead {
                if let Err(e) = apply_lookahead(&wallet, gap).await {
                    tracing::warn!(%wallet_id, error = %e, "could not raise the restore lookahead");
                }
                this.store_lookahead(wallet_id, gap).await;
            }
            this.refresh_wallet_state(&manager, wallet_id).await;
            // The wallet is registered; a failed name write leaves it with
            // the computed default name and is reported in the log only.
            if let Err(e) = this
                .store_name(wallet_id, name.unwrap_or(default_name), None)
                .await
            {
                tracing::warn!(%wallet_id, error = %e, "could not store the wallet name");
            }
            // Before the bring-up hears of the wallet, which reads it.
            if origin == Origin::CreatedHere {
                this.mark_created_here(wallet_id).await;
            }
            this.sink.emit(EngineEvent::WalletCreated {
                network: this.network.clone(),
                wallet_id,
            });
            if auto_backup == AutoBackup::Schedule {
                this.platform.signal(PlatformSignal::WalletAdded(wallet_id));
                // E0-04 §8.6 (review L-4): its funding statuses read and
                // its tracked rows resumed, as `load_wallet` does.
                this.leases.wallet_opened(wallet_id);
                this.schedule_automatic_backup(wallet_id);
            }
            Ok(wallet_id)
        })
        .await
    }

    pub(crate) async fn store_lookahead(&self, wallet_id: WalletId, gap: u32) {
        let Ok(live) = self.live() else { return };
        let key = wallet_id.to_string();
        let stored = tokio::task::spawn_blocking(move || {
            live.appdb
                .set_setting(&key, LOOKAHEAD_SETTING, Some(&gap.to_string()))
        })
        .await;
        if !matches!(stored, Ok(Ok(()))) {
            tracing::warn!(%wallet_id, "could not store the lookahead; it lasts until restart");
        }
    }

    /// Applies the stored lookahead of every wallet that has one (session
    /// open). Failures are logged; the wallet keeps the default gap.
    pub(crate) async fn apply_stored_lookaheads(&self, manager: &crate::session::Manager) {
        let Ok(live) = self.live() else { return };
        for id in manager.list_wallet_ids_blocking() {
            let key = hex::encode(id);
            let appdb = std::sync::Arc::clone(&live.appdb);
            let stored =
                tokio::task::spawn_blocking(move || appdb.setting(&key, LOOKAHEAD_SETTING))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|g| (1..=MAX_LOOKAHEAD).contains(g));
            let (Some(gap), Some(wallet)) = (stored, manager.get_wallet(&id).await) else {
                continue;
            };
            if let Err(e) = apply_lookahead(&wallet, gap).await {
                tracing::warn!(wallet_id = %hex::encode(id), error = %e, "could not apply the stored lookahead");
            }
        }
    }

    /// Stores the master key fingerprint of a wallet (dw-appdb, wallet
    /// scope); a failure only costs PSBT derivation records.
    async fn store_fingerprint(&self, wallet_id: WalletId, fingerprint: String) {
        let Ok(live) = self.live() else { return };
        let key = wallet_id.to_string();
        let stored = tokio::task::spawn_blocking(move || {
            live.appdb.set_setting(
                &key,
                crate::send::psbt::FINGERPRINT_SETTING,
                Some(&fingerprint),
            )
        })
        .await;
        if !matches!(stored, Ok(Ok(()))) {
            tracing::warn!(%wallet_id, "could not store the master key fingerprint");
        }
    }

    /// A removed wallet whose import failed after it was readmitted is
    /// recorded as removed again, with what the import wrote of it
    /// (DEC-210).
    async fn record_removed_again(&self, wallet_id: WalletId) {
        let id = wallet_id.to_string();
        if let Err(e) = self.appdb_op(move |db| db.delete_wallet(&id)).await {
            tracing::warn!(%wallet_id, error = %e, "could not record the wallet as removed again");
        }
    }

    /// Deletes the vault records of a wallet whose registration failed.
    pub(crate) async fn forget_secret(&self, wallet_id: WalletId) {
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
    /// Needs a `SignMessage` grant bound to `wallet_id`. The grant is
    /// redeemed only after the address checks pass, so a mistyped address
    /// does not consume it. The key is derived from the vault seed by
    /// `VaultSigner` and erased after signing; a passphrase grant on a locked
    /// vault signs with its own key and leaves the vault locked.
    pub async fn sign_message(
        self: &Arc<Self>,
        wallet_id: WalletId,
        address: String,
        message: Vec<u8>,
        grant_id: String,
    ) -> Result<String, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
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
                let token =
                    vault.redeem_grant(&grant_id, GrantKind::SignMessage, Some(&wallet_id.0))?;
                vault.signer(&wallet_id.0, &token)
            })
            .await??;
            Ok(signer.sign_message(&path, &message).await?)
        })
        .await
    }
}
