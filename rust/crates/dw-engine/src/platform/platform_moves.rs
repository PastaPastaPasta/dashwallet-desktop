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
//! the fee source), is the move's. An "Add from Dash wallet" (0.2,
//! `platform_fund.rs`) is no `incoming` row either: while its apply runs
//! its address is not compared, and once the apply is proved the first rise
//! of the address, up to what the fund added, is the fund's (its own
//! `from_core` row). Before that, a rise of its address up to the lock's
//! credits is read as the fund's (`absorbed`, so the fund done takes only
//! the rest) only once the fund is *armed*: recorded durably just before an
//! apply asks for the lock key's signature ([`Arming`]), and undone if that
//! apply released none. A fund
//! still waiting for its proof, refused before its signature, or never
//! tried absorbs nothing. A restored lock (no fund step of this install
//! signed it) has no record until its apply is armed.
//!
//! A pending fund ends on the lock's evidence, read by its exact outpoint
//! (`unfinished_funds::fund_evidence`), never by a second lock or apply:
//! the library's proved `Consumed` (live, or stored for a lock a reload no
//! longer loads) or a settled record completes it with
//! its estimate, marked `estimated`; Platform's unproved "already consumed"
//! (kept as a record of its own) completes an own fund only once an apply
//! of it released a signature, else closes it (`failed`), and removes a
//! restored lock's record (only a proved apply completes one: no row, no
//! allowance), a record closed or removed giving what it absorbed back as
//! `incoming`; and a lock the library no
//! longer tracks once SPV and the scan caught up closes an unarmed fund (a
//! lock swept by a conflicting transaction, or untracked as never sent:
//! platform-wallet's `sync/reconstruction.rs` and `sync/tracking.rs`). An
//! armed fund whose lock is neither tracked nor stored `Consumed` stays
//! pending. The hold is taken under the
//! read's lock ([`hold_fund`]), so a read ran wholly before it or sees it.

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
//! inputs at its nonces, or is refused the same way; when either pays a
//! Platform address it must also have its exact terms (the held send's
//! exact retry, re-signed on its stored terms), and a retry of a send that
//! landed pays nothing (m4 §2.12).
//!
//! **Restarts.** The record is stored able to land before anything is
//! signed, and only a definitive refusal clears that; every `pending`
//! record read back is treated so (one stored by an earlier build too).
//! So a move whose process stopped anywhere after the record, or whose
//! outcome write failed, is settled by the next reads exactly as a live
//! one: the first records its height, its nonces reached make it `done`,
//! a higher proof of them unused makes it `failed` and holding.
//!
//! **Groups.** When the nonces of moves sharing inputs and nonces are
//! taken, one proof settles all of them. With equal terms (the amounts,
//! the fee estimate, the payout and the address) the history is the same
//! whichever executed: the newest member the library answered is `done`,
//! else the oldest, the others `failed`. With other terms nothing says
//! which executed (the library's answer is a snapshot, rs-sdk
//! `broadcast.rs:70-77`): the newest is `done` for the group, with the
//! inputs' proved debit and no payout or fee attributed, and the others
//! `replaced`. 0.1 assumes one device per wallet.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use dash_sdk::platform::FetchMany;
use dash_sdk::query_types::AddressInfo;
use dashcore::OutPoint;
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
    PlatformActivityState, PlatformAddressBalance, PlatformAddressRow, PlatformReceiveAddress,
    PlatformSendOutcome, PlatformSendQuote, RecipientProblem,
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
const FUND_PREFIX: &str = "platform_address/fund/";

/// What a move pays: the wallet's Core address, or a Platform address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MoveKind {
    /// "Move to Dash wallet" (an address credit withdrawal); every 0.1
    /// record.
    #[default]
    ToCore,
    /// A send to a Platform address (an address funds transfer, 0.2).
    Transfer,
}

/// One move as recorded before it is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MoveRecord {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) kind: MoveKind,
    pub(crate) inputs: Vec<MoveInput>,
    /// What the addresses may lose: the amount and the fee the plan keeps
    /// for it (a withdrawal's estimate, a transfer's band), or, when the
    /// output pays the fee, the amount alone.
    pub(crate) credits: u64,
    /// The fee estimate (dpp's minimum fee for the transition; no cap
    /// exists).
    pub(crate) fee_credits: u64,
    /// A withdrawal's payout on Core; 0 for a transfer.
    pub(crate) expected_duffs: u64,
    /// The Core address (a withdrawal) or the bech32m Platform address (a
    /// transfer) it pays.
    pub(crate) to: String,
    /// A transfer whose one output pays the fee (`ReduceOutput`, Max): its
    /// inputs give exactly their amounts and the recipient receives the
    /// output less the fee.
    #[serde(default)]
    pub(crate) fee_from_output: bool,
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
    /// Done for a group of moves with other terms at its inputs and nonces
    /// (module doc, Moves): what the group took, unattributed.
    #[serde(default)]
    pub(crate) group: Option<GroupOutcome>,
    /// The protocol version it was signed at (`None` before 0.2): a
    /// definitive refusal of an identical transition at the same version
    /// says this one can never execute either.
    #[serde(default)]
    pub(crate) version: Option<u32>,
}

/// [`MoveRecord::terms`]: the kind and fee rule, the inputs (address,
/// nonce, credits), the fee estimate, the payout and the destination.
type Terms<'a> = (
    (MoveKind, bool),
    Vec<(&'a str, u32, u64)>,
    u64,
    u64,
    &'a str,
);

/// A group's `Done` (module doc, Moves): the proved debit of its inputs
/// (`None` when a balance before is not on record) and the destination,
/// when every member named the same one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GroupOutcome {
    pub(crate) debit: Option<u64>,
    pub(crate) to: Option<String>,
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

/// One "Add from Dash wallet" of this wallet (0.2, `platform_fund.rs`): its
/// lock and the address its apply pays; recorded once the lock is out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FundRecord {
    pub(crate) txid: String,
    pub(crate) duffs: u64,
    /// bech32m.
    pub(crate) address: String,
    /// `Pending` until a proved apply, then `Done`.
    pub(crate) state: PlatformActivityState,
    /// Pending: about what it adds (the lock's credits less the fee
    /// estimate); done: what the apply added, as its proof shows it.
    pub(crate) credits: u64,
    /// Once done: the rise of the address still to be read as the fund's
    /// (module doc), 0 once read.
    #[serde(default)]
    pub(crate) unseen: u64,
    /// The rise of the address read as the fund's while it was pending, at
    /// most the lock's credits (module doc).
    #[serde(default)]
    pub(crate) absorbed: u64,
    /// Done with the estimate, not what an apply's proof showed added.
    #[serde(default)]
    pub(crate) estimated: bool,
    /// The lock's credit output: the fund's lock is the outpoint
    /// (`txid`, `vout`), and evidence about it is by that outpoint.
    #[serde(default)]
    pub(crate) vout: u32,
    /// An apply of its lock released a signature (recorded just before
    /// it): only then may a rise of its address be the fund's while it is
    /// pending.
    #[serde(default)]
    pub(crate) armed: bool,
    /// Made by an apply's arming, for a lock no fund step of this install
    /// recorded (a restored one): only a proved apply completes it.
    #[serde(default)]
    pub(crate) restored: bool,
    pub(crate) at: u64,
}

impl FundRecord {
    fn activity(&self) -> PlatformActivity {
        PlatformActivity {
            id: format!("fund-{}", self.txid),
            kind: PlatformActivityKind::FromCore,
            state: self.state,
            credits: self.credits,
            address: Some(self.address.clone()),
            to: None,
            expected_duffs: None,
            fee_credits: None,
            at: self.at,
            may_land: false,
            estimated: self.estimated,
        }
    }

    /// While pending and armed: what of the address's rise may still be
    /// the fund's.
    fn expects(&self) -> u64 {
        if self.state == PlatformActivityState::Pending && self.armed {
            (self.duffs.saturating_mul(CREDITS_PER_DUFF)).saturating_sub(self.absorbed)
        } else {
            0
        }
    }

    /// Done as `credits` added, of which what was read while pending is no
    /// longer to read. Completed with the estimate after a smaller rise was
    /// read, the rest stays to read: at most the estimate's excess over
    /// the rise (Platform's fee above its minimum), it comes off the next
    /// rise of the address.
    fn complete(&mut self, credits: u64, estimated: bool) {
        self.state = PlatformActivityState::Done;
        self.credits = credits;
        self.unseen = credits.saturating_sub(self.absorbed);
        self.estimated = estimated;
    }

    /// A pending, unarmed record of the fund lock at `outpoint`.
    fn pending((outpoint, duffs): (OutPoint, u64), address: String, credits: u64) -> Self {
        Self {
            txid: outpoint.txid.to_string(),
            duffs,
            address,
            state: PlatformActivityState::Pending,
            credits,
            unseen: 0,
            absorbed: 0,
            estimated: false,
            vout: outpoint.vout,
            armed: false,
            restored: false,
            at: unix_now(),
        }
    }

