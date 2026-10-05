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
//! Spend cap (review H-3/H-4): a `Spend{max_duffs}` grant caps
//! [`PreparedSummary::external_sent`], the value paid to scripts the wallet
//! does not own (recipients and a foreign change address). The fee is not
//! part of the cap; it is bounded by [`MAX_TX_FEE`] (`send.absurd_fee`).

pub(crate) mod plan;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, PubkeyHash, ScriptBuf, ScriptHash, Transaction};
use dw_uri::keyio::{AddressKind, Destination, classify_address};
use dw_vault::{GrantKind, LockState, VaultError};
use key_wallet::Utxo;
use key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy;
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionBuilder;
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::core::{SEND_FUNDING_SOURCES, SignedCoreTransaction};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use self::plan::{InputChoice, MAX_MONEY, Plan, PlanError, PlanOutput, dust_threshold};
use crate::coins::{CoinSnapshot, now_secs};
use crate::{EngineError, EngineEvent, NetworkSession, WalletId};

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
    /// Only fully mixed CoinJoin coins (QT-051). Needs per-coin mixing
    /// rounds, which nothing tracks yet: returns `NotImplemented`.
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
    #[error("payment of {external_sent} exceeds the grant's {max_duffs}")]
    GrantExceeded { max_duffs: u64, external_sent: u64 },
    #[error("prepared transaction is no longer pending")]
    PreparedTxSpent,
    #[error("no peers")]
    NoPeers,
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
    #[error("broadcast outcome unknown: {reason}")]
    BroadcastUnknown { reason: String },
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
    /// Paid to scripts the wallet does not own; the figure the `Spend`
    /// grant caps.
    pub external_sent: u64,
    /// `external_sent + fee`: inputs minus outputs back to the wallet.
    pub total_debit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastOutcome {
    pub txid: String,
}

/// Inputs of prepared transactions that are neither broadcast nor
/// released, per wallet. Coin listings mark them `reserved` and automatic
/// selection skips them (key-wallet keeps its own reservations private).
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

    fn add(&self, wallet: WalletId, outpoints: impl IntoIterator<Item = OutPoint>) {
        self.map().entry(wallet).or_default().extend(outpoints);
    }

    fn remove(&self, wallet: &WalletId, outpoints: impl IntoIterator<Item = OutPoint>) {
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
}

