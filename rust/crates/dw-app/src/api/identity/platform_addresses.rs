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
    /// The network has address funds transitions: below protocol version 11
    /// a top-up or send from these addresses is refused.
    pub sends_active: bool,
    /// `"platform_address_sends"`: this build keeps sends and moves from
    /// these addresses off on this network (mainnet), whatever its
    /// protocol version (`platform.feature_off` with that feature), and
    /// `sendsActive` is `false`. `null` elsewhere.
    pub sends_off: Option<String>,
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
            sends_active,
            sends_off,
        } = b;
        PlatformAddressBalance {
            credits,
            movable_credits,
            duffs,
            addresses: addresses.into_iter().map(Into::into).collect(),
            synced_at,
            syncing,
            sends_active,
            sends_off,
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

impl From<PlatformAddressRow> for engine::PlatformAddressRow {
    fn from(r: PlatformAddressRow) -> Self {
        let PlatformAddressRow { address, credits } = r;
        engine::PlatformAddressRow { address, credits }
    }
}

/// `incoming` and `outgoing`: a balance change a sync pass saw;
/// `moveToCore`: a "Move to Dash wallet" of this wallet; `send`: a send of
/// this wallet to a Platform address (`to`); `fromCore`: an "Add from Dash
/// wallet" of this wallet to its Platform address (`address`), pending
/// until Platform proves it applied; `toIdentity`: a top-up of one of this
/// wallet's identities (`to`, base58) from its Platform addresses;
/// `contactPayment`: a payment to a DashPay contact (`contact`) from the
/// Platform balance, to its Core address (`to`);
/// `fromShielded`: an unshield of this wallet to its Platform address
/// (`address`), pending until Platform executed it (`done`) or did not
/// (`failed`);
/// `recovered`: the value left on a transfer to Shielded that Platform
/// refused after a penalty, recovered to this wallet's Platform address
/// (`address`), pending until Platform proves it applied;
/// `registrationRefund`: what Platform returned to this wallet's Platform
/// address (`address`) when it could not create the identity a username
/// paid from the Shielded balance (the payment less a penalty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlatformActivityKind {
    Incoming,
    Outgoing,
    MoveToCore,
    Send,
    FromCore,
    ToIdentity,
    ContactPayment,
    FromShielded,
    Recovered,
    RegistrationRefund,
}

mirror_enum!(engine::PlatformActivityKind => PlatformActivityKind {
    Incoming, Outgoing, MoveToCore, Send, FromCore, ToIdentity, ContactPayment, FromShielded, Recovered,
    RegistrationRefund
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
    /// A `contactPayment`'s contact; absent for every other row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub contact: Option<crate::dto::history::TxCounterparty>,
    /// `false` on every row of a group whose proof does not say which
    /// member went through (the newest `done` included); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub winner_known: Option<bool>,
    /// A `send` that cancels the held move or contact payment of this id:
    /// to one of the wallet's own Platform addresses, it pays no one;
    /// absent on every other row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cancels: Option<String>,
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
            contact,
            unattributed,
            cancels,
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
            contact: contact.map(Into::into),
            winner_known: unattributed.then_some(false),
            cancels,
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
    /// The held top-up or send from Platform this move is the exit of:
    /// pass it to `platformMoveToCore`.
    pub exit_of: Option<String>,
}

/// What cancelling a held move or contact payment takes: its inputs, at its
/// nonces and amounts, to one of the wallet's own Platform addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCancelQuote {
    /// The held move or contact payment it cancels.
    pub of: String,
    /// The held record's amount; the fee comes out of it.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    /// The wallet's own Platform address: `platformCancel` pays it.
    pub to: String,
    pub grant: GrantRequest,
}

impl From<engine::PlatformCancelQuote> for PlatformCancelQuote {
    fn from(q: engine::PlatformCancelQuote) -> Self {
        let engine::PlatformCancelQuote {
            of,
            credits,
            fee_credits,
            to,
            grant,
        } = q;
        PlatformCancelQuote {
            of,
            credits,
            fee_credits,
            to,
            grant: grant.into(),
        }
    }
}

