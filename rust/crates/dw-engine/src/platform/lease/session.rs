//! `NetworkSession`'s side of the lease table: creating leases from vault
//! grants, the locks, the revoking vault calls (§8.6), the per-wallet
//! revocations, the background lease (§4.5) and the journal's open and
//! load (§6.5).

use std::path::Path;
use std::sync::Arc;

use dw_appdb::dispatch::{DISPATCH_DB_FILE, DispatchJournal, DispatchRow, JournalOpen, StepRow};
use dw_vault::{GrantKind, GrantPurpose, SignerScope, Vault, VaultError, VaultStatus};
use zeroize::Zeroizing;

use super::fence::JournalBackend;
use super::lock::{Freeze, Scope};
use super::table::{Issued, Signers};
use super::{Lease, LeaseError, LockReport, parse_lease};
use crate::platform::flows::{FlowKind, RevokeCause};
use crate::platform::signers::VaultContactCrypto;
use crate::{EngineError, NetworkSession, WalletId};

/// Which vault grant kinds a flow's lease may carry: a `PlatformOp`, plus a
/// `Spend` for "Accept and pay"'s payment (§16.1).
fn kinds_for(flow: FlowKind) -> &'static [GrantKind] {
    match flow {
        FlowKind::AcceptAndPay => &[GrantKind::PlatformOp, GrantKind::Spend],
        _ => &[GrantKind::PlatformOp],
    }
}

/// While the redemption issues signers: an epoch change or a lock has
/// ended the tokens.
fn issuing(e: VaultError) -> LeaseError {
    match e {
        VaultError::Locked | VaultError::MixingOnly | VaultError::NoVault => LeaseError::Locked,
        VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => LeaseError::Invalid,
        _ => LeaseError::Vault("signer"),
    }
}

