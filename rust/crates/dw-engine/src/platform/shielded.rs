//! Shielded funds (0.1 lane E1, RELEASE-PLAN §1 "Shielded"; m4-dashpay-engine
//! §2.13): the wallet's Orchard account in Platform's shielded pool, over
//! platform-wallet's `wallet::shielded` (its coordinator's sync and balance,
//! `fund_from_asset_lock.rs` for Core → Shielded, `operations.rs` for the
//! transfer and the withdrawal).
//!
//! Every call that reads or moves shielded funds first checks the network's
//! protocol version, as the SDK proved it: below
//! [`SHIELDED_ACTIVATION_VERSION`] it refuses `shielded.not_active` and the UI
//! shows "Shielded is not active on this network yet".
//!
//! Amounts: what leaves Core is in duffs (`shield_from_core`, its quote);
//! what the pool holds and spends is in credits, as Platform counts it
//! (1 duff = [`dpp::balances::credits::CREDITS_PER_DUFF`] credits).

use serde::{Deserialize, Serialize};

use super::dashpay::DashPay;
use super::errors::ShieldedError;
use super::flows::GrantRequest;
use super::shielded_pool::{self as pool, on_pool, on_pool_signing};

/// The protocol version from which Platform takes shielded-pool
/// transitions (`SHIELDED_POOL_INITIAL_PROTOCOL_VERSION`).
pub const SHIELDED_ACTIVATION_VERSION: u32 =
    dpp::version::feature_initial_protocol_versions::SHIELDED_POOL_INITIAL_PROTOCOL_VERSION;

/// Where the wallet's shielded account stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedStatus {
    /// The network takes shielded transitions: `protocol_version` is at
    /// least `activation_version`. When false, every other field is empty
    /// and the shielded calls refuse `shielded.not_active`.
    pub active: bool,
    /// The network's protocol version, as the SDK last proved it.
    pub protocol_version: u32,
    pub activation_version: u32,
    /// The account's ledger is established: its keys are bound and its
    /// notes were restored from storage or scanned this session. Until then
    /// `balance` is not a reading (it is 0) and spends refuse
    /// `shielded.not_synced`.
    pub ready: bool,
    /// Spendable credits: unspent notes less those a pending spend holds.
    pub balance: u64,
    /// Credits of the account's activity still pending (sent, not yet seen
    /// confirmed), in either direction.
    pub pending: u64,
    /// Core → Shielded asset locks out but not applied to the pool yet
    /// (`resume_shield` applies one). Empty until the dispatch journal is
    /// read (the shield calls answer `platform.unavailable` meanwhile).
    pub pending_shields: Vec<PendingShield>,
    /// Spends whose broadcast the library could not confirm
    /// ([`ShieldedSpendState::Pending`]), until a sync pass resolves each.
    pub unresolved_spends: Vec<UnresolvedSpend>,
    /// Spends whose result is not known and whose record the library lost
    /// (DEC-1057): one may still go through, and its notes read unspent.
    /// Each stays until a proved scan shows all its notes spent, or the
    /// wallet is removed; while one does, Close Wallet and a network close
    /// or switch are refused (`shielded_spend_unresolved`), and a send or
    /// unshield needs `confirm_unrecorded` (`shielded.unrecorded_spend`).
    pub unrecorded_spends: Vec<UnrecordedSpend>,
    /// Position in the pool's note tree the account is scanned up to
    /// (exclusive); `None` before a completed scan.
    pub synced_index: Option<u64>,
    /// Unix seconds of the last completed sync pass this session.
    pub last_sync_at: Option<u64>,
    /// A sync pass is running now.
    pub syncing: bool,
    /// The Orchard prover's key is built, so a spend proves at once (the
    /// engine builds it in the background when the session's Platform side
    /// starts on an active network).
    pub prover_ready: bool,
}

/// A shielded payment address of the wallet, for "Receive" and its QR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedAddress {
    /// Bech32m (`dash1z…` on mainnet, `tdash1z…` elsewhere;
    /// dpp `OrchardAddress::to_bech32m_string`).
    pub address: String,
    /// The Orchard account (ZIP-32 `account'`), 0.
    pub account: u32,
}

