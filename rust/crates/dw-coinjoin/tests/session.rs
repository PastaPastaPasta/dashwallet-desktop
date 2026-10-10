//! A whole mixing session against an in-process masternode (QT-045): the
//! mock plays Dash Core's server side (`src/coinjoin/server.cpp`
//! ProcessDSACCEPT → CheckForCompleteQueue → ProcessDSVIN →
//! CreateFinalTransaction → ProcessDSSIGNFINALTX → RelayCompletedTransaction).

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use dashcore::blockdata::transaction::outpoint::OutPoint;
use dashcore::hashes::Hash;
use dashcore::secp256k1::{Message, SecretKey, ecdsa};
use dashcore::sighash::SighashCache;
use dashcore::{Network, PublicKey, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_coinjoin::bls::testing::OperatorKey;
use dw_coinjoin::client::{
    MIXING_SIGHASH, MixCoin, MixingWallet, SessionFailure, SessionOutcome, SessionParams,
    WalletError, run_session,
};
use dw_coinjoin::messages::{
    Accept, Complete, Entry, FinalTx, Queue, StatusUpdate, StatusUpdateKind, decode_signed_inputs,
};
use dw_coinjoin::status::{PoolMessage, PoolState};
use dw_p2p::PeerEntry;
use dw_p2p::testing::{MockConn, MockPeer};
use tokio::sync::watch;

const DENOM: u32 = 4; // 0.100001 DASH
const AMOUNT: u64 = 10_000_100;

struct TestWallet {
    key: SecretKey,
    coins: Vec<MixCoin>,
    locked: Mutex<HashSet<OutPoint>>,
    held: Mutex<Vec<OutPoint>>,
    next_script: Mutex<u8>,
    released: Mutex<Vec<ScriptBuf>>,
    kept: Mutex<Vec<ScriptBuf>>,
}

impl TestWallet {
    fn new(n: u8) -> Self {
        let key = SecretKey::from_secret_bytes([0x11; 32]).unwrap();
        let pk = PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&key));
        let script = ScriptBuf::new_p2pkh(&pk.pubkey_hash());
        let coins = (0..n)
            .map(|i| MixCoin {
                outpoint: OutPoint::new(Txid::from_byte_array([i + 1; 32]), 0),
                value: AMOUNT,
                script_pubkey: script.clone(),
                rounds: 0,
            })
            .collect();
        Self {
            key,
            coins,
            locked: Mutex::new(HashSet::new()),
            held: Mutex::new(Vec::new()),
            next_script: Mutex::new(0),
            released: Mutex::new(Vec::new()),
            kept: Mutex::new(Vec::new()),
        }
    }

    fn pubkey(&self) -> dashcore::secp256k1::PublicKey {
        dashcore::secp256k1::PublicKey::from_secret_key(&self.key)
    }
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
        Ok((0..count)
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

    async fn sign_inputs(
        &self,
        tx: &Transaction,
        inputs: &[(usize, ScriptBuf)],
    ) -> Result<Vec<TxIn>, WalletError> {
        let cache = SighashCache::new(tx);
        inputs
            .iter()
            .map(|(i, script)| {
                let h = cache
                    .legacy_signature_hash(*i, script, MIXING_SIGHASH)
                    .map_err(|e| WalletError(e.to_string()))?;
                let sig = ecdsa::sign(Message::from_digest(h.to_byte_array()), &self.key);
                let mut der = sig.serialize_der().to_vec();
                der.push(MIXING_SIGHASH as u8);
                let mut script_sig = vec![der.len() as u8];
                script_sig.extend_from_slice(&der);
                let pk = self.pubkey().serialize();
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
        let mut q = Queue {
            denom: DENOM,
            pro_tx_hash: self.hash,
            time: now(),
            ready: true,
            signature: vec![],
        };
        q.signature = self.op.sign(&q.signature_hash());
        q.encode()
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
    input.push(TxIn {
        previous_output: OutPoint::new(Txid::from_byte_array([0xf0; 32]), 3),
        ..entry.inputs[0].clone()
    });
    let mut output = entry.outputs.clone();
    if drop_our_output {
        output.pop();
    }
    output.push(TxOut {
        value: AMOUNT,
        script_pubkey: p2pkh(0xf0),
    });
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
            let pk = dashcore::secp256k1::PublicKey::from_slice(&bytes[sig_len + 2..]).unwrap();
            let script = ScriptBuf::new_p2pkh(&PublicKey::new(pk).pubkey_hash());
            let h = cache
                .legacy_signature_hash(idx, &script, MIXING_SIGHASH)
                .unwrap();
            ecdsa::verify(
                &ecdsa::Signature::from_der(der).unwrap(),
                Message::from_digest(h.to_byte_array()),
                &pk,
            )
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
