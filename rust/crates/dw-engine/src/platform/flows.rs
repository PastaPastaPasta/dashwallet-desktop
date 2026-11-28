//! Flows, grants and the dispatch journal as the host sees them (E0-04;
//! DASHPAY §2.3, §2.6). The shapes follow E0-04 design rev2 §16, the single
//! source for this surface; a later E0-04 revision changes them under the
//! contract's version rule.
//!
//! The lease itself (`Lease`, its table and signers) is E0-04's engine
//! internals; the facade hands out its id and a view of it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::dashpay::DashPay;
use super::errors::{NameError, PlatformError};
use super::lease::{ArtifactId, parse_lease};
use super::payment_locks::payout_source;
use super::payments::PaymentSource;
use crate::{EngineError, NetworkSession, WalletId};

/// What a lease is for. It fixes which budgets its grants may fund.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    Registration,
    TopUp,
    Withdraw,
    NameRegistration,
    ProfileEdit,
    ContactRequest,
    Accept,
    /// "Accept and pay": the accept, then the payment's `TxDraft.prepare`,
    /// under one lease and one prompt (§2.3). `begin_flow` takes the
    /// payment's source (C-funding P15).
    AcceptAndPay,
    PrivateDetails,
    EnableDashPayKeys,
    /// A contact payment's `TxDraft.prepare` with a `Spend` grant alone
    /// (DP3-01): `prepare` makes one from a plain grant, so the payment
    /// goes through the dispatch fence.
    ContactPayment,
    /// A batch of governance votes under one `Governance` grant
    /// (MG-05 §4, MG-00 D35 "Voting").
    Voting,
    /// A ProRegTx's `TxDraft.prepare` under a `MasternodeOp` grant
    /// (MG-05 §4, "Masternode registration").
    MasternodeRegistration,
    /// A ProUpRegTx's `TxDraft.prepare` under a `MasternodeOp` grant
    /// (MG-05 §4, "Key change").
    KeyChange,
    /// A shared masternode's consent and funding signatures under a
    /// `MasternodeOp` grant (MG-04b §6.3, "Shared masternode").
    SharedMasternode,
    /// A shared masternode's dissolution signatures under a `MasternodeOp`
    /// grant (MG-04b §6.3, "Dissolution").
    Dissolution,
    /// A governance proposal's fee transaction under a `Spend` grant
    /// (MG-06 D10, MG-00 D35 "Proposal").
    Proposal,
    /// "Move to Dash wallet": a withdrawal from the wallet's Platform
    /// addresses to its Core receive address under a `PlatformOp` grant
    /// (m4 §7 item 67), signed with the payment-address keys alone.
    PlatformAddressMove,
    /// A send to someone else's Platform address (m4 §2.12, 0.2): the same
    /// payment-address keys as `PlatformAddressMove`, from a fresh
    /// `PlatformOp` grant redeemed for that one send (never a held lease).
    PlatformAddressSend,
    /// Core → Shielded (m4 §2.13): the asset lock under a `PlatformOp`
    /// grant's `max_duffs`, then its ShieldFromAssetLock.
    Shield,
    /// "Add from Dash wallet" (m4 §2.12, 0.2): the asset lock under a
    /// `PlatformOp` grant's `max_duffs`, then its
    /// AddressFundingFromAssetLock to the wallet's Platform address.
    PlatformAddressFund,
    /// A shielded send or an unshield to Core (m4 §2.13) under a
    /// `PlatformOp` grant's `max_credits`: the vault's Orchard spend key
    /// for the wallet's shielded account, and nothing else.
    ShieldedSpend,
    /// "Move mixed coins" to Shielded (0.2 MIX-SHIELD): the drain of the
    /// CoinJoin account into one Core → Shielded lock under a `Spend`
    /// grant's `max_duffs`, then its ShieldFromAssetLock. Its funding
    /// signer is `ShieldDrainFunding` (CoinJoin account 0's address keys
    /// and the shielded credit keys), and nothing else.
    ShieldDrain,
    /// An evonode's Platform credit withdrawal under a `MasternodeWithdraw`
    /// grant (MG-13 P12): its single-use `MasternodeWithdrawal` signer and
    /// a Credits budget charged the amount plus the fee reserve.
    MasternodeWithdraw,
    /// One `contact_pay` from the Shielded or Platform balance under a
    /// `PlatformOp` grant (C-funding P15), with the one key class its source
    /// needs; never a `ContactPayment` lease, nor the reverse.
    ContactPayFromBalance,
    /// One App Connect login under an `AppConnect` grant: the MASTER-signed
    /// IdentityUpdate adding the app's keys, then the response document,
    /// each under its own call permit. Its signers are `PlatformIdentity`
    /// and an `AppConnect` one bound to the grant's request.
    AppConnect,
    /// Disconnecting one App Connect login under an `AppDisconnect` grant:
    /// the MASTER-signed IdentityUpdate disabling the app's session keys.
    /// Its only signer is `PlatformIdentity`.
    AppDisconnect,
}

