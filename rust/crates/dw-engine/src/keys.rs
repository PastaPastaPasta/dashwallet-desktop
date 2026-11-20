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

use crate::compat::ImportedState;
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
pub(crate) const LOOKAHEAD_SETTING: &str = "bip44.lookahead";
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

/// Tests: wallets whose [`WIDE_SCAN_SETTING`] cannot be read or durably
/// written.
#[cfg(test)]
pub(crate) static FAIL_WIDE_SCAN_IO: std::sync::Mutex<Vec<[u8; 32]>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: wallets whose [`WIDE_SCAN_SETTING`] cannot be read (it can be
/// written).
#[cfg(test)]
pub(crate) static FAIL_WIDE_SCAN_READ: std::sync::Mutex<Vec<[u8; 32]>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: wallets whose lookahead cannot be stored.
#[cfg(test)]
pub(crate) static FAIL_LOOKAHEAD_WRITE: std::sync::Mutex<Vec<[u8; 32]>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: wallets whose stored CoinJoin use cannot be read.
#[cfg(test)]
pub(crate) static FAIL_COINJOIN_USE_READ: std::sync::Mutex<Vec<[u8; 32]>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: wallets whose stored range and lookahead cannot be read.
#[cfg(test)]
pub(crate) static FAIL_RESTORE_READ: std::sync::Mutex<Vec<[u8; 32]>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn fails_for(list: &std::sync::Mutex<Vec<[u8; 32]>>, id: WalletId) -> bool {
    list.lock().unwrap().contains(&id.0)
}

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

/// What the first, read-only part of an import found.
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
                ImportedState::default(),
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
    /// strict mode), `Vault(NoVault | Locked | MixingOnly | …)`, `Storage`
    /// while the dispatch journal is unavailable (nothing is stored).
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
        self.import_secret_with_state(options, make_secret, ImportedState::default())
            .await
    }

    /// [`Self::import_secret_with`] for a wallet that comes with `state`
    /// from a Dash Core file: stored before the wallet is registered, and
    /// its range applied before anyone hears of the wallet.
    pub(crate) async fn import_secret_with_state<F>(
        self: &Arc<Self>,
        options: ImportOptions,
        make_secret: F,
        state: ImportedState,
    ) -> Result<WalletId, EngineError>
    where
        F: FnOnce() -> Result<WalletSecret, EngineError> + Send + 'static,
    {
        self.import_secret_inner(
            options,
            make_secret,
            AutoBackup::Schedule,
            Origin::Imported,
            state,
        )
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
        state: ImportedState,
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
            this.import_secret_locked(&held, checked, secret, auto_backup, origin, state)
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
        state: ImportedState,
    ) -> Result<WalletId, EngineError> {
        if origin == Origin::Imported {
            self.require_journal()?;
        }
        let Checked {
            name,
            lookahead,
            birth_height,
        } = checked;
        let manager = self.manager()?;
        let network = self.network.core_network();
        let vault = self.vault.clone();
        // Vault reads stay off the async workers.
        let (
            Stored {
                wallet_id,
                seed,
                had_secret,
                fingerprint,
            },
            secret,
        ) = tokio::task::spawn_blocking(move || -> Result<_, EngineError> {
            let id = mnemonic::wallet_id_for_seed(&secret.seed, network).map_err(mnemonic_error)?;
            // Kept for PSBT derivation records (watch-only use, no key needed).
            let fingerprint =
                key_wallet::bip32::ExtendedPrivKey::new_master(network, &secret.seed[..])
                    .map(|m| m.fingerprint().to_string())
                    .map_err(|e| EngineError::Internal(format!("master key: {e}")))?;
            let stored = Stored {
                wallet_id: WalletId(id),
                seed: Zeroizing::new(*secret.seed),
                had_secret: vault.has_wallet_secret(&id),
                fingerprint,
            };
            Ok((stored, secret))
        })
        .await??;
        // Rows of a removed wallet are skipped until it is readmitted
        // (DEC-210), before its first write.
        let readmit = wallet_id.to_string();
        let readmitted = self.appdb_op(move |db| db.readmit_wallet(&readmit)).await?;
        self.store_fingerprint(wallet_id, fingerprint).await;

        // A closed (unloaded) wallet is registered: open it instead.
        if self.live()?.store.is_unloaded(&wallet_id.0) {
            return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
        }
        let registered = manager.get_wallet(&wallet_id.0).await.is_some();
        if registered && had_secret {
            return Err(EngineError::WalletAlreadyExists(wallet_id.to_string()));
        }
        // Keys for a watch-only wallet: it gets Core's range first. A range
        // applied before a later step fails is not narrowed again: its
        // addresses are the wallet's own.
        // Its history from the start of Core's import or the wallet's own
        // birth, the earlier, was matched without that range: the replay is
        // marked, durably, before anything applies or stores the range or the
        // import's lookahead, so a crash after either replays with it. It is
        // replayed once the keys are attached, with the birth lowered to that
        // start if it is below.
        let attached = if registered && (state.issued.is_some() || lookahead.is_some()) {
            let birth = birth_checkpoint(&manager, wallet_id).await;
            let from = birth_height.map_or(birth, |h| h.saturating_sub(1).min(birth));
            let (from, replaced) = self.note_narrow_below(&manager, wallet_id, from).await?;
            self.cover_imported_range(&manager, wallet_id, &state)
                .await?;
            let mut raised = true;
            if let Some(gap) = lookahead {
                let wallet = (manager.get_wallet(&wallet_id.0).await)
                    .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))?;
                if let Err(e) = apply_lookahead(&wallet, gap).await {
                    raised = false;
                    tracing::warn!(%wallet_id, error = %e, "could not raise the attached keys' lookahead");
                }
                self.write_lookahead(wallet_id, gap).await?;
            }
            Some((from, replaced, raised))
        } else {
            if registered {
                self.cover_imported_range(&manager, wallet_id, &state)
                    .await?;
            }
            None
        };
        // A failed attach puts the previous marker back only when nothing of
        // it can remain: no lookahead written and none of Core's rows stored.
        let unmark_attached = || async move {
            if let (Some((_, Some(previous), _)), None) = (attached, lookahead) {
                self.set_wide_scan(wallet_id, previous).await;
            }
        };
        // A Dash Core file's range, coin locks and salt are stored, all or
        // none, before the wallet is registered or its seed is stored, so
        // nothing can see or spend from it with keys before they are there.
        // A wallet already there with its keys is left as it is.
        let prior = match self.write_imported_state(wallet_id, &state).await {
            Ok(prior) => prior,
            Err(e) => {
                unmark_attached().await;
                if readmitted || !had_secret {
                    self.record_removed_again(held, wallet_id).await;
                }
                return Err(e);
            }
        };
        // The vault keeps a seed it holds: one stored before, perhaps with
        // another derivation flag (the id derives from the seed), counts as
        // one already there. Vault file writes and fsync stay off the async
        // workers.
        let had_secret = had_secret || {
            let vault = self.vault.clone();
            let stored = tokio::task::spawn_blocking(move || {
                match vault.store_wallet_secret(&wallet_id.0, &secret) {
                    Err(VaultError::WalletSecretExists) => Ok(true),
                    stored => stored.map(|()| false),
                }
            })
            .await
            .map_err(EngineError::from)
            .and_then(|r| r.map_err(EngineError::from));
            match stored {
                Ok(existed) => existed,
                Err(e) => {
                    if registered {
                        if let Some(prior) = prior {
                            self.restore_imported_state(prior).await;
                        }
                    } else {
                        self.record_removed_again(held, wallet_id).await;
                    }
                    return Err(e);
                }
            }
        };
        if registered {
            if let Some((from, _, raised)) = attached {
                if self.widen_published_wallet(&manager, wallet_id, from).await && raised {
                    self.set_wide_scan(wallet_id, Some(WideScan::Done)).await;
                }
                self.refresh_wallet_state(&manager, wallet_id).await;
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
        // Before SPV can match a batch with the pools it publishes: the
        // lookahead it is to scan with and the marker, put back if nothing is
        // registered.
        let pending = async {
            let previous = self.wide_scan(wallet_id).await?;
            self.mark_pending(wallet_id, WideScan::Birth).await?;
            if let Some(gap) = lookahead {
                self.write_lookahead(wallet_id, gap).await?;
            }
            Ok::<_, EngineError>(previous)
        }
        .await;
        let previous = match pending {
            Ok(previous) => previous,
            Err(e) => {
                if !had_secret {
                    self.forget_secret(held, wallet_id).await;
                }
                if readmitted || !had_secret {
                    self.record_removed_again(held, wallet_id).await;
                }
                return Err(e);
            }
        };
        let unmark = || async move {
            self.set_wide_scan(wallet_id, previous).await;
        };
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
                unmark().await;
                if readmitted || !had_secret {
                    self.record_removed_again(held, wallet_id).await;
                }
                return Err(EngineError::Internal(format!(
                    "platform-wallet registered {} for the seed of {wallet_id}",
                    hex::encode(wallet.wallet_id())
                )));
            }
            Err(e) => {
                self.unadmit(&manager, wallet_id, was_unloaded).await;
                unmark().await;
                // A registration that exists keeps the stored seed: only
                // our own failure is rolled back.
                if !matches!(e, EngineError::WalletAlreadyExists(_)) {
                    if !had_secret {
                        self.forget_secret(held, wallet_id).await;
                    }
                    if readmitted || !had_secret {
                        self.record_removed_again(held, wallet_id).await;
                    }
                }
                return Err(e);
            }
        };
        let mut raised = true;
        if let Some(gap) = lookahead
            && let Err(e) = apply_lookahead(&wallet, gap).await
        {
            raised = false;
            tracing::warn!(%wallet_id, error = %e, "could not raise the restore lookahead");
        }
        // Not announced without Core's range: taken out again, with the rows
        // written for it.
        if let Err(e) = self.cover_imported_range(&manager, wallet_id, &state).await {
            let _discoveries = self.platform.recovery.end_discoveries(wallet_id).await;
            self.platform.forget(&wallet_id);
            if let Err(removal) = self.remove_registered(wallet_id).await {
                tracing::warn!(%wallet_id, error = %removal, "could not take out a wallet whose range was not applied");
            }
            if let Ok(live) = self.live() {
                live.tap.forget(&wallet_id);
            }
            if !had_secret {
                self.forget_secret(held, wallet_id).await;
            }
            return Err(e);
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
        self.list_on_startup(held, wallet_id, true).await;
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

    /// Tests: stores wallet `id`'s lookahead, a failure logged.
    #[cfg(test)]
    pub(crate) async fn store_lookahead(&self, wallet_id: WalletId, gap: u32) {
        if self.write_lookahead(wallet_id, gap).await.is_err() {
            tracing::warn!(%wallet_id, "could not store the lookahead; it lasts until restart");
        }
    }

    /// Stores wallet `id`'s lookahead `gap`, durable when it returns `Ok`:
    /// before a wallet is published, so a load after a crash scans with it.
    pub(crate) async fn write_lookahead(&self, id: WalletId, gap: u32) -> Result<(), EngineError> {
        #[cfg(test)]
        if fails_for(&FAIL_LOOKAHEAD_WRITE, id) {
            return Err(EngineError::Storage("test: lookahead not stored".into()));
        }
        let key = id.to_string();
        self.appdb_op(move |db| {
            db.set_setting_durable(&key, LOOKAHEAD_SETTING, Some(&gap.to_string()))
        })
        .await
    }

    /// Wallet `id`'s stored lookahead, if one is stored; `Err` when it cannot
    /// be read or is malformed (its range is then not known to be applied).
    pub(crate) async fn stored_lookahead(&self, id: WalletId) -> Result<Option<u32>, EngineError> {
        #[cfg(test)]
        if fails_for(&FAIL_RESTORE_READ, id) {
            return Err(EngineError::Storage("test: lookahead not read".into()));
        }
        let key = id.to_string();
        let value = self
            .appdb_op(move |db| db.setting(&key, LOOKAHEAD_SETTING))
            .await?;
        value
            .map(|v| {
                (v.parse::<u32>().ok())
                    .filter(|g| (1..=MAX_LOOKAHEAD).contains(g))
                    .ok_or_else(|| {
                        EngineError::Storage(format!("malformed stored lookahead {v:?}"))
                    })
            })
            .transpose()
    }

    /// [`Self::restore_loaded_wallet`] for every wallet (session open):
    /// `Err` when a wallet's replay obligation cannot be stored, and SPV
    /// does not start.
    pub(crate) async fn restore_loaded_wallets(
        &self,
        manager: &crate::session::Manager,
    ) -> Result<(), EngineError> {
        for id in manager.list_wallet_ids_blocking() {
            self.restore_loaded_wallet(manager, WalletId(id)).await?;
        }
        Ok(())
    }

    /// Applies wallet `id`'s stored legacy range and lookahead, if any.
    /// Returns whether a lookahead may be stored (one is, or it cannot be
    /// read) and whether all of it is applied: an unreadable or malformed
    /// setting is not.
    pub(crate) async fn apply_stored_ranges(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
    ) -> (bool, Result<(), EngineError>) {
        let covered = match self.stored_issued(id).await {
            Ok(Some(issued)) => self.cover_issued(manager, id, issued).await,
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        };
        if let Err(e) = &covered {
            tracing::warn!(wallet_id = %id, error = %e, "could not restore the legacy wallet's address range");
        }
        let lookahead = self.stored_lookahead(id).await;
        let raised = match &lookahead {
            Ok(Some(gap)) => match manager.get_wallet(&id.0).await {
                Some(wallet) => apply_lookahead(&wallet, *gap).await,
                None => Err(EngineError::WalletNotFound(id.to_string())),
            },
            Ok(None) => Ok(()),
            Err(e) => Err(EngineError::Storage(format!("the stored lookahead: {e}"))),
        };
        if let Err(e) = &raised {
            tracing::warn!(wallet_id = %id, error = %e, "could not raise the loaded wallet's lookahead");
        }
        (!matches!(lookahead, Ok(None)), covered.and(raised))
    }

    /// Stores, durably, that wallet `id` is to be replayed from the earliest
    /// start any obligation names: `from`, its outstanding replay record's
    /// (whose resume lowers the birth to it; no lower than the birth
    /// checkpoint for a cancelled one, and that checkpoint for one that
    /// cannot be read) and its marker's; its birth checkpoint when none is
    /// known.
    pub(crate) async fn require_replay_from(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        from: Option<u32>,
    ) -> Result<(), EngineError> {
        let birth = birth_checkpoint(manager, id).await;
        // A record that cannot be read may start anywhere: from the birth.
        let recorded = match self.live()?.replay.stored(id).await {
            Ok(record) => record.map(|r| {
                if r.cancelled {
                    r.reached.max(birth)
                } else {
                    r.reached
                }
            }),
            Err(_) => Some(birth),
        };
        let floor = from.into_iter().chain(recorded).min().unwrap_or(birth);
        let held = match self.wide_scan(id).await? {
            Some(WideScan::From(h)) => Some(h),
            Some(WideScan::Birth) => Some(birth),
            Some(WideScan::Done) | None => None,
        };
        if held.is_some_and(|h| h <= floor) {
            return Ok(());
        }
        self.mark_pending(id, WideScan::From(floor)).await
    }

    /// Applies wallet `id`'s stored legacy address range and lookahead, if
    /// any, then places the coins the load left outside the account that
    /// derives them ([`crate::restore`]), before SPV can replay them. Then
    /// widens its CoinJoin pools and replays what may have been matched with
    /// narrow ones ([`WIDE_SCAN_SETTING`]): above the loaded checkpoint, or
    /// the marker's, or the birth checkpoint of a wallet an older build
    /// scanned. Failures, an unreadable or malformed range or lookahead
    /// among them, are logged; before SPV may scan the wallet with the pools
    /// it has, the earliest start any obligation names is stored, durably
    /// ([`Self::require_replay_from`]). `Err` when that cannot be stored.
    pub(crate) async fn restore_loaded_wallet(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
    ) -> Result<(), EngineError> {
        let (lookahead, raised) = self.apply_stored_ranges(manager, id).await;
        // A partial raise still grew some pools, so the restore runs either way.
        if let Err(e) = self.restore_coins(manager, id).await {
            tracing::warn!(wallet_id = %id, error = %e, "could not restore the loaded wallet's coins");
        }
        let loaded_at = self
            .live()
            .ok()
            .and_then(|live| live.store.loaded_checkpoint(&id.0));
        let marker = self.wide_scan(id).await;
        // A wallet whose CoinJoin use cannot be read is replayed and stays
        // without a marker, so its next load reads it again.
        let mut unsure = false;
        let from = match &marker {
            Ok(Some(WideScan::Done)) => loaded_at,
            // It may owe any start: replayed from the birth, and nothing
            // writes `Done` over it.
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the wallet's wide-scan marker; replaying it from its birth");
                unsure = true;
                Some(birth_checkpoint(manager, id).await)
            }
            Ok(Some(WideScan::From(h))) => Some(loaded_at.map_or(*h, |c| c.min(*h))),
            Ok(Some(WideScan::Birth)) => Some(birth_checkpoint(manager, id).await),
            Ok(None) => match self.scanned_narrow(manager, id, lookahead).await {
                Ok(true) => Some(birth_checkpoint(manager, id).await),
                Ok(false) => {
                    tracing::info!(wallet_id = %id, "an older build scanned this wallet and its CoinJoin account shows no use: not replayed");
                    loaded_at
                }
                Err(e) => {
                    tracing::warn!(wallet_id = %id, error = %e, "could not read whether the wallet's CoinJoin account was used; replaying it");
                    unsure = true;
                    Some(birth_checkpoint(manager, id).await)
                }
            },
        };
        // A stored replay from at or below that start, up to the loaded
        // checkpoint, replays the range itself once the pools are wide, and a
        // cancelled one holds spends for a rescan from the birth: no rewind
        // of its own, which would restart a cancelled replay.
        let record = match self.live() {
            Ok(live) => live.replay.stored(id).await.ok().flatten(),
            Err(_) => None,
        };
        // A cancelled one holding above an owed start leaves that start
        // owed: the marker stays for the next open, not `Done`.
        let mut owed_below_hold = false;
        let from = match (from, record) {
            (Some(f), Some(r)) if r.cancelled && r.reached > f => {
                owed_below_hold = true;
                loaded_at
            }
            (Some(f), Some(r))
                if r.cancelled || (r.reached <= f && loaded_at.is_some_and(|c| c <= r.target)) =>
            {
                loaded_at
            }
            (from, _) => from,
        };
        let widened = match from {
            Some(checkpoint) => self.widen_published_wallet(manager, id, checkpoint).await,
            None => ensure_coinjoin_scan_gap(manager, id)
                .await
                .inspect_err(|e| tracing::warn!(wallet_id = %id, error = %e, "could not widen the CoinJoin scan gap"))
                .is_ok(),
        };
        if !(widened && raised.is_ok()) {
            // The session scans on with what pools it has: the next load
            // replays from the earliest start still owed.
            if let Err(e) = self.require_replay_from(manager, id, from).await {
                tracing::warn!(wallet_id = %id, error = %e, "could not mark the wallet for a replay at the next load");
                return Err(e);
            }
        } else if !unsure
            && !owed_below_hold
            && matches!(marker, Ok(m) if m != Some(WideScan::Done))
        {
            self.set_wide_scan(id, Some(WideScan::Done)).await;
        }
        Ok(())
    }

    /// Whether wallet `id`, without a [`WIDE_SCAN_SETTING`] (scanned by an
    /// older build), may have missed coins past its narrow pools: it was not
    /// created here (a created wallet's CoinJoin keys are only those its own
    /// mixing derived), and it has a raised lookahead or a CoinJoin account
    /// that shows use. An account with nothing used in its first window has
    /// no gap for a later coin to sit behind. `Err` when that use cannot be
    /// read.
    async fn scanned_narrow(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        lookahead: bool,
    ) -> Result<bool, EngineError> {
        use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
        let (coinjoin, used) = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let accounts = (wm.get_wallet_info(&id.0))
                .map(|info| &info.core_wallet.accounts.coinjoin_accounts);
            let used = accounts.is_some_and(|accounts| {
                accounts.values().any(|a| {
                    !a.utxos.is_empty()
                        || (a.managed_account_type().address_pools().iter())
                            .any(|p| !p.used_indices.is_empty())
                })
            });
            (accounts.is_some_and(|a| !a.is_empty()), used)
        };
        if !coinjoin && !lookahead {
            return Ok(false);
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let created_here = self
            .appdb_op(move |db| db.setting(&scope, crate::platform::CREATED_HERE_KEY))
            .await;
        if matches!(created_here, Ok(Some(_))) {
            return Ok(false);
        }
        if lookahead || used {
            return Ok(true);
        }
        #[cfg(test)]
        if fails_for(&FAIL_COINJOIN_USE_READ, id) {
            return Err(EngineError::Storage("test: CoinJoin use not read".into()));
        }
        let db_path = self.data_dir().join(crate::session::WALLET_DB_FILE);
        let persister = std::sync::Arc::clone(&self.live()?.persister);
        tokio::task::spawn_blocking(move || {
            crate::store::coinjoin_used(&db_path, &id.0, &persister)
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("the stored CoinJoin use: {e}")))
    }

    /// Wallet `id`'s [`WIDE_SCAN_SETTING`].
    pub(crate) async fn wide_scan(&self, id: WalletId) -> Result<Option<WideScan>, EngineError> {
        #[cfg(test)]
        if fails_for(&FAIL_WIDE_SCAN_IO, id) || fails_for(&FAIL_WIDE_SCAN_READ, id) {
            return Err(EngineError::Storage("test: marker not read".into()));
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let read = self
            .appdb_op(move |db| db.setting(&scope, WIDE_SCAN_SETTING))
            .await;
        if let Err(e) = &read {
            tracing::warn!(wallet_id = %id, error = %e, "could not read whether the wallet was scanned with wide CoinJoin pools");
        }
        Ok(read?.as_deref().map(WideScan::parse))
    }

    /// Notes that wallet `id`'s blocks above `from` may be matched with
    /// narrow pools, unless its marker already names a start at or below it,
    /// even one below the wallet's birth. Returns where a replay starts and
    /// the marker replaced, if any. `Err` when the marker cannot be read or
    /// stored.
    async fn note_narrow_below(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        from: u32,
    ) -> Result<(u32, Option<Option<WideScan>>), EngineError> {
        let marked = self.wide_scan(id).await?;
        let lower = match marked {
            Some(WideScan::From(h)) => Some(h),
            Some(WideScan::Birth) => Some(birth_checkpoint(manager, id).await),
            _ => None,
        };
        match lower {
            Some(h) if h <= from => Ok((h, None)),
            _ => {
                self.mark_pending(id, WideScan::From(from)).await?;
                Ok((from, Some(marked)))
            }
        }
    }

    /// Notes that wallet `id`'s blocks above `checkpoint` (its birth's when
    /// `None`) may be matched with narrow pools, unless its marker already
    /// names a lower start. `birth` is the wallet's birth checkpoint, when
    /// known: a marker at or below it is kept for `None`. `Err` when the
    /// marker cannot be read (a lower one may stand) or durably stored: the
    /// caller does not publish the wallet then.
    pub(crate) async fn note_narrow_from(
        &self,
        id: WalletId,
        checkpoint: Option<u32>,
        birth: Option<u32>,
    ) -> Result<(), EngineError> {
        let lower = match (self.wide_scan(id).await?, checkpoint) {
            (Some(WideScan::Done), Some(h)) => WideScan::From(h),
            (Some(WideScan::From(old)), Some(h)) if h < old => WideScan::From(h),
            (Some(WideScan::From(old)), None) if birth.is_some_and(|b| old <= b) => return Ok(()),
            (Some(WideScan::Done | WideScan::From(_)), None) => WideScan::Birth,
            _ => return Ok(()),
        };
        self.mark_pending(id, lower).await
    }

    /// Stores a pending [`WIDE_SCAN_SETTING`] of wallet `id`, durable when
    /// it returns `Ok`: wallet.sqlite may store a later checkpoint as soon as
    /// the wallet is published.
    pub(crate) async fn mark_pending(
        &self,
        id: WalletId,
        marker: WideScan,
    ) -> Result<(), EngineError> {
        #[cfg(test)]
        if fails_for(&FAIL_WIDE_SCAN_IO, id) {
            return Err(EngineError::Storage("test: marker not stored".into()));
        }
        let scope = dw_appdb::local_scope(&id.to_string());
        let value = marker.encode();
        let stored = self
            .appdb_op(move |db| db.set_setting_durable(&scope, WIDE_SCAN_SETTING, Some(&value)))
            .await;
        if let Err(e) = &stored {
            tracing::warn!(wallet_id = %id, error = %e, "could not store that the wallet needs a replay with wide CoinJoin pools");
        }
        stored
    }

    /// Stores wallet `id`'s [`WIDE_SCAN_SETTING`] once it is done, or puts a
    /// replaced one back; a failure is logged and leaves a pending one.
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
    /// record first; one below the wallet's birth lowers the birth first
    /// ([`Self::lower_birth_height`]). Returns whether all of it was done;
    /// failures are logged.
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
        if let Err(e) = (self.lower_birth_height(manager, id, published_at.saturating_add(1))).await
        {
            tracing::warn!(wallet_id = %id, error = %e, "could not lower the birth height to replay below it");
            return false;
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

    /// A removed wallet whose import failed after it was readmitted, or a
    /// new wallet whose first import failed, is recorded as removed (again),
    /// with what the import wrote of it, so a later import of the seed
    /// starts from no rows, unless
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
