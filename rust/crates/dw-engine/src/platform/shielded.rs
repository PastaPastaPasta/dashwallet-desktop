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

use super::credits::{CoreMax, FundAmount};
use super::dashpay::DashPay;
use super::errors::ShieldedError;
use super::flows::{DispatchState, GrantRequest};
use super::payments::Counterparty;
use super::shielded_pool::{self as pool, on_pool, on_pool_signing};
use crate::EngineError;

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
    /// When the pool was last scanned, or last proved unchanged with nothing
    /// to scan. Unix seconds, this session.
    pub last_sync_at: Option<u64>,
    /// A sync pass is running now.
    pub syncing: bool,
    /// The Orchard prover's key is built, so a spend proves at once (the
    /// engine builds it in the background when the session's Platform side
    /// starts on an active network).
    pub prover_ready: bool,
    /// Whether the account's viewing key is bound, or what holds its first
    /// bind; `None` while the network is not active. Once bound the receive
    /// address reads before any scan ([`ShieldedAddress`]).
    pub account: Option<ShieldedAccount>,
    /// How far the running pass has scanned the pool's note tree; `None`
    /// while no pass runs or before its first batch proves the pool's size.
    pub scan: Option<ShieldedScanProgress>,
    /// The wallet's last sync pass this session failed; cleared by the next
    /// one that succeeds (passes run again by themselves).
    pub last_pass_failed: bool,
}

/// Where the account's viewing key stands. Its first bind reads the key
/// from the vault; a restart binds it again from the stored key, locked or
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedAccount {
    /// The key is bound: the address reads, and passes scan for it.
    Bound,
    /// Not bound yet, for no reason the user can act on: a pass held the
    /// bind back. A later read binds it.
    Binding,
    /// The first bind needs the vault unlocked.
    Locked,
    /// The first bind needs the vault unlocked fully, not for mixing only.
    MixingOnly,
    /// A watch-only wallet: the vault holds no secret to derive the key.
    WatchOnly,
}

impl ShieldedAccount {
    pub fn is_bound(self) -> bool {
        self == Self::Bound
    }
}

/// The running pass's progress through the pool's note tree, network-wide:
/// one pass scans for every bound wallet of the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedScanProgress {
    /// Tree positions scanned, the ones before the pass included.
    pub checked: u64,
    /// The pool's notes, as the pass's latest batch proved them.
    pub total: u64,
}

/// Where the wallet's Shielded sync stands, for the status bar, Sync
/// progress and Receive: read from the engine's own state, with no read of
/// the note store and no bind, so it answers while a pass holds the store
/// (a first scan holds it to the end).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedSyncInfo {
    /// As [`ShieldedStatus::active`], and the session runs with Platform:
    /// without it no pass runs.
    pub active: bool,
    /// As [`ShieldedStatus::account`]; `Binding` for a key a read of the
    /// status binds next. `None` while not active.
    pub account: Option<ShieldedAccount>,
    /// The account's ledger is established: a pass scanned it this session,
    /// or a status read found it ready.
    pub established: bool,
    /// A pass is running.
    pub syncing: bool,
    pub scan: Option<ShieldedScanProgress>,
    /// The wallet's last pass this session failed, until one succeeds.
    pub last_pass_failed: bool,
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
    /// A Core Max's (`FundAmount::All`): `total_duffs` is then the coins'
    /// sum and `duffs` the lock's value at least, the fee out of it.
    pub max: Option<CoreMax>,
}

/// What moving CoinJoin account 0's mixed coins into the pool ("Move mixed
/// coins" to Shielded) takes and buys: one asset lock
/// spending every coin of the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixedShieldQuote {
    /// The coins the lock spends.
    pub inputs: u32,
    /// Their sum: what leaves the CoinJoin account, and the `Spend`
    /// grant's `max_duffs`.
    pub total_duffs: u64,
    /// The lock transaction's fee bound, out of `total_duffs`.
    pub fee_duffs: u64,
    /// The lock's value, at least: `total_duffs - fee_duffs`.
    pub duffs: u64,
    /// Platform's flat fee for the shield, out of the lock's credits.
    pub pool_fee_credits: u64,
    /// What the shielded balance gains, at least.
    pub credits: u64,
    /// Of `total_duffs`, the fully mixed coins' and the others'.
    pub fully_mixed_duffs: u64,
    pub not_fully_mixed_duffs: u64,
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

/// A shield's or a drain's funding whose outcome is unknown: its step
/// reads `MaybeSent` or `WillBeSent`, so it refuses every later shield and
/// drain (`platform.broadcast_unknown`, `platform.will_be_sent`) until it
/// settles or Repair resolves it (`resolve_stuck_funding` with `step`).
#[derive(Debug)]
pub struct StuckShieldFunding {
    /// The funding step, `shield/<k>/funding`.
    pub step: String,
    /// `MaybeSent` or `WillBeSent`.
    pub state: DispatchState,
    /// What Repair's self-spend for the step would spend now, priced
    /// without signing or reserving anything; or why Repair refuses the
    /// step now (its `resolve_stuck_funding` refusal).
    pub repair: Result<ShieldRepairCost, EngineError>,
}

