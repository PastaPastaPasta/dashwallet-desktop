//! Platform addresses behind the facade calls of `platform_addresses.rs`
//! (release 0.1, lane E2; m4 §2.12): the balance as the sync loop left it,
//! the activity rows, and "Move to Dash wallet". Not part of the facade.
//!
//! **Reads.** Every decision (the activity, a move's settlement, its plan
//! and nonces) rests on one proved read of the payment account's addresses
//! (`MoveNet::read`, `AddressInfo::fetch_many_with_metadata`): their nonces,
//! balances and the proof's block height. A move's read passes rule 5 as the
//! identity withdrawal does (DP3-04): once the fallback latch is set and
//! the mode enforces, it is repeated until its proofs used only SPV-verified
//! quorum keys (`trust::scoped`), within the gate's wait, else the move is
//! refused before anything is redeemed. The activity takes one read and,
//! unverified, decides nothing.
//!
//! **Activity.** platform-wallet keeps balances, not transfers. Each read
//! compares the proved balances with the baseline the last read stored, at
//! its proof height (a read older than the baseline decides nothing), and
//! records a row for each address whose balance rose (`incoming`) or fell
//! (`outgoing`); the first read records every funded address as incoming.
//! The rows, the moves the read settled and the new baseline are written in
//! one durable transaction. A move's own spend is no `outgoing` row: while
//! it is pending its inputs are not compared, and once it is done the first
//! fall of each input, up to what the move took from it (and the fee, on
//! the fee source), is the move's.
//!
//! **Moves.** A move withdraws to the quote's address, one of the wallet's
//! Core receiving addresses. Its inputs, the nonces it signs at (proved + 1)
//! and the credits it takes are fixed first, and recorded `pending` durably
//! (and read back) before the permit's signer can sign anything; it signs at
//! those nonces (`ExplicitWithNonces`), and Drive takes an input only at its
//! stored nonce + 1, so of two transitions at one input's nonce at most one
//! executes. A move is `done` only once a proved read shows every input at
//! or past its signed nonce (the library's answer is a snapshot of the
//! addresses, not proof of this transition); `failed` when it can never
//! execute (not every input was signed, or Platform's structure or signature
//! check refused it: deterministic, raised before the broadcast or by every
//! node), or when a proof at a height above the first read after its call
//! still shows its nonces unused. Nothing else ages a move out: an unknown
//! one stays `pending` and refuses the next move (`credits.move_pending`).
//! A move `failed` by height may still execute (its transition stays valid
//! while its nonces are unused), so a later move must sign exactly its
//! inputs at its nonces, or is refused the same way. When such a group's
//! nonces are taken, one proof settles all of it: the newest member the
//! library answered is `done`, else the oldest; the others `failed`.
//! 0.1 assumes one device per wallet.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use dash_sdk::platform::FetchMany;
use dash_sdk::query_types::AddressInfo;
use dpp::address_funds::{AddressFundsFeeStrategy, AddressFundsFeeStrategyStep, PlatformAddress};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::consensus::ConsensusError;
use dpp::dashcore::Network;
use dpp::identity::core_script::CoreScript;
use dpp::state_transition::address_credit_withdrawal_transition::MIN_CORE_FEE_PER_BYTE;
use dpp::version::PlatformVersion;
use key_wallet::PlatformP2PKHAddress;
use key_wallet::bip32::DerivationPath;
use platform_wallet::wallet::platform_addresses::InputSelection;
use platform_wallet::{PlatformWallet, PlatformWalletError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::credits::WithdrawAmount;
use super::errors::{CreditsError, PlatformError};
use super::flows::{FlowKind, GrantRequest};
use super::money_gate::Refusal;
use super::names::{NameAuthority, check_credit_cap};
use super::platform_addresses::{
    MoveToCoreOutcome, MoveToCoreQuote, PlatformActivity, PlatformActivityKind,
    PlatformActivityState, PlatformAddressBalance, PlatformAddressRow,
};
use super::requests::{leased, write_permit};
use super::signers::VaultPlatformAddressSigner;
use super::trust::{self, Rule5};
use super::withdrawal;
use crate::events::unix_now;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// The Platform payment account the wallet's addresses live in.
const ACCOUNT: u32 = 0;
/// The Core fee rate of the asset unlock, Platform's minimum (as DP6-02).
const CORE_FEE_PER_BYTE: u32 = MIN_CORE_FEE_PER_BYTE;
/// The change rows kept, the newest.
const ROWS_KEPT: usize = 200;
/// How long `sync_platform_addresses` waits for a pass already running.
const PASS_WAIT: Duration = Duration::from_secs(120);

/// Before the baseline had a height (never shipped; read once, then gone).
const LEGACY_SNAPSHOT_KEY: &str = "platform_address/snapshot";
const BASELINE_KEY: &str = "platform_address/baseline";
const ROW_PREFIX: &str = "platform_address/row/";
const MOVE_PREFIX: &str = "platform_address/move/";

/// One move as recorded before it is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MoveRecord {
    pub(crate) id: String,
    pub(crate) inputs: Vec<MoveInput>,
    /// What the addresses may lose: the withdrawal and the fee estimate.
    pub(crate) credits: u64,
    /// The fee estimate (dpp's minimum fee for the inputs; no cap exists).
    pub(crate) fee_credits: u64,
    pub(crate) expected_duffs: u64,
    pub(crate) to: String,
    pub(crate) at: u64,
    pub(crate) state: PlatformActivityState,
    /// A signature was released.
    #[serde(default)]
    pub(crate) signed: bool,
    /// The library's call returned `Ok` (no proof of execution).
    #[serde(default)]
    pub(crate) answered: bool,
    /// Its transition may still execute: signed, and nothing shows it never
    /// can. Such a move holds its inputs' nonces until they are taken.
    #[serde(default)]
    pub(crate) can_land: bool,
    /// The proof height of the first read after its call returned.
    #[serde(default)]
    pub(crate) seen_at: Option<u64>,
    /// Done: the fee the proved balances show it took.
    #[serde(default)]
    pub(crate) fee_taken: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MoveInput {
    /// bech32m.
    pub(crate) address: String,
    /// The nonce the transition signs at (Platform's stored one + 1).
    pub(crate) nonce: u32,
    /// What the move withdraws from it.
    pub(crate) credits: u64,
    /// Its proved balance when the move was planned.
    #[serde(default)]
    pub(crate) before: u64,
    /// The fee is deducted from it.
    #[serde(default)]
    pub(crate) fee_source: bool,
    /// Once the move is done: the fall of the address's balance still to
    /// be read as the move's (module doc), 0 once read.
    #[serde(default)]
    pub(crate) unseen: u64,
}

/// One proved read: bech32m → `(nonce, credits)` (`None`: Platform holds no
/// such address), at the proof's block height.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) height: u64,
    pub(crate) at: BTreeMap<String, Option<(u32, u64)>>,
}

