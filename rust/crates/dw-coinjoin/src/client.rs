//! One mixing session with one masternode, client side (Dash Core
//! `CCoinJoinClientSession`, `src/coinjoin/client.cpp`):
//!
//! 1. connect with `senddsq` and send `dsa` (denomination + collateral),
//!    state `Queue` (`JoinExistingQueue`/`StartNewQueue`,
//!    `ProcessPendingDsaRequest`, client.cpp:1384-1696);
//! 2. the masternode accepts with a `dssu` carrying the session id
//!    (`ProcessPoolStateUpdate`, client.cpp:414-457);
//! 3. on its ready `dsq` (operator-signed), select inputs of the session
//!    denomination, reserve fresh outputs and send `dsi`
//!    (`SubmitDenominate`/`PrepareDenominate`/`SendDenominate`,
//!    client.cpp:1740-1894, 354-411), state `AcceptingEntries`;
//! 4. on `dsf`, check the final transaction and sign our inputs with
//!    `SIGHASH_ALL | SIGHASH_ANYONECANPAY`, send `dss`, state `Signing`
//!    (`SignFinalTransaction`, client.cpp:464-631);
//! 5. `dsc` ends the session; on success the inputs stay locked until the
//!    wallet sees them spent (`CompletedTransaction`, client.cpp:634-674).
//!
//! A rejection, a timeout (queue 30 s / signing 15 s plus Core's 10 s lag,
//! `CheckTimeout`, client.cpp:304-333) or a dropped connection fails the
//! session: our inputs are unlocked and the reserved outputs released;
//! once our signatures are out (`dss`), a stop, timeout or dropped
//! connection is no verdict: the inputs stay held and the outputs kept, as
//! on success ([`SessionOutcome::SignedUnknown`]).
//!
//! The wallet side (coins, addresses, signing) is the [`MixingWallet`]
//! trait; the engine implements it.

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::time::Duration;

use dashcore::blockdata::transaction::outpoint::OutPoint;
use dashcore::{Network, ScriptBuf, Transaction, TxIn, TxOut};
use dw_p2p::{Delivery, PeerEntry, Session, SessionConfig};
use rand::Rng;
use tokio::sync::{mpsc, watch};

use crate::denoms::{DENOMINATIONS, ENTRY_MAX_INPUTS, QUEUE_TIMEOUT_SECS, SIGNING_TIMEOUT_SECS};
use crate::messages::{
    Accept, COINJOIN_REBALANCE_VERSION, Complete, Entry, FinalTx, GETDATA, INV, Queue,
    StatusUpdate, StatusUpdateKind, decode_dsq_inv, denomination_to_amount, encode_dsq_inventory,
    encode_signed_inputs,
};
use crate::rebalance::{PROMOTION_RATIO, Rebalance, RebalanceKind};
use crate::rounds::RANDOM_ROUNDS;
use crate::status::{PoolMessage, PoolState, StatusCode};

/// A session's message stream holds at most this many frames, and bytes.
const SUBSCRIPTION_FRAMES: usize = 64;
const SUBSCRIPTION_BYTES: usize = 4 << 20;

/// Extra seconds the client waits beyond the masternode's timeout
/// (`nLagTime`, client.cpp:317).
pub const LAG_SECS: u64 = 10;
/// `GetMaxPoolAmount` (coinjoin.h): the most an entry can carry.
pub const MAX_POOL_AMOUNT: u64 = ENTRY_MAX_INPUTS as u64 * DENOMINATIONS[0];
/// `SIGHASH_ALL | SIGHASH_ANYONECANPAY` (client.cpp:594).
pub const MIXING_SIGHASH: u32 = 0x81;

/// A denominated coin the session may put in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixCoin {
    pub outpoint: OutPoint,
    pub value: u64,
    /// The script the coin pays (the input's `prevPubKey`).
    pub script_pubkey: ScriptBuf,
    /// Rounds by the input-chain walk.
    pub rounds: i32,
}

/// Why the wallet could not do what the session asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WalletError(pub String);

/// What a session needs from the wallet.
pub trait MixingWallet: Send + Sync {
    /// Coins of exactly `amount` that are ready to mix: denominated, not
    /// fully mixed, confirmed or InstantSend-locked, neither locked nor
    /// reserved.
    fn ready_to_mix(
        &self,
        amount: u64,
    ) -> impl Future<Output = Result<Vec<MixCoin>, WalletError>> + Send;

    /// `count` fresh CoinJoin-account receive scripts, reserved until
    /// [`Self::release_scripts`] or [`Self::keep_scripts`].
    fn reserve_scripts(
        &self,
        count: usize,
    ) -> impl Future<Output = Result<Vec<ScriptBuf>, WalletError>> + Send;

    /// Returns reserved scripts to the pool (session failed).
    fn release_scripts(&self, scripts: &[ScriptBuf]) -> impl Future<Output = ()> + Send;

    /// Keeps reserved scripts (session succeeded, or ended after our
    /// signatures went out; they may receive coins).
    fn keep_scripts(&self, scripts: &[ScriptBuf]) -> impl Future<Output = ()> + Send;

    /// Locks coins against every other use.
    fn lock_coins(&self, outpoints: &[OutPoint]);

    /// Releases coins locked by [`Self::lock_coins`].
    fn unlock_coins(&self, outpoints: &[OutPoint]);

    /// Keeps coins locked until the wallet sees them spent (the final
    /// transaction may arrive well after `dsc`, or, with no verdict, after
    /// a stop with our signatures out).
    fn hold_until_spent(&self, outpoints: &[OutPoint]);

    /// Records the possible collateral spend before the coordinator receives its bytes.
    fn handoff_collateral(
        &self,
        collateral: &Transaction,
    ) -> impl Future<Output = Result<(), WalletError>> + Send;

    /// Every unspent coin of the wallet, on any account: a final
    /// transaction may spend only those of them our entry offered.
    fn unspent_outpoints(
        &self,
    ) -> impl Future<Output = Result<HashSet<OutPoint>, WalletError>> + Send;

    /// Whether any of `outpoints` was locked by the user, or left the
    /// wallet, since the session took it: such a coin is not submitted or
    /// signed.
    fn any_withdrawn(
        &self,
        outpoints: &[OutPoint],
    ) -> impl Future<Output = Result<bool, WalletError>> + Send;

