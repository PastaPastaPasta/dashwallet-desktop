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
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use zeroize::Zeroizing;

use crate::gate::Lifecycle;
use crate::platform::runtime::PlatformSignal;
use crate::wallets::validate_name;
use crate::{CreatedWallet, EngineError, EngineEvent, NetworkSession, WalletId};

/// Tests: the next wallet registration of a session on one of these data
/// directories fails.
#[cfg(test)]
pub(crate) static FAIL_REGISTRATION_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: the next seed import of a session on one of these data
/// directories reports a failure after registering the wallet.
#[cfg(test)]
pub(crate) static FAIL_AFTER_REGISTRATION_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: whether `list` names `data_dir`, taking it out.
#[cfg(test)]
pub(crate) fn take_test_failure(
    list: &std::sync::Mutex<Vec<std::path::PathBuf>>,
    data_dir: &std::path::Path,
) -> bool {
    let mut dirs = list.lock().unwrap_or_else(|p| p.into_inner());
    let before = dirs.len();
    dirs.retain(|dir| dir != data_dir);
    dirs.len() < before
}

/// Tests: whether [`FAIL_REGISTRATION_IN`] refuses this registration.
#[cfg(test)]
pub(crate) fn registration_refused(data_dir: &std::path::Path) -> bool {
    take_test_failure(&FAIL_REGISTRATION_IN, data_dir)
}

#[cfg(not(test))]
pub(crate) fn registration_refused(_: &std::path::Path) -> bool {
    false
}

/// Address lookahead of restores in Dash Core compatibility mode: dash-qt
/// restores scan 1000 keys ahead (QT-105), which also finds funds Core mixed
/// on its BIP44 chains (DESIGN.md R2).
pub const CORE_COMPAT_LOOKAHEAD: u32 = 1000;
/// CoinJoin scan window on restore and rescan. The account's default gap
/// derives indexes 0 through 99 only, so a key used at index 100 or beyond
/// is invisible to a filter scan until an earlier address is marked used.
/// This is the largest gap key-wallet will keep, applied to the CoinJoin
/// account alone.
pub(crate) const COINJOIN_SCAN_GAP: u32 = MAX_LOOKAHEAD;
/// Largest lookahead key-wallet supports (`MAX_GAP_LIMIT`).
pub const MAX_LOOKAHEAD: u32 = key_wallet::gap_limit::MAX_GAP_LIMIT;
/// dw-appdb setting (wallet scope) holding a raised lookahead, applied again
/// whenever the session opens (key-wallet keeps the gap limit in memory).
const LOOKAHEAD_SETTING: &str = "bip44.lookahead";
/// dw-appdb setting (local wallet scope) saying whether the wallet's history
/// is known to have been scanned with [`COINJOIN_SCAN_GAP`] and its raised
/// lookahead ([`WideScan`]). Absent on a wallet an older build scanned.
pub(crate) const WIDE_SCAN_SETTING: &str = "coinjoin.wide_scan";

/// What [`WIDE_SCAN_SETTING`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WideScan {
    /// Every block up to the stored checkpoint was matched with wide pools.
    Done,
    /// Blocks above this checkpoint may have been matched with narrow pools.
    From(u32),
    /// Blocks above the wallet's birth checkpoint may have been.
    Birth,
}

impl WideScan {
    fn parse(value: &str) -> Self {
        match value {
            "done" => Self::Done,
            "birth" => Self::Birth,
            height => height.parse().map_or(Self::Birth, Self::From),
        }
    }

    fn encode(self) -> String {
        match self {
            Self::Done => "done".into(),
            Self::Birth => "birth".into(),
            Self::From(height) => height.to_string(),
        }
    }
}

/// Tests: wallets whose CoinJoin pools [`ensure_coinjoin_scan_gap`] fails to
/// widen.
#[cfg(test)]
pub(crate) static FAIL_WIDEN: std::sync::Mutex<Vec<[u8; 32]>> = std::sync::Mutex::new(Vec::new());

/// app.sqlite setting of "ask for my passphrase for every payment", one
/// per vault: this prefix and the vault id in hex.
///
/// `"strict"` asks every time. `"window"` is the five-minute window.
/// A vault from before the window existed stored nothing when the choice
/// was the old default (ask every time) and `"0"` when the user turned
/// that off. Absence and any other value stay strict, so an upgrade does
/// not start signing without a passphrase. A vault created by this build
/// writes `"window"` itself. `"0"` was an explicit choice to stop asking,
/// and it now means the window rather than an unlimited unlock.
const PAYMENT_AUTH_KEY: &str = "auth.every_payment/";

