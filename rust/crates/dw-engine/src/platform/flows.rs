//! Flows, grants and the dispatch journal as the host sees them (E0-04;
//! DASHPAY §2.3, §2.6). **Pending E0-04 rev1:** the names and shapes here
//! follow the E0-04 design r1 review (findings 2 and 4) and may change with
//! the revised design, under the contract's version rule.
//!
//! The lease itself (`Lease`, its table and signers) is E0-04's engine
//! internals; the facade only hands out its id.

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
    Discovery,
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
}

/// The caps of the `PlatformOp{max_duffs, max_credits}` grant an action
/// needs. The engine charges no more than it quoted.
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

/// What the dispatch journal knows about a handed-off artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    /// Committed; the engine will send it again (`platform.will_be_sent`).
    WillBeSent,
    /// It may already be out (`platform.broadcast_unknown`).
    MaybeSent,
    Sent,
    /// Definitely never sent; a retry is safe.
    NotSent,
}

/// The payload of the event sent when a provisional outcome resolves: a
/// later resend was accepted, or a reload refused and cleaned up the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchResolved {
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

#[expect(unused_variables, reason = "stubs until E0-04")]
impl NetworkSession {
    /// Redeems `grants` into one lease for `flow` and returns its id, which
    /// every call taking a `grant: String` accepts.
    pub async fn begin_flow(
        &self,
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

    /// `None` if the journal has no entry for `artifact`.
    pub async fn dispatch_status(
        &self,
        artifact: String,
    ) -> Result<Option<DispatchState>, PlatformError> {
        stub("DashPay.dispatch_status")
    }
}