    /// Signs the inputs at the given indices of `tx` (with each one's
    /// previous output script) with [`MIXING_SIGHASH`] and returns them
    /// with their `scriptSig` filled.
    fn sign_inputs(
        &self,
        tx: &Transaction,
        inputs: &[(usize, ScriptBuf)],
    ) -> impl Future<Output = Result<Vec<TxIn>, WalletError>> + Send;
}

/// Where a session is, for the status the host shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProgress {
    pub state: PoolState,
    pub session_id: i32,
    pub denom: u32,
    /// Inputs in our entry (0 before `dsi`).
    pub entries: u32,
    pub last_message: Option<PoolMessage>,
    pub status: StatusCode,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// `dsc` with `MSG_SUCCESS`: our inputs are in the mixing transaction.
    Success {
        inputs: Vec<OutPoint>,
    },
    /// Our signatures went out (`dss`, or its send failed part way) and
    /// the session ended without the masternode's verdict (stopped, timed
    /// out or disconnected): the masternode may still broadcast the mixing
    /// transaction with our inputs, so they stay held, and our output
    /// scripts kept, as a success's.
    SignedUnknown {
        inputs: Vec<OutPoint>,
        failure: SessionFailure,
    },
    Failed(SessionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionFailure {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("rejected by the masternode: {0:?}")]
    Rejected(PoolMessage),
    #[error("timed out in state {0:?}")]
    Timeout(PoolState),
    #[error("the connection closed")]
    Disconnected,
    #[error("no compatible inputs")]
    NoInputs,
    #[error("the masternode speaks protocol {0}, below the rebalance protocol")]
    MasternodeTooOld(u32),
    #[error("cannot make the rebalance entry: {0}")]
    BadRebalance(&'static str),
    #[error("refusing to sign: {0}")]
    BadFinalTx(&'static str),
    #[error("wallet: {0}")]
    Wallet(String),
    #[error("the session completed with {0:?}")]
    Completed(PoolMessage),
    #[error("stopped")]
    Cancelled,
}

/// One session's inputs.
#[derive(Debug, Clone)]
pub struct SessionParams {
    pub network: Network,
    pub masternode: PeerEntry,
    pub denom: u32,
    pub collateral: Transaction,
    /// Options: mixing rounds (target).
    pub rounds: u32,
    pub session_config: SessionConfig,
    /// Seconds to wait in the queue and entry phases (30 + 10 in Core).
    pub queue_timeout: Duration,
    /// Seconds to wait while signing (15 + 10 in Core).
    pub signing_timeout: Duration,
    /// A promotion or demotion instead of 1:1 mixing.
    pub rebalance: Option<RebalanceEntry>,
}

/// A promotion's or demotion's inputs, chosen and locked before the session
/// starts (`SelectRebalanceInputs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebalanceEntry {
    pub kind: RebalanceKind,
    pub inputs: Vec<MixCoin>,
}

impl RebalanceEntry {
    /// The amount of each output for a session at `denom`, once the inputs
    /// are the entry's shape (`PreparePromotionEntry`/`PrepareDemotionEntry`).
    pub fn output_amount(&self, denom: u32) -> Result<u64, &'static str> {
        let r = Rebalance {
            denom,
            kind: self.kind,
        };
        let (Some(input), Some(output)) = (r.input_amount(), r.output_amount()) else {
            return Err("no adjacent denomination");
        };
        if self.inputs.len() != self.kind.input_count() {
            return Err("wrong input count");
        }
        if self.inputs.iter().any(|c| c.value != input) {
            return Err("an input at the wrong denomination");
        }
        let distinct: HashSet<OutPoint> = self.inputs.iter().map(|c| c.outpoint).collect();
        if distinct.len() != self.inputs.len() {
            return Err("an input twice");
        }
        let scripts: HashSet<&ScriptBuf> = self.inputs.iter().map(|c| &c.script_pubkey).collect();
        if scripts.len() != self.inputs.len() {
            return Err("two inputs pay one script");
        }
        Ok(output)
    }
}

impl SessionParams {
    pub fn new(
        network: Network,
        masternode: PeerEntry,
        denom: u32,
        collateral: Transaction,
        rounds: u32,
    ) -> Self {
        let config = SessionConfig {
            want_dsq: true,
            ..SessionConfig::default()
        };
        Self {
            network,
            masternode,
            denom,
            collateral,
            rounds,
            session_config: config,
            queue_timeout: Duration::from_secs(QUEUE_TIMEOUT_SECS + LAG_SECS),
            signing_timeout: Duration::from_secs(SIGNING_TIMEOUT_SECS + LAG_SECS),
            rebalance: None,
        }
    }