impl FlowKind {
    /// Whether the flow spends Core coins, credits or shielded notes: the
    /// restore's money gate refuses it while the restored mark is set
    /// (`restore_gate`). Profile and contact writes, DashPay keys and votes
    /// move nothing.
    pub(crate) fn moves_funds(self) -> bool {
        !matches!(
            self,
            Self::ProfileEdit
                | Self::ContactRequest
                | Self::Accept
                | Self::PrivateDetails
                | Self::EnableDashPayKeys
                | Self::Voting
        )
    }

    /// Whether the flow writes to Platform or spends credits or shielded
    /// notes: refused up front in a session without Platform. Core payments
    /// to contacts, masternode and governance transactions, and votes need
    /// Core alone.
    pub(crate) fn needs_platform(self) -> bool {
        !matches!(
            self,
            Self::ContactPayment
                | Self::Voting
                | Self::MasternodeRegistration
                | Self::KeyChange
                | Self::SharedMasternode
                | Self::Dissolution
                | Self::Proposal
        )
    }

    /// Whether the flow moves funds on Platform's side (credits, shielded
    /// notes, or Core coins into an asset lock): a restore's Platform
    /// residue refuses it. "Accept and pay" needs Platform for its accept,
    /// which moves nothing; its payment is Core's.
    pub(crate) fn moves_platform_funds(self) -> bool {
        self.moves_funds() && self.needs_platform() && self != Self::AcceptAndPay
    }
}

#[cfg(test)]
#[test]
fn app_approvals_and_disconnects_require_platform_funds() {
    for flow in [FlowKind::AppConnect, FlowKind::AppDisconnect] {
        assert!(flow.moves_funds());
        assert!(flow.needs_platform());
        assert!(flow.moves_platform_funds());
    }
}

/// A lease's budgets (E0-04 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPurpose {
    /// Core duffs for an asset lock (`PlatformOp.max_duffs`; a drain's
    /// `Spend.max_duffs`).
    Funding,
    /// Identity credits for state transitions (`PlatformOp.max_credits`).
    Credits,
    /// Core duffs for a contact payment (`Spend`).
    Spend,
    /// DashPay crypto; never hands anything off.
    Crypto,
    /// Core duffs for a masternode transaction (`MasternodeOp`): its
    /// outputs to scripts the wallet does not own plus the fee (MG-05 §2).
    Masternode,
    /// Governance votes (`Governance`); a vote moves nothing, so it is
    /// never charged (MG-05 §4).
    Voting,
}

/// Why a lease was revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeCause {
    Lock,
    Close,
    PassphraseChange,
    WalletRemoved,
    WalletClosed,
}

/// The caps of the `PlatformOp{max_duffs, max_credits}` grant an action
/// needs. The engine charges no more than it quoted; a quote made without
/// the payload (`GrantAction`) is a worst-case bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRequest {
    pub max_duffs: u64,
    pub max_credits: u64,
}

