//! Platform addresses: the balance, its activity, "Move to Dash wallet",
//! the receive address, a send to a Platform address and "Add from Dash
//! wallet" (m4 §2.12).
//! Credits as decimal strings, duffs as numbers.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;
use super::unfinished::UnfinishedState;
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
/// `moveToCore`: a "Move to Dash wallet" of this wallet; `send`: a send of
/// this wallet to a Platform address (`to`); `fromCore`: an "Add from Dash
/// wallet" of this wallet to its Platform address (`address`), pending
/// until Platform proves it applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlatformActivityKind {
    Incoming,
    Outgoing,
    MoveToCore,
    Send,
    FromCore,
}

mirror_enum!(engine::PlatformActivityKind => PlatformActivityKind {
    Incoming, Outgoing, MoveToCore, Send, FromCore
});

/// `pending`: a move whose outcome is not known yet (never retried);
/// `replaced`: another move at the same inputs and nonces carries the
/// outcome (its row is `done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlatformActivityState {
    Done,
    Pending,
    Failed,
    Replaced,
}

mirror_enum!(engine::PlatformActivityState => PlatformActivityState {
    Done, Pending, Failed, Replaced
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
    /// A move or send that may still execute: a `failed` one "may still go
    /// through"; a `failed` one without it never does.
    pub may_land: bool,
    /// A done send whose debit counts the fee estimate.
    pub estimated: bool,
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
            may_land,
            estimated,
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
            may_land,
            estimated,
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
/// nonces, else `pending`; `replaced` when another move of its group
/// carries the outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MoveToCoreOutcome {
    pub id: String,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// `null` when a group's outcome cannot attribute the payout.
    pub expected_duffs: Option<u64>,
    /// `null` when a group's outcome cannot attribute the destination.
    pub to: Option<String>,
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

/// The wallet's Platform receive address (bech32m), shown and put in a QR
/// code as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformReceiveAddress {
    pub address: String,
    /// Its index in the payment account.
    pub index: u32,
}

impl From<engine::PlatformReceiveAddress> for PlatformReceiveAddress {
    fn from(a: engine::PlatformReceiveAddress) -> Self {
        let engine::PlatformReceiveAddress { address, index } = a;
        PlatformReceiveAddress { address, index }
    }
}

/// What a send to a Platform address takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSendQuote {
    /// The planned debit, the grant's cap: the amount and, unless the
    /// amount pays the fee, `feeSourceCredits`. Not a ceiling Platform
    /// enforces.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The transfer's output: what the recipient receives, less the fee
    /// when `feeFromAmount`.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub amount_credits: u64,
    /// The fee estimate.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    /// What `feeSource` keeps after the send, as the quote read it: the
    /// fee Platform meters may take all of it, and a later deposit to that
    /// address too; `null` for Max, where it comes out of the amount.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub fee_source_credits: Option<u64>,
    /// The address Platform takes the fee from; `null` for Max.
    pub fee_source: Option<String>,
    /// Max: the amount pays the fee.
    pub fee_from_amount: bool,
    /// The recipient, normalized.
    pub to: String,
    pub grant: GrantRequest,
    /// Pass it to `platformSend`: it pays this quote's recipient and amount,
    /// once.
    pub operation: String,
    /// The held send this quote retries on its stored terms; its send pays
    /// nothing more if that one landed meanwhile.
    pub retry_of: Option<String>,
}

impl From<engine::PlatformSendQuote> for PlatformSendQuote {
    fn from(q: engine::PlatformSendQuote) -> Self {
        let engine::PlatformSendQuote {
            credits,
            amount_credits,
            fee_credits,
            fee_source_credits,
            fee_source,
            fee_from_amount,
            to,
            grant,
            operation,
            retry_of,
        } = q;
        PlatformSendQuote {
            credits,
            amount_credits,
            fee_credits,
            fee_source_credits,
            fee_source,
            fee_from_amount,
            to,
            grant: grant.into(),
            operation,
            retry_of,
        }
    }
}