    /// Ended without an apply of this install's (module doc): no rise is
    /// the fund's, and what it absorbed is given back
    /// ([`Self::give_back`]). Its writes.
    fn close(&mut self) -> Result<Vec<Write>, PlatformError> {
        let mut writes = self.give_back()?;
        self.state = PlatformActivityState::Failed;
        self.unseen = 0;
        self.armed = false;
        writes.push(fund_write(self)?);
        Ok(writes)
    }

    /// What it absorbed of its address's rise (as the fund's, while
    /// pending), an `incoming` row again, once.
    fn give_back(&mut self) -> Result<Vec<Write>, PlatformError> {
        let credits = std::mem::take(&mut self.absorbed);
        if credits == 0 {
            return Ok(Vec::new());
        }
        let at = unix_now();
        let row = PlatformActivity {
            id: format!("released-{at}-{}", self.txid),
            kind: PlatformActivityKind::Incoming,
            state: PlatformActivityState::Done,
            credits,
            address: Some(self.address.clone()),
            to: None,
            expected_duffs: None,
            fee_credits: None,
            at,
            may_land: false,
            estimated: false,
        };
        let key = format!("{ROW_PREFIX}{at:020}/released/{}", self.txid);
        Ok(vec![(key, Some(json(&row)?))])
    }

    /// After Platform's unproved "already consumed" of its lock, its
    /// writes and whether the record is kept: an own fund whose apply
    /// released a signature completes with `estimate`; one that never
    /// signed closes; a restored lock's record is removed, as no fund of
    /// this install's is shown to have reached the address. Closed or
    /// removed, what it absorbed is given back.
    fn reported(&mut self, estimate: u64) -> Result<(Vec<Write>, bool), PlatformError> {
        if self.restored {
            let mut writes = self.give_back()?;
            writes.push(fund_delete(&self.txid));
            return Ok((writes, false));
        }
        if self.armed {
            self.complete(estimate, true);
            return Ok((vec![fund_write(self)?], true));
        }
        Ok((self.close()?, true))
    }

    fn outpoint(&self) -> Option<OutPoint> {
        Some(OutPoint::new(self.txid.parse().ok()?, self.vout))
    }
}

/// What a read knows of the wallet's fund locks
/// (`unfinished_funds::fund_evidence`), each by outpoint.
#[derive(Debug, Default)]
pub(crate) struct FundEvidence {
    /// Applied: the library's proved `Consumed`, or recorded settled.
    pub(crate) done: HashSet<OutPoint>,
    /// Platform's "already consumed" for that outpoint, unproved.
    pub(crate) reported: HashSet<OutPoint>,
    /// Every fund lock the library tracks, any status.
    pub(crate) tracked: HashSet<OutPoint>,
    /// SPV and the wallet's scan caught up: a lock not tracked now is gone.
    pub(crate) caught_up: bool,
}

/// The addresses whose fund apply runs now, per wallet, with how many runs.
pub(crate) type FundHolds = Arc<Mutex<HashMap<WalletId, BTreeMap<String, usize>>>>;

/// While it lives, the reads of the wallet's activity do not compare
/// `address` (module doc): its rise is the fund's once the apply is proved.
pub(crate) struct FundHold {
    holds: FundHolds,
    wallet: WalletId,
    address: String,
}

impl FundHold {
    fn new(holds: &FundHolds, wallet: WalletId, address: String) -> Self {
        let mut of = holds.lock().unwrap_or_else(PoisonError::into_inner);
        *(of.entry(wallet).or_default())
            .entry(address.clone())
            .or_default() += 1;
        Self {
            holds: Arc::clone(holds),
            wallet,
            address,
        }
    }
}

impl Drop for FundHold {
    fn drop(&mut self) {
        let mut holds = self.holds.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(of) = holds.get_mut(&self.wallet) {
            if let Some(n) = of.get_mut(&self.address) {
                *n -= 1;
                if *n == 0 {
                    of.remove(&self.address);
                }
            }
            if of.is_empty() {
                holds.remove(&self.wallet);
            }
        }
    }
}

/// Holds `address` out of the wallet's activity reads ([`FundHold`]),
/// taken under their lock: a read either ran wholly before it or sees it.
/// The fund lock, when held, is taken before this one, never after.
pub(crate) async fn hold_fund(
    session: &NetworkSession,
    wallet: WalletId,
    address: String,
) -> FundHold {
    let lock = wallet_lock(wallet);
    let _one = lock.lock().await;
    FundHold::new(&session.platform.fund_holds, wallet, address)
}

/// The addresses the wallet's running fund applies hold.
pub(crate) fn fund_holds(session: &NetworkSession, wallet: WalletId) -> BTreeSet<String> {
    let holds = (session.platform.fund_holds.lock()).unwrap_or_else(PoisonError::into_inner);
    holds
        .get(&wallet)
        .map_or_else(BTreeSet::new, |of| of.keys().cloned().collect())
}

/// Records fund `txid`'s lock out, paying `address`, about `credits` to
/// add; a record of it already there stands.
pub(crate) async fn record_fund(
    session: &NetworkSession,
    wallet: WalletId,
    (outpoint, duffs): (OutPoint, u64),
    address: String,
    credits: u64,
) -> Result<(), PlatformError> {
    let lock = wallet_lock(wallet);
    let _one = lock.lock().await;
    let store = Store::of(session, wallet)?;
    let txid = outpoint.txid.to_string();
    if store.fund(&txid).await?.is_some() {
        return Ok(());
    }
    let record = FundRecord::pending((outpoint, duffs), address, credits);
    store.commit(vec![fund_write(&record)?]).await
}

/// Fund `txid`'s record, if it has one.
pub(crate) async fn fund_record(
    session: &NetworkSession,
    wallet: WalletId,
    txid: &str,
) -> Result<Option<FundRecord>, PlatformError> {
    let store = Store::of(session, wallet)?;
    store.fund(txid).await
}

/// What [`arm_fund`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arming {
    /// The lock had no record (a restored one): made, armed.
    Made,
    /// Its record armed now: pending unarmed, or closed (the lock came
    /// back), pending again.
    Armed,
    /// Already armed, or done: nothing written.
    Unchanged,
}

/// Records that an apply of the fund lock at `outpoint` is about to
/// release a signature: its record armed (module doc), made first for a
/// lock that has none (a restored one: paying `address`, about
/// `credits`).
pub(crate) async fn arm_fund(
    session: &NetworkSession,
    wallet: WalletId,
    lock: (OutPoint, u64),
    address: String,
    credits: u64,
) -> Result<Arming, PlatformError> {
    let one = wallet_lock(wallet);
    let _one = one.lock().await;
    let store = Store::of(session, wallet)?;
    let (record, arming) = match store.fund(&lock.0.txid.to_string()).await? {
        Some(r)
            if r.state == PlatformActivityState::Failed
                || (r.state == PlatformActivityState::Pending && !r.armed) =>
        {
            let r = FundRecord {
                state: PlatformActivityState::Pending,
                armed: true,
                ..r
            };
            (r, Arming::Armed)
        }
        Some(_) => return Ok(Arming::Unchanged),
        None => {
            let r = FundRecord {
                armed: true,
                restored: true,
                ..FundRecord::pending(lock, address, credits)
            };
            (r, Arming::Made)
        }
    };
    store.commit(vec![fund_write(&record)?]).await?;
    Ok(arming)
}

/// Undoes [`arm_fund`]'s `arming` when no signature was released after
/// all: the record it made is removed, the one it armed disarmed.
pub(crate) async fn disarm_fund(
    session: &NetworkSession,
    wallet: WalletId,
    txid: &str,
    arming: Arming,
) -> Result<(), PlatformError> {
    let lock = wallet_lock(wallet);
    let _one = lock.lock().await;
    let store = Store::of(session, wallet)?;
    let Some(record) = pending_fund(&store, txid).await? else {
        return Ok(());
    };
    let write = match arming {
        Arming::Unchanged => return Ok(()),
        Arming::Made => fund_delete(txid),
        Arming::Armed => fund_write(&FundRecord {
            armed: false,
            ..record
        })?,
    };
    store.commit(vec![write]).await
}

/// Platform answered "already consumed" for fund `txid`'s lock
/// ([`FundRecord::reported`]).
pub(crate) async fn fund_used(
    session: &NetworkSession,
    wallet: WalletId,
    txid: &str,
    estimate: u64,
) -> Result<(), PlatformError> {
    let lock = wallet_lock(wallet);
    let _one = lock.lock().await;
    let store = Store::of(session, wallet)?;
    let Some(mut record) = pending_fund(&store, txid).await? else {
        return Ok(());
    };
    let (writes, _) = record.reported(estimate)?;
    commit_outcome(&store, writes).await
}

