//! Shielded funds (0.1 E1; m4-dashpay-engine §2.13, §7 item 72) over
//! platform-wallet's `wallet::shielded`, behind the facade calls of
//! `shielded.rs`. Not part of the facade.
//!
//! - **Keys.** The wallet's Orchard account 0 (`dw_vault::SHIELDED_ACCOUNT`,
//!   platform-wallet's ZIP-32 `m_orchard/32'/coin'/0'`). Its full viewing
//!   key is bound to the library's network coordinator (configured over
//!   `shielded.sqlite` in the network directory) the first time a call or
//!   the Platform start finds the wallet unbound: from the row the library
//!   persisted, else from the vault ([`dw_vault::Vault::shielded_viewing_key`],
//!   prompt-free only), which is persisted through the session's store
//!   first so a restart rebinds without the vault. The spend authority is
//!   derived per spend under a `ShieldedSpend` lease
//!   ([`CallPermit::shielded_keys`]).
//! - **The gate.** Every call but the status reads the network's protocol
//!   version (the SDK's, refreshed by one proved read only while it is
//!   below [`SHIELDED_ACTIVATION_VERSION`]) and refuses below it.
//! - **Spends.** A shielded send (`operations::transfer`) and an unshield
//!   (`operations::withdraw`, Core fee rate 1) run one at a time per wallet
//!   under the wallet's shielded lock, charged their quote (credits plus
//!   Platform's fee for the notes they take) from the lease's Credits
//!   budget; the key set's release counts as the permit's signature, so
//!   once it is out the quote is kept whatever the library returns. A
//!   broadcast whose execution the library could not confirm is
//!   pending, never failed (E0-04 Q15, ruling (c) for 0.1): the library
//!   keeps its notes reserved, its activity row pending and its signed
//!   transition (`shielded_pending_spends` in `shielded.sqlite`), which the
//!   coordinator's sync pass re-sends byte for byte, at most 3 times
//!   (`redrive_pending_spends`), until the scan sees the notes spent (the
//!   row confirms: done) or the prune backstop releases them (the row
//!   fails: not executed), or Platform answers a re-send with a definitive
//!   rejection (the library releases them and fails the row,
//!   `operations.rs:3286-3310`). The spend's outcome and the status name that
//!   transition by Platform's hash of it, and the dispatch journal records
//!   it against the activity; nothing here resends. Every pass goes through
//!   [`guarded_pass`]: admitted only while the session is open (the
//!   supervisor's kick, or [`on_pool`]), only while each wallet holding such
//!   a spend holds its background lease, and only once each is on record
//!   (a spend holds the passes off from its library call until its record
//!   is written, so none can find a spend stored mid-pass). The library's
//!   own loop never runs. A pass admitted before a lock or a close may
//!   still run when the leases drop, as a leased call admitted before a
//!   freeze finishes in its drain: the lock, async or synchronous (the
//!   relock timer's, the console's), a revocation that drops background
//!   leases (a vault operation's, a wallet's removal or unload) and the
//!   close wait for its end ([`drain_passes`], which states the bound), so
//!   no redrive runs after a lock completes or a close returns. A lock
//!   completes at its `LockProgress::Done`, which the async lock's return
//!   also waits for; the synchronous lock (`lock_vault_sync`: dw-ffi's,
//!   the console's, the relock timer's) returns after its own vault gate,
//!   before the drain, so a pass admitted before it may still re-send
//!   until `Done`. The close then seals the coordinator
//!   (`Manager::shutdown`).
//!   0.2: the library admits each send (`q15-hook`).
//! - **Shields.** Core → Shielded is a top-up's two steps (`topup.rs`): an
//!   asset lock of the Core balance (funding type
//!   `AssetLockShieldedAddressTopUp`, step `shield/<k>/funding`, held at
//!   most once by the dispatch journal as a top-up's), then the library's
//!   `shielded_fund_from_asset_lock` of the proved lock to the account's own
//!   address, signed with the lock's credit key by the lease's uncounted
//!   funding signer. A lock not applied pends (`pending_shields`) and
//!   blocks a new shield until `resume_shield` applies it. An apply whose
//!   transition was signed and then failed may have executed, and
//!   Platform's "already consumed" proves nothing (any node can say it):
//!   such a lock, and one of ours a restore read back from the chain, reads
//!   `consumption_unknown` ("Checking", recorded in `app.sqlite`) and pends
//!   until a proved apply (`resume_shield` again: the same outpoint, with
//!   any proof the library derives for it, never a new lock; see [`resume`]
//!   for those proofs and why Platform executes it at most once),
//!   but blocks no new shield, since it may never settle (DEC-1045, SR1);
//!   only a refusal before the signature is `NotApplied`. A recovered lock
//!   no shield step signed is history.
//!   Until the dispatch journal is read none of this can be told, so the
//!   shield calls answer `platform.unavailable`. The lock spends at most
//!   [`MAX_LOCK_INPUTS`] coins. Nothing stops a shield partway: the call
//!   runs in [`on_pool`]'s task, which the close waits for and never
//!   aborts, under no engine timeout; the library's own waits are bounded
//!   by its arguments.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, Weak};
use std::time::Duration;

use dashcore::{Address, Network, OutPoint, Txid};
use dpp::address_funds::{OrchardAddress, PlatformAddress};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::shielded::compute_minimum_shielded_fee;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::TopUpSettled;
use dw_vault::{SHIELDED_ACCOUNT, VaultError, VaultSigner};
use platform_wallet::changeset::{
    PlatformWalletChangeSet, PlatformWalletPersistence, ShieldedChangeSet,
};
use platform_wallet::wallet::asset_lock::AssetLockFunding;
use platform_wallet::wallet::shielded::note_selection::{ShieldedFeeKind, select_notes_with_fee};
use platform_wallet::wallet::shielded::store::StalePendingSpend;
use platform_wallet::wallet::shielded::{
    CachedOrchardProver, NetworkShieldedCoordinator, ShieldedActivityEntry,
    ShieldedActivityKind as Kind, ShieldedActivityStatus as Status, ShieldedDirection as Direction,
    ShieldedLocalBalanceState, ShieldedNote, ShieldedStore, SubwalletId, operations,
};
use platform_wallet::{AssetLockFundingType, PlatformWalletError};
use sha2::{Digest, Sha256};

use super::errors::{PlatformError, ShieldedError, ShieldedSyncWait as Wait};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest};
use super::lease::{CallPermit, FundingKey, Lease, LeaseError};
use super::registration::{RegistrationWait, funding_cap, grant_cap};
use super::registration_flow::{
    FundError, LockInfo, LockStatus, PwLib, RegistrationLib as Reg, Spendable,
};
use super::requests::{leased, write_permit};
use super::shielded::{
    PendingShield, SHIELDED_ACTIVATION_VERSION, ShieldOutcome, ShieldQuote, ShieldState,
    ShieldedActivity, ShieldedActivityKind, ShieldedActivityStatus, ShieldedAddress,
    ShieldedDirection, ShieldedSpendOutcome, ShieldedSpendQuote, ShieldedSpendState,
    ShieldedStatus, UnrecordedSpend, UnresolvedSpend,
};
use super::withdrawal::{Rules, expected_duffs};
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// The coordinator's commitment tree, in the network directory.
pub(crate) const SHIELDED_DB_FILE: &str = "shielded.sqlite";
/// The most rows one history page returns.
const MAX_HISTORY: u32 = 500;
/// Orchard's memo field.
const MEMO_BYTES: usize = 36;
/// The fewest actions a spend's bundle carries (Orchard pads a one-note
/// spend to two), which the library's selection and fee assume.
const MIN_ACTIONS: usize = 2;
/// The Core fee rate an unshield asks for, Platform's minimum.
const CORE_FEE_PER_BYTE: u32 =
    dpp::state_transition::identity_credit_withdrawal_transition::MIN_CORE_FEE_PER_BYTE;
/// How long `resume_shield` waits for a lock's proof.
const RESUME_PROOF_WAIT: Duration = Duration::from_secs(30);
/// The most coins a shield's asset lock may spend. Its InstantSend proof
/// carries the whole transaction, which stops fitting a transition from
/// about 65 inputs, and a fresh lock's first apply carries its InstantSend
/// proof (a ChainLock one only once the lock is chain-locked locally, its
/// InstantSend wait timed out, or Platform refused the InstantSend
/// signature), so a bigger lock would stall (0.2 design review X1b-2).
pub(crate) const MAX_LOCK_INPUTS: usize = 50;
/// How long an apply waits for a ChainLock proof when Platform refuses the
/// lock's InstantSend one (the library's `cl_wait`, `None` being for ever):
/// past it the library answers a timeout, nothing is dropped mid-call, and
/// the lock pends for `resume_shield`.
const APPLY_CHAIN_LOCK_WAIT: Duration = Duration::from_secs(300);

/// The account's ledger as the library holds it locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ledger {
    /// Unspent credits less the notes pending spends reserve.
    pub(crate) spendable: u64,
    /// The note-tree position scanned to (exclusive), once a scan completed.
    pub(crate) synced_index: Option<u64>,
}

/// The library's shielded sync loop, as the status reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SyncState {
    pub(crate) syncing: bool,
    pub(crate) last_sync_at: Option<u64>,
}