impl From<engine::MoveToCoreQuote> for MoveToCoreQuote {
    fn from(q: engine::MoveToCoreQuote) -> Self {
        let engine::MoveToCoreQuote {
            credits,
            fee_credits,
            expected_duffs,
            to,
            grant,
            exit_of,
        } = q;
        MoveToCoreQuote {
            credits,
            fee_credits,
            expected_duffs,
            to,
            grant: grant.into(),
            exit_of,
        }
    }
}

/// What moving credits from the Platform addresses into Shielded takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformShieldQuote {
    /// What the shielded balance gains.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The pool's flat fee, claimed on the first input.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub pool_fee_credits: u64,
    /// What the first input keeps for the fee Platform meters there
    /// ("about").
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_reserve_credits: u64,
    /// The planned debit and the grant's cap.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub debit_credits: u64,
    /// The addresses taken from with their claims; the first pays the fee.
    pub inputs: Vec<PlatformAddressRow>,
    pub grant: GrantRequest,
}

impl From<engine::PlatformShieldQuote> for PlatformShieldQuote {
    fn from(q: engine::PlatformShieldQuote) -> Self {
        let engine::PlatformShieldQuote {
            credits,
            pool_fee_credits,
            fee_reserve_credits,
            debit_credits,
            inputs,
            grant,
        } = q;
        PlatformShieldQuote {
            credits,
            pool_fee_credits,
            fee_reserve_credits,
            debit_credits,
            inputs: inputs.into_iter().map(Into::into).collect(),
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
    /// `false` when its group settled with no evidence of which member
    /// went through; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub winner_known: Option<bool>,
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
            unattributed,
        } = o;
        MoveToCoreOutcome {
            id,
            credits,
            expected_duffs,
            to,
            state: state.into(),
            fee_credits,
            winner_known: unattributed.then_some(false),
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
    /// `false` when its group settled with no evidence of which member
    /// went through; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub winner_known: Option<bool>,
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
            unattributed,
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
            winner_known: unattributed.then_some(false),
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
    /// A Max's (`all`): `totalDuffs` is the coins' sum, `duffs` the lock.
    pub max: Option<super::shielded::CoreMax>,
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
            max,
        } = q;
        PlatformFundQuote {
            duffs,
            fee_duffs,
            total_duffs,
            fee_credits,
            credits,
            to,
            grant: grant.into(),
            max: max.map(Into::into),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(unattributed: bool) -> serde_json::Value {
        serde_json::to_value(PlatformActivity::from(engine::PlatformActivity {
            id: "move-1-0".into(),
            kind: engine::PlatformActivityKind::MoveToCore,
            state: engine::PlatformActivityState::Replaced,
            credits: 1,
            address: None,
            to: None,
            expected_duffs: None,
            fee_credits: None,
            at: 1,
            may_land: false,
            estimated: false,
            contact: None,
            unattributed,
            cancels: None,
        }))
        .unwrap()
    }

    #[test]
    fn a_row_whose_group_names_no_winner_crosses_winner_known_false() {
        assert_eq!(row(true)["winnerKnown"], serde_json::json!(false));
        assert!(row(false).get("winnerKnown").is_none());
    }

    #[test]
    fn an_outcome_whose_group_names_no_winner_crosses_winner_known_false() {
        let moved = |unattributed| {
            serde_json::to_value(MoveToCoreOutcome::from(engine::MoveToCoreOutcome {
                id: "move-1-0".into(),
                credits: 1,
                expected_duffs: None,
                to: None,
                state: engine::PlatformActivityState::Done,
                fee_credits: None,
                unattributed,
            }))
            .unwrap()
        };
        let sent = |unattributed| {
            serde_json::to_value(PlatformSendOutcome::from(engine::PlatformSendOutcome {
                id: "move-1-0".into(),
                credits: 1,
                amount_credits: 1,
                fee_from_amount: false,
                to: None,
                state: engine::PlatformActivityState::Done,
                may_land: false,
                fee_credits: None,
                estimated: false,
                unattributed,
            }))
            .unwrap()
        };
        assert_eq!(moved(true)["winnerKnown"], serde_json::json!(false));
        assert_eq!(sent(true)["winnerKnown"], serde_json::json!(false));
        assert!(moved(false).get("winnerKnown").is_none());
        assert!(sent(false).get("winnerKnown").is_none());
    }
}
