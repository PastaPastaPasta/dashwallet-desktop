//! `NetworkSession`'s side of the lease table: creating leases from vault
//! grants, the locks, the revoking vault calls (§8.6), the per-wallet
//! revocations, the background lease (§4.5) and the journal's open and
//! load (§6.5).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use dw_appdb::dispatch::{
    CallRows, DISPATCH_DB_FILE, DispatchJournal, DispatchRow, JournalOpen, StepRow,
};
use dw_vault::{GrantKind, GrantPurpose, SignerLife, SignerScope, Vault, VaultError, VaultStatus};
use key_wallet::bip32::DerivationPath;
use zeroize::Zeroizing;

use super::fence::JournalBackend;
use super::lock::{Freeze, Scope};
use super::table::{Issued, Signers};
use super::{Lease, LeaseError, LockReport, parse_lease};
use crate::platform::flows::{FlowKind, RevokeCause};
use crate::platform::payments::PaymentSource;
use crate::platform::signers::VaultContactCrypto;
use crate::{EngineError, NetworkSession, WalletId};

use crate::platform::shielded_pool::drain_passes;

/// Which vault grant kinds a flow's lease may carry: a `PlatformOp`, plus a
/// `Spend` for "Accept and pay"'s payment (§16.1); a contact payment's own
/// lease only the `Spend` (DP3-01), as a proposal's fee (MG-06 D10) and a
/// CoinJoin drain its coins (0.2 MIX-SHIELD: no `PlatformOp` grant reaches a
/// CoinJoin coin); the masternode flows their own grant alone (MG-05 §4), as
/// a masternode withdrawal (MG-13 P12).
fn kinds_for(flow: FlowKind) -> &'static [GrantKind] {
    match flow {
        FlowKind::AcceptAndPay => &[GrantKind::PlatformOp, GrantKind::Spend],
        FlowKind::ContactPayment | FlowKind::Proposal | FlowKind::ShieldDrain => {
            &[GrantKind::Spend]
        }
        FlowKind::Voting => &[GrantKind::Governance],
        FlowKind::MasternodeRegistration
        | FlowKind::KeyChange
        | FlowKind::SharedMasternode
        | FlowKind::Dissolution => &[GrantKind::MasternodeOp],
        FlowKind::MasternodeWithdraw => &[GrantKind::MasternodeWithdraw],
        FlowKind::AppConnect => &[GrantKind::AppConnect],
        FlowKind::AppDisconnect => &[GrantKind::AppDisconnect],
        FlowKind::Registration
        | FlowKind::TopUp
        | FlowKind::Withdraw
        | FlowKind::NameRegistration
        | FlowKind::ProfileEdit
        | FlowKind::ContactRequest
        | FlowKind::Accept
        | FlowKind::PrivateDetails
        | FlowKind::EnableDashPayKeys
        | FlowKind::PlatformAddressMove
        | FlowKind::PlatformAddressSend
        | FlowKind::ContactPayFromBalance
        | FlowKind::Shield
        | FlowKind::PlatformAddressFund
        | FlowKind::ShieldedSpend => &[GrantKind::PlatformOp],
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

/// An identity key a write signs with: its DIP-13 path and on-chain data.
pub(crate) type HeldKey = (DerivationPath, Vec<u8>);

/// §4.1 step 2 on the blocking pool: checks every grant before consuming
/// any, redeems them, moves an own key into one `KeyHold`, issues every
/// signer the purposes need, and drops the tokens. With `platform_address`,
/// a `Registration` lease with credits also carries the Platform-address
/// signer (a registration paid from the Platform addresses, C-funding F5).
/// With `keys`, the
/// `PlatformOp` grant is redeemed only while the vault holds one of them
/// (dw-vault `redeem_grant_holding_key`), else `KeysNotHeld` with every
/// grant unused (none given included).
/// `source`: a `ContactPayFromBalance` payment's (C-funding P15), whose
/// lease carries that source's key class alone; `Shielded` for a
/// `Registration` paid from the Shielded balance (F5), whose lease carries
/// the shielded spend key beside the identity signer and no funding signer.
pub(crate) fn issue(
    vault: &Vault,
    wallet: &WalletId,
    (flow, source): (FlowKind, Option<PaymentSource>),
    grants: &[String],
    keys: Option<&[HeldKey]>,
    platform_address: bool,
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
    // Window grants are capped as a set, not one by one. A passphrase grant
    // is not part of that sum.
    vault
        .window_lease_allowed(grants)
        .map_err(|_| LeaseError::Invalid)?;
    // A Shielded payout's grant must cap credits; refused before it is
    // redeemed.
    if flow == FlowKind::ContactPayFromBalance && source == Some(PaymentSource::Shielded) {
        for g in grants {
            let purpose = vault.grant_purpose(g, GrantKind::PlatformOp, Some(&wallet.0));
            if !matches!(purpose, Ok(GrantPurpose::PlatformOp { max_credits, .. }) if max_credits > 0)
            {
                return Err(LeaseError::Invalid);
            }
        }
    }
    // The key-checked redemption first, so a key the vault does not hold
    // leaves every grant unused.
    let mut order: Vec<_> = grants.iter().zip(&kinds).collect();
    order.sort_by_key(|(_, k)| {
        !matches!(
            **k,
            GrantKind::PlatformOp | GrantKind::AppConnect | GrantKind::AppDisconnect
        )
    });
    let redeem = |g: &String, kind: GrantKind| match (kind, keys) {
        (GrantKind::PlatformOp | GrantKind::AppConnect | GrantKind::AppDisconnect, Some(keys)) => {
            vault
                .redeem_grant_holding_key(g, &wallet.0, keys)?
                .ok_or(VaultError::NoSecret)
        }
        _ => vault.redeem_grant(g, kind, Some(&wallet.0)),
    };
    let mut tokens = order
        .into_iter()
        .map(|(g, k)| redeem(g, *k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| match e {
            VaultError::Locked | VaultError::MixingOnly => LeaseError::Locked,
            VaultError::NoSecret if keys.is_some() => LeaseError::KeysNotHeld,
            VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => LeaseError::Invalid,
            // The key check reads the seed: a vault fault, not the grant's.
            _ => LeaseError::Vault("redemption"),
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
            GrantPurpose::PlatformOp { max_credits, .. }
                if matches!(
                    flow,
                    FlowKind::PlatformAddressMove
                        | FlowKind::PlatformAddressSend
                        | FlowKind::ContactPayFromBalance
                ) && source != Some(PaymentSource::Shielded) =>
            {
                // "Move to Dash wallet" (release 0.1), a send to a Platform
                // address (0.2) and a contact payout from the Platform
                // balance (C-funding P15): the DIP-17 payment keys alone, no
                // identity key and no contact crypto.
                out.credits = max_credits;
                if max_credits > 0 {
                    out.signers.platform_address =
                        Some(scoped(SignerScope::PlatformAddress).map_err(issuing)?);
                }
            }
            GrantPurpose::PlatformOp { max_credits, .. }
                if flow == FlowKind::ContactPayFromBalance =>
            {
                // A contact payment from the Shielded balance (C-funding
                // P15): the shielded spend key and the contact crypto its
                // address reservation drains through; no identity key, no
                // funding signer.
                if source != Some(PaymentSource::Shielded) || max_credits == 0 {
                    return Err(LeaseError::Invalid);
                }
                out.credits = max_credits;
                out.crypto = true;
                let spend_key = match &key {
                    Some(hold) => vault.shielded_spend_key_held(&wallet.0, hold, token),
                    None => vault.shielded_spend_key(&wallet.0, token),
                };
                out.signers.shielded = Some(spend_key.map_err(issuing)?);
                out.signers.crypto = Some(scoped(SignerScope::DashPayCrypto).map_err(issuing)?);
            }
            GrantPurpose::PlatformOp { max_duffs, .. } if flow == FlowKind::PlatformAddressFund => {
                // "Add from Dash wallet" (0.2): the asset-lock credit keys
                // alone, no identity key and no contact crypto.
                out.funding = max_duffs;
                if max_duffs > 0 {
                    out.signers.funding =
                        Some(scoped(SignerScope::PlatformFunding { max_duffs }).map_err(issuing)?);
                }
            }
            GrantPurpose::PlatformOp {
                max_duffs,
                max_credits,
            } if flow == FlowKind::Registration && source == Some(PaymentSource::Shielded) => {
                // A registration paid from the Shielded balance (C-funding
                // F5): the identity signer, the shielded spend key and the
                // contact crypto; never a funding signer.
                if max_duffs > 0 || max_credits == 0 || platform_address {
                    return Err(LeaseError::Invalid);
                }
                out.credits = max_credits;
                out.crypto = true;
                out.signers.identity =
                    Some(scoped(SignerScope::PlatformIdentity).map_err(issuing)?);
                let spend_key = match &key {
                    Some(hold) => vault.shielded_spend_key_held(&wallet.0, hold, token),
                    None => vault.shielded_spend_key(&wallet.0, token),
                };
                out.signers.shielded = Some(spend_key.map_err(issuing)?);
                out.signers.crypto = Some(scoped(SignerScope::DashPayCrypto).map_err(issuing)?);
            }
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
                if max_credits > 0 && flow == FlowKind::ShieldedSpend {
                    // The shielded spend key, and no identity key (§7 item 72).
                    let spend_key = match &key {
                        Some(hold) => vault.shielded_spend_key_held(&wallet.0, hold, token),
                        None => vault.shielded_spend_key(&wallet.0, token),
                    };
                    out.signers.shielded = Some(spend_key.map_err(issuing)?);
                } else if max_credits > 0 {
                    out.signers.identity =
                        Some(scoped(SignerScope::PlatformIdentity).map_err(issuing)?);
                    if platform_address && flow == FlowKind::Registration {
                        out.signers.platform_address =
                            Some(scoped(SignerScope::PlatformAddress).map_err(issuing)?);
                    }
                }
                out.signers.crypto = Some(scoped(SignerScope::DashPayCrypto).map_err(issuing)?);
            }
            GrantPurpose::Spend { max_duffs } if flow == FlowKind::ShieldDrain => {
                // "Move mixed coins" to Shielded: the drain's inputs and the
                // lock's credit key alone, as an asset lock's funding; no
                // spend signer, no other input, no identity key, no contact
                // crypto. The signer stops when the lease drops its life,
                // whatever key it signs with.
                out.funding = max_duffs;
                if max_duffs > 0 {
                    let life = SignerLife::new();
                    let signer = match &key {
                        Some(hold) => {
                            vault.shield_drain_signer_held(&wallet.0, hold, token, max_duffs, &life)
                        }
                        None => vault.shield_drain_signer(&wallet.0, token, max_duffs, &life),
                    };
                    out.signers.funding = Some(signer.map_err(issuing)?);
                    out.signers.life = Some(life);
                }
            }
            GrantPurpose::Spend { max_duffs } => {
                out.spend = max_duffs;
                let signer = match &key {
                    Some(hold) => vault.signer_held(&wallet.0, hold, token),
                    None => vault.signer(&wallet.0, token),
                };
                out.signers.spend = Some(signer.map_err(issuing)?);
            }
            GrantPurpose::MasternodeOp { max_duffs } => {
                out.masternode = max_duffs;
                let signer = match &key {
                    Some(hold) => vault.masternode_signer_held(&wallet.0, hold, token),
                    None => vault.masternode_signer(&wallet.0, token),
                };
                out.signers.masternode = Some(signer.map_err(issuing)?);
            }
            GrantPurpose::MasternodeWithdraw { max_credits, .. } => {
                // MG-13 P12: the credits the withdrawal and its fee take;
                // the signer is single use (dw-vault, r2 m5).
                out.credits = max_credits;
                let signer = match &key {
                    Some(hold) => vault.masternode_withdrawal_signer_held(&wallet.0, hold, token),
                    None => vault.masternode_withdrawal_signer(&wallet.0, token),
                };
                out.signers.withdrawal = Some(signer.map_err(issuing)?);
            }
            GrantPurpose::AppConnect {
                max_credits,
                request,
                ..
            } if flow == FlowKind::AppConnect => {
                // An App Connect login: the identity keys (the MASTER key's
                // update, the document key's response) and the request's
                // application keys; no funding signer, no contact crypto.
                out.credits = max_credits;
                out.signers.identity =
                    Some(scoped(SignerScope::PlatformIdentity).map_err(issuing)?);
                out.signers.app_connect =
                    Some(scoped(SignerScope::AppConnect { request }).map_err(issuing)?);
            }
            GrantPurpose::AppDisconnect { max_credits, .. } if flow == FlowKind::AppDisconnect => {
                // Disconnecting an App Connect login: the identity keys
                // (MASTER) alone; no AppConnect signer, no funding signer,
                // no contact crypto.
                out.credits = max_credits;
                out.signers.identity =
                    Some(scoped(SignerScope::PlatformIdentity).map_err(issuing)?);
            }
            GrantPurpose::Governance => {
                out.voting = true;
                let signer = match &key {
                    Some(hold) => vault.vote_signer_held(&wallet.0, hold, token),
                    None => vault.vote_signer(&wallet.0, token),
                };
                out.signers.vote = Some(signer.map_err(issuing)?);
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
        self.begin_issuing((wallet, flow, None), grants, None, false)
            .await
    }

    /// A `Registration` lease; `from_platform`: it also carries the
    /// Platform-address signer (C-funding F5).
    pub(crate) async fn begin_registration_lease(
        self: &Arc<Self>,
        wallet: WalletId,
        grants: &[String],
        from_platform: bool,
    ) -> Result<Lease, LeaseError> {
        let flow = FlowKind::Registration;
        self.begin_issuing((wallet, flow, None), grants, None, from_platform)
            .await
    }

    /// A `Registration` lease paid from the Shielded balance: the identity
    /// signer and the shielded spend key, no funding signer (C-funding F5).
    pub(crate) async fn begin_shielded_registration_lease(
        self: &Arc<Self>,
        wallet: WalletId,
        grants: &[String],
    ) -> Result<Lease, LeaseError> {
        let scope = (
            wallet,
            FlowKind::Registration,
            Some(PaymentSource::Shielded),
        );
        self.begin_issuing(scope, grants, None, false).await
    }

    /// [`Self::begin_lease`] for a `ContactPayFromBalance` payment from
    /// `source` (C-funding P15).
    pub(crate) async fn begin_payout_lease(
        self: &Arc<Self>,
        wallet: WalletId,
        source: PaymentSource,
        grants: &[String],
    ) -> Result<Lease, LeaseError> {
        let flow = FlowKind::ContactPayFromBalance;
        self.begin_issuing((wallet, flow, Some(source)), grants, None, false)
            .await
    }

    /// [`Self::begin_lease`] for a write that signs with one of `keys`:
    /// its `PlatformOp` grant is redeemed only while the vault holds one
    /// ([`issue`]); `KeysNotHeld` with the grants unused otherwise (DP2-02).
    pub(crate) async fn begin_lease_holding(
        self: &Arc<Self>,
        wallet: WalletId,
        flow: FlowKind,
        grants: &[String],
        keys: Vec<HeldKey>,
    ) -> Result<Lease, LeaseError> {
        self.begin_issuing((wallet, flow, None), grants, Some(keys), false)
            .await
    }

    async fn begin_issuing(
        self: &Arc<Self>,
        (wallet, flow, source): (WalletId, FlowKind, Option<PaymentSource>),
        grants: &[String],
        keys: Option<Vec<HeldKey>>,
        platform_address: bool,
    ) -> Result<Lease, LeaseError> {
        if flow.moves_funds() {
            let gate = &self.hub.restore;
            if flow.needs_platform() && self.platform.funds_off() {
                return Err(LeaseError::PlatformUnavailable);
            }
            if gate.is_restored()
                || (flow.moves_platform_funds() && self.wallet_platform_held(wallet))
            {
                return Err(LeaseError::Restored);
            }
        }
        let vault = self.vault.clone();
        let epoch_vault = self.vault.clone();
        let grants = grants.to_vec();
        self.leases
            .begin(
                wallet,
                flow,
                move || {
                    let (scope, keys) = ((flow, source), keys.as_deref());
                    issue(&vault, &wallet, scope, &grants, keys, platform_address)
                },
                move || epoch_vault.epoch(),
            )
            .await
    }

    /// §4.3's rebind of a lease in `NeedsGrant` with fresh grants. A
    /// contact payout's lease pays once and is never rebound: `Invalid`
    /// before the grants are redeemed (a host begins another flow).
    pub async fn rebind_lease(&self, lease: &Lease, grants: &[String]) -> Result<(), LeaseError> {
        self.rebind_lease_scoped(lease, grants, (false, None)).await
    }

    /// [`Self::rebind_lease`]; `platform_address` and `source` as
    /// [`issue`] takes them.
    pub(crate) async fn rebind_lease_scoped(
        &self,
        lease: &Lease,
        grants: &[String],
        (platform_address, source): (bool, Option<PaymentSource>),
    ) -> Result<(), LeaseError> {
        if lease.flow == FlowKind::ContactPayFromBalance {
            return Err(LeaseError::Invalid);
        }
        let vault = self.vault.clone();
        let epoch_vault = self.vault.clone();
        let (wallet, flow, grants) = (lease.wallet, lease.flow, grants.to_vec());
        lease
            .rebind(
                move || {
                    let scope = (flow, source);
                    issue(&vault, &wallet, scope, &grants, None, platform_address)
                },
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
    /// and returns after the drain, within `max(H, T_gate)` of the call,
    /// and after a shielded pass in flight ends ([`Self::drain_shielded_pass`]).
    /// Cancels a pending [`Self::relock_after`] timer.
    pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError> {
        let _op = self.try_enter()?;
        self.manager()?;
        self.cancel_relock();
        let this = Arc::clone(self);
        let passes = Arc::clone(&self.platform.shielded_passes);
        let fence = Arc::clone(&self.platform.hook_fence);
        let report = (self.leases)
            .lock_then(RevokeCause::Lock, move || this.vault_gate(), async move {
                drain_passes(&passes, &fence).await
            })
            .await;
        Ok(report)
    }

    /// A shielded pass admitted under the background leases the lock just
    /// dropped ends before the lock completes, at its `LockProgress::Done`
    /// (the async lock's return; the synchronous lock returns before it)
    /// (0.1 E1, DEC-1032): it may re-send
    /// a stored spend, as a leased call admitted before a freeze may finish
    /// in its drain. Every pass holds the pass lock to read from its
    /// admission to its end; once this write is had, the next pass is
    /// admitted only while each wallet holding such a spend holds its
    /// background lease again (`shielded_pool::admit_pass`). It waits for
    /// the pass's end, which the SDK's request settings bound
    /// (`shielded_pool::drain_passes`).
    pub(crate) async fn drain_shielded_pass(&self) {
        drain_passes(&self.platform.shielded_passes, &self.platform.hook_fence).await;
    }

    /// The synchronous lock (`Vault.lock()`, the console, the relock
    /// timer): the freeze and this request's vault gate inline; the drain,
    /// a shielded pass in flight's among it, continues in the background
    /// and ends with `LockProgress::Done`.
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
        // A shielded pass admitted before the freeze ends before `Done`
        // (`drain_shielded_pass`); none is admitted after it for a wallet
        // holding a stored spend (its background lease is gone).
        let passes = Arc::clone(&self.platform.shielded_passes);
        let fence = Arc::clone(&self.platform.hook_fence);
        self.leases
            .lock_sync_then(RevokeCause::Lock, || self.vault_gate(), async move {
                drain_passes(&passes, &fence).await
            })
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
            // The background leases went with the freeze: a shielded pass
            // admitted under them ends first (DEC-1032).
            this.drain_shielded_pass().await;
            freeze.finish(this.vault.status());
            this.ensure_background().await;
            out
        });
        let out = task.await??.map_err(EngineError::from)?;
        self.back_up_after_credential_change();
        Ok(out)
    }

    /// One wallet's revocation (§8.6: removal, "Close Wallet"): its leases
    /// revoked, its background lease dropped, its permits drained. Runs in
    /// the caller's spawned operation, which holds the returned barrier
    /// until the wallet is gone: a lease begun later finds no wallet.
    #[must_use = "the barrier is released when this is dropped"]
    pub(crate) async fn revoke_wallet(&self, wallet: WalletId, cause: RevokeCause) -> Freeze {
        let mut freeze = self.leases.freeze(cause, Scope::Wallet(wallet), false);
        freeze.drained().await;
        // The wallet's background lease went with the freeze: a shielded
        // pass admitted under it ends first (DEC-1032).
        self.drain_shielded_pass().await;
        freeze
    }

    /// `storage` while the dispatch journal is not read or is unavailable,
    /// as a leased First is refused then: a wallet's import, restore and
    /// removal change which of its rows stand (the removal archives them,
    /// the next open classifies an import's), which only a journal that is
    /// open can keep.
    pub(crate) fn require_journal(&self) -> Result<(), EngineError> {
        if self.leases.journal_ready() {
            return Ok(());
        }
        Err(EngineError::Storage(
            LeaseError::JournalUnavailable.to_string(),
        ))
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
        // The refreshes and resume passes hold the session's wallets: none
        // outlives the close (BL-544).
        self.leases.background.close().await;
    }

    /// The background `DashPayCrypto` of `wallet` (§4.5), created on
    /// demand under H8's rule. `None` while the vault is not prompt-free.
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
        let made = self
            .rt
            .spawn_blocking(move || {
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

/// What the open loads: the journal and its rows.
pub(crate) type LoadedJournal = (
    Arc<dyn JournalBackend>,
    Vec<DispatchRow>,
    Vec<StepRow>,
    CallRows,
);

/// Opens and loads `dispatch.sqlite` (§6.2, §6.5), Mode B's standing rows
/// included, after reading its imported shielded spends against `known`,
/// the wallets the session knows: one whose wallet is not among them is
/// archived. `Ok(None)`: the journal is unavailable (unopenable, or a newer
/// schema); the fence then refuses leased Firsts, registrations and call
/// steps. An error: the imported spends could not be read, and the
/// session must not open (a removal or restore before they are read would
/// change which are archived); the next open reads them.
///
/// Then, before the journal is loaded, the drain of a `shielded.sqlite` an
/// earlier build wrote (Q110 110-2), beside `persister`, this start's
/// persister open: the second value is its verdict, which upstream's
/// shielded store waits for. With no journal there is no drain and the
/// verdict is a refusal, so that store stays closed.
pub(crate) async fn open_journal(
    data_dir: &Path,
    known: HashSet<[u8; 32]>,
    persister: Arc<dw_store_upgrade::PersisterOpened>,
) -> Result<(Option<LoadedJournal>, ShieldedDrainVerdict), EngineError> {
    enum Failed {
        Unavailable(dw_appdb::AppDbError),
        Unread(dw_appdb::AppDbError),
    }
    use crate::platform::ShieldedStoreClosed;
    let no_journal = Err(ShieldedStoreClosed::Failed);
    if let Err(e) = crate::fsutil::create_owned_file(data_dir, Path::new(DISPATCH_DB_FILE)) {
        tracing::warn!(error = %e, "could not create the dispatch journal");
        return Ok((None, no_journal));
    }
    let path = data_dir.join(DISPATCH_DB_FILE);
    let network_dir = data_dir.to_path_buf();
    let opened = tokio::task::spawn_blocking(move || {
        let journal = match DispatchJournal::open(&path, crate::events::unix_now())
            .map_err(Failed::Unavailable)?
        {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => {
                tracing::warn!(schema = v, "the dispatch journal is newer than this build");
                return Ok(None);
            }
        };
        let c =
            (journal.classify_shielded_imports(|w| known.contains(w))).map_err(Failed::Unread)?;
        if c != Default::default() {
            tracing::info!(
                unresolved = c.unresolved,
                archived = c.archived,
                "imported shielded spends read"
            );
        }
        let drained = drain_shielded_store(&network_dir, &persister, &journal);
        let (rows, steps) = journal.load().map_err(Failed::Unavailable)?;
        let calls = journal.load_calls().map_err(Failed::Unavailable)?;
        Ok(Some(((journal, rows, steps, calls), drained)))
    })
    .await;
    match opened {
        Ok(Ok(Some(((journal, rows, steps, calls), drained)))) => Ok((
            Some((
                Arc::new(journal) as Arc<dyn JournalBackend>,
                rows,
                steps,
                calls,
            )),
            drained,
        )),
        Ok(Ok(None)) => Ok((None, no_journal)),
        Ok(Err(Failed::Unread(e))) => Err(EngineError::Storage(format!(
            "the imported shielded spends could not be read: {e}"
        ))),
        Ok(Err(Failed::Unavailable(e))) => {
            tracing::warn!(error = %e, "could not open the dispatch journal");
            Ok((None, no_journal))
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not open the dispatch journal");
            Ok((None, no_journal))
        }
    }
}

/// Whether upstream's shielded store may open this session: `Ok` once the
/// drain left no fork rows in it, else why it stays closed.
pub(crate) type ShieldedDrainVerdict = Result<(), crate::platform::ShieldedStoreClosed>;

/// The drain of `network_dir`'s `shielded.sqlite` into `journal`
/// ([`dw_shielded_drain`]), logged.
fn drain_shielded_store(
    network_dir: &Path,
    opened: &dw_store_upgrade::PersisterOpened,
    journal: &DispatchJournal,
) -> ShieldedDrainVerdict {
    match dw_shielded_drain::drain_fork_shielded_store(network_dir, opened, journal) {
        Ok(outcome) => {
            tracing::info!(?outcome, "shielded store drain");
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "the shielded store drain did not finish; the shielded store stays closed"
            );
            Err((&e).into())
        }
    }
}