/// What moving `duffs` of the Core balance into the pool costs and buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldQuote {
    /// The asset lock's value.
    pub duffs: u64,
    /// The asset lock transaction's fee bound.
    pub fee_duffs: u64,
    /// `duffs + fee_duffs`: what leaves the Core balance at most.
    pub total_duffs: u64,
    /// Platform's flat fee for the shield, out of the lock's credits.
    pub pool_fee_credits: u64,
    /// What the shielded balance gains: the lock's credits less
    /// `pool_fee_credits`.
    pub credits: u64,
    /// `max_duffs` = `total_duffs`, `max_credits` 0.
    pub grant: GrantRequest,
}

/// Where a Core → Shielded move stands when the call returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShieldState {
    /// Platform took the lock: the credits are in the pool. `balance` is
    /// the shielded balance after.
    Applied { balance: u64 },
    /// The lock is out with no proof yet; `resume_shield` applies it.
    ProofWaiting,
    /// The lock has its proof and this apply was refused before its
    /// transition was signed, with `error` (a code): nothing of this
    /// attempt reached Platform (an earlier one of a `consumption_unknown`
    /// lock may have; the lock keeps that mark). `resume_shield` tries
    /// again.
    NotApplied { error: String },
    /// The apply may have executed, unproved: Platform answered that the
    /// lock is already consumed (any node can say it), or the apply failed
    /// after its transition was signed. The lock stays pending
    /// (`consumption_unknown`, "Checking") until a proved apply
    /// (`resume_shield`, "Check again"); it blocks no new shield.
    ConsumptionUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldOutcome {
    /// The asset lock's txid (hex, display order).
    pub txid: String,
    pub state: ShieldState,
}

/// A Core → Shielded asset lock not applied to the pool yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingShield {
    /// The asset lock's txid (hex, display order).
    pub txid: String,
    pub duffs: u64,
    /// The lock has its proof: `resume_shield` applies it at once.
    pub proved: bool,
    /// The lock may already be applied ("Checking"): Platform said so
    /// without proof, an apply failed after signing, or a restore read it
    /// back from the chain. Its amount may already be in the shielded
    /// balance, unconfirmed. `resume_shield` checks again; only a proved
    /// apply clears it. It blocks no new shield.
    pub consumption_unknown: bool,
}

/// What a spend of `credits` from the pool costs: a shielded send or an
/// unshield to Core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedSpendQuote {
    /// What the recipient gets, in credits.
    pub credits: u64,
    /// Platform's fee for the transition, out of the spent notes besides.
    pub fee_credits: u64,
    /// `credits + fee_credits`: what the shielded balance loses.
    pub total_credits: u64,
    /// For an unshield, what reaches the Core address, in duffs.
    pub expected_duffs: Option<u64>,
    /// `max_credits` = `total_credits`, `max_duffs` 0.
    pub grant: GrantRequest,
}

/// Where a shielded spend stands when the call returns (E0-04 Q15, ruling
/// (c) for 0.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedSpendState {
    /// Platform executed it.
    Done,
    /// It went out and the library could not confirm its result. It stays
    /// pending, never failed, until a sync pass resolves it: its history row
    /// (`activity_id`) turns `confirmed` when the scan sees its notes spent
    /// (done), or `failed` when the library's prune backstop releases them
    /// (not executed). Meanwhile the library re-sends the same signed
    /// transition (`transition`) during sync passes, at most 3 times. The
    /// facade offers no resend and the host offers no retry.
    Pending,
}

/// A spend the library could not confirm, with the signed transition it
/// holds to re-send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedSpend {
    /// The spend's history row ([`ShieldedActivity::id`]).
    pub activity_id: String,
    /// Platform's hash of the signed state transition (SHA-256 of its
    /// bytes, hex): what each re-send carries, byte for byte.
    pub transition: String,
    /// Re-sends so far.
    pub attempts: u32,
}

