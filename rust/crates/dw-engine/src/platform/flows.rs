//! Flows, grants and the dispatch journal as the host sees them (E0-04;
//! DASHPAY §2.3, §2.6). The shapes follow E0-04 design rev2 §16, the single
//! source for this surface; a later E0-04 revision changes them under the
//! contract's version rule.
//!
//! The lease itself (`Lease`, its table and signers) is E0-04's engine
//! internals; the facade hands out its id and a view of it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::PlatformError;
use crate::{NetworkSession, WalletId};

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

/// What the engine knows about a handed-off artifact. `dispatch_status`
/// returns `None` when it knows nothing (E0-04's `Unknown`), which the host
/// treats exactly like `MaybeSent`.
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
    /// allows a retry, a discard or a second funding.
    NotSent,
}

/// The payload of the event sent when a provisional outcome resolves: a
/// later resend was accepted, or a reload refused and cleaned up the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchResolved {
    /// Lower-case hex, as `WalletId` displays.
    pub wallet_id: String,
    /// The txid of a Core transaction or the hash of a state transition.
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

#[expect(unused_variables, reason = "stubs until E0-04")]
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
        stub("NetworkSession.begin_flow")
    }

    /// Releases a lease. Idempotent.
    pub fn end_flow(&self, lease: String) -> Result<(), PlatformError> {
        stub("NetworkSession.end_flow")
    }

    pub fn leases(&self) -> Result<Vec<LeaseView>, PlatformError> {
        stub("NetworkSession.leases")
    }
}

#[expect(unused_variables, reason = "stubs until E0-04")]
impl DashPay {
    pub async fn grant_request(
        &self,
        identity: String,
        action: GrantAction,
    ) -> Result<GrantRequest, PlatformError> {
        stub("DashPay.grant_request")
    }

    /// `None`: the engine knows nothing (E0-04's `Unknown`); never a reason
    /// to retry.
    pub async fn dispatch_status(
        &self,
        artifact: String,
    ) -> Result<Option<DispatchState>, PlatformError> {
        stub("DashPay.dispatch_status")
    }
}
