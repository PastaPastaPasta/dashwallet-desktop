//! Contact payments: the per-contact lock, contact activity and the frequent
//! strip (DP3-01, DP3-02). The `TxDraft` recipient lives in `send/`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::contacts::ContactSummary;
use super::dashpay::DashPay;
use super::errors::{ContactError, PlatformError};

/// A per-contact send lock after an ambiguous broadcast (`dp_payment_lock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentLock {
    pub txid: String,
    pub since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockResolution {
    /// The payment was sent: an attempt or a Resend finished `Sent`, or the
    /// wallet has seen it. The lock is cleared.
    Sent,
    /// Positive evidence that it was never sent: in this process its
    /// `dispatch_status` is `Some(NotSent)`, or a ChainLocked spend conflicts
    /// with one of its inputs. Never "not found". The lock is cleared.
    NotSent,
    /// Anything else, read like `MaybeSent`; the lock stays.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityFilter {
    All,
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub txid: String,
    pub direction: ActivityDirection,
    /// Duffs.
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDirection {
    Sent,
    Received,
}

/// The other side of a contact payment; history and transaction records
/// gain it as `counterparty` in DP3-02.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterparty {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl DashPay {
    /// The lock an unknown-outcome payment to `contact` set, if any; from
    /// memory (DP3-01, `payment_locks.rs`). Re-query on `Platform{Payments}`.
    pub fn payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<PaymentLock>, ContactError> {
        (self.session).payment_lock_of(self.wallet_id(), &identity, &contact)
    }

    /// Settles the pair's lock on positive evidence only (m4 §2.5, item
    /// 34): `Sent` or `NotSent` clears it, `Unknown` keeps it. A pair with
    /// no lock is `invalid_argument`.
    pub async fn resolve_payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<LockResolution, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(
                async move { Ok(session.resolve_payment_lock_of(id, identity, contact).await) },
            )
            .await
            .map_err(PlatformError::from)?
    }

    /// `identity`'s payments to and from `contact`, newest first (mined by
    /// block time, unmined first), 50 per page; `cursor` is the previous
    /// page's `next_cursor`. Attributed as history attributes them
    /// (`attribution.rs`); abandoned and refused transactions are left out.
    /// Re-query on `Platform{Payments}`, `Platform{Contacts}` and the
    /// wallet's `HistoryChanged` (DP3-02).
    pub async fn contact_activity(
        &self,
        identity: String,
        contact: String,
        cursor: Option<String>,
        f: ActivityFilter,
    ) -> Result<ActivityPage, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move {
                Ok(session
                    .contact_activity_of(id, identity, contact, cursor, f)
                    .await)
            })
            .await
            .map_err(PlatformError::from)?
    }

    /// The Pay screen's frequent strip (Android's top contacts): at most
    /// `limit` (clamped to 1–20) established contacts that are not hidden,
    /// the ones `identity` paid most often in the last 90 days first, then
    /// the ones that paid it most often; in memory, from the contacts read
    /// model and the history (DP3-02).
    pub fn frequent_contacts(
        &self,
        identity: String,
        limit: u32,
    ) -> Result<Vec<ContactSummary>, ContactError> {
        self.session.frequent_of(self.wallet_id(), identity, limit)
    }
}