/// platform-wallet's shielded side as these flows use it; [`PwShielded`] is
/// the real one, `shielded_tests.rs` scripts a fake.
pub(crate) trait ShieldedLib: Send + Sync {
    fn network(&self) -> Network;
    /// The network's protocol version as the SDK last proved it; no read.
    fn protocol_version(&self) -> u32;
    /// The version after one proved read of Platform's (the one held when
    /// the read fails).
    fn refresh_protocol_version(&self) -> impl Future<Output = u32> + Send;
    /// Binds the account's viewing key when it is not bound (module doc):
    /// `false` while neither a persisted key nor a prompt-free vault has it.
    fn bind(&self) -> impl Future<Output = Result<bool, PlatformError>> + Send;
    /// The account's default payment address, once bound.
    fn address(&self) -> impl Future<Output = Option<[u8; 43]>> + Send;
    /// `None` until the ledger is established (bound and restored or
    /// scanned).
    fn ledger(&self) -> impl Future<Output = Result<Option<Ledger>, PlatformError>> + Send;
    /// The notes a spend may select (unspent, not reserved).
    fn unspent(&self) -> impl Future<Output = Result<Vec<ShieldedNote>, PlatformError>> + Send;
    /// A page of the account's activity, sorted for display.
    fn activity(
        &self,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ShieldedActivityEntry>, PlatformError>> + Send;
    /// A sync pass of every bound wallet now, past the caught-up cooldown,
    /// through [`guarded_pass`]; `false` when it was held back.
    fn sync_now(&self) -> impl Future<Output = bool> + Send;
    fn sync_state(&self) -> SyncState;
    fn prover_ready(&self) -> bool;
    fn transfer(
        &self,
        permit: &CallPermit,
        to: &OrchardAddress,
        credits: u64,
        memo: [u8; MEMO_BYTES],
    ) -> impl Future<Output = Result<(), SpendError>> + Send;
    fn withdraw(
        &self,
        permit: &CallPermit,
        to: &Address,
        credits: u64,
    ) -> impl Future<Output = Result<(), SpendError>> + Send;
    /// A fresh Core receive address of the wallet, for an unshield to self.
    fn receive_address(&self) -> impl Future<Output = Result<String, PlatformError>> + Send;

    /// The wallet's tracked `AssetLockShieldedAddressTopUp` locks.
    fn shield_locks(&self) -> impl Future<Output = Vec<LockInfo>> + Send;
    /// [`Self::shield_locks`] with what a finish of one reads of its row
    /// and transaction (`unfinished_locks::KindLock`).
    fn shield_lock_rows(
        &self,
    ) -> impl Future<Output = Vec<super::unfinished_locks::KindLock>> + Send;
    /// The locks the wallet's shield steps signed (the dispatch journal's
    /// records); `None` until the journal is read.
    fn own_locks(&self) -> Option<HashSet<Txid>>;
    /// The shield locks whose apply may have executed, unproved: Platform
    /// answered "already consumed", or an apply failed after the lock's
    /// credit key signed its transition ([`Applied::Unknown`]). Kept in
    /// `app.sqlite` across restarts.
    fn unknown_locks(&self) -> impl Future<Output = HashSet<Txid>> + Send;
    /// Records `txid` among [`Self::unknown_locks`].
    fn mark_unknown(&self, txid: Txid) -> impl Future<Output = ()> + Send;
    /// The wallet's locks the dispatch journal records settled; none when
    /// they cannot be read (the locks then pend, fail closed).
    fn settled_locks(&self) -> impl Future<Output = HashSet<Txid>> + Send;
    /// Records the shield lock `txid` settled by a proved apply.
    fn settle(&self, txid: Txid) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// Records `spend` in the dispatch journal against its activity
    /// (schema 9); the first record stands.
    fn record(
        &self,
        spend: &UnresolvedSpend,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// The account's spends the library could not confirm and keeps to
    /// re-send.
    fn unresolved(
        &self,
    ) -> impl Future<Output = Result<Vec<UnresolvedSpend>, PlatformError>> + Send;
    /// The account's notes reserved by a spend built on a recorded anchor,
    /// as the library holds them in memory: `(nullifier, anchor,
    /// activity_id)` (`stale_pending_spends`, store.rs:286-293). A spend
    /// reserves them before its broadcast (operations.rs:1840, :2009,
    /// :1661 → :2890-2910) and keeps them on an unknown result.
    fn reservations(
        &self,
    ) -> impl Future<Output = Result<Vec<StalePendingSpend>, PlatformError>> + Send;
    /// The values of the coins a funding may select (those
    /// [`Spendable::duffs`] counts).
    fn coin_values(&self) -> impl Future<Output = Result<Vec<u64>, PlatformError>> + Send;
    fn spendable(&self) -> impl Future<Output = Result<Spendable, PlatformError>> + Send;
    /// E0-04's `fee_bound_worst`: the lock's fee quote.
    fn fee_bound(&self) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// What holds a funding back (SPV behind, the bring-up not answered).
    fn funding_hold(&self) -> Option<RegistrationWait>;
    /// Whether the wallet's DashPay bring-up is running (not run yet, or
    /// starting): a `Startup` hold that clears on its own.
    fn startup_running(&self) -> bool {
        false
    }
    /// Whether SPV runs: a `Sync` hold of a stopped SPV does not clear on
    /// its own.
    fn spv_running(&self) -> bool {
        true
    }
    fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> impl Future<Output = Result<CallPermit, FundError>> + Send;
    /// Builds, tracks and broadcasts the lock with `permit`'s counting
    /// funding signer, and waits for its InstantSend lock.
    fn fund(
        &self,
        duffs: u64,
        permit: &CallPermit,
    ) -> impl Future<Output = Result<OutPoint, FundError>> + Send;
    fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), FundError>> + Send;
    /// ShieldFromAssetLock of the proved lock at `outpoint` to the account's
    /// own address, its proof signed with the lock's credit key.
    fn apply<S>(
        &self,
        outpoint: OutPoint,
        signer: &S,
    ) -> impl Future<Output = Result<Applied, PlatformError>> + Send
    where
        S: key_wallet::ExtendedPubKeySigner
            + key_wallet::Signer<Error = dw_vault::SignerError>
            + Send
            + Sync;
    /// The script of the tracked lock's credit output at `outpoint`.
    fn credit_script(
        &self,
        _outpoint: OutPoint,
    ) -> impl Future<Output = Option<dashcore::ScriptBuf>> + Send {
        async { None }
    }
}

/// How an apply ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Platform's proved execution.
    Done,
    /// Platform answered that the lock is already consumed: unproved.
    Taken,
    /// The apply failed after the lock's credit key signed its transition:
    /// it may have reached Platform and executed.
    Unknown,
}

/// How a library spend failed.
#[derive(Debug)]
pub(crate) enum SpendError {
    /// Before the library ran: the lease's key set (nothing left the vault
    /// for a refusal; a release keeps the quote).
    Lease(LeaseError),
    Library(Box<PlatformWalletError>),
}

impl From<LeaseError> for SpendError {
    fn from(e: LeaseError) -> Self {
        Self::Lease(e)
    }
}

/// The lock serializing the wallet's shielded spends and shields in
/// `session`, shared by every caller while any holds it; Close Wallet and
/// the network close read it as "a shielded call is in flight".
pub(crate) fn wallet_lock(
    session: &NetworkSession,
    wallet: WalletId,
) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks =
        (session.platform.shielded_calls.lock()).unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&wallet).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(wallet, Arc::downgrade(&lock));
    lock
}

// MARK: the gate

/// The network's protocol version, refreshed only while below the
/// activation (the SDK's version only rises).
async fn network_version(lib: &impl ShieldedLib) -> u32 {
    let held = lib.protocol_version();
    if held >= SHIELDED_ACTIVATION_VERSION {
        return held;
    }
    lib.refresh_protocol_version().await
}

/// `shielded.not_active` below the activation; the version otherwise.
pub(crate) async fn gate(lib: &impl ShieldedLib) -> Result<u32, ShieldedError> {
    active(network_version(lib).await)
}

/// [`gate`] for a price: the rules at the version after one proved read.
/// The SDK holds the network's floor until a proof raises it, and the
/// library builds at the version it holds by then, so a quote at the floor
/// would price another version's fee (13's above 14's on testnet).
async fn priced_gate(lib: &impl ShieldedLib) -> Result<&'static PlatformVersion, ShieldedError> {
    rules_at(active(lib.refresh_protocol_version().await)?)
}

fn active(protocol_version: u32) -> Result<u32, ShieldedError> {
    if protocol_version < SHIELDED_ACTIVATION_VERSION {
        return Err(ShieldedError::NotActive {
            protocol_version,
            activation_version: SHIELDED_ACTIVATION_VERSION,
        });
    }
    Ok(protocol_version)
}

/// The rules at `protocol_version`.
fn rules_at(protocol_version: u32) -> Result<&'static PlatformVersion, ShieldedError> {
    Ok(super::protocol::platform_version(protocol_version)?)
}

/// The account's ledger, bound first; `shielded.not_synced{account}` until
/// it is established.
async fn ready_ledger(lib: &impl ShieldedLib) -> Result<Ledger, ShieldedError> {
    if !lib.bind().await? {
        return Err(ShieldedError::not_synced(Wait::Account));
    }
    lib.ledger()
        .await?
        .ok_or(ShieldedError::not_synced(Wait::Account))
}

/// The refusal while the dispatch journal is not ready: `journal` while it
/// is not read (only a closing session's: one reads it before it opens);
/// `platform.unavailable` once it was found unopenable or of a newer
/// schema, which does not clear.
fn journal_unread(session: &NetworkSession) -> ShieldedError {
    if session.leases.journal_loading() {
        ShieldedError::not_synced(Wait::Journal)
    } else {
        PlatformError::Unavailable.into()
    }
}

// MARK: reads

pub(crate) async fn status(lib: &impl ShieldedLib) -> Result<ShieldedStatus, ShieldedError> {
    let protocol_version = lib.protocol_version();
    let mut status = ShieldedStatus {
        active: protocol_version >= SHIELDED_ACTIVATION_VERSION,
        protocol_version,
        activation_version: SHIELDED_ACTIVATION_VERSION,
        ready: false,
        balance: 0,
        pending: 0,
        pending_shields: Vec::new(),
        unresolved_spends: Vec::new(),
        unrecorded_spends: Vec::new(),
        synced_index: None,
        last_sync_at: None,
        syncing: false,
        prover_ready: false,
    };
    if !status.active {
        return Ok(status);
    }
    let sync = lib.sync_state();
    status.syncing = sync.syncing;
    status.last_sync_at = sync.last_sync_at;
    status.prover_ready = lib.prover_ready();
    // Empty until the journal is read (the shield calls say so).
    status.pending_shields = pending_shields(lib).await.unwrap_or_default();
    if !lib.bind().await? {
        return Ok(status);
    }
    status.unresolved_spends = lib.unresolved().await?;
    if let Some(ledger) = lib.ledger().await? {
        status.ready = true;
        status.balance = ledger.spendable;
        status.synced_index = ledger.synced_index;
        status.pending = (lib.activity(0, MAX_HISTORY).await?.iter())
            .take_while(|e| e.status == Status::Pending)
            .map(|e| e.amount)
            .fold(0u64, u64::saturating_add);
    }
    Ok(status)
}

pub(crate) async fn sync(
    session: &NetworkSession,
    lib: &impl ShieldedLib,
) -> Result<ShieldedStatus, ShieldedError> {
    gate(lib).await?;
    if !session.platform.is_running() {
        return Err(PlatformError::Unavailable.into());
    }
    lib.bind().await?;
    lib.sync_now().await;
    status(lib).await
}

/// `status` with the wallet's marked spends (DEC-1057), active network or
/// not: they gate its sends either way.
pub(crate) async fn marked_status(
    session: &NetworkSession,
    wallet: WalletId,
    mut status: ShieldedStatus,
) -> Result<ShieldedStatus, ShieldedError> {
    let markers = (session.leases.unrecorded_spends(wallet).await)
        .map_err(|detail| PlatformError::Storage { detail })?;
    status.unrecorded_spends = (markers.into_iter())
        .map(|m| UnrecordedSpend {
            activity_id: hex::encode(m.activity),
            since: m.at,
        })
        .collect();
    Ok(status)
}

/// DEC-1057: while `wallet` has a marked spend (one of unknown result whose
/// record the library lost, so its notes read unspent), a send or unshield
/// runs only when the user confirmed it may still go through. Refused here,
/// before any note is selected or a key leaves the vault. Until the
/// dispatch journal is read no marker can be (a relaunch's), so every
/// spend waits then, as a shield does ([`journal_unread`]); read failures
/// refuse too.
async fn confirm_unrecorded(
    session: &NetworkSession,
    wallet: WalletId,
    confirmed: bool,
) -> Result<(), ShieldedError> {
    if !session.leases.journal_ready() {
        return Err(journal_unread(session));
    }
    if confirmed {
        return Ok(());
    }
    let markers = (session.leases.unrecorded_spends(wallet).await)
        .map_err(|detail| PlatformError::Storage { detail })?;
    if markers.is_empty() {
        Ok(())
    } else {
        Err(ShieldedError::UnrecordedSpend)
    }
}

