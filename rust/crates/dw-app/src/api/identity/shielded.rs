//! Shielded funds (m4 §2.13): the wallet's account in Platform's shielded
//! pool. What leaves Core is in duffs (numbers); what the pool holds and
//! spends is in credits (decimal strings).

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;
use crate::dto::mirror_enum;
use crate::error::{AppError, EngineCode};

/// A credit amount the renderer sends as a decimal string.
pub(super) fn parse_credits(credits: &str) -> Result<u64, AppError> {
    credits.parse().map_err(|_| {
        AppError::new(
            EngineCode::InvalidArgument,
            "credits must be a decimal string",
        )
    })
}

/// Where the wallet's shielded account stands. When `active` is false the
/// network does not take shielded transitions yet ("Shielded is not active
/// on this network yet") and the other fields are empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedStatus {
    pub active: bool,
    pub protocol_version: u32,
    pub activation_version: u32,
    /// The ledger is established (keys bound, notes restored or scanned);
    /// until then `balance` is not a reading and spends are refused.
    pub ready: bool,
    /// Spendable credits.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub balance: u64,
    /// Credits of activity still pending, either direction.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub pending: u64,
    /// Core → Shielded locks not applied yet (`resumeShield`).
    pub pending_shields: Vec<PendingShield>,
    /// The note-tree position scanned to; `null` before a completed scan.
    pub synced_index: Option<u64>,
    /// Unix seconds of the last completed sync pass.
    pub last_sync_at: Option<u64>,
    pub syncing: bool,
    /// The prover is ready, so a spend proves at once.
    pub prover_ready: bool,
}

impl From<engine::ShieldedStatus> for ShieldedStatus {
    fn from(s: engine::ShieldedStatus) -> Self {
        let engine::ShieldedStatus {
            active,
            protocol_version,
            activation_version,
            ready,
            balance,
            pending,
            pending_shields,
            synced_index,
            last_sync_at,
            syncing,
            prover_ready,
        } = s;
        ShieldedStatus {
            active,
            protocol_version,
            activation_version,
            ready,
            balance,
            pending,
            pending_shields: pending_shields.into_iter().map(Into::into).collect(),
            synced_index,
            last_sync_at,
            syncing,
            prover_ready,
        }
    }
}

/// A shielded receive address (bech32m, `dash1z…` / `tdash1z…`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedAddress {
    pub address: String,
    pub account: u32,
}

impl From<engine::ShieldedAddress> for ShieldedAddress {
    fn from(a: engine::ShieldedAddress) -> Self {
        let engine::ShieldedAddress { address, account } = a;
        ShieldedAddress { address, account }
    }
}

/// What moving duffs of the Core balance into the pool costs and buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldQuote {
    pub duffs: u64,
    pub fee_duffs: u64,
    pub total_duffs: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub pool_fee_credits: u64,
    /// What the shielded balance gains.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub grant: GrantRequest,
}

impl From<engine::ShieldQuote> for ShieldQuote {
    fn from(q: engine::ShieldQuote) -> Self {
        let engine::ShieldQuote {
            duffs,
            fee_duffs,
            total_duffs,
            pool_fee_credits,
            credits,
            grant,
        } = q;
        ShieldQuote {
            duffs,
            fee_duffs,
            total_duffs,
            pool_fee_credits,
            credits,
            grant: grant.into(),
        }
    }
}

/// Where a Core → Shielded move stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShieldState {
    /// The credits are in the pool; `balance` is the shielded balance after.
    Applied {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        balance: u64,
    },
    /// The lock waits for its proof (`resumeShield`).
    ProofWaiting,
    /// Applying the proved lock failed with `error` (a code).
    NotApplied { error: String },
}

impl From<engine::ShieldState> for ShieldState {
    fn from(s: engine::ShieldState) -> Self {
        use engine::ShieldState as S;
        match s {
            S::Applied { balance } => ShieldState::Applied { balance },
            S::ProofWaiting => ShieldState::ProofWaiting,
            S::NotApplied { error } => ShieldState::NotApplied { error },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldOutcome {
    /// The asset lock's txid (hex).
    pub txid: String,
    pub state: ShieldState,
}

impl From<engine::ShieldOutcome> for ShieldOutcome {
    fn from(o: engine::ShieldOutcome) -> Self {
        let engine::ShieldOutcome { txid, state } = o;
        ShieldOutcome {
            txid,
            state: state.into(),
        }
    }
}

/// A Core → Shielded lock not applied yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingShield {
    pub txid: String,
    pub duffs: u64,
    pub proved: bool,
}

impl From<engine::PendingShield> for PendingShield {
    fn from(p: engine::PendingShield) -> Self {
        let engine::PendingShield {
            txid,
            duffs,
            proved,
        } = p;
        PendingShield {
            txid,
            duffs,
            proved,
        }
    }
}

/// What a shielded send or an unshield costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedSpendQuote {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub total_credits: u64,
    /// For an unshield, what reaches the Core address.
    pub expected_duffs: Option<u64>,
    pub grant: GrantRequest,
}

impl From<engine::ShieldedSpendQuote> for ShieldedSpendQuote {
    fn from(q: engine::ShieldedSpendQuote) -> Self {
        let engine::ShieldedSpendQuote {
            credits,
            fee_credits,
            total_credits,
            expected_duffs,
            grant,
        } = q;
        ShieldedSpendQuote {
            credits,
            fee_credits,
            total_credits,
            expected_duffs,
            grant: grant.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedSpendOutcome {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The shielded balance after.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub balance: u64,
    /// The address paid.
    pub to: String,
}

impl From<engine::ShieldedSpendOutcome> for ShieldedSpendOutcome {
    fn from(o: engine::ShieldedSpendOutcome) -> Self {
        let engine::ShieldedSpendOutcome {
            credits,
            balance,
            to,
        } = o;
        ShieldedSpendOutcome {
            credits,
            balance,
            to,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedActivityKind {
    Shield,
    ShieldFromCore,
    Received,
    Sent,
    Unshield,
    Withdrawal,
    IdentityCreate,
    IdentityTopUp,
    Spend,
}

mirror_enum!(engine::ShieldedActivityKind => ShieldedActivityKind {
    Shield, ShieldFromCore, Received, Sent, Unshield, Withdrawal, IdentityCreate, IdentityTopUp,
    Spend
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedDirection {
    In,
    Out,
    SelfTransfer,
}

mirror_enum!(engine::ShieldedDirection => ShieldedDirection { In, Out, SelfTransfer });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedActivityStatus {
    Pending,
    Confirmed,
    Failed,
    Unknown,
}

mirror_enum!(engine::ShieldedActivityStatus => ShieldedActivityStatus {
    Pending, Confirmed, Failed, Unknown
});

/// One row of the wallet's shielded history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedActivity {
    pub id: String,
    pub kind: ShieldedActivityKind,
    pub direction: ShieldedDirection,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub fee_credits: Option<u64>,
    pub counterparty: Option<String>,
    pub memo: Option<String>,
    pub block_height: Option<u64>,
    pub status: ShieldedActivityStatus,
    pub created_at: Option<u64>,
}

impl From<engine::ShieldedActivity> for ShieldedActivity {
    fn from(a: engine::ShieldedActivity) -> Self {
        let engine::ShieldedActivity {
            id,
            kind,
            direction,
            credits,
            fee_credits,
            counterparty,
            memo,
            block_height,
            status,
            created_at,
        } = a;
        ShieldedActivity {
            id,
            kind: kind.into(),
            direction: direction.into(),
            credits,
            fee_credits,
            counterparty,
            memo,
            block_height,
            status: status.into(),
            created_at,
        }
    }
}
