//! A whole mixing session against an in-process masternode (QT-045): the
//! mock plays Dash Core's server side (`src/coinjoin/server.cpp`
//! ProcessDSACCEPT → CheckForCompleteQueue → ProcessDSVIN →
//! CreateFinalTransaction → ProcessDSSIGNFINALTX → RelayCompletedTransaction).

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use dashcore::blockdata::transaction::outpoint::OutPoint;
use dashcore::ecdsa::{EcdsaPublicKey, EcdsaSecretKey, EcdsaSignature};
use dashcore::sighash::SighashCache;
use dashcore::{Network, PublicKey, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_coinjoin::client::{
    MIXING_SIGHASH, MixCoin, MixingWallet, RebalanceEntry, SessionFailure, SessionOutcome,
    SessionParams, WalletError, run_session,
};
use dw_coinjoin::denoms::DENOMINATIONS;
use dw_coinjoin::messages::{
    Accept, Complete, Entry, FinalTx, GETDATA, INV, Queue, StatusUpdate, StatusUpdateKind,
    decode_dsq_inv, decode_signed_inputs, encode_dsq_inventory,
};
use dw_coinjoin::rebalance::{FLAG_DEMOTION, FLAG_PROMOTION, RebalanceKind};
use dw_coinjoin::status::{PoolMessage, PoolState};
use dw_p2p::PeerEntry;
use dw_p2p::testing::{MockConn, MockPeer};
use tokio::sync::watch;

#[path = "support/operator_key.rs"]
mod operator_key;
use operator_key::OperatorKey;

const DENOM: u32 = 4; // 0.100001 DASH
const AMOUNT: u64 = 10_000_100;
const LARGER: u64 = DENOMINATIONS[1];
const REBALANCE_VERSION: u32 = 70241;

struct TestWallet {
    keys: Vec<EcdsaSecretKey>,
    coins: Vec<MixCoin>,
    /// Unspent coins of the wallet that no session offers.
    others: Vec<OutPoint>,
    signs: Mutex<usize>,
    locked: Mutex<HashSet<OutPoint>>,
    held: Mutex<Vec<OutPoint>>,
    next_script: Mutex<u8>,
    /// At most this many output scripts per reservation.
    script_limit: usize,
    released: Mutex<Vec<ScriptBuf>>,
    kept: Mutex<Vec<ScriptBuf>>,
    /// Coins the user locked, or that left the wallet, mid-session.
    withdrawn: Mutex<HashSet<OutPoint>>,
}

impl TestWallet {
    fn new(n: u8) -> Self {
        // One key, and so one script, per coin: Dash Core refuses an entry
        // whose inputs pay one script twice.
        let keys: Vec<EcdsaSecretKey> = (0..n)
            .map(|i| {
                let mut b = [0x11; 32];
                b[31] = i.wrapping_add(1);
                EcdsaSecretKey::from_bytes(&b).unwrap()
            })
            .collect();
        let coins = (0..n)
            .map(|i| MixCoin {
                outpoint: OutPoint::new(Txid::from_byte_array([i + 1; 32]), 0),
                value: AMOUNT,
                script_pubkey: script_of(&keys[i as usize]),
                rounds: 0,
            })
            .collect();
        Self {
            keys,
            coins,
            others: vec![OutPoint::new(Txid::from_byte_array([0xee; 32]), 2)],
            signs: Mutex::new(0),
            locked: Mutex::new(HashSet::new()),
            held: Mutex::new(Vec::new()),
            next_script: Mutex::new(0),
            script_limit: usize::MAX,
            released: Mutex::new(Vec::new()),
            kept: Mutex::new(Vec::new()),
            withdrawn: Mutex::new(HashSet::new()),
        }
    }

    /// `n` fully mixed coins of `value`.
    fn with_coins(n: u8, value: u64) -> Self {
        let mut w = Self::new(n);
        for c in &mut w.coins {
            c.value = value;
            c.rounds = 4;
        }
        w
    }

    fn entry(&self, kind: RebalanceKind) -> RebalanceEntry {
        RebalanceEntry {
            kind,
            inputs: self.coins.clone(),
        }
    }

    fn key_for(&self, script: &ScriptBuf) -> Result<&EcdsaSecretKey, WalletError> {
        (self.keys.iter())
            .find(|k| script_of(k) == *script)
            .ok_or_else(|| WalletError("no key for the script".into()))
    }
}

fn script_of(key: &EcdsaSecretKey) -> ScriptBuf {
    let pk = PublicKey::new(key.public_key());
    ScriptBuf::new_p2pkh(&pk.pubkey_hash())
}

fn p2pkh(seed: u8) -> ScriptBuf {
    let mut b = vec![0x76, 0xa9, 0x14];
    b.extend_from_slice(&[seed; 20]);
    b.extend_from_slice(&[0x88, 0xac]);
    ScriptBuf::from_bytes(b)
}

impl MixingWallet for TestWallet {
    async fn ready_to_mix(&self, amount: u64) -> Result<Vec<MixCoin>, WalletError> {
        let locked = self.locked.lock().unwrap();
        Ok(self
            .coins
            .iter()
            .filter(|c| c.value == amount && !locked.contains(&c.outpoint))
            .cloned()
            .collect())
    }

    async fn reserve_scripts(&self, count: usize) -> Result<Vec<ScriptBuf>, WalletError> {
        let mut n = self.next_script.lock().unwrap();
        Ok((0..count.min(self.script_limit))
            .map(|_| {
                *n += 1;
                p2pkh(*n)
            })
            .collect())
    }

    async fn release_scripts(&self, scripts: &[ScriptBuf]) {
        self.released.lock().unwrap().extend_from_slice(scripts);
    }

    async fn keep_scripts(&self, scripts: &[ScriptBuf]) {
        self.kept.lock().unwrap().extend_from_slice(scripts);
    }

    fn lock_coins(&self, outpoints: &[OutPoint]) {
        self.locked
            .lock()
            .unwrap()
            .extend(outpoints.iter().copied());
    }

    fn unlock_coins(&self, outpoints: &[OutPoint]) {
        let mut l = self.locked.lock().unwrap();
        for o in outpoints {
            l.remove(o);
        }
    }

    fn hold_until_spent(&self, outpoints: &[OutPoint]) {
        self.held.lock().unwrap().extend_from_slice(outpoints);
    }

    async fn unspent_outpoints(&self) -> Result<HashSet<OutPoint>, WalletError> {
        Ok(self
            .coins
            .iter()
            .map(|c| c.outpoint)
            .chain(self.others.iter().copied())
            .collect())
    }

    async fn any_withdrawn(&self, outpoints: &[OutPoint]) -> Result<bool, WalletError> {
        let withdrawn = self.withdrawn.lock().unwrap();
        Ok(outpoints.iter().any(|o| withdrawn.contains(o)))
    }

    async fn sign_inputs(
        &self,
        tx: &Transaction,
        inputs: &[(usize, ScriptBuf)],
    ) -> Result<Vec<TxIn>, WalletError> {
        *self.signs.lock().unwrap() += 1;
        let cache = SighashCache::new(tx);
        inputs
            .iter()
            .map(|(i, script)| {
                let h = cache
                    .legacy_signature_hash(*i, script, MIXING_SIGHASH)
                    .map_err(|e| WalletError(e.to_string()))?;
                let key = self.key_for(script)?;
                let mut der = key.sign(&h.to_byte_array()).with_der(<[u8]>::to_vec);
                der.push(MIXING_SIGHASH as u8);
                let mut script_sig = vec![der.len() as u8];
                script_sig.extend_from_slice(&der);
                let pk = key.public_key().to_compressed();
                script_sig.push(pk.len() as u8);
                script_sig.extend_from_slice(&pk);
                let mut input = tx.input[*i].clone();
                input.script_sig = ScriptBuf::from_bytes(script_sig);
                Ok(input)
            })
            .collect()
    }
}

fn collateral() -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0xcc; 32]), 1),
            script_sig: ScriptBuf::new(),
            sequence: 0xffff_ffff,
            witness: Default::default(),
        }],
        output: vec![TxOut {
            value: 30_000,
            script_pubkey: p2pkh(0xcc),
        }],
        special_transaction_payload: None,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct Mn {
    peer: MockPeer,
    op: OperatorKey,
    hash: [u8; 32],
}

