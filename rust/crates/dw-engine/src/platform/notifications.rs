//! The bell: the `dp_events` journal read back (DP2-05).

use serde::{Deserialize, Serialize};

use super::contacts::ContactSummary;
use super::dashpay::{DashPay, stub};
use super::errors::PlatformError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPage {
    /// Incoming requests waiting for an answer.
    pub pending: Vec<ContactSummary>,
    /// Unread events, newest first.
    pub new: Vec<DashPayEvent>,
    /// Read events, newest first.
    pub earlier: Vec<DashPayEvent>,
    pub next_cursor: Option<u64>,
}

/// One `dp_events` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPayEvent {
    pub id: u64,
    pub kind: EventKind,
    pub contact: Option<String>,
    /// Per kind: the txid of `PaymentReceived`, the contact-request id of
    /// the request kinds, the label of the name kinds.
    pub reference: Option<String>,
    pub at: u64,
    pub read_at: Option<u64>,
}

/// The journal kinds of §3.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UsernameRegistered,
    ContestWon,
    ContestLost,
    ContestLocked,
    RequestReceived,
    RequestAccepted,
    ContactEstablished,
    PaymentReceived,
}

#[expect(unused_variables, reason = "stubs until DP2-05")]
impl DashPay {
    pub async fn events(
        &self,
        identity: String,
        cursor: Option<u64>,
        limit: u32,
    ) -> Result<EventPage, PlatformError> {
        stub("DashPay.events")
    }

    pub fn unread_count(&self, identity: String) -> Result<u32, PlatformError> {
        stub("DashPay.unread_count")
    }

    pub async fn mark_read(&self, identity: String, up_to: u64) -> Result<(), PlatformError> {
        stub("DashPay.mark_read")
    }
}