impl NetworkSession {
    /// A new draft for `wallet_id`: no recipients, source `Any`, fee
    /// `Recommended{6}`, change `Auto`.
    pub fn new_tx_draft(self: &Arc<Self>, wallet_id: WalletId) -> Result<Arc<TxDraft>, EngineError> {
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
            let wallet = this.wallet(&wallet_id).await?;
            let snapshot = this.coin_snapshot(&wallet, wallet_id).await?;
            let coins = candidates(&snapshot, &source)?;
            Ok(coins.iter().map(Utxo::value).sum())
        })
        .await
    }

    pub(crate) async fn wallet(&self, wallet_id: &WalletId) -> Result<Arc<PlatformWallet>, EngineError> {
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
        CoinSource::FullyMixedOnly => Err(EngineError::NotImplemented(
            "TxDraft source FullyMixedOnly (CoinJoin rounds are not tracked yet)".into(),
        )),
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
            CoinSource::FullyMixedOnly => {
                return Err(EngineError::NotImplemented(
                    "TxDraft.set_source(FullyMixedOnly): CoinJoin rounds are not tracked yet"
                        .into(),
                ));
            }
            CoinSource::Outpoints(list) => {
                if list.is_empty() {
                    return Err(EngineError::InvalidArgument("no outpoints selected".into()));
                }
                let mut seen = HashSet::new();
                if let Some(dup) = list.iter().find(|o| !seen.insert(**o)) {
                    return Err(EngineError::InvalidArgument(format!("outpoint {dup} listed twice")));
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
        let wallet = self.session.wallet(&self.wallet_id).await?;
        let snapshot = self.session.coin_snapshot(&wallet, self.wallet_id).await?;
        let coins = candidates(&snapshot, &state.source)?;
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
        let plan = plan::plan(choice, &outputs, state.fee, change_len, snapshot.height)
            .map_err(SendFailure::from)?;
        if plan.fee > MAX_TX_FEE {
            return Err(SendFailure::AbsurdFee { fee: plan.fee }.into());
        }
        Ok(Resolved {
            plan,
            recipients: state.recipients,
            change: state.change,
            fee_rate: state.fee,
        })
    }

    /// Coin selection and fee for the current draft. Nothing is signed or
    /// reserved.
    pub async fn estimate(self: &Arc<Self>) -> Result<TxEstimate, EngineError> {
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move {
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
    /// succeeded, so a balance error does not consume it.
    pub async fn prepare(self: &Arc<Self>, grant_id: String) -> Result<Arc<PreparedTx>, EngineError> {
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move { this.prepare_inner(grant_id).await })
            .await
    }

    async fn prepare_inner(self: &Arc<Self>, grant_id: String) -> Result<Arc<PreparedTx>, EngineError> {
        let session = &self.session;
        let wallet_id = self.wallet_id;
        if !session.vault.has_wallet_secret(&wallet_id.0) {
            return Err(SendFailure::WatchOnly.into());
        }
        match session.vault.lock_state() {
            LockState::NoVault | LockState::Locked | LockState::UnlockedMixingOnly => {
                return Err(SendFailure::VaultLocked.into());
            }
            LockState::NoKeys | LockState::Unencrypted | LockState::Unlocked => {}
        }
        let r = self.resolve().await?;
        let wallet = session.wallet(&wallet_id).await?;
        let network = self.network();

        // What leaves the wallet: outputs to scripts it does not own.
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
            + if change_mine { 0 } else { r.plan.change.unwrap_or(0) };

        let vault = session.vault.clone();
        let signer = tokio::task::spawn_blocking(move || {
            let token = vault.redeem_grant(&grant_id, GrantKind::Spend)?;
            let max_duffs = token.max_duffs().unwrap_or(0);
            if external_sent > max_duffs {
                return Ok(Err(SendFailure::GrantExceeded {
                    max_duffs,
                    external_sent,
                }));
            }
            vault.signer(&wallet_id.0, &token).map(Ok)
        })
        .await?
        .map_err(vault_failure)??;

        // The change address: fresh only when the plan has change. The
        // builder always budgets one; without change any P2PKH of the same
        // length (the first input's) keeps its estimate equal to the plan's.
        let change_address = match (&r.change, r.plan.change) {
            (ChangeTarget::Address(a), _) => a.clone(),
            (ChangeTarget::Auto, Some(_)) => wallet.core().next_change_address_for_account(0).await?,
            (ChangeTarget::Auto, None) => r.plan.inputs[0].address.clone(),
        };
        let mut builder = TransactionBuilder::new()
            .set_fee_rate(r.fee_rate)
            .set_selection_strategy(SelectionStrategy::LargestFirst)
            .set_change_address(change_address.clone())
            .add_inputs(r.plan.inputs.clone());
        for (rc, amount) in r.recipients.iter().zip(&r.plan.amounts) {
            builder = builder.add_output(&rc.address, *amount);
        }
        if let Some(change) = r.plan.change {
            builder = builder.add_output(&change_address, change);
        }
        let signed = wallet
            .core()
            .finalize_transaction_with_options(builder, &SEND_FUNDING_SOURCES, 0, &signer, true)
            .await
            .map_err(finalize_failure)?;

        if let Err(detail) = check_against_plan(signed.transaction(), &r.plan, &change_address) {
            wallet.core().abandon_transaction(&signed).await;
            return Err(match detail {
                Mismatch::MissingInput(o) => SendFailure::OutpointUnavailable(o).into(),
                Mismatch::Other(d) => {
                    EngineError::Internal(format!("built transaction differs from the plan: {d}"))
                }
            });
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
            phase: Mutex::new(Phase::Pending),
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
    /// Metadata (message, address-book labels) is written first, so the
    /// history shows it as soon as the transaction appears.
    ///
    /// - accepted: `Ok`, the transaction is done;
    /// - never sent (no peers, SPV stopped): `NoPeers` / `BroadcastRejected`;
    ///   platform-wallet released the inputs, so the prepared transaction is
    ///   spent and a new `prepare` is needed;
    /// - outcome unknown: `BroadcastUnknown`; the inputs stay reserved and
    ///   `broadcast` may be called again (same transaction, same txid), but
    ///   never `abandon` (review M-7).
    pub async fn broadcast(
        self: &Arc<Self>,
        prepared: Arc<PreparedTx>,
    ) -> Result<BroadcastOutcome, EngineError> {
        self.check_mine(&prepared)?;
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move { this.broadcast_inner(prepared).await })
            .await
    }

    async fn broadcast_inner(&self, prepared: Arc<PreparedTx>) -> Result<BroadcastOutcome, EngineError> {
        {
            let mut phase = prepared.phase();
            match *phase {
                Phase::Pending | Phase::Unknown => *phase = Phase::Broadcasting,
                _ => return Err(SendFailure::PreparedTxSpent.into()),
            }
        }
        let outcome = self.dispatch(&prepared).await;
        let mut phase = prepared.phase();
        match &outcome {
            Ok(_) => {
                *phase = Phase::Sent;
                self.session.spends.remove(&self.wallet_id, prepared.inputs.iter().copied());
            }
            Err(EngineError::Send(SendFailure::BroadcastUnknown { .. })) => *phase = Phase::Unknown,
            Err(EngineError::Send(
                SendFailure::NoPeers | SendFailure::BroadcastRejected { .. } | SendFailure::PreparedTxSpent,
            )) => {
                *phase = Phase::Released;
                self.session.spends.remove(&self.wallet_id, prepared.inputs.iter().copied());
            }
            // Not dispatched (session closed, wallet gone): still pending.
            Err(_) => *phase = Phase::Pending,
        }
        drop(phase);
        if outcome.is_ok() || matches!(outcome, Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))) {
            self.session.sink.emit(EngineEvent::WalletChanged {
                network: self.session.network.clone(),
                wallet_id: self.wallet_id,
            });
        }
        outcome
    }

    async fn dispatch(&self, prepared: &PreparedTx) -> Result<BroadcastOutcome, EngineError> {
        let wallet = self.session.wallet(&self.wallet_id).await?;
        let manager = self.session.manager()?;
        if !manager.spv().is_started() {
            // Nothing was sent; the reservation stays and the transaction
            // remains pending for a later broadcast.
            return Err(EngineError::SpvNotRunning);
        }
        self.session.record_send_metadata(self.wallet_id, prepared).await?;
        // `Phase::Broadcasting` keeps abandon and drop away while the
        // broadcast awaits the network (up to about a minute).
        let Some(signed) = prepared.signed().clone() else {
            return Err(SendFailure::PreparedTxSpent.into());
        };
        match wallet.core().broadcast_finalized_transaction(&signed).await {
            Ok(txid) => Ok(BroadcastOutcome {
                txid: txid.to_string(),
            }),
            Err(PlatformWalletError::TransactionBroadcastUnconfirmed(reason)) => {
                Err(SendFailure::BroadcastUnknown { reason }.into())
            }
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

/// dash-spv's never-sent rejections: client not started, no connected peer.
fn reason_means_no_peers(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("not started") || r.contains("not connected") || r.contains("notconnected") || r.contains("no peer")
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
        return Err(Mismatch::Other(format!("outputs {got:?} instead of {want:?}")));
    }
    let fee = plan.total_in() - tx.output.iter().map(|o| o.value).sum::<u64>();
    if fee != plan.fee {
        return Err(Mismatch::Other(format!("fee {fee} instead of {}", plan.fee)));
    }
    if let Some(value) = plan.change {
        let script = change.script_pubkey();
        if !tx.output.iter().any(|o| o.value == value && o.script_pubkey == script) {
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
    phase: Mutex<Phase>,
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
        session.spends.remove(&self.wallet_id, self.inputs.iter().copied());
        let signed = self.signed().take();
        if let (Some(signed), Ok(wallet)) = (signed, session.wallet(&self.wallet_id).await) {
            wallet.core().abandon_transaction(&signed).await;
        }
    }
}

impl Drop for PreparedTx {
    fn drop(&mut self) {
        if *self.phase() != Phase::Pending {
            return;
        }
        let session = Arc::clone(&self.session);
        let wallet_id = self.wallet_id;
        let inputs = std::mem::take(&mut self.inputs);
        let signed = self.signed().take();
        session.spends.remove(&wallet_id, inputs.iter().copied());
        let Some(signed) = signed else { return };
        let rt = session.rt.clone();
        rt.spawn(async move {
            if let Ok(wallet) = session.wallet(&wallet_id).await {
                wallet.core().abandon_transaction(&signed).await;
            }
        });
    }
}

impl NetworkSession {
    /// Stores the payment's messages and recipient labels (dash-qt
    /// `WalletModel::sendCoins`): a labelled recipient not in the address
    /// book is added (Send, or Receive for one of the wallet's addresses); a
    /// listed one is relabelled.
    async fn record_send_metadata(&self, wallet_id: WalletId, prepared: &PreparedTx) -> Result<(), EngineError> {
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
        let labels: Vec<(String, String, bool)> = prepared
            .records
            .iter()
            .zip(owned)
            .filter_map(|((a, l, _), mine)| l.clone().map(|l| (a.clone(), l, mine)))
            .collect();
        let txid = prepared.summary.txid.clone();
        let id = wallet_id.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            if let Some(m) = &message {
                db.set_tx_message(&id, &txid, Some(m), now)?;
            }
            for (address, label, mine) in &labels {
                let purpose = match db.book_entry(&id, address)? {
                    Some(entry) => entry.purpose,
                    None if *mine => dw_appdb::BookPurpose::Receive,
                    None => dw_appdb::BookPurpose::Send,
                };
                db.upsert_book_entry(&id, address, purpose, label, now)?;
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
        assert_eq!(validate_recipients(vec![], net).unwrap_err(), SendFailure::NoRecipients);
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
        let e = validate_recipients(
            vec![rc(ADDR, MAX_MONEY), rc("yX3d5a5NQb9dCmYyJYnEYNDQqyZFkNDUCB", 1000)],
            net,
        );
        assert!(matches!(e, Err(SendFailure::InvalidAmount { .. }) | Err(SendFailure::InvalidAddress { .. })));
    }

    #[test]
    fn fee_modes_are_bounded() {
        assert_eq!(FeeMode::Recommended { target_blocks: 6 }.rate().unwrap(), FeeRate::new(1000));
        assert!(FeeMode::Recommended { target_blocks: 0 }.rate().is_err());
        assert!(FeeMode::Recommended { target_blocks: 1009 }.rate().is_err());
        assert!(FeeMode::PerKb(999).rate().is_err());
        assert_eq!(FeeMode::PerKb(1000).rate().unwrap(), FeeRate::new(1000));
        assert!(FeeMode::PerKb(MAX_FEE_PER_KB + 1).rate().is_err());
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