/// Fund `txid`'s record while it is pending.
async fn pending_fund(store: &Store, txid: &str) -> Result<Option<FundRecord>, PlatformError> {
    Ok((store.fund(txid).await?).filter(|r| r.state == PlatformActivityState::Pending))
}

/// The address fund `txid`'s record names, if it has one.
pub(crate) async fn fund_address(
    session: &NetworkSession,
    wallet: WalletId,
    txid: &str,
) -> Result<Option<String>, PlatformError> {
    Ok(fund_record(session, wallet, txid).await?.map(|r| r.address))
}

/// Records fund `txid` applied, `credited` added to `address` (the
/// estimate when `estimated`); once.
pub(crate) async fn fund_applied(
    session: &NetworkSession,
    wallet: WalletId,
    lock: (OutPoint, u64),
    address: String,
    (credited, estimated): (u64, bool),
) -> Result<(), PlatformError> {
    let one = wallet_lock(wallet);
    let _one = one.lock().await;
    let store = Store::of(session, wallet)?;
    let mut record = match store.fund(&lock.0.txid.to_string()).await? {
        Some(r) if r.state == PlatformActivityState::Done => return Ok(()),
        Some(r) => r,
        None => FundRecord {
            armed: true,
            ..FundRecord::pending(lock, address, credited)
        },
    };
    record.complete(credited, estimated);
    commit_outcome(&store, vec![fund_write(&record)?]).await
}

/// Ends each pending fund of `funds` its lock's evidence `ev` decides
/// (module doc), but not one whose apply runs now (its address `held`: its
/// settle records it): applied, it completes with its estimate; reported
/// used, as [`FundRecord::reported`]; gone from the library unarmed, it
/// closes.
async fn reconcile(
    store: &Store,
    funds: &mut Vec<FundRecord>,
    ev: &FundEvidence,
    held: &BTreeSet<String>,
) -> Result<bool, PlatformError> {
    let mut writes = Vec::new();
    let mut removed = HashSet::new();
    for fund in funds.iter_mut() {
        let at = (fund.state == PlatformActivityState::Pending && !held.contains(&fund.address))
            .then(|| fund.outpoint())
            .flatten();
        let Some(at) = at else {
            continue;
        };
        if ev.done.contains(&at) {
            fund.complete(fund.credits, true);
            writes.push(fund_write(fund)?);
        } else if ev.reported.contains(&at) {
            let (ended, kept) = fund.reported(fund.credits)?;
            writes.extend(ended);
            if !kept {
                removed.insert(fund.txid.clone());
            }
        } else if !fund.armed && ev.caught_up && !ev.tracked.contains(&at) {
            writes.extend(fund.close()?);
        }
    }
    funds.retain(|f| !removed.contains(&f.txid));
    let changed = !writes.is_empty();
    if changed {
        store.commit(writes).await?;
    }
    Ok(changed)
}

impl MoveRecord {
    fn activity(&self) -> PlatformActivity {
        let (credits, to, expected_duffs) = match &self.group {
            Some(g) => (g.debit.unwrap_or(self.credits), g.to.clone(), None),
            None => (
                self.debit(),
                Some(self.to.clone()),
                (self.kind == MoveKind::ToCore).then_some(self.expected_duffs),
            ),
        };
        PlatformActivity {
            id: self.id.clone(),
            kind: match self.kind {
                MoveKind::ToCore => PlatformActivityKind::MoveToCore,
                MoveKind::Transfer => PlatformActivityKind::Send,
            },
            state: self.state,
            credits,
            address: None,
            to,
            expected_duffs,
            fee_credits: self.fee_taken,
            at: self.at,
            may_land: self.holds(),
            estimated: self.estimated(),
        }
    }

    /// What its inputs give: a send's amount (for Max, before the fee).
    fn given(&self) -> u64 {
        self.inputs.iter().map(|i| i.credits).sum()
    }

    /// What it shows as taken: a move what it may take; a send its amount
    /// until it is done, then what it took: its inputs and, unless the
    /// output paid it, the fee the balances show or else the estimate.
    fn debit(&self) -> u64 {
        if self.kind != MoveKind::Transfer {
            return self.credits;
        }
        if self.state != PlatformActivityState::Done || self.fee_from_output {
            return self.given();
        }
        self.given()
            .saturating_add(self.fee_taken.unwrap_or(self.fee_credits))
    }

    /// A done send whose fee the balances did not show: its debit counts
    /// the estimate.
    fn estimated(&self) -> bool {
        self.kind == MoveKind::Transfer
            && self.state == PlatformActivityState::Done
            && self.group.is_none()
            && !self.fee_from_output
            && self.fee_taken.is_none()
    }

    /// Asked again for `amount` to `to`, a send it is: its exact retry.
    fn retried_by(&self, to: &str, amount: WithdrawAmount) -> bool {
        self.kind == MoveKind::Transfer
            && self.to == to
            && match amount {
                WithdrawAmount::All => self.fee_from_output,
                WithdrawAmount::Credits { credits } => {
                    !self.fee_from_output
                        && self.inputs.iter().map(|i| i.credits).sum::<u64>() == credits
                }
            }
    }

    /// Its plan again, to sign the same transition: an exact retry of a
    /// send re-signs these terms whatever the balances are now (they only
    /// rose: a spend of them takes their nonces, and that settles it). What
    /// the fee may take is what the fee source keeps now (`balances`).
    fn replan(&self, balances: &BTreeMap<PlatformAddress, u64>) -> Option<Plan> {
        let inputs: BTreeMap<_, _> = (self.inputs.iter())
            .map(|i| parse(&i.address).map(|a| (a, i.credits)))
            .collect::<Option<_>>()?;
        let fee_source = (self.inputs.iter())
            .find(|i| i.fee_source)
            .or(self.inputs.first())
            .and_then(|i| parse(&i.address))?;
        let reserve = match self.inputs.iter().find(|i| i.fee_source) {
            Some(i) if !self.fee_from_output => {
                let now = balances.get(&fee_source).copied().unwrap_or(0);
                now.max(i.before).saturating_sub(i.credits)
            }
            _ => 0,
        };
        Some(Plan {
            reserve,
            inputs,
            fee_source,
            fee: self.fee_credits,
            fee_from_output: self.fee_from_output,
        })
    }