/// A send's outcome: as a move's; `mayLand` while it may still execute,
/// once. A retry of a send that landed answers that send's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSendOutcome {
    pub id: String,
    /// Its amount until it is done; then what it took as the balances show
    /// it (a deposit to the fee source while it settled hides part of the
    /// fee, so this is "about").
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The amount it sends (for Max, before the fee).
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub amount_credits: u64,
    pub fee_from_amount: bool,
    pub to: Option<String>,
    pub state: PlatformActivityState,
    pub may_land: bool,
    /// Done: the fee taken from the fee source as the balances show it;
    /// `null` when the amount paid it or the balances hide it.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub fee_credits: Option<u64>,
    /// Done: `credits` counts the fee estimate.
    pub estimated: bool,
}

impl From<engine::PlatformSendOutcome> for PlatformSendOutcome {
    fn from(o: engine::PlatformSendOutcome) -> Self {
        let engine::PlatformSendOutcome {
            id,
            credits,
            amount_credits,
            fee_from_amount,
            to,
            state,
            may_land,
            fee_credits,
            estimated,
        } = o;
        PlatformSendOutcome {
            id,
            credits,
            amount_credits,
            fee_from_amount,
            to,
            state: state.into(),
            may_land,
            fee_credits,
            estimated,
        }
    }
}

/// What "Add from Dash wallet" of `duffs` costs and adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformFundQuote {
    /// The asset lock's value.
    pub duffs: u64,
    /// The lock transaction's fee bound.
    pub fee_duffs: u64,
    /// What leaves the Core balance at most.
    pub total_duffs: u64,
    /// Platform's fee estimate, out of the lock's credits.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    /// About what the Platform address gains.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The wallet's Platform receive address the fund pays (bech32m).
    pub to: String,
    pub grant: GrantRequest,
}

impl From<engine::PlatformFundQuote> for PlatformFundQuote {
    fn from(q: engine::PlatformFundQuote) -> Self {
        let engine::PlatformFundQuote {
            duffs,
            fee_duffs,
            total_duffs,
            fee_credits,
            credits,
            to,
            grant,
        } = q;
        PlatformFundQuote {
            duffs,
            fee_duffs,
            total_duffs,
            fee_credits,
            credits,
            to,
            grant: grant.into(),
        }
    }
}

/// Where an "Add from Dash wallet" stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlatformFundState {
    /// `credits` are on the Platform address.
    Applied {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        credits: u64,
    },
    /// The lock waits for its proof (`resumePlatformFund`).
    ProofWaiting,
    /// The apply was refused before its transition was signed, with `error`
    /// (a code): nothing reached Platform. `resumePlatformFund` tries again.
    NotApplied { error: String },
    /// The apply may have executed, unproved: it stays pending
    /// (`consumptionUnknown`, "Checking") until a proved apply; it blocks
    /// no new fund.
    ConsumptionUnknown,
}

impl From<engine::PlatformFundState> for PlatformFundState {
    fn from(s: engine::PlatformFundState) -> Self {
        use engine::PlatformFundState as S;
        match s {
            S::Applied { credits } => PlatformFundState::Applied { credits },
            S::ProofWaiting => PlatformFundState::ProofWaiting,
            S::NotApplied { error } => PlatformFundState::NotApplied { error },
            S::ConsumptionUnknown => PlatformFundState::ConsumptionUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformFundOutcome {
    /// The asset lock's txid (hex).
    pub txid: String,
    pub state: PlatformFundState,
}

impl From<engine::PlatformFundOutcome> for PlatformFundOutcome {
    fn from(o: engine::PlatformFundOutcome) -> Self {
        let engine::PlatformFundOutcome { txid, state } = o;
        PlatformFundOutcome {
            txid,
            state: state.into(),
        }
    }
}

/// An "Add from Dash wallet" lock of this install not applied yet
/// ("Pending — finish").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingPlatformFund {
    pub txid: String,
    pub duffs: u64,
    pub proved: bool,
    /// The lock may already be applied ("Checking"); only a proved apply
    /// clears it. It blocks no new fund.
    pub consumption_unknown: bool,
    /// The recovery's state of the lock (§2.14): `tooSmall` and
    /// `unsupported` can't be finished in this version, so the host offers
    /// no Finish, and neither blocks a new fund.
    pub state: UnfinishedState,
}

impl From<engine::PendingPlatformFund> for PendingPlatformFund {
    fn from(p: engine::PendingPlatformFund) -> Self {
        let engine::PendingPlatformFund {
            txid,
            duffs,
            proved,
            consumption_unknown,
            state,
        } = p;
        PendingPlatformFund {
            txid,
            duffs,
            proved,
            consumption_unknown,
            state: state.into(),
        }
    }
}
