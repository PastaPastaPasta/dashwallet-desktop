//! Shielded funds (m4 §2.13): the wallet's account in Platform's shielded
//! pool. What leaves Core is in duffs (numbers); what the pool holds and
//! spends is in credits (decimal strings).

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;
use crate::dto::mirror_enum;
use crate::dto::platform::{DispatchStateName, PendingDispatch};
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
    /// Core → Shielded locks not applied yet (`resumeShield`); empty until
    /// the dispatch journal is read.
    pub pending_shields: Vec<PendingShield>,
    /// Spends whose result is not confirmed yet (`pending`), until a sync
    /// pass resolves each. Offer no retry.
    pub unresolved_spends: Vec<UnresolvedSpend>,
    /// Spends that may still go through, whose notes stay held (out of
    /// `balance`) until the network shows they can no longer go through.
    /// Offer no retry.
    pub held_spends: Vec<HeldSpend>,
    /// Credits the `heldSpends` hold; "0" when nothing is held.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub held: u64,
    /// Spends from before a restart whose result is not known and whose
    /// record was lost: one may still go through, so every send and
    /// unshield needs `confirmUnrecorded` until a sync shows all its notes
    /// spent (`shielded.unrecorded_spend`).
    pub unrecorded_spends: Vec<UnrecordedSpend>,
    /// The note-tree position scanned to; `null` before a completed scan.
    pub synced_index: Option<u64>,
    /// When the pool was last scanned, or last proved unchanged with nothing
    /// to scan. Unix seconds, this session.
    pub last_sync_at: Option<u64>,
    pub syncing: bool,
    /// The prover is ready, so a spend proves at once.
    pub prover_ready: bool,
    /// This build can send, unshield and pay a contact from Shielded. When
    /// false those calls, and a new shield into the pool, refuse
    /// `platform.unavailable`; finishing a shield already sent, receive,
    /// sync and balance still work.
    pub spends_available: bool,
    /// Whether the viewing key is bound, or what holds its first bind;
    /// `null` while the network is not active. Once bound the receive
    /// address reads before any scan.
    pub account: Option<ShieldedAccount>,
    /// The running pass's progress through the pool's note tree
    /// (network-wide); `null` while no pass runs or before it knows the
    /// pool's size.
    pub scan: Option<ShieldedScanProgress>,
    /// The last sync pass failed; cleared by the next one that succeeds.
    pub last_pass_failed: bool,
}

/// Where the shielded account's viewing key stands. `locked` and
/// `mixingOnly` clear with a full unlock; `watchOnly` does not clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedAccount {
    Bound,
    Binding,
    Locked,
    MixingOnly,
    WatchOnly,
}

mirror_enum!(engine::ShieldedAccount => ShieldedAccount {
    Bound, Binding, Locked, MixingOnly, WatchOnly
});

/// Tree positions a pass scanned of the pool's notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedScanProgress {
    pub checked: u64,
    pub total: u64,
}

impl From<engine::ShieldedScanProgress> for ShieldedScanProgress {
    fn from(p: engine::ShieldedScanProgress) -> Self {
        let engine::ShieldedScanProgress { checked, total } = p;
        ShieldedScanProgress { checked, total }
    }
}

/// Where the wallet's Shielded sync stands, for the status bar, Sync
/// progress and Receive; read with no read of the note store, so it answers
/// while a pass runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldedSyncInfo {
    /// The network takes shielded transitions and the session runs Platform.
    pub active: bool,
    /// `binding` for a key a status read binds next; `null` while not active.
    pub account: Option<ShieldedAccount>,
    /// A pass scanned the account this session, or a status read found its
    /// ledger ready.
    pub established: bool,
    /// A pass is running.
    pub syncing: bool,
    pub scan: Option<ShieldedScanProgress>,
    /// The wallet's last pass this session failed, until one succeeds.
    pub last_pass_failed: bool,
    /// As `ShieldedStatus.spendsAvailable`, and read on an inactive network
    /// too.
    pub spends_available: bool,
}