    /// A promotion or demotion session: it announces the rebalance protocol
    /// so the `dsa` carries its flag.
    pub fn rebalancing(
        network: Network,
        masternode: PeerEntry,
        denom: u32,
        collateral: Transaction,
        rounds: u32,
        entry: RebalanceEntry,
    ) -> Self {
        let mut p = Self::new(network, masternode, denom, collateral, rounds);
        p.session_config.protocol_version = COINJOIN_REBALANCE_VERSION;
        // From this version Dash Core announces the ready `dsq` by `inv`,
        // and announces no `dsq` to a peer that asked for no transaction
        // relay (net_processing.cpp, `MSG_DSQ`).
        p.session_config.relay = true;
        p.rebalance = Some(entry);
        p
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `SelectDenominatedAmounts` (wallet/coinjoin.cpp:99-122): the amounts of
/// ready-to-mix coins, larger denominations first, while the running total
/// stays within `value_max`. `None` below the smallest denomination.
pub fn select_denominated_amounts(values: &[u64], value_max: u64) -> Option<BTreeSet<u64>> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut total = 0u64;
    let mut out = BTreeSet::new();
    for v in sorted {
        if total + v <= value_max {
            total += v;
            out.insert(v);
        }
    }
    (total >= DENOMINATIONS[4]).then_some(out)
}

/// `StartNewQueue`'s pick (client.cpp:1531-1538): from the largest amount
/// down, each skipped with probability ½ while more than one is offered.
pub fn choose_session_denom(amounts: &BTreeSet<u64>, rng: &mut impl Rng) -> Option<u32> {
    if amounts.is_empty() {
        return None;
    }
    loop {
        for a in amounts.iter().rev() {
            if amounts.len() > 1 && rng.gen_bool(0.5) {
                continue;
            }
            return Some(crate::messages::amount_to_denomination(*a)).filter(|d| *d != 0);
        }
    }
}

/// `SelectTxDSInsByDenomination` (wallet/coinjoin.cpp:49-97): coins of the
/// denomination, at most one per transaction, total within `value_max`.
/// `coins` arrive shuffled.
pub fn select_by_denomination(coins: &[MixCoin], amount: u64, value_max: u64) -> Vec<MixCoin> {
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut out = Vec::new();
    for c in coins {
        if c.value != amount || total + c.value > value_max || seen.contains(&c.outpoint.txid) {
            continue;
        }
        total += c.value;
        seen.insert(c.outpoint.txid);
        out.push(c.clone());
    }
    out
}

/// `PrepareDenominate(nMinRounds, nMaxRounds, …, fDryRun)` without the
/// output keys: which coins go into the entry. Outside a dry run, an input
/// after the first is skipped with probability 1/5 but still counts as a
/// step (client.cpp:1855-1876).
pub fn prepare_denominate(
    coins: &[MixCoin],
    min_rounds: i32,
    max_rounds: i32,
    dry_run: bool,
    rng: &mut impl Rng,
) -> Vec<MixCoin> {
    let mut steps = 0;
    let mut out = Vec::new();
    for c in coins {
        if steps >= ENTRY_MAX_INPUTS {
            break;
        }
        if c.rounds < min_rounds || c.rounds > max_rounds {
            continue;
        }
        if !dry_run && steps >= 1 && rng.gen_range(0..5) == 0 {
            steps += 1;
            continue;
        }
        out.push(c.clone());
        steps += 1;
    }
    out
}

/// `SubmitDenominate`'s choice of rounds (client.cpp:1775-1806): the rounds
/// value with the most inputs, fewer rounds on a tie; then everything below
/// the target as a last resort.
pub fn choose_entry_inputs(coins: &[MixCoin], rounds: u32, rng: &mut impl Rng) -> Vec<MixCoin> {
    let mut by_rounds: Vec<(i32, usize)> = (0..(rounds + RANDOM_ROUNDS) as i32)
        .map(|i| (i, prepare_denominate(coins, i, i, true, rng).len()))
        .filter(|(_, n)| *n > 0)
        .collect();
    by_rounds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if let Some((r, _)) = by_rounds.first() {
        let picked = prepare_denominate(coins, *r, *r, false, rng);
        if !picked.is_empty() {
            return picked;
        }
    }
    prepare_denominate(coins, 0, rounds as i32 - 1, false, rng)
}

/// BIP69 order (`CompareInputBIP69`/`CompareOutputBIP69`): inputs by txid
/// in display order then vout; outputs by value then script bytes.
pub fn is_bip69_sorted(tx: &Transaction) -> bool {
    let key_in = |i: &TxIn| {
        let mut t = i.previous_output.txid.to_byte_array();
        t.reverse();
        (t, i.previous_output.vout)
    };
    let ins = tx.input.windows(2).all(|w| key_in(&w[0]) <= key_in(&w[1]));
    let outs = tx.output.windows(2).all(|w| {
        (w[0].value, w[0].script_pubkey.as_bytes()) <= (w[1].value, w[1].script_pubkey.as_bytes())
    });
    ins && outs
}

/// How many inputs of a final transaction sit at the session denomination
/// and how many at the larger one, from what an SPV wallet can see: the
/// input count and the outputs' amounts. A transaction Core's server
/// finalizes pays no fee and spends coins of those two amounts only, the
/// larger exactly [`PROMOTION_RATIO`] times the smaller (`IsValidInOuts`),
/// so `session_in + ratio * larger_in = session_out + ratio * larger_out`
/// fixes both counts. `None` when no such split exists, or the split is not
/// composable from standard, promotion and demotion entries
/// (`ValidateFinalTxComposition`).
pub fn final_tx_composition(
    inputs: usize,
    session_outputs: usize,
    larger_outputs: usize,
    has_larger: bool,
) -> Option<(usize, usize)> {
    let ratio = PROMOTION_RATIO;
    let units = session_outputs + ratio * larger_outputs;
    let surplus = units.checked_sub(inputs)?;
    if surplus % (ratio - 1) != 0 {
        return None;
    }
    let larger_inputs = surplus / (ratio - 1);
    let session_inputs = inputs.checked_sub(larger_inputs)?;
    if !has_larger && larger_inputs + larger_outputs > 0 {
        return None;
    }
    let composable =
        session_inputs >= larger_outputs * ratio && session_outputs >= larger_inputs * ratio;
    composable.then_some((session_inputs, larger_inputs))
}

/// The SPV form of `SignFinalTransaction`'s checks (client.cpp:464-631).
/// Core also checks every foreign input against its UTXO set and
/// mempool (`IsValidInOuts`); an SPV wallet cannot, so foreign inputs are
/// counted and their amounts inferred ([`final_tx_composition`]). Checked:
/// BIP69 order; every output P2PKH at the session denomination or the next
/// larger one (post-V24 rebalance entries), no script twice; no input twice;
/// each of our outputs (exact amount and script) and inputs present; no
/// other coin of `wallet_unspent` spent; the inputs and outputs balance; and
/// somebody else holds coins at the session denomination on each side we
/// occupy there.
pub fn check_final_tx(
    tx: &Transaction,
    denom: u32,
    our_inputs: &[MixCoin],
    our_outputs: &[TxOut],
    wallet_unspent: &HashSet<OutPoint>,
) -> Result<Vec<usize>, &'static str> {
    let session_amount = denomination_to_amount(denom).ok_or("bad session denomination")?;
    let larger =
        (denom.trailing_zeros() > 0).then(|| DENOMINATIONS[denom.trailing_zeros() as usize - 1]);
    if !is_bip69_sorted(tx) {
        return Err("not BIP69 sorted");
    }
    let mut scripts = HashSet::new();
    for o in &tx.output {
        if o.value != session_amount && Some(o.value) != larger {
            return Err("output at a foreign denomination");
        }
        if !o.script_pubkey.is_p2pkh() {
            return Err("output is not P2PKH");
        }
        if !scripts.insert(o.script_pubkey.clone()) {
            return Err("output script used twice");
        }
    }
    if tx.input.iter().any(|i| i.previous_output.is_null()) {
        return Err("null input");
    }
    let spent: HashSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    if spent.len() != tx.input.len() {
        return Err("input spent twice");
    }
    for ours in our_outputs {
        if !tx.output.contains(ours) {
            return Err("an output of ours is missing");
        }
    }
    let mut indices = Vec::with_capacity(our_inputs.len());
    for c in our_inputs {
        let Some(i) = tx
            .input
            .iter()
            .position(|i| i.previous_output == c.outpoint)
        else {
            return Err("an input of ours is missing");
        };
        // The input as we submitted it (Core compares the whole `CTxIn`).
        if tx.input[i] != unsigned_input(c.outpoint) {
            return Err("an input of ours was changed");
        }
        if !wallet_unspent.contains(&c.outpoint) {
            return Err("an input of ours is no longer unspent");
        }
        indices.push(i);
    }
    let offered: HashSet<OutPoint> = our_inputs.iter().map(|c| c.outpoint).collect();
    if spent
        .iter()
        .any(|o| wallet_unspent.contains(o) && !offered.contains(o))
    {
        return Err("spends a coin of ours we did not offer");
    }
    let session_outputs = tx
        .output
        .iter()
        .filter(|o| o.value == session_amount)
        .count();
    let larger_outputs = tx.output.len() - session_outputs;
    let (session_inputs, larger_inputs) = final_tx_composition(
        tx.input.len(),
        session_outputs,
        larger_outputs,
        larger.is_some(),
    )
    .ok_or("inputs and outputs do not balance")?;
    let own_session_inputs = our_inputs
        .iter()
        .filter(|c| c.value == session_amount)
        .count();
    if own_session_inputs
        + our_inputs
            .iter()
            .filter(|c| Some(c.value) == larger)
            .count()
        != our_inputs.len()
    {
        return Err("an input of ours at a foreign denomination");
    }
    if own_session_inputs > session_inputs || our_inputs.len() - own_session_inputs > larger_inputs
    {
        return Err("inputs and outputs do not balance");
    }
    let own_session_outputs = our_outputs
        .iter()
        .filter(|o| o.value == session_amount)
        .count();
    if own_session_outputs > 0 && session_outputs <= own_session_outputs {
        return Err("no other participant's output at the session denomination");
    }
    if own_session_inputs > 0 && session_inputs <= own_session_inputs {
        return Err("no other participant's input at the session denomination");
    }
    if tx.input.len() <= our_inputs.len() {
        return Err("no other participant's input");
    }
    Ok(indices)
}