/// Whether the stored value asks for the passphrase on every payment.
pub(crate) fn asks_every_payment(stored: Option<&str>) -> bool {
    !matches!(stored, Some("window") | Some("0"))
}

fn payment_auth_key(vault_id: &[u8]) -> String {
    format!("{PAYMENT_AUTH_KEY}{}", hex::encode(vault_id))
}

/// "Require authentication for every payment" as read for one vault
/// ([`NetworkSession::payment_authentication`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentAuthentication {
    pub on: bool,
    /// The vault it was read for ([`Vault::vault_id`]); `None` without a
    /// vault, which reads as on. A host that issues a grant later checks the
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

/// The options of an import, checked before its secret is made.
pub(crate) struct Checked {
    name: Option<String>,
    lookahead: Option<u32>,
    birth_height: Option<u32>,
}

impl Checked {
    pub(crate) fn new(options: ImportOptions) -> Result<Self, EngineError> {
        let name = options.name.as_deref().map(validate_name).transpose()?;
        let lookahead = options.effective_lookahead();
        if let Some(n) = lookahead
            && !(1..=MAX_LOOKAHEAD).contains(&n)
        {
            return Err(EngineError::InvalidArgument(format!(
                "lookahead {n} outside 1..={MAX_LOOKAHEAD}"
            )));
        }
        Ok(Self {
            name,
            lookahead,
            birth_height: options.birth_height,
        })
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

/// Raises the receive/change lookahead of the wallet's BIP44 accounts and of
/// its DIP9 CoinJoin accounts to `gap` (DESIGN.md R2: Core-mixed funds sit on the
/// BIP44 chains, dashj-mixed funds on the CoinJoin account; both need the
/// restore gap). An account that fails doesn't stop the others; the first
/// error is returned.
pub(crate) async fn apply_lookahead(
    wallet: &platform_wallet::wallet::platform_wallet::PlatformWallet,
    gap: u32,
) -> Result<(), EngineError> {
    // Every one the wallet has: a watch-only wallet's is BIP44 account N.
    let accounts: Vec<(AccountTypePreference, u32)> = {
        let state = wallet.state().await;
        let accounts = &state.core_wallet.accounts;
        (accounts.standard_bip44_accounts.keys())
            .map(|i| (AccountTypePreference::BIP44, *i))
            .chain(
                (accounts.coinjoin_accounts.keys()).map(|i| (AccountTypePreference::CoinJoin, *i)),
            )
            .collect()
    };
    let mut first_error = None;
    for (account, index) in accounts {
        if let Err(e) = wallet.core().set_gap_limit(account, index, gap).await {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), |e| Err(e.into()))
}

/// Raises each CoinJoin account of open wallet `id` to [`COINJOIN_SCAN_GAP`]
/// when a pool of it is narrower, so a restore or rescan derives index 100
/// and beyond. BIP44 is untouched. A wallet no longer open is skipped.
pub(crate) async fn ensure_coinjoin_scan_gap(
    manager: &crate::session::Manager,
    id: WalletId,
) -> Result<(), EngineError> {
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    #[cfg(test)]
    if FAIL_WIDEN.lock().unwrap().contains(&id.0) {
        return Err(EngineError::Internal("test: widening refused".into()));
    }
    let narrow: Vec<u32> = {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&id.0) else {
            return Ok(());
        };
        (info.core_wallet.accounts.coinjoin_accounts.iter())
            .filter(|(_, account)| {
                (account.managed_account_type().address_pools().iter())
                    .any(|pool| pool.gap_limit < COINJOIN_SCAN_GAP)
            })
            .map(|(index, _)| *index)
            .collect()
    };
    if narrow.is_empty() {
        return Ok(());
    }
    let Some(wallet) = manager.get_wallet(&id.0).await else {
        return Ok(());
    };
    let mut first_error = None;
    for index in narrow {
        if let Err(e) = (wallet.core())
            .set_gap_limit(AccountTypePreference::CoinJoin, index, COINJOIN_SCAN_GAP)
            .await
        {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), |e| Err(e.into()))
}

/// The filter checkpoint open wallet `id` starts its scan from: one below
/// its birth height (0 for a wallet no longer open).
pub(crate) async fn birth_checkpoint(manager: &crate::session::Manager, id: WalletId) -> u32 {
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    (wm.get_wallet_info(&id.0)).map_or(0, |info| info.core_wallet.birth_height().saturating_sub(1))
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

    /// "Ask for my passphrase for every payment". While on, a host issues a
    /// payment grant on an encrypted vault only against a credential (dw-app
    /// enforces it at its IPC boundary). While off, the vault's signing
    /// window may issue one, up to the sealed spend limit for the whole
    /// window. Stored per vault id. A vault that has no row yet is on:
    /// that is how every vault from before the window stored the default.
    /// [`Self::adopt_signing_window_default`] writes the window for a vault
    /// this build creates.
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
                    Ok(asks_every_payment(
                        db.setting(dw_appdb::GLOBAL_SCOPE, &key)?.as_deref(),
                    ))
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
            let value = if on { "strict" } else { "window" };
            this.appdb_op(move |db| db.set_setting(dw_appdb::GLOBAL_SCOPE, &key, Some(value)))
                .await
        })
        .await
    }

    /// Writes the signing-window default for a vault this build just
    /// created, when the vault has no row yet. An older vault is left
    /// absent, which still asks every time.
    pub async fn adopt_signing_window_default(self: &Arc<Self>) -> Result<(), EngineError> {
        let Some(vault_id) = self.vault().vault_id() else {
            return Ok(());
        };
        let key = payment_auth_key(&vault_id);
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.appdb_op(move |db| {
                if db.setting(dw_appdb::GLOBAL_SCOPE, &key)?.is_none() {
                    db.set_setting(dw_appdb::GLOBAL_SCOPE, &key, Some("window"))?;
                }
                Ok(())
            })
            .await
        })
        .await
    }

    /// Replaces this vault's payment-auth row. `None` deletes it, which is
    /// how a vault from before the signing window stored "ask every time".
    /// Tests use it to prove an upgrade still asks.
    #[cfg(all(debug_assertions, feature = "test-support"))]
    pub async fn set_payment_auth_row_for_test(
        self: &Arc<Self>,
        value: Option<String>,
    ) -> Result<(), EngineError> {
        let Some(vault_id) = self.vault().vault_id() else {
            return Ok(());
        };
        let key = payment_auth_key(&vault_id);
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.appdb_op(move |db| db.set_setting(dw_appdb::GLOBAL_SCOPE, &key, value.as_deref()))
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
        let checked = Checked::new(options)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // PBKDF2 stays off the async workers, and before the lifecycle
            // lock.
            let secret = tokio::task::spawn_blocking(make_secret).await??;
            let held = this.wallet_lifecycle.hold().await;
            this.import_secret_locked(&held, checked, secret, auto_backup, origin)
                .await
        })
        .await
    }

    /// [`Self::import_secret_inner`] once the secret is made, under the
    /// lifecycle lock (DEC-210 (3b)): from the seed stored in the vault to
    /// the wallet registered, or what was stored of it undone.
    pub(crate) async fn import_secret_locked(
        self: &Arc<Self>,
        held: &Lifecycle<'_>,
        checked: Checked,
        secret: WalletSecret,
        auto_backup: AutoBackup,
        origin: Origin,
    ) -> Result<WalletId, EngineError> {
        let Checked {
            name,
            lookahead,
            birth_height,
        } = checked;
        let manager = self.manager()?;
        let network = self.network.core_network();
        let vault = self.vault.clone();
        // Vault file writes and fsync stay off the async workers.
        let Stored {
            wallet_id,
            seed,
            had_secret,
            fingerprint,
        } = tokio::task::spawn_blocking(move || -> Result<Stored, EngineError> {
            let id = mnemonic::wallet_id_for_seed(&secret.seed, network).map_err(mnemonic_error)?;
            // The vault keeps a seed it holds: one stored before, perhaps
            // with another derivation flag (the id derives from the seed),
            // counts as one already there.
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
        let readmitted = match self.appdb_op(move |db| db.readmit_wallet(&readmit)).await {
            Ok(readmitted) => readmitted,
            Err(e) => {
                if !had_secret {
                    self.forget_secret(held, wallet_id).await;
                }
                return Err(e);
            }
        };
        self.store_fingerprint(wallet_id, fingerprint).await;

        // A closed (unloaded) wallet is registered: open it instead.
        if self.live()?.store.is_unloaded(&wallet_id.0) {
            if !had_secret {
                self.forget_secret(held, wallet_id).await;
            }
            return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
        }
        if manager.get_wallet(&wallet_id.0).await.is_some() {
            if had_secret {
                return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
            }
            // Keys attached: watch-only becomes false; hosts reload.
            self.sink.emit(EngineEvent::WalletCreated {
                network: self.network.clone(),
                wallet_id,
            });
            if auto_backup == AutoBackup::Schedule {
                self.platform.signal(PlatformSignal::WalletAdded(wallet_id));
                // E0-04 §8.6 (review L-4): as `load_wallet`.
                self.leases.wallet_opened(wallet_id);
                self.schedule_automatic_backup(wallet_id);
            }
            return Ok(wallet_id);
        }

        let default_name = self.next_default_name();
        // Before SPV can match a batch with the pools it publishes.
        self.set_wide_scan(wallet_id, Some(WideScan::Birth)).await;
        let was_unloaded = self.hub.admit(wallet_id);
        let registered = if registration_refused(self.data_dir()) {
            Err(EngineError::Internal("test: registration refused".into()))
        } else {
            manager
                .create_wallet_from_seed_bytes(
                    network,
                    &seed,
                    WalletAccountCreationOptions::Default,
                    birth_height,
                )
                .await
                .map_err(EngineError::from)
        };
        #[cfg(test)]
        let registered = registered.and_then(|wallet| {
            if take_test_failure(&FAIL_AFTER_REGISTRATION_IN, self.data_dir()) {
                return Err(EngineError::Internal(
                    "test: failed after registering".into(),
                ));
            }
            Ok(wallet)
        });
        drop(seed);
        let wallet = match registered {
            Ok(wallet) if wallet.wallet_id() == wallet_id.0 => wallet,
            Ok(wallet) => {
                self.unadmit(&manager, wallet_id, was_unloaded).await;
                return Err(EngineError::Internal(format!(
                    "platform-wallet registered {} for the seed of {wallet_id}",
                    hex::encode(wallet.wallet_id())
                )));
            }
            Err(e) => {
                self.unadmit(&manager, wallet_id, was_unloaded).await;
                // A registration that exists keeps the stored seed: only
                // our own failure is rolled back.
                if !matches!(e, EngineError::WalletAlreadyExists(_)) {
                    if !had_secret {
                        self.forget_secret(held, wallet_id).await;
                    }
                    if readmitted {
                        self.record_removed_again(held, wallet_id).await;
                    }
                }
                return Err(e);
            }
        };
        let mut raised = true;
        if let Some(gap) = lookahead {
            if let Err(e) = apply_lookahead(&wallet, gap).await {
                raised = false;
                tracing::warn!(%wallet_id, error = %e, "could not raise the restore lookahead");
            }
            self.store_lookahead(wallet_id, gap).await;
        }
        let published_at = birth_checkpoint(&manager, wallet_id).await;
        if self
            .widen_published_wallet(&manager, wallet_id, published_at)
            .await
            && raised
        {
            self.set_wide_scan(wallet_id, Some(WideScan::Done)).await;
        }
        self.refresh_wallet_state(&manager, wallet_id).await;
        // The wallet is registered; a failed name write leaves it with
        // the computed default name and is reported in the log only.
        if let Err(e) = self
            .store_name(wallet_id, name.unwrap_or(default_name), None)
            .await
        {
            tracing::warn!(%wallet_id, error = %e, "could not store the wallet name");
        }
        // Before the bring-up hears of the wallet, which reads it.
        if origin == Origin::CreatedHere {
            self.mark_created_here(wallet_id).await;
        }
        self.sink.emit(EngineEvent::WalletCreated {
            network: self.network.clone(),
            wallet_id,
        });
        if auto_backup == AutoBackup::Schedule {
            self.platform.signal(PlatformSignal::WalletAdded(wallet_id));
            // E0-04 §8.6 (review L-4): its funding statuses read and
            // its tracked rows resumed, as `load_wallet` does.
            self.leases.wallet_opened(wallet_id);
            self.schedule_automatic_backup(wallet_id);
        }
        Ok(wallet_id)
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

    /// Wallet `id`'s stored lookahead, if a valid one is stored.
    pub(crate) async fn stored_lookahead(&self, id: WalletId) -> Option<u32> {
        let appdb = std::sync::Arc::clone(&self.live().ok()?.appdb);
        let key = id.to_string();
        tokio::task::spawn_blocking(move || appdb.setting(&key, LOOKAHEAD_SETTING))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|g| (1..=MAX_LOOKAHEAD).contains(g))
    }

    /// [`Self::restore_loaded_wallet`] for every wallet (session open).
    pub(crate) async fn restore_loaded_wallets(&self, manager: &crate::session::Manager) {
        for id in manager.list_wallet_ids_blocking() {
            self.restore_loaded_wallet(manager, WalletId(id)).await;
        }
    }

    /// Applies wallet `id`'s stored lookahead, if any, then places the coins
    /// the load left outside the account that derives them
    /// ([`crate::restore`]), before SPV can replay them. Then widens its
    /// CoinJoin pools and replays what may have been matched with narrow
    /// ones ([`WIDE_SCAN_SETTING`]): above the loaded checkpoint, or the
    /// marker's, or the birth checkpoint of a wallet an older build
    /// scanned. Failures are logged and leave the marker for the next load.
    pub(crate) async fn restore_loaded_wallet(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
    ) {
        let lookahead = self.stored_lookahead(id).await;
        let restored = match lookahead {
            Some(gap) => self.raise_lookahead(manager, id, gap).await,
            None => self.restore_coins(manager, id).await.map(|_| ()),
        };
        if let Err(e) = &restored {
            tracing::warn!(wallet_id = %id, error = %e, "could not restore the loaded wallet's lookahead or coins");
        }
        let loaded_at = self
            .live()
            .ok()
            .and_then(|live| live.store.loaded_checkpoint(&id.0));
        let marker = self.wide_scan(id).await;
        let from = match marker {
            Ok(Some(WideScan::Done)) | Err(_) => loaded_at,
            Ok(Some(WideScan::From(h))) => Some(loaded_at.map_or(h, |c| c.min(h))),
            Ok(Some(WideScan::Birth)) => Some(birth_checkpoint(manager, id).await),
            Ok(None) if self.scanned_narrow(manager, id, lookahead).await => {
                Some(birth_checkpoint(manager, id).await)
            }
            Ok(None) => loaded_at,
        };
        let widened = match from {
            Some(checkpoint) => self.widen_published_wallet(manager, id, checkpoint).await,
            None => ensure_coinjoin_scan_gap(manager, id)
                .await
                .inspect_err(|e| tracing::warn!(wallet_id = %id, error = %e, "could not widen the CoinJoin scan gap"))
                .is_ok(),
        };
        let raised = lookahead.is_none() || restored.is_ok();
        if widened && raised && matches!(marker, Ok(m) if m != Some(WideScan::Done)) {
            self.set_wide_scan(id, Some(WideScan::Done)).await;
        }
    }

    /// Whether wallet `id`, without a [`WIDE_SCAN_SETTING`], may have been
    /// scanned with narrow pools: it has a CoinJoin account or a raised
    /// lookahead, and was not created here (a created wallet's CoinJoin
    /// keys are only those its own mixing derived).
    async fn scanned_narrow(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        lookahead: Option<u32>,
    ) -> bool {
        let coinjoin = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (wm.get_wallet_info(&id.0))
                .is_some_and(|info| !info.core_wallet.accounts.coinjoin_accounts.is_empty())
        };
        if !coinjoin && lookahead.is_none() {
            return false;
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let created_here = self
            .appdb_op(move |db| db.setting(&scope, crate::platform::CREATED_HERE_KEY))
            .await;
        !matches!(created_here, Ok(Some(_)))
    }

    /// Wallet `id`'s [`WIDE_SCAN_SETTING`].
    pub(crate) async fn wide_scan(&self, id: WalletId) -> Result<Option<WideScan>, EngineError> {
        let scope = dw_appdb::local_scope(&id.to_string());
        let read = self
            .appdb_op(move |db| db.setting(&scope, WIDE_SCAN_SETTING))
            .await;
        if let Err(e) = &read {
            tracing::warn!(wallet_id = %id, error = %e, "could not read whether the wallet was scanned with wide CoinJoin pools");
        }
        Ok(read?.as_deref().map(WideScan::parse))
    }

    /// Stores wallet `id`'s [`WIDE_SCAN_SETTING`]; a failure is logged.
    pub(crate) async fn set_wide_scan(&self, id: WalletId, marker: Option<WideScan>) {
        let scope = dw_appdb::local_scope(&id.to_string());
        let value = marker.map(WideScan::encode);
        let stored = self
            .appdb_op(move |db| db.set_setting(&scope, WIDE_SCAN_SETTING, value.as_deref()))
            .await;
        if let Err(e) = stored {
            tracing::warn!(wallet_id = %id, error = %e, "could not store whether the wallet was scanned with wide CoinJoin pools");
        }
    }

    /// Widens open wallet `id`'s CoinJoin pools, then rewinds its filter
    /// checkpoint to `published_at`, the one it had when platform-wallet
    /// made it visible to SPV, if a batch committed above it since: that
    /// batch matched the narrower pools. The rewind stores its replay
    /// record first. Returns whether both were done; failures are logged.
    pub(crate) async fn widen_published_wallet(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        published_at: u32,
    ) -> bool {
        let widened = ensure_coinjoin_scan_gap(manager, id).await;
        if let Err(e) = &widened {
            tracing::warn!(wallet_id = %id, error = %e, "could not widen the CoinJoin scan gap");
        }
        let synced = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            wm.get_wallet_info(&id.0)
                .map(|info| info.core_wallet.synced_height())
        };
        let Some(synced) = synced else {
            return false;
        };
        if synced <= published_at {
            return widened.is_ok();
        }
        match self.rewind_recorded(manager, id, published_at).await {
            Ok(rewound) => widened.is_ok() && rewound.is_some(),
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not rescan what SPV matched before the CoinJoin gap was widened");
                false
            }
        }
    }

    /// Raises open wallet `id`'s BIP44 and CoinJoin lookahead to `gap`, then
    /// places the coins only the larger gap reaches in the accounts that
    /// derive them ([`Self::restore_coins`]), before anything replays them.
    /// Callers refresh the hub.
    pub(crate) async fn raise_lookahead(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        gap: u32,
    ) -> Result<(), EngineError> {
        let wallet = (manager.get_wallet(&id.0).await)
            .ok_or_else(|| EngineError::WalletNotFound(id.to_string()))?;
        let raised = apply_lookahead(&wallet, gap).await;
        // A partial raise still grew some pools, so the restore runs either way.
        let restored = self.restore_coins(manager, id).await;
        match (raised, restored) {
            (Err(raise), Err(restore)) => Err(EngineError::Internal(format!(
                "{restore}; raising the lookahead to {gap} failed first: {raise}"
            ))),
            (raised, restored) => raised.and(restored.map(|_| ())),
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
    /// recorded as removed again, with what the import wrote of it, unless
    /// the failed call registered it after all (DEC-210). The lifecycle
    /// lock keeps any other registration out meanwhile.
    pub(crate) async fn record_removed_again(&self, _held: &Lifecycle<'_>, wallet_id: WalletId) {
        let Ok(live) = self.live() else { return };
        if live.store.is_unloaded(&wallet_id.0)
            || live.manager.get_wallet(&wallet_id.0).await.is_some()
        {
            return;
        }
        let id = wallet_id.to_string();
        if let Err(e) = self.appdb_op(move |db| db.delete_wallet(&id)).await {
            tracing::warn!(%wallet_id, error = %e, "could not record the wallet as removed again");
        }
    }

    /// Deletes the vault records of a wallet whose registration failed.
    pub(crate) async fn forget_secret(&self, _held: &Lifecycle<'_>, wallet_id: WalletId) {
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
            // Not a DashPay contact's address: the wallet does not own it
            // (MG-05 D19).
            let path = {
                let state = wallet.state().await;
                crate::coins::own_address_info(&state.core_wallet, &parsed).map(|info| info.path)
            }
            .ok_or_else(|| EngineError::AddressNotMine(address.clone()))?;
            // A ProTx owner key signs owner payloads only (MG-04 D50): no
            // key for a message. Refused before the grant is redeemed, so
            // it isn't spent; the vault enforces the same rule. Nor a voting
            // key: off testnet a vote signature is a message one.
            if dw_vault::paths::is_owner_key(&path, network)
                || dw_vault::paths::is_voting_key(&path, network)
            {
                return Err(EngineError::AddressNoKey(address));
            }

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

#[cfg(test)]
mod payment_auth_tests {
    use super::asks_every_payment;

    #[test]
    fn an_absent_or_unknown_row_asks_and_only_an_explicit_window_does_not() {
        assert!(asks_every_payment(None));
        assert!(asks_every_payment(Some("strict")));
        assert!(asks_every_payment(Some("1")));
        assert!(asks_every_payment(Some("garbage")));
        assert!(!asks_every_payment(Some("window")));
        assert!(!asks_every_payment(Some("0")));
    }
}