/// §4.1 step 2 on the blocking pool: checks every grant before consuming
/// any, redeems them, moves an own key into one `KeyHold`, issues every
/// signer the purposes need, and drops the tokens.
pub(crate) fn issue(
    vault: &Vault,
    wallet: &WalletId,
    flow: FlowKind,
    grants: &[String],
) -> Result<Issued, LeaseError> {
    let epoch_before = vault.epoch();
    let allowed = kinds_for(flow);
    let mut kinds = Vec::with_capacity(grants.len());
    for g in grants {
        let kind = allowed
            .iter()
            .copied()
            .find(|k| vault.check_grant(g, *k, Some(&wallet.0)).is_ok())
            .ok_or(LeaseError::Invalid)?;
        if kinds.contains(&kind) {
            // One grant per kind; a set is summed per purpose by kind.
            return Err(LeaseError::Invalid);
        }
        kinds.push(kind);
    }
    if kinds.is_empty() {
        return Err(LeaseError::Invalid);
    }
    let mut tokens = grants
        .iter()
        .zip(&kinds)
        .map(|(g, k)| vault.redeem_grant(g, *k, Some(&wallet.0)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| match e {
            VaultError::Locked | VaultError::MixingOnly => LeaseError::Locked,
            _ => LeaseError::Invalid,
        })?;
    if tokens.iter().any(|t| t.wallet() != Some(wallet.0)) {
        return Err(LeaseError::Invalid);
    }
    let key = vault.hold_key(&mut tokens).map_err(issuing)?;
    let mut out = Issued {
        key: None,
        signers: Signers::default(),
        epoch_before,
        ..Default::default()
    };
    for token in &tokens {
        let scoped = |scope| match &key {
            Some(hold) => vault.platform_signer_held(&wallet.0, hold, token, scope),
            None => vault.platform_signer(&wallet.0, token, scope),
        };
        match token.purpose() {
            GrantPurpose::PlatformOp {
                max_duffs,
                max_credits,
            } => {
                out.funding = max_duffs;
                out.credits = max_credits;
                out.crypto = true;
                if max_duffs > 0 {
                    out.signers.funding =
                        Some(scoped(SignerScope::PlatformFunding { max_duffs }).map_err(issuing)?);
                }
                if max_credits > 0 {
                    out.signers.identity =
                        Some(scoped(SignerScope::PlatformIdentity).map_err(issuing)?);
                }
                out.signers.crypto = Some(scoped(SignerScope::DashPayCrypto).map_err(issuing)?);
            }
            GrantPurpose::Spend { max_duffs } => {
                out.spend = max_duffs;
                let signer = match &key {
                    Some(hold) => vault.signer_held(&wallet.0, hold, token),
                    None => vault.signer(&wallet.0, token),
                };
                out.signers.spend = Some(signer.map_err(issuing)?);
            }
            _ => return Err(LeaseError::Invalid),
        }
    }
    drop(tokens);
    out.key = key;
    out.epoch_after = vault.epoch();
    Ok(out)
}

impl NetworkSession {
    /// The session's lease table; the library's fence wiring (P2b, P4)
    /// and DP1-02 reach the fence through it.
    pub fn lease_table(&self) -> &Arc<super::LeaseTable> {
        &self.leases
    }

    /// Creates a lease for `flow` from vault grants (§4.1, H8). Waits while
    /// a lock's barrier is set; `LeaseError::Locked` when a lock landed
    /// while it redeemed.
    /// Callers outside the crate go through `begin_flow`, which re-checks
    /// the wallet after a removal's barrier.
    pub(crate) async fn begin_lease(
        self: &Arc<Self>,
        wallet: WalletId,
        flow: FlowKind,
        grants: &[String],
    ) -> Result<Lease, LeaseError> {
        let vault = self.vault.clone();
        let epoch_vault = self.vault.clone();
        let grants = grants.to_vec();
        self.leases
            .begin(
                wallet,
                flow,
                move || issue(&vault, &wallet, flow, &grants),
                move || epoch_vault.epoch(),
            )
            .await
    }

    /// §4.3's rebind of a lease in `NeedsGrant` with fresh grants.
    pub async fn rebind_lease(&self, lease: &Lease, grants: &[String]) -> Result<(), LeaseError> {
        let vault = self.vault.clone();
        let epoch_vault = self.vault.clone();
        let (wallet, flow, grants) = (lease.wallet, lease.flow, grants.to_vec());
        lease
            .rebind(
                move || issue(&vault, &wallet, flow, &grants),
                move || epoch_vault.epoch(),
            )
            .await
    }

    /// The lease a facade call's `grant` names, when it is a lease id
    /// (`None`: a vault grant id). Another wallet's or an unknown lease is
    /// `Invalid` (§16.1).
    pub(crate) fn lease_for(
        &self,
        wallet: &WalletId,
        grant: &str,
    ) -> Option<Result<Lease, LeaseError>> {
        let id = parse_lease(grant)?;
        Some(self.leases.lease(&id, wallet))
    }

    /// Locks the vault (§8.1): revokes every lease and drops the background
    /// leases before the first await, runs this request's own vault gate,
    /// and returns after the drain, within `max(H, T_gate)` of the call.
    /// Cancels a pending [`Self::relock_after`] timer.
    pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError> {
        let _op = self.try_enter()?;
        self.manager()?;
        self.cancel_relock();
        let this = Arc::clone(self);
        Ok(self
            .leases
            .lock(RevokeCause::Lock, move || this.vault_gate())
            .await)
    }

    /// The synchronous lock (`Vault.lock()`, the console, the relock
    /// timer): the freeze and this request's vault gate inline; the drain
    /// continues in the background and ends with `LockProgress::Done`.
    /// Never block-waits on the runtime. Revokes leases even on a vault
    /// that is already locked.
    pub fn lock_vault_sync(&self) -> Result<VaultStatus, EngineError> {
        let _op = self.try_enter()?;
        self.manager()?;
        self.cancel_relock();
        Ok(self.lock_now())
    }

    /// A lock without admission (the relock timer holds its own).
    pub(crate) fn lock_now(&self) -> VaultStatus {
        self.leases
            .lock_sync(RevokeCause::Lock, || self.vault_gate())
    }

    /// One lock request's vault gate: ends the epoch and every grant issued
    /// before it (E0-03 `Vault::lock`).
    fn vault_gate(&self) -> VaultStatus {
        let before = self.vault.lock_state();
        let status = self.vault.lock();
        self.platform.note_lock();
        self.emit_lock_state_change(before);
        self.leases.observe_epoch(self.vault.epoch());
        status
    }

    /// A vault call that ends the epoch on purpose (§8.6): encrypt,
    /// recover, destroy (the passphrase change is
    /// [`Self::change_passphrase`]). Freezes with `cause` before the call,
    /// runs the call as its gate and returns after the drain, then
    /// re-creates the background leases where the vault stays prompt-free.
    pub async fn revoking_vault_op<T, F>(
        self: &Arc<Self>,
        cause: RevokeCause,
        f: F,
    ) -> Result<T, EngineError>
    where
        F: FnOnce(&Vault) -> Result<T, VaultError> + Send + 'static,
        T: Send + 'static,
    {
        self.checked_revoking_vault_op(cause, |_| Ok(()), move |v, ()| f(v))
            .await
    }

    /// Changes the vault passphrase (QT-111). The old passphrase is
    /// verified, and the new one validated, before anything is revoked
    /// (DEC-134): a refusal returns its error and every lease stays as it
    /// was. A correct one revokes every lease like any revoking call.
    pub async fn change_passphrase(
        self: &Arc<Self>,
        old: Zeroizing<Vec<u8>>,
        new: Zeroizing<Vec<u8>>,
    ) -> Result<VaultStatus, EngineError> {
        self.checked_revoking_vault_op(
            RevokeCause::PassphraseChange,
            move |v| v.check_passphrase_change(&old, &new),
            |v, change| v.apply_passphrase_change(change),
        )
        .await
    }

    /// [`Self::revoking_vault_op`] after `check`, which runs on the blocking
    /// pool before the freeze: its refusal revokes nothing.
    async fn checked_revoking_vault_op<P, T, C, F>(
        self: &Arc<Self>,
        cause: RevokeCause,
        check: C,
        f: F,
    ) -> Result<T, EngineError>
    where
        C: FnOnce(&Vault) -> Result<P, VaultError> + Send + 'static,
        P: Send + 'static,
        F: FnOnce(&Vault, P) -> Result<T, VaultError> + Send + 'static,
        T: Send + 'static,
    {
        let _op = self.enter().await?;
        self.manager()?;
        let vault = self.vault.clone();
        let checked = self.rt.spawn_blocking(move || check(&vault)).await??;
        let mut freeze = self.leases.freeze(cause, Scope::All, true);
        let this = Arc::clone(self);
        let task = self.rt.spawn(async move {
            let gate = Arc::clone(&this);
            let out = this
                .rt
                .spawn_blocking(move || {
                    let before = gate.vault.lock_state();
                    let out = f(&gate.vault, checked);
                    gate.emit_lock_state_change(before);
                    gate.leases.observe_epoch(gate.vault.epoch());
                    out
                })
                .await;
            freeze.gate_done();
            freeze.drain().await;
            freeze.finish(this.vault.status());
            this.ensure_background().await;
            out
        });
        task.await??.map_err(EngineError::from)
    }

    /// One wallet's revocation (§8.6: removal, "Close Wallet"): its leases
    /// revoked, its background lease dropped, its permits drained. Runs in
    /// the caller's spawned operation, which holds the returned barrier
    /// until the wallet is gone: a lease begun later finds no wallet.
    #[must_use = "the barrier is released when this is dropped"]
    pub(crate) async fn revoke_wallet(&self, wallet: WalletId, cause: RevokeCause) -> Freeze {
        let mut freeze = self.leases.freeze(cause, Scope::Wallet(wallet), false);
        freeze.drained().await;
        freeze
    }

    /// Erases a removed wallet's journal rows (§6.5, DEC-134), under the
    /// removal's barrier.
    pub(crate) async fn erase_dispatch_rows(&self, wallet: WalletId) {
        self.leases.erase_wallet_rows(wallet).await;
    }

    /// Close's lease steps (§8.5), before the session's gate closes: the
    /// freeze, the drain, then every flow task aborted.
    pub(crate) async fn close_leases(&self) {
        let mut freeze = self.leases.freeze(RevokeCause::Close, Scope::All, false);
        freeze.drain().await;
        freeze.finish(self.vault.status());
        self.leases.abort_tasks();
    }

    /// The background `DashPayCrypto` of `wallet` (§4.5), created on
    /// demand under H8's rule. `None` while the vault is not prompt-free.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "DashPay contact sync (DP2-01) is its consumer")
    )]
    pub(crate) async fn background_crypto(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Option<VaultContactCrypto> {
        if let Some(c) = self.leases.background(&wallet) {
            return Some(c);
        }
        self.create_background(wallet).await;
        self.leases.background(&wallet)
    }

    async fn create_background(self: &Arc<Self>, wallet: WalletId) {
        let gens = self.leases.wait_clear(&wallet).await;
        let vault = self.vault.clone();
        let made = tokio::task::spawn_blocking(move || {
            let epoch = vault.epoch();
            let signer = vault.dashpay_crypto_signer(&wallet.0).ok()?;
            Some((epoch, VaultContactCrypto::new(signer).ok()?))
        })
        .await;
        if let Ok(Some((epoch, crypto))) = made {
            self.leases.insert_background(wallet, gens, epoch, crypto);
        }
    }

    /// Creates the background lease of every wallet with a seed that lacks
    /// one (bring-up, after an epoch change that keeps the vault
    /// prompt-free, §4.5). A no-op on a locked or mixing-only vault.
    pub(crate) async fn ensure_background(self: &Arc<Self>) {
        if !matches!(
            self.vault.lock_state(),
            dw_vault::LockState::Unlocked
                | dw_vault::LockState::Unencrypted
                | dw_vault::LockState::NoKeys
        ) {
            return;
        }
        for wallet in self.vault.status().wallets_with_secrets {
            let wallet = WalletId(wallet);
            // A removal whose secret wipe failed leaves its seed behind.
            if self.leases.background(&wallet).is_none() && self.require_wallet(&wallet).is_ok() {
                self.create_background(wallet).await;
            }
        }
    }

    pub(crate) fn spawn_ensure_background(self: &Arc<Self>) {
        let this = Arc::clone(self);
        self.rt.spawn(async move { this.ensure_background().await });
    }
}