impl Mn {
    async fn new() -> Self {
        Self {
            peer: MockPeer::bind(Network::Regtest).await.unwrap(),
            op: OperatorKey::from_seed(b"mn"),
            hash: [0x99; 32],
        }
    }

    fn entry(&self) -> PeerEntry {
        PeerEntry {
            pro_tx_hash: self.hash,
            service: Some(self.peer.addr()),
            operator_public_key: self.op.public_key(),
            is_valid: true,
            is_evonode: false,
        }
    }

    fn ready_queue(&self) -> Vec<u8> {
        self.ready().encode()
    }

    fn ready(&self) -> Queue {
        let mut q = Queue {
            denom: DENOM,
            pro_tx_hash: self.hash,
            time: now(),
            ready: true,
            signature: vec![],
        };
        q.signature = self.op.sign(&q.signature_hash());
        q
    }
}

fn status(kind: StatusUpdateKind, state: PoolState, message: PoolMessage) -> Vec<u8> {
    StatusUpdate {
        session_id: 42,
        state,
        kind,
        message,
    }
    .encode()
}

/// Runs the server side up to receiving the entry.
async fn serve_until_entry(mn: &Mn, conn: &mut MockConn) -> Entry {
    let dsa = conn.recv_command("dsa").await.unwrap();
    let accept = Accept::decode(&dsa.payload, 70233).unwrap();
    assert_eq!(accept.denom, DENOM);
    assert_eq!(accept.flags, 0);
    assert_eq!(accept.collateral, collateral());
    conn.send(
        "dssu",
        &status(
            StatusUpdateKind::Accepted,
            PoolState::Queue,
            PoolMessage::NoError,
        ),
    )
    .await
    .unwrap();
    conn.send("dsq", &mn.ready_queue()).await.unwrap();
    let dsi = conn.recv_command("dsi").await.unwrap();
    Entry::decode(&dsi.payload).unwrap()
}

