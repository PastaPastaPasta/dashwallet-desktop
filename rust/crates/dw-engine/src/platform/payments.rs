//! Contact payments: the per-contact lock, contact activity and the frequent
//! strip (DP3-01, DP3-02). The `TxDraft` recipient lives in `send/`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::contacts::ContactSummary;
use super::dashpay::DashPay;
use super::errors::{ContactError, PlatformError};
use super::flows::GrantRequest;
use super::shielded_payout;
use super::shielded_pool::{on_pool, on_pool_signing};

/// A per-contact send lock: a Core payment after an ambiguous broadcast
/// (`dp_payment_lock`), or a `pending` payout from another balance
/// (`dp_contact_payout`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentLock {
    /// Equals `reference`; a Core txid only when `source` is `core`.
    pub txid: String,
    pub since: u64,
    pub source: PaymentSource,
    pub reference: String,
}

/// The balance a contact payment comes from (C-funding D-C5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentSource {
    Core,
    Shielded,
    Platform,
}

/// `contact_pay_quote`'s answer: the typed amount leaves the balance (M3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPayQuote {
    pub source: PaymentSource,
    /// What leaves the balance: the amount typed.
    pub credits: u64,
    /// What the contact receives on Core, about.
    pub expected_duffs: u64,
    /// The network fee estimate.
    pub fee_credits: u64,
    /// What to authorize: no duffs, `credits` credits.
    pub grant: GrantRequest,
}

/// Where a payout stands (`dp_contact_payout.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutState {
    /// It may have been sent: the contact stays locked.
    Pending,
    Sent,
    /// Proved never to execute.
    NotSent,
}

/// `contact_pay`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPayOutcome {
    /// The payout's reference: `PaymentLock.reference`, `ActivityItem.reference`.
    pub reference: String,
    pub state: PayoutState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockResolution {
    /// The payment was sent: an attempt or a Resend finished `Sent`, or the
    /// wallet has seen it. The lock is cleared.
    Sent,
    /// Positive evidence that it was never sent: in this process its
    /// `dispatch_status` is `Some(NotSent)`, or a ChainLocked spend conflicts
    /// with one of its inputs. Never "not found". The lock is cleared.
    NotSent,
    /// Anything else, read like `MaybeSent`; the lock stays.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityFilter {
    All,
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    /// Equals `reference`; a Core txid only when `source` is `core`.
    pub txid: String,
    pub direction: ActivityDirection,
    /// Duffs.
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
    pub source: PaymentSource,
    pub reference: String,
    /// The Core transaction, once the wallet has seen it.
    pub core_txid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDirection {
    Sent,
    Received,
}

/// The other side of a contact payment; history and transaction records
/// gain it as `counterparty` in DP3-02.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterparty {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl DashPay {
    /// The lock an unknown-outcome payment to `contact` set, if any; from
    /// memory (DP3-01, `payment_locks.rs`). Re-query on `Platform{Payments}`.
    pub fn payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<PaymentLock>, ContactError> {
        (self.session).payment_lock_of(self.wallet_id(), &identity, &contact)
    }

    /// Settles the pair's lock on positive evidence only (m4 §2.5, item
    /// 34): `Sent` or `NotSent` clears it, `Unknown` keeps it. A pair with
    /// no lock is `invalid_argument`.
    pub async fn resolve_payment_lock(
        &self,
        identity: String,
        contact: String,
    ) -> Result<LockResolution, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(
                async move { Ok(session.resolve_payment_lock_of(id, identity, contact).await) },
            )
            .await
            .map_err(PlatformError::from)?
    }

    /// `identity`'s payments to and from `contact`, newest first (mined by
    /// block time, unmined first), 50 per page; `cursor` is the previous
    /// page's `next_cursor`. Attributed as history attributes them
    /// (`attribution.rs`); abandoned and refused transactions are left out.
    /// Re-query on `Platform{Payments}`, `Platform{Contacts}` and the
    /// wallet's `HistoryChanged` (DP3-02).
    pub async fn contact_activity(
        &self,
        identity: String,
        contact: String,
        cursor: Option<String>,
        f: ActivityFilter,
    ) -> Result<ActivityPage, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move {
                Ok(session
                    .contact_activity_of(id, identity, contact, cursor, f)
                    .await)
            })
            .await
            .map_err(PlatformError::from)?
    }

    /// Quotes a payment of `credits` to `contact` from the Shielded or
    /// Platform balance (C-funding P1, P13). `core` is the `TxDraft`'s:
    /// `invalid_argument`.
    pub async fn contact_pay_quote(
        &self,
        identity: String,
        contact: String,
        credits: u64,
        source: PaymentSource,
    ) -> Result<ContactPayQuote, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        // Off on mainnet (`shielded_payout::FEATURE`): the refusal below.
        if source == PaymentSource::Shielded && shielded_payout::enabled_on(&self.session.network) {
            {
                let _op = self.session.try_enter().map_err(PlatformError::from)?;
                (self.session).payout_quote_gates(id, &identity, &contact, credits, source)?;
            }
            let quoted = on_pool(&self.session, id, move |_, lib| async move {
                Ok(shielded_payout::quote(&lib, credits).await)
            });
            return quoted.await?;
        }
        self.session
            .on_runtime(async move {
                Ok(session
                    .contact_pay_quote_of(id, identity, contact, credits, source)
                    .await)
            })
            .await
            .map_err(PlatformError::from)?
    }

    /// Pays `credits` to `contact` from the Shielded or Platform balance
    /// under `grant` (C-funding P2): the payee, the pair's lock and rule 5
    /// before the source's gates. `note` stays in this wallet (M5).
    #[expect(
        clippy::too_many_arguments,
        reason = "the payment's terms (C-funding P1)"
    )]
    pub async fn contact_pay(
        &self,
        identity: String,
        contact: String,
        credits: u64,
        source: PaymentSource,
        note: Option<String>,
        grant: String,
        confirm_unrecorded: bool,
    ) -> Result<ContactPayOutcome, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        let call = super::payment_locks::PayCall {
            identity,
            contact,
            credits,
            source,
            note,
            grant,
            confirm_unrecorded,
        };
        if source == PaymentSource::Shielded && shielded_payout::enabled_on(&self.session.network) {
            // The payee, the pair's lock and rule 5 before the pool opens.
            let gated = self
                .session
                .on_runtime(async move {
                    let _op = session.enter().await?;
                    Ok(session.pay_gates(id, &call).await.map(|pair| (pair, call)))
                })
                .await
                .map_err(PlatformError::from)?;
            let (pair, call) = gated?;
            let paid = on_pool_signing(&self.session, id, move |session, lib| async move {
                Ok(shielded_payout::pay_gated(&session, id, &lib, pair, call).await)
            });
            return paid.await?;
        }
        self.session
            .on_runtime(async move { Ok(session.contact_pay_of(id, call).await) })
            .await
            .map_err(PlatformError::from)?
    }

    /// The Pay screen's frequent strip (Android's top contacts): at most
    /// `limit` (clamped to 1–20) established contacts that are not hidden,
    /// the ones `identity` paid most often in the last 90 days first, then
    /// the ones that paid it most often; in memory, from the contacts read
    /// model and the history (DP3-02).
    pub fn frequent_contacts(
        &self,
        identity: String,
        limit: u32,
    ) -> Result<Vec<ContactSummary>, ContactError> {
        self.session.frequent_of(self.wallet_id(), identity, limit)
    }
}