/// A write whose grant `grant_request` sizes. Registration, top-up and
/// withdraw carry theirs in their quotes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantAction {
    SendRequest,
    AcceptRequest,
    RegisterName { label: String },
    UpdateProfile,
    PublishPrivateDetails,
    EnableDashPayKeys,
}

/// What the engine knows about a handed-off artifact (E0-04 §16.6, the
/// contract's §4.1). `dispatch_status` returns `None` when the engine has no
/// entry; the host reads every `None` as unknown, exactly like `MaybeSent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    /// Committed; the engine will send it again (`platform.will_be_sent`).
    WillBeSent,
    /// It may already be out, or is still in flight
    /// (`platform.broadcast_unknown`).
    MaybeSent,
    Sent,
    /// Definitely never sent, on positive evidence: the only state that
    /// allows a retry the host offers.
    NotSent,
}

/// Broadcast evidence for a transfer that has not settled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PendingDispatch {
    Broadcast,
    #[default]
    Queued,
    Preparing,
}

impl PendingDispatch {
    pub(crate) fn from_broadcast_evidence(sent: bool) -> Self {
        if sent { Self::Broadcast } else { Self::Queued }
    }
}

/// The payload of the event sent when a provisional outcome settles, from
/// any of E0-04 §4.6's sources: a Resend accepted or the transaction seen,
/// a reload that cleans up an `Unsent` row, a row-less artifact settled
/// definitely unsent, Mode B's derived status moving, or H16's evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchResolved {
    /// Lower-case hex, as `WalletId` displays.
    pub wallet_id: String,
    /// A txid, a state-transition hash, or a funding step id
    /// (`registration/<draft>/funding`, `topup/<id>/funding`).
    pub artifact: String,
    pub resolution: DispatchResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchResolution {
    Sent,
    NotSent,
}

/// A lease as the UI sees it (E0-04 §4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseView {
    /// The first 8 hex digits of the id; for logs and the UI only.
    pub id: String,
    /// Lower-case hex.
    pub wallet_id: String,
    pub flow: FlowKind,
    pub state: LeaseStateView,
    pub own_key: bool,
    /// While an own key is held: seconds until it is dropped.
    pub key_expires_in_secs: Option<u64>,
    /// E0-04 §16.5's definition, as `RegistrationStatus.funds_committed`.
    pub funds_committed: bool,
    pub budgets: Vec<BudgetView>,
    /// Permits held now.
    pub in_flight: u32,
    /// A library call of the flow is running; selects the copy.
    pub call_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeaseStateView {
    Active,
    /// Funding handed off with an own key, until `key_expires_in_secs`.
    AwaitingProof,
    Parked {
        reason: ParkReason,
    },
    NeedsGrant,
    Revoked {
        cause: RevokeCause,
    },
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    ProofWaiting,
}

/// One budget of the lease's current generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetView {
    pub purpose: BudgetPurpose,
    pub ceiling: u64,
    pub spent: u64,
}