/// What a `shielded.not_synced` waits for (DEC-1058 #4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedSyncWait {
    /// The account is not established: its viewing key is not bound (a
    /// first bind needs the vault unlocked) or its notes are neither
    /// restored nor scanned (`ShieldedStatus::ready` false).
    Account,
    /// The note tree is not on an anchor Platform recorded yet
    /// (platform-wallet's `ShieldedNoRecordedAnchor`, "wait for the next
    /// shielded sync"): the next sync pass moves it.
    Tree,
    /// The dispatch journal, where the shield and spend records are kept,
    /// is not read. A session reads it before it opens, so a host sees this
    /// only from a closing one; fail-closed, never "Platform did not answer".
    Journal,
    /// SPV runs and is not caught up, or Platform writes wait for
    /// masternode sync: a shield's asset lock waits (as a top-up's does).
    Core,
    /// The wallet's DashPay bring-up is running: a shield's funding waits
    /// for it (as a top-up's does).
    Startup,
}

impl ShieldedSyncWait {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Tree => "tree",
            Self::Journal => "journal",
            Self::Core => "core",
            Self::Startup => "startup",
        }
    }
}

impl std::fmt::Display for ShieldedSyncWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A spend of unknown result whose record the library lost (DEC-1057),
/// as the engine's marker keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnrecordedSpend {
    /// The spend's history row ([`ShieldedActivity::id`]), hex.
    pub activity_id: String,
    /// Unix seconds of the spend.
    pub since: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedSpendOutcome {
    pub credits: u64,
    /// The shielded balance after (while `pending`, less the notes the
    /// spend holds).
    pub balance: u64,
    /// For an unshield, the Core address paid.
    pub to: String,
    pub state: ShieldedSpendState,
    /// For a `pending` spend, the library's record of it, when it kept one.
    pub unresolved: Option<UnresolvedSpend>,
}

/// The kind of a shielded activity row (platform-wallet's
/// `ShieldedActivityKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedActivityKind {
    /// From Platform addresses into the pool.
    Shield,
    /// From Core into the pool (an asset lock).
    ShieldFromCore,
    /// Notes received from someone else (or a restored shield: the scan
    /// cannot tell them apart).
    Received,
    /// To another shielded address.
    Sent,
    /// To a Platform address.
    Unshield,
    /// To a Core address.
    Withdrawal,
    /// Funding a new identity.
    IdentityCreate,
    /// Topping up an identity.
    IdentityTopUp,
    /// A spend whose kind a restore could not tell.
    Spend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedDirection {
    In,
    Out,
    /// Between the wallet's own notes.
    SelfTransfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedActivityStatus {
    Pending,
    Confirmed,
    Failed,
    /// Recovery stopped without proof either way; a later scan may still
    /// confirm it.
    Unknown,
}

/// One row of the wallet's shielded history, for Activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedActivity {
    /// The row's id (hex), stable across rescans.
    pub id: String,
    pub kind: ShieldedActivityKind,
    pub direction: ShieldedDirection,
    /// The principal in credits: what the counterparty got, or what the
    /// wallet received; change is not counted.
    pub credits: u64,
    pub fee_credits: Option<u64>,
    /// The other side when the wallet knows it: a shielded address for a
    /// send, a Platform address for an unshield, a Core address for a
    /// withdrawal.
    pub counterparty: Option<String>,
    /// The memo's text, when it has one that reads as UTF-8.
    pub memo: Option<String>,
    /// Platform block height it confirmed at; `None` while pending and for
    /// rows a scan restored (the pool's proofs carry no note height).
    pub block_height: Option<u64>,
    pub status: ShieldedActivityStatus,
    /// Unix seconds when this device recorded it; `None` for restored rows.
    pub created_at: Option<u64>,
}

