//! What a party checks before its consent signature (MG-04b §4, the
//! registration row), past what [`super::funding::check_frozen`] already
//! holds (`payloadMatchesEnvelope`): the transaction is one Core can accept
//! (D12, D16), this wallet's row and contribution are exactly what it
//! recorded at join or start, no other contribution spends one of its coins
//! (`foreignWalletInputs`, rc.3 `sharedmncreatedialog.cpp:2433-2446`), and
//! its debit is exactly its recorded contribution's, within
//! `ownContributionError`'s bound (`:2475-2513`, m-1).

use dashcore::OutPoint;

use super::funding::Frozen;
use super::{Contribution, Session, Share};
use crate::shared::rawtx::SPECIAL_VERSION;

/// `GetDustThreshold` of a P2PKH output at Core's `DUST_RELAY_TX_FEE`
/// (3,000 duffs/kB, rc.3 `policy/policy.h`): (34 + 148) × 3 duffs.
pub const P2PKH_DUST: u64 = 546;

/// `nSequence` the wallet signs with (D16).
const FINAL_SEQUENCE: u32 = u32::MAX;

/// The engine's `shared_funding_mismatch{reason}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FundingMismatch {
    /// Not a transaction Core could accept: `nVersion` ≠ 3 or a wrong
    /// `inputsHash` (D12; the payload version is the decoder's).
    #[error("the registration isn't one Core accepts")]
    Template,
    /// A lock time or a non-final sequence (D16).
    #[error("the registration has a time lock")]
    TimeLock,
    #[error("this wallet's share changed")]
    RowChanged,
    #[error("this wallet's contribution changed")]
    ContributionChanged,
    /// Another contribution spends one of this wallet's coins.
    #[error("another contribution spends one of this wallet's coins")]
    ForeignInput,
    /// The recorded contribution debits more than the share (and fee).
    #[error("this wallet would pay more than its share")]
    Overdraw,
}

/// This wallet's side of a session, as recorded at `join_shared` or
/// `start_shared` (§4, m-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSide {
    /// Its row (one share per wallet and session, D10).
    pub share_index: usize,
    pub row: Share,
    pub contribution: Contribution,
    /// `I`: the total of its reserved inputs, which only it can value.
    pub input_total: u64,
    /// The registration fee it pays: `Some` for the coordinator (D9).
    pub fee: Option<u64>,
}

impl OwnSide {
    /// `I − C`, the debit its recorded contribution makes.
    pub fn debit(&self) -> Option<u64> {
        let change = self
            .contribution
            .change
            .as_ref()
            .map_or(0, |c| c.amount as u64);
        self.input_total.checked_sub(change)
    }

    /// m-1's bound: `I − C ≤ share + fee + (no change ? dust − 1 : 0)`.
    pub fn within_bound(&self) -> bool {
        let slack = if self.contribution.change.is_none() {
            P2PKH_DUST - 1
        } else {
            0
        };
        let allowed = (self.row.amount as u64)
            .saturating_add(self.fee.unwrap_or(0))
            .saturating_add(slack);
        self.debit().is_some_and(|d| d <= allowed)
    }
}

/// §4's registration checks, in order. `is_wallet_coin` says whether an
/// outpoint is one of this wallet's coins (spendable or reserved).
pub fn check_consent(
    session: &Session,
    frozen: &Frozen,
    own: &OwnSide,
    is_wallet_coin: impl Fn(&OutPoint) -> bool,
) -> Result<(), FundingMismatch> {
    let tx = &frozen.tx;
    if tx.version != SPECIAL_VERSION || frozen.payload.inputs_hash != tx.inputs_hash() {
        return Err(FundingMismatch::Template);
    }
    if tx.lock_time != 0 || tx.input.iter().any(|i| i.sequence != FINAL_SEQUENCE) {
        return Err(FundingMismatch::TimeLock);
    }
    let row = session.shares.get(own.share_index);
    if !row
        .is_some_and(|r| r.label == own.row.label && r.amount == own.row.amount && r.same(&own.row))
    {
        return Err(FundingMismatch::RowChanged);
    }
    let theirs = session
        .contributions
        .iter()
        .find(|c| c.label == own.row.label);
    if !theirs.is_some_and(|c| c.same(&own.contribution)) {
        return Err(FundingMismatch::ContributionChanged);
    }
    let mine: Vec<OutPoint> =
        super::funding::funding_parts(std::slice::from_ref(&own.contribution), session.network)
            .map_err(|_| FundingMismatch::ContributionChanged)?
            .0
            .into_iter()
            .map(|i| i.previous_output)
            .collect();
    if tx
        .input
        .iter()
        .any(|i| !mine.contains(&i.previous_output) && is_wallet_coin(&i.previous_output))
    {
        return Err(FundingMismatch::ForeignInput);
    }
    if !own.within_bound() {
        return Err(FundingMismatch::Overdraw);
    }
    Ok(())
}