impl NetworkSession {
    /// Redeems `grants` into one lease for `flow` and returns its id, which
    /// the calls of its wallet accept as their `grant` for the purposes the
    /// lease carries. `source`: the payment's, for `AcceptAndPay` (`None`
    /// is `core`) and `ContactPayFromBalance`; no other flow takes one.
    pub async fn begin_flow(
        self: &Arc<Self>,
        wallet_id: WalletId,
        flow: FlowKind,
        grants: Vec<String>,
        source: Option<PaymentSource>,
    ) -> Result<String, PlatformError> {
        let _op = self.enter().await.map_err(engine)?;
        #[cfg(test)]
        crate::coinjoin::test_pause(crate::coinjoin::sweep_hooks(self).flow_admitted).await;
        self.require_wallet(&wallet_id).map_err(engine)?;
        match (flow, source) {
            // An approval or disconnect begins its own lease, under the grant
            // its quote bound (app_connect.rs): never a host's.
            (FlowKind::AppConnect | FlowKind::AppDisconnect, _) => {
                return Err(PlatformError::GrantInvalid);
            }
            (FlowKind::AcceptAndPay, None | Some(PaymentSource::Core)) => {}
            (FlowKind::ContactPayFromBalance, Some(PaymentSource::Shielded))
                if super::shielded_payout::enabled_on(&self.network) => {}
            (FlowKind::ContactPayFromBalance, Some(PaymentSource::Platform))
                if super::contact_payout::refuse_on_mainnet(self).is_ok() => {}
            (FlowKind::AcceptAndPay | FlowKind::ContactPayFromBalance, Some(source)) => {
                return Err(payout_source(source));
            }
            (FlowKind::ContactPayFromBalance, None) | (_, Some(_)) => {
                return Err(PlatformError::InvalidArgument {
                    detail: "the payment source does not fit the flow".into(),
                });
            }
            _ => {}
        }
        let lease = match (flow, source) {
            (FlowKind::ContactPayFromBalance, Some(PaymentSource::Shielded)) => {
                (self.begin_payout_lease(wallet_id, PaymentSource::Shielded, &grants)).await?
            }
            _ => self.begin_lease(wallet_id, flow, &grants).await?,
        };
        // A removal or Close Wallet holds its barrier until the wallet is
        // gone (§8.6), so a lease that waited it out finds no wallet here.
        // Dropping the owning handle ends it.
        self.require_wallet(&wallet_id).map_err(engine)?;
        let id = lease.id_string();
        // The session owns it until `end_flow`; owners of leases that ended
        // otherwise go now. A lease a lock revoked keeps its owner, so the
        // host can still read the cause, until the reaper ends it an idle
        // period later (review O-3).
        let ended: Vec<super::lease::Lease> = {
            let mut owners = self.flow_leases.lock().unwrap_or_else(|e| e.into_inner());
            let ended = owners
                .extract_if(|_, l| {
                    l.view()
                        .is_none_or(|v| matches!(v.state, LeaseStateView::Ended))
                })
                .map(|(_, l)| l)
                .collect();
            owners.insert(lease.id(), lease);
            ended
        };
        drop(ended);
        Ok(id)
    }

    /// Releases a lease: drops the session's owning handle, which ends it.
    /// Idempotent.
    pub fn end_flow(&self, lease: String) -> Result<(), PlatformError> {
        let id = parse_lease(&lease).ok_or(PlatformError::GrantInvalid)?;
        let owner = self
            .flow_leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        drop(owner);
        self.leases.end(&id);
        Ok(())
    }

    pub fn leases(&self) -> Result<Vec<LeaseView>, PlatformError> {
        Ok(self.leases.views())
    }
}

fn engine(e: EngineError) -> PlatformError {
    match e {
        EngineError::NetworkNotOpen(_) => PlatformError::NetworkNotOpen,
        EngineError::WalletNotFound(_) => PlatformError::WalletNotFound,
        other => PlatformError::Internal {
            detail: other.to_string(),
        },
    }
}

/// `grant_request`'s credit bounds per action (E0-04 §4.2, Q7; `RegisterName`
/// is DP1-03's quote), until
/// DP1-06's cost table replaces them. Derived from the pin's fee schedule
/// (rs-platform-version `fee/storage/v1.rs`: 27 000 credits per stored byte
/// plus 400 to process it; `state_transition_min_fees/v1.rs`: 100 000 per
/// document sub-transition and identity update),
/// with each document's stored size, its index entries and tree overhead
/// bounded generously:
///
/// | Action | Stored bytes bounded at | Credits |
/// |---|---|---|
/// | `SendRequest`, `AcceptRequest` (one `contactRequest`, ≤ 0.5 KB) | 4 KiB | 150 000 000 |
/// | `UpdateProfile` (`profile`, avatar URL ≤ 2 KiB) | 10 KiB | 300 000 000 |
/// | `PublishPrivateDetails` (`contactInfo`, ≤ 2.2 KiB) | 10 KiB | 300 000 000 |
/// | `EnableDashPayKeys` (identity update adding 2 keys) | 3.5 KiB | 100 000 000 |
///
/// None of them funds from Core, so `max_duffs` is 0.
pub(super) const CONTACT_REQUEST_CREDITS: u64 = 150_000_000;
pub(super) const DOCUMENT_10K_CREDITS: u64 = 300_000_000;
pub(super) const KEY_UPDATE_CREDITS: u64 = 100_000_000;