impl From<engine::ShieldedSyncInfo> for ShieldedSyncInfo {
    fn from(s: engine::ShieldedSyncInfo) -> Self {
        let engine::ShieldedSyncInfo {
            active,
            account,
            established,
            syncing,
            scan,
            last_pass_failed,
            spends_available,
        } = s;
        ShieldedSyncInfo {
            active,
            account: account.map(Into::into),
            established,
            syncing,
            scan: scan.map(Into::into),
            last_pass_failed,
            spends_available,
        }
    }
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
            unresolved_spends,
            held_spends,
            held,
            unrecorded_spends,
            synced_index,
            last_sync_at,
            syncing,
            prover_ready,
            spends_available,
            account,
            scan,
            last_pass_failed,
        } = s;
        ShieldedStatus {
            active,
            protocol_version,
            activation_version,
            ready,
            balance,
            pending,
            pending_shields: pending_shields.into_iter().map(Into::into).collect(),
            unresolved_spends: unresolved_spends.into_iter().map(Into::into).collect(),
            held_spends: held_spends.into_iter().map(Into::into).collect(),
            held,
            unrecorded_spends: unrecorded_spends.into_iter().map(Into::into).collect(),
            synced_index,
            last_sync_at,
            syncing,
            prover_ready,
            spends_available,
            account: account.map(Into::into),
            scan: scan.map(Into::into),
            last_pass_failed,
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

/// How much of the Core balance a shield or an "Add from Dash wallet"
/// moves: `duffs`, its fee on top, or everything in the Dash wallet's
/// account 0 (Max), the fee out of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FundAmount {
    Duffs { duffs: u64 },
    All,
    QuotedAll { coin_set: String },
}

impl From<FundAmount> for engine::FundAmount {
    fn from(a: FundAmount) -> Self {
        match a {
            FundAmount::Duffs { duffs } => Self::Duffs { duffs },
            FundAmount::All => Self::All,
            FundAmount::QuotedAll { coin_set } => {
                let mut decoded = [0; 32];
                match hex::decode_to_slice(coin_set, &mut decoded) {
                    Ok(()) => Self::QuotedAll { coin_set: decoded },
                    Err(_) => Self::All,
                }
            }
        }
    }
}

/// What a Max quote adds: the coins it spends, and what stays in the Dash
/// wallet (other accounts' coins, coins still arriving).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreMax {
    pub inputs: u32,
    pub left_duffs: u64,
    pub arriving_duffs: u64,
    pub coin_set: String,
}

impl From<engine::CoreMax> for CoreMax {
    fn from(m: engine::CoreMax) -> Self {
        let engine::CoreMax {
            inputs,
            left_duffs,
            arriving_duffs,
            coin_set,
        } = m;
        CoreMax {
            inputs,
            left_duffs,
            arriving_duffs,
            coin_set: hex::encode(coin_set),
        }
    }
}

/// What moving duffs of the Core balance into the pool costs and buys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    /// A Max's (`all`): `totalDuffs` is the coins' sum, `duffs` the lock.
    pub max: Option<CoreMax>,
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
            max,
        } = q;
        ShieldQuote {
            duffs,
            fee_duffs,
            total_duffs,
            pool_fee_credits,
            credits,
            grant: grant.into(),
            max: max.map(Into::into),
        }
    }
}

/// What "Move mixed coins" to Shielded takes and buys: one asset lock
/// spending every coin of the CoinJoin account. Its grant is a `Spend`
/// grant of `totalDuffs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MixedShieldQuote {
    pub inputs: u32,
    pub total_duffs: u64,
    pub fee_duffs: u64,
    pub duffs: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub pool_fee_credits: u64,
    /// What the shielded balance gains, at least.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub fully_mixed_duffs: u64,
    pub not_fully_mixed_duffs: u64,
}

