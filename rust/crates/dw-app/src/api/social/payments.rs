//! Contact payments: the per-contact lock and contact activity.

use dw_engine::platform::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, ContactPayOutcome,
    ContactPayQuote, LockResolution, PaymentLock, PaymentSource, PayoutState,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::identity::flows::GrantRequest;
use crate::dto::mirror_enum;

/// The balance a contact payment comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PaymentSourceName {
    Core,
    Shielded,
    Platform,
}

mirror_enum!(PaymentSource => PaymentSourceName { Core, Shielded, Platform });
mirror_enum!(PaymentSourceName => PaymentSource { Core, Shielded, Platform });

/// A per-contact send lock: a Core payment after an ambiguous broadcast, or
/// a payout from another balance that may have been sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PaymentLockInfo {
    /// Equals `reference`; a Core txid only when `source` is `core`.
    pub txid: String,
    /// UNIX time the lock was set.
    pub since: u64,
    pub source: PaymentSourceName,
    pub reference: String,
}

impl From<PaymentLock> for PaymentLockInfo {
    fn from(l: PaymentLock) -> Self {
        let PaymentLock {
            txid,
            since,
            source,
            reference,
        } = l;
        PaymentLockInfo {
            txid,
            since,
            source: source.into(),
            reference,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LockResolutionName {
    /// The payment was sent; the lock is cleared.
    Sent,
    /// Positive evidence that it was never sent; the lock is cleared.
    NotSent,
    /// Anything else; the lock stays.
    Unknown,
}

mirror_enum!(LockResolution => LockResolutionName { Sent, NotSent, Unknown });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ActivityFilterName {
    All,
    Sent,
    Received,
}

mirror_enum!(ActivityFilterName => ActivityFilter { All, Sent, Received });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ActivityDirectionName {
    Sent,
    Received,
}

mirror_enum!(ActivityDirection => ActivityDirectionName { Sent, Received });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactActivityItem {
    /// Equals `reference`; a Core txid only when `source` is `core`.
    pub txid: String,
    pub direction: ActivityDirectionName,
    /// Duffs.
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
    pub source: PaymentSourceName,
    pub reference: String,
    /// The Core transaction, once the wallet has seen it.
    pub core_txid: Option<String>,
}

impl From<ActivityItem> for ContactActivityItem {
    fn from(i: ActivityItem) -> Self {
        let ActivityItem {
            txid,
            direction,
            amount,
            height,
            timestamp,
            source,
            reference,
            core_txid,
        } = i;
        ContactActivityItem {
            txid,
            direction: direction.into(),
            amount,
            height,
            timestamp,
            source: source.into(),
            reference,
            core_txid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactActivityPage {
    pub items: Vec<ContactActivityItem>,
    /// Pass it back as `cursor` for the next page; `null` at the end.
    pub next_cursor: Option<String>,
}

impl From<ActivityPage> for ContactActivityPage {
    fn from(p: ActivityPage) -> Self {
        let ActivityPage { items, next_cursor } = p;
        ContactActivityPage {
            items: items.into_iter().map(Into::into).collect(),
            next_cursor,
        }
    }
}

/// What a payment to a contact from the Shielded or Platform balance takes
/// and pays (C-funding P13): the typed amount leaves the balance, the
/// contact receives about `expectedDuffs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactPayQuoteInfo {
    pub source: PaymentSourceName,
    /// What leaves the balance: the amount typed.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// What the contact receives on Core, about.
    pub expected_duffs: u64,
    /// The network fee estimate.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    /// What to authorize: the amount and the fee the payment may take.
    pub grant: GrantRequest,
}

impl From<ContactPayQuote> for ContactPayQuoteInfo {
    fn from(q: ContactPayQuote) -> Self {
        let ContactPayQuote {
            source,
            credits,
            expected_duffs,
            fee_credits,
            grant,
        } = q;
        ContactPayQuoteInfo {
            source: source.into(),
            credits,
            expected_duffs,
            fee_credits,
            grant: grant.into(),
        }
    }
}

/// The terms of `contactPay`: what leaves which balance, and the note kept
/// in this wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactPayArgs {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub source: PaymentSourceName,
    pub note: Option<String>,
    /// Spend from a shielded account whose last scan is not on record.
    pub confirm_unrecorded: bool,
}

/// Where a payment from the Shielded or Platform balance stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PayoutStateName {
    /// It may have been sent: the contact stays locked.
    Pending,
    Sent,
    /// Proved never to execute.
    NotSent,
}

mirror_enum!(PayoutState => PayoutStateName { Pending, Sent, NotSent });

/// `contactPay`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactPayOutcomeInfo {
    /// The payment's reference: the lock's and the activity row's.
    pub reference: String,
    pub state: PayoutStateName,
}

impl From<ContactPayOutcome> for ContactPayOutcomeInfo {
    fn from(o: ContactPayOutcome) -> Self {
        let ContactPayOutcome { reference, state } = o;
        ContactPayOutcomeInfo {
            reference,
            state: state.into(),
        }
    }
}