pub(crate) async fn receive_address(
    lib: &impl ShieldedLib,
) -> Result<ShieldedAddress, ShieldedError> {
    gate(lib).await?;
    if !lib.bind().await? {
        return Err(ShieldedError::not_synced(Wait::Account));
    }
    let raw = (lib.address().await).ok_or(ShieldedError::not_synced(Wait::Account))?;
    let address = OrchardAddress::from_raw_bytes(&raw).map_err(internal)?;
    Ok(ShieldedAddress {
        address: address.to_bech32m_string(lib.network()),
        account: SHIELDED_ACCOUNT,
    })
}

pub(crate) async fn history(
    lib: &impl ShieldedLib,
    offset: u32,
    limit: u32,
) -> Result<Vec<ShieldedActivity>, ShieldedError> {
    gate(lib).await?;
    if limit > MAX_HISTORY {
        return Err(invalid("limit is above 500").into());
    }
    if !lib.bind().await? {
        return Ok(Vec::new());
    }
    let network = lib.network();
    Ok((lib.activity(offset, limit).await?.into_iter())
        .map(|e| activity(e, network))
        .collect())
}

/// One library activity row as the facade shows it.
pub(crate) fn activity(e: ShieldedActivityEntry, network: Network) -> ShieldedActivity {
    let kind = match e.kind {
        // From a Platform balance into the pool: an identity's is one too.
        Kind::Shield | Kind::ShieldFromIdentity { .. } => ShieldedActivityKind::Shield,
        Kind::ShieldFromAssetLock => ShieldedActivityKind::ShieldFromCore,
        Kind::Received => ShieldedActivityKind::Received,
        Kind::Sent => ShieldedActivityKind::Sent,
        Kind::Unshield => ShieldedActivityKind::Unshield,
        Kind::Withdrawal => ShieldedActivityKind::Withdrawal,
        Kind::IdentityCreate { .. } => ShieldedActivityKind::IdentityCreate,
        Kind::IdentityTopUp { .. } => ShieldedActivityKind::IdentityTopUp,
        Kind::ShieldedSpend => ShieldedActivityKind::Spend,
    };
    let counterparty = (e.counterparty.as_deref()).and_then(|bytes| match e.kind {
        Kind::Sent => <[u8; 43]>::try_from(bytes)
            .ok()
            .and_then(|raw| OrchardAddress::from_raw_bytes(&raw).ok())
            .map(|a| a.to_bech32m_string(network)),
        Kind::Unshield => PlatformAddress::from_bytes(bytes)
            .ok()
            .map(|a| a.to_bech32m_string(network)),
        Kind::Withdrawal => {
            Address::from_script(&dashcore::ScriptBuf::from(bytes.to_vec()), network)
                .ok()
                .map(|a| a.to_string())
        }
        _ => None,
    });
    let memo = (e.memo.as_deref())
        .map(|m| m.split(|b| *b == 0).next().unwrap_or_default())
        .filter(|m| !m.is_empty())
        .and_then(|m| std::str::from_utf8(m).ok())
        .map(str::to_owned);
    ShieldedActivity {
        id: hex::encode(e.id),
        kind,
        direction: match e.direction {
            Direction::In => ShieldedDirection::In,
            Direction::Out => ShieldedDirection::Out,
            Direction::SelfTransfer => ShieldedDirection::SelfTransfer,
        },
        credits: e.amount,
        fee_credits: e.fee,
        counterparty,
        memo,
        block_height: e.block_height,
        status: match e.status {
            Status::Pending => ShieldedActivityStatus::Pending,
            Status::Confirmed => ShieldedActivityStatus::Confirmed,
            Status::Failed => ShieldedActivityStatus::Failed,
            Status::Unknown => ShieldedActivityStatus::Unknown,
        },
        created_at: (e.created_at_ms != 0).then_some(e.created_at_ms / 1000),
    }
}

// MARK: spends

/// Where a spend goes.
enum To {
    Shielded(OrchardAddress, String),
    Core(Address),
}

/// `to` as a shielded payment address of `network`.
pub(crate) fn shielded_address(
    network: Network,
    to: &str,
) -> Result<OrchardAddress, ShieldedError> {
    let to = to.trim();
    let invalid = |detail: &str| ShieldedError::InvalidAddress {
        detail: detail.into(),
    };
    let hrp = PlatformAddress::hrp_for_network(network);
    let prefix = to.split_once('1').map(|(h, _)| h.to_ascii_lowercase());
    if prefix.as_deref() != Some(hrp) {
        return Err(invalid("not a shielded address of this network"));
    }
    OrchardAddress::from_bech32m_string(to).map_err(|_| invalid("not a shielded address"))
}

/// `to` as a P2PKH or P2SH address of `network`.
fn core_address(network: Network, to: &str) -> Result<Address, ShieldedError> {
    let invalid = |detail: &str| ShieldedError::InvalidAddress {
        detail: detail.into(),
    };
    let address = Address::from_str(to.trim()).map_err(|_| invalid("not a Dash address"))?;
    let address = (address.require_network(network))
        .map_err(|_| invalid("the address is of another network"))?;
    let script = address.script_pubkey();
    if !script.is_p2pkh() && !script.is_p2sh() {
        return Err(invalid("only P2PKH and P2SH addresses take withdrawals"));
    }
    Ok(address)
}

/// The memo field of `memo`: at most 36 bytes of UTF-8, zero-padded.
fn memo_field(memo: Option<&str>) -> Result<[u8; MEMO_BYTES], ShieldedError> {
    let bytes = memo.unwrap_or_default().as_bytes();
    if bytes.len() > MEMO_BYTES {
        return Err(invalid("the memo is over 36 bytes").into());
    }
    let mut field = [0; MEMO_BYTES];
    field[..bytes.len()].copy_from_slice(bytes);
    Ok(field)
}

/// What a spend of `credits` takes: Platform's fee for the notes the
/// library would select now.
async fn spend_fee(
    lib: &impl ShieldedLib,
    version: &PlatformVersion,
    credits: u64,
    kind: ShieldedFeeKind,
) -> Result<u64, ShieldedError> {
    let ledger = ready_ledger(lib).await?;
    let notes = lib.unspent().await?;
    match select_notes_with_fee(&notes, credits, MIN_ACTIONS, kind, version) {
        Ok((_, _, fee)) => Ok(fee),
        Err(PlatformWalletError::ShieldedInsufficientBalance { required, .. }) => {
            Err(ShieldedError::InsufficientFunds {
                needed: required,
                available: ledger.spendable,
            })
        }
        Err(PlatformWalletError::ShieldedNoUnspentNotes) => Err(ShieldedError::InsufficientFunds {
            needed: credits,
            available: ledger.spendable,
        }),
        Err(e) => Err(PlatformError::from(e).into()),
    }
}

/// The quote of a send of `credits` to `to`.
pub(crate) async fn send_quote(
    lib: &impl ShieldedLib,
    to: &str,
    credits: u64,
) -> Result<ShieldedSpendQuote, ShieldedError> {
    let version = priced_gate(lib).await?;
    shielded_address(lib.network(), to)?;
    if credits == 0 {
        return Err(ShieldedError::BelowMinimum { min: 1 });
    }
    let fee = spend_fee(lib, version, credits, ShieldedFeeKind::Base).await?;
    Ok(spend_quote(credits, fee, None))
}

fn spend_quote(credits: u64, fee: u64, expected_duffs: Option<u64>) -> ShieldedSpendQuote {
    let total = credits.saturating_add(fee);
    ShieldedSpendQuote {
        credits,
        fee_credits: fee,
        total_credits: total,
        expected_duffs,
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: total,
        },
    }
}

/// The quote of an unshield of `credits`: Platform's withdrawal range
/// (`withdrawal::Rules`, the range a payout clears dust in) and its fee.
async fn unshield_plan(
    lib: &impl ShieldedLib,
    credits: u64,
) -> Result<ShieldedSpendQuote, ShieldedError> {
    let version = priced_gate(lib).await?;
    let rules = Rules::at(version)?;
    if credits < rules.min {
        return Err(ShieldedError::BelowMinimum { min: rules.min });
    }
    if credits > rules.max {
        return Err(invalid("the amount is above Platform's largest withdrawal").into());
    }
    let fee = spend_fee(lib, version, credits, ShieldedFeeKind::Withdrawal).await?;
    let duffs = expected_duffs(credits, version)?;
    Ok(spend_quote(credits, fee, Some(duffs)))
}

pub(crate) async fn unshield_quote(
    lib: &impl ShieldedLib,
    credits: u64,
    to: Option<&str>,
) -> Result<ShieldedSpendQuote, ShieldedError> {
    if let Some(to) = to {
        gate(lib).await?;
        core_address(lib.network(), to)?;
    }
    unshield_plan(lib, credits).await
}

/// The lease `grant` names (a `flow` lease of this wallet), or a new one
/// redeemed from it.
async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    flow: FlowKind,
) -> Result<Lease, PlatformError> {
    match session.lease_for(&wallet, grant) {
        Some(lease) => {
            let lease = lease?;
            if lease.flow() != flow {
                return Err(PlatformError::GrantInvalid);
            }
            Ok(lease)
        }
        None => Ok(session
            .begin_lease(wallet, flow, &[grant.to_owned()])
            .await?),
    }
}

/// The library's refusal of a spend as the facade's error: the pool's note
/// floor (Platform's `InsufficientPoolNotesError`, which the library hands
/// on as text) is its own kind. An unconfirmed result is no refusal:
/// `spend` keeps it pending.
pub(crate) fn spend_error(e: PlatformWalletError) -> ShieldedError {
    match e {
        PlatformWalletError::ShieldedInsufficientBalance {
            available,
            required,
        } => ShieldedError::InsufficientFunds {
            needed: required,
            available,
        },
        // Retryable: the next sync moves the tree onto a recorded anchor
        // (operations.rs:2736, "wait for the next shielded sync").
        PlatformWalletError::ShieldedNoRecordedAnchor(_) => ShieldedError::not_synced(Wait::Tree),
        PlatformWalletError::ShieldedBroadcastFailed(ref text) => match pool_notes(text) {
            Some((notes, required)) => ShieldedError::PoolTooSmall { notes, required },
            None => PlatformError::from(e).into(),
        },
        e => PlatformError::from(e).into(),
    }
}

/// The counts in Platform's `InsufficientPoolNotesError` text ("Shielded
/// pool has insufficient notes …: pool has {current} notes but minimum
/// {required} required"), from its last "pool has".
fn pool_notes(text: &str) -> Option<(u64, u64)> {
    let (_, rest) = text.rsplit_once("pool has ")?;
    let (notes, rest) = rest.split_once(" notes but minimum ")?;
    let (required, _) = rest.split_once(" required")?;
    Some((notes.trim().parse().ok()?, required.trim().parse().ok()?))
}

/// What a spend that went out left.
struct Spent {
    balance: u64,
    state: ShieldedSpendState,
    unresolved: Option<UnresolvedSpend>,
}

/// The spendable balance as the library holds it now; 0 when it cannot be
/// read (the spend went out either way).
async fn balance(lib: &impl ShieldedLib) -> u64 {
    (lib.ledger().await.ok().flatten()).map_or(0, |l| l.spendable)
}