impl Seen {
    fn nonce(&self, address: &str) -> Option<u32> {
        self.at.get(address).copied().flatten().map(|(n, _)| n)
    }

    fn credits(&self, address: &str) -> Option<u64> {
        self.at.get(address).copied().flatten().map(|(_, c)| c)
    }

    /// The funded addresses.
    fn balances(&self) -> Snapshot {
        (self.at.iter())
            .filter_map(|(a, i)| i.filter(|(_, c)| *c > 0).map(|(_, c)| (a.clone(), c)))
            .collect()
    }
}

impl MoveRecord {
    fn activity(&self) -> PlatformActivity {
        PlatformActivity {
            id: self.id.clone(),
            kind: PlatformActivityKind::MoveToCore,
            state: self.state,
            credits: self.credits,
            address: None,
            to: Some(self.to.clone()),
            expected_duffs: Some(self.expected_duffs),
            fee_credits: self.fee_taken,
            at: self.at,
        }
    }

    /// Its inputs at their signed nonces: two moves with one key cannot
    /// both execute.
    fn key(&self) -> Vec<(String, u32)> {
        let mut key: Vec<_> = (self.inputs.iter())
            .map(|i| (i.address.clone(), i.nonce))
            .collect();
        key.sort();
        key
    }

    /// It may still execute and is not known done.
    fn holds(&self) -> bool {
        self.can_land && self.state != PlatformActivityState::Done
    }

    /// `seen` shows every input at or past its signed nonce.
    fn reached(&self, seen: &Seen) -> bool {
        (self.inputs.iter()).all(|i| seen.nonce(&i.address).is_some_and(|n| n >= i.nonce))
    }

    /// `seen` shows every input still below its signed nonce.
    fn unused(&self, seen: &Seen) -> bool {
        (self.inputs.iter()).all(|i| seen.nonce(&i.address).is_some_and(|n| n < i.nonce))
    }

    /// Done as `seen` shows it: the fee its inputs' balances lost beyond
    /// the withdrawal (unknown when a balance before is not on record or
    /// they do not add up), and each input's fall the move's, its credits
    /// and, on the fee source, the fee.
    fn done(&mut self, seen: &Seen) {
        self.state = PlatformActivityState::Done;
        let before: Option<u64> = (self.inputs.iter())
            .map(|i| (i.before > 0).then_some(i.before))
            .sum();
        let after: u64 = (self.inputs.iter())
            .map(|i| seen.credits(&i.address).unwrap_or(0))
            .sum();
        let withdrawn: u64 = self.inputs.iter().map(|i| i.credits).sum();
        self.fee_taken = before
            .and_then(|b| b.checked_sub(after))
            .and_then(|fell| fell.checked_sub(withdrawn));
        let fee = self.fee_taken.unwrap_or(self.fee_credits);
        for input in &mut self.inputs {
            input.unseen = input.credits + if input.fee_source { fee } else { 0 };
        }
    }
}

/// Settles the moves that hold their inputs against `seen` (module doc);
/// the indices of those it changed.
pub(crate) fn settle(moves: &mut [MoveRecord], seen: &Seen) -> BTreeSet<usize> {
    let mut changed = BTreeSet::new();
    let landed: BTreeSet<Vec<(String, u32)>> = (moves.iter())
        .filter(|m| m.holds() && m.reached(seen))
        .map(MoveRecord::key)
        .collect();
    for key in landed {
        let group: Vec<usize> = (0..moves.len())
            .filter(|&i| moves[i].holds() && moves[i].key() == key)
            .collect();
        let order = |i: usize| (moves[i].at, moves[i].id.clone());
        let winner = (group.iter().copied().filter(|&i| moves[i].answered))
            .max_by_key(|&i| order(i))
            .or_else(|| group.iter().copied().min_by_key(|&i| order(i)));
        for i in group {
            if Some(i) == winner {
                moves[i].done(seen);
            } else {
                moves[i].state = PlatformActivityState::Failed;
                moves[i].can_land = false;
            }
            changed.insert(i);
        }
    }
    for (i, m) in moves.iter_mut().enumerate() {
        if !m.holds() {
            continue;
        }
        match m.seen_at {
            None => {
                m.seen_at = Some(seen.height);
                changed.insert(i);
            }
            Some(after) => {
                if m.state == PlatformActivityState::Pending
                    && seen.height > after
                    && m.unused(seen)
                {
                    m.state = PlatformActivityState::Failed;
                    changed.insert(i);
                }
            }
        }
    }
    changed
}

/// address (bech32m) → credits.
pub(crate) type Snapshot = BTreeMap<String, u64>;

/// The last read's balances and its proof height.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Baseline {
    pub(crate) height: u64,
    pub(crate) balances: Snapshot,
}