struct Running<'a, W: MixingWallet> {
    wallet: &'a W,
    /// Our entry's inputs.
    inputs: Vec<MixCoin>,
    scripts: Vec<ScriptBuf>,
}

impl<W: MixingWallet> Running<'_, W> {
    fn outpoints(&self) -> Vec<OutPoint> {
        self.inputs.iter().map(|c| c.outpoint).collect()
    }

    async fn fail(&mut self) {
        self.wallet.unlock_coins(&self.outpoints());
        self.inputs.clear();
        self.wallet.release_scripts(&self.scripts).await;
        self.scripts.clear();
    }
}

fn unsigned_input(outpoint: OutPoint) -> TxIn {
    TxIn {
        previous_output: outpoint,
        script_sig: ScriptBuf::new(),
        sequence: 0xffff_ffff,
        witness: Default::default(),
    }
}

/// Connects and sends `dsa`. A rebalance `dsa` goes only to a masternode
/// that speaks the rebalance protocol (`ProcessPendingDsaRequest`): an older
/// one would read it as 1:1 mixing and charge the collateral for the
/// rebalance entry.
async fn open_session<W: MixingWallet>(
    wallet: &W,
    params: &SessionParams,
    flags: u8,
    stop: &mut watch::Receiver<bool>,
) -> Result<(Session, mpsc::Receiver<Delivery>), SessionFailure> {
    let Some(service) = params.masternode.service else {
        return Err(SessionFailure::Connect("no service address".into()));
    };
    let session = tokio::select! {
        s = Session::connect(service, params.network, params.session_config.clone()) => {
            s.map_err(|e| SessionFailure::Connect(e.to_string()))?
        }
        _ = stop.wait_for(|s| *s) => return Err(SessionFailure::Cancelled),
    };
    if flags != 0 && session.common_version() < COINJOIN_REBALANCE_VERSION {
        let version = session.common_version();
        session.close();
        return Err(SessionFailure::MasternodeTooOld(version));
    }
    // Before the `dsa`: the stream holds only what arrives after it, and
    // the acceptance can be quick.
    // Bounded: with relay on, the masternode announces transactions too,
    // and a stream that only grows would hold them all.
    let rx = session.subscribe_bounded(
        &[
            dw_p2p::commands::DSSTATUSUPDATE,
            dw_p2p::commands::DSQUEUE,
            dw_p2p::commands::DSFINALTX,
            dw_p2p::commands::DSCOMPLETE,
            INV,
        ],
        SUBSCRIPTION_FRAMES,
        SUBSCRIPTION_BYTES,
    );
    let accept = Accept {
        denom: params.denom,
        collateral: params.collateral.clone(),
        flags,
    };
    if let Err(error) = wallet.handoff_collateral(&params.collateral).await {
        session.close();
        return Err(SessionFailure::Wallet(error.0));
    }
    if session
        .send(
            dw_p2p::commands::DSACCEPT,
            accept.encode(session.common_version()),
        )
        .await
        .is_err()
    {
        session.close();
        return Err(SessionFailure::Disconnected);
    }
    Ok((session, rx))
}

