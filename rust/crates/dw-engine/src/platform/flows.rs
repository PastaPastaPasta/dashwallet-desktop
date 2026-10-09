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
    /// under one lease and one prompt (§2.3).
    AcceptAndPay,
    PrivateDetails,
    EnableDashPayKeys,
}

/// A lease's budgets (E0-04 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPurpose {
    /// Core duffs for an asset lock (`PlatformOp.max_duffs`).
    Funding,
    /// Identity credits for state transitions (`PlatformOp.max_credits`).
    Credits,
    /// Core duffs for a contact payment (`Spend`).
    Spend,
    /// DashPay crypto; never hands anything off.
    Crypto,
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
    /// lease carries.
    pub async fn begin_flow(
        self: &Arc<Self>,
        wallet_id: WalletId,
        flow: FlowKind,
        grants: Vec<String>,
    ) -> Result<String, PlatformError> {
        let _op = self.enter().await.map_err(engine)?;
        self.require_wallet(&wallet_id).map_err(engine)?;
        let lease = self.begin_lease(wallet_id, flow, &grants).await?;
        // A removal or Close Wallet holds its barrier until the wallet is
        // gone (§8.6), so a lease that waited it out finds no wallet here.
        // Dropping the owning handle ends it.
        self.require_wallet(&wallet_id).map_err(engine)?;
        let id = lease.id_string();
        // The session owns it until `end_flow`; owners of leases that ended
        // otherwise (reaper, lock) go now.
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
const CONTACT_REQUEST_CREDITS: u64 = 150_000_000;
const DOCUMENT_10K_CREDITS: u64 = 300_000_000;
const KEY_UPDATE_CREDITS: u64 = 100_000_000;

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
    pub async fn dispatch_status(
        &self,
        artifact: String,
    ) -> Result<Option<DispatchState>, PlatformError> {
        match ArtifactId::parse(&artifact) {
            Some(id) => Ok(self
                .session
                .lease_table()
                .dispatch_status(self.wallet_id(), id)),
            // A funding step id (`registration/<draft>/funding`): Mode B's
            // derived status (P2b); unknown until then.
            None if artifact.contains('/') => Ok(None),
            None => Err(PlatformError::InvalidArgument {
                detail: "not a txid, transition hash or step id".into(),
            }),
        }
    }
}