impl DashPay {
    /// `RegisterName` is quoted (DP1-03, `register_name_grant`); the other
    /// actions are E0-04's.
    pub async fn grant_request(
        &self,
        identity: String,
        action: GrantAction,
    ) -> Result<GrantRequest, PlatformError> {
        if identity.trim().is_empty() {
            return Err(PlatformError::InvalidArgument {
                detail: "identity is empty".into(),
            });
        }
        let max_credits = match action {
            GrantAction::RegisterName { label } => {
                return self.register_name_grant(label).await.map_err(|e| match e {
                    NameError::Platform(e) => e,
                    e => PlatformError::InvalidArgument {
                        detail: e.to_string(),
                    },
                });
            }
            GrantAction::SendRequest | GrantAction::AcceptRequest => CONTACT_REQUEST_CREDITS,
            GrantAction::UpdateProfile | GrantAction::PublishPrivateDetails => DOCUMENT_10K_CREDITS,
            GrantAction::EnableDashPayKeys => KEY_UPDATE_CREDITS,
        };
        Ok(GrantRequest {
            max_duffs: 0,
            max_credits,
        })
    }

    /// `None`: the engine has no entry. The host reads it as unknown, never
    /// as a reason to retry; only the engine's funding gates read an asset
    /// lock's `None`, with the tracked-row check (E0-04 §16.6).
    ///
    /// A funding step id (`registration/<draft>/funding`,
    /// `topup/<id>/funding`) reads Mode B's derived status (§2a.5); the
    /// wallet's tracked rows are read first if this session has not read
    /// them yet. Any other step id reads its marker (§7.6): a standing
    /// marker is `MaybeSent`.
    pub async fn dispatch_status(
        &self,
        artifact: String,
    ) -> Result<Option<DispatchState>, PlatformError> {
        let table = self.session.lease_table();
        let wallet = self.wallet_id();
        match ArtifactId::parse(&artifact) {
            Some(id) => Ok(table.dispatch_status(wallet, id)),
            None if is_funding_step(&artifact) => {
                if !table.has_tracked_snapshot(wallet) {
                    table.refresh_funding(wallet).await;
                }
                Ok(table.funding_status(wallet, &artifact))
            }
            None if artifact.contains('/') => {
                Ok(step_state(table.call_step(wallet, &artifact).status))
            }
            None => Err(PlatformError::InvalidArgument {
                detail: "not a txid, transition hash or step id".into(),
            }),
        }
    }
}

/// A Mode B funding step id (§2a.3): `registration/<draft>/funding`,
/// `topup/<id>/funding`, (0.1 E1) a shield's `shield/<k>/funding` or (0.2)
/// a Platform address fund's `paddr/<k>/funding`.
pub(crate) fn is_funding_step(step: &str) -> bool {
    ["registration/", "topup/", "shield/", "paddr/"]
        .iter()
        .any(|p| step.starts_with(p))
        && step.ends_with("/funding")
        && step.matches('/').count() == 2
}

/// A resumable step's `dispatch_status` from its marker (§7.6, §16.6).
fn step_state(status: super::lease::CallStepStatus) -> Option<DispatchState> {
    use super::lease::CallStepStatus as S;
    match status {
        S::None => None,
        S::Marked { .. } => Some(DispatchState::MaybeSent),
        S::NotSent => Some(DispatchState::NotSent),
        S::Sent => Some(DispatchState::Sent),
    }
}
