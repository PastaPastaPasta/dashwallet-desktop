//! Credits: the cost table, top-up and withdraw, each with a quote
//! (DP1-06, DP6-02).

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::CreditsError;
use super::flows::GrantRequest;
use super::topup;

/// Credit costs of the DashPay actions at the network's protocol version,
/// for "≈ N contact requests" and the low-credit warnings (F8). Every
/// price is the network's (`credit_costs.rs`); a document is priced at its
/// largest size, so each is an upper bound of what Platform charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTable {
    /// The protocol version priced: the network's, as the SDK last proved
    /// it.
    pub protocol_version: u32,
    pub contact_request: u64,
    pub profile_update: u64,
    pub contact_info: u64,
    pub enable_dashpay_keys: u64,
    pub credits_per_duff: u64,
    /// What Drive requires a top-up's lock to hold before it processes the
    /// top-up, in credits (`calculate_min_required_fee`: its 50,000-duff
    /// processing floor plus the top-up's base cost). Not the fee charged:
    /// Platform meters that at execution, and the one testnet measurement
    /// was far lower, so a lock of `duffs` added more than
    /// `duffs × credits_per_duff` less this (m4 §7 item 46).
    pub top_up_fee: u64,
    pub top_up_min_duffs: u64,
    /// At or below this balance the credits are low.
    pub low_credits: u64,
    /// Below this balance the credits are depleted: no DashPay action fits.
    pub empty_credits: u64,
}

/// Where an identity's credit balance stands against the cost table
/// (Android `CreditBalanceInfo`; DASHPAY F8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditLevel {
    Ok,
    /// "Your credit balance is low".
    Low,
    /// "Your credit balance is fully depleted": below one of the dearest
    /// action.
    Empty,
}

/// What a top-up of `duffs` costs and buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpQuote {
    /// The asset lock's fee bound (E0-04's `fee_bound_worst`).
    pub fee_duffs: u64,
    /// `duffs + fee_duffs`: what the grant must cover.
    pub total_duffs: u64,
    /// `duffs × credits_per_duff` less the table's `top_up_fee`: an
    /// estimate that errs low as far as measured. The top-up adds the
    /// lock's credits less Platform's metered fee, which no bound proves.
    pub credits: u64,
    pub grant: GrantRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpOutcome {
    /// The asset lock's txid (hex, display order).
    pub txid: String,
    pub state: TopUpState,
}

/// Where a top-up's asset lock stands once the call returns. Past the
/// funding, nothing is lost: the lock stays tracked until Platform's
/// proved apply takes it (or the user resolves an unknown one),
/// `pending_top_ups` lists it and `resume_top_up` (or Tools ▸ Repair
/// "Finish transfers") applies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopUpState {
    /// Platform added the credits. `balance` is its new balance (proved);
    /// `credits_added` is the rise, when the balance before was read.
    Applied {
        credits_added: Option<u64>,
        balance: Option<u64>,
    },
    /// The lock was applied before (the library holds Platform's proved
    /// apply): nothing was added now. `balance` is read afresh.
    AlreadyApplied { balance: Option<u64> },
    /// Platform answered that the lock is taken, an answer nothing
    /// proves: nothing was added now, and the lock stays pending
    /// (`consumption_unknown` once the library holds it final) until a
    /// proved apply or `resolve_top_up`. `balance` is read afresh.
    ConsumptionUnknown { balance: Option<u64> },
    /// The lock is out with no proof yet (InstantSend did not lock it in
    /// its window; a ChainLock will).
    ProofWaiting,
    /// The lock has its proof; applying it failed with `error` (a code).
    NotApplied { error: String },
}

/// A top-up lock of the wallet that Platform has not taken yet, as far as
/// the wallet can prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTopUp {
    /// The asset lock's txid (hex, display order).
    pub txid: String,
    /// The identity it tops up (base58), when the wallet knows the
    /// identity at its index.
    pub identity: Option<String>,
    pub identity_index: u32,
    pub duffs: u64,
    /// The lock has its proof: `resume_top_up` applies it at once.
    pub proved: bool,
    /// A lock a top-up of this wallet funded, final on the chain, whose
    /// consumption by Platform is not known (a restart lost its apply, or
    /// Platform answered "already consumed" without proof). It blocks the
    /// identity's top-ups until `resume_top_up` applies it with proof or
    /// the user resolves it (`resolve_top_up`).
    pub consumption_unknown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawAmount {
    /// The balance less the fee reserve.
    All,
    Credits {
        credits: u64,
    },
}

/// What a withdrawal of `amount` takes and pays out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawQuote {
    pub credits: u64,
    pub fee_credits: u64,
    pub expected_duffs: u64,
    pub grant: GrantRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawOutcome {
    pub credits: u64,
    pub expected_duffs: Option<u64>,
    pub remaining_credits: Option<u64>,
}

impl DashPay {
    /// The cost table at the network's protocol version (`credit_costs.rs`).
    pub fn cost_table(&self) -> Result<CostTable, CreditsError> {
        Ok(self.session.cost_table()?)
    }

    /// What a top-up of `duffs` costs and buys, with the same refusals as
    /// [`Self::top_up`].
    pub async fn top_up_quote(
        &self,
        identity: String,
        duffs: u64,
    ) -> Result<TopUpQuote, CreditsError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, lib| async move {
            topup::quote(&session, wallet, &*lib, &identity, duffs).await
        })
        .await
    }

    /// Funds the identity from the Core balance: an asset lock of `duffs`,
    /// then its IdentityTopUp (`topup.rs`).
    pub async fn top_up(
        &self,
        identity: String,
        duffs: u64,
        grant: String,
    ) -> Result<TopUpOutcome, CreditsError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, lib| async move {
            topup::top_up(&session, wallet, &*lib, &identity, duffs, &grant).await
        })
        .await
    }

    /// The wallet's top-up locks Platform has not taken, oldest first.
    pub async fn pending_top_ups(&self) -> Result<Vec<PendingTopUp>, CreditsError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, lib| async move {
            Ok(topup::pending(&session, wallet, &*lib).await?)
        })
        .await
    }

    /// Applies the tracked top-up lock `txid` (a `pending_top_ups` entry),
    /// waiting briefly for its proof first. A lock applied before reads
    /// `AlreadyApplied`.
    pub async fn resume_top_up(
        &self,
        txid: String,
        grant: String,
    ) -> Result<TopUpOutcome, CreditsError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, lib| async move {
            topup::resume(&session, wallet, &*lib, &txid, &grant).await
        })
        .await
    }

    /// The user's resolution of the `consumption_unknown` top-up lock
    /// `txid` (they saw its credits arrive): it no longer pends or blocks
    /// the identity's top-ups. `resume_top_up` and "Finish transfers" still
    /// apply it, so a lock Platform had not taken is not lost.
    pub async fn resolve_top_up(&self, txid: String) -> Result<(), CreditsError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, lib| async move {
            topup::resolve(&session, wallet, &*lib, &txid).await
        })
        .await
    }

    #[expect(unused_variables, reason = "stub until DP6-02")]
    pub async fn withdraw_quote(
        &self,
        identity: String,
        amount: WithdrawAmount,
    ) -> Result<WithdrawQuote, CreditsError> {
        stub("DashPay.withdraw_quote")
    }

    #[expect(unused_variables, reason = "stub until DP6-02")]
    pub async fn withdraw(
        &self,
        identity: String,
        to: String,
        amount: WithdrawAmount,
        grant: String,
    ) -> Result<WithdrawOutcome, CreditsError> {
        stub("DashPay.withdraw")
    }
}