/// The final transaction: the entry plus one foreign participant, BIP69.
fn final_tx(entry: &Entry, drop_our_output: bool) -> Transaction {
    let mut input = entry.inputs.clone();
    input.push(foreign_input(0xf0));
    let mut output = entry.outputs.clone();
    if drop_our_output {
        output.pop();
    }
    output.push(TxOut {
        value: AMOUNT,
        script_pubkey: p2pkh(0xf0),
    });
    bip69(input, output)
}

fn foreign_input(seed: u8) -> TxIn {
    TxIn {
        previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 3),
        script_sig: ScriptBuf::new(),
        sequence: 0xffff_ffff,
        witness: Default::default(),
    }
}

fn foreign_output(seed: u8, value: u64) -> TxOut {
    TxOut {
        value,
        script_pubkey: p2pkh(seed),
    }
}

fn bip69(mut input: Vec<TxIn>, mut output: Vec<TxOut>) -> Transaction {
    let key_in = |i: &TxIn| {
        let mut t = i.previous_output.txid.to_byte_array();
        t.reverse();
        (t, i.previous_output.vout)
    };
    input.sort_by_key(key_in);
    output.sort_by(|a, b| {
        (a.value, a.script_pubkey.as_bytes()).cmp(&(b.value, b.script_pubkey.as_bytes()))
    });
    Transaction {
        version: 1,
        lock_time: 0,
        input,
        output,
        special_transaction_payload: None,
    }
}

fn params(mn: &Mn) -> SessionParams {
    let mut p = SessionParams::new(Network::Regtest, mn.entry(), DENOM, collateral(), 4);
    p.queue_timeout = Duration::from_secs(5);
    p.signing_timeout = Duration::from_secs(5);
    p
}

#[tokio::test]
async fn test_qt_045_a_full_session_signs_with_anyonecanpay_and_succeeds() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(3);
    let (_stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        assert!(!entry.inputs.is_empty() && entry.inputs.len() <= 3);
        assert_eq!(entry.inputs.len(), entry.outputs.len());
        assert!(entry.outputs.iter().all(|o| o.value == AMOUNT));
        assert_eq!(entry.collateral, collateral());
        let tx = final_tx(&entry, false);
        conn.send(
            "dsf",
            &FinalTx {
                session_id: 42,
                tx: tx.clone(),
            }
            .encode(),
        )
        .await
        .unwrap();
        let dss = conn.recv_command("dss").await.unwrap();
        let signed = decode_signed_inputs(&dss.payload).unwrap();
        assert_eq!(signed.len(), entry.inputs.len());
        // Every signature verifies over the ANYONECANPAY|ALL sighash.
        let cache = SighashCache::new(&tx);
        for input in &signed {
            let idx = tx
                .input
                .iter()
                .position(|i| i.previous_output == input.previous_output)
                .unwrap();
            let bytes = input.script_sig.as_bytes();
            let sig_len = bytes[0] as usize;
            let der = &bytes[1..sig_len];
            assert_eq!(bytes[sig_len], MIXING_SIGHASH as u8);
            let pk = EcdsaPublicKey::from_bytes(&bytes[sig_len + 2..]).unwrap();
            let script = ScriptBuf::new_p2pkh(&PublicKey::new(pk).pubkey_hash());
            let h = cache
                .legacy_signature_hash(idx, &script, MIXING_SIGHASH)
                .unwrap();
            pk.verify(&h.to_byte_array(), EcdsaSignature::from_der(der).unwrap())
                .unwrap();
        }
        conn.send(
            "dsc",
            &Complete {
                session_id: 42,
                message: PoolMessage::Success,
            }
            .encode(),
        )
        .await
        .unwrap();
        entry
    };
    let states = Mutex::new(Vec::new());
    let (entry, outcome) = tokio::join!(
        server,
        run_session(&wallet, p, |s| states.lock().unwrap().push(s.state), stop)
    );
    let SessionOutcome::Success { inputs } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(inputs.len(), entry.inputs.len());
    assert_eq!(*wallet.held.lock().unwrap(), inputs);
    assert_eq!(wallet.kept.lock().unwrap().len(), entry.outputs.len());
    assert!(wallet.released.lock().unwrap().is_empty());
    let states = states.into_inner().unwrap();
    for s in [
        PoolState::Queue,
        PoolState::AcceptingEntries,
        PoolState::Signing,
    ] {
        assert!(states.contains(&s), "{states:?}");
    }
    assert_eq!(states.last(), Some(&PoolState::Idle));
}

