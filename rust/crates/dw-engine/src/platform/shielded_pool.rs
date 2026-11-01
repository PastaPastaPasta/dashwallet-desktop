//! Shielded funds (0.1 E1; m4-dashpay-engine §2.13, §7 item 69) over
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
//!   fails: not executed). The spend's outcome and the status name that
//!   transition by Platform's hash of it, and the dispatch journal records
//!   it against the activity; nothing here resends. Every pass goes through
//!   [`guarded_pass`]: admitted only while the session is open (the
//!   supervisor's kick, or [`on_pool`]), only while each wallet holding such
//!   a spend holds its background lease, and only once each is on record
//!   (a spend holds the passes off from its library call until its record
//!   is written, so none can find a spend stored mid-pass). The library's
//!   own loop never runs. A pass admitted before a lock or a close may
//!   finish after the leases drop, as a leased call admitted before a freeze
//!   finishes in its drain: the lock (async, and the relock timer) and the
//!   close wait for it up to [`SHIELDED_PASS_DRAIN`],
//!   and the close then seals the coordinator (`Manager::shutdown`). The
//!   library has no stop inside a pass, so one stuck longer is logged and
//!   ends on its own; the synchronous lock does not wait (0.2: the library
//!   admits each send, `q15-hook`).
//! - **Shields.** Core → Shielded is a top-up's two steps (`topup.rs`): an
//!   asset lock of the Core balance (funding type
//!   `AssetLockShieldedAddressTopUp`, step `shield/<k>/funding`, held at
//!   most once by the dispatch journal as a top-up's), then the library's
//!   `shielded_fund_from_asset_lock` of the proved lock to the account's own
//!   address, signed with the lock's credit key by the lease's uncounted
//!   funding signer. A lock not applied pends (`pending_shields`) and
//!   blocks a new shield until `resume_shield` applies it. As for top-ups
//!   (`topup.rs`), Platform's "already consumed" proves nothing: such a
//!   lock (this session's answer), and one of ours a restore read back from
//!   the chain, reads `consumption_unknown` and pends until a proved apply or the user's
//!   `resolve_shield`; a recovered lock no shield step signed is history.
//!   Until the dispatch journal is read none of this can be told, so the
//!   shield calls answer `platform.unavailable`. The lock spends at most
//!   [`MAX_LOCK_INPUTS`] coins. Nothing stops a shield partway: the call
//!   runs in [`on_pool`]'s task, which the close waits for and never
//!   aborts, under no engine timeout; the library's own waits are bounded
//!   by its arguments.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
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
use platform_wallet::wallet::shielded::{
    CachedOrchardProver, NetworkShieldedCoordinator, ShieldedActivityEntry,
    ShieldedActivityKind as Kind, ShieldedActivityStatus as Status, ShieldedDirection as Direction,
    ShieldedLocalBalanceState, ShieldedNote, ShieldedStore, SubwalletId, operations,
};
use platform_wallet::{AssetLockFundingType, PlatformWalletError};
use sha2::{Digest, Sha256};