/// Runs one session to its end. `observe` gets every state change;
/// `stop` set to `true` abandons the session (`ResetPool`). A rebalance
/// session's inputs are locked from its start and, like a 1:1 session's,
/// released on failure.
pub async fn run_session<W: MixingWallet>(
    wallet: &W,
    params: SessionParams,
    observe: impl Fn(&SessionProgress) + Send + Sync,
    mut stop: watch::Receiver<bool>,
) -> SessionOutcome {
    let mut progress = SessionProgress {
        state: PoolState::Queue,
        session_id: 0,
        denom: params.denom,
        entries: 0,
        last_message: None,
        status: StatusCode::TryingToConnect,
    };
    observe(&progress);
    let mut run = Running {
        wallet,
        inputs: Vec::new(),
        scripts: Vec::new(),
    };
    let mut flags = 0;
    let mut rebalance_output = Ok(0);
    if let Some(entry) = &params.rebalance {
        run.inputs = entry.inputs.clone();
        wallet.lock_coins(&run.outpoints());
        flags = entry.kind.dsa_flag();
        rebalance_output = entry.output_amount(params.denom);
    }
    let opened = match rebalance_output {
        Err(why) => Err(SessionFailure::BadRebalance(why)),
        Ok(_) => open_session(wallet, &params, flags, &mut stop).await,
    };
    let rebalance_output = rebalance_output.unwrap_or(0);
    let (session, mut rx) = match opened {
        Ok(s) => s,
        Err(failure) => {
            run.fail().await;
            progress.state = PoolState::Error;
            observe(&progress);
            return SessionOutcome::Failed(failure);
        }
    };
    let mut our_outputs: Vec<TxOut> = Vec::new();
    let mut deadline = tokio::time::Instant::now() + params.queue_timeout;
    let mut rng = rand::rngs::OsRng;

    // From the `dss` send on, the masternode may hold our signatures.
    let mut signatures_out = false;
    let outcome = loop {
        let msg = tokio::select! {
            m = rx.recv() => m,
            _ = tokio::time::sleep_until(deadline) => {
                break SessionOutcome::Failed(SessionFailure::Timeout(progress.state));
            }
            _ = stop.wait_for(|s| *s) => break SessionOutcome::Failed(SessionFailure::Cancelled),
        };
        let Some(Delivery { message: msg, .. }) = msg else {
            break SessionOutcome::Failed(SessionFailure::Disconnected);
        };
        match msg.command.as_str() {
            INV => {
                if progress.state != PoolState::Queue || progress.session_id == 0 {
                    continue;
                }
                let Ok(hashes) = decode_dsq_inv(&msg.payload) else {
                    continue;
                };
                // Never waits for room: a stop or the deadline must still
                // be heard. A full send queue closes the session.
                if !hashes.is_empty()
                    && session
                        .try_send(GETDATA, encode_dsq_inventory(&hashes))
                        .is_err()
                {
                    break SessionOutcome::Failed(SessionFailure::Disconnected);
                }
            }
            dw_p2p::commands::DSSTATUSUPDATE => {
                let Ok(up) = StatusUpdate::decode(&msg.payload) else {
                    continue;
                };
                progress.last_message = Some(up.message);
                progress.status = StatusCode::Masternode(up.message);
                match up.kind {
                    StatusUpdateKind::Rejected => {
                        observe(&progress);
                        break SessionOutcome::Failed(SessionFailure::Rejected(up.message));
                    }
                    StatusUpdateKind::Accepted => {
                        if progress.state == PoolState::Queue
                            && up.state == PoolState::Queue
                            && progress.session_id == 0
                            && up.session_id != 0
                        {
                            progress.session_id = up.session_id;
                            deadline = tokio::time::Instant::now() + params.queue_timeout;
                        }
                        observe(&progress);
                    }
                }
            }
            dw_p2p::commands::DSQUEUE => {
                let Ok(dsq) = Queue::decode(&msg.payload) else {
                    continue;
                };
                let ours = dsq.ready
                    && dsq.pro_tx_hash == params.masternode.pro_tx_hash
                    && dsq.denom == params.denom
                    && progress.state == PoolState::Queue
                    && progress.session_id != 0
                    && !dsq.is_time_out_of_bounds(unix_now())
                    && dsq.verify(&params.masternode.operator_public_key);
                if !ours {
                    continue;
                }
                // SubmitDenominate.
                let Some(amount) = denomination_to_amount(params.denom) else {
                    break SessionOutcome::Failed(SessionFailure::NoInputs);
                };
                let (output_amount, output_count) = match &params.rebalance {
                    Some(entry) => {
                        let unspent = match wallet.unspent_outpoints().await {
                            Ok(u) => u,
                            Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                        };
                        if run.inputs.iter().any(|c| !unspent.contains(&c.outpoint)) {
                            break SessionOutcome::Failed(SessionFailure::BadRebalance(
                                "an input is no longer unspent",
                            ));
                        }
                        match wallet.any_withdrawn(&run.outpoints()).await {
                            Ok(false) => {}
                            Ok(true) => {
                                break SessionOutcome::Failed(SessionFailure::BadRebalance(
                                    "an input was locked or left the wallet",
                                ));
                            }
                            Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                        }
                        (rebalance_output, entry.kind.output_count())
                    }
                    None => {
                        let coins = match wallet.ready_to_mix(amount).await {
                            Ok(c) => c,
                            Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                        };
                        let candidates = select_by_denomination(&coins, amount, MAX_POOL_AMOUNT);
                        let inputs = choose_entry_inputs(&candidates, params.rounds, &mut rng);
                        if inputs.is_empty() {
                            break SessionOutcome::Failed(SessionFailure::NoInputs);
                        }
                        run.inputs = inputs;
                        wallet.lock_coins(&run.outpoints());
                        (amount, run.inputs.len())
                    }
                };
                let scripts = match wallet.reserve_scripts(output_count).await {
                    Ok(s) if s.len() == output_count => s,
                    Ok(s) => {
                        run.scripts = s;
                        break SessionOutcome::Failed(SessionFailure::Wallet(
                            "too few output scripts".into(),
                        ));
                    }
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
                run.scripts = scripts.clone();
                our_outputs = scripts
                    .into_iter()
                    .map(|script_pubkey| TxOut {
                        value: output_amount,
                        script_pubkey,
                    })
                    .collect();
                let entry = Entry {
                    inputs: run.outpoints().into_iter().map(unsigned_input).collect(),
                    collateral: params.collateral.clone(),
                    outputs: our_outputs.clone(),
                };
                if session
                    .send(dw_p2p::commands::DSVIN, entry.encode())
                    .await
                    .is_err()
                {
                    break SessionOutcome::Failed(SessionFailure::Disconnected);
                }
                progress.state = PoolState::AcceptingEntries;
                progress.entries = run.inputs.len() as u32;
                progress.status = StatusCode::MixingInProgress;
                deadline = tokio::time::Instant::now() + params.queue_timeout;
                observe(&progress);
            }
            dw_p2p::commands::DSFINALTX => {
                let Ok(f) = FinalTx::decode(&msg.payload) else {
                    continue;
                };
                if f.session_id != progress.session_id
                    || progress.state != PoolState::AcceptingEntries
                {
                    continue;
                }
                let unspent = match wallet.unspent_outpoints().await {
                    Ok(u) => u,
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
                let indices = match check_final_tx(
                    &f.tx,
                    params.denom,
                    &run.inputs,
                    &our_outputs,
                    &unspent,
                ) {
                    Ok(i) => i,
                    Err(why) => break SessionOutcome::Failed(SessionFailure::BadFinalTx(why)),
                };
                match wallet.any_withdrawn(&run.outpoints()).await {
                    Ok(false) => {}
                    Ok(true) => {
                        break SessionOutcome::Failed(SessionFailure::BadFinalTx(
                            "an input of ours was locked or left the wallet",
                        ));
                    }
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                }
                let to_sign: Vec<(usize, ScriptBuf)> = indices
                    .iter()
                    .zip(&run.inputs)
                    .map(|(idx, c)| (*idx, c.script_pubkey.clone()))
                    .collect();
                let signed = match wallet.sign_inputs(&f.tx, &to_sign).await {
                    Ok(s) => s,
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
                signatures_out = true;
                if session
                    .send(
                        dw_p2p::commands::DSSIGNFINALTX,
                        encode_signed_inputs(&signed),
                    )
                    .await
                    .is_err()
                {
                    break SessionOutcome::Failed(SessionFailure::Disconnected);
                }
                progress.state = PoolState::Signing;
                progress.status = StatusCode::Signing;
                deadline = tokio::time::Instant::now() + params.signing_timeout;
                observe(&progress);
            }
            dw_p2p::commands::DSCOMPLETE => {
                let Ok(c) = Complete::decode(&msg.payload) else {
                    continue;
                };
                if c.session_id != progress.session_id || progress.session_id == 0 {
                    continue;
                }
                progress.last_message = Some(c.message);
                observe(&progress);
                if c.message == PoolMessage::Success && progress.state == PoolState::Signing {
                    break SessionOutcome::Success {
                        inputs: run.outpoints(),
                    };
                }
                break SessionOutcome::Failed(SessionFailure::Completed(c.message));
            }
            _ => {}
        }
    };

    // Once our signatures are out the masternode may still broadcast them,
    // whatever it said: the inputs and scripts stay held.
    let outcome = match outcome {
        SessionOutcome::Failed(failure) if signatures_out => SessionOutcome::SignedUnknown {
            inputs: run.outpoints(),
            failure,
        },
        outcome => outcome,
    };
    match &outcome {
        SessionOutcome::Success { inputs } => {
            wallet.hold_until_spent(inputs);
            wallet.keep_scripts(&run.scripts).await;
            progress.state = PoolState::Idle;
        }
        SessionOutcome::SignedUnknown { inputs, .. } => {
            wallet.hold_until_spent(inputs);
            wallet.keep_scripts(&run.scripts).await;
            progress.state = PoolState::Error;
        }
        SessionOutcome::Failed(_) => {
            run.fail().await;
            progress.state = PoolState::Error;
        }
    }
    observe(&progress);
    session.close();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `check_final_tx` with our own inputs still unspent, as at signing.
    fn check_ours_unspent(
        tx: &Transaction,
        denom: u32,
        ours_in: &[MixCoin],
        ours_out: &[TxOut],
        others: &HashSet<OutPoint>,
    ) -> Result<Vec<usize>, &'static str> {
        let mut unspent = others.clone();
        unspent.extend(ours_in.iter().map(|c| c.outpoint));
        check_final_tx(tx, denom, ours_in, ours_out, &unspent)
    }
    use dashcore::Txid;

    const S: u64 = DENOMINATIONS[2];
    const L: u64 = DENOMINATIONS[1];

    #[test]
    fn honest_compositions_are_recognised() {
        let r = PROMOTION_RATIO;
        // Core's `ValidateFinalTxComposition` cases, as (inputs, session
        // outputs, larger outputs) with the larger inputs they imply.
        assert_eq!(final_tx_composition(5, 5, 0, true), Some((5, 0)));
        assert_eq!(final_tx_composition(3 + r, 3, 1, true), Some((3 + r, 0)));
        assert_eq!(final_tx_composition(3 + 1, 3 + r, 0, true), Some((3, 1)));
        assert_eq!(
            final_tx_composition(3 + r + 1, 3 + r, 1, true),
            Some((3 + r, 1))
        );
        assert_eq!(final_tx_composition(2 * r, 0, 2, true), Some((2 * r, 0)));
        assert_eq!(final_tx_composition(2, 2 * r, 0, true), Some((0, 2)));
    }

    #[test]
    fn compositions_that_cannot_balance_are_refused() {
        let r = PROMOTION_RATIO;
        // More inputs than the outputs' value can account for.
        assert_eq!(final_tx_composition(6, 5, 0, true), None);
        // A surplus that is not a whole number of larger inputs.
        assert_eq!(final_tx_composition(5, 6, 0, true), None);
        // A promotion short of session-denomination inputs.
        assert_eq!(final_tx_composition(r - 1, 0, 1, true), None);
        // A demotion short of session-denomination outputs.
        assert_eq!(final_tx_composition(1, r - 1, 0, true), None);
        // Two promotions' outputs for one promotion's inputs.
        assert_eq!(final_tx_composition(r, 0, 2, true), None);
        // Larger coins where the session denomination is the largest.
        assert_eq!(final_tx_composition(2, 2 * r, 0, false), None);
        assert_eq!(final_tx_composition(3, 3, 0, false), Some((3, 0)));
        // A larger coin passed through to a larger output.
        assert_eq!(final_tx_composition(1, 0, 1, true), None);
        assert_eq!(final_tx_composition(3 + 1, 3, 1, true), None);
    }

    fn txid(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    fn coin(n: u8, value: u64) -> MixCoin {
        MixCoin {
            outpoint: OutPoint::new(txid(n), 0),
            value,
            script_pubkey: ScriptBuf::from_bytes(vec![n]),
            rounds: 4,
        }
    }

    fn out(seed: u8, value: u64) -> TxOut {
        let mut b = vec![0x76, 0xa9, 0x14];
        b.extend_from_slice(&[seed; 20]);
        b.extend_from_slice(&[0x88, 0xac]);
        TxOut {
            value,
            script_pubkey: ScriptBuf::from_bytes(b),
        }
    }

    fn tx(mut inputs: Vec<OutPoint>, mut outputs: Vec<TxOut>) -> Transaction {
        inputs.sort_by_key(|o| {
            let mut t = o.txid.to_byte_array();
            t.reverse();
            (t, o.vout)
        });
        outputs.sort_by(|a, b| {
            (a.value, a.script_pubkey.as_bytes()).cmp(&(b.value, b.script_pubkey.as_bytes()))
        });
        Transaction {
            version: 1,
            lock_time: 0,
            input: inputs.into_iter().map(unsigned_input).collect(),
            output: outputs,
            special_transaction_payload: None,
        }
    }

    /// Our demotion (one 1 DASH coin, ten 0.1 outputs) beside a standard
    /// participant and another demotion.
    fn demotion() -> (Vec<MixCoin>, Vec<TxOut>, Vec<OutPoint>, Vec<TxOut>) {
        let ours_in = vec![coin(1, L)];
        let ours_out: Vec<TxOut> = (0..10).map(|i| out(i, S)).collect();
        let mut inputs = vec![ours_in[0].outpoint, OutPoint::new(txid(0xf0), 0)];
        inputs.push(OutPoint::new(txid(0xf1), 0));
        let mut outputs = ours_out.clone();
        outputs.extend((0..10).map(|i| out(0x40 + i, S)));
        outputs.push(out(0x80, S));
        (ours_in, ours_out, inputs, outputs)
    }

    #[test]
    fn a_demotion_beside_others_is_signed() {
        let (ours_in, ours_out, inputs, outputs) = demotion();
        // Inputs: ours (L), a demotion (L), a standard one (S): 3 inputs,
        // 21 session outputs, so 2 larger and 1 session input.
        let t = tx(inputs, outputs);
        let idx = check_ours_unspent(&t, 4, &ours_in, &ours_out, &HashSet::new()).unwrap();
        assert_eq!(t.input[idx[0]].previous_output, ours_in[0].outpoint);
    }

    /// Core compares the whole submitted input; so do we, and the coin must
    /// still be unspent when we sign.
    #[test]
    fn a_changed_or_spent_input_of_ours_is_not_signed() {
        let (ours_in, ours_out, inputs, outputs) = demotion();
        let mut t = tx(inputs, outputs);
        let at = (t.input.iter())
            .position(|i| i.previous_output == ours_in[0].outpoint)
            .unwrap();
        let mut late = t.clone();
        late.input[at].sequence = 0xffff_fffe;
        assert_eq!(
            check_ours_unspent(&late, 4, &ours_in, &ours_out, &HashSet::new()),
            Err("an input of ours was changed")
        );
        t.input[at].script_sig = ScriptBuf::from_bytes(vec![0x51]);
        assert_eq!(
            check_ours_unspent(&t, 4, &ours_in, &ours_out, &HashSet::new()),
            Err("an input of ours was changed")
        );
        let (ours_in, ours_out, inputs, outputs) = demotion();
        assert_eq!(
            check_final_tx(
                &tx(inputs, outputs),
                4,
                &ours_in,
                &ours_out,
                &HashSet::new()
            ),
            Err("an input of ours is no longer unspent")
        );
    }

    #[test]
    fn a_demotion_whose_outputs_are_the_only_session_outputs_is_refused() {
        let ours_in = vec![coin(1, L)];
        let ours_out: Vec<TxOut> = (0..10).map(|i| out(i, S)).collect();
        // A foreign promotion's ten 0.1 inputs pay for one 1 DASH output:
        // our ten outputs are then the only 0.1 outputs.
        let mut inputs = vec![ours_in[0].outpoint];
        inputs.extend((0..10).map(|i| OutPoint::new(txid(0xf0 + i), 0)));
        let mut outputs = ours_out.clone();
        outputs.push(out(0x80, L));
        assert_eq!(
            check_ours_unspent(
                &tx(inputs, outputs),
                4,
                &ours_in,
                &ours_out,
                &HashSet::new()
            ),
            Err("no other participant's output at the session denomination")
        );
    }

    #[test]
    fn a_coin_of_ours_the_entry_did_not_offer_is_refused() {
        let (ours_in, ours_out, inputs, outputs) = demotion();
        let t = tx(inputs, outputs);
        let mine: HashSet<OutPoint> = [ours_in[0].outpoint].into();
        assert!(check_ours_unspent(&t, 4, &ours_in, &ours_out, &mine).is_ok());
        let mine: HashSet<OutPoint> = [ours_in[0].outpoint, OutPoint::new(txid(0xf1), 0)].into();
        assert_eq!(
            check_ours_unspent(&t, 4, &ours_in, &ours_out, &mine),
            Err("spends a coin of ours we did not offer")
        );
    }

    #[test]
    fn a_final_tx_that_cannot_balance_is_refused() {
        let (ours_in, ours_out, mut inputs, outputs) = demotion();
        inputs.push(OutPoint::new(txid(0xf2), 0));
        assert_eq!(
            check_ours_unspent(
                &tx(inputs, outputs),
                4,
                &ours_in,
                &ours_out,
                &HashSet::new()
            ),
            Err("inputs and outputs do not balance")
        );
    }

    #[test]
    fn an_input_spent_twice_is_refused() {
        let (ours_in, ours_out, mut inputs, outputs) = demotion();
        let dup = inputs[1];
        inputs.push(dup);
        assert_eq!(
            check_ours_unspent(
                &tx(inputs, outputs),
                4,
                &ours_in,
                &ours_out,
                &HashSet::new()
            ),
            Err("input spent twice")
        );
    }

    /// The 1:1 path keeps its checks: our inputs and outputs present, and
    /// somebody else's on each side.
    #[test]
    fn a_standard_entry_keeps_its_checks() {
        let ours_in = vec![coin(1, S), coin(2, S)];
        let ours_out = vec![out(1, S), out(2, S)];
        let mut inputs: Vec<OutPoint> = ours_in.iter().map(|c| c.outpoint).collect();
        inputs.push(OutPoint::new(txid(0xf0), 0));
        let mut outputs = ours_out.clone();
        outputs.push(out(0xf0, S));
        let t = tx(inputs.clone(), outputs.clone());
        assert!(check_ours_unspent(&t, 4, &ours_in, &ours_out, &HashSet::new()).is_ok());
        let alone = tx(inputs[..2].to_vec(), ours_out.clone());
        assert_eq!(
            check_ours_unspent(&alone, 4, &ours_in, &ours_out, &HashSet::new()),
            Err("no other participant's output at the session denomination")
        );
        let mut short = outputs.clone();
        short.remove(0);
        short.push(out(0xf1, S));
        assert_eq!(
            check_ours_unspent(&tx(inputs, short), 4, &ours_in, &ours_out, &HashSet::new()),
            Err("an output of ours is missing")
        );
    }

    /// Two 0.1 inputs and outputs of ours beside one foreign 1:1
    /// participant: (our inputs, our outputs, inputs, outputs).
    fn standard() -> (Vec<MixCoin>, Vec<TxOut>, Vec<OutPoint>, Vec<TxOut>) {
        let ours_in = vec![coin(1, S), coin(2, S)];
        let ours_out = vec![out(1, S), out(2, S)];
        let mut inputs: Vec<OutPoint> = ours_in.iter().map(|c| c.outpoint).collect();
        inputs.push(OutPoint::new(txid(0xf0), 0));
        let mut outputs = ours_out.clone();
        outputs.push(out(0xf0, S));
        (ours_in, ours_out, inputs, outputs)
    }

    fn refusal(t: &Transaction, ours_in: &[MixCoin], ours_out: &[TxOut]) -> &'static str {
        check_ours_unspent(t, 4, ours_in, ours_out, &HashSet::new()).unwrap_err()
    }

    /// Each structural refusal on an otherwise valid final transaction.
    #[test]
    fn each_structural_refusal_names_its_reason() {
        let (ours_in, ours_out, inputs, outputs) = standard();
        let ok = tx(inputs.clone(), outputs.clone());
        assert!(check_ours_unspent(&ok, 4, &ours_in, &ours_out, &HashSet::new()).is_ok());

        let mut unsorted = ok.clone();
        unsorted.output.reverse();
        assert_eq!(refusal(&unsorted, &ours_in, &ours_out), "not BIP69 sorted");

        let mut more_in = inputs.clone();
        more_in.push(OutPoint::new(txid(0xf1), 0));
        let mut foreign_denom = outputs.clone();
        foreign_denom.push(out(0xf1, DENOMINATIONS[3]));
        assert_eq!(
            refusal(&tx(more_in.clone(), foreign_denom), &ours_in, &ours_out),
            "output at a foreign denomination"
        );

        let mut p2sh = outputs.clone();
        let mut b = vec![0xa9, 0x14];
        b.extend_from_slice(&[0xf1; 20]);
        b.push(0x87);
        p2sh.push(TxOut {
            value: S,
            script_pubkey: ScriptBuf::from_bytes(b),
        });
        assert_eq!(
            refusal(&tx(more_in.clone(), p2sh), &ours_in, &ours_out),
            "output is not P2PKH"
        );

        let mut reused = outputs.clone();
        reused.push(out(0xf0, S));
        assert_eq!(
            refusal(&tx(more_in, reused), &ours_in, &ours_out),
            "output script used twice"
        );

        let mut null_in = inputs.clone();
        null_in.push(OutPoint::null());
        let mut one_more = outputs.clone();
        one_more.push(out(0xf1, S));
        assert_eq!(
            refusal(&tx(null_in, one_more), &ours_in, &ours_out),
            "null input"
        );

        let without_ours = vec![
            ours_in[0].outpoint,
            OutPoint::new(txid(0xf0), 0),
            OutPoint::new(txid(0xf1), 0),
        ];
        assert_eq!(
            refusal(&tx(without_ours, outputs), &ours_in, &ours_out),
            "an input of ours is missing"
        );
    }

    /// More inputs than the outputs can pay for: one of them would fund the
    /// rest as fee.
    #[test]
    fn a_standard_entry_beside_an_unbalanced_input_is_refused() {
        let (ours_in, ours_out, mut inputs, outputs) = standard();
        inputs.push(OutPoint::new(txid(0xf1), 0));
        assert_eq!(
            refusal(&tx(inputs, outputs), &ours_in, &ours_out),
            "inputs and outputs do not balance"
        );
    }

    /// The outputs balance only if our 0.1 inputs were worth more.
    #[test]
    fn our_session_inputs_read_as_larger_coins_are_refused() {
        let (ours_in, ours_out, inputs, mut outputs) = standard();
        // Three inputs for 21 0.1 outputs: one 0.1 input and two 1 DASH.
        outputs.extend((0..18).map(|i| out(0x40 + i, S)));
        assert_eq!(
            refusal(&tx(inputs, outputs), &ours_in, &ours_out),
            "inputs and outputs do not balance"
        );
    }

    /// The outputs balance only if our 1 DASH input were a 0.1 coin: the
    /// difference would go to fee.
    #[test]
    fn our_demotion_input_read_as_a_session_coin_is_refused() {
        let ours_in = vec![coin(1, L)];
        let ours_out: Vec<TxOut> = (0..10).map(|i| out(i, S)).collect();
        let mut inputs = vec![ours_in[0].outpoint];
        inputs.extend((0..11).map(|i| OutPoint::new(txid(0xe0 + i), 0)));
        let mut outputs = ours_out.clone();
        outputs.extend([out(0xf0, S), out(0xf1, S)]);
        assert_eq!(
            refusal(&tx(inputs, outputs), &ours_in, &ours_out),
            "inputs and outputs do not balance"
        );
    }

    #[test]
    fn an_input_of_ours_at_a_foreign_denomination_is_refused() {
        let (mut ours_in, ours_out, inputs, outputs) = standard();
        ours_in[1].value = DENOMINATIONS[3];
        assert_eq!(
            refusal(&tx(inputs, outputs), &ours_in, &ours_out),
            "an input of ours at a foreign denomination"
        );
    }

    #[test]
    fn a_rebalance_entry_has_its_kind_shape() {
        let demote = |inputs: Vec<MixCoin>| RebalanceEntry {
            kind: RebalanceKind::Demotion,
            inputs,
        };
        let promote = |inputs: Vec<MixCoin>| RebalanceEntry {
            kind: RebalanceKind::Promotion,
            inputs,
        };
        assert_eq!(demote(vec![coin(1, L)]).output_amount(4), Ok(S));
        assert_eq!(
            demote(vec![coin(1, DENOMINATIONS[0])]).output_amount(1),
            Err("no adjacent denomination")
        );
        assert_eq!(
            demote(vec![coin(1, S)]).output_amount(4),
            Err("an input at the wrong denomination")
        );
        let ten: Vec<MixCoin> = (1..=10).map(|n| coin(n, S)).collect();
        assert_eq!(promote(ten.clone()).output_amount(4), Ok(L));
        assert_eq!(
            promote(ten[..9].to_vec()).output_amount(4),
            Err("wrong input count")
        );
        let mut same_script = ten.clone();
        same_script[9].script_pubkey = same_script[0].script_pubkey.clone();
        assert_eq!(
            promote(same_script).output_amount(4),
            Err("two inputs pay one script")
        );
        let mut twice = ten;
        twice[9] = twice[0].clone();
        assert_eq!(promote(twice).output_amount(4), Err("an input twice"));
    }
}