/// The change rows from `before` (`None`: no read yet) to `now`, read at
/// `height`, and the snapshot to store. `held` (a pending move's inputs) is
/// not compared and keeps its stored value; `unseen` (address → a done
/// move's fall still to read) absorbs the first fall of its address and is
/// then spent.
pub(crate) fn changes(
    before: Option<&Snapshot>,
    now: &Snapshot,
    held: &BTreeSet<String>,
    unseen: &mut BTreeMap<String, u64>,
    (at, height): (u64, u64),
) -> (Vec<PlatformActivity>, Snapshot) {
    let empty = Snapshot::new();
    let before_map = before.unwrap_or(&empty);
    let mut rows = Vec::new();
    let mut next = Snapshot::new();
    let addresses: BTreeSet<&String> = before_map.keys().chain(now.keys()).collect();
    for address in addresses {
        let (was, is) = (
            before_map.get(address).copied().unwrap_or(0),
            now.get(address).copied().unwrap_or(0),
        );
        if held.contains(address) {
            if let Some(&was) = before_map.get(address) {
                next.insert(address.clone(), was);
            }
            continue;
        }
        if is > 0 {
            next.insert(address.clone(), is);
        }
        let row = |kind, credits| PlatformActivity {
            id: format!("seen-{height}-{address}"),
            kind,
            state: PlatformActivityState::Done,
            credits,
            address: Some(address.clone()),
            to: None,
            expected_duffs: None,
            fee_credits: None,
            at,
        };
        if is > was {
            rows.push(row(PlatformActivityKind::Incoming, is - was));
        } else if was > is {
            let fell = was - is;
            let ours = unseen.get_mut(address).map_or(0, std::mem::take);
            if fell > ours {
                rows.push(row(PlatformActivityKind::Outgoing, fell - ours));
            }
        }
    }
    (rows, next)
}

/// The address-withdrawal rules at one protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The least an input may take (dpp `address_funds.min_input_amount`).
    pub(crate) min_input: u64,
    /// The most inputs one transition has.
    pub(crate) max_inputs: usize,
    /// The least and the most one withdrawal takes ([`withdrawal::address_range`]).
    pub(crate) min: u64,
    pub(crate) max: u64,
    /// dpp's minimum fee: `fee_base + fee_per_input × inputs`, no change
    /// output (`AddressCreditWithdrawalTransition::estimate_min_fee`).
    pub(crate) fee_base: u64,
    pub(crate) fee_per_input: u64,
}

impl Limits {
    pub(crate) fn at(version: &PlatformVersion) -> Result<Self, PlatformError> {
        let (min, max) = withdrawal::address_range(version)?;
        let funds = &version.dpp.state_transitions;
        let fees = &version.fee_version.state_transition_min_fees;
        Ok(Self {
            min_input: funds.address_funds.min_input_amount,
            max_inputs: funds.max_address_inputs as usize,
            min,
            max,
            fee_base: fees.address_credit_withdrawal,
            fee_per_input: fees.address_funds_transfer_input_cost,
        })
    }

    pub(crate) fn fee(&self, inputs: usize) -> u64 {
        (self.fee_per_input.saturating_mul(inputs as u64)).saturating_add(self.fee_base)
    }
}

/// What a move takes: each input's amount, the fee source (which keeps the
/// fee estimate), and the estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) inputs: BTreeMap<PlatformAddress, u64>,
    pub(crate) fee_source: PlatformAddress,
    pub(crate) fee: u64,
}

impl Plan {
    /// What reaches Core, in credits (before the asset unlock's own fee).
    pub(crate) fn credits(&self) -> u64 {
        self.inputs.values().sum()
    }

    fn fee_strategy(&self) -> AddressFundsFeeStrategy {
        let index = (self.inputs.keys()).position(|a| *a == self.fee_source);
        let index = index.and_then(|i| u16::try_from(i).ok()).unwrap_or(0);
        vec![AddressFundsFeeStrategyStep::DeductFromInput(index)]
    }
}

/// Why no plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// `platform.insufficient_credits`.
    Short { needed: u64, available: u64 },
    /// `credits.withdrawal_out_of_range`.
    OutOfRange,
}

impl Refused {
    fn error(self, limits: &Limits) -> CreditsError {
        match self {
            Refused::Short { needed, available } => {
                PlatformError::InsufficientCredits { needed, available }.into()
            }
            Refused::OutOfRange => CreditsError::WithdrawalOutOfRange {
                min: limits.min,
                max: limits.max,
            },
        }
    }
}