impl From<engine::MixedShieldQuote> for MixedShieldQuote {
    fn from(q: engine::MixedShieldQuote) -> Self {
        let engine::MixedShieldQuote {
            inputs,
            total_duffs,
            fee_duffs,
            duffs,
            pool_fee_credits,
            credits,
            fully_mixed_duffs,
            not_fully_mixed_duffs,
        } = q;
        MixedShieldQuote {
            inputs,
            total_duffs,
            fee_duffs,
            duffs,
            pool_fee_credits,
            credits,
            fully_mixed_duffs,
            not_fully_mixed_duffs,
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
    /// The apply was refused before its transition was signed, with `error`
    /// (a code): nothing reached Platform. `resumeShield` tries again.
    NotApplied { error: String },
    /// The apply may have executed, unproved (Platform said the lock is
    /// already consumed, or the apply failed after signing): it stays
    /// pending (`consumptionUnknown`, "Checking") until a proved apply
    /// (`resumeShield`, "Check again"); it blocks no new shield.
    ConsumptionUnknown,
}

impl From<engine::ShieldState> for ShieldState {
    fn from(s: engine::ShieldState) -> Self {
        use engine::ShieldState as S;
        match s {
            S::Applied { balance } => ShieldState::Applied { balance },
            S::ProofWaiting => ShieldState::ProofWaiting,
            S::NotApplied { error } => ShieldState::NotApplied { error },
            S::ConsumptionUnknown => ShieldState::ConsumptionUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldOutcome {
    /// The asset lock's txid (hex).
    pub txid: String,
    pub state: ShieldState,
    /// Broadcast evidence while the transfer is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_dispatch: Option<PendingDispatch>,
}

impl From<engine::ShieldOutcome> for ShieldOutcome {
    fn from(o: engine::ShieldOutcome) -> Self {
        let engine::ShieldOutcome {
            txid,
            state,
            pending_dispatch,
        } = o;
        ShieldOutcome {
            txid,
            state: state.into(),
            pending_dispatch: pending_dispatch.map(Into::into),
        }
    }
}

/// A move into Shielded (a shield or a move of mixed coins) whose payment
/// may have been sent: it refuses every later one until it settles or
/// Tools ▸ Repair resolves it (`resolveStuckFunding` with `step`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StuckShieldFunding {
    /// `shield/<k>/funding`.
    pub step: String,
    /// `maybeSent` or `willBeSent`.
    pub state: DispatchStateName,
    /// What Repair would spend for it now, or `null` with `blocked`.
    pub repair: Option<ShieldRepairCost>,
    /// Why Repair refuses it now, naming no transaction, coin or address
    /// (`resolveStuckFunding` answers the details); `null` with `repair`.
    pub blocked: Option<ShieldRepairBlocked>,
}

/// Why Repair refuses a listed move into Shielded now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldRepairBlocked {
    pub reason: ShieldRepairBlockedReason,
    /// The same Repair may succeed later with nothing done by the user.
    pub retry_later: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldRepairBlockedReason {
    /// The wallet's records of the move are not read yet, or a call of it
    /// still runs.
    NotReady,
    /// A spend of the wallet's coins is not ChainLocked yet.
    SpendUnsettled,
    OwnLockUnfinalized,
    /// The coins the move's payment spends are already spent.
    LockInputsSpent,
    /// A payment of the wallet may be the move's and is not matched yet.
    UnattributedLock,
    /// No payment the move signed is known, so nothing shows which coins
    /// to send back.
    NoKnownLock,
    /// A coin Repair would send back is locked or held by another action.
    CoinsHeld,
    /// The wallet is mixing.
    Mixing,
    /// No coin is left to send back.
    NoCoins,
    Other,
}

impl From<&dw_engine::EngineError> for ShieldRepairBlocked {
    fn from(e: &dw_engine::EngineError) -> Self {
        use ShieldRepairBlockedReason as B;
        use dw_engine::platform::lease::StuckFundingRefusal as R;
        use dw_engine::{CoinJoinFailure, EngineError as E, SendFailure};
        let reason = match e {
            E::StuckFunding(r) => match r {
                R::RowsUnknown | R::CallRunning | R::JournalUnavailable | R::LockAbandoned(_) => {
                    B::NotReady
                }
                R::SpendUnsettled(_) => B::SpendUnsettled,
                R::OwnLockUnfinalized(_) => B::OwnLockUnfinalized,
                R::LockInputsSpent(_) | R::MissesSignedLock(_) => B::LockInputsSpent,
                R::UnattributedLock(_) => B::UnattributedLock,
                R::NoKnownLock | R::NoMarker => B::NoKnownLock,
                R::NotStuck(_) => B::Other,
            },
            E::Send(SendFailure::OutpointUnavailable(_)) => B::CoinsHeld,
            E::CoinJoin(CoinJoinFailure::MixingActive) => B::Mixing,
            E::CoinJoin(CoinJoinFailure::MoveRunning) => B::CoinsHeld,
            E::InvalidArgument(_) => B::NoCoins,
            _ => B::Other,
        };
        let retry_later = match e {
            E::StuckFunding(r) => r.retry_later(),
            E::CoinJoin(CoinJoinFailure::MoveRunning) => true,
            _ => false,
        };
        ShieldRepairBlocked {
            reason,
            retry_later,
        }
    }
}

/// Repair's payment of every coin of a stuck move back to the wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShieldRepairCost {
    /// A move of mixed coins: Repair spends only those coins, never the
    /// Core balance.
    pub mixed: bool,
    pub inputs: u32,
    /// The coins' sum; the wallet gets it back less `feeDuffs`.
    pub total_duffs: u64,
    pub fee_duffs: u64,
}

impl From<engine::StuckShieldFunding> for StuckShieldFunding {
    fn from(s: engine::StuckShieldFunding) -> Self {
        let engine::StuckShieldFunding {
            step,
            state,
            repair,
        } = s;
        let (repair, blocked) = match repair {
            Ok(engine::ShieldRepairCost {
                mixed,
                inputs,
                total_duffs,
                fee_duffs,
            }) => (
                Some(ShieldRepairCost {
                    mixed,
                    inputs,
                    total_duffs,
                    fee_duffs,
                }),
                None,
            ),
            Err(e) => (None, Some(ShieldRepairBlocked::from(&e))),
        };
        StuckShieldFunding {
            step,
            state: state.into(),
            repair,
            blocked,
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
    /// Broadcast evidence for this asset lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_dispatch: Option<PendingDispatch>,
    /// The lock may already be applied ("Checking": Platform said so
    /// unproved, an apply failed after signing, or a restore read it back);
    /// its amount may already be in the shielded balance. `resumeShield`
    /// checks again; only a proved apply clears it. It blocks no new
    /// shield.
    pub consumption_unknown: bool,
}

impl From<engine::PendingShield> for PendingShield {
    fn from(p: engine::PendingShield) -> Self {
        let engine::PendingShield {
            txid,
            duffs,
            proved,
            pending_dispatch,
            consumption_unknown,
        } = p;
        PendingShield {
            txid,
            duffs,
            proved,
            pending_dispatch: Some(pending_dispatch.into()),
            consumption_unknown,
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
    pub state: ShieldedSpendState,
    /// Broadcast evidence while the transfer is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_dispatch: Option<PendingDispatch>,
    /// For a `pending` spend, the library's record of it, when it kept one.
    pub unresolved: Option<UnresolvedSpend>,
}

impl From<engine::ShieldedSpendOutcome> for ShieldedSpendOutcome {
    fn from(o: engine::ShieldedSpendOutcome) -> Self {
        let engine::ShieldedSpendOutcome {
            credits,
            balance,
            to,
            state,
            unresolved,
        } = o;
        ShieldedSpendOutcome {
            credits,
            balance,
            to,
            state: state.into(),
            pending_dispatch: (state == engine::ShieldedSpendState::Pending)
                .then_some(PendingDispatch::Queued),
            unresolved: unresolved.map(Into::into),
        }
    }
}

/// What an unshield to the wallet's own Platform address did: as
/// `ShieldedSpendOutcome`, plus `record`, the `id` of its `fromShielded`
/// Platform activity row when the wallet recorded one. That row turns
/// `done` only once Platform executed the unshield.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldToPlatformOutcome {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    /// The shielded balance after.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub balance: u64,
    /// The Platform address paid.
    pub to: String,
    pub state: ShieldedSpendState,
    /// Broadcast evidence while the transfer is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_dispatch: Option<PendingDispatch>,
    pub unresolved: Option<UnresolvedSpend>,
    pub record: Option<String>,
}

impl From<engine::UnshieldToPlatformOutcome> for UnshieldToPlatformOutcome {
    fn from(o: engine::UnshieldToPlatformOutcome) -> Self {
        let engine::UnshieldToPlatformOutcome {
            credits,
            balance,
            to,
            state,
            unresolved,
            record,
        } = o;
        UnshieldToPlatformOutcome {
            credits,
            balance,
            to,
            state: state.into(),
            pending_dispatch: (state == engine::ShieldedSpendState::Pending)
                .then_some(PendingDispatch::Queued),
            unresolved: unresolved.map(Into::into),
            record,
        }
    }
}

/// Where a spend stands when the call returns. `pending`: the wallet
/// holds the spend and its result is unknown. Its history
/// row (`unresolved.activityId`) turns `confirmed` when a scan sees it
/// executed, or `failed` when the notes come back (not executed). The
/// wallet re-sends the same signed transition itself; offer no retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedSpendState {
    Done,
    Pending,
}

mirror_enum!(engine::ShieldedSpendState => ShieldedSpendState { Done, Pending });

/// A spend whose result is not confirmed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedSpend {
    /// Its history row's id.
    pub activity_id: String,
    /// Platform's hash of its signed state transition (hex).
    pub transition: String,
    /// Re-sends so far.
    pub attempts: u32,
}

impl From<engine::UnresolvedSpend> for UnresolvedSpend {
    fn from(u: engine::UnresolvedSpend) -> Self {
        let engine::UnresolvedSpend {
            activity_id,
            transition,
            attempts,
        } = u;
        UnresolvedSpend {
            activity_id,
            transition,
            attempts,
        }
    }
}

/// A spend that may still go through, whose notes are held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeldSpend {
    /// Its history row's id.
    pub activity_id: String,
    /// Platform's hash of its signed state transition (hex).
    pub transition: String,
    /// Credits of the unspent notes it holds; "0" once all are spent. Two
    /// spends that hold one note both count it.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
}

impl From<engine::HeldSpend> for HeldSpend {
    fn from(h: engine::HeldSpend) -> Self {
        let engine::HeldSpend {
            activity_id,
            transition,
            credits,
        } = h;
        HeldSpend {
            activity_id,
            transition,
            credits,
        }
    }
}

/// A spend of unknown result whose record was lost (DEC-1057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnrecordedSpend {
    /// Its history row's id.
    pub activity_id: String,
    /// Unix seconds of the spend.
    pub since: u64,
}

impl From<engine::UnrecordedSpend> for UnrecordedSpend {
    fn from(u: engine::UnrecordedSpend) -> Self {
        let engine::UnrecordedSpend { activity_id, since } = u;
        UnrecordedSpend { activity_id, since }
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

/// Whether a spend's outcome is held open: `held` while the wallet cannot
/// rule out that it executes (a refusal at its broadcast, or an outcome not
/// recorded or not readable yet; its row reads pending), `released` once a
/// refusal is proved never to go through (its row reads failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShieldedRefusal {
    Held,
    Released,
}

mirror_enum!(engine::ShieldedRefusal => ShieldedRefusal { Held, Released });

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
    /// Broadcast evidence while the transfer is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending_dispatch: Option<PendingDispatch>,
    pub created_at: Option<u64>,
    /// The DashPay contact a withdrawal paid, when it was a contact
    /// payment.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<crate::dto::history::TxCounterparty>,
    /// Set on a spend whose outcome is held open, or a refusal released.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<ShieldedRefusal>,
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
            contact,
            refusal,
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
            pending_dispatch: (status == engine::ShieldedActivityStatus::Pending)
                .then_some(PendingDispatch::Queued),
            created_at,
            contact: contact.map(Into::into),
            refusal: refusal.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use dashcore::{OutPoint, Txid};
    use dw_engine::platform::lease::StuckFundingRefusal as R;
    use dw_engine::{EngineError, SendFailure};
    use serde_json::json;

    use super::*;

    #[test]
    fn unconfirmed_shielded_spends_cross_as_queued() {
        let engine = engine::ShieldedSpendOutcome {
            credits: 1,
            balance: 2,
            to: "fixture".into(),
            state: engine::ShieldedSpendState::Pending,
            unresolved: None,
        };
        let shielded = serde_json::to_value(ShieldedSpendOutcome::from(engine.clone())).unwrap();
        assert_eq!(shielded["pendingDispatch"], "queued");
        let platform = serde_json::to_value(UnshieldToPlatformOutcome::from(
            engine::UnshieldToPlatformOutcome {
                credits: engine.credits,
                balance: engine.balance,
                to: engine.to,
                state: engine.state,
                unresolved: engine.unresolved,
                record: Some("from-shielded-fixture".into()),
            },
        ))
        .unwrap();
        assert_eq!(platform["pendingDispatch"], "queued");
    }

    #[test]
    fn core_max_coin_set_round_trips_through_the_app_contract() {
        let coin_set = [0xba; 32];
        let quote = CoreMax::from(engine::CoreMax {
            inputs: 2,
            left_duffs: 0,
            arriving_duffs: 0,
            coin_set,
        });
        let wire = json!({ "kind": "quotedAll", "coinSet": quote.coin_set });
        let amount: FundAmount = serde_json::from_value(wire).unwrap();
        assert_eq!(engine::FundAmount::from(amount), engine::FundAmount::QuotedAll { coin_set });
    }

    /// Repair's listing says why it refuses a move without naming a
    /// transaction, a coin or an address; `resolveStuckFunding` answers
    /// those.
    #[test]
    fn a_listed_refusal_names_no_transaction_coin_or_address() {
        let txid = Txid::from_str(&"5a".repeat(32)).unwrap();
        let outpoint = OutPoint::new(txid, 7);
        for (error, reason, retry_later) in [
            (
                EngineError::StuckFunding(R::SpendUnsettled(txid)),
                "spendUnsettled",
                true,
            ),
            (
                EngineError::StuckFunding(R::OwnLockUnfinalized(txid)),
                "ownLockUnfinalized",
                true,
            ),
            (
                EngineError::StuckFunding(R::LockInputsSpent(txid)),
                "lockInputsSpent",
                false,
            ),
            (
                EngineError::StuckFunding(R::MissesSignedLock(txid)),
                "lockInputsSpent",
                false,
            ),
            (
                EngineError::Send(SendFailure::OutpointUnavailable(outpoint)),
                "coinsHeld",
                false,
            ),
        ] {
            let listed = StuckShieldFunding::from(engine::StuckShieldFunding {
                step: "shield/0/funding".into(),
                state: engine::DispatchState::MaybeSent,
                repair: Err(error),
            });
            let value = serde_json::to_value(&listed).unwrap();
            assert_eq!(
                value,
                json!({
                    "step": "shield/0/funding",
                    "state": "maybeSent",
                    "repair": null,
                    "blocked": { "reason": reason, "retryLater": retry_later },
                })
            );
            let text = value.to_string();
            assert!(!text.contains("5a5a"), "{text}");
            assert!(!text.contains(&outpoint.to_string()), "{text}");
        }
    }
}