/// Repair's self-spend of a stuck shield funding: the coins it would send
/// back to the wallet and its fee, as `resolve_stuck_funding` would build it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldRepairCost {
    /// The lock drained the CoinJoin account: the self-spend takes only
    /// the CoinJoin coins its records name, never the Core balance.
    pub mixed: bool,
    pub inputs: u32,
    /// The inputs' sum; the wallet gets it back less `fee_duffs`.
    pub total_duffs: u64,
    pub fee_duffs: u64,
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
    /// facade offers no resend and the host offers no retry. A spend the
    /// library resolves without attributing its notes' spend to it
    /// (`Unattributable`) while its history row reads `confirmed` is done
    /// too: never failed, and never retried, rebuilt or re-sent.
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
    /// restored nor scanned (`ShieldedStatus::ready` false). For now also a
    /// first bind the vault cannot serve (locked, mixing-only, watch-only),
    /// which does not clear by itself: the user unlocks, or (watch-only) it
    /// is not supported. Its own refusal comes in 0.1.x.
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
    /// The shielded balance after. While `pending`, the balance before
    /// less `credits` and the fee: the change is not a note until a pass
    /// finds it.
    pub balance: u64,
    /// For an unshield, the Core address paid.
    pub to: String,
    pub state: ShieldedSpendState,
    /// For a `pending` spend, the library's record of it, when it kept one.
    pub unresolved: Option<UnresolvedSpend>,
}

/// What `unshield_to_platform` did: the spend's outcome, and the Platform
/// activity row of its `FromShielded` record when the dispatch hook armed
/// one (C-routes C1-9). That row is `done` only once the spend's artifact
/// reads executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnshieldToPlatformOutcome {
    pub credits: u64,
    /// The shielded balance after.
    pub balance: u64,
    /// The Platform address paid.
    pub to: String,
    pub state: ShieldedSpendState,
    pub unresolved: Option<UnresolvedSpend>,
    /// The `id` of the record's `PlatformActivity` row (`platform_activity`).
    pub record: Option<String>,
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
    /// Unix seconds when this device recorded it, or for a row a scan
    /// derived (another wallet's payment), when the engine first saw its
    /// notes after the wallet's first scan; `None` for a row of that first
    /// scan (restored) and for one a scan derived that is not incoming.
    pub created_at: Option<u64>,
    /// The DashPay contact a withdrawal paid (C-funding P10): from the
    /// contact payout's row, else by the destination address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<Counterparty>,
}

impl DashPay {
    /// The account's status (`ShieldedStatus`). Reads local state and the
    /// SDK's proved protocol version; an inactive network is a status
    /// (`active` false), not an error.
    pub async fn shielded_status(&self) -> Result<ShieldedStatus, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            let status = pool::status(&lib).await?;
            if status.ready {
                session.hub.note_shielded_ready(wallet);
            }
            pool::marked_status(&session, wallet, status).await
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

    /// Where the wallet's Shielded sync stands ([`ShieldedSyncInfo`]), with
    /// no read of the note store.
    pub async fn shielded_sync_info(&self) -> Result<ShieldedSyncInfo, ShieldedError> {
        let wallet = self.wallet_id;
        pool::on_wallet(&self.session, wallet, |_, pw| async move {
            Ok(pool::sync_info(&pw).await)
        })
        .await
    }

    /// The wallet's shielded receive address. A bound key's address reads
    /// with no read of the note store, so it answers during a pass; the
    /// first bind goes through the pool.
    pub async fn shielded_receive_address(&self) -> Result<ShieldedAddress, ShieldedError> {
        let wallet = self.wallet_id;
        let bound = pool::on_wallet(&self.session, wallet, |_, pw| async move {
            Ok(pool::bound_address(&pw).await)
        })
        .await?;
        if let Some(address) = bound {
            return Ok(address);
        }
        on_pool(&self.session, wallet, |_, lib| async move {
            pool::receive_address(&lib).await
        })
        .await
    }