/// Stopped once our signatures are out (`dss` sent, no `dsc`): the
/// masternode may still broadcast the mixing transaction, so the inputs
/// stay held and the output scripts kept, nothing released. Stopped before
/// signing, everything is released.
#[tokio::test]
async fn a_stop_after_our_signatures_holds_the_inputs() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(3);
    let (stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        let tx = final_tx(&entry, false);
        conn.send("dsf", &FinalTx { session_id: 42, tx }.encode())
            .await
            .unwrap();
        conn.recv_command("dss").await.unwrap();
        stop_tx.send(true).unwrap();
        (conn, entry)
    };
    let ((_conn, entry), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    let SessionOutcome::SignedUnknown { inputs, failure } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(failure, SessionFailure::Cancelled);
    assert_eq!(inputs.len(), entry.inputs.len());
    assert_eq!(*wallet.held.lock().unwrap(), inputs);
    assert_eq!(wallet.locked.lock().unwrap().len(), inputs.len());
    assert_eq!(wallet.kept.lock().unwrap().len(), entry.outputs.len());
    assert!(wallet.released.lock().unwrap().is_empty());

    // Before `dsf`: no signature left us, everything goes back.
    let mn = Mn::new().await;
    let wallet = TestWallet::new(3);
    let (stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        stop_tx.send(true).unwrap();
        (conn, entry)
    };
    let ((_conn, entry), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(outcome, SessionOutcome::Failed(SessionFailure::Cancelled));
    assert!(wallet.held.lock().unwrap().is_empty());
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(wallet.released.lock().unwrap().len(), entry.outputs.len());
}

/// A coin the user locked while the session waited is not signed.
#[tokio::test]
async fn a_coin_locked_mid_session_is_not_signed() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(3);
    let (_stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        wallet
            .withdrawn
            .lock()
            .unwrap()
            .insert(entry.inputs[0].previous_output);
        let tx = final_tx(&entry, false);
        conn.send("dsf", &FinalTx { session_id: 42, tx }.encode())
            .await
            .unwrap();
        (conn, entry)
    };
    let ((_conn, entry), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::BadFinalTx(
            "an input of ours was locked or left the wallet"
        ))
    );
    assert_eq!(*wallet.signs.lock().unwrap(), 0);
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(wallet.released.lock().unwrap().len(), entry.outputs.len());
}

/// A masternode that says no after our signatures went out may still
/// broadcast them: the inputs and scripts stay held, as after a stop.
#[tokio::test]
async fn a_refusal_after_our_signatures_holds_the_inputs() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(3);
    let (_stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        let tx = final_tx(&entry, false);
        conn.send("dsf", &FinalTx { session_id: 42, tx }.encode())
            .await
            .unwrap();
        conn.recv_command("dss").await.unwrap();
        conn.send(
            "dsc",
            &Complete {
                session_id: 42,
                message: PoolMessage::InvalidTx,
            }
            .encode(),
        )
        .await
        .unwrap();
        (conn, entry)
    };
    let ((_conn, entry), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    let SessionOutcome::SignedUnknown { inputs, failure } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(failure, SessionFailure::Completed(PoolMessage::InvalidTx));
    assert_eq!(*wallet.held.lock().unwrap(), inputs);
    assert_eq!(wallet.kept.lock().unwrap().len(), entry.outputs.len());
    assert!(wallet.released.lock().unwrap().is_empty());
}

/// Dash Core announces the ready `dsq` of a rebalance session by `inv`,
/// and only to a peer that asked for transaction relay.
#[tokio::test]
async fn a_rebalance_session_asks_for_relay() {
    let mn = Mn::new().await;
    let wallet = TestWallet::with_coins(1, DENOMINATIONS[2]);
    let p = rebalance_params(&mn, wallet.entry(RebalanceKind::Demotion));
    assert!(p.session_config.relay);
    assert!(!params(&mn).session_config.relay);
}