/// The plan for `amount` over the proved `balances` (module doc; as
/// platform-wallet's own plan: inputs of at least the input minimum, at most
/// `max_inputs`, the largest the fee source keeping the estimate). `All`
/// moves every such input, less the fee, at most [`Limits::max`] (a second
/// move takes the rest); an amount takes the fee source first, then the
/// largest, and a last input below the minimum borrows from the one before.
pub(crate) fn plan(
    balances: &BTreeMap<PlatformAddress, u64>,
    amount: WithdrawAmount,
    limits: &Limits,
) -> Result<Plan, Refused> {
    let available: u64 = balances.values().sum();
    let mut candidates: Vec<(PlatformAddress, u64)> = (balances.iter())
        .filter(|(_, c)| **c >= limits.min_input)
        .map(|(a, c)| (*a, *c))
        .collect();
    candidates.sort_by_key(|(a, c)| (std::cmp::Reverse(*c), *a));
    candidates.truncate(limits.max_inputs.max(1));
    // One address must hold the least withdrawal and its one-input fee.
    let least = Refused::Short {
        needed: limits.min.saturating_add(limits.fee(1)),
        available,
    };
    let Some(&(source, top)) = candidates.first() else {
        return Err(least);
    };
    let want = match amount {
        WithdrawAmount::All => {
            let all = (candidates.iter().map(|(_, c)| *c).sum::<u64>())
                .saturating_sub(limits.fee(candidates.len()));
            if all < limits.min {
                return Err(least);
            }
            all.min(limits.max)
        }
        WithdrawAmount::Credits { credits } => {
            if credits < limits.min || credits > limits.max {
                return Err(Refused::OutOfRange);
            }
            credits
        }
    };
    let mut count = 1;
    for _ in 0..=candidates.len() {
        let fee = limits.fee(count);
        let first = top.saturating_sub(fee).min(want);
        if first < limits.min_input {
            return Err(least);
        }
        let mut taken = vec![(source, first)];
        let mut left = want - first;
        for &(address, credits) in &candidates[1..] {
            if left == 0 {
                break;
            }
            let take = credits.min(left);
            taken.push((address, take));
            left -= take;
        }
        if left > 0 {
            return Err(Refused::Short {
                needed: want.saturating_add(fee),
                available,
            });
        }
        if taken.len() != count {
            // More inputs, a larger fee: again with that many.
            count = taken.len();
            continue;
        }
        if let [.., before, last] = taken.as_mut_slice()
            && last.1 < limits.min_input
        {
            let short = limits.min_input - last.1;
            if before.1 < limits.min_input.saturating_add(short) {
                return Err(Refused::OutOfRange);
            }
            before.1 -= short;
            last.1 = limits.min_input;
        }
        return Ok(Plan {
            inputs: taken.into_iter().collect(),
            fee_source: source,
            fee,
        });
    }
    Err(Refused::OutOfRange)
}

/// What `balances` (credits) can move: an `All` plan's withdrawal, 0 when
/// none (a remainder below the least withdrawal and its fee, or dust below
/// the input minimum, stays where it is).
pub(crate) fn movable(balances: &BTreeMap<PlatformAddress, u64>, limits: &Limits) -> u64 {
    plan(balances, WithdrawAmount::All, limits).map_or(0, |p| p.credits())
}

/// A transition Platform can never take: its structure or a signature
/// refused (consensus basic and signature errors are stateless, raised by
/// the SDK's structure check before the broadcast or by every node).
fn never_valid(e: &PlatformWalletError) -> bool {
    match e {
        PlatformWalletError::Sdk(dash_sdk::Error::Protocol(
            dpp::ProtocolError::ConsensusError(ce),
        )) => matches!(
            **ce,
            ConsensusError::BasicError(_) | ConsensusError::SignatureError(_)
        ),
        _ => false,
    }
}

/// address → `(nonce, credits)` (`None`: unknown to Platform), and the
/// proof's block height.
pub(crate) type Proved = (u64, BTreeMap<PlatformAddress, Option<(u32, u64)>>);

/// The network a move uses; tests script it.
pub(crate) trait MoveNet: Send + Sync {
    /// The network's protocol version, as the SDK last proved it.
    fn version(&self) -> &'static PlatformVersion;
    /// The addresses' nonces and balances, proved, with the proof's height.
    fn read(
        &self,
        addresses: BTreeSet<PlatformAddress>,
    ) -> impl Future<Output = Result<Proved, PlatformError>> + Send;
    /// Signs the withdrawal with `signer` and broadcasts it; `Ok` once
    /// Platform answered with the addresses' state.
    fn withdraw(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        script: CoreScript,
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> impl Future<Output = Result<(), PlatformWalletError>> + Send;
}

/// The SDK and platform-wallet.
pub(crate) struct SdkMoves {
    pub(crate) sdk: Arc<dash_sdk::Sdk>,
    pub(crate) wallet: Arc<PlatformWallet>,
}

impl MoveNet for SdkMoves {
    fn version(&self) -> &'static PlatformVersion {
        self.sdk.version()
    }

    async fn read(&self, addresses: BTreeSet<PlatformAddress>) -> Result<Proved, PlatformError> {
        let (infos, metadata) =
            AddressInfo::fetch_many_with_metadata(&self.sdk, addresses, None).await?;
        let infos = (infos.into_iter())
            .map(|(a, i)| (a, i.map(|i| (i.nonce, i.balance))))
            .collect();
        Ok((metadata.height, infos))
    }

    async fn withdraw(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        script: CoreScript,
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), PlatformWalletError> {
        (self.wallet.platform())
            .withdraw(
                ACCOUNT,
                InputSelection::ExplicitWithNonces(inputs),
                script,
                CORE_FEE_PER_BYTE,
                fee_strategy,
                Some(self.sdk.version()),
                signer,
            )
            .await
            .map(drop)
    }
}

/// `addresses` read once (module doc, Reads), converted for `network`. With
/// `wait`, rule 5 applies: under an enforcing mode with the fallback latch
/// set, the read repeats until its proofs used only SPV-verified quorum
/// keys, within the gate's wait. Without, an unverified read is refused at
/// once.
async fn read(
    session: &NetworkSession,
    net: &impl MoveNet,
    addresses: BTreeSet<PlatformAddress>,
    wait: bool,
) -> Result<Seen, PlatformError> {
    let network = session.vault.network();
    let gated =
        session.trust.rule_5() == Rule5::Enforce && session.live()?.provenance.fallback_used();
    let waits = session.platform.money_gate.waits();
    let deadline = tokio::time::Instant::now() + waits.wait;
    let (height, infos) = if gated {
        loop {
            let (read, tally) = trust::scoped(net.read(addresses.clone())).await;
            let read = read?;
            if tally.verified() {
                break read;
            }
            if !wait || tokio::time::Instant::now() + waits.retry >= deadline {
                return Err(Refusal::Unverified.platform());
            }
            tokio::time::sleep(waits.retry).await;
        }
    } else {
        net.read(addresses).await?
    };
    Ok(Seen {
        height,
        at: (infos.into_iter())
            .map(|(a, i)| (bech32m(&a, network), i))
            .collect(),
    })
}

