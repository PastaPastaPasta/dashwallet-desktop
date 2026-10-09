//! Sending: transaction drafts, coin selection, prepare (sign and reserve,
//! never broadcast), broadcast and abandon (docs/contracts/m1-engine.md §2.7).
//!
//! Flow: [`NetworkSession::new_tx_draft`] → setters (offline validation) →
//! [`TxDraft::estimate`] (plan only) → [`TxDraft::prepare`] (plan, redeem the
//! `Spend` grant, build and sign through the vault's `VaultSigner`, reserve
//! the inputs) → [`TxDraft::broadcast`] or [`TxDraft::abandon`].
//!
//! Funds and reservations go through platform-wallet's
//! `CoreWallet::finalize_transaction_with_options` with the planned inputs
//! seeded and funding in reservation-only mode, so key-wallet reserves
//! exactly the planned coins and the build can be released owner-guarded.
//!
//! Spend cap (review H-3/H-4, fix-review L4): a `Spend{max_duffs}` grant caps
//! [`PreparedSummary::total_debit`], what leaves the wallet: the value paid
//! to scripts it does not own (recipients and a foreign change address)
//! plus the fee. `sign_psbt` caps the same outflow, so a quick-unlock
//! spending limit means the same in both. The fee alone is also bounded by
//! [`MAX_TX_FEE`] (`send.absurd_fee`).

#[cfg(test)]
mod flow_tests;
pub(crate) mod plan;
pub(crate) mod psbt;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, PubkeyHash, ScriptBuf, ScriptHash, Transaction, TxOut};
use dw_uri::keyio::{AddressKind, Destination, classify_address};
use dw_vault::{GrantKind, VaultError};
use key_wallet::Utxo;
use key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy;
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionBuilder;
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::core::{SEND_FUNDING_SOURCES, SignedCoreTransaction};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use self::plan::{InputChoice, MAX_MONEY, Plan, PlanError, PlanOutput, dust_threshold};
use crate::coins::{CoinSnapshot, now_secs};
use crate::platform::lease::fence::HandOff;
use crate::platform::lease::{
    AdmitRequest, ArtifactId, ArtifactKind, DispatchScope, Lease, LeaseError, Outcome, Settlement,
    Verdict,
};
use crate::{EngineError, NetworkSession, WalletId};

/// dash-qt `-maxtxfee` default: a fee above 0.1 DASH is absurd (QT-058).
pub const MAX_TX_FEE: u64 = 10_000_000;
/// Minimum relay fee rate and the floor of a custom rate, duffs per kB.
pub const MIN_FEE_PER_KB: u64 = 1_000;
/// Highest custom fee rate accepted, duffs per kB (0.1 DASH/kB, Dash Core's
/// maximum broadcast fee rate).
pub const MAX_FEE_PER_KB: u64 = 10_000_000;
/// Longest confirmation target dash-qt offers (QT-057).
pub const MAX_CONF_TARGET: u32 = 1008;