#[tokio::test]
async fn a_rejected_dsa_fails_the_session() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(1);
    let (_stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        conn.recv_command("dsa").await.unwrap();
        conn.send(
            "dssu",
            &status(
                StatusUpdateKind::Rejected,
                PoolState::Idle,
                PoolMessage::QueueFull,
            ),
        )
        .await
        .unwrap();
        conn
    };
    let (_conn, outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::Rejected(PoolMessage::QueueFull))
    );
}

#[tokio::test]
async fn a_final_tx_without_our_output_is_not_signed_and_releases_everything() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(2);
    let (_stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_until_entry(&mn, &mut conn).await;
        let tx = final_tx(&entry, true);
        conn.send("dsf", &FinalTx { session_id: 42, tx }.encode())
            .await
            .unwrap();
        (conn, entry)
    };
    let ((_conn, entry), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert!(matches!(
        outcome,
        SessionOutcome::Failed(SessionFailure::BadFinalTx(_))
    ));
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(wallet.released.lock().unwrap().len(), entry.outputs.len());
}

#[tokio::test]
async fn a_silent_masternode_times_out_and_stop_cancels() {
    let mn = Mn::new().await;
    let wallet = TestWallet::new(1);
    let (_stop_tx, stop) = watch::channel(false);
    let mut p = params(&mn);
    p.queue_timeout = Duration::from_millis(300);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        conn.recv_command("dsa").await.unwrap();
        conn
    };
    let (_conn, outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::Timeout(PoolState::Queue))
    );

    let mn = Mn::new().await;
    let (stop_tx, stop) = watch::channel(false);
    let p = params(&mn);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        conn.recv_command("dsa").await.unwrap();
        stop_tx.send(true).unwrap();
        conn
    };
    let (_conn, outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(outcome, SessionOutcome::Failed(SessionFailure::Cancelled));
}

/// Every signed input verifies over the ANYONECANPAY|ALL sighash of `tx`.
fn assert_signed(tx: &Transaction, signed: &[TxIn]) {
    let cache = SighashCache::new(tx);
    for input in signed {
        let idx = tx
            .input
            .iter()
            .position(|i| i.previous_output == input.previous_output)
            .unwrap();
        let bytes = input.script_sig.as_bytes();
        let sig_len = bytes[0] as usize;
        let der = &bytes[1..sig_len];
        assert_eq!(bytes[sig_len], MIXING_SIGHASH as u8);
        let pk = EcdsaPublicKey::from_bytes(&bytes[sig_len + 2..]).unwrap();
        let script = ScriptBuf::new_p2pkh(&PublicKey::new(pk).pubkey_hash());
        let h = cache
            .legacy_signature_hash(idx, &script, MIXING_SIGHASH)
            .unwrap();
        pk.verify(&h.to_byte_array(), EcdsaSignature::from_der(der).unwrap())
            .unwrap();
    }
}

fn rebalance_params(mn: &Mn, entry: RebalanceEntry) -> SessionParams {
    let mut p =
        SessionParams::rebalancing(Network::Regtest, mn.entry(), DENOM, collateral(), 4, entry);
    p.queue_timeout = Duration::from_secs(5);
    p.signing_timeout = Duration::from_secs(5);
    p
}

/// A rebalance session's server side up to its ready `dsq`, as a Core
/// masternode talks to a peer at the rebalance protocol: the `dsa` carries
/// the flag and the ready `dsq` is announced by `inv` and sent on
/// `getdata`.
async fn serve_rebalance_until_ready(mn: &Mn, conn: &mut MockConn, flag: u8) {
    let dsa = conn.recv_command("dsa").await.unwrap();
    let accept = Accept::decode(&dsa.payload, REBALANCE_VERSION).unwrap();
    assert_eq!(accept.denom, DENOM);
    assert_eq!(accept.flags, flag);
    conn.send(
        "dssu",
        &status(
            StatusUpdateKind::Accepted,
            PoolState::Queue,
            PoolMessage::NoError,
        ),
    )
    .await
    .unwrap();
    let ready = mn.ready();
    conn.send(INV, &encode_dsq_inventory(&[ready.hash()]))
        .await
        .unwrap();
    let getdata = conn.recv_command(GETDATA).await.unwrap();
    assert_eq!(
        decode_dsq_inv(&getdata.payload).unwrap(),
        vec![ready.hash()]
    );
    conn.send("dsq", &ready.encode()).await.unwrap();
}