    /// What it pays and takes: two moves of one key with equal terms
    /// leave the same history whichever executed.
    fn terms(&self) -> Terms<'_> {
        let mut inputs: Vec<_> = (self.inputs.iter())
            .map(|i| (i.address.as_str(), i.nonce, i.credits))
            .collect();
        inputs.sort();
        (
            (self.kind, self.fee_from_output),
            inputs,
            self.fee_credits,
            self.expected_duffs,
            self.to.as_str(),
        )
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
        if self.fee_from_output {
            // The recipient's output paid the fee: the inputs fell by their
            // amounts alone, and nothing here shows the fee.
            self.fee_taken = None;
            for input in &mut self.inputs {
                input.unseen = input.credits;
            }
            return;
        }
        self.fee_taken = before
            .and_then(|b| b.checked_sub(after))
            .and_then(|fell| fell.checked_sub(withdrawn));
        let fee = self.fee_taken.unwrap_or(self.fee_credits);
        for input in &mut self.inputs {
            input.unseen = input.credits + if input.fee_source { fee } else { 0 };
        }
    }

    /// Done for its group (module doc, Moves): the inputs' proved debit
    /// since this move's plan, no payout or fee attributed, and each
    /// input's fall the group's.
    fn done_for_group(&mut self, seen: &Seen, to: Option<String>) {
        self.done(seen);
        self.fee_taken = None;
        let known = self.inputs.iter().all(|i| i.before > 0);
        let fall = |i: &MoveInput| (i.before).saturating_sub(seen.credits(&i.address).unwrap_or(0));
        let debit = known.then(|| self.inputs.iter().map(fall).sum());
        if known {
            for input in &mut self.inputs {
                input.unseen = fall(input);
            }
        }
        self.group = Some(GroupOutcome { debit, to });
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
        let Some(newest) = group.iter().copied().max_by_key(|&i| order(i)) else {
            continue;
        };
        let alike = (group.iter()).all(|&i| moves[i].terms() == moves[newest].terms());
        if alike {
            // Whichever executed, the history is the same.
            let winner = (group.iter().copied().filter(|&i| moves[i].answered))
                .max_by_key(|&i| order(i))
                .or_else(|| group.iter().copied().min_by_key(|&i| order(i)));
            for &i in &group {
                if Some(i) == winner {
                    moves[i].done(seen);
                } else {
                    // A send's identical retry superseded it; a move reads
                    // as failed (0.1).
                    moves[i].state = match moves[i].kind {
                        MoveKind::Transfer => PlatformActivityState::Replaced,
                        MoveKind::ToCore => PlatformActivityState::Failed,
                    };
                    moves[i].can_land = false;
                }
            }
        } else {
            // One of them executed and no proof says which (an answer is a
            // snapshot, not evidence of this transition): the newest
            // carries the group's outcome, the others are replaced by it.
            let to = (group.iter())
                .all(|&i| moves[i].to == moves[newest].to)
                .then(|| moves[newest].to.clone());
            moves[newest].done_for_group(seen, to);
            for &i in group.iter().filter(|&&i| i != newest) {
                moves[i].state = PlatformActivityState::Replaced;
                moves[i].can_land = false;
            }
        }
        changed.extend(group);
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
/// `height`, and the snapshot to store. `held` (a pending move's inputs, an
/// address a fund's apply pays while it runs) is not compared and keeps its
/// stored value; `unseen` (address → a done move's fall still to read)
/// absorbs the first fall of its address and is then spent, `arriving`
/// (address → an applied fund's credits still to read) the first rise,
/// and `expecting` (address → what pending funds may still add) any rise,
/// as far as it reaches, keeping the rest.
pub(crate) fn changes(
    before: Option<&Snapshot>,
    now: &Snapshot,
    held: &BTreeSet<String>,
    unseen: &mut BTreeMap<String, u64>,
    arriving: &mut BTreeMap<String, u64>,
    expecting: &mut BTreeMap<String, u64>,
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
            may_land: false,
            estimated: false,
        };
        if is > was {
            let rose = is - was;
            let applied = arriving.get_mut(address).map_or(0, std::mem::take);
            let left = rose.saturating_sub(applied);
            let pending = expecting.get_mut(address).map_or(0, |e| {
                let n = (*e).min(left);
                *e -= n;
                n
            });
            if left > pending {
                rows.push(row(PlatformActivityKind::Incoming, left - pending));
            }
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

/// The fee a transfer's fee source keeps, as a multiple of dpp's static
/// estimate: platform-wallet's own safety band for a transfer's fee
/// (`REDUCE_OUTPUT_FEE_SAFETY_MULTIPLE`, `transfer.rs:426` at the pin), as
/// Platform's metered fee may exceed the estimate (platform#3040).
pub(crate) const TRANSFER_FEE_BAND: u64 = 3;

/// The address rules of one transition kind at one protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The least an input may take (dpp `address_funds.min_input_amount`).
    pub(crate) min_input: u64,
    /// The most inputs one transition has.
    pub(crate) max_inputs: usize,
    /// The least and the most one transition takes: a withdrawal's
    /// [`withdrawal::address_range`]; a transfer's output minimum, no most.
    pub(crate) min: u64,
    pub(crate) max: u64,
    /// dpp's minimum fee: `fee_base + fee_per_input × inputs`, no change
    /// output (`AddressCreditWithdrawalTransition::estimate_min_fee`; a
    /// transfer's `AddressFundsTransferTransition::estimate_min_fee` with
    /// one output, the output's cost being the base).
    pub(crate) fee_base: u64,
    pub(crate) fee_per_input: u64,
    /// What the fee source keeps, as a multiple of the estimate: 1 for a
    /// withdrawal, [`TRANSFER_FEE_BAND`] for a transfer.
    pub(crate) band: u64,
}

impl Limits {
    /// An address credit withdrawal's ("Move to Dash wallet").
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
            band: 1,
        })
    }

    /// An address funds transfer's with one output (a Platform send).
    pub(crate) fn transfer_at(version: &PlatformVersion) -> Self {
        let funds = &version.dpp.state_transitions;
        let fees = &version.fee_version.state_transition_min_fees;
        Self {
            min_input: funds.address_funds.min_input_amount,
            max_inputs: funds.max_address_inputs as usize,
            min: (funds.address_funds.min_output_amount).max(funds.address_funds.min_input_amount),
            max: u64::MAX,
            fee_base: fees.address_funds_transfer_output_cost,
            fee_per_input: fees.address_funds_transfer_input_cost,
            band: TRANSFER_FEE_BAND,
        }
    }

    /// dpp's static estimate for `inputs`.
    pub(crate) fn fee(&self, inputs: usize) -> u64 {
        (self.fee_per_input.saturating_mul(inputs as u64)).saturating_add(self.fee_base)
    }

    /// What the fee source keeps for the fee of `inputs`.
    pub(crate) fn keep(&self, inputs: usize) -> u64 {
        self.fee(inputs).saturating_mul(self.band)
    }
}

/// What a move takes: each input's amount, the fee source (which keeps the
/// fee estimate, or the band), and the estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) inputs: BTreeMap<PlatformAddress, u64>,
    pub(crate) fee_source: PlatformAddress,
    pub(crate) fee: u64,
    /// What the fee source keeps that the fee may take, at the balances
    /// planned on: for a withdrawal the estimate, for a transfer's amount
    /// all it keeps (at least the band; nothing on Platform caps the fee
    /// below it, and a later deposit adds to it); 0 when the output pays
    /// the fee.
    pub(crate) reserve: u64,
    /// The one output pays the fee (a transfer's Max).
    pub(crate) fee_from_output: bool,
}

impl Plan {
    /// What the inputs give: what reaches Core (before the asset unlock's
    /// own fee), or a transfer's output.
    pub(crate) fn credits(&self) -> u64 {
        self.inputs.values().sum()
    }

    /// The planned debit: what the inputs give and what the fee may take
    /// of the fee source as planned (a withdrawal's estimate; a transfer's
    /// remainder).
    pub(crate) fn may_lose(&self) -> u64 {
        self.credits().saturating_add(self.reserve)
    }

    /// Whether `other` takes its fee as this plan does: from the output
    /// for both, or from the same fee source giving the same amount. Its
    /// other inputs may differ: the grant still caps the debit, and Confirm
    /// names only the fee source.
    fn takes_fee_as(&self, other: &Plan) -> bool {
        self.fee_from_output == other.fee_from_output
            && (self.fee_from_output
                || (self.fee_source == other.fee_source
                    && self.inputs.get(&self.fee_source) == other.inputs.get(&other.fee_source)))
    }