/// Which coins a draft may spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoinSource {
    /// Every spendable coin of the standard accounts (BIP44, BIP32, DashPay
    /// receiving), except user-locked, reserved, immature and untrusted
    /// unconfirmed ones. CoinJoin coins are never pooled with them.
    Any,
    /// Only fully mixed CoinJoin coins, confirmed or InstantSend-locked
    /// (QT-051, the CoinJoin send page). The transaction has no change
    /// output: what the inputs hold beyond the recipients goes to the fee,
    /// as dash-qt's CoinJoin page does (still capped by `absurd_fee`). Only
    /// `ChangePolicy::Auto` is accepted with this source.
    FullyMixedOnly,
    /// Exactly these coins, all of them (coin control, QT-068).
    Outpoints(Vec<OutPoint>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeMode {
    /// On SPV every target uses the minimum relay fee (DESIGN-opus §1.14).
    Recommended { target_blocks: u32 },
    /// Custom rate, duffs per 1000 bytes.
    PerKb(u64),
}

impl FeeMode {
    /// The rate this mode pays, or `InvalidArgument`.
    pub fn rate(self) -> Result<FeeRate, EngineError> {
        match self {
            FeeMode::Recommended { target_blocks } => {
                if target_blocks == 0 || target_blocks > MAX_CONF_TARGET {
                    return Err(EngineError::InvalidArgument(format!(
                        "confirmation target {target_blocks} is outside 1..={MAX_CONF_TARGET}"
                    )));
                }
                Ok(FeeRate::new(MIN_FEE_PER_KB))
            }
            FeeMode::PerKb(rate) => {
                if !(MIN_FEE_PER_KB..=MAX_FEE_PER_KB).contains(&rate) {
                    return Err(EngineError::InvalidArgument(format!(
                        "fee rate {rate} duff/kB is outside {MIN_FEE_PER_KB}..={MAX_FEE_PER_KB}"
                    )));
                }
                Ok(FeeRate::new(rate))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePolicy {
    /// A fresh address on the BIP44 internal chain.
    Auto,
    /// Custom change address (QT-073); may be foreign, which counts against
    /// the spend cap.
    Address(String),
}

/// One payment line as the host enters it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub address: String,
    pub amount: u64,
    pub subtract_fee_from_amount: bool,
    /// Saved to the address book after a broadcast (QT-063).
    pub label: Option<String>,
    /// `message` of a payment URI, stored with the transaction (QT-054).
    pub message: Option<String>,
}

/// Why a payment cannot be drafted, prepared or sent. One variant per
/// `send.*` code of the contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendFailure {
    #[error("no recipients")]
    NoRecipients,
    #[error("recipient {index}: invalid address")]
    InvalidAddress { index: u32 },
    #[error("recipient {index}: platform address")]
    PlatformAddress { index: u32 },
    #[error("recipient {index}: invalid amount")]
    InvalidAmount { index: u32 },
    #[error("recipient {index}: dust amount")]
    DustAmount { index: u32 },
    #[error("recipient {index}: duplicate address")]
    DuplicateAddress { index: u32 },
    #[error("amount exceeds the available {available}")]
    AmountExceedsBalance { available: u64 },
    #[error("amount with fee {fee} exceeds the available {available}")]
    AmountWithFeeExceedsBalance { fee: u64, available: u64 },
    #[error("recipient {index} is too small to pay its share of the fee")]
    AmountTooSmallAfterFee { index: u32 },
    #[error("insufficient mixed funds, {available} available")]
    InsufficientMixedFunds { available: u64 },
    #[error("outpoint {0} is unavailable")]
    OutpointUnavailable(OutPoint),
    #[error("absurd fee {fee}")]
    AbsurdFee { fee: u64 },
    #[error("transaction too large")]
    TxTooLarge,
    #[error("invalid change address")]
    InvalidChangeAddress,
    #[error("watch-only wallet")]
    WatchOnly,
    #[error("vault locked")]
    VaultLocked,
    #[error("grant invalid")]
    GrantInvalid,
    /// `outflow` = `external_sent + fee`, what the payment takes from the
    /// wallet.
    #[error("outflow of {outflow} exceeds the grant's {max_duffs}")]
    GrantExceeded { max_duffs: u64, outflow: u64 },
    #[error("prepared transaction is no longer pending")]
    PreparedTxSpent,
    #[error("no peers")]
    NoPeers,
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
    #[error("broadcast outcome unknown: {reason}")]
    BroadcastUnknown { reason: String },
    /// A payment under a lease that Lock (or another revocation) ended
    /// before it was handed off: nothing was sent (E0-04 §4.6, DP3-01
    /// `send.cancelled`).
    #[error("cancelled before anything was sent")]
    Cancelled,
}

impl From<SendFailure> for EngineError {
    fn from(f: SendFailure) -> Self {
        EngineError::Send(f)
    }
}

impl From<PlanError> for SendFailure {
    fn from(e: PlanError) -> Self {
        match e {
            PlanError::AmountExceedsBalance { available } => {
                SendFailure::AmountExceedsBalance { available }
            }
            PlanError::AmountWithFeeExceedsBalance { fee, available } => {
                SendFailure::AmountWithFeeExceedsBalance { fee, available }
            }
            PlanError::AmountTooSmallAfterFee { index } => SendFailure::AmountTooSmallAfterFee {
                index: index as u32,
            },
            PlanError::TooManyInputs { .. } => SendFailure::TxTooLarge,
            // A sum beyond u64 is far beyond MAX_MONEY.
            PlanError::Overflow => SendFailure::TxTooLarge,
        }
    }
}

/// Maps a lease's refusal at `prepare` (E0-04 §4.6, §16.4).
fn lease_failure(e: LeaseError, outflow: u64) -> EngineError {
    match e {
        LeaseError::Revoked(_) | LeaseError::Locked => SendFailure::Cancelled,
        LeaseError::Ended | LeaseError::Invalid => SendFailure::GrantInvalid,
        LeaseError::Parked(_) | LeaseError::NeedsGrant(_) | LeaseError::Vault(_) => {
            SendFailure::VaultLocked
        }
        LeaseError::Exceeded { remaining, .. } => SendFailure::GrantExceeded {
            max_duffs: remaining,
            outflow,
        },
    }
    .into()
}

/// A lease's Spend charge made before signing (E0-04 §4.2): refunded on
/// drop unless bound to the signed txid.
struct SpendCharge {
    lease: Lease,
    charge: Option<crate::platform::lease::budget::Charge>,
}

impl SpendCharge {
    fn bind(mut self, txid: ArtifactId) {
        if let Some(charge) = self.charge.take() {
            self.lease
                .table
                .bind_spend(self.lease.id(), self.lease.wallet(), txid, charge);
        }
    }
}

impl Drop for SpendCharge {
    fn drop(&mut self) {
        if let Some(charge) = self.charge.take() {
            self.lease
                .refund(crate::platform::BudgetPurpose::Spend, charge);
        }
    }
}

/// Maps a vault refusal while redeeming the grant or opening the signer.
fn vault_failure(e: VaultError) -> EngineError {
    match e {
        VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly => {
            SendFailure::VaultLocked.into()
        }
        VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => {
            SendFailure::GrantInvalid.into()
        }
        VaultError::NoSecret => SendFailure::WatchOnly.into(),
        other => EngineError::Vault(other),
    }
}

/// Fee and size preview; nothing signed or reserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxEstimate {
    pub fee: u64,
    /// key-wallet's size estimate (148 bytes per input).
    pub size_bytes: u32,
    pub input_count: u32,
    pub change: Option<u64>,
    /// What the recipients receive, after any subtract-fee share.
    pub total_sent: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInput {
    pub outpoint: OutPoint,
    pub address: Option<String>,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedOutput {
    /// `None` for a script with no address form.
    pub address: Option<String>,
    pub amount: u64,
    pub is_change: bool,
    /// Pays one of this wallet's addresses.
    pub is_mine: bool,
    /// The recipient's label, for recipient outputs.
    pub label: Option<String>,
}

/// What the confirm dialog shows (QT-059, IOS-046), read from the signed
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSummary {
    pub txid: String,
    pub fee: u64,
    pub fee_rate_per_kb: u64,
    /// Serialized size of the signed transaction.
    pub size_bytes: u32,
    pub inputs: Vec<PreparedInput>,
    /// Transaction order (BIP69).
    pub outputs: Vec<PreparedOutput>,
    /// What the recipients receive.
    pub total_sent: u64,
    /// Paid to scripts the wallet does not own.
    pub external_sent: u64,
    /// `external_sent + fee`: inputs minus outputs back to the wallet; the
    /// figure the `Spend` grant caps.
    pub total_debit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastOutcome {
    pub txid: String,
}

/// Inputs of prepared transactions that are neither accepted by the network
/// nor released, per wallet and per session (in memory). Coin listings mark
/// them `reserved` and selection skips them (key-wallet keeps its own
/// reservations private).
///
/// An entry goes when its transaction is accepted, released (abandoned,
/// dropped while pending, or never sent), or when the wallet no longer
/// holds the coin unspent: every coin read ([`NetworkSession::coin_snapshot`])
/// drops entries whose coin the wallet has seen spent, by the transaction
/// itself (SPV injects a broadcast into the wallet's own mempool view, so
/// that is immediate) or by a conflicting one. Nothing that merely elapses
/// releases the inputs of a transaction whose outcome is unknown, matching
/// platform-wallet's pending-spend fence: re-selecting them could sign a
/// second spend of coins the first transaction may still take. A session
/// close drops them all.
#[derive(Debug, Default)]
pub(crate) struct PendingSpends {
    inner: Mutex<HashMap<WalletId, HashSet<OutPoint>>>,
}

impl PendingSpends {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<WalletId, HashSet<OutPoint>>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn snapshot(&self, wallet: &WalletId) -> HashSet<OutPoint> {
        self.map().get(wallet).cloned().unwrap_or_default()
    }

    pub(crate) fn add(&self, wallet: WalletId, outpoints: impl IntoIterator<Item = OutPoint>) {
        self.map().entry(wallet).or_default().extend(outpoints);
    }

    pub(crate) fn remove(&self, wallet: &WalletId, outpoints: impl IntoIterator<Item = OutPoint>) {
        let mut map = self.map();
        if let Some(set) = map.get_mut(wallet) {
            for o in outpoints {
                set.remove(&o);
            }
            if set.is_empty() {
                map.remove(wallet);
            }
        }
    }

    /// Drops entries whose coin is no longer unspent (a broadcast whose
    /// outcome was unknown has since been seen spent).
    /// Drops every reservation of an unloaded wallet.
    pub(crate) fn forget_wallet(&self, wallet: &WalletId) {
        self.map().remove(wallet);
    }

    pub(crate) fn retain_unspent(&self, wallet: &WalletId, unspent: &HashSet<OutPoint>) {
        let mut map = self.map();
        if let Some(set) = map.get_mut(wallet) {
            set.retain(|o| unspent.contains(o));
            if set.is_empty() {
                map.remove(wallet);
            }
        }
    }
}

/// An L1 address of `network`, or why `text` is not one.
pub(crate) fn l1_address(text: &str, network: dashcore::Network) -> Result<Address, AddressKind> {
    match classify_address(text, network) {
        AddressKind::Core(Destination::PubKeyHash(h)) => Ok(Address::new(
            network,
            Payload::PubkeyHash(PubkeyHash::from_byte_array(h)),
        )),
        AddressKind::Core(Destination::ScriptHash(h)) => Ok(Address::new(
            network,
            Payload::ScriptHash(ScriptHash::from_byte_array(h)),
        )),
        other => Err(other),
    }
}

/// A recipient that passed offline validation.
#[derive(Debug, Clone)]
struct ValidRecipient {
    address: Address,
    amount: u64,
    subtract_fee: bool,
    label: Option<String>,
    message: Option<String>,
}

/// Validates a recipient list for `network` (QT-055, QT-060, QT-067).
fn validate_recipients(
    recipients: Vec<Recipient>,
    network: dashcore::Network,
) -> Result<Vec<ValidRecipient>, SendFailure> {
    if recipients.is_empty() {
        return Err(SendFailure::NoRecipients);
    }
    let mut seen: HashSet<ScriptBuf> = HashSet::new();
    let mut out = Vec::with_capacity(recipients.len());
    for (i, r) in recipients.into_iter().enumerate() {
        let index = i as u32;
        let address = match l1_address(&r.address, network) {
            Ok(a) => a,
            Err(AddressKind::Platform(_)) => return Err(SendFailure::PlatformAddress { index }),
            Err(_) => return Err(SendFailure::InvalidAddress { index }),
        };
        if r.amount == 0 || r.amount > MAX_MONEY {
            return Err(SendFailure::InvalidAmount { index });
        }
        let script = address.script_pubkey();
        if r.amount < dust_threshold(script.len()) {
            return Err(SendFailure::DustAmount { index });
        }
        if !seen.insert(script) {
            return Err(SendFailure::DuplicateAddress { index });
        }
        out.push(ValidRecipient {
            address,
            amount: r.amount,
            subtract_fee: r.subtract_fee_from_amount,
            label: r.label.filter(|l| !l.is_empty()),
            message: r.message.filter(|m| !m.is_empty()),
        });
    }
    let total = out
        .iter()
        .try_fold(0u64, |acc, r| acc.checked_add(r.amount))
        .filter(|t| *t <= MAX_MONEY);
    if total.is_none() {
        return Err(SendFailure::InvalidAmount {
            index: out.len() as u32 - 1,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
enum ChangeTarget {
    Auto,
    Address(Address),
}

#[derive(Debug, Clone)]
struct DraftState {
    recipients: Vec<ValidRecipient>,
    source: CoinSource,
    fee: FeeRate,
    change: ChangeTarget,
}

static NEXT_DRAFT_ID: AtomicU64 = AtomicU64::new(1);

/// An editable payment for one wallet. Setters validate offline; `estimate`
/// and `prepare` read the wallet. A draft keeps its session alive but works
/// only while the session is open (`NetworkNotOpen` after close) and the
/// wallet is registered (`WalletNotFound` after removal).
pub struct TxDraft {
    session: Arc<NetworkSession>,
    wallet_id: WalletId,
    /// Binds the drafts's `PreparedTx`s to it.
    id: u64,
    state: Mutex<DraftState>,
    /// Stands in for the library's broadcast, so the leased hand-off runs
    /// end to end without SPV (E0-04 L5, review P2a r1 F3).
    #[cfg(test)]
    pub(crate) broadcaster: Mutex<Option<Arc<dyn TestBroadcaster>>>,
}

/// The library's broadcast, faked (tests).
#[cfg(test)]
pub(crate) trait TestBroadcaster: Send + Sync {
    /// A wait before the library is entered (the manager guard, the SPV
    /// configuration); the permit's deadline bounds it.
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;
    /// The library's hand-off of `tx`, entered: its result.
    fn broadcast(
        &self,
        tx: &Transaction,
        first: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<dashcore::Txid, PlatformWalletError>> + Send>>;
}

impl std::fmt::Debug for TxDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxDraft")
            .field("wallet_id", &self.wallet_id)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What a draft resolved to against the current wallet state.
struct Resolved {
    plan: Plan,
    recipients: Vec<ValidRecipient>,
    change: ChangeTarget,
    fee_rate: FeeRate,
    /// The wallet's processed height the plan was made at (coinbase
    /// maturity in key-wallet's selector).
    height: u32,
    /// Funded by fully mixed CoinJoin coins (no change; signed by the
    /// engine, not key-wallet's builder, which would add change).
    coinjoin: bool,
}

impl Resolved {
    /// key-wallet's builder for exactly this plan: the planned inputs, the
    /// recipient outputs and the change as an explicit output paying
    /// `change`. The builder always budgets a change output; without change
    /// any P2PKH address keeps its estimate equal to the plan's.
    fn builder(&self, change: &Address) -> TransactionBuilder {
        let mut builder = TransactionBuilder::new()
            .set_fee_rate(self.fee_rate)
            .set_current_height(self.height)
            .set_selection_strategy(SelectionStrategy::LargestFirst)
            .set_change_address(change.clone())
            .add_inputs(self.plan.inputs.clone());
        for (rc, amount) in self.recipients.iter().zip(&self.plan.amounts) {
            builder = builder.add_output(&rc.address, *amount);
        }
        if let Some(value) = self.plan.change {
            builder = builder.add_output(change, value);
        }
        builder
    }

    /// Builds the plan unsigned with no reservation set attached (nothing is
    /// reserved) and checks that key-wallet keeps it: its selector leaves
    /// out a planned input worth no more than its own input fee, which coin
    /// control must not do. Runs before a grant is redeemed, so such a plan
    /// fails without consuming the grant. The change script is the custom
    /// change address, or for `Auto` the first input's P2PKH address, which
    /// has the length of the fresh change address `prepare` derives.
    fn dry_run(&self) -> Result<(), EngineError> {
        if self.coinjoin {
            return Ok(());
        }
        let change = match (&self.change, self.plan.inputs.first()) {
            (ChangeTarget::Address(a), _) => a.clone(),
            (ChangeTarget::Auto, Some(first)) => first.address.clone(),
            (ChangeTarget::Auto, None) => return Ok(()),
        };
        let (tx, _, _) = self
            .builder(&change)
            .build_unsigned_reserved()
            .map_err(|e| EngineError::Internal(format!("key-wallet cannot build the plan: {e}")))?;
        check_against_plan(&tx, &self.plan, &change).map_err(Mismatch::into_error)
    }
}

impl NetworkSession {
    /// A new draft for `wallet_id`: no recipients, source `Any`, fee
    /// `Recommended{6}`, change `Auto`.
    pub fn new_tx_draft(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<Arc<TxDraft>, EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        if manager.get_wallet_blocking(&wallet_id.0).is_none() {
            return Err(EngineError::WalletNotFound(wallet_id.to_string()));
        }
        Ok(Arc::new(TxDraft {
            session: Arc::clone(self),
            wallet_id,
            id: NEXT_DRAFT_ID.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(DraftState {
                recipients: Vec::new(),
                source: CoinSource::Any,
                fee: FeeRate::new(MIN_FEE_PER_KB),
                change: ChangeTarget::Auto,
            }),
            #[cfg(test)]
            broadcaster: Mutex::new(None),
        }))
    }

    /// The spendable amount of `source`: the sum dash-qt's "Use available
    /// balance" starts from (review M-4). Hosts set a recipient to this
    /// minus the other recipients' amounts with `subtract_fee_from_amount`,
    /// so the fee comes out of it whatever the rate. `fee` is validated.
    /// Excludes user-locked, reserved, immature and untrusted unconfirmed
    /// coins.
    pub async fn max_spendable(
        self: &Arc<Self>,
        wallet_id: WalletId,
        source: CoinSource,
        fee: FeeMode,
    ) -> Result<u64, EngineError> {
        fee.rate()?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let (coins, _) = this.source_coins(wallet_id, &source).await?;
            Ok(coins.iter().map(Utxo::value).sum())
        })
        .await
    }

    /// The coins `source` offers and the wallet height they were read at.
    async fn source_coins(
        &self,
        wallet_id: WalletId,
        source: &CoinSource,
    ) -> Result<(Vec<Utxo>, u32), EngineError> {
        if *source == CoinSource::FullyMixedOnly {
            let view = self.mix_view(wallet_id).await?;
            return Ok((crate::coinjoin::fully_mixed_candidates(&view), view.height));
        }
        let wallet = self.wallet(&wallet_id).await?;
        let snapshot = self.coin_snapshot(&wallet, wallet_id).await?;
        Ok((candidates(&snapshot, source)?, snapshot.height))
    }

    pub(crate) async fn wallet(
        &self,
        wallet_id: &WalletId,
    ) -> Result<Arc<PlatformWallet>, EngineError> {
        self.manager()?
            .get_wallet(&wallet_id.0)
            .await
            .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))
    }
}

/// The coins `source` offers to a build.
fn candidates(snapshot: &CoinSnapshot, source: &CoinSource) -> Result<Vec<Utxo>, EngineError> {
    match source {
        CoinSource::Any => Ok(snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            .map(|c| c.utxo.clone())
            .collect()),
        // Read from the mixing view by `NetworkSession::source_coins`.
        CoinSource::FullyMixedOnly => Ok(Vec::new()),
        CoinSource::Outpoints(outpoints) => outpoints
            .iter()
            .map(|o| {
                snapshot
                    .coins
                    .iter()
                    .find(|c| c.utxo.outpoint == *o)
                    .filter(|c| c.send_account && c.chosen_selectable(snapshot.height))
                    .map(|c| c.utxo.clone())
                    .ok_or_else(|| SendFailure::OutpointUnavailable(*o).into())
            })
            .collect(),
    }
}

impl TxDraft {
    pub fn wallet_id(&self) -> WalletId {
        self.wallet_id
    }

    fn state(&self) -> std::sync::MutexGuard<'_, DraftState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn network(&self) -> dashcore::Network {
        self.session.network.core_network()
    }

    /// Replaces the recipients after validating addresses (network, Platform
    /// rejection), amounts (non-zero, ≤ 21M DASH, not dust) and duplicates.
    pub fn set_recipients(&self, recipients: Vec<Recipient>) -> Result<(), EngineError> {
        let valid = validate_recipients(recipients, self.network())?;
        self.state().recipients = valid;
        Ok(())
    }

    /// Sets the coin source. Outpoints must be distinct; whether they are
    /// spendable is checked by `estimate` and `prepare`.
    pub fn set_source(&self, source: CoinSource) -> Result<(), EngineError> {
        match &source {
            CoinSource::FullyMixedOnly => {}
            CoinSource::Outpoints(list) => {
                if list.is_empty() {
                    return Err(EngineError::InvalidArgument("no outpoints selected".into()));
                }
                let mut seen = HashSet::new();
                if let Some(dup) = list.iter().find(|o| !seen.insert(**o)) {
                    return Err(EngineError::InvalidArgument(format!(
                        "outpoint {dup} listed twice"
                    )));
                }
            }
            CoinSource::Any => {}
        }
        self.state().source = source;
        Ok(())
    }

    pub fn set_fee(&self, fee: FeeMode) -> Result<(), EngineError> {
        let rate = fee.rate()?;
        self.state().fee = rate;
        Ok(())
    }

    pub fn set_change(&self, change: ChangePolicy) -> Result<(), EngineError> {
        let target = match change {
            ChangePolicy::Auto => ChangeTarget::Auto,
            ChangePolicy::Address(text) => ChangeTarget::Address(
                l1_address(&text, self.network()).map_err(|_| SendFailure::InvalidChangeAddress)?,
            ),
        };
        self.state().change = target;
        Ok(())
    }

    /// Plans the draft against the wallet's current coins.
    async fn resolve(&self) -> Result<Resolved, EngineError> {
        let state = self.state().clone();
        if state.recipients.is_empty() {
            return Err(SendFailure::NoRecipients.into());
        }
        let coinjoin = state.source == CoinSource::FullyMixedOnly;
        if coinjoin && !matches!(state.change, ChangeTarget::Auto) {
            return Err(EngineError::InvalidArgument(
                "the CoinJoin source pays its change as fee; only the automatic change policy applies"
                    .into(),
            ));
        }
        let (coins, height) = self
            .session
            .source_coins(self.wallet_id, &state.source)
            .await?;
        let outputs: Vec<PlanOutput> = state
            .recipients
            .iter()
            .map(|r| PlanOutput {
                script: r.address.script_pubkey(),
                amount: r.amount,
                subtract_fee: r.subtract_fee,
            })
            .collect();
        let change_len = match &state.change {
            // BIP44 change addresses are P2PKH.
            ChangeTarget::Auto => 25,
            ChangeTarget::Address(a) => a.script_pubkey().len(),
        };
        let choice = match state.source {
            CoinSource::Outpoints(_) => InputChoice::UseAll(&coins),
            _ => InputChoice::Select(&coins),
        };
        let mut plan =
            plan::plan(choice, &outputs, state.fee, change_len, height).map_err(|e| match e {
                // The CoinJoin page's own message (dash-qt "Insufficient
                // mixed funds").
                PlanError::AmountExceedsBalance { available }
                | PlanError::AmountWithFeeExceedsBalance { available, .. }
                    if coinjoin =>
                {
                    SendFailure::InsufficientMixedFunds { available }
                }
                e => SendFailure::from(e),
            })?;
        if coinjoin && let Some(change) = plan.change.take() {
            // dash-qt's CoinJoin page: no change output, the rest is fee
            // (`CoinType::ONLY_FULLY_MIXED`, wallet/spend.cpp:980-995).
            plan.fee += change;
            plan.estimated_size -= plan::output_size(change_len);
        }
        if plan.fee > MAX_TX_FEE {
            return Err(SendFailure::AbsurdFee { fee: plan.fee }.into());
        }
        let resolved = Resolved {
            plan,
            recipients: state.recipients,
            change: state.change,
            fee_rate: state.fee,
            height,
            coinjoin,
        };
        resolved.dry_run()?;
        Ok(resolved)
    }

    /// Coin selection and fee for the current draft. Nothing is signed or
    /// reserved.
    pub async fn estimate(self: &Arc<Self>) -> Result<TxEstimate, EngineError> {
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move {
                let _op = this.session.enter().await?;
                let r = this.resolve().await?;
                Ok(TxEstimate {
                    fee: r.plan.fee,
                    size_bytes: r.plan.estimated_size as u32,
                    input_count: r.plan.inputs.len() as u32,
                    change: r.plan.change,
                    total_sent: r.plan.total_sent(),
                })
            })
            .await
    }

    /// Plans, signs through the vault with a `Spend` grant and reserves the
    /// inputs. Never broadcasts. The grant is redeemed only after the plan
    /// and an unsigned dry-run build of it succeeded, so a balance error or
    /// an unbuildable coin-control choice does not consume it.
    ///
    /// `grant_id` may also name a lease (E0-04 §16.1, "Accept and pay"):
    /// the payment is then charged to the lease's Spend budget before it is
    /// signed, the charge is bound to its txid, and `broadcast` hands it
    /// off through the dispatch fence, so a lock before the hand-off
    /// cancels it (`send.cancelled`).
    pub async fn prepare(
        self: &Arc<Self>,
        grant_id: String,
    ) -> Result<Arc<PreparedTx>, EngineError> {
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move {
                let _op = this.session.enter().await?;
                this.prepare_inner(grant_id).await
            })
            .await
    }

    async fn prepare_inner(
        self: &Arc<Self>,
        grant_id: String,
    ) -> Result<Arc<PreparedTx>, EngineError> {
        let session = &self.session;
        let wallet_id = self.wallet_id;
        if !session.vault.has_wallet_secret(&wallet_id.0) {
            return Err(SendFailure::WatchOnly.into());
        }
        let lease = session
            .lease_for(&wallet_id, &grant_id)
            .transpose()
            .map_err(|e| lease_failure(e, 0))?;
        if let Some(lease) = &lease {
            if !lease.has_spend() {
                return Err(SendFailure::GrantInvalid.into());
            }
            lease.spend_signer().map_err(|e| lease_failure(e, 0))?;
            return self.prepare_leased(lease.clone()).await;
        }
        // No key to sign with (locked or mixing-only vault, and the grant
        // does not carry its own key) fails before the plan. A grant that is
        // unknown, expired, of another purpose or for another wallet fails
        // after it, when it is redeemed, so plan errors are reported first.
        if let Err(e @ (VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly)) =
            session
                .vault
                .check_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))
        {
            return Err(vault_failure(e));
        }
        let r = self.resolve().await?;
        let wallet = session.wallet(&wallet_id).await?;
        let (recipient_mine, change_mine, external_sent) = self.outflow_of(&r, &wallet).await;

        // What the grant caps: everything that leaves the wallet, fee
        // included (the same outflow `sign_psbt` caps).
        let outflow = external_sent.saturating_add(r.plan.fee);
        let vault = session.vault.clone();
        let signer = tokio::task::spawn_blocking(move || {
            let token = vault.redeem_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))?;
            let max_duffs = token.max_duffs().unwrap_or(0);
            if outflow > max_duffs {
                return Ok(Err(SendFailure::GrantExceeded { max_duffs, outflow }));
            }
            vault.signer(&wallet_id.0, &token).map(Ok)
        })
        .await?
        .map_err(vault_failure)??;
        self.sign_and_reserve(
            r,
            wallet,
            recipient_mine,
            change_mine,
            external_sent,
            signer,
            None,
        )
        .await
    }

    /// `prepare` under a lease: the plan, then the Spend charge before
    /// signing (E0-04 §4.2: "a lease cannot sign a spend above its cap").
    async fn prepare_leased(
        self: &Arc<Self>,
        lease: Lease,
    ) -> Result<Arc<PreparedTx>, EngineError> {
        let r = self.resolve().await?;
        let wallet = self.session.wallet(&self.wallet_id).await?;
        let (recipient_mine, change_mine, external_sent) = self.outflow_of(&r, &wallet).await;
        let outflow = external_sent.saturating_add(r.plan.fee);
        // One J step: a payment exactly at the cap is charged and signs.
        let (charge, signer) = lease
            .charge_spend_signer(outflow)
            .map_err(|e| lease_failure(e, outflow))?;
        let charge = SpendCharge {
            lease: lease.clone(),
            charge: Some(charge),
        };
        self.sign_and_reserve(
            r,
            wallet,
            recipient_mine,
            change_mine,
            external_sent,
            signer,
            Some(charge),
        )
        .await
    }

    /// Which recipients and change the wallet owns, and what leaves it:
    /// the outputs to scripts it does not own.
    async fn outflow_of(&self, r: &Resolved, wallet: &PlatformWallet) -> (Vec<bool>, bool, u64) {
        let (recipient_mine, change_mine) = {
            let state = wallet.state().await;
            let owns = |a: &Address| crate::coins::wallet_owns(&state.core_wallet, a);
            let recipients: Vec<bool> = r.recipients.iter().map(|rc| owns(&rc.address)).collect();
            let change = match &r.change {
                ChangeTarget::Auto => true,
                ChangeTarget::Address(a) => owns(a),
            };
            (recipients, change)
        };
        let external_sent = r
            .plan
            .amounts
            .iter()
            .zip(&recipient_mine)
            .filter(|(_, mine)| !**mine)
            .map(|(a, _)| *a)
            .sum::<u64>()
            + if change_mine {
                0
            } else {
                r.plan.change.unwrap_or(0)
            };
        (recipient_mine, change_mine, external_sent)
    }

    /// Signs the plan with `signer`, checks it and reserves its inputs.
    /// A lease's `charge` is bound to the txid once signed.
    #[expect(
        clippy::too_many_arguments,
        reason = "the two prepare paths' shared tail"
    )]
    async fn sign_and_reserve(
        self: &Arc<Self>,
        r: Resolved,
        wallet: Arc<PlatformWallet>,
        recipient_mine: Vec<bool>,
        change_mine: bool,
        external_sent: u64,
        signer: dw_vault::VaultSigner,
        charge: Option<SpendCharge>,
    ) -> Result<Arc<PreparedTx>, EngineError> {
        let session = &self.session;
        let wallet_id = self.wallet_id;
        let network = self.network();
        let lease = charge.as_ref().map(|c| c.lease.clone());

        if r.coinjoin {
            let outputs: Vec<TxOut> = r
                .recipients
                .iter()
                .zip(&r.plan.amounts)
                .map(|(rc, amount)| TxOut {
                    value: *amount,
                    script_pubkey: rc.address.script_pubkey(),
                })
                .collect();
            let tx = session
                .sign_coinjoin_payment(wallet_id, &r.plan.inputs, outputs, &signer)
                .await?;
            let first_input = r.plan.inputs[0].address.script_pubkey();
            let summary = summarize(
                &tx,
                &r,
                &recipient_mine,
                change_mine,
                first_input,
                external_sent,
                network,
            );
            let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
            session.spends.add(wallet_id, inputs.iter().copied());
            let artifact = ArtifactId::from_txid(&tx.txid());
            if let Some(charge) = charge {
                charge.bind(artifact);
            }
            return Ok(Arc::new(PreparedTx {
                session: Arc::clone(session),
                wallet_id,
                draft_id: self.id,
                summary,
                records: r
                    .recipients
                    .iter()
                    .map(|rc| (rc.address.to_string(), rc.label.clone(), rc.message.clone()))
                    .collect(),
                inputs,
                signed: Mutex::new(None),
                mixed: Some(tx),
                phase: Mutex::new(Phase::Pending),
                lease: lease.map(|l| (l, artifact)),
            }));
        }

        // The change address: fresh only when the plan has change. The
        // builder always budgets one; without change any P2PKH of the same
        // length (the first input's) keeps its estimate equal to the plan's.
        let change_address = match (&r.change, r.plan.change) {
            (ChangeTarget::Address(a), _) => a.clone(),
            (ChangeTarget::Auto, Some(_)) => {
                wallet.core().next_change_address_for_account(0).await?
            }
            (ChangeTarget::Auto, None) => r.plan.inputs[0].address.clone(),
        };
        let signed = wallet
            .core()
            .finalize_transaction_with_options(
                r.builder(&change_address),
                &SEND_FUNDING_SOURCES,
                0,
                &signer,
                true,
            )
            .await
            .map_err(finalize_failure)?;

        if let Err(detail) = check_against_plan(signed.transaction(), &r.plan, &change_address) {
            wallet.core().abandon_transaction(&signed).await;
            return Err(detail.into_error());
        }

        let summary = summarize(
            signed.transaction(),
            &r,
            &recipient_mine,
            change_mine,
            change_address.script_pubkey(),
            external_sent,
            network,
        );
        let inputs: Vec<OutPoint> = signed
            .transaction()
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect();
        session.spends.add(wallet_id, inputs.iter().copied());
        let artifact = ArtifactId::from_txid(&signed.transaction().txid());
        if let Some(charge) = charge {
            charge.bind(artifact);
        }
        Ok(Arc::new(PreparedTx {
            session: Arc::clone(session),
            wallet_id,
            draft_id: self.id,
            summary,
            records: r
                .recipients
                .iter()
                .map(|rc| (rc.address.to_string(), rc.label.clone(), rc.message.clone()))
                .collect(),
            inputs,
            signed: Mutex::new(Some(Arc::new(signed))),
            mixed: None,
            phase: Mutex::new(Phase::Pending),
            lease: lease.map(|l| (l, artifact)),
        }))
    }

    fn check_mine(&self, prepared: &PreparedTx) -> Result<(), EngineError> {
        if prepared.draft_id != self.id {
            return Err(EngineError::InvalidArgument(
                "the prepared transaction belongs to another draft".into(),
            ));
        }
        Ok(())
    }

    /// Announces `prepared` and waits for dash-spv's acceptance verdict.
    ///
    /// First broadcast of a pending transaction:
    /// - accepted: `Ok`, the transaction is done;
    /// - never sent (SPV stopped, no peers, rejected before dispatch):
    ///   `NoPeers` / `BroadcastRejected`; the inputs are released (as
    ///   platform-wallet releases key-wallet's reservation on such a
    ///   rejection, for an immediate rebuild) and the prepared transaction is
    ///   spent, so the host prepares again with a new grant;
    /// - outcome unknown: `BroadcastUnknown`; the inputs stay reserved.
    ///
    /// Once a broadcast ended `BroadcastUnknown` the transaction may be on
    /// the network, so it is never released again (review M3): `broadcast`
    /// may be called again with the same handle (same transaction, same
    /// txid), and every outcome of such a repeat other than acceptance is
    /// `BroadcastUnknown`, its reason saying what this attempt saw (no
    /// peers, rejected before dispatch, no verdict). `abandon` is refused.
    /// The inputs stay reserved until the wallet sees them spent, by this
    /// transaction or a conflicting one (see [`PendingSpends`]), or the
    /// session closes.
    ///
    /// The payment's message and address-book entries are written once the
    /// broadcast was accepted or its outcome is unknown, never for a payment
    /// that was not sent (review M7).
    pub async fn broadcast(
        self: &Arc<Self>,
        prepared: Arc<PreparedTx>,
    ) -> Result<BroadcastOutcome, EngineError> {
        self.check_mine(&prepared)?;
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move {
                let _op = this.session.enter().await?;
                this.broadcast_inner(prepared).await
            })
            .await
    }

    async fn broadcast_inner(
        &self,
        prepared: Arc<PreparedTx>,
    ) -> Result<BroadcastOutcome, EngineError> {
        let first = {
            let mut phase = prepared.phase();
            let first = match *phase {
                Phase::Pending => true,
                Phase::Unknown => false,
                Phase::Broadcasting | Phase::Sent | Phase::Released => {
                    return Err(SendFailure::PreparedTxSpent.into());
                }
            };
            *phase = Phase::Broadcasting;
            first
        };
        let (outcome, next) = match &prepared.lease {
            Some((lease, artifact)) => self.fenced(&prepared, lease, *artifact, first).await,
            None => {
                let outcome = self.dispatch(&prepared, first, None).await;
                let next = settle(first, &outcome);
                (outcome, next)
            }
        };
        *prepared.phase() = next;
        match next {
            Phase::Sent => self
                .session
                .spends
                .remove(&self.wallet_id, prepared.inputs.iter().copied()),
            // Never sent: release key-wallet's reservation (owner-guarded, a
            // no-op where platform-wallet already did) and the engine's.
            Phase::Released => prepared.release(&self.session).await,
            Phase::Pending | Phase::Unknown | Phase::Broadcasting => {}
        }
        let dispatched = matches!(
            outcome,
            Ok(_) | Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))
        );
        if dispatched {
            if let Ok(txid) = prepared.summary.txid.parse::<dashcore::Txid>() {
                self.session.hub.note_announced(txid);
            }
            if let Err(e) = self
                .session
                .record_send_metadata(self.wallet_id, &prepared)
                .await
            {
                // The payment went out; losing its message or address-book
                // entries must not turn that into an error.
                tracing::warn!(error = %e, txid = %prepared.summary.txid,
                    "storing the payment's message and address-book entries failed");
            }
            // The spent coins and the new transaction change what the
            // wallet shows: re-read its state and let the pump announce it.
            if let Ok(manager) = self.session.manager() {
                self.session
                    .refresh_wallet_state(&manager, self.wallet_id)
                    .await;
            }
            self.session.hub.pump.mark_balances(self.wallet_id);
            self.session.hub.pump.mark_history(self.wallet_id, None);
        }
        outcome
    }

    /// A leased payment's hand-off through the dispatch fence (E0-04 §5.4,
    /// §5.5): the fence decides First, Resend or refusal, and the release
    /// follows the artifact's settlement, never this attempt alone. A lock
    /// before the hand-off is `send.cancelled`.
    async fn fenced(
        &self,
        prepared: &PreparedTx,
        lease: &Lease,
        artifact: ArtifactId,
        first: bool,
    ) -> (Result<BroadcastOutcome, EngineError>, Phase) {
        let table = std::sync::Arc::clone(&lease.table);
        let wallet = self.wallet_id;
        let verdict = lease
            .scope(async move {
                table
                    .admit(AdmitRequest {
                        wallet,
                        artifact,
                        kind: ArtifactKind::CoreTx { asset_lock: false },
                        tracked_row: false,
                        scope: DispatchScope::current(),
                    })
                    .await
            })
            .await;
        let unknown = |reason: String| -> (Result<BroadcastOutcome, EngineError>, Phase) {
            (
                Err(SendFailure::BroadcastUnknown { reason }.into()),
                Phase::Unknown,
            )
        };
        let (result, settlement) = match verdict {
            Verdict::First(permit) => {
                // L5: the transport only before the permit's deadline, and
                // under it (`dispatch`).
                let result = self
                    .dispatch(prepared, first, Some(permit.deadline()))
                    .await;
                let s = permit.finish(attempt_outcome(&result));
                (result, s)
            }
            Verdict::Resend(guard) | Verdict::FirstUnleased(guard) => {
                let result = self.dispatch(prepared, false, None).await;
                let s = guard.finish(attempt_outcome(&result));
                (result, s)
            }
            Verdict::Refused { .. } => {
                return (Err(SendFailure::Cancelled.into()), Phase::Released);
            }
            Verdict::Deferred => return unknown("the hand-off was deferred".into()),
        };
        match settlement {
            Settlement::Sent => (
                result.or_else(|_| {
                    Ok(BroadcastOutcome {
                        txid: artifact.to_string(),
                    })
                }),
                Phase::Sent,
            ),
            Settlement::DefinitelyUnsent => (result, Phase::Released),
            Settlement::MaybeOut => match result {
                Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => {
                    (result, Phase::Unknown)
                }
                Err(e) => unknown(format!("another attempt may have sent it: {e}")),
                Ok(_) => unknown("accepted, but recorded as unknown".into()),
            },
        }
    }

    /// One broadcast attempt. `first`: the transaction was never handed to
    /// the network before. A repeat goes through platform-wallet's plain
    /// broadcast: its inputs are already held by the first dispatch's
    /// pending-spend fence, and re-sending the same transaction cannot spend
    /// them twice, so the finalized handle's reservation-age guard (which
    /// would refuse a repeat hours later) does not apply.
    ///
    /// `deadline`: a leased First's permit deadline (E0-04 L5). Every wait
    /// before the library is entered is bounded by it, and the library is
    /// entered only before it: a First past its deadline never dispatches
    /// (`Cancelled`, definitely unsent). The library call itself is
    /// bounded by it, checked before each poll (`LeaseTable::hand_off`);
    /// a cut is MaybeSent, and the
    /// library keeps the inputs fenced on that cancellation
    /// (`dispatch_unexpired`'s in-broadcast pin). The pinned library has
    /// no enqueue/acceptance split (L10), so a verdict later than the
    /// deadline arrives as `note_seen` instead.
    async fn dispatch(
        &self,
        prepared: &PreparedTx,
        first: bool,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<BroadcastOutcome, EngineError> {
        let ready = async {
            let wallet = self.session.wallet(&self.wallet_id).await?;
            #[cfg(test)]
            let seam = self.broadcaster.lock().unwrap().clone();
            #[cfg(test)]
            if let Some(seam) = seam {
                seam.before_entry().await;
                return Ok((wallet, Some(seam)));
            }
            let manager = self.session.manager()?;
            if !manager.spv().is_started() {
                // Nothing to send to; handled like dash-spv's never-sent
                // rejection.
                return Err(if first {
                    SendFailure::NoPeers
                } else {
                    SendFailure::BroadcastUnknown {
                        reason: "SPV is not running".into(),
                    }
                }
                .into());
            }
            Ok::<_, EngineError>((wallet, None))
        };
        // `Phase::Broadcasting` keeps abandon and drop away while the
        // broadcast awaits the network (up to about a minute).
        let signed = prepared.signed().clone();
        let handed = self
            .session
            .leases
            .hand_off(deadline, ready, |(wallet, seam)| async move {
                let tx = match (&prepared.mixed, &signed) {
                    (Some(tx), _) => tx,
                    (None, Some(signed)) => signed.transaction(),
                    (None, None) => return None,
                };
                #[cfg(test)]
                if let Some(seam) = seam {
                    return Some(seam.broadcast(tx, first).await);
                }
                #[cfg(not(test))]
                let _: Option<()> = seam;
                Some(match (&prepared.mixed, &signed) {
                    (None, Some(signed)) if first => {
                        wallet.core().broadcast_finalized_transaction(signed).await
                    }
                    _ => wallet.core().broadcast_transaction(tx).await,
                })
            })
            .await?;
        let sent = match handed {
            HandOff::Done(sent) => sent,
            // Never entered at or past the deadline.
            HandOff::NotEntered => return Err(SendFailure::Cancelled.into()),
            HandOff::Cut => {
                return Err(SendFailure::BroadcastUnknown {
                    reason: "the lease's deadline passed during the hand-off".into(),
                }
                .into());
            }
        };
        let Some(sent) = sent else {
            return Err(SendFailure::PreparedTxSpent.into());
        };
        match sent {
            Ok(txid) => Ok(BroadcastOutcome {
                txid: txid.to_string(),
            }),
            Err(PlatformWalletError::TransactionBroadcastUnconfirmed(reason)) => {
                Err(SendFailure::BroadcastUnknown { reason }.into())
            }
            // Whatever a repeat ran into, the first dispatch may have
            // reached the network.
            Err(other) if !first => Err(SendFailure::BroadcastUnknown {
                reason: format!("not sent this time: {other}"),
            }
            .into()),
            Err(PlatformWalletError::TransactionBroadcast(reason)) => {
                Err(if reason_means_no_peers(&reason) {
                    SendFailure::NoPeers
                } else {
                    SendFailure::BroadcastRejected { reason }
                }
                .into())
            }
            Err(PlatformWalletError::StaleReservation) => Err(SendFailure::PreparedTxSpent.into()),
            Err(other) => Err(other.into()),
        }
    }

    /// Releases the inputs of `prepared`. Idempotent for a pending or
    /// released transaction; `PreparedTxSpent` once it was handed to the
    /// network (sent or outcome unknown).
    pub async fn abandon(self: &Arc<Self>, prepared: Arc<PreparedTx>) -> Result<(), EngineError> {
        self.check_mine(&prepared)?;
        let session = Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let _op = session.enter().await?;
                {
                    let mut phase = prepared.phase();
                    match *phase {
                        Phase::Pending => *phase = Phase::Released,
                        Phase::Released => return Ok(()),
                        Phase::Broadcasting | Phase::Sent | Phase::Unknown => {
                            return Err(SendFailure::PreparedTxSpent.into());
                        }
                    }
                }
                prepared.release(&session).await;
                Ok(())
            })
            .await
    }
}