/// [`serve_rebalance_until_ready`], then the entry.
async fn serve_rebalance_until_entry(mn: &Mn, conn: &mut MockConn, flag: u8) -> Entry {
    serve_rebalance_until_ready(mn, conn, flag).await;
    let dsi = conn.recv_command("dsi").await.unwrap();
    Entry::decode(&dsi.payload).unwrap()
}

/// Our promotion with two foreign 1:1 participants: twelve 0.1 inputs, our
/// 1 DASH output and two 0.1 outputs.
fn promotion_final_tx(entry: &Entry) -> (Vec<TxIn>, Vec<TxOut>) {
    let mut input = entry.inputs.clone();
    input.extend([foreign_input(0xf0), foreign_input(0xf1)]);
    let mut output = entry.outputs.clone();
    output.extend([foreign_output(0xf0, AMOUNT), foreign_output(0xf1, AMOUNT)]);
    (input, output)
}

/// Our demotion with a foreign demotion: two 1 DASH inputs, twenty 0.1
/// outputs.
fn demotion_final_tx(entry: &Entry) -> (Vec<TxIn>, Vec<TxOut>) {
    let mut input = entry.inputs.clone();
    input.push(foreign_input(0xf0));
    let mut output = entry.outputs.clone();
    output.extend((0..10).map(|i| foreign_output(0xd0 + i, AMOUNT)));
    (input, output)
}

/// Serves a rebalance session to a signed final transaction and `dsc`.
async fn run_signed_rebalance(
    kind: RebalanceKind,
    wallet: &TestWallet,
    final_tx: fn(&Entry) -> (Vec<TxIn>, Vec<TxOut>),
) -> (Entry, SessionOutcome) {
    let mn = Mn::new().await;
    let (_stop_tx, stop) = watch::channel(false);
    let flag = match kind {
        RebalanceKind::Promotion => FLAG_PROMOTION,
        RebalanceKind::Demotion => FLAG_DEMOTION,
    };
    let p = rebalance_params(&mn, wallet.entry(kind));
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_rebalance_until_entry(&mn, &mut conn, flag).await;
        let locked = wallet.locked.lock().unwrap().clone();
        assert!(
            entry
                .inputs
                .iter()
                .all(|i| locked.contains(&i.previous_output)),
            "the entry's inputs are locked while it is out"
        );
        let (input, output) = final_tx(&entry);
        let tx = bip69(input, output);
        conn.send(
            "dsf",
            &FinalTx {
                session_id: 42,
                tx: tx.clone(),
            }
            .encode(),
        )
        .await
        .unwrap();
        let dss = conn.recv_command("dss").await.unwrap();
        let signed = decode_signed_inputs(&dss.payload).unwrap();
        assert_eq!(signed.len(), entry.inputs.len());
        assert_signed(&tx, &signed);
        conn.send(
            "dsc",
            &Complete {
                session_id: 42,
                message: PoolMessage::Success,
            }
            .encode(),
        )
        .await
        .unwrap();
        entry
    };
    tokio::join!(server, run_session(wallet, p, |_| {}, stop))
}

