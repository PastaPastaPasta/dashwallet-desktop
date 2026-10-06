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
//! session: our inputs are unlocked and the reserved outputs released.
//!
//! The wallet side (coins, addresses, signing) is the [`MixingWallet`]
//! trait; the engine implements it.

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::time::Duration;

use dashcore::blockdata::transaction::outpoint::OutPoint;
use dashcore::hashes::Hash;
use dashcore::{Network, ScriptBuf, Transaction, TxIn, TxOut};
use dw_p2p::{PeerEntry, Session, SessionConfig};
use rand::Rng;
use tokio::sync::watch;

use crate::denoms::{DENOMINATIONS, ENTRY_MAX_INPUTS, QUEUE_TIMEOUT_SECS, SIGNING_TIMEOUT_SECS};
use crate::messages::{
    Accept, Complete, Entry, FinalTx, Queue, StatusUpdate, StatusUpdateKind,
    denomination_to_amount, encode_signed_inputs,
};
use crate::rounds::RANDOM_ROUNDS;
use crate::status::{PoolMessage, PoolState, StatusCode};

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

    /// Keeps reserved scripts (session succeeded; they receive coins).
    fn keep_scripts(&self, scripts: &[ScriptBuf]) -> impl Future<Output = ()> + Send;

    /// Locks coins against every other use.
    fn lock_coins(&self, outpoints: &[OutPoint]);

    /// Releases coins locked by [`Self::lock_coins`].
    fn unlock_coins(&self, outpoints: &[OutPoint]);

    /// Keeps coins locked until the wallet sees them spent (the final
    /// transaction may arrive well after `dsc`).
    fn hold_until_spent(&self, outpoints: &[OutPoint]);

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
        }
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

/// The SPV form of `SignFinalTransaction`'s checks (client.cpp:483-589).
/// Core also checks every foreign input against its UTXO set and
/// mempool (`IsValidInOuts`); an SPV wallet cannot, so foreign inputs are
/// only counted. Checked: BIP69 order; every output P2PKH at the session
/// denomination or the next larger one (post-V24 rebalance entries), no
/// script twice; each of our outputs and inputs present; and somebody else
/// holds coins at the session denomination on each side we occupy.
pub fn check_final_tx(
    tx: &Transaction,
    denom: u32,
    our_inputs: &[OutPoint],
    our_outputs: &[TxOut],
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
    for ours in our_outputs {
        if !tx.output.contains(ours) {
            return Err("an output of ours is missing");
        }
    }
    let mut indices = Vec::with_capacity(our_inputs.len());
    for op in our_inputs {
        let Some(i) = tx.input.iter().position(|i| i.previous_output == *op) else {
            return Err("an input of ours is missing");
        };
        indices.push(i);
    }
    let session_outputs = tx
        .output
        .iter()
        .filter(|o| o.value == session_amount)
        .count();
    if session_outputs <= our_outputs.len() {
        return Err("no other participant's output at the session denomination");
    }
    if tx.input.len() <= our_inputs.len() {
        return Err("no other participant's input");
    }
    Ok(indices)
}

struct Running<'a, W: MixingWallet> {
    wallet: &'a W,
    /// Our entry's inputs with their previous output scripts.
    inputs: Vec<(OutPoint, ScriptBuf)>,
    scripts: Vec<ScriptBuf>,
}

impl<W: MixingWallet> Running<'_, W> {
    fn outpoints(&self) -> Vec<OutPoint> {
        self.inputs.iter().map(|(o, _)| *o).collect()
    }

    async fn fail(&mut self) {
        self.wallet.unlock_coins(&self.outpoints());
        self.inputs.clear();
        self.wallet.release_scripts(&self.scripts).await;
        self.scripts.clear();
    }
}