/// Writes the engine's marker of the spend the library just could not
/// confirm and kept no record of (DEC-1057): the reservations it made for
/// it before its broadcast, those of an activity not reserved before, give
/// its activity id, anchor and nullifiers (one spend's by construction:
/// one `pending_entry`, operations.rs:1822-1840). Exactly one such spend,
/// or an error.
async fn mark_unrecorded(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    reserved_before: &HashSet<[u8; 32]>,
) -> Result<(), String> {
    let reserved = lib.reservations().await.map_err(|e| e.to_string())?;
    // Activity → (anchor, nullifiers).
    type Reserved = ([u8; 32], Vec<[u8; 32]>);
    let mut spends: BTreeMap<[u8; 32], Reserved> = BTreeMap::new();
    for (nullifier, anchor, activity) in reserved {
        let Some(activity) = activity.filter(|a| !reserved_before.contains(a)) else {
            continue;
        };
        spends
            .entry(activity)
            .or_insert((anchor, Vec::new()))
            .1
            .push(nullifier);
    }
    let mut spends = spends.into_iter();
    let (Some((activity, (anchor, nullifiers))), None) = (spends.next(), spends.next()) else {
        return Err("the library holds no single new reservation of the spend".into());
    };
    (session.leases)
        .record_unrecorded_spend(wallet, activity, anchor, nullifiers)
        .await
}

/// A spend of `credits` under a `ShieldedSpend` lease from `grant`,
/// charged the total of its quote under the hold, so what the lease is
/// charged is what the library builds (Opus r1 m1). One the library could
/// not confirm is `Pending` with the record the library keeps of it
/// (module doc), never an error: the host must not offer to send it again.
async fn spend(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    credits: u64,
    grant: &str,
    to: &To,
    memo: [u8; MEMO_BYTES],
) -> Result<Spent, ShieldedError> {
    let operation = match to {
        To::Shielded(..) => "transfer",
        To::Core(_) => "withdraw",
    };
    // No pass runs from here until this spend's record is written: one
    // admitted meanwhile could find what the library stores on an unknown
    // result and re-send it before `admit_pass` saw it (Q15).
    let hold = hold_passes(session).await?;
    // Quoted again under the hold and the wallet's lock: nothing changes
    // the notes the library selects from now (a pass could, up to the
    // hold), so its fee is the one the library builds, at the version
    // `priced_gate` reads now. Checked against the grant before it is
    // redeemed, so every refusal keeps it: a fee above what the grant
    // confirmed is `platform.grant_exceeded`, nothing signed (Opus r1 m1).
    let quote = match to {
        To::Shielded(_, text) => send_quote(lib, text, credits).await?,
        To::Core(_) => unshield_plan(lib, credits).await?,
    };
    let remaining = grant_cap(session, wallet, grant, BudgetPurpose::Credits).await?;
    if quote.total_credits > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: quote.total_credits,
            remaining,
        }
        .into());
    }
    let lease = authority(session, wallet, grant, FlowKind::ShieldedSpend).await?;
    let permit = write_permit(&lease, operation, quote.total_credits)?;
    // The records and reservations before, to tell this spend's (spends of
    // the wallet run one at a time, under its lock, and no pass runs
    // under the hold).
    let before: HashSet<String> = (lib.unresolved().await?.into_iter())
        .map(|u| u.activity_id)
        .collect();
    let reserved_before: HashSet<[u8; 32]> = (lib.reservations().await?.into_iter())
        .filter_map(|(_, _, activity)| activity)
        .collect();
    let sent = leased(&lease, async {
        match to {
            To::Shielded(address, _) => lib.transfer(&permit, address, quote.credits, memo).await,
            To::Core(address) => lib.withdraw(&permit, address, quote.credits).await,
        }
    })
    .await;
    permit.finish(sent.is_ok());
    session
        .hub
        .pump
        .mark_platform(wallet, [PlatformChange::Shielded]);
    match sent {
        Ok(()) => {
            drop(hold);
            // The change note is the library's once a pass finds it.
            lib.sync_now().await;
            Ok(Spent {
                balance: balance(lib).await,
                state: ShieldedSpendState::Done,
                unresolved: None,
            })
        }
        Err(SpendError::Library(e))
            if matches!(*e, PlatformWalletError::ShieldedSpendUnconfirmed { .. }) =>
        {
            let unresolved = (lib.unresolved().await.unwrap_or_default().into_iter())
                .find(|u| !before.contains(&u.activity_id));
            match &unresolved {
                Some(u) => {
                    tracing::warn!(
                        operation,
                        activity_id = %u.activity_id,
                        transition = %u.transition,
                        "a shielded spend's result is unconfirmed; it stays pending and the \
                         library re-sends its signed transition during sync passes"
                    );
                    // Before any pass the engine admits after this one
                    // (`admit_pass` records it too, and holds the pass if
                    // it cannot).
                    if let Err(e) = lib.record(u).await {
                        tracing::warn!(error = %e, "could not record a shielded spend's transition");
                    }
                    drop(hold);
                }
                None => {
                    // The library's record write failed (best-effort at the
                    // pin, operations.rs:2983-3029): nothing of its keeps the
                    // spend past the session. The engine's own marker does
                    // (DEC-1057), before the send returns; without it the
                    // send is an error saying the payment may still go
                    // through, never a silent success.
                    tracing::warn!(
                        operation,
                        error = %e,
                        "a shielded spend's result is unconfirmed and the library kept no \
                         transition to re-send; it is marked until a scan finds its notes spent"
                    );
                    if let Err(detail) =
                        mark_unrecorded(session, wallet, lib, &reserved_before).await
                    {
                        tracing::error!(
                            operation,
                            %detail,
                            "an unrecorded shielded spend could not be marked"
                        );
                        return Err(PlatformError::BroadcastUnknown {
                            artifact: format!("shielded {operation}"),
                        }
                        .into());
                    }
                }
            }
            Ok(Spent {
                balance: balance(lib).await,
                state: ShieldedSpendState::Pending,
                unresolved,
            })
        }
        Err(SpendError::Lease(e)) => Err(PlatformError::from(e).into()),
        Err(SpendError::Library(e)) => Err(spend_error(*e)),
    }
}

pub(crate) async fn send(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    (to, credits): (&str, u64),
    memo: Option<&str>,
    grant: &str,
    confirmed: bool,
) -> Result<ShieldedSpendOutcome, ShieldedError> {
    let memo = memo_field(memo)?;
    let address = shielded_address(lib.network(), to)?;
    let lock = wallet_lock(session, wallet);
    let _spending = lock.lock().await;
    confirm_unrecorded(session, wallet, confirmed).await?;
    // Before the grant, so a refusal keeps it; `spend` quotes again under
    // the hold.
    send_quote(lib, to, credits).await?;
    let to = To::Shielded(address, to.trim().to_owned());
    let spent = spend(session, wallet, lib, credits, grant, &to, memo).await?;
    let To::Shielded(_, to) = to else {
        unreachable!("a send pays a shielded address")
    };
    Ok(outcome(credits, to, spent))
}

pub(crate) async fn unshield(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    credits: u64,
    to: Option<&str>,
    grant: &str,
    confirmed: bool,
) -> Result<ShieldedSpendOutcome, ShieldedError> {
    let lock = wallet_lock(session, wallet);
    let _spending = lock.lock().await;
    confirm_unrecorded(session, wallet, confirmed).await?;
    unshield_quote(lib, credits, to).await?;
    let to = match to {
        Some(to) => to.to_owned(),
        None => lib.receive_address().await?,
    };
    let address = core_address(lib.network(), &to)?;
    let spent = spend(
        session,
        wallet,
        lib,
        credits,
        grant,
        &To::Core(address),
        [0; MEMO_BYTES],
    )
    .await?;
    Ok(outcome(credits, to.trim().to_owned(), spent))
}

fn outcome(credits: u64, to: String, spent: Spent) -> ShieldedSpendOutcome {
    ShieldedSpendOutcome {
        credits,
        balance: spent.balance,
        to,
        state: spent.state,
        unresolved: spent.unresolved,
    }
}

// MARK: shields

/// Platform's flat fee for a ShieldFromAssetLock of one output, taken from
/// the lock's credits (platform-wallet's `shield_from_asset_lock_pool_fee`):
/// the minimum shielded fee of its two actions plus the asset lock's base
/// cost.
pub(crate) fn shield_pool_fee(version: &PlatformVersion) -> Result<u64, ShieldedError> {
    let base = (version.dpp.state_transitions.identities.asset_locks)
        .required_asset_lock_duff_balance_for_processing_start_for_address_funding
        .saturating_mul(CREDITS_PER_DUFF);
    let fee = compute_minimum_shielded_fee(MIN_ACTIONS, version).map_err(internal)?;
    Ok(fee.saturating_add(base))
}

/// A shield lock Platform has not been seen to take.
struct OpenShield {
    lock: LockInfo,
    /// It may be taken: Platform said so unproved, or a restore read it
    /// back from the chain (`RecoveredFromChain`).
    consumption_unknown: bool,
}

/// The wallet's shield locks Platform has not been seen to take (module
/// doc), `platform.unavailable` until the journal is read. The library's
/// `Consumed` mark does not survive SPV seeing the lock transaction again
/// in a later session (the row comes back `Broadcast`, then proved), so a
/// proved apply is also recorded in the dispatch journal (`top_up_settled`,
/// as `topup.rs` records its own).
async fn open_shields(lib: &impl ShieldedLib) -> Result<Vec<OpenShield>, ShieldedError> {
    let ours = lib.own_locks().ok_or(PlatformError::Unavailable)?;
    let settled = lib.settled_locks().await;
    let unknown = lib.unknown_locks().await;
    Ok((lib.shield_locks().await.into_iter())
        .filter(|l| !settled.contains(&l.outpoint.txid))
        .filter_map(|lock| {
            let consumption_unknown = match lock.status {
                LockStatus::Consumed => return None,
                // Another wallet's or an earlier install's: history.
                LockStatus::RecoveredFromChain if !ours.contains(&lock.outpoint.txid) => {
                    return None;
                }
                LockStatus::RecoveredFromChain => true,
                _ => unknown.contains(&lock.outpoint.txid),
            };
            Some(OpenShield {
                lock,
                consumption_unknown,
            })
        })
        .collect())
}

async fn pending_shields(lib: &impl ShieldedLib) -> Result<Vec<PendingShield>, ShieldedError> {
    let mut out: Vec<_> = (open_shields(lib).await?.into_iter())
        .map(|o| PendingShield {
            txid: o.lock.outpoint.txid.to_string(),
            duffs: o.lock.amount,
            proved: o.lock.status.has_proof(),
            consumption_unknown: o.consumption_unknown,
        })
        .collect();
    out.sort_by(|a, b| a.txid.cmp(&b.txid));
    Ok(out)
}

/// The fewest of `coins` that cover `duffs`, largest first: the fewest
/// inputs a lock of `duffs` can have (the library's selection may take
/// more).
fn fewest_inputs(coins: &[u64], duffs: u64) -> usize {
    let mut coins = coins.to_vec();
    coins.sort_unstable_by(|a, b| b.cmp(a));
    let mut sum = 0u64;
    for (n, value) in coins.iter().enumerate() {
        if sum >= duffs {
            return n;
        }
        sum = sum.saturating_add(*value);
    }
    coins.len()
}