    /// What moving `amount` of the Core balance into the pool costs and
    /// buys, with the refusals of [`Self::shield_from_core`].
    pub async fn shield_quote(&self, amount: FundAmount) -> Result<ShieldQuote, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            match amount {
                FundAmount::Duffs { duffs } => pool::shield_quote(&session, &lib, duffs).await,
                FundAmount::All => pool::shield_max_quote(&session, wallet, &lib).await,
            }
        })
        .await
    }

    /// Core → Shielded: an asset lock of `amount` from the Core balance
    /// under a `Shield` lease begun from `grant`, then Platform's
    /// ShieldFromAssetLock of it to the wallet's own shielded address.
    /// `All` drains BIP44 account 0 (`core_max.rs`).
    pub async fn shield_from_core(
        &self,
        amount: FundAmount,
        grant: String,
    ) -> Result<ShieldOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            pool::shield(&session, wallet, &lib, amount, &grant).await
        })
        .await
    }

    /// What "Move mixed coins" to Shielded would move: CoinJoin account
    /// 0's coins in one asset lock. Reads coins only.
    pub async fn shield_mixed_quote(&self) -> Result<MixedShieldQuote, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            super::shield_drain::quote(&session, wallet, &lib).await
        })
        .await
    }

    /// The wallet's shield and drain fundings whose outcome is unknown, in
    /// step order, each with what Repair would spend for it. Reads the
    /// dispatch journal and the wallet's coins only: no pool, no network.
    /// `platform.unavailable` until the journal and the tracked rows are
    /// read.
    pub async fn stuck_shield_fundings(&self) -> Result<Vec<StuckShieldFunding>, ShieldedError> {
        let (session, wallet) = (std::sync::Arc::clone(&self.session), self.wallet_id);
        let read = std::sync::Arc::clone(&session);
        let steps = session
            .on_runtime(async move {
                read.wallet(&wallet).await?;
                let ready =
                    read.leases.journal_ready() && read.leases.refresh_funding(wallet).await;
                Ok(ready
                    .then(|| read.leases.stuck_shield_steps(wallet).ok())
                    .flatten())
            })
            .await
            .map_err(super::PlatformError::from)?
            .ok_or(super::PlatformError::Unavailable)?;
        let mut out = Vec::with_capacity(steps.len());
        for (step, state) in steps {
            let repair = repair_cost(&session, wallet, &step).await;
            out.push(StuckShieldFunding {
                step,
                state,
                repair,
            });
        }
        Ok(out)
    }

    /// "Move mixed coins" to Shielded: one asset lock spending every coin
    /// of CoinJoin account 0 under a `ShieldDrain` lease begun from
    /// `grant` (a `Spend` grant of at least the quote's `total_duffs`),
    /// then Platform's ShieldFromAssetLock of it to the wallet's own
    /// shielded address. The lock is a shield's: `pending_shields` lists
    /// it and `resume_shield` applies it.
    pub async fn shield_mixed_coins(&self, grant: String) -> Result<ShieldOutcome, ShieldedError> {
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            super::shield_drain::drain(&session, wallet, &lib, &grant).await
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

    /// What moving `credits` from Shielded to the wallet's own Platform
    /// address `to` costs (its receive address when `None`), with the
    /// refusals of [`Self::unshield_to_platform`].
    pub async fn unshield_to_platform_quote(
        &self,
        credits: u64,
        to: Option<String>,
    ) -> Result<ShieldedSpendQuote, ShieldedError> {
        pool::refuse_on_mainnet(self.session.network().core_network())?;
        on_pool(&self.session, self.wallet_id, move |_, lib| async move {
            pool::unshield_to_platform_quote(&lib, credits, to.as_deref()).await
        })
        .await
    }

    /// Shielded → Platform: unshields `credits` from the wallet's notes to
    /// `to`, one of the wallet's own Platform addresses (its receive address
    /// when `None`; any other is `shielded.invalid_address` before the grant
    /// is redeemed), under a `ShieldedSpend` lease begun from `grant`;
    /// `confirm_unrecorded` as for [`Self::shielded_send`].
    pub async fn unshield_to_platform(
        &self,
        credits: u64,
        to: Option<String>,
        grant: String,
        confirm_unrecorded: bool,
    ) -> Result<UnshieldToPlatformOutcome, ShieldedError> {
        pool::refuse_on_mainnet(self.session.network().core_network())?;
        let wallet = self.wallet_id;
        on_pool_signing(&self.session, wallet, move |session, lib| async move {
            let (to, confirmed) = (to.as_deref(), confirm_unrecorded);
            pool::unshield_to_platform(&session, wallet, &lib, credits, to, &grant, confirmed).await
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
        let wallet = self.wallet_id;
        on_pool(&self.session, wallet, move |session, lib| async move {
            let mut rows = pool::history(&lib, offset, limit).await?;
            session.name_shielded_payouts(wallet, &mut rows).await;
            Ok(rows)
        })
        .await
    }
}

/// [`ShieldRepairCost`] of `step`: Repair's own draft, estimated.
async fn repair_cost(
    session: &std::sync::Arc<crate::NetworkSession>,
    wallet: crate::WalletId,
    step: &str,
) -> Result<ShieldRepairCost, EngineError> {
    let draft = session.stuck_funding_draft(wallet, step).await?;
    let estimate = draft.estimate().await?;
    Ok(ShieldRepairCost {
        mixed: draft.spends_coinjoin(),
        inputs: estimate.input_count,
        total_duffs: estimate.total_sent + estimate.fee,
        fee_duffs: estimate.fee,
    })
}