#[tokio::test]
async fn a_promotion_sends_ten_inputs_for_one_larger_output_and_signs_them() {
    let wallet = TestWallet::with_coins(10, AMOUNT);
    let (entry, outcome) =
        run_signed_rebalance(RebalanceKind::Promotion, &wallet, promotion_final_tx).await;
    assert_eq!(entry.inputs.len(), 10);
    let offered: HashSet<OutPoint> = wallet.coins.iter().map(|c| c.outpoint).collect();
    assert!(
        entry
            .inputs
            .iter()
            .all(|i| offered.contains(&i.previous_output))
    );
    assert_eq!(entry.outputs.len(), 1);
    assert_eq!(entry.outputs[0].value, LARGER);
    assert!(entry.outputs[0].script_pubkey.is_p2pkh());
    let SessionOutcome::Success { inputs } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(inputs.len(), 10);
    assert_eq!(*wallet.held.lock().unwrap(), inputs);
    assert_eq!(
        *wallet.kept.lock().unwrap(),
        vec![entry.outputs[0].script_pubkey.clone()]
    );
    assert!(wallet.released.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_demotion_sends_one_input_for_ten_smaller_outputs_and_signs_it() {
    let wallet = TestWallet::with_coins(1, LARGER);
    let (entry, outcome) =
        run_signed_rebalance(RebalanceKind::Demotion, &wallet, demotion_final_tx).await;
    assert_eq!(entry.inputs.len(), 1);
    assert_eq!(entry.inputs[0].previous_output, wallet.coins[0].outpoint);
    assert_eq!(entry.outputs.len(), 10);
    assert!(entry.outputs.iter().all(|o| o.value == AMOUNT));
    let scripts: HashSet<&ScriptBuf> = entry.outputs.iter().map(|o| &o.script_pubkey).collect();
    assert_eq!(scripts.len(), 10);
    let SessionOutcome::Success { inputs } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(inputs, vec![wallet.coins[0].outpoint]);
    assert_eq!(wallet.kept.lock().unwrap().len(), 10);
}

/// The masternode sends a final transaction built by `tamper` from our
/// entry: the client must refuse it, sign nothing, send no `dss` and give
/// back its coins and scripts.
async fn assert_refused(
    kind: RebalanceKind,
    wallet: TestWallet,
    tamper: impl FnOnce(&Entry, &TestWallet) -> Transaction,
    why: &'static str,
) {
    let mn = Mn::new().await;
    let (_stop_tx, stop) = watch::channel(false);
    let flag = match kind {
        RebalanceKind::Promotion => FLAG_PROMOTION,
        RebalanceKind::Demotion => FLAG_DEMOTION,
    };
    let p = rebalance_params(&mn, wallet.entry(kind));
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        let entry = serve_rebalance_until_entry(&mn, &mut conn, flag).await;
        let tx = tamper(&entry, &wallet);
        conn.send("dsf", &FinalTx { session_id: 42, tx }.encode())
            .await
            .unwrap();
        assert!(
            conn.recv_command("dss").await.is_err(),
            "no signature may leave"
        );
        entry
    };
    let (entry, outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::BadFinalTx(why))
    );
    assert_eq!(*wallet.signs.lock().unwrap(), 0);
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert!(wallet.held.lock().unwrap().is_empty());
    assert_eq!(wallet.released.lock().unwrap().len(), entry.outputs.len());
}

#[tokio::test]
async fn a_promotion_without_its_larger_output_is_not_signed() {
    assert_refused(
        RebalanceKind::Promotion,
        TestWallet::with_coins(10, AMOUNT),
        |entry, _| {
            let (input, mut output) = promotion_final_tx(entry);
            output.retain(|o| o.value != LARGER);
            output.push(foreign_output(0xf2, LARGER));
            bip69(input, output)
        },
        "an output of ours is missing",
    )
    .await;
}

#[tokio::test]
async fn a_demotion_with_nine_of_its_ten_outputs_is_not_signed() {
    assert_refused(
        RebalanceKind::Demotion,
        TestWallet::with_coins(1, LARGER),
        |entry, _| {
            let (input, mut output) = demotion_final_tx(entry);
            let ours = entry.outputs[3].clone();
            output.retain(|o| *o != ours);
            output.push(foreign_output(0xf2, AMOUNT));
            bip69(input, output)
        },
        "an output of ours is missing",
    )
    .await;
}

#[tokio::test]
async fn a_promotion_output_at_a_changed_amount_is_not_signed() {
    assert_refused(
        RebalanceKind::Promotion,
        TestWallet::with_coins(10, AMOUNT),
        |entry, _| {
            let (input, mut output) = promotion_final_tx(entry);
            for o in &mut output {
                if o.value == LARGER {
                    o.value = AMOUNT;
                }
            }
            bip69(input, output)
        },
        "an output of ours is missing",
    )
    .await;
}

#[tokio::test]
async fn a_final_tx_spending_a_coin_of_ours_we_did_not_offer_is_not_signed() {
    assert_refused(
        RebalanceKind::Demotion,
        TestWallet::with_coins(1, LARGER),
        |entry, wallet| {
            let (mut input, output) = demotion_final_tx(entry);
            input.retain(|i| i.previous_output.txid != Txid::from_byte_array([0xf0; 32]));
            input.push(TxIn {
                previous_output: wallet.others[0],
                ..foreign_input(0)
            });
            bip69(input, output)
        },
        "spends a coin of ours we did not offer",
    )
    .await;
}

/// Our ten promotion inputs padded with a foreign 1 DASH input (a demotion)
/// instead of anyone else's 0.1 inputs: on chain ours would be the only 0.1
/// inputs, linked to each other.
#[tokio::test]
async fn a_promotion_with_no_other_input_at_the_session_denomination_is_not_signed() {
    assert_refused(
        RebalanceKind::Promotion,
        TestWallet::with_coins(10, AMOUNT),
        |entry, _| {
            let mut input = entry.inputs.clone();
            input.push(foreign_input(0xf0));
            let mut output = entry.outputs.clone();
            output.extend((0..10).map(|i| foreign_output(0xd0 + i, AMOUNT)));
            bip69(input, output)
        },
        "no other participant's input at the session denomination",
    )
    .await;
}