/// What a shield of `duffs` needs, checked by the quote and by the shield
/// under the wallet's shielded lock.
async fn shield_plan(
    session: &NetworkSession,
    lib: &impl ShieldedLib,
    duffs: u64,
) -> Result<ShieldQuote, ShieldedError> {
    let version = priced_gate(lib).await?;
    if !session.leases.journal_ready() {
        return Err(journal_unread(session));
    }
    let pool_fee = shield_pool_fee(version)?;
    let min = pool_fee / CREDITS_PER_DUFF + 1;
    if duffs < min {
        return Err(ShieldedError::BelowMinimum { min });
    }
    // A lock whose consumption is unknown may never settle (only a proved
    // apply does, and Platform answers a consumed lock "already consumed",
    // unproved): it stays listed, resumable, and blocks nothing. The new
    // shield funds a new lock from the wallet's coins; that outpoint is no
    // coin of the wallet's.
    if let Some(open) = (open_shields(lib).await?.iter()).find(|o| !o.consumption_unknown) {
        return Err(ShieldedError::ShieldPending {
            txid: open.lock.outpoint.txid.to_string(),
        });
    }
    let fee_duffs = lib.fee_bound().await?;
    let spendable = lib.spendable().await?;
    let total_duffs = duffs.saturating_add(fee_duffs);
    if spendable.duffs < total_duffs {
        return Err(ShieldedError::FundingInsufficient {
            needed: total_duffs,
            available: spendable.duffs,
        });
    }
    if spendable.protected > 0 {
        return Err(ShieldedError::CoinsLocked);
    }
    let inputs = fewest_inputs(&lib.coin_values().await?, total_duffs);
    if inputs > MAX_LOCK_INPUTS {
        return Err(ShieldedError::TooManyInputs {
            inputs: inputs as u64,
            max: MAX_LOCK_INPUTS as u64,
        });
    }
    Ok(ShieldQuote {
        duffs,
        fee_duffs,
        total_duffs,
        pool_fee_credits: pool_fee,
        credits: duffs
            .saturating_mul(CREDITS_PER_DUFF)
            .saturating_sub(pool_fee),
        grant: GrantRequest {
            max_duffs: total_duffs,
            max_credits: 0,
        },
    })
}

pub(crate) async fn shield_quote(
    session: &NetworkSession,
    lib: &impl ShieldedLib,
    duffs: u64,
) -> Result<ShieldQuote, ShieldedError> {
    shield_plan(session, lib, duffs).await
}

/// What holds a shield's funding back, as the code the host sees: as a
/// top-up's (`topup.rs`), except that the waits which clear on their own
/// are Shielded still syncing (DEC-1058 #4): Core catching up or Platform
/// writes waiting for masternode sync while SPV runs (`Sync`, which a
/// top-up reports `platform.context_unavailable`), and a DashPay bring-up
/// that is running (`Startup`, `platform.unavailable` for a top-up). A
/// bring-up that ended without Platform's answers (none, partial, or
/// discovery failed locally) stays `platform.unavailable`: only a run again
/// changes it, which the call asks for. Read before that run starts.
fn held(lib: &impl ShieldedLib, wait: RegistrationWait) -> ShieldedError {
    match wait {
        RegistrationWait::Unlock => PlatformError::SignerUnavailable.into(),
        RegistrationWait::Sync if lib.spv_running() => ShieldedError::not_synced(Wait::Core),
        RegistrationWait::Startup if lib.startup_running() => {
            ShieldedError::not_synced(Wait::Startup)
        }
        RegistrationWait::Network | RegistrationWait::Startup => PlatformError::Unavailable.into(),
        _ => PlatformError::ContextUnavailable.into(),
    }
}

/// The wallet's shield step `k`.
pub(super) fn shield_step(k: u32) -> String {
    format!("shield/{k}/funding")
}

/// The step the next shield funds under: the first the funding gate
/// admits (`topup.rs`'s `next_step`).
pub(super) fn next_step(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<String, PlatformError> {
    use super::flows::DispatchState as D;
    use super::lease::FundingBlock as B;
    for k in 0.. {
        let step = shield_step(k);
        match session.leases.funding_block(wallet, &step) {
            None => return Ok(step),
            Some(B::Step(Some(D::MaybeSent))) => {
                return Err(PlatformError::BroadcastUnknown { artifact: step });
            }
            Some(B::Step(Some(D::WillBeSent))) => {
                return Err(PlatformError::WillBeSent { artifact: step });
            }
            Some(B::Step(_) | B::StepCancelled) => {}
            Some(block) => return Err(LeaseError::FundingBlocked(block).into()),
        }
    }
    unreachable!("the steps are unbounded")
}

pub(crate) async fn shield(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    duffs: u64,
    grant: &str,
) -> Result<ShieldOutcome, ShieldedError> {
    gate(lib).await?;
    if let Some(wait) = lib.funding_hold() {
        let refusal = held(lib, wait);
        if wait == RegistrationWait::Startup {
            session.platform.retry_startup(wallet);
        }
        return Err(refusal);
    }
    if !lib.bind().await? {
        return Err(ShieldedError::not_synced(Wait::Account));
    }
    let lock = wallet_lock(session, wallet);
    let _shielding = lock.lock().await;
    let plan = shield_plan(session, lib, duffs).await?;
    if !session.leases.refresh_funding(wallet).await {
        return Err(PlatformError::Unavailable.into());
    }
    let step = next_step(session, wallet)?;
    let remaining = funding_cap(session, wallet, grant).await?;
    if plan.total_duffs > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: plan.total_duffs,
            remaining,
        }
        .into());
    }
    let lease = authority(session, wallet, grant, FlowKind::Shield).await?;
    // Before the funding charges the budget (`topup.rs`).
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    let key = FundingKey {
        funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
        identity_index: 0,
        account: 0,
    };
    let funded = lease
        .scope_step(Some(step.clone()), async {
            let permit = lib.funding_permit(&lease, step.clone(), key, duffs).await?;
            let funded = lib.fund(duffs, &permit).await;
            permit.finish(funded.is_ok());
            funded
        })
        .await;
    // The step's own lock decides, never the call's result alone.
    let signed = session.leases.signed_lock_txids(wallet, &step);
    let own = (lib.shield_locks().await.into_iter()).find(|l| signed.contains(&l.outpoint.txid));
    let Some(lock) = own else {
        return Err(match funded {
            Err(FundError::Refused(e) | FundError::NotSent(e)) => e.into(),
            Err(FundError::CoinsLocked) => ShieldedError::CoinsLocked,
            // "Combine coins first": at least one coin past the cap.
            Err(FundError::TooManyInputs { max }) => ShieldedError::TooManyInputs {
                inputs: max + 1,
                max,
            },
            _ => PlatformError::BroadcastUnknown { artifact: step }.into(),
        });
    };
    let txid = lock.outpoint.txid.to_string();
    if !lock.status.has_proof() {
        tracing::info!(%txid, ?funded, "a shield's lock waits for its proof");
        return Ok(ShieldOutcome {
            txid,
            state: ShieldState::ProofWaiting,
        });
    }
    let state = apply(session, wallet, lib, &lease, &signer, lock.outpoint).await;
    Ok(ShieldOutcome { txid, state })
}

/// Applies the proved lock at `outpoint` under a call permit of `lease`.
async fn apply(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    lease: &Lease,
    signer: &VaultSigner,
    outpoint: OutPoint,
) -> ShieldState {
    let applied = lease
        .scope(async {
            let permit = lease.call_permit("shield", None)?;
            let applied = lib.apply(outpoint, signer).await;
            permit.finish(applied.is_ok());
            applied
        })
        .await;
    session
        .hub
        .pump
        .mark_platform(wallet, [PlatformChange::Shielded]);
    match applied {
        Ok(Applied::Done) => {
            if let Err(e) = lib.settle(outpoint.txid).await {
                tracing::warn!(%outpoint, error = %e, "could not record a shield's lock settled");
            }
            // The shielded note is the library's once a pass finds it.
            lib.sync_now().await;
            ShieldState::Applied {
                balance: balance(lib).await,
            }
        }
        // Neither settles (SR1): the lock pends, checking, until a proved
        // apply.
        Ok(how @ (Applied::Taken | Applied::Unknown)) => {
            tracing::warn!(
                %outpoint,
                ?how,
                "a shield's apply may have executed, unproved; its lock stays pending"
            );
            lib.mark_unknown(outpoint.txid).await;
            ShieldState::ConsumptionUnknown
        }
        // Refused before the lock's key signed a transition: nothing
        // reached Platform.
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a shield's apply failed; the lock stays tracked");
            ShieldState::NotApplied {
                error: e.code().to_owned(),
            }
        }
    }
}

/// Applies the pending shield lock `txid` again ("Check again"), whatever
/// its consumption: the library's `FromExistingAssetLock` of the same
/// outpoint ([`shield_funding`]), never a new lock, new funding or other
/// coins.
///
/// The pin cannot re-send a shield's signed bytes (it keeps none), so
/// this signs a new bundle (DEC-1045's exception for Core → Shielded).
/// Platform executes at most one shield of an outpoint: the transition's
/// `transform_into_action` validates the asset-lock proof
/// (rs-drive-abci `state_transitions/shield_from_asset_lock/transform_into_action/v1/mod.rs:126-138`),
/// whose `verify_is_not_spent` refuses an outpoint already fully consumed
/// (`common/asset_lock/proof/verify_is_not_spent/v0/mod.rs:30-55`; pin
/// test `should_return_already_consumed_error_when_fully_consumed`,
/// `:181`), and an executed shield records the outpoint used with nothing
/// remaining (rs-drive `action_convert_to_operations/shielded/shield_from_asset_lock_transition.rs:39-53`).
///
/// The proof is any the library derives for that outpoint (DEC-1051), not
/// necessarily the one the first apply carried. In platform-wallet at the
/// pin: the tracked row's proof, upgraded to the outpoint's ChainLock proof
/// once the lock is chain-locked locally (`asset_lock/orchestration.rs:701`
/// → `sync/recovery.rs:1693` → `sync/proof.rs:443-455`); a ChainLock proof
/// after the InstantSend wait timed out (`shielded/fund_from_asset_lock.rs:245-262`)
/// or after Platform refused the InstantSend one (`:396-433`); the
/// ChainLock proof persisted on an "already consumed" report, the row then
/// `RecoveredFromChain` (`asset_lock/orchestration.rs:478-521`,
/// `sync/tracking.rs:351`); after Platform refused a ChainLock proof, the
/// record's InstantSend proof, another ChainLock proof, or none until the
/// next resume derives one (`fund_from_asset_lock.rs:441-445` →
/// `sync/tracking.rs:564-620`). Within one call the ChainLock-height
/// retries sign fresh bundles of the same outpoint and proof (`:378`,
/// `:416`).
///
/// Only a proved apply settles the lock; Platform's "already consumed" and
/// a failure after signing leave it pending, `consumption_unknown` (SR1).
pub(crate) async fn resume(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    txid: &str,
    grant: &str,
) -> Result<ShieldOutcome, ShieldedError> {
    gate(lib).await?;
    let txid: Txid = txid
        .trim()
        .parse()
        .map_err(|_| invalid("txid is not a hex txid"))?;
    if !lib.bind().await? {
        return Err(ShieldedError::not_synced(Wait::Account));
    }
    if !session.leases.journal_ready() {
        return Err(journal_unread(session));
    }
    let lock = wallet_lock(session, wallet);
    let _shielding = lock.lock().await;
    let pending = (open_shields(lib).await?.into_iter())
        .map(|o| o.lock)
        .find(|l| l.outpoint.txid == txid)
        .ok_or_else(|| invalid("no pending shield has that txid"))?;
    let outcome = |state| ShieldOutcome {
        txid: txid.to_string(),
        state,
    };
    if !pending.status.has_proof()
        && lib
            .wait_proof(pending.outpoint, RESUME_PROOF_WAIT)
            .await
            .is_err()
    {
        return Ok(outcome(ShieldState::ProofWaiting));
    }
    let lease = authority(session, wallet, grant, FlowKind::Shield).await?;
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    let state = apply(session, wallet, lib, &lease, &signer, pending.outpoint).await;
    Ok(outcome(state))
}

