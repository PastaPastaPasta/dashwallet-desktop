//! The bell: the DashPay event journal.

use dw_engine::platform::{DashPayEvent, EventKind, EventPage};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::contacts::ContactSummaryInfo;
use crate::dto::mirror_enum;

/// The journal kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EventKindName {
    UsernameRegistered,
    ContestWon,
    ContestLost,
    ContestLocked,
    RequestReceived,
    RequestAccepted,
    ContactEstablished,
    PaymentReceived,
}

mirror_enum!(EventKind => EventKindName {
    UsernameRegistered, ContestWon, ContestLost, ContestLocked, RequestReceived,
    RequestAccepted, ContactEstablished, PaymentReceived
});

/// One journal row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayEventInfo {
    /// Pass the newest id read to `dashpayMarkRead`.
    pub id: u64,
    pub kind: EventKindName,
    pub contact: Option<String>,
    /// Per kind: the txid of `paymentReceived`, the contact-request id of
    /// the request kinds, the label of the name kinds.
    pub reference: Option<String>,
    pub at: u64,
    pub read_at: Option<u64>,
}

impl From<DashPayEvent> for DashPayEventInfo {
    fn from(e: DashPayEvent) -> Self {
        let DashPayEvent {
            id,
            kind,
            contact,
            reference,
            at,
            read_at,
        } = e;
        DashPayEventInfo {
            id,
            kind: kind.into(),
            contact,
            reference,
            at,
            read_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayEventPage {
    /// Incoming requests waiting for an answer.
    pub pending: Vec<ContactSummaryInfo>,
    /// Unread events, newest first.
    pub new: Vec<DashPayEventInfo>,
    /// Read events, newest first.
    pub earlier: Vec<DashPayEventInfo>,
    pub next_cursor: Option<u64>,
}

impl From<EventPage> for DashPayEventPage {
    fn from(p: EventPage) -> Self {
        let EventPage {
            pending,
            new,
            earlier,
            next_cursor,
        } = p;
        DashPayEventPage {
            pending: pending.into_iter().map(Into::into).collect(),
            new: new.into_iter().map(Into::into).collect(),
            earlier: earlier.into_iter().map(Into::into).collect(),
            next_cursor,
        }
    }
}