/// The phase one broadcast attempt leaves a transaction in. `first`: it had
/// never been handed to the network before this attempt. A transaction that
/// may be on the network (a previous outcome was unknown) is never released.
fn settle(first: bool, outcome: &Result<BroadcastOutcome, EngineError>) -> Phase {
    match outcome {
        Ok(_) => Phase::Sent,
        Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => Phase::Unknown,
        Err(EngineError::Send(
            SendFailure::NoPeers
            | SendFailure::BroadcastRejected { .. }
            | SendFailure::PreparedTxSpent,
        )) if first => Phase::Released,
        // Not dispatched (session closed, wallet gone): as before.
        Err(_) if first => Phase::Pending,
        Err(_) => Phase::Unknown,
    }
}

/// What one attempt says about its own bytes (E0-04 §5.5): only a
/// rejection before the network took them is a definite `NotSent`.
fn attempt_outcome(result: &Result<BroadcastOutcome, EngineError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Sent,
        // `Cancelled`: a First past its deadline, never dispatched (L5).
        Err(EngineError::Send(
            SendFailure::NoPeers
            | SendFailure::BroadcastRejected { .. }
            | SendFailure::PreparedTxSpent
            | SendFailure::Cancelled,
        ))
        | Err(EngineError::NetworkNotOpen(_) | EngineError::WalletNotFound(_)) => Outcome::NotSent,
        Err(_) => Outcome::MaybeSent,
    }
}