/// One wallet's moves and activity reads, one at a time.
fn wallet_lock(wallet: WalletId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<Mutex<HashMap<WalletId, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(locks.entry(wallet).or_default())
}

/// The wallet's records in app.sqlite (its local scope).
struct Store {
    appdb: Arc<dw_appdb::AppDb>,
    scope: String,
}

impl Store {
    fn of(session: &NetworkSession, wallet: WalletId) -> Result<Self, PlatformError> {
        Ok(Self {
            appdb: session.live()?.appdb,
            scope: dw_appdb::local_scope(&wallet.to_string()),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dw_appdb::AppDb, &str) -> Result<T, dw_appdb::AppDbError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (appdb, scope) = (Arc::clone(&self.appdb), self.scope.clone());
        tokio::task::spawn_blocking(move || f(&appdb, &scope))
            .await
            .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
            .map_err(|e| PlatformError::Storage {
                detail: e.to_string(),
            })
    }

    /// The values under `prefix`; one that does not parse is skipped.
    async fn all<T: DeserializeOwned + Send + 'static>(
        &self,
        prefix: &'static str,
    ) -> Result<Vec<(String, T)>, PlatformError> {
        let rows = self
            .blocking(move |db, scope| db.settings_with_prefix(scope, prefix))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(key, value)| match serde_json::from_str(&value) {
                Ok(v) => Some((key, v)),
                Err(e) => {
                    tracing::warn!(%key, "an unreadable Platform-address record: {e}");
                    None
                }
            })
            .collect())
    }

    async fn delete(&self, key: String) -> Result<(), PlatformError> {
        self.blocking(move |db, scope| db.set_setting_durable(scope, &key, None))
            .await
    }

    /// Writes every `(key, value)` (`None` deletes) in one durable
    /// transaction, and reads them back.
    async fn commit(&self, writes: Vec<(String, Option<String>)>) -> Result<(), PlatformError> {
        if writes.is_empty() {
            return Ok(());
        }
        let read = self
            .blocking(move |db, scope| {
                let batch: Vec<(&str, Option<&str>)> = (writes.iter())
                    .map(|(k, v)| (k.as_str(), v.as_deref()))
                    .collect();
                db.set_settings_durable(scope, &batch)?;
                for (key, value) in &writes {
                    if db.setting(scope, key)? != *value {
                        return Ok(false);
                    }
                }
                Ok(true)
            })
            .await?;
        if read {
            Ok(())
        } else {
            Err(PlatformError::Storage {
                detail: "a Platform-address record did not read back".into(),
            })
        }
    }

    /// The baseline; a legacy snapshot reads as one at height 0.
    async fn baseline(&self) -> Result<Option<Baseline>, PlatformError> {
        if let Some((_, b)) = self.all::<Baseline>(BASELINE_KEY).await?.into_iter().next() {
            return Ok(Some(b));
        }
        Ok(
            (self.all::<Snapshot>(LEGACY_SNAPSHOT_KEY).await?.into_iter())
                .next()
                .map(|(_, balances)| Baseline {
                    height: 0,
                    balances,
                }),
        )
    }

    async fn moves(&self) -> Result<Vec<MoveRecord>, PlatformError> {
        let mut moves: Vec<MoveRecord> = (self.all(MOVE_PREFIX).await?.into_iter())
            .map(|(_, m)| m)
            .collect();
        moves.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        Ok(moves)
    }
}

fn json(value: &impl Serialize) -> Result<String, PlatformError> {
    serde_json::to_string(value).map_err(|e| PlatformError::Internal {
        detail: e.to_string(),
    })
}

fn move_write(record: &MoveRecord) -> Result<(String, Option<String>), PlatformError> {
    Ok((format!("{MOVE_PREFIX}{}", record.id), Some(json(record)?)))
}

fn bech32m(address: &PlatformAddress, network: Network) -> String {
    address.to_bech32m_string(network)
}

fn parse(address: &str) -> Option<PlatformAddress> {
    PlatformAddress::from_bech32m_string(address).ok()
}

/// The wallet's funded Platform addresses (bech32m → credits) as the last
/// sync pass left them.
async fn cached(wallet: &PlatformWallet) -> BTreeMap<PlatformAddress, u64> {
    (wallet
        .platform()
        .addresses_with_balances()
        .await
        .into_iter())
    .filter(|(_, c)| *c > 0)
    .collect()
}

/// The payment account's addresses and their DIP-17 paths.
pub(crate) async fn pool(
    wallet: &PlatformWallet,
) -> Result<BTreeMap<PlatformAddress, DerivationPath>, PlatformError> {
    let wm = wallet.wallet_manager().read().await;
    let info = wm
        .get_wallet_info(&wallet.wallet_id())
        .ok_or(PlatformError::WalletNotFound)?;
    let account = info
        .core_wallet
        .platform_payment_managed_account_at_index(ACCOUNT)
        .ok_or_else(|| PlatformError::Internal {
            detail: "the wallet has no Platform payment account".into(),
        })?;
    Ok((account.addresses.addresses.values())
        .filter_map(|a| {
            let p2pkh = PlatformP2PKHAddress::from_address(&a.address).ok()?;
            Some((PlatformAddress::P2pkh(p2pkh.to_bytes()), a.path.clone()))
        })
        .collect())
}