/// A masternode below the rebalance protocol would read the flags byte as
/// missing and take a promotion entry for 1:1 mixing (and charge the
/// collateral): no `dsa` goes out, the inputs come back.
#[tokio::test]
async fn a_rebalance_never_goes_to_a_masternode_below_the_rebalance_protocol() {
    let mn = Mn {
        peer: MockPeer::bind_with_version(Network::Regtest, REBALANCE_VERSION - 1)
            .await
            .unwrap(),
        op: OperatorKey::from_seed(b"mn"),
        hash: [0x99; 32],
    };
    let wallet = TestWallet::with_coins(10, AMOUNT);
    let (_stop_tx, stop) = watch::channel(false);
    let p = rebalance_params(&mn, wallet.entry(RebalanceKind::Promotion));
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        assert!(conn.recv_command("dsa").await.is_err());
    };
    let ((), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::MasternodeTooOld(REBALANCE_VERSION - 1))
    );
    assert!(wallet.locked.lock().unwrap().is_empty());
}

/// Before V24 a Core masternode turns a rebalance `dsa` away with
/// `ERR_VERSION` (`CreateNewSession`), charging nothing: the session ends
/// and its inputs come back.
#[tokio::test]
async fn a_rebalance_dsa_turned_away_releases_its_inputs() {
    let mn = Mn::new().await;
    let wallet = TestWallet::with_coins(1, LARGER);
    let (_stop_tx, stop) = watch::channel(false);
    let p = rebalance_params(&mn, wallet.entry(RebalanceKind::Demotion));
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        conn.recv_command("dsa").await.unwrap();
        conn.send(
            "dssu",
            &status(
                StatusUpdateKind::Rejected,
                PoolState::Idle,
                PoolMessage::Version,
            ),
        )
        .await
        .unwrap();
        conn
    };
    let (_conn, outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::Rejected(PoolMessage::Version))
    );
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(*wallet.signs.lock().unwrap(), 0);
}

/// Entries of the wrong shape never reach a masternode.
#[tokio::test]
async fn a_malformed_rebalance_entry_is_never_sent() {
    let mn = Mn::new().await;
    let wallet = TestWallet::with_coins(9, AMOUNT);
    let (_stop_tx, stop) = watch::channel(false);
    let p = rebalance_params(&mn, wallet.entry(RebalanceKind::Promotion));
    let outcome = run_session(&wallet, p, |_| {}, stop).await;
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::BadRebalance("wrong input count"))
    );
    assert!(wallet.locked.lock().unwrap().is_empty());
}

/// A rebalance input spent since the session was planned: at the ready
/// `dsq` the entry is not sent and the inputs come back.
#[tokio::test]
async fn a_rebalance_whose_input_was_spent_sends_no_entry() {
    let mn = Mn::new().await;
    let wallet = TestWallet::with_coins(1, LARGER);
    let mut entry = wallet.entry(RebalanceKind::Demotion);
    entry.inputs[0].outpoint = OutPoint::new(Txid::from_byte_array([0xdd; 32]), 0);
    let (_stop_tx, stop) = watch::channel(false);
    let p = rebalance_params(&mn, entry);
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        serve_rebalance_until_ready(&mn, &mut conn, FLAG_DEMOTION).await;
        assert!(conn.recv_command("dsi").await.is_err(), "no entry");
    };
    let ((), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::BadRebalance(
            "an input is no longer unspent"
        ))
    );
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(*wallet.signs.lock().unwrap(), 0);
}

/// A demotion promises ten outputs: with nine scripts it sends no entry
/// and gives the nine back.
#[tokio::test]
async fn a_demotion_short_of_output_scripts_sends_no_entry() {
    let mn = Mn::new().await;
    let mut wallet = TestWallet::with_coins(1, LARGER);
    wallet.script_limit = 9;
    let (_stop_tx, stop) = watch::channel(false);
    let p = rebalance_params(&mn, wallet.entry(RebalanceKind::Demotion));
    let server = async {
        let mut conn = mn.peer.accept().await.unwrap();
        serve_rebalance_until_ready(&mn, &mut conn, FLAG_DEMOTION).await;
        assert!(conn.recv_command("dsi").await.is_err(), "no entry");
    };
    let ((), outcome) = tokio::join!(server, run_session(&wallet, p, |_| {}, stop));
    assert_eq!(
        outcome,
        SessionOutcome::Failed(SessionFailure::Wallet("too few output scripts".into()))
    );
    assert!(wallet.locked.lock().unwrap().is_empty());
    assert_eq!(wallet.released.lock().unwrap().len(), 9);
}
