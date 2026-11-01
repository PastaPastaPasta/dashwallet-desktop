//! Platform addresses: the balance, its activity and "Move to Dash wallet"
//! (m4 §2.12). Credits as decimal strings, duffs as numbers.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;
use crate::dto::mirror_enum;

/// The wallet's Platform addresses as the last sync pass left them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformAddressBalance {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// What a "Move to Dash wallet" of everything would withdraw now (0 when
    /// it would be refused); a remainder too small to move is in `credits`
    /// only.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub movable_credits: u64,
    /// `movableCredits` in whole duffs: what Home adds to the spendable
    /// total.
    pub duffs: u64,
    /// The funded addresses, the largest first.
    pub addresses: Vec<PlatformAddressRow>,
    /// Unix seconds of this session's last sync pass; `null` before one.
    pub synced_at: Option<u64>,
    pub syncing: bool,
}

impl From<engine::PlatformAddressBalance> for PlatformAddressBalance {
    fn from(b: engine::PlatformAddressBalance) -> Self {
        let engine::PlatformAddressBalance {
            credits,
            movable_credits,
            duffs,
            addresses,
            synced_at,
            syncing,
        } = b;
        PlatformAddressBalance {
            credits,
            movable_credits,
            duffs,
            addresses: addresses.into_iter().map(Into::into).collect(),
            synced_at,
            syncing,
        }
    }
}

/// One funded Platform address (bech32m).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformAddressRow {
    pub address: String,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
}

impl From<engine::PlatformAddressRow> for PlatformAddressRow {
    fn from(r: engine::PlatformAddressRow) -> Self {
        let engine::PlatformAddressRow { address, credits } = r;
        PlatformAddressRow { address, credits }
    }
}

/// `incoming` and `outgoing`: a balance change a sync pass saw;
/// `moveToCore`: a "Move to Dash wallet" of this wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlatformActivityKind {
    Incoming,
    Outgoing,
    MoveToCore,
}

mirror_enum!(engine::PlatformActivityKind => PlatformActivityKind {
    Incoming, Outgoing, MoveToCore
});

/// `pending`: a move whose outcome is not known yet (never retried).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlatformActivityState {
    Done,
    Pending,
    Failed,
}

mirror_enum!(engine::PlatformActivityState => PlatformActivityState {
    Done, Pending, Failed
});

/// One Platform-address activity row, for Activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformActivity {
    pub id: String,
    pub kind: PlatformActivityKind,
    pub state: PlatformActivityState,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub address: Option<String>,
    pub to: Option<String>,
    pub expected_duffs: Option<u64>,
    /// A done move's fee as its inputs' proved balances show it.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub fee_credits: Option<u64>,
    /// Unix seconds.
    pub at: u64,
}

impl From<engine::PlatformActivity> for PlatformActivity {
    fn from(a: engine::PlatformActivity) -> Self {
        let engine::PlatformActivity {
            id,
            kind,
            state,
            credits,
            address,
            to,
            expected_duffs,
            fee_credits,
            at,
        } = a;
        PlatformActivity {
            id,
            kind: kind.into(),
            state: state.into(),
            credits,
            address,
            to,
            expected_duffs,
            fee_credits,
            at,
        }
    }
}

/// What "Move to Dash wallet" takes and pays out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MoveToCoreQuote {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    pub expected_duffs: u64,
    /// The wallet's own Core receive address: `platformMoveToCore` pays it.
    pub to: String,
    pub grant: GrantRequest,
}

impl From<engine::MoveToCoreQuote> for MoveToCoreQuote {
    fn from(q: engine::MoveToCoreQuote) -> Self {
        let engine::MoveToCoreQuote {
            credits,
            fee_credits,
            expected_duffs,
            to,
            grant,
        } = q;
        MoveToCoreQuote {
            credits,
            fee_credits,
            expected_duffs,
            to,
            grant: grant.into(),
        }
    }
}

/// A move's outcome: `done` once a proved read shows its inputs' signed
/// nonces, else `pending`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MoveToCoreOutcome {
    pub id: String,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub expected_duffs: u64,
    pub to: String,
    pub state: PlatformActivityState,
    /// Done: the fee taken.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub fee_credits: Option<u64>,
}

impl From<engine::MoveToCoreOutcome> for MoveToCoreOutcome {
    fn from(o: engine::MoveToCoreOutcome) -> Self {
        let engine::MoveToCoreOutcome {
            id,
            credits,
            expected_duffs,
            to,
            state,
            fee_credits,
        } = o;
        MoveToCoreOutcome {
            id,
            credits,
            expected_duffs,
            to,
            state: state.into(),
            fee_credits,
        }
    }
}