fn invalid(detail: &str) -> PlatformError {
    PlatformError::InvalidArgument {
        detail: detail.into(),
    }
}

fn internal(e: impl std::fmt::Display) -> ShieldedError {
    PlatformError::Internal {
        detail: e.to_string(),
    }
    .into()
}

/// The key prefix, in a wallet's local scope of `app.sqlite`, of its shield
/// locks of unknown consumption (`ShieldedLib::unknown_locks`).
const UNKNOWN_SHIELD_PREFIX: &str = "shielded.shield_unknown.";

/// `wallet`'s shield locks recorded of unknown consumption.
pub(crate) fn read_unknown_shields(
    db: &dw_appdb::AppDb,
    wallet: WalletId,
) -> Result<HashSet<Txid>, dw_appdb::AppDbError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let rows = db.settings_with_prefix(&scope, UNKNOWN_SHIELD_PREFIX)?;
    Ok((rows.into_iter())
        .filter_map(|(key, _)| key[UNKNOWN_SHIELD_PREFIX.len()..].parse().ok())
        .collect())
}

/// Records `wallet`'s shield lock `txid` of unknown consumption, durably.
pub(crate) fn mark_unknown_shield(
    db: &dw_appdb::AppDb,
    wallet: WalletId,
    txid: Txid,
) -> Result<(), dw_appdb::AppDbError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    db.set_setting_durable(&scope, &format!("{UNKNOWN_SHIELD_PREFIX}{txid}"), Some("1"))
}

// MARK: the library

/// The real [`ShieldedLib`]: the wallet's library handles and the network's
/// coordinator.
pub(crate) struct PwShielded {
    pw: PwLib,
    manager: Arc<Manager>,
    coordinator: Arc<NetworkShieldedCoordinator>,
}

impl PwShielded {
    /// Runs `f` on the session's `app.sqlite` off the async threads.
    async fn on_appdb<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dw_appdb::AppDb) -> Result<T, dw_appdb::AppDbError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let appdb = self.pw.session.live()?.appdb;
        tokio::task::spawn_blocking(move || f(&appdb))
            .await
            .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
            .map_err(|e| PlatformError::Storage {
                detail: e.to_string(),
            })
    }

    pub(crate) async fn open(
        session: &Arc<NetworkSession>,
        wallet_id: WalletId,
    ) -> Result<Self, PlatformError> {
        let (manager, pw) = PwLib::open(session, wallet_id).await?;
        let coordinator = coordinator(session, &manager).await?;
        Ok(Self {
            pw,
            manager,
            coordinator,
        })
    }

    fn subwallet(&self) -> SubwalletId {
        SubwalletId::new(self.pw.wallet_id.0, SHIELDED_ACCOUNT)
    }
}

/// The manager's shielded coordinator, configured over the network
/// directory's [`SHIELDED_DB_FILE`] the first time (the library opens it
/// once per manager).
pub(crate) async fn coordinator(
    session: &NetworkSession,
    manager: &Manager,
) -> Result<Arc<NetworkShieldedCoordinator>, PlatformError> {
    if let Some(coordinator) = manager.shielded_coordinator().await {
        return Ok(coordinator);
    }
    // The store's open rehydrates every stored spend, so it is admitted as
    // a wallet load is (E1 r3 m1): an in-app close waits for one in flight
    // before its check, and none starts from its check to its commit,
    // where it would bring in a spend the check did not see. `try_read`:
    // the close holds the write side while the session's close awaits the
    // supervisor, which may be here (`read().await` would deadlock).
    let Ok(_admitted) = session.platform.wallet_loads.try_read() else {
        return Err(PlatformError::NetworkNotOpen);
    };
    // Not under a pass (R1-3, `PlatformRuntime::shielded_passes`).
    let _passes = session.platform.shielded_passes.write().await;
    (manager
        .configure_shielded(session.data_dir().join(SHIELDED_DB_FILE))
        .await)?;
    (manager.shielded_coordinator().await).ok_or_else(|| PlatformError::Internal {
        detail: "the shielded coordinator is not configured".into(),
    })
}

/// Runs `body` on the engine runtime with the wallet's [`PwShielded`].
pub(crate) async fn on_pool<T, F, Fut>(
    session: &Arc<NetworkSession>,
    wallet_id: WalletId,
    body: F,
) -> Result<T, ShieldedError>
where
    T: Send + 'static,
    F: FnOnce(Arc<NetworkSession>, PwShielded) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ShieldedError>> + Send + 'static,
{
    run_on_pool(session, wallet_id, false, body).await
}

/// [`on_pool`] for a call that signs (a send, an unshield, a shield or its
/// "Check again"), admitted under the session's
/// [`close_preflight`](super::runtime::PlatformRuntime::close_preflight):
/// it waits out an in-app close's check, and finds the session closed if
/// that close went ahead (E1 r2 R2-3). While it runs, that close is
/// refused.
pub(crate) async fn on_pool_signing<T, F, Fut>(
    session: &Arc<NetworkSession>,
    wallet_id: WalletId,
    body: F,
) -> Result<T, ShieldedError>
where
    T: Send + 'static,
    F: FnOnce(Arc<NetworkSession>, PwShielded) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ShieldedError>> + Send + 'static,
{
    run_on_pool(session, wallet_id, true, body).await
}

async fn run_on_pool<T, F, Fut>(
    session: &Arc<NetworkSession>,
    wallet_id: WalletId,
    signing: bool,
    body: F,
) -> Result<T, ShieldedError>
where
    T: Send + 'static,
    F: FnOnce(Arc<NetworkSession>, PwShielded) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ShieldedError>> + Send + 'static,
{
    let session = Arc::clone(session);
    let rt = session.rt.clone();
    rt.spawn(async move {
        // Before the session's gate: a close holding it to write waits for
        // the calls that entered, never for this one.
        let _admitted = match signing {
            true => Some(session.platform.close_preflight.read().await),
            false => None,
        };
        let _op = session.enter().await.map_err(PlatformError::from)?;
        session
            .require_wallet(&wallet_id)
            .map_err(PlatformError::from)?;
        let lib = PwShielded::open(&session, wallet_id).await?;
        body(Arc::clone(&session), lib).await
    })
    .await
    .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
}

fn internal_platform(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Internal {
        detail: e.to_string(),
    }
}

fn store_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Internal {
        detail: format!("shielded store: {e}"),
    }
}

impl ShieldedLib for PwShielded {
    fn network(&self) -> Network {
        self.pw.session.vault.network()
    }

    fn protocol_version(&self) -> u32 {
        self.pw.sdk.protocol_version_number()
    }

    async fn refresh_protocol_version(&self) -> u32 {
        (self.pw.session.refresh_protocol_version().await)
            .unwrap_or_else(|_| self.protocol_version())
    }

