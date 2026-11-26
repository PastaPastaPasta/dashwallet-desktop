//! Unfinished transfers (m4 §2.14): the wallet's funding asset locks whose
//! Platform side is not known to have happened, and finishing one on the
//! same outpoint.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;

/// A kind of funding lock whose apply moves its value to the wallet's own
/// Platform-side balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LockKind {
    /// Core → Shielded.
    Shielded,
    /// Core → Platform address.
    PlatformAddress,
}

mirror_enum!(engine::LockKindId => LockKind { Shielded, PlatformAddress });

impl From<LockKind> for engine::LockKindId {
    fn from(k: LockKind) -> Self {
        match k {
            LockKind::Shielded => engine::LockKindId::Shielded,
            LockKind::PlatformAddress => engine::LockKindId::PlatformAddress,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LockOrigin {
    /// A funding step on this install signed it.
    ThisDevice,
    /// A restore read it back from the chain.
    Restored,
}

mirror_enum!(engine::LockOrigin => LockOrigin { ThisDevice, Restored });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UnfinishedState {
    /// No proof yet: nothing to apply.
    Confirming,
    /// Ready to finish.
    Ready,
    /// The apply may have executed, or a lock read back from the chain may
    /// already have been taken, unproved ("Checking"): its finish tries it.
    Checking,
    /// Not listed in this version: a lock Platform reported already used
    /// leaves the list, and its finish reads [`LockFinish::ReportedUsed`].
    ReportedUsed,
    /// The kind is not active on this network yet.
    NotActive,
    /// Can't be finished in this version (a credit output other than its
    /// transaction's first). Never offered a finish.
    Unsupported,
    /// Worth less than the Platform fee its finish pays. Never offered a
    /// finish. With `remainingCredits`: a partly used transfer to Shielded
    /// whose remaining value is below Platform's fee for moving it, never
    /// offered a recover.
    TooSmall,
    /// A transfer to Shielded Platform refused after a penalty, proved
    /// partly used: `remainingCredits` are left, at least Platform's fee
    /// for moving them. Offered a recover (`recoverTransfer`), never a
    /// finish.
    Recoverable,
}

mirror_enum!(engine::UnfinishedState => UnfinishedState {
    Confirming, Ready, Checking, ReportedUsed, NotActive, Unsupported, TooSmall, Recoverable
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedLock {
    pub txid: String,
    pub vout: u32,
    pub kind: LockKind,
    /// The credit output's value.
    pub duffs: u64,
    pub origin: LockOrigin,
    pub state: UnfinishedState,
    /// For a partly used transfer, the credits Platform's proved state
    /// showed left when it was read; a recover reads them again.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub remaining_credits: Option<u64>,
}

impl From<engine::UnfinishedLock> for UnfinishedLock {
    fn from(l: engine::UnfinishedLock) -> Self {
        let engine::UnfinishedLock {
            txid,
            vout,
            kind,
            duffs,
            origin,
            state,
            remaining_credits,
        } = l;
        UnfinishedLock {
            txid,
            vout,
            kind: kind.into(),
            duffs,
            origin: origin.into(),
            state: state.into(),
            remaining_credits,
        }
    }
}

/// What a recover of a partly used transfer to Shielded would move, read
/// from Platform's proved state when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RecoverQuote {
    /// `credits` are left, at least `feeCredits` (Platform's fee for moving
    /// them, which comes out of them). The recover's grant carries exactly
    /// `credits` as its credits cap.
    Ready {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        credits: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        fee_credits: u64,
    },
    /// `credits` are left, less than `feeCredits`: never recovered.
    TooSmall {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        credits: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        fee_credits: u64,
    },
    /// Nothing left to move: recovered, or Platform proved it used up.
    Finished,
    /// Not recoverable now, with `error` (a code).
    NotApplied { error: String },
}

impl From<engine::RecoverQuote> for RecoverQuote {
    fn from(q: engine::RecoverQuote) -> Self {
        use engine::RecoverQuote as Q;
        match q {
            Q::Ready {
                credits,
                fee_credits,
            } => RecoverQuote::Ready {
                credits,
                fee_credits,
            },
            Q::TooSmall {
                credits,
                fee_credits,
            } => RecoverQuote::TooSmall {
                credits,
                fee_credits,
            },
            Q::Finished => RecoverQuote::Finished,
            Q::NotApplied { error } => RecoverQuote::NotApplied { error },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedTransfers {
    pub locks: Vec<UnfinishedLock>,
    /// False while sync or a restore's scan has not caught up: a restored
    /// lock may still be missing.
    pub complete: bool,
    /// The kinds that couldn't be checked: none of their locks is listed,
    /// the other kinds' are.
    pub unread: Vec<LockKind>,
}

impl From<engine::UnfinishedTransfers> for UnfinishedTransfers {
    fn from(t: engine::UnfinishedTransfers) -> Self {
        UnfinishedTransfers {
            locks: t.locks.into_iter().map(Into::into).collect(),
            complete: t.complete,
            unread: t.unread.into_iter().map(Into::into).collect(),
        }
    }
}

/// How finishing one transfer ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LockFinish {
    /// Platform's proved execution: finished.
    Applied,
    /// Platform answered "already consumed" for this lock's exact output,
    /// unproved (type 4, and a restored type-5 lock): taken as finished
    /// elsewhere, the lock leaves the lists and nothing is settled. This
    /// device's own shield locks read `Checking` instead.
    ReportedUsed,
    /// The apply may have executed, unproved ("Checking").
    Checking,
    /// The lock has no proof yet.
    ProofWaiting,
    /// Refused before anything was signed, with `error` (a code).
    NotApplied { error: String },
}

impl From<engine::LockFinish> for LockFinish {
    fn from(f: engine::LockFinish) -> Self {
        use engine::LockFinish as F;
        match f {
            F::Applied => LockFinish::Applied,
            F::ReportedUsed => LockFinish::ReportedUsed,
            F::Checking => LockFinish::Checking,
            F::ProofWaiting => LockFinish::ProofWaiting,
            F::NotApplied { error } => LockFinish::NotApplied { error },
        }
    }
}

/// One kind's part of "Finish transfers".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TransfersReport {
    pub kind: LockKind,
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}

impl From<engine::TransfersReport> for TransfersReport {
    fn from(r: engine::TransfersReport) -> Self {
        let engine::TransfersReport {
            kind,
            resumed,
            completed,
            still_pending,
        } = r;
        TransfersReport {
            kind: kind.into(),
            resumed,
            completed,
            still_pending,
        }
    }
}