/// `DashPay::platform_addresses`: the cached balances; `movable_credits`
/// (and `duffs`, what Home adds) only what an `All` move would take.
pub(crate) async fn balance(
    manager: &Manager,
    wallet: &PlatformWallet,
    network: Network,
) -> PlatformAddressBalance {
    let cached = cached(wallet).await;
    let movable = Limits::at(manager.sdk_arc().version()).map_or(0, |l| movable(&cached, &l));
    let mut addresses: Vec<PlatformAddressRow> = (cached.iter())
        .map(|(a, c)| PlatformAddressRow {
            address: bech32m(a, network),
            credits: *c,
        })
        .collect();
    addresses.sort_by(|a, b| b.credits.cmp(&a.credits).then(a.address.cmp(&b.address)));
    let sync = manager.platform_address_sync();
    PlatformAddressBalance {
        credits: cached.values().sum(),
        movable_credits: movable,
        duffs: movable / CREDITS_PER_DUFF,
        addresses,
        synced_at: sync.last_sync_unix_seconds(),
        syncing: sync.is_syncing(),
    }
}

/// `DashPay::sync_platform_addresses`: one pass for the wallet, or the end
/// of the one already running.
pub(crate) async fn sync(
    session: &NetworkSession,
    manager: &Manager,
    wallet_id: WalletId,
) -> Result<(), CreditsError> {
    if !session.platform.enabled() {
        return Err(PlatformError::FeatureOff {
            feature: "platform".into(),
        }
        .into());
    }
    let sync = manager.platform_address_sync_arc();
    let handle = tokio::runtime::Handle::current();
    let pass = Arc::clone(&sync);
    // The library's pass is not `Send`: it runs on the blocking pool.
    let ran = tokio::task::spawn_blocking(move || {
        handle
            .block_on(pass.sync_wallet(&wallet_id.0))
            .map(drop)
            .map_err(Box::new)
    })
    .await
    .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?;
    match ran {
        Ok(()) => {}
        Err(e) if matches!(*e, PlatformWalletError::AddressSync(_)) && sync.is_syncing() => {
            let until = tokio::time::Instant::now() + PASS_WAIT;
            while sync.is_syncing() {
                if tokio::time::Instant::now() >= until {
                    return Err(PlatformError::Timeout.into());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Err(e) => return Err(PlatformError::from(*e).into()),
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    Ok(())
}

/// Settles `moves` against `seen` and records the activity it shows, all
/// in one durable write (module doc). Whether anything changed.
async fn observe(
    store: &Store,
    moves: &mut [MoveRecord],
    seen: &Seen,
) -> Result<bool, PlatformError> {
    let before = moves.to_vec();
    settle(moves, seen);
    let baseline = store.baseline().await?;
    let mut writes = Vec::new();
    let mut rows = 0;
    // A proof older than the baseline's (a node behind) decides nothing.
    if baseline.as_ref().is_none_or(|b| seen.height >= b.height) {
        let held: BTreeSet<String> = (moves.iter())
            .filter(|m| m.state == PlatformActivityState::Pending)
            .flat_map(|m| m.inputs.iter().map(|i| i.address.clone()))
            .collect();
        let mut unseen: BTreeMap<String, u64> = BTreeMap::new();
        for input in moves.iter().flat_map(|m| &m.inputs) {
            *unseen.entry(input.address.clone()).or_default() += input.unseen;
        }
        let (found, next) = changes(
            baseline.as_ref().map(|b| &b.balances),
            &seen.balances(),
            &held,
            &mut unseen,
            (unix_now(), seen.height),
        );
        // A done move's fall, once read, is spent.
        for input in moves.iter_mut().flat_map(|m| &mut m.inputs) {
            if unseen.get(&input.address) == Some(&0) {
                input.unseen = 0;
            }
        }
        for row in &found {
            let address = row.address.as_deref().unwrap_or("");
            let key = format!("{ROW_PREFIX}{:020}/{:020}/{address}", row.at, seen.height);
            writes.push((key, Some(json(row)?)));
        }
        rows = found.len();
        let next = Baseline {
            height: seen.height,
            balances: next,
        };
        if baseline.as_ref() != Some(&next) {
            writes.push((BASELINE_KEY.to_owned(), Some(json(&next)?)));
            writes.push((LEGACY_SNAPSHOT_KEY.to_owned(), None));
        }
    }
    let mut changed = rows > 0;
    for (m, was) in moves.iter().zip(&before) {
        if m != was {
            writes.push(move_write(m)?);
            changed = true;
        }
    }
    store.commit(writes).await?;
    Ok(changed)
}

/// Every address a read for the wallet covers: the payment account's and
/// any a recorded move signed.
fn covered(
    pool: &BTreeMap<PlatformAddress, DerivationPath>,
    moves: &[MoveRecord],
) -> BTreeSet<PlatformAddress> {
    (pool.keys().copied())
        .chain(
            moves
                .iter()
                .flat_map(|m| m.inputs.iter().filter_map(|i| parse(&i.address))),
        )
        .collect()
}

/// The proved balances of the payment account's addresses.
fn proved_balances(
    pool: &BTreeMap<PlatformAddress, DerivationPath>,
    seen: &Seen,
    network: Network,
) -> BTreeMap<PlatformAddress, u64> {
    (pool.keys())
        .filter_map(|a| Some((*a, seen.credits(&bech32m(a, network))?)))
        .filter(|(_, c)| *c > 0)
        .collect()
}

/// `DashPay::platform_activity`.
pub(crate) async fn activity(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
) -> Result<Vec<PlatformActivity>, CreditsError> {
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let store = Store::of(session, wallet_id)?;
    let mut moves = store.moves().await?;
    let addresses = covered(&pool(wallet).await?, &moves);
    let changed = match read(session, net, addresses, false).await {
        Ok(seen) => observe(&store, &mut moves, &seen).await?,
        Err(e) => {
            // Nothing is known: what is stored stands.
            tracing::info!("reading the Platform addresses for their activity: {e}");
            false
        }
    };
    let mut stored = store.all::<PlatformActivity>(ROW_PREFIX).await?;
    // Keys sort by time: the oldest beyond the kept ones go.
    while stored.len() > ROWS_KEPT {
        let (key, _) = stored.remove(0);
        store.delete(key).await?;
    }
    if changed {
        session
            .hub
            .pump
            .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    }
    let mut out: Vec<PlatformActivity> = (stored.into_iter().map(|(_, r)| r))
        .chain(moves.iter().map(MoveRecord::activity))
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(out)
}

/// `DashPay::move_to_core_quote`.
pub(crate) async fn quote(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    amount: WithdrawAmount,
) -> Result<MoveToCoreQuote, CreditsError> {
    let network = session.vault.network();
    let pool = pool(wallet).await?;
    let seen = read(session, net, pool.keys().copied().collect(), true).await?;
    let version = net.version();
    let limits = Limits::at(version)?;
    let planned = plan(&proved_balances(&pool, &seen, network), amount, &limits)
        .map_err(|r| r.error(&limits))?;
    let to = session
        .current_receiving_address(wallet_id)
        .await
        .map_err(PlatformError::from)?;
    let credits = planned.credits() + planned.fee;
    Ok(MoveToCoreQuote {
        credits,
        fee_credits: planned.fee,
        expected_duffs: withdrawal::expected_duffs(planned.credits(), version)?,
        to: to.to_string(),
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: credits,
        },
    })
}

/// `DashPay::move_to_core` (module doc).
pub(crate) async fn move_to_core(
    net: &impl MoveNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (amount, to): (WithdrawAmount, &str),
    grant: &str,
) -> Result<MoveToCoreOutcome, CreditsError> {
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let network = session.vault.network();
    let store = Store::of(session, wallet_id)?;
    let mut moves = store.moves().await?;
    let pool = pool(wallet).await?;
    // Rule 5 and the one read every decision below rests on.
    let seen = read(session, net, covered(&pool, &moves), true).await?;
    if observe(&store, &mut moves, &seen).await? {
        session
            .hub
            .pump
            .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    }
    if let Some(pending) = (moves.iter()).find(|m| m.state == PlatformActivityState::Pending) {
        return Err(CreditsError::MovePending {
            id: pending.id.clone(),
        });
    }
    let version = net.version();
    let limits = Limits::at(version)?;
    let mut balances = proved_balances(&pool, &seen, network);
    // A move that may still execute: the next one draws from its inputs only,
    // so it signs at the same nonces (checked below).
    let held = moves.iter().find(|m| m.holds());
    if let Some(held) = held {
        let only: BTreeSet<&str> = held.inputs.iter().map(|i| i.address.as_str()).collect();
        balances.retain(|a, _| only.contains(bech32m(a, network).as_str()));
    }
    let planned = plan(&balances, amount, &limits).map_err(|r| r.error(&limits))?;
    let mut inputs = BTreeMap::new();
    for (address, credits) in &planned.inputs {
        let nonce = seen
            .nonce(&bech32m(address, network))
            .ok_or(PlatformError::Unavailable)?;
        inputs.insert(*address, (nonce.saturating_add(1), *credits));
    }
    let mut record = MoveRecord {
        id: format!("move-{}-{}", seen.height, moves.len()),
        inputs: (inputs.iter())
            .map(|(a, (nonce, credits))| {
                let address = bech32m(a, network);
                MoveInput {
                    before: seen.credits(&address).unwrap_or(0),
                    address,
                    nonce: *nonce,
                    credits: *credits,
                    fee_source: *a == planned.fee_source,
                    unseen: 0,
                }
            })
            .collect(),
        credits: planned.credits() + planned.fee,
        fee_credits: planned.fee,
        expected_duffs: withdrawal::expected_duffs(planned.credits(), version)?,
        to: to.to_owned(),
        at: unix_now(),
        state: PlatformActivityState::Pending,
        signed: false,
        answered: false,
        can_land: false,
        seen_at: None,
        fee_taken: None,
    };
    // A move `failed` by height may still execute: only one at its very
    // inputs and nonces may follow it.
    if let Some(held) = held.filter(|h| h.key() != record.key()) {
        return Err(CreditsError::MovePending {
            id: held.id.clone(),
        });
    }
    let script = withdrawal::destination(network, to)?;
    // `destination` checked it parses, for this network.
    let address = std::str::FromStr::from_str(to.trim())
        .ok()
        .and_then(|a: dpp::dashcore::Address<_>| a.require_network(network).ok())
        .ok_or(PlatformError::InvalidArgument {
            detail: "not a Dash address of this network".into(),
        })?;
    let own = session
        .is_receiving_address(wallet_id, &address)
        .await
        .map_err(PlatformError::from)?;
    if !own {
        return Err(PlatformError::InvalidArgument {
            detail: "a move pays only one of this wallet's receiving addresses".into(),
        }
        .into());
    }
    // The authority before anything is redeemed: a refusal keeps the grant.
    let authority = match session.lease_for(&wallet_id, grant) {
        Some(lease) => {
            let lease = lease.map_err(PlatformError::from)?;
            if lease.flow() != FlowKind::PlatformAddressMove {
                return Err(PlatformError::GrantInvalid.into());
            }
            NameAuthority::Lease(lease)
        }
        None => NameAuthority::Grant(grant.to_owned()),
    };
    check_credit_cap(session, wallet_id, &authority, record.credits).await?;
    let paths: BTreeMap<[u8; 20], DerivationPath> = (inputs.keys())
        .map(|a| match (a, pool.get(a)) {
            (PlatformAddress::P2pkh(hash), Some(path)) => Ok((*hash, path.clone())),
            _ => Err(PlatformError::Unavailable),
        })
        .collect::<Result<_, _>>()?;
    let lease = match authority {
        NameAuthority::Lease(lease) => lease,
        NameAuthority::Grant(grant) => session
            .begin_lease(wallet_id, FlowKind::PlatformAddressMove, &[grant])
            .await
            .map_err(PlatformError::from)?,
    };
    let permit = write_permit(&lease, "move_to_core", record.credits)?;
    let signer = match permit.platform_address_signer(paths) {
        Ok(signer) => signer,
        Err(e) => {
            permit.finish(false);
            return Err(PlatformError::from(e).into());
        }
    };
    // Recorded and read back before anything is signed.
    if let Err(e) = store.commit(vec![move_write(&record)?]).await {
        drop(signer);
        permit.finish(false);
        return Err(e.into());
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    let call = leased(
        &lease,
        net.withdraw(inputs, script, planned.fee_strategy(), &signer),
    )
    .await;
    let released = permit.signatures().released();
    drop(signer);
    permit.finish(call.is_ok());
    record.signed = released > 0;
    record.answered = call.is_ok();
    let cannot_land =
        released < record.inputs.len() as u64 || call.as_ref().err().is_some_and(never_valid);
    if let Err(e) = &call {
        tracing::info!(id = %record.id, cannot_land, "a move's call failed: {e}");
    }
    moves.push(record);
    let at = moves.len() - 1;
    if cannot_land {
        moves[at].state = PlatformActivityState::Failed;
    } else {
        moves[at].can_land = true;
        // Done only on its proved nonces, whatever the call answered.
        let signed = moves[at].inputs.iter().filter_map(|i| parse(&i.address));
        match read(session, net, signed.collect(), false).await {
            Ok(after) => {
                settle(&mut moves, &after);
            }
            Err(e) => tracing::info!(id = %moves[at].id, "reading a move's nonces: {e}"),
        }
    }
    let writes: Vec<_> = moves.iter().map(move_write).collect::<Result<_, _>>()?;
    if let Err(e) = store.commit(writes).await {
        // The pending record stands and settles on a later read.
        tracing::warn!(id = %moves[at].id, "storing a move's outcome: {e}");
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    let record = &moves[at];
    if cannot_land && let Err(e) = call {
        return Err(PlatformError::from(e).into());
    }
    Ok(MoveToCoreOutcome {
        id: record.id.clone(),
        credits: record.credits,
        expected_duffs: record.expected_duffs,
        to: record.to.clone(),
        state: record.state,
        fee_credits: record.fee_taken,
    })
}

#[cfg(all(feature = "test-fund", debug_assertions))]
impl NetworkSession {
    /// **Test only** (dwcli `platform-addr fund`, the release-0.1 testnet
    /// smoke; the app never calls it, and it is no facade call): funds the
    /// wallet's first Platform payment address from its Core balance with
    /// an asset lock of `duffs`. `address_grant` is a `PlatformOp` grant
    /// with credits (the address's signer), `spend_grant` a `Spend` grant
    /// (the asset lock's inputs and credit key, which no Platform scope
    /// reaches). No lease, no fence: a lock broadcast and not consumed is
    /// left to the library's tracked asset locks.
    #[doc(hidden)]
    pub async fn fund_platform_addresses_for_test(
        self: &Arc<Self>,
        wallet_id: WalletId,
        duffs: u64,
        address_grant: String,
        spend_grant: String,
    ) -> Result<PlatformAddressBalance, CreditsError> {
        self.dashpay(wallet_id)
            .on_wallet(move |session, manager, wallet| async move {
                let vault = session.vault.clone();
                let w = wallet_id.0;
                let (address_signer, lock_signer) = tokio::task::spawn_blocking(move || {
                    let token = vault.redeem_grant(
                        &address_grant,
                        dw_vault::GrantKind::PlatformOp,
                        Some(&w),
                    )?;
                    let address = vault.platform_signer(
                        &w,
                        &token,
                        dw_vault::SignerScope::PlatformAddress,
                    )?;
                    let token =
                        vault.redeem_grant(&spend_grant, dw_vault::GrantKind::Spend, Some(&w))?;
                    Ok::<_, dw_vault::VaultError>((address, vault.signer(&w, &token)?))
                })
                .await
                .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
                .map_err(PlatformError::from)?;
                let (hash, path) = {
                    let wm = wallet.wallet_manager().read().await;
                    let info = (wm.get_wallet_info(&wallet.wallet_id()))
                        .ok_or(PlatformError::WalletNotFound)?;
                    let account = (info.core_wallet)
                        .platform_payment_managed_account_at_index(ACCOUNT)
                        .ok_or(PlatformError::Unavailable)?;
                    let first = (account.addresses.addresses.values().next())
                        .ok_or(PlatformError::Unavailable)?;
                    let p2pkh =
                        PlatformP2PKHAddress::from_address(&first.address).map_err(|e| {
                            PlatformError::Internal {
                                detail: e.to_string(),
                            }
                        })?;
                    (p2pkh.to_bytes(), first.path.clone())
                };
                let signer = super::signers::VaultPlatformAddressSigner::new(
                    address_signer,
                    [(hash, path)].into(),
                    Arc::new(super::lease::SignatureCount::default()),
                )
                .map_err(PlatformError::from)?;
                wallet
                    .platform()
                    .fund_from_asset_lock(
                        platform_wallet::AssetLockFunding::FromWalletBalance {
                            amount_duffs: duffs,
                            account_index: 0,
                        },
                        ACCOUNT,
                        [(PlatformAddress::P2pkh(hash), None)].into(),
                        Vec::new(),
                        &signer,
                        &lock_signer,
                        None,
                    )
                    .await
                    .map_err(PlatformError::from)?;
                session
                    .hub
                    .pump
                    .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
                Ok(balance(&manager, &wallet, session.vault.network()).await)
            })
            .await
    }
}

#[cfg(test)]
#[path = "platform_moves_tests.rs"]
mod tests;