/// dash-spv's never-sent rejections: client not started, no connected peer.
fn reason_means_no_peers(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("not started")
        || r.contains("not connected")
        || r.contains("notconnected")
        || r.contains("no peer")
}

fn finalize_failure(e: PlatformWalletError) -> EngineError {
    match e {
        PlatformWalletError::InputMidBroadcast { outpoint } => {
            SendFailure::OutpointUnavailable(outpoint).into()
        }
        PlatformWalletError::CoreInsufficientFunds { available, .. }
        | PlatformWalletError::CorePooledInsufficientFunds { available, .. } => {
            SendFailure::AmountExceedsBalance {
                available: available.unwrap_or(0),
            }
            .into()
        }
        other => other.into(),
    }
}

enum Mismatch {
    MissingInput(OutPoint),
    Other(String),
}

impl Mismatch {
    fn into_error(self) -> EngineError {
        match self {
            Mismatch::MissingInput(o) => SendFailure::OutpointUnavailable(o).into(),
            Mismatch::Other(d) => {
                EngineError::Internal(format!("built transaction differs from the plan: {d}"))
            }
        }
    }
}

/// Checks that key-wallet built exactly the planned transaction.
fn check_against_plan(tx: &Transaction, plan: &Plan, change: &Address) -> Result<(), Mismatch> {
    let spent: HashSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    if let Some(missing) = plan.inputs.iter().find(|u| !spent.contains(&u.outpoint)) {
        return Err(Mismatch::MissingInput(missing.outpoint));
    }
    if tx.input.len() != plan.inputs.len() {
        return Err(Mismatch::Other(format!(
            "{} inputs instead of {}",
            tx.input.len(),
            plan.inputs.len()
        )));
    }
    let mut got: Vec<u64> = tx.output.iter().map(|o| o.value).collect();
    let mut want: Vec<u64> = plan.amounts.iter().copied().chain(plan.change).collect();
    got.sort_unstable();
    want.sort_unstable();
    if got != want {
        return Err(Mismatch::Other(format!(
            "outputs {got:?} instead of {want:?}"
        )));
    }
    let fee = plan.total_in() - tx.output.iter().map(|o| o.value).sum::<u64>();
    if fee != plan.fee {
        return Err(Mismatch::Other(format!(
            "fee {fee} instead of {}",
            plan.fee
        )));
    }
    if let Some(value) = plan.change {
        let script = change.script_pubkey();
        if !tx
            .output
            .iter()
            .any(|o| o.value == value && o.script_pubkey == script)
        {
            return Err(Mismatch::Other("change output missing".into()));
        }
    }
    Ok(())
}

