//! Contact payments: the per-contact lock and contact activity.

use dw_engine::platform::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, LockResolution, PaymentLock,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;

/// A per-contact send lock after an ambiguous broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PaymentLockInfo {
    pub txid: String,
    /// UNIX time the lock was set.
    pub since: u64,
}

impl From<PaymentLock> for PaymentLockInfo {
    fn from(l: PaymentLock) -> Self {
        let PaymentLock { txid, since } = l;
        PaymentLockInfo { txid, since }
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
    pub txid: String,
    pub direction: ActivityDirectionName,
    /// Duffs.
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
}

impl From<ActivityItem> for ContactActivityItem {
    fn from(i: ActivityItem) -> Self {
        let ActivityItem {
            txid,
            direction,
            amount,
            height,
            timestamp,
        } = i;
        ContactActivityItem {
            txid,
            direction: direction.into(),
            amount,
            height,
            timestamp,
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
