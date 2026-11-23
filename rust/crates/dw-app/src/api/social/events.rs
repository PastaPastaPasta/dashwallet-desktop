//! The bell: the DashPay event journal.

use dw_engine::platform::{DashPayEvent, EventKind, EventPage};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::contacts::ContactSummaryInfo;
use crate::dto::platform::MAX_ID_BYTES;
use crate::dto::{cap_text, mirror_enum};

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

impl DashPayEventInfo {
    /// With its texts cut to the bridge's id bound (`dashPayNews`).
    pub(crate) fn capped(self) -> Self {
        let cap = |t: Option<String>| t.map(|t| cap_text(t, MAX_ID_BYTES));
        Self {
            contact: cap(self.contact),
            reference: cap(self.reference),
            ..self
        }
    }
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

/// dw-engine writes the bell's contract vector by hand in this crate's shapes
/// (`notifications_tests.rs`, `vectors`; the renderer's fixture replays it):
/// its kind names, events, pages and errors must be this crate's.
#[cfg(test)]
mod vector_tests {
    use dw_engine::platform::{IdentityError, PlatformError};
    use serde_json::{Value, json};

    use super::*;
    use crate::error::AppError;

    fn vector() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/desktop/src/fixture/vectors/notifications.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// `v` through `T` and back: the same JSON when `v` has `T`'s shape.
    fn round_trip<T: Serialize + for<'de> Deserialize<'de>>(v: &Value) -> Value {
        serde_json::to_value(serde_json::from_value::<T>(v.clone()).unwrap()).unwrap()
    }

    #[test]
    fn notification_vector_is_dw_apps() {
        let doc = vector();
        let kinds = doc["kinds"].as_object().unwrap();
        assert_eq!(kinds.len(), 8);
        for (snake, camel) in kinds {
            let kind: EventKind = serde_json::from_value(json!(snake)).unwrap();
            assert_eq!(&serde_json::to_value(EventKindName::from(kind)).unwrap(), camel);
        }

        let engine: [AppError; 3] = [
            PlatformError::InvalidArgument { detail: "x".into() }.into(),
            PlatformError::Identity(IdentityError::NotFound).into(),
            PlatformError::WalletNotFound.into(),
        ];
        let converted: Vec<Value> = (engine.iter())
            .map(|e| {
                let v = serde_json::to_value(e).unwrap();
                json!({ "code": v["code"], "data": v["data"] })
            })
            .collect();

        let (mut pages, mut errors) = (0, 0);
        for step in (doc["cases"].as_array().unwrap().iter())
            .flat_map(|c| c["steps"].as_array().unwrap())
        {
            if let Some(e) = step["result"].get("error") {
                assert!(converted.contains(e), "{e} is not dw-app's conversion");
                errors += 1;
                continue;
            }
            let Some(page) = step["result"].get("ok").filter(|_| step["call"].get("events").is_some())
            else {
                continue;
            };
            // The fixture's clock stands in for "now"; pending holds labels.
            let page = serde_json::from_str::<Value>(&page.to_string().replace("\"now\"", "0"))
                .unwrap();
            let mut events_only = page.clone();
            events_only["pending"] = json!([]);
            assert_eq!(round_trip::<DashPayEventPage>(&events_only), events_only);
            pages += 1;
        }
        assert!(pages >= 15 && errors >= 5, "{pages} pages, {errors} errors");
        for e in &converted {
            assert!(doc.to_string().contains(&e.to_string()), "{e} is not recorded: drop it");
        }
    }
}