fn summarize(
    tx: &Transaction,
    r: &Resolved,
    recipient_mine: &[bool],
    change_mine: bool,
    change_script: ScriptBuf,
    external_sent: u64,
    network: dashcore::Network,
) -> PreparedSummary {
    let size = dashcore::consensus::serialize(tx).len();
    let by_outpoint: HashMap<OutPoint, &Utxo> =
        r.plan.inputs.iter().map(|u| (u.outpoint, u)).collect();
    let inputs = tx
        .input
        .iter()
        .map(|i| {
            let utxo = by_outpoint.get(&i.previous_output);
            PreparedInput {
                outpoint: i.previous_output,
                address: utxo.map(|u| u.address.to_string()),
                amount: utxo.map(|u| u.value()).unwrap_or(0),
            }
        })
        .collect();
    // Match outputs to recipients by (script, amount); the rest is change.
    let mut unmatched: Vec<Option<usize>> = (0..r.recipients.len()).map(Some).collect();
    let mut owned_out = 0u64;
    let outputs = tx
        .output
        .iter()
        .map(|o| {
            let recipient = unmatched.iter_mut().find_map(|slot| {
                let i = (*slot)?;
                (r.recipients[i].address.script_pubkey() == o.script_pubkey
                    && r.plan.amounts[i] == o.value)
                    .then(|| slot.take())
                    .flatten()
            });
            let (is_change, is_mine, label) = match recipient {
                Some(i) => (false, recipient_mine[i], r.recipients[i].label.clone()),
                None => (o.script_pubkey == change_script, change_mine, None),
            };
            if is_mine {
                owned_out += o.value;
            }
            PreparedOutput {
                address: Address::from_script(&o.script_pubkey, network)
                    .ok()
                    .map(|a| a.to_string()),
                amount: o.value,
                is_change,
                is_mine,
                label,
            }
        })
        .collect();
    let fee = r.plan.fee;
    PreparedSummary {
        txid: tx.txid().to_string(),
        fee,
        fee_rate_per_kb: fee.saturating_mul(1000) / size.max(1) as u64,
        size_bytes: size as u32,
        inputs,
        outputs,
        total_sent: r.plan.total_sent(),
        external_sent,
        total_debit: r.plan.total_in().saturating_sub(owned_out),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Signed, inputs reserved, not handed to the network.
    Pending,
    /// A broadcast is running.
    Broadcasting,
    /// Accepted by the network.
    Sent,
    /// Handed to the network without an acceptance verdict.
    Unknown,
    /// Abandoned or never sent; inputs released.
    Released,
}

/// A signed transaction with its inputs reserved. Only
/// [`TxDraft::broadcast`] sends it; [`TxDraft::abandon`] or dropping it
/// while pending releases the inputs.
pub struct PreparedTx {
    session: Arc<NetworkSession>,
    wallet_id: WalletId,
    draft_id: u64,
    summary: PreparedSummary,
    /// (address, label, message) per recipient, written at broadcast.
    records: Vec<(String, Option<String>, Option<String>)>,
    inputs: Vec<OutPoint>,
    signed: Mutex<Option<Arc<SignedCoreTransaction>>>,
    /// A CoinJoin-page payment (QT-051), signed by the engine: no
    /// key-wallet reservation, only the engine's pending spends.
    mixed: Option<Transaction>,
    phase: Mutex<Phase>,
    /// Prepared under a lease: its hand-offs go through the dispatch
    /// fence, scoped to the lease, as this artifact (E0-04 §5.4).
    lease: Option<(Lease, ArtifactId)>,
}

impl std::fmt::Debug for PreparedTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedTx")
            .field("txid", &self.summary.txid)
            .field("phase", &*self.phase())
            .finish_non_exhaustive()
    }
}

