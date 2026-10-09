//! Contact payments: the per-contact lock, contact activity and the frequent
//! strip (DP3-01, DP3-02). The `TxDraft` recipient lives in `send/`.

use serde::{Deserialize, Serialize};

use super::contacts::ContactSummary;
use super::dashpay::{DashPay, stub};
use super::errors::ContactError;

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

#[expect(unused_variables, reason = "stubs until DP3-01 and DP3-02")]
impl DashPay {
    pub fn payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<PaymentLock>, ContactError> {
        stub("DashPay.payment_lock")
    }

    pub async fn resolve_payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<LockResolution, ContactError> {
        stub("DashPay.resolve_payment_lock")
    }

    pub async fn contact_activity(
        &self,
        identity: String,
        contact: String,
        cursor: Option<String>,
        f: ActivityFilter,
    ) -> Result<ActivityPage, ContactError> {
        stub("DashPay.contact_activity")
    }

    pub fn frequent_contacts(
        &self,
        identity: String,
        limit: u32,
    ) -> Result<Vec<ContactSummary>, ContactError> {
        stub("DashPay.frequent_contacts")
    }
}