    async fn bind(&self) -> Result<bool, PlatformError> {
        let wallet = &self.pw.wallet;
        let accounts = [SHIELDED_ACCOUNT];
        if wallet.is_shielded_bound().await {
            return Ok(true);
        }
        // A bind adds the wallet to the coordinator's passes: never while
        // one is in flight (R1-3, `PlatformRuntime::shielded_passes`). A
        // read waits for it a little, then answers "not bound yet", so a
        // wallet that cannot bind (watch-only, the vault locked) does not
        // hold its status behind every pass.
        let passes = self.pw.session.platform.shielded_passes.write();
        let Ok(_passes) = tokio::time::timeout(BIND_PASS_WAIT, passes).await else {
            return Ok(false);
        };
        if (wallet.bind_shielded_from_persisted(&accounts, &self.coordinator)).await? {
            return Ok(true);
        }
        // First bind: the vault's viewing key, persisted before it is bound
        // (the library's own seed bind's order), so a restart rebinds from
        // the row.
        let (vault, id) = (self.pw.session.vault.clone(), self.pw.wallet_id);
        let fvk = tokio::task::spawn_blocking(move || vault.shielded_viewing_key(&id.0))
            .await
            .map_err(|e| PlatformError::Internal {
                detail: e.to_string(),
            })?;
        let fvk = match fvk {
            Ok(fvk) => fvk,
            Err(VaultError::Locked | VaultError::MixingOnly | VaultError::NoSecret) => {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let mut shielded = ShieldedChangeSet::default();
        shielded.record_viewing_key(self.subwallet(), fvk);
        let store = Arc::clone(&self.pw.session.live().map_err(PlatformError::from)?.store);
        tokio::task::spawn_blocking(move || {
            let changeset = PlatformWalletChangeSet {
                shielded: Some(shielded),
                ..Default::default()
            };
            store.store(id.0, changeset)?;
            store.flush(id.0)
        })
        .await
        .map_err(store_error)?
        .map_err(store_error)?;
        Ok((wallet.bind_shielded_from_persisted(&accounts, &self.coordinator)).await?)
    }

    async fn address(&self) -> Option<[u8; 43]> {
        self.pw
            .wallet
            .shielded_default_address(SHIELDED_ACCOUNT)
            .await
    }

    async fn ledger(&self) -> Result<Option<Ledger>, PlatformError> {
        let state = (self.coordinator)
            .local_balance_snapshot(self.pw.wallet_id.0)
            .await?;
        Ok(match state {
            // At the pin the wallet's store keeps the viewing key only, not
            // the notes (rs-platform-wallet-storage), so a restore leaves
            // the account empty and "Ready": established once a pass has
            // scanned it this session.
            ShieldedLocalBalanceState::Ready(snapshot) => {
                (snapshot.accounts).get(&SHIELDED_ACCOUNT).and_then(|a| {
                    a.last_scanned_index.map(|index| Ledger {
                        spendable: a.spendable_credits,
                        synced_index: Some(index),
                    })
                })
            }
            _ => None,
        })
    }

    async fn unspent(&self) -> Result<Vec<ShieldedNote>, PlatformError> {
        let store = self.coordinator.store().read().await;
        store
            .get_unspent_notes(self.subwallet())
            .map_err(store_error)
    }

    async fn activity(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ShieldedActivityEntry>, PlatformError> {
        let store = self.coordinator.store().read().await;
        (store.get_activity(self.subwallet(), offset as usize, limit as usize)).map_err(store_error)
    }

    async fn sync_now(&self) -> bool {
        guarded_pass(&self.pw.session, &self.manager, true).await
    }

    fn sync_state(&self) -> SyncState {
        let sync = self.manager.shielded_sync();
        SyncState {
            syncing: sync.is_syncing(),
            last_sync_at: sync.last_sync_unix_seconds(),
        }
    }

    fn prover_ready(&self) -> bool {
        CachedOrchardProver::new().is_ready()
    }

    async fn transfer(
        &self,
        permit: &CallPermit,
        to: &OrchardAddress,
        credits: u64,
        memo: [u8; MEMO_BYTES],
    ) -> Result<(), SpendError> {
        let keys = permit.shielded_keys()?;
        let prover = CachedOrchardProver::new();
        operations::transfer(
            &self.pw.sdk,
            self.coordinator.store(),
            Some(self.pw.wallet.persister()),
            self.pw.wallet_id.0,
            &keys,
            SHIELDED_ACCOUNT,
            to.inner(),
            credits,
            memo,
            &&prover,
        )
        .await
        .map_err(|e| SpendError::Library(Box::new(e)))
    }

    async fn withdraw(
        &self,
        permit: &CallPermit,
        to: &Address,
        credits: u64,
    ) -> Result<(), SpendError> {
        let keys = permit.shielded_keys()?;
        let prover = CachedOrchardProver::new();
        operations::withdraw(
            &self.pw.sdk,
            self.coordinator.store(),
            Some(self.pw.wallet.persister()),
            self.pw.wallet_id.0,
            &keys,
            SHIELDED_ACCOUNT,
            to,
            credits,
            CORE_FEE_PER_BYTE,
            &&prover,
        )
        .await
        .map_err(|e| SpendError::Library(Box::new(e)))
    }

    async fn receive_address(&self) -> Result<String, PlatformError> {
        let session = &self.pw.session;
        let issued = session
            .issue_address(self.pw.wallet_id, Some("Unshield".into()))
            .await?;
        Ok(issued.address)
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        (self.pw.session.leases).shield_lock_txids(self.pw.wallet_id)
    }

    async fn unknown_locks(&self) -> HashSet<Txid> {
        let mut out = (self.pw.session.platform.shield_unknown.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let wallet = self.pw.wallet_id;
        match self
            .on_appdb(move |db| read_unknown_shields(db, wallet))
            .await
        {
            Ok(stored) => out.extend(stored),
            Err(e) => {
                tracing::warn!(wallet_id = %wallet, error = %e, "could not read the shields of unknown consumption");
            }
        }
        out
    }

    async fn mark_unknown(&self, txid: Txid) {
        // In memory first: a failed write still shows it this session.
        (self.pw.session.platform.shield_unknown.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .insert(txid);
        let wallet = self.pw.wallet_id;
        if let Err(e) = self
            .on_appdb(move |db| mark_unknown_shield(db, wallet, txid))
            .await
        {
            tracing::warn!(%txid, error = %e, "could not record a shield of unknown consumption");
        }
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        let wallet = self.pw.wallet_id;
        (self.pw.session.leases.top_ups_settled(wallet).await).unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %wallet, error = %e, "could not read the settled shields");
            HashSet::new()
        })
    }

    async fn settle(&self, txid: Txid) -> Result<(), PlatformError> {
        let (leases, wallet) = (&self.pw.session.leases, self.pw.wallet_id);
        (leases
            .settle_top_up(wallet, txid, TopUpSettled::Applied)
            .await)
            .map_err(|e| {
                tracing::warn!(%txid, error = %e, "could not record a shield's lock settled");
                PlatformError::Unavailable
            })
    }

    async fn record(&self, spend: &UnresolvedSpend) -> Result<(), PlatformError> {
        let bytes = |hex_text: &str| {
            let mut out = [0u8; 32];
            hex::decode_to_slice(hex_text, &mut out).map_err(internal_platform)?;
            Ok::<_, PlatformError>(out)
        };
        let record = (bytes(&spend.activity_id)?, bytes(&spend.transition)?);
        (self.pw.session.leases)
            .record_shielded_redrives(self.pw.wallet_id, vec![record])
            .await
            .map_err(|e| PlatformError::Storage { detail: e })
    }

    async fn unresolved(&self) -> Result<Vec<UnresolvedSpend>, PlatformError> {
        let store = self.coordinator.store().read().await;
        let redrives = (store.pending_redrives(self.subwallet())).map_err(store_error)?;
        Ok((redrives.into_iter())
            .map(|r| UnresolvedSpend {
                activity_id: hex::encode(r.activity_id),
                // Platform's state transition hash: SHA-256 of the
                // serialized transition, the bytes the library re-sends.
                transition: hex::encode(transition_hash(&r.st_bytes)),
                attempts: r.attempts,
            })
            .collect())
    }

    async fn reservations(&self) -> Result<Vec<StalePendingSpend>, PlatformError> {
        let store = self.coordinator.store().read().await;
        (store.stale_pending_spends(self.subwallet())).map_err(store_error)
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        let session = &self.pw.session;
        let fence = session.coin_fence(self.pw.wallet_id).await?;
        let snapshot = session.fenced_coins(&fence).await?;
        Ok((snapshot.coins.iter())
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            .map(|c| c.utxo.value())
            .collect())
    }

    async fn shield_locks(&self) -> Vec<LockInfo> {
        (self.shield_lock_rows().await.into_iter())
            .map(|l| l.info)
            .collect()
    }

    async fn shield_lock_rows(&self) -> Vec<super::unfinished_locks::KindLock> {
        (self
            .pw
            .wallet
            .asset_locks()
            .list_tracked_locks()
            .await
            .iter())
        .filter(|l| l.funding_type == AssetLockFundingType::AssetLockShieldedAddressTopUp)
        .map(super::unfinished_locks::KindLock::of_row)
        .collect()
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Reg::spendable(&self.pw).await
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        self.pw.session.fee_bound_worst(self.pw.wallet_id).await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        Reg::funding_hold(&self.pw)
    }

    fn startup_running(&self) -> bool {
        use super::startup::StartupStatus as S;
        let startup = self.pw.session.dashpay_startup(&self.pw.wallet_id);
        matches!(startup.map(|s| s.startup), Ok(S::NotRun | S::Starting))
    }

    fn spv_running(&self) -> bool {
        self.pw.session.hub.tracker().snapshot().running
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        Reg::funding_permit(&self.pw, lease, step, key, duffs).await
    }

    async fn fund(&self, duffs: u64, permit: &CallPermit) -> Result<OutPoint, FundError> {
        // At most `MAX_LOCK_INPUTS` coins, whatever key-wallet's selection
        // picks (its BranchAndBound counts no inputs): the quote's check is
        // largest first.
        let signer = (permit.funding_signer())
            .map_err(|e| FundError::Refused(e.into()))?
            .with_input_cap(MAX_LOCK_INPUTS);
        let funded = (self.pw)
            .fund_lock_with(
                AssetLockFundingType::AssetLockShieldedAddressTopUp,
                duffs,
                0,
                &signer,
            )
            .await;
        if signer.capped() {
            tracing::info!(
                ?funded,
                "a shield's lock needed more than {MAX_LOCK_INPUTS} coins"
            );
            return Err(FundError::TooManyInputs {
                max: MAX_LOCK_INPUTS as u64,
            });
        }
        funded
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        Reg::wait_proof(&self.pw, outpoint, timeout).await
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<dashcore::ScriptBuf> {
        use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
        let locks = self.pw.wallet.asset_locks().list_tracked_locks().await;
        let lock = locks.into_iter().find(|l| l.out_point == outpoint)?;
        let Some(TransactionPayload::AssetLockPayloadType(payload)) =
            lock.transaction.special_transaction_payload
        else {
            return None;
        };
        let output = payload.credit_outputs.get(outpoint.vout as usize)?;
        Some(output.script_pubkey.clone())
    }

    async fn apply<S>(&self, outpoint: OutPoint, signer: &S) -> Result<Applied, PlatformError>
    where
        S: key_wallet::ExtendedPubKeySigner
            + key_wallet::Signer<Error = dw_vault::SignerError>
            + Send
            + Sync,
    {
        let raw = (self.address().await).ok_or(PlatformError::Internal {
            detail: "the shielded account is not bound".into(),
        })?;
        let to = OrchardAddress::from_raw_bytes(&raw).map_err(|e| PlatformError::Internal {
            detail: e.to_string(),
        })?;
        let funding = shield_funding(outpoint);
        let signer = Witnessed::new(signer);
        let applied = (self.pw.wallet)
            .shielded_fund_from_asset_lock(
                &self.coordinator,
                funding,
                vec![(to, None)],
                &signer,
                &CachedOrchardProver::new(),
                None,
                0,
                None,
                Some(APPLY_CHAIN_LOCK_WAIT),
            )
            .await;
        applied_from(applied, outpoint, signer.signed())
    }
}

/// The library funding of a shield's apply: the tracked lock at `outpoint`
/// as it is, whatever proof the library then derives for it; never a new
/// lock (`FromWalletBalance`) nor an invitation's (DEC-1045 (b), DEC-1051).
pub(crate) fn shield_funding(outpoint: OutPoint) -> AssetLockFunding {
    AssetLockFunding::FromExistingAssetLock {
        out_point: outpoint,
        consume_invitation_voucher: false,
    }
}

/// How the library's shield call of the lock at `outpoint` ended, given
/// whether the lock's credit key signed a transition in it. Only "already
/// consumed" naming `outpoint` is [`Applied::Taken`].
pub(crate) fn applied_from(
    result: Result<(), PlatformWalletError>,
    outpoint: OutPoint,
    signed: bool,
) -> Result<Applied, PlatformError> {
    match result {
        Ok(()) => Ok(Applied::Done),
        // The library keeps the lock "consumption unknown" with its
        // ChainLock proof (it cannot authenticate the report).
        Err(PlatformWalletError::AssetLockAlreadyConsumed(o)) if o == outpoint => {
            Ok(Applied::Taken)
        }
        // The library signs the transition and then broadcasts it
        // (`build_and_broadcast_shielded`, `fund_from_asset_lock.rs:707-757`):
        // a failure after a signature may follow its submission.
        Err(e) if signed => {
            tracing::warn!(error = %e, "a shield's apply failed after its transition was signed");
            Ok(Applied::Unknown)
        }
        Err(e) => Err(e.into()),
    }
}

/// A shield's credit-key signer that notes whether it signed: nothing can
/// reach Platform before it has (R1-2).
pub(crate) struct Witnessed<'a, S> {
    inner: &'a S,
    signed: AtomicBool,
}

impl<'a, S> Witnessed<'a, S> {
    pub(crate) fn new(inner: &'a S) -> Self {
        Self {
            inner,
            signed: AtomicBool::new(false),
        }
    }

    /// Whether `inner` returned a signature.
    pub(crate) fn signed(&self) -> bool {
        self.signed.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl<S: key_wallet::Signer<Error = dw_vault::SignerError>> key_wallet::Signer for Witnessed<'_, S> {
    type Error = dw_vault::SignerError;

    fn supported_methods(&self) -> &[key_wallet::SignerMethod] {
        self.inner.supported_methods()
    }

    async fn sign_ecdsa(
        &self,
        path: &key_wallet::bip32::DerivationPath,
        sighash: [u8; 32],
    ) -> Result<
        (
            dashcore::secp256k1::ecdsa::Signature,
            dashcore::secp256k1::PublicKey,
        ),
        Self::Error,
    > {
        let signature = self.inner.sign_ecdsa(path, sighash).await?;
        self.signed.store(true, Ordering::SeqCst);
        Ok(signature)
    }

    async fn public_key(
        &self,
        path: &key_wallet::bip32::DerivationPath,
    ) -> Result<dashcore::secp256k1::PublicKey, Self::Error> {
        self.inner.public_key(path).await
    }
}

#[async_trait::async_trait]
impl<S: key_wallet::ExtendedPubKeySigner + key_wallet::Signer<Error = dw_vault::SignerError>>
    key_wallet::ExtendedPubKeySigner for Witnessed<'_, S>
{
    async fn extended_public_key(
        &self,
        path: &key_wallet::bip32::DerivationPath,
    ) -> Result<key_wallet::bip32::ExtendedPubKey, Self::Error> {
        self.inner.extended_public_key(path).await
    }
}

// MARK: sync passes

/// Why the engine held a shielded sync pass back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PassHeld {
    /// The network's coordinator is not configured (an inactive network).
    NoCoordinator,
    /// The wallet holds a spend the pass would re-send and not its
    /// background lease (E0-04 §4.5: the vault is not prompt-free).
    NoLease(WalletId),
    /// The library's redrive records or the journal could not be read or
    /// written.
    Store(String),
    Journal(String),
}

/// A test's pause of an in-app close's check after its first wallet
/// (`PlatformRuntime::close_check_pause`).
#[cfg(test)]
pub(crate) struct CheckPause {
    pub(crate) reached: tokio::sync::oneshot::Sender<WalletId>,
    pub(crate) resume: tokio::sync::oneshot::Receiver<()>,
}

/// A test's pause of a pass after its admission (`PlatformRuntime::pass_pause`).
#[cfg(test)]
pub(crate) struct PassPause {
    pub(crate) reached: tokio::sync::oneshot::Sender<()>,
    pub(crate) resume: tokio::sync::oneshot::Receiver<()>,
}

/// Platform's hash of a state transition: SHA-256 of its serialized bytes.
fn transition_hash(st_bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(st_bytes).into()
}

/// The checks before each shielded sync pass the engine runs (E0-04 Q15,
/// ruling (c), DEC-1032). The pass is where the library re-sends a spend it
/// could not confirm, byte for byte (`redrive_pending_spends` from
/// `coordinator.rs:1431`), so it is admitted only when each wallet holding
/// such a spend holds its background lease (the vault prompt-free), and
/// after each such spend is on record in the dispatch journal against its
/// activity, with its transition's hash.
pub(crate) async fn admit_pass(
    session: &NetworkSession,
    manager: &Manager,
) -> Result<Arc<NetworkShieldedCoordinator>, PassHeld> {
    let coordinator = (manager.shielded_coordinator().await).ok_or(PassHeld::NoCoordinator)?;
    // Every wallet the library's pass will cover: the manager's and the
    // coordinator's registered ones (the pass syncs and re-sends for the
    // latter, `coordinator.rs:1342-1348`, `:1423-1439`). Neither set changes
    // until the pass ends: loads, binds, removals and the configuration
    // wait for `shielded_passes` (R1-3).
    let mut wallets: BTreeSet<[u8; 32]> = manager.list_wallet_ids_blocking().into_iter().collect();
    wallets.extend(
        (coordinator.registered_subwallets().await)
            .into_iter()
            .map(|s| s.wallet_id),
    );
    for id in wallets {
        let stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(id)
            .map_err(|e| PassHeld::Store(e.to_string()))?;
        if stored.is_empty() {
            continue;
        }
        let wallet = WalletId(id);
        if session.leases.background(&wallet).is_none() {
            return Err(PassHeld::NoLease(wallet));
        }
        let records = (stored.iter())
            .map(|(_, r)| (r.activity_id, transition_hash(&r.st_bytes)))
            .collect();
        (session
            .leases
            .record_shielded_redrives(wallet, records)
            .await)
            .map_err(PassHeld::Journal)?;
    }
    Ok(coordinator)
}

/// Whether wallet `id` holds a spend whose result is not known, stored by
/// the library for its identical-bytes redrive (every row of the wallet in
/// `shielded_pending_spends`, rehydrated at the store's open). Removing the
/// wallet from the manager would delete it (DEC-1046,
/// `PlatformRuntime::remove_from_manager`). No coordinator: nothing is
/// purged, so nothing is held. A store that cannot be read holds.
pub(crate) async fn holds_stored_spends(manager: &Manager, id: WalletId) -> bool {
    let Some(coordinator) = manager.shielded_coordinator().await else {
        return false;
    };
    let stored = (coordinator.store().read().await).pending_redrives_for_wallet(id.0);
    stored.map_or(true, |stored| !stored.is_empty())
}

/// Whether wallet `id` holds a spend whose result is not known: one the
/// library stored ([`holds_stored_spends`]) or one the engine marked
/// because the library kept no record of it (DEC-1057, read fail closed).
/// Close Wallet, an in-app network close and a restore's rollback refuse
/// such a wallet (`shielded_spend_unresolved`).
pub(crate) async fn holds_unresolved(
    leases: &super::lease::LeaseTable,
    manager: &Manager,
    id: WalletId,
) -> bool {
    holds_stored_spends(manager, id).await || leases.marks_unrecorded(id).await
}

/// After this long a drain logs that a shielded pass in flight is still
/// running ([`drain_passes`]), and keeps waiting.
pub(crate) const SHIELDED_PASS_DRAIN: Duration = Duration::from_secs(10);

/// Waits until no shielded pass is in flight (the drain of a lock, of a
/// revocation and of the close, `drain_shielded_pass`); a spend's network
/// call is not waited for (`shielded_spends`). A pass ends on its own,
/// every wait in it bounded at the pin:
/// - each SDK call (the anchors' fetch, the note sync's chunk fetches, a
///   redrive's `st.broadcast(sdk, None)`) under the SDK's default request
///   settings: every attempt cut at 10 s over the whole attempt
///   (rs-dapi-client `request_settings.rs:9`, `dapi_client.rs:1198`), at
///   most `retries` + 1 = 4 attempts (rs-sdk `sdk.rs:89`,
///   `dapi_client.rs:1086`);
/// - a proof's unknown quorum: the engine's context provider waits for SPV
///   up to `UNKNOWN_QUORUM_WAIT` (120 s, `trust.rs`), and the trusted
///   provider's HTTP lookup is cut at 30 s
///   (rs-sdk-trusted-context-provider `provider.rs:230`);
/// - the number of calls: the note sync's chunks (the pool's size), and
///   at most 3 broadcasts per stored spend.
///
/// The worst case is therefore a pass's call count times about 160 s; on
/// a reachable network a pass takes seconds (testnet, 6,430 notes: about 1
/// s).
pub(crate) async fn drain_passes(passes: &tokio::sync::RwLock<()>) {
    // One acquire, kept across the log: its place in the lock's queue too.
    let write = passes.write();
    tokio::pin!(write);
    tokio::select! {
        _ = &mut write => return,
        () = tokio::time::sleep(SHIELDED_PASS_DRAIN) => {}
    }
    tracing::warn!("a shielded sync pass is still running; the drain waits for its end");
    drop(write.await);
    tracing::info!("the shielded sync pass ended; the drain is done");
}

/// How long a spend waits for a pass in flight before it holds the passes
/// off ([`hold_passes`]); a pass scanning a large pool takes seconds.
const SPEND_PASS_WAIT: Duration = Duration::from_secs(60);

/// How long a first bind waits for a pass in flight before it answers
/// "not bound yet" (`PwShielded::bind`).
const BIND_PASS_WAIT: Duration = Duration::from_secs(5);

/// Holds the shielded passes off, from a spend's library call until its
/// record is written; `platform.unavailable` (retry) when a pass in flight
/// does not end within [`SPEND_PASS_WAIT`].
async fn hold_passes(
    session: &NetworkSession,
) -> Result<tokio::sync::RwLockWriteGuard<'_, ()>, ShieldedError> {
    let spends = session.platform.shielded_spends.write();
    (tokio::time::timeout(SPEND_PASS_WAIT, spends).await)
        .map_err(|_| PlatformError::Unavailable.into())
}

/// One shielded sync pass, the only way the engine runs one: after
/// [`admit_pass`], started only while the session is open (the facade's
/// calls in [`on_pool`], the supervisor's `kick_shielded_pass`), and
/// holding the session's pass lock from the admission to its end, so a
/// lock's drain (`drain_shielded_pass`) waits for it and no pass admitted
/// before the lock outlives it. `false` when it was held back.
pub(crate) async fn guarded_pass(session: &NetworkSession, manager: &Manager, force: bool) -> bool {
    // A spend's hold first, then the drain's lock (always in this order).
    let _spends = session.platform.shielded_spends.read().await;
    let _pass = session.platform.shielded_passes.read().await;
    if let Err(held) = admit_pass(session, manager).await {
        tracing::info!(?held, "a shielded sync pass is held back");
        return false;
    }
    #[cfg(test)]
    {
        let pause = (session.platform.pass_pause.lock())
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.resume.await;
        }
    }
    let summary = manager.shielded_sync().sync_now(force).await;
    if summary.error_count() > 0 {
        tracing::info!(
            errors = summary.error_count(),
            "a shielded sync pass failed"
        );
    }
    true
}