/// Runs one session to its end. `observe` gets every state change;
/// `stop` set to `true` abandons the session (`ResetPool`).
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
    let Some(service) = params.masternode.service else {
        return SessionOutcome::Failed(SessionFailure::Connect("no service address".into()));
    };
    let session = tokio::select! {
        s = Session::connect(service, params.network, params.session_config.clone()) => match s {
            Ok(s) => s,
            Err(e) => return SessionOutcome::Failed(SessionFailure::Connect(e.to_string())),
        },
        _ = stop.wait_for(|s| *s) => return SessionOutcome::Failed(SessionFailure::Cancelled),
    };
    let mut rx = session.subscribe(&[
        dw_p2p::commands::DSSTATUSUPDATE,
        dw_p2p::commands::DSQUEUE,
        dw_p2p::commands::DSFINALTX,
        dw_p2p::commands::DSCOMPLETE,
    ]);
    let accept = Accept {
        denom: params.denom,
        collateral: params.collateral.clone(),
        flags: 0,
    };
    if session
        .send(
            dw_p2p::commands::DSACCEPT,
            accept.encode(session.common_version()),
        )
        .await
        .is_err()
    {
        return SessionOutcome::Failed(SessionFailure::Disconnected);
    }
    let mut run = Running {
        wallet,
        inputs: Vec::new(),
        scripts: Vec::new(),
    };
    let mut our_outputs: Vec<TxOut> = Vec::new();
    let mut deadline = tokio::time::Instant::now() + params.queue_timeout;
    let mut rng = rand::rngs::OsRng;

    let outcome = loop {
        let msg = tokio::select! {
            m = rx.recv() => m,
            _ = tokio::time::sleep_until(deadline) => {
                break SessionOutcome::Failed(SessionFailure::Timeout(progress.state));
            }
            _ = stop.wait_for(|s| *s) => break SessionOutcome::Failed(SessionFailure::Cancelled),
        };
        let Some(msg) = msg else {
            break SessionOutcome::Failed(SessionFailure::Disconnected);
        };
        match msg.command.as_str() {
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
                let coins = match wallet.ready_to_mix(amount).await {
                    Ok(c) => c,
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
                let candidates = select_by_denomination(&coins, amount, MAX_POOL_AMOUNT);
                let inputs = choose_entry_inputs(&candidates, params.rounds, &mut rng);
                if inputs.is_empty() {
                    break SessionOutcome::Failed(SessionFailure::NoInputs);
                }
                let scripts = match wallet.reserve_scripts(inputs.len()).await {
                    Ok(s) if s.len() == inputs.len() => s,
                    Ok(_) => {
                        break SessionOutcome::Failed(SessionFailure::Wallet(
                            "too few output scripts".into(),
                        ));
                    }
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
                run.scripts = scripts.clone();
                run.inputs = inputs
                    .iter()
                    .map(|c| (c.outpoint, c.script_pubkey.clone()))
                    .collect();
                wallet.lock_coins(&run.outpoints());
                our_outputs = scripts
                    .into_iter()
                    .map(|script_pubkey| TxOut {
                        value: amount,
                        script_pubkey,
                    })
                    .collect();
                let entry = Entry {
                    inputs: inputs
                        .iter()
                        .map(|c| TxIn {
                            previous_output: c.outpoint,
                            script_sig: ScriptBuf::new(),
                            sequence: 0xffff_ffff,
                            witness: Default::default(),
                        })
                        .collect(),
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
                progress.entries = inputs.len() as u32;
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
                let indices =
                    match check_final_tx(&f.tx, params.denom, &run.outpoints(), &our_outputs) {
                        Ok(i) => i,
                        Err(why) => break SessionOutcome::Failed(SessionFailure::BadFinalTx(why)),
                    };
                let to_sign: Vec<(usize, ScriptBuf)> = indices
                    .iter()
                    .zip(&run.inputs)
                    .map(|(idx, (_, script))| (*idx, script.clone()))
                    .collect();
                let signed = match wallet.sign_inputs(&f.tx, &to_sign).await {
                    Ok(s) => s,
                    Err(e) => break SessionOutcome::Failed(SessionFailure::Wallet(e.0)),
                };
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

    match &outcome {
        SessionOutcome::Success { inputs } => {
            wallet.hold_until_spent(inputs);
            wallet.keep_scripts(&run.scripts).await;
            progress.state = PoolState::Idle;
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