impl PreparedTx {
    pub fn summary(&self) -> &PreparedSummary {
        &self.summary
    }

    /// The signed transaction, consensus-encoded (hex in tests and dwcli).
    pub fn raw(&self) -> Option<Vec<u8>> {
        self.signed()
            .as_ref()
            .map(|s| dashcore::consensus::serialize(s.transaction()))
            .or_else(|| self.mixed.as_ref().map(dashcore::consensus::serialize))
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn signed(&self) -> std::sync::MutexGuard<'_, Option<Arc<SignedCoreTransaction>>> {
        self.signed.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Releases the reservation (key-wallet's, owner-guarded, and the
    /// engine's) of a transaction that will not be sent.
    async fn release(&self, session: &NetworkSession) {
        if let Some((lease, artifact)) = &self.lease {
            lease.table.unbind_spend(*artifact);
        }
        session
            .spends
            .remove(&self.wallet_id, self.inputs.iter().copied());
        let signed = self.signed().take();
        if let (Some(signed), Ok(wallet)) = (signed, session.wallet(&self.wallet_id).await) {
            wallet.core().abandon_transaction(&signed).await;
        }
    }
}

/// Dropping a pending transaction releases its inputs. Dropping one whose
/// outcome is unknown releases nothing: it may be on the network, so its
/// inputs stay reserved until the wallet sees them spent or the session
/// closes (platform-wallet's pending-spend fence holds them the same way).
impl Drop for PreparedTx {
    fn drop(&mut self) {
        if *self.phase() != Phase::Pending {
            return;
        }
        if let Some((lease, artifact)) = &self.lease {
            lease.table.unbind_spend(*artifact);
        }
        let session = Arc::clone(&self.session);
        let wallet_id = self.wallet_id;
        let inputs = std::mem::take(&mut self.inputs);
        let signed = self.signed().take();
        session.spends.remove(&wallet_id, inputs.iter().copied());
        let Some(signed) = signed else { return };
        let rt = session.rt.clone();
        rt.spawn(async move {
            // Admitted like any operation, so a close in progress is not
            // raced; after the close there is no wallet left to release in.
            let Ok(_op) = session.enter().await else {
                return;
            };
            if let Ok(wallet) = session.wallet(&wallet_id).await {
                wallet.core().abandon_transaction(&signed).await;
            }
        });
    }
}

impl NetworkSession {
    /// Stores the payment's messages and adds its recipients to the address
    /// book, as dash-qt's `WalletModel::sendCoins` does after a send, with
    /// one difference (review M7, dash-qt quirk #6): a label is never
    /// replaced. A recipient not yet listed is added (Send, or Receive for
    /// one of the wallet's own addresses) with its label, or unlabelled; a
    /// listed entry without a label gets the recipient's label; a listed
    /// entry with a label, of either purpose, is left as it is. Called once
    /// a broadcast was accepted or its outcome is unknown.
    async fn record_send_metadata(
        &self,
        wallet_id: WalletId,
        prepared: &PreparedTx,
    ) -> Result<(), EngineError> {
        let messages: Vec<&str> = prepared
            .records
            .iter()
            .filter_map(|(_, _, m)| m.as_deref())
            .collect();
        let message = (!messages.is_empty()).then(|| messages.join("\n"));
        let wallet = self.wallet(&wallet_id).await?;
        let owned: Vec<bool> = {
            let state = wallet.state().await;
            prepared
                .records
                .iter()
                .map(|(a, _, _)| {
                    Address::from_str(a)
                        .ok()
                        .map(|a| a.assume_checked())
                        .is_some_and(|a| crate::coins::wallet_owns(&state.core_wallet, &a))
                })
                .collect()
        };
        let entries: Vec<(String, Option<String>, bool)> = prepared
            .records
            .iter()
            .zip(owned)
            .map(|((a, l, _), mine)| (a.clone(), l.clone(), mine))
            .collect();
        let txid = prepared.summary.txid.clone();
        let id = wallet_id.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            if let Some(m) = &message {
                db.set_tx_message(&id, &txid, Some(m), now)?;
            }
            for (address, label, mine) in &entries {
                // The address's current label, whether or not it is listed.
                let current = db
                    .label(&id, dw_appdb::LabelKind::Address, address)?
                    .filter(|l| !l.is_empty());
                match db.book_entry(&id, address)? {
                    Some(entry) => {
                        if let (None, Some(label)) = (&current, label) {
                            db.upsert_book_entry(&id, address, entry.purpose, label, now)?;
                        }
                    }
                    None => {
                        let purpose = if *mine {
                            dw_appdb::BookPurpose::Receive
                        } else {
                            dw_appdb::BookPurpose::Send
                        };
                        let label = current.as_deref().or(label.as_deref()).unwrap_or("");
                        db.upsert_book_entry(&id, address, purpose, label, now)?;
                    }
                }
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

    fn rc(address: &str, amount: u64) -> Recipient {
        Recipient {
            address: address.into(),
            amount,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }
    }

    #[test]
    fn recipients_are_validated_with_indices() {
        let net = dashcore::Network::Regtest;
        assert_eq!(
            validate_recipients(vec![], net).unwrap_err(),
            SendFailure::NoRecipients
        );
        let e = validate_recipients(vec![rc(ADDR, 1000), rc("nonsense", 1000)], net).unwrap_err();
        assert_eq!(e, SendFailure::InvalidAddress { index: 1 });
        let e = validate_recipients(vec![rc(ADDR, 0)], net).unwrap_err();
        assert_eq!(e, SendFailure::InvalidAmount { index: 0 });
        let e = validate_recipients(vec![rc(ADDR, MAX_MONEY + 1)], net).unwrap_err();
        assert_eq!(e, SendFailure::InvalidAmount { index: 0 });
        let e = validate_recipients(vec![rc(ADDR, 545)], net).unwrap_err();
        assert_eq!(e, SendFailure::DustAmount { index: 0 });
        validate_recipients(vec![rc(ADDR, 546)], net).unwrap();
        let e = validate_recipients(vec![rc(ADDR, 1000), rc(ADDR, 2000)], net).unwrap_err();
        assert_eq!(e, SendFailure::DuplicateAddress { index: 1 });
        // A mainnet address on regtest.
        let e = validate_recipients(vec![rc("XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg", 1000)], net)
            .unwrap_err();
        assert_eq!(e, SendFailure::InvalidAddress { index: 0 });
        let other = Address::new(
            net,
            Payload::PubkeyHash(PubkeyHash::from_byte_array([5; 20])),
        );
        let e = validate_recipients(vec![rc(ADDR, MAX_MONEY), rc(&other.to_string(), 1000)], net)
            .unwrap_err();
        assert_eq!(e, SendFailure::InvalidAmount { index: 1 });
        // A Platform (DIP-18) address is rejected with its own code
        // (testdata/address_cases.json).
        let platform = "tdash1kpauhq3lu52qdjqzxtx34wr4zsxangwrg5ddkfkq";
        let e = validate_recipients(vec![rc(ADDR, 1000), rc(platform, 1000)], net).unwrap_err();
        assert_eq!(e, SendFailure::PlatformAddress { index: 1 });
    }

    #[test]
    fn fee_modes_are_bounded() {
        assert_eq!(
            FeeMode::Recommended { target_blocks: 6 }.rate().unwrap(),
            FeeRate::new(1000)
        );
        assert!(FeeMode::Recommended { target_blocks: 0 }.rate().is_err());
        assert!(
            FeeMode::Recommended {
                target_blocks: 1009
            }
            .rate()
            .is_err()
        );
        assert!(FeeMode::PerKb(999).rate().is_err());
        assert_eq!(FeeMode::PerKb(1000).rate().unwrap(), FeeRate::new(1000));
        assert!(FeeMode::PerKb(MAX_FEE_PER_KB + 1).rate().is_err());
    }

    #[test]
    fn a_transaction_that_may_be_on_the_network_is_never_released() {
        let accepted: Result<BroadcastOutcome, EngineError> = Ok(BroadcastOutcome {
            txid: "ab".repeat(32),
        });
        let unknown = || {
            Err(SendFailure::BroadcastUnknown {
                reason: "no verdict".into(),
            }
            .into())
        };
        let never_sent = [
            SendFailure::NoPeers,
            SendFailure::BroadcastRejected {
                reason: "not connected".into(),
            },
            SendFailure::PreparedTxSpent,
        ];
        let closed = || Err(EngineError::NetworkNotOpen("regtest".into()));

        // First broadcast of a pending transaction.
        assert_eq!(settle(true, &accepted), Phase::Sent);
        assert_eq!(settle(true, &unknown()), Phase::Unknown);
        for f in &never_sent {
            assert_eq!(settle(true, &Err(f.clone().into())), Phase::Released);
        }
        assert_eq!(settle(true, &closed()), Phase::Pending);
        // Review L5: errors raised before dispatch leave a first broadcast
        // pending (not dispatched); hosts report them as definite failures.
        let pre_dispatch: [fn() -> EngineError; 6] = [
            || EngineError::WalletNotFound("w".into()),
            || EngineError::InvalidArgument("other draft".into()),
            || EngineError::Wallet("w".into()),
            || EngineError::Storage("s".into()),
            || EngineError::Spv("s".into()),
            || EngineError::Io("i".into()),
        ];
        for e in pre_dispatch {
            assert_eq!(settle(true, &Err(e())), Phase::Pending, "{:?}", e());
            assert_eq!(settle(false, &Err(e())), Phase::Unknown, "{:?}", e());
        }

        // A repeat after an unknown outcome.
        assert_eq!(settle(false, &accepted), Phase::Sent);
        assert_eq!(settle(false, &unknown()), Phase::Unknown);
        for f in &never_sent {
            assert_eq!(settle(false, &Err(f.clone().into())), Phase::Unknown);
        }
        assert_eq!(settle(false, &closed()), Phase::Unknown);
    }

    #[test]
    fn pending_spends_track_per_wallet() {
        let p = PendingSpends::default();
        let w = WalletId([1; 32]);
        let o = OutPoint::new(dashcore::Txid::from_byte_array([2; 32]), 0);
        p.add(w, [o]);
        assert!(p.snapshot(&w).contains(&o));
        p.retain_unspent(&w, &HashSet::new());
        assert!(p.snapshot(&w).is_empty());
        p.add(w, [o]);
        p.remove(&w, [o]);
        assert!(p.snapshot(&w).is_empty());
    }
}