use super::errors::{PlatformError, ShieldedError};
use super::flows::{BudgetPurpose, FlowKind, GrantRequest};
use super::lease::{CallPermit, FundingKey, Lease, LeaseError};
use super::registration::{RegistrationWait, funding_cap};
use super::registration_flow::{
    FundError, LockInfo, LockStatus, PwLib, RegistrationLib as Reg, Spendable, lock_status,
};
use super::requests::{leased, write_permit};
use super::shielded::{
    PendingShield, SHIELDED_ACTIVATION_VERSION, ShieldOutcome, ShieldQuote, ShieldState,
    ShieldedActivity, ShieldedActivityKind, ShieldedActivityStatus, ShieldedAddress,
    ShieldedDirection, ShieldedSpendOutcome, ShieldedSpendQuote, ShieldedSpendState,
    ShieldedStatus, UnresolvedSpend,
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
/// about 65 inputs, and the library falls back to a ChainLock proof only
/// when Platform finds the InstantSend signature invalid, so a bigger lock
/// would strand (0.2 design review X1b-2).
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
    /// The locks the wallet's shield steps signed (the dispatch journal's
    /// records); `None` until the journal is read.
    fn own_locks(&self) -> Option<HashSet<Txid>>;
    /// The shield locks Platform answered "already consumed", unproved, for
    /// in this session.
    fn taken_locks(&self) -> HashSet<Txid>;
    /// Records Platform's unproved "already consumed" for `txid`.
    fn mark_taken(&self, txid: Txid);
    /// The wallet's locks the dispatch journal records settled; none when
    /// they cannot be read (the locks then pend, fail closed).
    fn settled_locks(&self) -> impl Future<Output = HashSet<Txid>> + Send;
    /// Records the shield lock `txid` settled: a proved apply or the user's
    /// word.
    fn settle(
        &self,
        txid: Txid,
        how: TopUpSettled,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
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
    /// The values of the coins a funding may select (those
    /// [`Spendable::duffs`] counts).
    fn coin_values(&self) -> impl Future<Output = Result<Vec<u64>, PlatformError>> + Send;
    fn spendable(&self) -> impl Future<Output = Result<Spendable, PlatformError>> + Send;
    /// E0-04's `fee_bound_worst`: the lock's fee quote.
    fn fee_bound(&self) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// What holds a funding back (SPV behind, the bring-up not answered).
    fn funding_hold(&self) -> Option<RegistrationWait>;
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
    fn apply(
        &self,
        outpoint: OutPoint,
        signer: &VaultSigner,
    ) -> impl Future<Output = Result<Applied, PlatformError>> + Send;
}

/// How an apply ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Platform's proved execution.
    Done,
    /// Platform answered that the lock is already consumed: unproved.
    Taken,
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

/// The lock serializing the wallet's shielded spends and shields, shared by
/// every caller while any holds it.
fn wallet_lock(wallet: WalletId) -> Arc<tokio::sync::Mutex<()>> {
    type Locks = HashMap<WalletId, Weak<tokio::sync::Mutex<()>>>;
    static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
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

/// The account's ledger, bound first; `shielded.not_synced` until it is
/// established.
async fn ready_ledger(lib: &impl ShieldedLib) -> Result<Ledger, ShieldedError> {
    if !lib.bind().await? {
        return Err(ShieldedError::NotSynced);
    }
    lib.ledger().await?.ok_or(ShieldedError::NotSynced)
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

pub(crate) async fn receive_address(
    lib: &impl ShieldedLib,
) -> Result<ShieldedAddress, ShieldedError> {
    gate(lib).await?;
    if !lib.bind().await? {
        return Err(ShieldedError::NotSynced);
    }
    let raw = lib.address().await.ok_or(ShieldedError::NotSynced)?;
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
        // Retryable: the next sync moves the tree onto a recorded anchor.
        PlatformWalletError::ShieldedNoRecordedAnchor(_) => ShieldedError::NotSynced,
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

/// A spend under a `ShieldedSpend` lease from `grant`, charged `quote`'s
/// total. One the library could not confirm is `Pending` with the record
/// the library keeps of it (module doc), never an error: the host must
/// not offer to send it again.
async fn spend(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    quote: &ShieldedSpendQuote,
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
    // The records before, to tell this spend's (spends of the wallet run
    // one at a time, under its lock).
    let before: HashSet<String> = (lib.unresolved().await?.into_iter())
        .map(|u| u.activity_id)
        .collect();
    let lease = authority(session, wallet, grant, FlowKind::ShieldedSpend).await?;
    let permit = write_permit(&lease, operation, quote.total_credits)?;
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
                None => tracing::warn!(
                    operation,
                    error = %e,
                    "a shielded spend's result is unconfirmed and the library kept no \
                     transition to re-send; it stays pending until a scan or the prune \
                     backstop resolves it"
                ),
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
) -> Result<ShieldedSpendOutcome, ShieldedError> {
    let memo = memo_field(memo)?;
    let address = shielded_address(lib.network(), to)?;
    let lock = wallet_lock(wallet);
    let _spending = lock.lock().await;
    // Under the lock, so the notes quoted are the ones this send takes,
    // and before the grant, so a refusal keeps it.
    let quote = send_quote(lib, to, credits).await?;
    let to = To::Shielded(address, to.trim().to_owned());
    let spent = spend(session, wallet, lib, &quote, grant, &to, memo).await?;
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
) -> Result<ShieldedSpendOutcome, ShieldedError> {
    let lock = wallet_lock(wallet);
    let _spending = lock.lock().await;
    let quote = unshield_quote(lib, credits, to).await?;
    let to = match to {
        Some(to) => to.to_owned(),
        None => lib.receive_address().await?,
    };
    let address = core_address(lib.network(), &to)?;
    let spent = spend(
        session,
        wallet,
        lib,
        &quote,
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
    let taken = lib.taken_locks();
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
                _ => taken.contains(&lock.outpoint.txid),
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
        return Err(PlatformError::Unavailable.into());
    }
    let pool_fee = shield_pool_fee(version)?;
    let min = pool_fee / CREDITS_PER_DUFF + 1;
    if duffs < min {
        return Err(ShieldedError::BelowMinimum { min });
    }
    if let Some(open) = open_shields(lib).await?.first() {
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

/// What holds a funding back, as the code the host sees (`topup.rs`).
fn held(wait: RegistrationWait) -> PlatformError {
    match wait {
        RegistrationWait::Unlock => PlatformError::SignerUnavailable,
        RegistrationWait::Network | RegistrationWait::Startup => PlatformError::Unavailable,
        _ => PlatformError::ContextUnavailable,
    }
}

/// The wallet's shield step `k`.
pub(super) fn shield_step(k: u32) -> String {
    format!("shield/{k}/funding")
}

/// The step the next shield funds under: the first the funding gate
/// admits (`topup.rs`'s `next_step`).
fn next_step(session: &NetworkSession, wallet: WalletId) -> Result<String, PlatformError> {
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
        if wait == RegistrationWait::Startup {
            session.platform.retry_startup(wallet);
        }
        return Err(held(wait).into());
    }
    if !lib.bind().await? {
        return Err(ShieldedError::NotSynced);
    }
    let lock = wallet_lock(wallet);
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
            if let Err(e) = lib.settle(outpoint.txid, TopUpSettled::Applied).await {
                tracing::warn!(%outpoint, error = %e, "could not record a shield's lock settled");
            }
            // The shielded note is the library's once a pass finds it.
            lib.sync_now().await;
            ShieldState::Applied {
                balance: balance(lib).await,
            }
        }
        Ok(Applied::Taken) => {
            tracing::warn!(
                %outpoint,
                "Platform says a shield's lock is already consumed, unproved; it stays pending"
            );
            lib.mark_taken(outpoint.txid);
            ShieldState::ConsumptionUnknown
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a shield's apply failed; the lock stays tracked");
            ShieldState::NotApplied {
                error: e.code().to_owned(),
            }
        }
    }
}

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
        return Err(ShieldedError::NotSynced);
    }
    let lock = wallet_lock(wallet);
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

/// The user's word that the pending shield `txid`, one whose consumption
/// is unknown, reached the pool: recorded settled in the journal, so it
/// stops pending and blocking (`topup.rs`'s `resolve`).
pub(crate) async fn resolve(
    wallet: WalletId,
    lib: &impl ShieldedLib,
    txid: &str,
) -> Result<(), ShieldedError> {
    gate(lib).await?;
    let txid: Txid = txid
        .trim()
        .parse()
        .map_err(|_| invalid("txid is not a hex txid"))?;
    let lock = wallet_lock(wallet);
    let _shielding = lock.lock().await;
    (open_shields(lib).await?.into_iter())
        .find(|o| o.lock.outpoint.txid == txid && o.consumption_unknown)
        .ok_or_else(|| invalid("no pending shield of that txid has an unknown consumption"))?;
    lib.settle(txid, TopUpSettled::Resolved).await?;
    tracing::info!(%txid, "the user resolved a shield's lock");
    Ok(())
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

// MARK: the library

/// The real [`ShieldedLib`]: the wallet's library handles and the network's
/// coordinator.
pub(crate) struct PwShielded {
    pw: PwLib,
    manager: Arc<Manager>,
    coordinator: Arc<NetworkShieldedCoordinator>,
}

impl PwShielded {
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
    let session = Arc::clone(session);
    let rt = session.rt.clone();
    rt.spawn(async move {
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
        if wallet.is_shielded_bound().await
            || (wallet.bind_shielded_from_persisted(&accounts, &self.coordinator)).await?
        {
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

    fn taken_locks(&self) -> HashSet<Txid> {
        (self.pw.session.platform.shield_taken.lock())
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn mark_taken(&self, txid: Txid) {
        (self.pw.session.platform.shield_taken.lock())
            .unwrap_or_else(|p| p.into_inner())
            .insert(txid);
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        let wallet = self.pw.wallet_id;
        (self.pw.session.leases.top_ups_settled(wallet).await).unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %wallet, error = %e, "could not read the settled shields");
            HashSet::new()
        })
    }

    async fn settle(&self, txid: Txid, how: TopUpSettled) -> Result<(), PlatformError> {
        let (leases, wallet) = (&self.pw.session.leases, self.pw.wallet_id);
        leases.settle_top_up(wallet, txid, how).await.map_err(|e| {
            tracing::warn!(%txid, ?how, error = %e, "could not record a shield's lock settled");
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
        (self
            .pw
            .wallet
            .asset_locks()
            .list_tracked_locks()
            .await
            .into_iter())
        .filter(|l| l.funding_type == AssetLockFundingType::AssetLockShieldedAddressTopUp)
        .map(|l| LockInfo {
            outpoint: l.out_point,
            registration: false,
            identity_index: l.identity_index,
            account: l.account_index,
            amount: l.amount,
            status: lock_status(&l.status),
        })
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
        (self.pw)
            .fund_lock(
                AssetLockFundingType::AssetLockShieldedAddressTopUp,
                duffs,
                0,
                permit,
            )
            .await
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        Reg::wait_proof(&self.pw, outpoint, timeout).await
    }

    async fn apply(
        &self,
        outpoint: OutPoint,
        signer: &VaultSigner,
    ) -> Result<Applied, PlatformError> {
        let raw = (self.address().await).ok_or(PlatformError::Internal {
            detail: "the shielded account is not bound".into(),
        })?;
        let to = OrchardAddress::from_raw_bytes(&raw).map_err(|e| PlatformError::Internal {
            detail: e.to_string(),
        })?;
        let funding = AssetLockFunding::FromExistingAssetLock {
            out_point: outpoint,
            consume_invitation_voucher: false,
        };
        let applied = (self.pw.wallet)
            .shielded_fund_from_asset_lock(
                &self.coordinator,
                funding,
                vec![(to, None)],
                signer,
                &CachedOrchardProver::new(),
                None,
                0,
                None,
                Some(APPLY_CHAIN_LOCK_WAIT),
            )
            .await;
        match applied {
            Ok(()) => Ok(Applied::Done),
            // The library keeps the lock "consumption unknown" with its
            // ChainLock proof (it cannot authenticate the report).
            Err(PlatformWalletError::AssetLockAlreadyConsumed(_)) => Ok(Applied::Taken),
            Err(e) => Err(e.into()),
        }
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
    for id in manager.list_wallet_ids_blocking() {
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

/// One shielded sync pass, the only way the engine runs one: after
/// [`admit_pass`], started only while the session is open (the facade's
/// calls in [`on_pool`], the supervisor's `kick_shielded_pass`), and
/// holding the session's pass lock from the admission to its end, so a
/// lock's drain (`drain_shielded_pass`) waits for it and no pass admitted
/// before the lock outlives it. `false` when it was held back.
/// How long a lock or a close waits for a shielded pass in flight
/// (`drain_shielded_pass`), as the library's own drain
/// (`COORDINATOR_DRAIN_BUDGET`).
pub(crate) const SHIELDED_PASS_DRAIN: Duration = Duration::from_secs(10);

/// How long a spend waits for a pass in flight before it holds the passes
/// off ([`hold_passes`]); a pass scanning a large pool takes seconds.
const SPEND_PASS_WAIT: Duration = Duration::from_secs(60);

/// Holds the shielded passes off, from a spend's library call until its
/// record is written; `platform.unavailable` (retry) when a pass in flight
/// does not end within [`SPEND_PASS_WAIT`].
async fn hold_passes(
    session: &NetworkSession,
) -> Result<tokio::sync::RwLockWriteGuard<'_, ()>, ShieldedError> {
    let passes = session.platform.shielded_passes.write();
    (tokio::time::timeout(SPEND_PASS_WAIT, passes).await)
        .map_err(|_| PlatformError::Unavailable.into())
}

pub(crate) async fn guarded_pass(session: &NetworkSession, manager: &Manager, force: bool) -> bool {
    let _pass = session.platform.shielded_passes.read().await;
    if let Err(held) = admit_pass(session, manager).await {
        tracing::info!(?held, "a shielded sync pass is held back");
        return false;
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