impl DashPay {
    /// The account's status (`ShieldedStatus`). Reads local state and the
    /// SDK's proved protocol version; an inactive network is a status
    /// (`active` false), not an error.
    pub async fn shielded_status(&self) -> Result<ShieldedStatus, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            pool::marked_status(&session, wallet, pool::status(&lib).await?).await
        })
        .await
    }

    /// Runs a sync pass of the wallet's shielded notes now and returns the
    /// status after.
    pub async fn shielded_sync(&self) -> Result<ShieldedStatus, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            let status = pool::sync(&session, &lib).await?;
            pool::marked_status(&session, wallet, status).await
        })
        .await
    }

    /// The wallet's shielded receive address.
    pub async fn shielded_receive_address(&self) -> Result<ShieldedAddress, ShieldedError> {
        on_pool(&self.session, self.wallet_id, |_, lib| async move {
            pool::receive_address(&lib).await
        })
        .await
    }

    /// What moving `duffs` of the Core balance into the pool costs and
    /// buys, with the refusals of [`Self::shield_from_core`].
    pub async fn shield_quote(&self, duffs: u64) -> Result<ShieldQuote, ShieldedError> {
        on_pool(
            &self.session,
            self.wallet_id,
            move |session, lib| async move { pool::shield_quote(&session, &lib, duffs).await },
        )
        .await
    }

    /// Core → Shielded: an asset lock of `duffs` from the Core balance under
    /// a `Shield` lease begun from `grant`, then Platform's
    /// ShieldFromAssetLock of it to the wallet's own shielded address.
    pub async fn shield_from_core(
        &self,
        duffs: u64,
        grant: String,
    ) -> Result<ShieldOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            pool::shield(&session, wallet, &lib, duffs, &grant).await
        })
        .await
    }

    /// Applies the pending Core → Shielded lock `txid` (`pending_shields`)
    /// again, its consumption known or not ("Check again"): the same
    /// outpoint, with any proof the library derives for it, never a new
    /// lock, new funding or other coins; only a proved apply settles it.
    pub async fn resume_shield(
        &self,
        txid: String,
        grant: String,
    ) -> Result<ShieldOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            pool::resume(&session, wallet, &lib, &txid, &grant).await
        })
        .await
    }

    /// What sending `credits` to `to` costs, with the refusals of
    /// [`Self::shielded_send`].
    pub async fn shielded_send_quote(
        &self,
        to: String,
        credits: u64,
    ) -> Result<ShieldedSpendQuote, ShieldedError> {
        on_pool(&self.session, self.wallet_id, move |_, lib| async move {
            pool::send_quote(&lib, &to, credits).await
        })
        .await
    }

    /// Shielded → Shielded: sends `credits` from the wallet's notes to the
    /// shielded address `to`, with an optional memo of at most 36 bytes,
    /// under a `ShieldedSpend` lease begun from `grant`. While the wallet
    /// has an [`UnrecordedSpend`] it runs only with `confirm_unrecorded`
    /// (the user confirmed that payment may still go through), else
    /// `shielded.unrecorded_spend` before anything is selected or signed.
    pub async fn shielded_send(
        &self,
        to: String,
        credits: u64,
        memo: Option<String>,
        grant: String,
        confirm_unrecorded: bool,
    ) -> Result<ShieldedSpendOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            let what = (to.as_str(), credits);
            let (memo, confirmed) = (memo.as_deref(), confirm_unrecorded);
            pool::send(&session, wallet, &lib, what, memo, &grant, confirmed).await
        })
        .await
    }

    /// What moving `credits` back to Core costs, with the refusals of
    /// [`Self::unshield_to_core`].
    pub async fn unshield_quote(
        &self,
        credits: u64,
        to: Option<String>,
    ) -> Result<ShieldedSpendQuote, ShieldedError> {
        on_pool(&self.session, self.wallet_id, move |_, lib| async move {
            pool::unshield_quote(&lib, credits, to.as_deref()).await
        })
        .await
    }

    /// Shielded → Core: withdraws `credits` from the wallet's notes to the
    /// Core address `to`, or to the wallet's next receive address when
    /// `None`, under a `ShieldedSpend` lease begun from `grant`;
    /// `confirm_unrecorded` as for [`Self::shielded_send`].
    pub async fn unshield_to_core(
        &self,
        credits: u64,
        to: Option<String>,
        grant: String,
        confirm_unrecorded: bool,
    ) -> Result<ShieldedSpendOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            let (to, confirmed) = (to.as_deref(), confirm_unrecorded);
            pool::unshield(&session, wallet, &lib, credits, to, &grant, confirmed).await
        })
        .await
    }

    /// The wallet's shielded history for Activity, pending rows first, then
    /// newest first; `limit` at most 500.
    pub async fn shielded_history(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ShieldedActivity>, ShieldedError> {
        on_pool(&self.session, self.wallet_id, move |_, lib| async move {
            pool::history(&lib, offset, limit).await
        })
        .await
    }
}