// MARK: the Platform side's start

/// The shielded part of the Platform start, before its loops start
/// (`bringup.rs`): on an active network, the coordinator, each of
/// `wallets` bound (where its key is at hand), and the prover's key built
/// in the background. The library's own sync loop never runs (its passes
/// would skip [`admit_pass`]): the supervisor runs [`guarded_pass`] every
/// interval while the loops are on, and the stop quiesces a pass in flight
/// with the other loops.
pub(crate) async fn start(session: &Arc<NetworkSession>, manager: &Manager, wallets: &[WalletId]) {
    if manager.sdk_arc().protocol_version_number() < SHIELDED_ACTIVATION_VERSION {
        return;
    }
    if let Err(e) = coordinator(session, manager).await {
        tracing::warn!(error = %e, "the shielded coordinator did not open");
        return;
    }
    for &wallet in wallets {
        let bound = match PwShielded::open(session, wallet).await {
            Ok(lib) => lib.bind().await,
            Err(e) => Err(e),
        };
        if let Err(e) = bound {
            tracing::warn!(wallet_id = %wallet, error = %e, "a wallet's shielded keys did not bind");
        }
    }
    warm_prover();
}

/// Builds the Orchard prover's key on the blocking pool, once per process
/// (about 3.5 s of CPU, `docs/research/shielded-cost.md`): a process-wide
/// cache that holds nothing of a session, so a close does not wait for it.
pub(crate) fn warm_prover() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if CachedOrchardProver::new().is_ready() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::task::spawn_blocking(|| {
        let started = std::time::Instant::now();
        CachedOrchardProver::new().warm_up();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "the Orchard prover is ready"
        );
    });
}

/// Whether the supervisor runs shielded passes on this network: the
/// coordinator is configured (an active network's Platform start did).
pub(crate) async fn runs_sync(manager: &Manager) -> bool {
    manager.shielded_coordinator().await.is_some()
}