/// Opens and loads `dispatch.sqlite` (§6.2, §6.5). `None`: the journal is
/// unavailable (unopenable, or a newer schema); the fence then refuses
/// leased Firsts and registrations.
pub(crate) async fn open_journal(
    data_dir: &Path,
) -> Option<(Arc<dyn JournalBackend>, Vec<DispatchRow>, Vec<StepRow>)> {
    if let Err(e) = crate::fsutil::create_owned_file(data_dir, Path::new(DISPATCH_DB_FILE)) {
        tracing::warn!(error = %e, "could not create the dispatch journal");
        return None;
    }
    let path = data_dir.join(DISPATCH_DB_FILE);
    let opened = tokio::task::spawn_blocking(move || {
        let journal = match DispatchJournal::open(&path, crate::events::unix_now())? {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => {
                tracing::warn!(schema = v, "the dispatch journal is newer than this build");
                return Ok(None);
            }
        };
        let (rows, steps) = journal.load()?;
        Ok::<_, dw_appdb::AppDbError>(Some((journal, rows, steps)))
    })
    .await;
    match opened {
        Ok(Ok(Some((journal, rows, steps)))) => {
            Some((Arc::new(journal) as Arc<dyn JournalBackend>, rows, steps))
        }
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not open the dispatch journal");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not open the dispatch journal");
            None
        }
    }
}