    fn fee_strategy(&self) -> AddressFundsFeeStrategy {
        if self.fee_from_output {
            return vec![AddressFundsFeeStrategyStep::ReduceOutput(0)];
        }
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
    fn error(self, kind: MoveKind, limits: &Limits) -> CreditsError {
        match (self, kind) {
            (Refused::Short { needed, available }, _) => {
                PlatformError::InsufficientCredits { needed, available }.into()
            }
            (Refused::OutOfRange, MoveKind::ToCore) => CreditsError::WithdrawalOutOfRange {
                min: limits.min,
                max: limits.max,
            },
            (Refused::OutOfRange, MoveKind::Transfer) => {
                CreditsError::SendOutOfRange { min: limits.min }
            }
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
    let (candidates, available) = candidates(balances, limits);
    // One address must hold the least withdrawal and its one-input fee.
    let least = least(limits, available);
    let Some(&source) = candidates.first() else {
        return Err(least);
    };
    let want = match amount {
        WithdrawAmount::All => {
            let all = (candidates.iter().map(|(_, c)| *c).sum::<u64>())
                .saturating_sub(limits.keep(candidates.len()));
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
    plan_from(&candidates, source, want, limits, available)
}

/// The addresses a plan may take from (at least the input minimum, the
/// largest first, at most `max_inputs`) and what all of them hold.
fn candidates(
    balances: &BTreeMap<PlatformAddress, u64>,
    limits: &Limits,
) -> (Vec<(PlatformAddress, u64)>, u64) {
    let available: u64 = balances.values().sum();
    let mut candidates: Vec<(PlatformAddress, u64)> = (balances.iter())
        .filter(|(_, c)| **c >= limits.min_input)
        .map(|(a, c)| (*a, *c))
        .collect();
    candidates.sort_by_key(|(a, c)| (std::cmp::Reverse(*c), *a));
    candidates.truncate(limits.max_inputs.max(1));
    (candidates, available)
}

fn least(limits: &Limits, available: u64) -> Refused {
    Refused::Short {
        needed: limits.min.saturating_add(limits.keep(1)),
        available,
    }
}

/// [`plan`] of `want` with `source` (one of `candidates`) as the fee
/// source: it gives first, keeping the fee's reserve; the others give the
/// rest, the largest first; a last input below the minimum borrows from
/// the one before.
fn plan_from(
    candidates: &[(PlatformAddress, u64)],
    (source, top): (PlatformAddress, u64),
    want: u64,
    limits: &Limits,
    available: u64,
) -> Result<Plan, Refused> {
    let others: Vec<_> = (candidates.iter()).filter(|(a, _)| *a != source).collect();
    let mut count = 1;
    for _ in 0..=candidates.len() {
        let keep = limits.keep(count);
        let first = top.saturating_sub(keep).min(want);
        if first < limits.min_input {
            return Err(least(limits, available));
        }
        let mut taken = vec![(source, first)];
        let mut left = want - first;
        for &&(address, credits) in &others {
            if left == 0 {
                break;
            }
            let take = credits.min(left);
            taken.push((address, take));
            left -= take;
        }
        if left > 0 {
            return Err(Refused::Short {
                needed: want.saturating_add(keep),
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
            fee: limits.fee(count),
            reserve: keep,
            fee_from_output: false,
        });
    }
    Err(Refused::OutOfRange)
}

/// A Platform send's plan (`limits`: [`Limits::transfer_at`]).
///
/// An amount is [`plan`]'s with the fee source chosen to bound the fee:
/// `DeductFromInput` takes Platform's metered fee from what the fee source
/// keeps, all of it if need be (dpp
/// `deduct_fee_from_inputs_and_outputs/v0/mod.rs:38-49`; nothing caps it),
/// so of the addresses that can keep the band the plan takes the one left
/// with the least, and [`Plan::reserve`] is that remainder: what the fee
/// can take at these balances (a later deposit to it adds to that). It is refused only when no address can carry the amount
/// and the band.
///
/// `All` (Max) moves every input `plan` would take, in full, to the one
/// output, which pays the fee (`ReduceOutput`): the inputs give exactly
/// their balances, and the fee comes out of the output, as much as it
/// meters. The output must carry the band. Dust below the input minimum,
/// and inputs past the most one transition takes, stay (a second Max takes
/// the rest).
pub(crate) fn transfer_plan(
    balances: &BTreeMap<PlatformAddress, u64>,
    amount: WithdrawAmount,
    limits: &Limits,
) -> Result<Plan, Refused> {
    let (candidates, available) = candidates(balances, limits);
    if let WithdrawAmount::Credits { credits } = amount {
        let bounded = |mut p: Plan| {
            let held = balances.get(&p.fee_source).copied().unwrap_or(0);
            p.reserve = held.saturating_sub(p.inputs.get(&p.fee_source).copied().unwrap_or(0));
            p
        };
        // The largest fee source's plan decides whether it goes at all.
        let mut best = bounded(plan(balances, amount, limits)?);
        for &source in candidates.iter().skip(1) {
            if let Ok(p) = plan_from(&candidates, source, credits, limits, available) {
                let p = bounded(p);
                if (p.reserve, p.inputs.len()) < (best.reserve, best.inputs.len()) {
                    best = p;
                }
            }
        }
        return Ok(best);
    }
    let keep = limits.keep(candidates.len().max(1));
    let total: u64 = candidates.iter().map(|(_, c)| *c).sum();
    let Some(&(source, _)) = candidates.first() else {
        return Err(Refused::Short {
            needed: limits.min.saturating_add(limits.keep(1)),
            available,
        });
    };
    if total < limits.min.saturating_add(keep) {
        return Err(Refused::Short {
            needed: limits.min.saturating_add(keep),
            available,
        });
    }
    Ok(Plan {
        fee: limits.fee(candidates.len()),
        inputs: candidates.into_iter().collect(),
        fee_source: source,
        reserve: 0,
        fee_from_output: true,
    })
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
    /// [`Self::version`], asked of Platform once first when it is below
    /// address funds: the SDK starts at the network's floor until a proof
    /// raises it, so a gate read before any proof is not refused on that.
    fn sends_version(&self) -> impl Future<Output = &'static PlatformVersion> + Send {
        std::future::ready(self.version())
    }
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
    /// Signs the transfer of `inputs` to the one output `to` with `signer`
    /// and broadcasts it; `Ok` once Platform answered with the addresses'
    /// state.
    fn transfer(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        to: (PlatformAddress, u64),
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

    async fn sends_version(&self) -> &'static PlatformVersion {
        use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION;
        if self.sdk.protocol_version_number() < ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION {
            super::protocol::refreshed(&self.sdk).await;
        }
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

    async fn transfer(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        (to, credits): (PlatformAddress, u64),
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), PlatformWalletError> {
        (self.wallet.platform())
            .transfer(
                ACCOUNT,
                InputSelection::ExplicitWithNonces(inputs),
                [(to, credits)].into(),
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
    let provenance = session.live()?.provenance;
    let waits = session.platform.money_gate.waits();
    let deadline = tokio::time::Instant::now() + waits.wait;
    let (height, infos) = loop {
        let (read, tally) = trust::scoped(net.read(addresses.clone())).await;
        let read = read?;
        // Sampled after the read: the read itself may set the latch.
        let gated = session.trust.rule_5() == Rule5::Enforce && provenance.fallback_used();
        if !gated || tally.verified() {
            break read;
        }
        if !wait || tokio::time::Instant::now() + waits.retry >= deadline {
            return Err(Refusal::Unverified.platform());
        }
        tokio::time::sleep(waits.retry).await;
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

    async fn funds(&self) -> Result<Vec<FundRecord>, PlatformError> {
        Ok((self.all(FUND_PREFIX).await?.into_iter())
            .map(|(_, f)| f)
            .collect())
    }

    async fn fund(&self, txid: &str) -> Result<Option<FundRecord>, PlatformError> {
        let key = format!("{FUND_PREFIX}{txid}");
        let value = self
            .blocking(move |db, scope| db.setting(scope, &key))
            .await?;
        Ok(value.and_then(|v| serde_json::from_str(&v).ok()))
    }

    /// The recorded moves, oldest first. A `pending` one may have been
    /// signed and may execute, whatever the process got to record before
    /// it stopped (module doc, Moves): it holds its inputs' nonces.
    async fn moves(&self) -> Result<Vec<MoveRecord>, PlatformError> {
        let mut moves: Vec<MoveRecord> = (self.all(MOVE_PREFIX).await?.into_iter())
            .map(|(_, m)| m)
            .collect();
        for m in &mut moves {
            if m.state == PlatformActivityState::Pending {
                m.can_land = true;
            }
        }
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

/// A write of the app.sqlite settings: the key and its value (`None`
/// deletes it).
type Write = (String, Option<String>);

fn fund_write(record: &FundRecord) -> Result<Write, PlatformError> {
    Ok((format!("{FUND_PREFIX}{}", record.txid), Some(json(record)?)))
}

fn fund_delete(txid: &str) -> Write {
    (format!("{FUND_PREFIX}{txid}"), None)
}

pub(super) fn bech32m(address: &PlatformAddress, network: Network) -> String {
    address.to_bech32m_string(network)
}

pub(super) fn parse(address: &str) -> Option<PlatformAddress> {
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

/// The library's refusal of a pass while one is in flight
/// (`platform_address_sync.rs:436-439` at the pin); any other `AddressSync`
/// error is the pass's own failure.
const PASS_IN_FLIGHT: &str = "a platform-address sync pass is already in flight";

fn pass_in_flight(e: &PlatformWalletError) -> bool {
    matches!(e, PlatformWalletError::AddressSync(m) if m.starts_with(PASS_IN_FLIGHT))
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
        Err(e) if pass_in_flight(&e) && sync.is_syncing() => {
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
/// in one durable write (module doc), the addresses of `held_funds` not
/// compared. Whether anything changed.
async fn observe(
    store: &Store,
    (moves, funds): (&mut [MoveRecord], &mut [FundRecord]),
    held_funds: BTreeSet<String>,
    seen: &Seen,
) -> Result<bool, PlatformError> {
    let before = moves.to_vec();
    let funds_before = funds.to_vec();
    settle(moves, seen);
    let baseline = store.baseline().await?;
    let mut writes = Vec::new();
    let mut rows = 0;
    // A proof older than the baseline's (a node behind) decides nothing.
    if baseline.as_ref().is_none_or(|b| seen.height >= b.height) {
        let held: BTreeSet<String> = (moves.iter())
            .filter(|m| m.state == PlatformActivityState::Pending)
            .flat_map(|m| m.inputs.iter().map(|i| i.address.clone()))
            .chain(held_funds)
            .collect();
        let mut unseen: BTreeMap<String, u64> = BTreeMap::new();
        for input in moves.iter().flat_map(|m| &m.inputs) {
            *unseen.entry(input.address.clone()).or_default() += input.unseen;
        }
        let mut arriving: BTreeMap<String, u64> = BTreeMap::new();
        let mut expected: BTreeMap<String, u64> = BTreeMap::new();
        for fund in funds.iter() {
            *arriving.entry(fund.address.clone()).or_default() += fund.unseen;
            *expected.entry(fund.address.clone()).or_default() += fund.expects();
        }
        let mut expecting = expected.clone();
        let (found, next) = changes(
            baseline.as_ref().map(|b| &b.balances),
            &seen.balances(),
            &held,
            &mut unseen,
            &mut arriving,
            &mut expecting,
            (unix_now(), seen.height),
        );
        // What the pending funds took of a rise, the oldest's first.
        let mut took: BTreeMap<String, u64> = (expected.into_iter())
            .map(|(a, e)| {
                let left = expecting.get(&a).copied().unwrap_or(0);
                (a, e - left)
            })
            .collect();
        let mut pending: Vec<&mut FundRecord> =
            funds.iter_mut().filter(|f| f.expects() > 0).collect();
        pending.sort_by(|a, b| (a.at, &a.txid).cmp(&(b.at, &b.txid)));
        for fund in pending {
            if let Some(left) = took.get_mut(&fund.address) {
                let n = fund.expects().min(*left);
                fund.absorbed += n;
                *left -= n;
            }
        }
        // A done move's fall and an applied fund's rise, once read, are
        // spent.
        for input in moves.iter_mut().flat_map(|m| &mut m.inputs) {
            if unseen.get(&input.address) == Some(&0) {
                input.unseen = 0;
            }
        }
        for fund in funds.iter_mut() {
            if arriving.get(&fund.address) == Some(&0) {
                fund.unseen = 0;
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
    for (f, was) in funds.iter().zip(&funds_before) {
        if f != was {
            writes.push(fund_write(f)?);
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
    let mut funds = store.funds().await?;
    let addresses = covered(&pool(wallet).await?, &moves);
    let held = fund_holds(session, wallet_id);
    let pending = (funds.iter())
        .filter(|f| f.state == PlatformActivityState::Pending)
        .filter_map(FundRecord::outpoint);
    let ev = super::unfinished_funds::fund_evidence(session, wallet_id, wallet, pending).await;
    let completed = reconcile(&store, &mut funds, &ev, &held).await?;
    let observed = match read(session, net, addresses, false).await {
        Ok(seen) => observe(&store, (&mut moves, &mut funds), held, &seen).await?,
        Err(e) => {
            // Nothing is known: what is stored stands.
            tracing::info!("reading the Platform addresses for their activity: {e}");
            false
        }
    };
    let changed = completed || observed;
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
        .chain(funds.iter().map(FundRecord::activity))
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(out)
}

/// Where a move pays.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// One of the wallet's Core receiving addresses (a withdrawal).
    Core(&'a str),
    /// A Platform address, checked by [`recipient`] (a transfer).
    Platform(PlatformAddress),
}

impl Target<'_> {
    fn kind(&self) -> MoveKind {
        match self {
            Target::Core(_) => MoveKind::ToCore,
            Target::Platform(_) => MoveKind::Transfer,
        }
    }

    fn limits(&self, version: &PlatformVersion) -> Result<Limits, PlatformError> {
        match self {
            Target::Core(_) => Limits::at(version),
            Target::Platform(_) => Ok(Limits::transfer_at(version)),
        }
    }

    fn plan(
        &self,
        balances: &BTreeMap<PlatformAddress, u64>,
        amount: WithdrawAmount,
        limits: &Limits,
    ) -> Result<Plan, CreditsError> {
        match self {
            Target::Core(_) => plan(balances, amount, limits),
            Target::Platform(_) => transfer_plan(balances, amount, limits),
        }
        .map_err(|r| r.error(self.kind(), limits))
    }
}

/// The plan of a move to `target` while `held` may still execute: from its
/// inputs only, so it signs at the same nonces (a send beside a hold is
/// only the exact retry its quote named, [`run`]).
fn plan_beside(
    target: &Target<'_>,
    mut balances: BTreeMap<PlatformAddress, u64>,
    amount: WithdrawAmount,
    limits: &Limits,
    held: Option<&MoveRecord>,
    network: Network,
) -> Result<Plan, CreditsError> {
    if let Some(held) = held {
        let only: BTreeSet<&str> = held.inputs.iter().map(|i| i.address.as_str()).collect();
        balances.retain(|a, _| only.contains(bech32m(a, network).as_str()));
    }
    target.plan(&balances, amount, limits)
}

/// A quoted send (`PlatformSendQuote::operation`): what `platform_send`
/// pays, once. `retry_of`: the held send it re-signs; `None`, a new send,
/// which any hold refuses. `credits`: the quote's planned debit, the cap
/// of the grant issued for it; `grant`: that grant, the one that pays it.
/// `plan`: what the quote showed; a new send signs only a plan that takes
/// its fee the same way.
#[derive(Debug)]
struct SendOp {
    wallet: WalletId,
    to: String,
    amount: WithdrawAmount,
    retry_of: Option<String>,
    credits: u64,
    grant: Option<String>,
    plan: Plan,
}

/// The session's quoted sends not yet sent, the newest [`OPS_KEPT`]. Ids
/// are 128 random bits, so none repeats in this session or another (a
/// restart): an id from before matches nothing.
#[derive(Debug, Default)]
pub(crate) struct SendOps(Mutex<Vec<(String, SendOp)>>);

const OPS_KEPT: usize = 256;

fn unknown_op() -> PlatformError {
    PlatformError::InvalidArgument {
        detail: "no quoted payment has that operation id; quote it again".into(),
    }
}

impl SendOps {
    fn ops(&self) -> std::sync::MutexGuard<'_, Vec<(String, SendOp)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn issue(&self, op: SendOp) -> String {
        let id = format!("op-{:032x}", rand::random::<u128>());
        let mut ops = self.ops();
        ops.push((id.clone(), op));
        if ops.len() > OPS_KEPT {
            ops.remove(0);
        }
        id
    }

    fn with<T>(
        &self,
        id: &str,
        wallet: WalletId,
        f: impl FnOnce(&mut Vec<(String, SendOp)>, usize) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let mut ops = self.ops();
        let at = (ops.iter())
            .position(|(i, op)| i == id && op.wallet == wallet)
            .ok_or_else(unknown_op)?;
        f(&mut ops, at)
    }

    /// The cap of the grant for the quoted send `id` of `wallet`: its
    /// planned debit.
    pub(crate) fn cap(&self, id: &str, wallet: WalletId) -> Result<u64, PlatformError> {
        self.with(id, wallet, |ops, at| Ok(ops[at].1.credits))
    }

    /// Makes `grant`, just issued for it, the grant that pays `id`, and
    /// returns the grant it replaces, which no longer pays it.
    pub(crate) fn bind(
        &self,
        id: &str,
        wallet: WalletId,
        grant: &str,
    ) -> Result<Option<String>, PlatformError> {
        self.with(id, wallet, |ops, at| {
            Ok(ops[at].1.grant.replace(grant.to_owned()))
        })
    }

    /// Takes the quoted send `id` of `wallet` to pay under `grant`: each is
    /// sent at most once, and only under the grant issued for it. Any other
    /// grant (one issued for another send, or for none) is
    /// `platform.grant_invalid` before anything is redeemed, and the send
    /// stays quoted.
    fn take(&self, id: &str, wallet: WalletId, grant: &str) -> Result<SendOp, PlatformError> {
        self.with(id, wallet, |ops, at| {
            if ops[at].1.grant.as_deref() != Some(grant) {
                return Err(PlatformError::GrantInvalid);
            }
            Ok(ops.remove(at).1)
        })
    }

    /// A new process's registry: nothing quoted.
    #[cfg(test)]
    pub(crate) fn reset(&self) {
        self.ops().clear();
    }
}

/// `to` as a Platform address a send of the wallet with `pool` may pay on
/// `network`: a DIP-18 P2PKH or P2SH address whose HRP is the network's
/// (dw-uri's classification, as Pay's; dpp's own parse takes either HRP),
/// and none of the wallet's own.
pub(crate) fn recipient(
    network: Network,
    to: &str,
    pool: &BTreeMap<PlatformAddress, DerivationPath>,
) -> Result<PlatformAddress, RecipientProblem> {
    use dw_uri::keyio::{AddressKind, PlatformDestination, classify_address};
    let address = match classify_address(to, network) {
        AddressKind::Platform(PlatformDestination::PubKeyHash(h)) => PlatformAddress::P2pkh(h),
        AddressKind::Platform(PlatformDestination::ScriptHash(h)) => PlatformAddress::P2sh(h),
        AddressKind::Core(_) => return Err(RecipientProblem::Core),
        AddressKind::Shielded(_) => return Err(RecipientProblem::Shielded),
        AddressKind::Invalid(_) => {
            let other = match network {
                Network::Mainnet => Network::Testnet,
                _ => Network::Mainnet,
            };
            return Err(match classify_address(to, other) {
                AddressKind::Invalid(_) => RecipientProblem::Malformed,
                _ => RecipientProblem::Network,
            });
        }
    };
    if pool.contains_key(&address) {
        return Err(RecipientProblem::Own);
    }
    Ok(address)
}

/// Platform sends need address funds transitions
/// (`ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION`): below it a send is
/// `platform.feature_off{feature: "platform_addresses"}`.
pub(super) fn sends_active(version: &PlatformVersion) -> Result<(), PlatformError> {
    use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION;
    if version.protocol_version < ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION {
        return Err(PlatformError::FeatureOff {
            feature: "platform_addresses".into(),
        });
    }
    Ok(())
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
    let moves = Store::of(session, wallet_id)?.moves().await?;
    let seen = read(session, net, covered(&pool, &moves), true).await?;
    // As the move will decide, before anything is asked of the user: a
    // send that may still execute holds the next move (it must have the
    // send's exact terms, which no move has).
    if let Some(h) =
        (moves.iter()).find(|m| m.kind == MoveKind::Transfer && m.holds() && !m.reached(&seen))
    {
        return Err(CreditsError::MovePending { id: h.id.clone() });
    }
    let version = net.version();
    let target = Target::Core("");
    let limits = target.limits(version)?;
    let planned = target.plan(&proved_balances(&pool, &seen, network), amount, &limits)?;
    let to = session
        .current_receiving_address(wallet_id)
        .await
        .map_err(PlatformError::from)?;
    let credits = planned.may_lose();
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

/// `DashPay::platform_send_quote`.
pub(crate) async fn send_quote(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (to, amount, retry_of): (&str, WithdrawAmount, Option<&str>),
) -> Result<PlatformSendQuote, CreditsError> {
    let network = session.vault.network();
    let pool = pool(wallet).await?;
    let address = recipient(network, to, &pool)
        .map_err(|reason| CreditsError::RecipientInvalid { reason })?;
    let version = net.sends_version().await;
    sends_active(version)?;
    let moves = Store::of(session, wallet_id)?.moves().await?;
    let seen = read(session, net, covered(&pool, &moves), true).await?;
    let target = Target::Platform(address);
    let limits = target.limits(version)?;
    let to = bech32m(&address, network);
    let balances = proved_balances(&pool, &seen, network);
    // As the send will decide, before anything is asked of the user: a
    // move or send that may still execute refuses this one, unless this is
    // its exact retry. A retry the host names stays that send's retry
    // whatever it became (one that landed is answered, never paid again).
    let held = match retry_of {
        Some(id) => Some(retried(&moves, id, (&to, amount))?),
        None => moves.iter().find(|m| m.holds()),
    };
    let (planned, retry_of) = match held {
        Some(h) if retry_of.is_some() || h.retried_by(&to, amount) => {
            if h.state == PlatformActivityState::Pending && !h.reached(&seen) {
                return Err(CreditsError::MovePending { id: h.id.clone() });
            }
            let planned = h.replan(&balances).ok_or(PlatformError::Unavailable)?;
            (planned, Some(h.id.clone()))
        }
        Some(h) if !h.reached(&seen) => {
            return Err(CreditsError::MovePending { id: h.id.clone() });
        }
        _ => (target.plan(&balances, amount, &limits)?, None),
    };
    let credits = planned.may_lose();
    let operation = session.platform.send_ops.issue(SendOp {
        wallet: wallet_id,
        to: to.clone(),
        amount,
        retry_of: retry_of.clone(),
        credits,
        grant: None,
        plan: planned.clone(),
    });
    Ok(PlatformSendQuote {
        credits,
        amount_credits: planned.credits(),
        fee_credits: planned.fee,
        fee_source_credits: (!planned.fee_from_output).then_some(planned.reserve),
        fee_source: (!planned.fee_from_output).then(|| bech32m(&planned.fee_source, network)),
        fee_from_amount: planned.fee_from_output,
        to,
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: credits,
        },
        operation,
        retry_of,
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
    let record = run(
        net,
        session,
        wallet,
        wallet_id,
        (amount, Target::Core(to), None, None),
        grant,
    )
    .await?;
    let row = record.activity();
    Ok(MoveToCoreOutcome {
        id: record.id.clone(),
        credits: row.credits,
        expected_duffs: row.expected_duffs,
        to: row.to,
        state: record.state,
        fee_credits: row.fee_credits,
    })
}

/// `DashPay::platform_send` (module doc): the quoted `operation`, as
/// `move_to_core` moves, to the Platform address its quote checked.
pub(crate) async fn send(
    net: &impl MoveNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    operation: &str,
    grant: &str,
) -> Result<PlatformSendOutcome, CreditsError> {
    let op = (session.platform.send_ops).take(operation, wallet_id, grant)?;
    let address = recipient(session.vault.network(), &op.to, &pool(wallet).await?)
        .map_err(|reason| CreditsError::RecipientInvalid { reason })?;
    sends_active(net.sends_version().await)?;
    let ask = (
        op.amount,
        Target::Platform(address),
        op.retry_of.as_deref(),
        Some(&op.plan),
    );
    let record = run(net, session, wallet, wallet_id, ask, grant).await?;
    let row = record.activity();
    Ok(PlatformSendOutcome {
        id: record.id.clone(),
        credits: row.credits,
        amount_credits: record.given(),
        fee_from_amount: record.fee_from_output,
        to: row.to,
        state: record.state,
        may_land: row.may_land,
        fee_credits: row.fee_credits,
        estimated: row.estimated,
    })
}

/// What a send naming the held send `retry_of` (its quote's exact retry)
/// does, under the wallet's lock after the read settled the records:
/// `Ok(Some)` the record whose settled outcome it answers, signing
/// nothing; `Ok(None)` re-sign it; else refused.
fn retry_decision<'m>(
    moves: &'m [MoveRecord],
    retry_of: &str,
    ask: (&str, WithdrawAmount),
) -> Result<Option<&'m MoveRecord>, CreditsError> {
    let held = retried(moves, retry_of, ask)?;
    match held.state {
        PlatformActivityState::Done => Ok(Some(held)),
        PlatformActivityState::Pending => Err(CreditsError::MovePending {
            id: held.id.clone(),
        }),
        _ if held.holds() => Ok(None),
        // Its identical retry executed in its place, or it never can.
        _ => Ok(Some(
            (moves.iter())
                .find(|m| {
                    m.state == PlatformActivityState::Done
                        && m.key() == held.key()
                        && m.terms() == held.terms()
                })
                .unwrap_or(held),
        )),
    }
}

/// The send `id` that a send of `amount` to `to` retries: refused when there
/// is none or that is not its exact retry.
fn retried<'m>(
    moves: &'m [MoveRecord],
    id: &str,
    (to, amount): (&str, WithdrawAmount),
) -> Result<&'m MoveRecord, CreditsError> {
    let held = (moves.iter().find(|m| m.id == id)).ok_or(PlatformError::InvalidArgument {
        detail: "no send has that id".into(),
    })?;
    if !held.retried_by(to, amount) {
        return Err(PlatformError::InvalidArgument {
            detail: "not that send's retry: the same address and amount".into(),
        }
        .into());
    }
    Ok(held)
}

/// One move to `target` (module doc, Moves); the record as it stands when
/// the call returns.
async fn run(
    net: &impl MoveNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (amount, target, retry_of, quoted): (WithdrawAmount, Target<'_>, Option<&str>, Option<&Plan>),
    grant: &str,
) -> Result<MoveRecord, CreditsError> {
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let network = session.vault.network();
    let store = Store::of(session, wallet_id)?;
    let mut moves = store.moves().await?;
    let pool = pool(wallet).await?;
    // Rule 5 and the one read every decision below rests on.
    let seen = read(session, net, covered(&pool, &moves), true).await?;
    let mut funds = store.funds().await?;
    let held = fund_holds(session, wallet_id);
    if observe(&store, (&mut moves, &mut funds), held, &seen).await? {
        session
            .hub
            .pump
            .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    }
    let version = net.version();
    let limits = target.limits(version)?;
    let balances = proved_balances(&pool, &seen, network);
    // A move that may still execute: the next one draws from its inputs only,
    // so it signs at the same nonces (checked below); a send beside it is
    // only its exact retry, and a retry of one that landed pays nothing.
    let (held, planned) = match (target, retry_of) {
        (Target::Platform(to), Some(id)) => {
            let to = bech32m(&to, network);
            if let Some(settled) = retry_decision(&moves, id, (&to, amount))? {
                return Ok(settled.clone());
            }
            let held = (moves.iter()).find(|m| m.id == id);
            let planned =
                (held.and_then(|h| h.replan(&balances))).ok_or(PlatformError::Unavailable)?;
            (held, planned)
        }
        _ => {
            if let Some(pending) =
                (moves.iter()).find(|m| m.state == PlatformActivityState::Pending)
            {
                return Err(CreditsError::MovePending {
                    id: pending.id.clone(),
                });
            }
            let held = moves.iter().find(|m| m.holds());
            if let (Target::Platform(_), Some(h)) = (target, held) {
                // A new send: the hold came after its quote.
                return Err(CreditsError::MovePending { id: h.id.clone() });
            }
            let planned = plan_beside(&target, balances, amount, &limits, held, network)?;
            // A new send signs the fee its Confirm showed or nothing: the
            // read since the quote may pick another fee source.
            if quoted.is_some_and(|q| !q.takes_fee_as(&planned)) {
                return Err(CreditsError::QuoteChanged);
            }
            (held, planned)
        }
    };
    let mut inputs = BTreeMap::new();
    for (address, credits) in &planned.inputs {
        let nonce = seen
            .nonce(&bech32m(address, network))
            .ok_or(PlatformError::Unavailable)?;
        inputs.insert(*address, (nonce.saturating_add(1), *credits));
    }
    let (to, expected_duffs) = match target {
        Target::Core(to) => (
            to.to_owned(),
            withdrawal::expected_duffs(planned.credits(), version)?,
        ),
        Target::Platform(address) => (bech32m(&address, network), 0),
    };
    let mut record = MoveRecord {
        id: format!("move-{}-{}", seen.height, moves.len()),
        kind: target.kind(),
        inputs: (inputs.iter())
            .map(|(a, (nonce, credits))| {
                let address = bech32m(a, network);
                MoveInput {
                    before: seen.credits(&address).unwrap_or(0),
                    address,
                    nonce: *nonce,
                    credits: *credits,
                    fee_source: !planned.fee_from_output && *a == planned.fee_source,
                    unseen: 0,
                }
            })
            .collect(),
        credits: planned.may_lose(),
        fee_credits: planned.fee,
        expected_duffs,
        to,
        fee_from_output: planned.fee_from_output,
        at: unix_now(),
        state: PlatformActivityState::Pending,
        signed: false,
        answered: false,
        // Until a definitive refusal says otherwise, it may be signed and
        // may execute: so it is recorded, and so a restart reads it.
        can_land: true,
        seen_at: None,
        fee_taken: None,
        group: None,
        version: Some(version.protocol_version),
    };
    // A move `failed` by height may still execute: only one at its very
    // inputs and nonces may follow it, and when either pays a Platform
    // address (someone else's) only its exact retry, so whichever of the
    // two lands pays what the user asked, once.
    let blocks = |h: &&MoveRecord| {
        h.key() != record.key()
            || ((h.kind == MoveKind::Transfer || record.kind == MoveKind::Transfer)
                && h.terms() != record.terms())
    };
    if let Some(held) = held.filter(blocks) {
        return Err(CreditsError::MovePending {
            id: held.id.clone(),
        });
    }
    let script = match target {
        Target::Core(to) => Some(own_core_destination(session, wallet_id, network, to).await?),
        Target::Platform(_) => None,
    };
    // The authority before anything is redeemed: a refusal keeps the grant.
    // A send pays someone else: only a fresh grant, redeemed into a lease
    // for this send alone, never a lease another action holds.
    let flow = match target {
        Target::Core(_) => FlowKind::PlatformAddressMove,
        Target::Platform(_) => FlowKind::PlatformAddressSend,
    };
    let authority = match session.lease_for(&wallet_id, grant) {
        Some(lease) => {
            let lease = lease.map_err(PlatformError::from)?;
            if flow != FlowKind::PlatformAddressMove || lease.flow() != flow {
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
            .begin_lease(wallet_id, flow, &[grant])
            .await
            .map_err(PlatformError::from)?,
    };
    let label = match target {
        Target::Core(_) => "move_to_core",
        Target::Platform(_) => "platform_send",
    };
    let permit = write_permit(&lease, label, record.credits)?;
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
    let strategy = planned.fee_strategy();
    let call = match (target, script) {
        (Target::Platform(address), _) => {
            let output = (address, planned.credits());
            leased(&lease, net.transfer(inputs, output, strategy, &signer)).await
        }
        (Target::Core(_), Some(script)) => {
            leased(&lease, net.withdraw(inputs, script, strategy, &signer)).await
        }
        (Target::Core(_), None) => unreachable!("a withdrawal has its destination"),
    };
    // The testnet smoke's restart (release 0.1, review r2): the process
    // ends between the call and its outcome, as a crash would there.
    #[cfg(all(feature = "test-fund", debug_assertions))]
    if std::env::var_os("DW_TEST_EXIT_AFTER_MOVE_CALL").is_some() {
        tracing::warn!(id = %record.id, "DW_TEST_EXIT_AFTER_MOVE_CALL: exiting");
        std::process::exit(86);
    }
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
    // An identical transition at the same protocol version that Platform's
    // stateless checks refused: the held send it retried never executes
    // either. A signer's shortfall says nothing about the held one.
    let identical_refused = released == record.inputs.len() as u64
        && call.as_ref().err().is_some_and(never_valid)
        && retry_of.is_some();
    if identical_refused
        && let Some(h) = (moves.iter_mut()).find(|m| Some(m.id.as_str()) == retry_of)
        && h.holds()
        && h.version == record.version
        && h.terms() == record.terms()
    {
        tracing::info!(id = %h.id, "an identical retry was refused for good: the held send never executes");
        h.state = PlatformActivityState::Failed;
        h.can_land = false;
    }
    moves.push(record);
    let at = moves.len() - 1;
    if cannot_land {
        moves[at].state = PlatformActivityState::Failed;
        moves[at].can_land = false;
    } else {
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
    if let Err(e) = commit_outcome(&store, writes).await {
        // The record stored before signing stands, `pending` and able to
        // land: the next read settles it by its nonces.
        tracing::warn!(id = %moves[at].id, "storing a move's outcome: {e}");
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    if cannot_land && let Err(e) = call {
        return Err(PlatformError::from(e).into());
    }
    Ok(moves.swap_remove(at))
}

/// `to` as the script of one of the wallet's Core receiving addresses on
/// `network`, the only destination a withdrawal pays.
async fn own_core_destination(
    session: &NetworkSession,
    wallet_id: WalletId,
    network: Network,
    to: &str,
) -> Result<CoreScript, PlatformError> {
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
        });
    }
    Ok(script)
}

/// `DashPay::platform_receive_address`: the lowest unused address of the
/// payment account (platform-wallet's `next_unused_receive_address`, from
/// public key material; the same one until a sync pass sees it funded).
pub(crate) async fn receive_address(
    net: &impl MoveNet,
    wallet: &PlatformWallet,
    network: Network,
) -> Result<PlatformReceiveAddress, PlatformError> {
    // It ships with the send: below address funds, nothing could spend it.
    sends_active(net.sends_version().await)?;
    let key = key_wallet::account::account_collection::PlatformPaymentAccountKey {
        account: ACCOUNT,
        key_class: 0,
    };
    let address = wallet
        .platform()
        .next_unused_receive_address(key)
        .await
        .map_err(PlatformError::from)?;
    let index = (pool(wallet).await?.get(&address))
        .and_then(|path| path.as_ref().last().copied())
        .map(u32::from)
        .ok_or_else(|| PlatformError::Internal {
            detail: "the receive address is not in the payment account".into(),
        })?;
    Ok(PlatformReceiveAddress {
        address: bech32m(&address, network),
        index,
    })
}

/// The outcome's write; tests make it fail.
async fn commit_outcome(
    store: &Store,
    writes: Vec<(String, Option<String>)>,
) -> Result<(), PlatformError> {
    #[cfg(test)]
    if tests::outcome_write_fails(&store.appdb) {
        return Err(PlatformError::Storage {
            detail: "injected".into(),
        });
    }
    store.commit(writes).await
}

#[cfg(all(feature = "test-support", debug_assertions))]
impl NetworkSession {
    /// **Test only** (dw-app's tests of `authorize_platform_send`; no facade
    /// call): records a quoted send of `credits` for `wallet_id`, as
    /// `platform_send_quote` does but with no read, and returns its
    /// operation.
    #[doc(hidden)]
    pub fn quote_send_for_test(&self, wallet_id: WalletId, credits: u64) -> String {
        self.platform.send_ops.issue(SendOp {
            wallet: wallet_id,
            to: String::new(),
            amount: WithdrawAmount::Credits { credits },
            retry_of: None,
            credits,
            grant: None,
            plan: Plan {
                inputs: BTreeMap::new(),
                fee_source: PlatformAddress::P2pkh([0; 20]),
                fee: 0,
                reserve: 0,
                fee_from_output: true,
            },
        })
    }

    /// **Test only**: forgets the quoted send `operation`, as its send or
    /// an eviction does.
    #[doc(hidden)]
    pub fn forget_send_for_test(&self, operation: &str) {
        self.platform
            .send_ops
            .ops()
            .retain(|(id, _)| id != operation);
    }
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
                // The input bound (`lock_inputs`), before a grant is redeemed.
                super::lock_inputs::check(&wallet, duffs).await?;
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
                    let lock = super::lock_inputs::Capped::new(vault.signer(&w, &token)?);
                    Ok::<_, dw_vault::VaultError>((address, lock))
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
                let funded = wallet
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
                    .await;
                match funded {
                    Ok(_) => {}
                    Err(_) if lock_signer.refused() => {
                        return Err(super::lock_inputs::too_many(None).into());
                    }
                    Err(e) => return Err(PlatformError::from(e).into()),
                }
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
pub(crate) mod tests;
