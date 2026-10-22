//! Shared masternodes, slice 1a, on a regtest session (MG-04b §12, T0
//! dw-engine): three wallets of one process take a registration from the
//! Invitation to the participants' signed contributions with every message
//! passed as text, the capability override standing in for BL-187; a
//! dash-qt participant signs with a raw owner key; restarts and a crash
//! between the `out_at` write and the return; refusals in §7's order; the
//! charges, the in-flight mark, §3.0's cells and D30. Nothing is ever
//! handed to the network (the broadcaster fails the test if it is).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::secp256k1::{Message as SecpMessage, PublicKey, ecdsa::Signature};
use dashcore::{OutPoint, Transaction, TxOut, Txid};
use dw_appdb::GLOBAL_SCOPE;
use dw_protx::shared::envelope::encode_base64;
use dw_protx::shared::json::Json;
use dw_protx::shared::session::funding::{check_frozen, decode_frozen};
use dw_protx::shared::session::merge::{add_signature, check_join_signatures};
use dw_protx::shared::session::{Contribution, Input, Session, Stage};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use platform_wallet::PlatformWalletError;
use zeroize::Zeroizing;

use super::super::protx::ops::{CrashPoint, Seam};
use crate::masternodes::{
    Masternodes, MasternodesError, OperatorChoice, RewardChoice, ShareSlot, ShareTerms,
    SharedImport, SharedNext, SharedOutcome, SharedRole, SharedSession, SharedStage,
    StartSharedRequest, UpdateRegistrarRequest, UpdateShareRequest, VotingChoice,
};
use crate::send::TestBroadcaster;
use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, WalletId};

const COIN: u64 = 100_000_000;
const FEE: u64 = 100_000;
const PHRASES: [&str; 3] = [
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
    "legal winner thank year wave sausage worth useful legal winner thank yellow",
];

/// Nothing in slice 1a may reach the network.
struct NoNetwork;

impl TestBroadcaster for NoNetwork {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn broadcast(
        &self,
        _: &Transaction,
        _: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        panic!("slice 1a handed a transaction to the network");
    }
}

struct NoEvents;

impl crate::events::EventSink for NoEvents {
    fn emit(&self, _: crate::events::EngineEvent) {}
}

struct Fx {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallets: Vec<WalletId>,
    seam: Arc<Seam>,
    /// Coins credited, per wallet, to credit again after a restart.
    coins: Mutex<Vec<(usize, u8, u64)>>,
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap()
}

fn seam_for(session: &NetworkSession) -> Arc<Seam> {
    let seam = Arc::new(Seam {
        broadcaster: Arc::new(NoNetwork),
        crash: Mutex::new(None),
        list: Mutex::new(Some(Vec::new())),
        list_height: Mutex::new(100),
        shared_mined: Mutex::default(),
    });
    session.masternodes.set_protx_seam(Some(seam.clone()));
    seam
}

/// A regtest session with `n` wallets; v24 seen when `v24`.
fn fx(n: usize, v24: bool) -> Fx {
    let dir = dw_testutil::private_tempdir();
    let config = EngineConfig {
        data_root: dir.path().join("data"),
        worker_threads: Some(2),
        vault: VaultConfig {
            kdf: KdfPolicy::Fixed(KdfParams::TEST),
            os_store: Arc::new(MemoryOsStore::new()),
            ..VaultConfig::default()
        },
    };
    let engine = Engine::new(config, Arc::new(NoEvents)).unwrap();
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let wallets = PHRASES[..n]
        .iter()
        .map(|p| {
            engine
                .block_on(session.import_wallet(
                    Zeroizing::new(p.as_bytes().to_vec()),
                    Zeroizing::new(Vec::new()),
                    ImportOptions {
                        birth_height: Some(0),
                        core_compat: false,
                        ..Default::default()
                    },
                ))
                .unwrap()
        })
        .collect();
    let seam = seam_for(&session);
    let f = Fx {
        _dir: dir,
        engine,
        session,
        wallets,
        seam,
        coins: Mutex::default(),
    };
    if v24 {
        f.db()
            .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", Some("1"))
            .unwrap();
    }
    f
}

impl Fx {
    /// The facade with the capability override (BL-187's stand-in).
    fn mn(&self) -> Masternodes {
        Masternodes::with_shared_send(&self.session, true)
    }

    /// The facade as shipped: `can_send` false.
    fn pin(&self) -> Masternodes {
        self.session.masternodes()
    }

    fn db(&self) -> Arc<dw_appdb::AppDb> {
        self.session.live().unwrap().appdb
    }

    fn run<T>(&self, f: impl Future<Output = T>) -> T {
        self.engine.block_on(f)
    }

    fn grant(&self, w: usize, max_duffs: u64) -> String {
        self.session
            .vault()
            .authorize(
                GrantPurpose::MasternodeOp { max_duffs },
                Some(&self.wallets[w].0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    /// A confirmed coin `seed` of `value` at a fresh receive address of
    /// wallet `w`.
    fn credit(&self, w: usize, seed: u8, value: u64) -> OutPoint {
        self.coins.lock().unwrap().push((w, seed, value));
        self.credit_again(w, seed, value)
    }

    fn credit_again(&self, w: usize, seed: u8, value: u64) -> OutPoint {
        let (session, id) = (Arc::clone(&self.session), self.wallets[w]);
        let inner = Arc::clone(&session);
        self.run(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let address = wallet.core().next_receive_address_for_account(0).await?;
            let outpoint = OutPoint::new(Txid::from_byte_array([seed; 32]), 0);
            let mut utxo = Utxo::new(
                outpoint,
                TxOut {
                    value,
                    script_pubkey: address.script_pubkey(),
                },
                address,
                1,
                false,
            );
            utxo.is_confirmed = true;
            wallet
                .state_mut()
                .await
                .core_wallet
                .accounts
                .standard_bip44_accounts
                .get_mut(&0)
                .expect("BIP44 account 0")
                .utxos
                .insert(outpoint, utxo);
            Ok(outpoint)
        }))
        .unwrap()
    }

    /// The wallet no longer holds `outpoint` (spent elsewhere).
    fn spend(&self, w: usize, outpoint: OutPoint) {
        let (session, id) = (Arc::clone(&self.session), self.wallets[w]);
        let inner = Arc::clone(&session);
        self.run(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            wallet
                .state_mut()
                .await
                .core_wallet
                .accounts
                .standard_bip44_accounts
                .get_mut(&0)
                .expect("BIP44 account 0")
                .utxos
                .remove(&outpoint);
            Ok(())
        }))
        .unwrap();
    }

    /// Close and reopen on the same data, as a crash would leave it.
    fn restart(&mut self) {
        assert!(
            self.engine
                .block_on(self.engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        self.session = open(&self.engine);
        self.seam = seam_for(&self.session);
        let coins = self.coins.lock().unwrap().clone();
        for (w, seed, value) in coins {
            self.credit_again(w, seed, value);
        }
    }

    fn locked(&self, w: usize) -> HashSet<OutPoint> {
        self.db()
            .locks(&self.wallets[w].to_string())
            .unwrap()
            .into_iter()
            .filter(|r| r.is_locked())
            .map(|r| OutPoint::new(r.txid.parse().unwrap(), r.vout))
            .collect()
    }

    fn row(&self, w: usize, id: &str) -> dw_appdb::SharedSessionRow {
        self.db()
            .shared_session(&self.wallets[w].to_string(), id)
            .unwrap()
            .expect("a stored session")
    }

    fn import(&self, w: usize, text: &str) -> Result<SharedImport, MasternodesError> {
        self.run(
            self.mn()
                .import_shared_message(self.wallets[w], text.to_string(), None),
        )
    }

    fn imported(&self, w: usize, text: &str) -> (SharedSession, bool) {
        match self.import(w, text).unwrap() {
            SharedImport::Session {
                session, changed, ..
            } => (session, changed),
        }
    }

    fn session_of(&self, w: usize, id: &str) -> SharedSession {
        self.run(self.mn().shared_session(self.wallets[w], id.to_string()))
            .unwrap()
    }

    fn message(&self, w: usize, id: &str) -> String {
        self.run(self.mn().shared_message(self.wallets[w], id.to_string()))
            .unwrap()
            .text
    }

    fn join(&self, w: usize, id: &str) -> SharedOutcome {
        self.run(self.mn().join_shared(
            self.wallets[w],
            id.to_string(),
            crate::masternodes::JoinSharedRequest {
                // Two rows are open in a three-party Invitation: each names
                // its own (A's is 0).
                share_index: Some(u16::try_from(w).unwrap()),
                reward: RewardChoice::Refund,
            },
        ))
        .unwrap()
    }

    /// `lock_shared_terms` on the revision the session record shows now.
    fn lock(&self, w: usize, id: &str) -> Result<SharedOutcome, MasternodesError> {
        self.lock_at(w, id, self.session_of(w, id).revision)
    }

    fn lock_at(
        &self,
        w: usize,
        id: &str,
        revision: u32,
    ) -> Result<SharedOutcome, MasternodesError> {
        let grant = self.grant(w, 0);
        self.run(
            self.mn()
                .lock_shared_terms(self.wallets[w], id.to_string(), revision, grant),
        )
    }

    /// `approve_shared` on the consent hash the session record shows now.
    fn approve(&self, w: usize, id: &str) -> Result<SharedOutcome, MasternodesError> {
        let hash = self.session_of(w, id).consent_hash.unwrap_or_default();
        self.approve_with(w, id, &hash)
    }

    fn approve_with(
        &self,
        w: usize,
        id: &str,
        consent_hash: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let grant = self.grant(w, 0);
        self.run(self.mn().approve_shared(
            self.wallets[w],
            id.to_string(),
            consent_hash.to_string(),
            grant,
        ))
    }

    fn sign(&self, w: usize, id: &str, max: u64) -> Result<SharedOutcome, MasternodesError> {
        let grant = self.grant(w, max);
        self.run(
            self.mn()
                .sign_shared_funding(self.wallets[w], id.to_string(), grant),
        )
    }
}

fn request(shares: &[(&str, u64)], fee: u64) -> StartSharedRequest {
    StartSharedRequest {
        shares: shares
            .iter()
            .map(|(label, dash)| ShareSlot {
                label: (*label).into(),
                amount: dash * COIN,
            })
            .collect(),
        terms: ShareTerms {
            service: "1.2.3.4:19999".into(),
            operator: OperatorChoice::NextUnused,
            operator_reward: 0,
            voting: VotingChoice::NextUnused,
            early_period_blocks: 1_000,
            early_penalty: COIN,
        },
        fee,
        reward: RewardChoice::Refund,
        node_run_by: Some("A".into()),
    }
}

fn start(f: &Fx, w: usize, req: StartSharedRequest) -> Result<SharedOutcome, MasternodesError> {
    f.run(f.mn().start_shared(f.wallets[w], req))
}

fn parse(text: &str) -> Session {
    Session::from_json(
        &Json::read(text.as_bytes()).unwrap(),
        dashcore::Network::Regtest,
    )
    .unwrap()
}

fn code(e: MasternodesError) -> String {
    e.code().to_string()
}

/// The `send.*` code a facade error carries.
fn send_code(e: &MasternodesError) -> String {
    match e.engine() {
        Some(e @ crate::EngineError::Send(_)) => super::super::protx::ops::error_code(e),
        _ => panic!("not a send failure: {e:?}"),
    }
}

fn assert_message(e: MasternodesError, reason: &str) {
    match e {
        MasternodesError::SharedMessage { reason: r, .. } => assert_eq!(r, reason),
        other => panic!("expected shared_message{{{reason}}}, got {other:?}"),
    }
}

/// The three-party session up to every approval: A coordinates (400),
/// B and C take 300 each. Returns the session id.
fn three_approved(f: &Fx) -> String {
    f.credit(0, 1, 401 * COIN);
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let invitation = start(f, 0, request(&[("A", 400), ("B", 300), ("C", 300)], FEE)).unwrap();
    let id = invitation.session.id.clone();
    assert_eq!(invitation.message.name, "Invitation");
    assert_eq!(
        invitation.message.file_name,
        format!("shared-mn-session-{}.json", id[..6].to_uppercase())
    );
    for w in [1, 2] {
        let (s, changed) = f.imported(w, &invitation.message.text);
        assert!(changed);
        assert_eq!(
            (s.stage, s.role, s.next),
            (
                SharedStage::Invited,
                SharedRole::Participant,
                SharedNext::Join
            )
        );
        assert!(f.locked(w).is_empty(), "an invitation touches no coin");
    }
    let b = f.join(1, &id);
    assert_eq!(
        (b.message.name.as_str(), b.session.revision),
        ("Details", 2)
    );
    let c = f.join(2, &id);
    assert_eq!(f.imported(0, &b.message.text).0.revision, 2);
    let after = f.imported(0, &c.message.text).0;
    assert_eq!((after.revision, after.next), (3, SharedNext::Lock));
    // A re-pasted reply adds nothing.
    assert!(!f.imported(0, &c.message.text).1);

    let locked = f.lock(0, &id).unwrap();
    assert_eq!(locked.message.name, "Locked Terms");
    assert_eq!(locked.session.revision, 3, "freeze keeps the revision");
    for w in [1, 2] {
        let (s, _) = f.imported(w, &locked.message.text);
        assert_eq!(
            (s.stage, s.next),
            (SharedStage::Signing, SharedNext::Approve)
        );
        let approval = f.approve(w, &id).unwrap();
        assert_eq!(approval.message.name, "Approval");
        assert_eq!(approval.session.revision, 3);
        f.imported(0, &approval.message.text);
    }
    id
}

#[test]
fn test_QT_126_three_wallets_reach_the_signed_registration() {
    let f = fx(3, true);
    let id = three_approved(&f);
    let a = f.session_of(0, &id);
    assert_eq!((a.stage, a.next), (SharedStage::Combined, SharedNext::Copy));
    let request = f.message(0, &id);
    assert_eq!(parse(&request).stage, Stage::Combined);

    // Each participant: D12's checks, its inputs signed, `out_at` first.
    for w in [1, 2] {
        let (s, _) = f.imported(w, &request);
        assert_eq!((s.stage, s.next), (SharedStage::Combined, SharedNext::Sign));
        // The charge is the debit: a grant below it is refused.
        let short = f.sign(w, &id, 300 * COIN - 1).unwrap_err();
        assert_eq!(send_code(&short), "send.grant_exceeded");
        assert!(f.row(w, &id).out_at.is_none());
        let signed = f.sign(w, &id, 301 * COIN).unwrap();
        assert_eq!(signed.message.name, "Signed Contribution");
        assert_eq!(signed.session.stage, SharedStage::FundingSigned);
        assert!(f.row(w, &id).out_at.is_some());
        f.imported(0, &signed.message.text);
    }
    let a = f.session_of(0, &id);
    assert_eq!(
        a.next,
        SharedNext::Send,
        "only the coordinator's inputs are left"
    );
    assert!(
        a.rows
            .iter()
            .skip(1)
            .all(|r| r.funding_signed && r.approved)
    );
    assert!(!a.rows[0].funding_signed);

    // The registration decodes; every join signature verifies; with the
    // coordinator's inputs (1b) stood in, it passes `fromJson` at
    // `fundingSigned`; each participant scriptSig signs its input.
    let mut s = parse(&f.message(0, &id));
    let frozen = check_frozen(&s).unwrap();
    check_join_signatures(&s, &frozen).unwrap();
    assert_eq!(frozen.payload.shares.len(), 3);
    let own_a: HashSet<OutPoint> = f.locked(0);
    for (i, input) in frozen.tx.input.iter().enumerate() {
        if own_a.contains(&input.previous_output) {
            assert!(input.script_sig.is_empty(), "D24: no coordinator scriptSig");
            continue;
        }
        let b = input.script_sig.as_bytes();
        let sig_len = b[0] as usize;
        let der = &b[1..sig_len];
        assert_eq!(b[sig_len], 1, "SIGHASH_ALL");
        let pk = PublicKey::from_slice(&b[sig_len + 2..]).unwrap();
        let owner = (1..3)
            .find(|w| f.locked(*w).contains(&input.previous_output))
            .expect("a participant's input");
        let script = dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(pk).pubkey_hash());
        let (session, id) = (Arc::clone(&f.session), f.wallets[owner]);
        let inner = Arc::clone(&session);
        let o = input.previous_output;
        let coin_script = f.run({
            session.on_runtime(async move {
                let w = inner.wallet(&id).await?;
                let state = w.state().await;
                Ok(crate::coins::read_coins(&state.core_wallet)
                    .coins
                    .into_iter()
                    .find(|c| c.utxo.outpoint == o)
                    .unwrap()
                    .utxo
                    .txout
                    .script_pubkey)
            })
        });
        assert_eq!(coin_script.unwrap(), script);
        let hash = frozen.tx.signature_hash_all(i, &script).unwrap();
        Signature::from_der(der)
            .unwrap()
            .verify(SecpMessage::from_digest(hash), &pk)
            .unwrap();
    }
    let mut tx = frozen.tx.clone();
    for input in &mut tx.input {
        if input.script_sig.is_empty() {
            input.script_sig = dashcore::ScriptBuf::from_bytes(vec![0x51]);
        }
    }
    s.protx = hex::encode(tx.to_bytes());
    s.stage = Stage::FundingSigned;
    check_frozen(&s).unwrap();

    // A copy carrying another scriptSig on one of B's inputs is refused
    // (m-7): B's inputs are only the ones B signed.
    let mut forged = parse(&f.message(0, &id));
    let mut tx = decode_frozen(&forged).unwrap().tx;
    let b_coins = f.locked(1);
    for input in &mut tx.input {
        if b_coins.contains(&input.previous_output) {
            input.script_sig = dashcore::ScriptBuf::from_bytes(vec![0x52]);
        }
    }
    forged.protx = hex::encode(tx.to_bytes());
    match f.import(1, &forged.to_text()).unwrap_err() {
        MasternodesError::SharedFundingMismatch { reason, .. } => {
            assert_eq!(reason, "foreign_input")
        }
        other => panic!("{other:?}"),
    }

    // Nothing went out; the participants' sessions can't be discarded.
    for w in [1, 2] {
        let e = f
            .run(f.mn().discard_shared_session(f.wallets[w], id.clone()))
            .unwrap_err();
        assert_eq!(code(e), "masternodes.shared_stage");
    }
}

#[test]
fn test_QT_126_sign_shared_funding_refuses_at_the_pin() {
    let f = fx(3, true);
    let id = three_approved(&f);
    let request = f.message(0, &id);
    f.imported(1, &request);
    let grant = f.grant(1, 400 * COIN);
    let e = f
        .run(f.pin().sign_shared_funding(f.wallets[1], id.clone(), grant))
        .unwrap_err();
    match e {
        MasternodesError::Unsupported { feature, field } => {
            assert_eq!((feature.as_str(), field), ("shared_masternodes", None));
        }
        other => panic!("{other:?}"),
    }
    assert!(f.row(1, &id).out_at.is_none());
    let cap = f.run(f.pin().shared_capability(f.wallets[1])).unwrap();
    assert!(!cap.can_send);
    assert_eq!(cap.v24, crate::masternodes::Activation::Active);
    assert_eq!(cap.evonode_shares, crate::masternodes::Activation::Inactive);
}

#[test]
fn test_QT_126_a_dash_qt_participant_signing_with_its_owner_key() {
    // dashd's `shared_sign` is `SignCompact` of the consent digest by the
    // share's owner key (oracle_shared proves the two equal); the second
    // party here is that key, from the vectors, editing the envelope by
    // hand as dash-qt would.
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../testdata/shared_registration.json"
    ))
    .unwrap();
    let party = &vectors["cases"][0]["parties"][1];
    let wif = party["owner_wif"].as_str().unwrap();
    let secret = dashcore::PrivateKey::from_wif(wif)
        .unwrap()
        .inner
        .to_secret_bytes();
    let f = fx(1, true);
    f.credit(0, 1, 701 * COIN);
    let inv = start(&f, 0, request(&[("A", 700), ("Q", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();

    let mut q = parse(&inv.message.text);
    q.shares[1].owner_address = party["share"]["ownerAddress"].as_str().unwrap().into();
    q.shares[1].refund_address = party["share"]["refundAddress"].as_str().unwrap().into();
    q.contributions.push(Contribution {
        label: "Q".into(),
        inputs: vec![Input {
            txid: party["funding_coin"]["txid"].as_str().unwrap().into(),
            vout: 1,
            sequence: u32::MAX,
        }],
        change: None,
        change_index: -1,
    });
    dw_protx::shared::session::funding::rebuild_funding_tx(&mut q).unwrap();
    q.revision += 1;
    let s = f.imported(0, &q.to_text()).0;
    assert_eq!(
        s.warnings.len(),
        1,
        "Q's input can't be valued: {:?}",
        s.warnings
    );

    let locked = f.lock(0, &id).unwrap();
    let mut q = parse(&locked.message.text);
    let frozen = decode_frozen(&q).unwrap();
    let sig = dw_protx::payloads::sign_hash_compact(&secret, &frozen.consent_hash).unwrap();
    assert!(add_signature(&mut q, &frozen, 1, &encode_base64(&sig)).unwrap());
    let a = f.imported(0, &q.to_text()).0;
    assert_eq!(a.stage, SharedStage::Combined);
    let combined = parse(&f.message(0, &id));
    check_join_signatures(&combined, &check_frozen(&combined).unwrap()).unwrap();

    // A second, different signature for Q's share is refused.
    let mut forged = parse(&locked.message.text);
    let other = dw_protx::payloads::sign_hash_compact(&[7; 32], &frozen.consent_hash).unwrap();
    forged.sigs.push(dw_protx::shared::session::Signature {
        share_index: 1,
        signature: encode_base64(&other),
    });
    forged.stage = Stage::Signing;
    assert!(f.import(0, &forged.to_text()).is_err());
}

#[test]
fn test_QT_126_restarts_keep_locks_and_never_reselect() {
    let mut f = fx(3, true);
    f.credit(0, 1, 401 * COIN);
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let inv = start(&f, 0, request(&[("A", 400), ("B", 300), ("C", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let a_locks = f.locked(0);
    assert_eq!(a_locks.len(), 1);
    f.restart();
    assert_eq!(f.locked(0), a_locks);
    assert_eq!(f.session_of(0, &id).reserved.len(), 1);

    f.imported(1, &inv.message.text);
    let b = f.join(1, &id);
    let b_locks = f.locked(1);
    f.restart();
    assert_eq!(f.locked(1), b_locks);
    f.imported(0, &b.message.text);
    f.imported(2, &inv.message.text);
    let c = f.join(2, &id);
    f.imported(0, &c.message.text);
    let locked = f.lock(0, &id).unwrap();
    f.restart();
    for w in [1, 2] {
        f.imported(w, &locked.message.text);
        let ap = f.approve(w, &id).unwrap();
        f.imported(0, &ap.message.text);
    }
    f.restart();
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Combined);
    let request = f.message(0, &id);
    f.imported(1, &request);

    // A crash after the `out_at` write, before the return: the row is out,
    // never returned, never deletable, and its signatures are kept.
    *f.seam.crash.lock().unwrap() = Some(CrashPoint::SharedOut);
    assert!(f.sign(1, &id, 301 * COIN).is_err());
    let row = f.row(1, &id);
    assert!(row.out_at.is_some() && row.signed_tx.is_some());
    f.restart();
    let b = f.session_of(1, &id);
    assert_eq!(b.stage, SharedStage::FundingSigned);
    assert_eq!(f.locked(1), b_locks, "nothing re-selected or released");
    let e = f
        .run(f.mn().discard_shared_session(f.wallets[1], id.clone()))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.shared_stage");
    let again = f.sign(1, &id, 301 * COIN).unwrap_err();
    assert_eq!(code(again), "masternodes.shared_stage");
    // The Signed Contribution is still there to copy.
    let signed = parse(&f.message(1, &id));
    let tx = decode_frozen(&signed).unwrap().tx;
    assert!(tx.input.iter().any(|i| !i.script_sig.is_empty()));

    // A spent reservation is a dead session, never re-selected (D17).
    let c_coin = *f.locked(2).iter().next().unwrap();
    f.spend(2, c_coin);
    f.imported(2, &request);
    let e = f.sign(2, &id, 400 * COIN).unwrap_err();
    assert!(
        matches!(e, MasternodesError::SharedStage { ref stage } if stage == "dead"),
        "{e:?}"
    );
    assert!(matches!(
        f.session_of(2, &id).stage,
        SharedStage::Dead { .. }
    ));
}

#[test]
fn test_QT_126_calls_refuse_in_order() {
    let f = fx(2, false);
    let mn = f.mn();
    let w = f.wallets[0];
    let unknown = WalletId([9; 32]);
    let bad = "NOT-AN-ID".to_string();
    let good = "ab".repeat(32);

    // Arguments first, whatever the wallet.
    for e in [
        f.run(mn.shared_session(unknown, bad.clone())).unwrap_err(),
        f.run(mn.set_shared_fee(unknown, bad.clone(), FEE))
            .unwrap_err(),
        f.run(mn.shared_message(unknown, bad.clone())).unwrap_err(),
        f.run(mn.lock_shared_terms(unknown, bad.clone(), 1, "g".into()))
            .unwrap_err(),
        f.run(mn.unlock_shared_terms(unknown, bad.clone()))
            .unwrap_err(),
        f.run(mn.approve_shared(unknown, bad.clone(), String::new(), "g".into()))
            .unwrap_err(),
        f.run(mn.approve_shared(unknown, good.clone(), "zz".into(), "g".into()))
            .unwrap_err(),
        f.run(mn.sign_shared_funding(unknown, bad.clone(), "g".into()))
            .unwrap_err(),
        f.run(mn.discard_shared_session(unknown, bad.clone()))
            .unwrap_err(),
        f.run(mn.import_shared_message(unknown, "{}".into(), Some(bad.clone())))
            .unwrap_err(),
        f.run(mn.start_shared(unknown, request(&[("A", 1000)], 0)))
            .unwrap_err(),
    ] {
        assert_eq!(code(e), "invalid_argument");
    }
    // Then the wallet, never quoting its id.
    let e = f.run(mn.shared_session(unknown, good.clone())).unwrap_err();
    assert_eq!(e.code(), "wallet_not_found");
    assert!(!e.to_string().contains(&unknown.to_string()));
    // Then v24 for creation; `can_send` for the signing call.
    for e in [
        f.run(mn.start_shared(w, request(&[("A", 500), ("B", 500)], FEE)))
            .unwrap_err(),
        f.run(mn.join_shared(
            w,
            good.clone(),
            crate::masternodes::JoinSharedRequest {
                share_index: None,
                reward: RewardChoice::Refund,
            },
        ))
        .unwrap_err(),
        f.run(mn.lock_shared_terms(w, good.clone(), 1, "g".into()))
            .unwrap_err(),
        f.run(mn.approve_shared(w, good.clone(), "0".repeat(64), "g".into()))
            .unwrap_err(),
        f.run(f.pin().sign_shared_funding(w, good.clone(), "g".into()))
            .unwrap_err(),
    ] {
        assert_eq!(code(e), "masternodes.unsupported");
    }
    assert_eq!(
        f.run(mn.shared_capability(w)).unwrap().v24,
        crate::masternodes::Activation::Unknown
    );
    f.db()
        .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", Some("1"))
        .unwrap();
    // An IPv6 coordinator service is unsupported (D5), after the wallet.
    let mut v6 = request(&[("A", 500), ("B", 500)], FEE);
    v6.terms.service = "[2001:db8::1]:19999".into();
    match f.run(mn.start_shared(w, v6)).unwrap_err() {
        MasternodesError::Unsupported { field, .. } => {
            assert_eq!(field.as_deref(), Some("service"))
        }
        other => panic!("{other:?}"),
    }
    // Then the list.
    *f.seam.list.lock().unwrap() = None;
    let e = f
        .run(mn.start_shared(w, request(&[("A", 500), ("B", 500)], FEE)))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.list_unavailable");
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    // Then the session.
    for e in [
        f.run(mn.shared_session(w, good.clone())).unwrap_err(),
        f.run(mn.lock_shared_terms(w, good.clone(), 1, "g".into()))
            .unwrap_err(),
        f.run(mn.discard_shared_session(w, good.clone()))
            .unwrap_err(),
    ] {
        assert_eq!(code(e), "masternodes.shared_session_not_found");
    }
    // Then the message: labels (D28) before Core's rules.
    let e = f
        .run(mn.start_shared(w, request(&[("A", 500), (" A ", 500)], FEE)))
        .unwrap_err();
    assert_message(e, "labels");
    // Then Core's rules, with Core's reasons.
    let e = f
        .run(mn.start_shared(w, request(&[("A", 500), ("B", 400)], FEE)))
        .unwrap_err();
    assert!(
        matches!(e, MasternodesError::ProtxRefused { ref reason, .. } if reason == "bad-protx-shares-amount-sum"),
        "{e:?}"
    );
    let mut reward = request(&[("A", 500), ("B", 500)], FEE);
    reward.terms.operator_reward = 10_001;
    let e = f.run(mn.start_shared(w, reward)).unwrap_err();
    assert!(
        matches!(e, MasternodesError::ProtxRefused { ref reason, .. } if reason == "bad-protx-operator-reward"),
        "{e:?}"
    );
    // No coins: the send's code, last.
    let e = f
        .run(mn.start_shared(w, request(&[("A", 500), ("B", 500)], FEE)))
        .unwrap_err();
    assert_eq!(send_code(&e), "send.amount_exceeds_balance");
    assert!(f.locked(0).is_empty());
}

#[test]
fn test_QT_126_messages_are_refused_without_echo() {
    let f = fx(1, true);
    let w = f.wallets[0];
    let mn = f.mn();
    let secret = "cUfkacao5atR827gWUCARinpJJmFqPyL2VECySZxEzsEhYcP4qkd";
    for (text, reason) in [
        (secret.to_string(), "not_shared"),
        (format!("{{\"type\":\"{secret}\"}}"), "not_shared"),
        ("[]".to_string(), "parse"),
    ] {
        let e = f.run(mn.import_shared_message(w, text, None)).unwrap_err();
        assert!(!e.to_string().contains(secret));
        assert_message(e, reason);
    }
    // Maintenance messages are part 2.
    let e = f
        .run(mn.import_shared_message(w, "[]".into(), Some("ab".repeat(32))))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.unsupported");
    // A frozen copy without a session of ours: nothing to check it against.
    let g = fx(2, true);
    let id = {
        g.credit(0, 1, 501 * COIN);
        let inv = start(&g, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
        let mut q = parse(&inv.message.text);
        q.shares[1].owner_address = "yNacHvuusYb8xz94o2YvCh332yVzpuoSMw".into();
        q.shares[1].refund_address = "ydwX3cfcvo1xezp1QeZ9Bvj8sgqVeAvWe5".into();
        q.contributions.push(Contribution {
            label: "B".into(),
            inputs: vec![Input {
                txid: "11".repeat(32),
                vout: 0,
                sequence: u32::MAX,
            }],
            change: None,
            change_index: -1,
        });
        dw_protx::shared::session::funding::rebuild_funding_tx(&mut q).unwrap();
        q.revision += 1;
        g.imported(0, &q.to_text());
        g.lock(0, &inv.session.id).unwrap();
        inv.session.id
    };
    let e = g.import(1, &g.message(0, &id)).unwrap_err();
    assert_message(e, "not_participant");
}

#[test]
fn test_QT_126_import_table_cells() {
    let f = fx(3, true);
    f.credit(0, 1, 401 * COIN);
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let inv = start(&f, 0, request(&[("A", 400), ("B", 300), ("C", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();
    f.imported(1, &inv.message.text);
    f.imported(2, &inv.message.text);
    let b = f.join(1, &id);
    let c = f.join(2, &id);

    // set_shared_fee: `replied` once another contribution is in (D33); a
    // reply to an Invitation from before a fee change is `older`.
    let g = fx(2, true);
    g.credit(0, 1, 501 * COIN);
    g.credit(1, 2, 501 * COIN);
    let ginv = start(&g, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    g.imported(1, &ginv.message.text);
    let gb = g.join(1, &ginv.session.id);
    let refeed = g
        .run(
            g.mn()
                .set_shared_fee(g.wallets[0], ginv.session.id.clone(), 2 * FEE),
        )
        .unwrap();
    assert_eq!(refeed.session.revision, 2);
    assert_eq!(refeed.session.fee, Some(2 * FEE));
    assert_message(g.import(0, &gb.message.text).unwrap_err(), "older");
    // B's joined copy and the new Invitation share a revision: B joins
    // again from the current Invitation (D33).
    assert_message(g.import(1, &refeed.message.text).unwrap_err(), "conflict");
    g.run(
        g.mn()
            .discard_shared_session(g.wallets[1], ginv.session.id.clone()),
    )
    .unwrap();
    assert!(g.locked(1).is_empty());
    let (again, _) = g.imported(1, &refeed.message.text);
    assert_eq!(again.stage, SharedStage::Invited);
    let rejoined = g.join(1, &ginv.session.id);
    let merged = g.imported(0, &rejoined.message.text).0;
    assert_eq!((merged.revision, merged.next), (3, SharedNext::Lock));

    // Coordinator draft: replies merge; a frozen copy is `older`.
    f.imported(0, &b.message.text);
    f.imported(0, &c.message.text);
    let e = f
        .run(f.mn().set_shared_fee(f.wallets[0], id.clone(), 2 * FEE))
        .unwrap_err();
    assert!(matches!(e, MasternodesError::SharedStage { ref stage } if stage == "replied"));
    // A lower-revision copy is `older` for a participant.
    assert_message(f.import(1, &inv.message.text).unwrap_err(), "older");

    let locked = f.lock(0, &id).unwrap();
    // Coordinator signing: a draft is `older`.
    assert_message(f.import(0, &b.message.text).unwrap_err(), "older");
    f.imported(1, &locked.message.text);
    let approval = f.approve(1, &id).unwrap();
    // Approving twice signs nothing new.
    assert_eq!(
        f.approve(1, &id).unwrap().message.text,
        approval.message.text
    );
    f.imported(0, &approval.message.text);

    // M-1 case 1: the coordinator unlocks; B takes the new draft, back to
    // `joined`, same reservation, its approval dropped.
    let b_locks = f.locked(1);
    let unlocked = f
        .run(f.mn().unlock_shared_terms(f.wallets[0], id.clone()))
        .unwrap();
    assert_eq!(unlocked.session.revision, 4);
    assert_eq!(unlocked.session.stage, SharedStage::Draft);
    let (bs, _) = f.imported(1, &unlocked.message.text);
    assert_eq!((bs.stage, bs.revision), (SharedStage::Draft, 4));
    assert!(
        bs.approved_terms.is_some(),
        "the last approval stays the diff base"
    );
    assert_eq!(f.locked(1), b_locks);
    assert_eq!(parse(&f.message(1, &id)).sigs.len(), 0);

    // M-1 case 2: re-locked; B must approve again.
    let relocked = f.lock(0, &id).unwrap();
    let (bs, _) = f.imported(1, &relocked.message.text);
    assert_eq!(
        (bs.stage, bs.next),
        (SharedStage::Signing, SharedNext::Approve)
    );

    // A copy whose own row changed is refused (M-1, mutant 24).
    let mut tampered = parse(&relocked.message.text);
    tampered.revision += 1;
    tampered.shares[1].refund_address = "ydwX3cfcvo1xezp1QeZ9Bvj8sgqVeAvWe5".into();
    // (from_json re-checks the frozen payload, so the change can't ride
    // a valid copy: it fails as a malformed or changed message.)
    assert!(f.import(1, &tampered.to_text()).is_err());

    // Coordinator past draft: a higher revision is `conflict`.
    let mut higher = parse(&relocked.message.text);
    higher.revision += 1;
    assert_message(f.import(0, &higher.to_text()).unwrap_err(), "conflict");

    // A message of another network is refused as such.
    let other = relocked.message.text.replace("\"regtest\"", "\"test\"");
    assert_message(f.import(1, &other).unwrap_err(), "network");
}

#[test]
fn test_QT_126_in_flight_mark_is_per_session() {
    let f = fx(1, true);
    f.credit(0, 1, 501 * COIN);
    let inv = start(&f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let held = f.session.claim_shared_session(f.wallets[0], &id).unwrap();
    let e = f
        .run(f.mn().set_shared_fee(f.wallets[0], id.clone(), 2 * FEE))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.in_flight");
    drop(held);
    f.run(f.mn().set_shared_fee(f.wallets[0], id.clone(), 2 * FEE))
        .unwrap();
    // Discard unlocks and deletes.
    f.run(f.mn().discard_shared_session(f.wallets[0], id.clone()))
        .unwrap();
    assert!(f.locked(0).is_empty());
    assert!(
        f.run(f.mn().shared_sessions(f.wallets[0]))
            .unwrap()
            .is_empty()
    );
}

/// A confirmed registration of A's: the participants' inputs signed, the
/// coordinator's stood in (1b signs them), confirmed at `height`.
fn confirmed(f: &Fx, height: u32) -> (String, Txid) {
    let id = three_approved(f);
    let request = f.message(0, &id);
    for w in [1, 2] {
        f.imported(w, &request);
        let signed = f.sign(w, &id, 301 * COIN).unwrap();
        f.imported(0, &signed.message.text);
    }
    let mut s = parse(&f.message(0, &id));
    let mut tx = decode_frozen(&s).unwrap().tx;
    for input in &mut tx.input {
        if input.script_sig.is_empty() {
            input.script_sig = dashcore::ScriptBuf::from_bytes(vec![0x51]);
        }
    }
    s.protx = hex::encode(tx.to_bytes());
    s.stage = Stage::FundingSigned;
    let row = f.row(0, &id);
    f.db()
        .update_shared_session_durable(
            &f.wallets[0].to_string(),
            &id,
            &dw_appdb::SharedSessionUpdate {
                envelope: s.to_text(),
                my_shares: row.my_shares,
                contribution: row.contribution,
                approved_terms: row.approved_terms,
                signed_tx: None,
                txid: None,
                last_error: None,
                updated_at: row.updated_at,
            },
        )
        .unwrap();
    // The pinned library reads no version-3 payload (BL-187): the history
    // holds its inputs and outputs under the ProTx hash.
    let txid = tx.txid();
    let mined = Transaction {
        version: 3,
        lock_time: 0,
        input: tx.input.clone(),
        output: tx.output.clone(),
        special_transaction_payload: None,
    };
    f.seam
        .shared_mined
        .lock()
        .unwrap()
        .push((txid, mined, height));
    (id, txid)
}

#[test]
fn test_QT_127_update_share_preview_and_d30() {
    let f = fx(3, true);
    let (id, txid) = confirmed(&f, 50);
    let a = f.session_of(0, &id);
    assert_eq!(a.stage, SharedStage::Confirmed { height: 50 });
    assert_eq!(a.pro_tx_hash.as_deref(), Some(txid.to_string().as_str()));
    // A participant holds only its own signed copy, whose txid isn't the
    // ProTx hash: the registration is found in its history all the same.
    for w in [1, 2] {
        let p = f.session_of(w, &id);
        assert_eq!(p.stage, SharedStage::Confirmed { height: 50 }, "wallet {w}");
        assert_eq!(p.pro_tx_hash.as_deref(), Some(txid.to_string().as_str()));
        assert_ne!(p.txid.as_deref(), Some(txid.to_string().as_str()));
    }
    // The follower releases a settled session's locks.
    f.run(f.session.protx_follow_pass()).unwrap();
    assert!(f.locked(0).is_empty());

    let preview = |share: u16, reward: RewardChoice| {
        f.run(f.mn().preview_update_share(
            f.wallets[0],
            UpdateShareRequest {
                pro_tx_hash: txid.to_string(),
                share_index: share,
                reward,
            },
        ))
    };
    // The masternode in the synced list (m-16).
    let entry = platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: txid.to_byte_array(),
        service_address: Some("1.2.3.4:19999".parse().unwrap()),
        platform_http_port: None,
        operator_public_key: [1; 48],
        voting_key_id: [1; 20],
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: true,
    };
    *f.seam.list.lock().unwrap() = Some(vec![entry]);
    f.credit(0, 9, COIN);
    let p = preview(0, RewardChoice::Refund).unwrap();
    assert!(p.reward_is_refund && p.fee > 0 && p.total_debit == p.fee);
    assert_eq!(p.reward_address, a.rows[0].refund_address);
    let p = preview(0, RewardChoice::NewAddress).unwrap();
    assert_eq!((p.reward_address, p.reward_is_refund), (None, false));
    assert_eq!(
        code(preview(1, RewardChoice::Refund).unwrap_err()),
        "masternodes.share_not_mine"
    );
    *f.seam.list_height.lock().unwrap() = 50;
    assert_eq!(
        code(preview(0, RewardChoice::Refund).unwrap_err()),
        "masternodes.shared_too_early"
    );
    *f.seam.list_height.lock().unwrap() = 100;
    let listed = f.seam.list.lock().unwrap().replace(Vec::new());
    assert_eq!(
        code(preview(0, RewardChoice::Refund).unwrap_err()),
        "masternodes.shared_too_early"
    );
    *f.seam.list.lock().unwrap() = listed;
    let e = f
        .run(f.mn().preview_update_share(
            f.wallets[0],
            UpdateShareRequest {
                pro_tx_hash: "cd".repeat(32),
                share_index: 0,
                reward: RewardChoice::Refund,
            },
        ))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.shared_registration_unknown");

    // D30: MG-04's registrar calls refuse the shared masternode.
    let e = f
        .run(f.mn().preview_update_registrar(
            f.wallets[0],
            UpdateRegistrarRequest {
                pro_tx_hash: txid.to_string(),
                operator: crate::masternodes::KeepOr::Keep,
                voting: crate::masternodes::KeepOr::Keep,
                payout: crate::masternodes::PayoutChoice::NewAddress,
            },
        ))
        .unwrap_err();
    assert!(
        matches!(e, MasternodesError::ProtxRefused { ref reason, .. } if reason == "bad-protx-shared-mn"),
        "{e:?}"
    );
}

#[test]
fn test_QT_126_locked_terms_never_carry_our_approval() {
    let f = fx(2, true);
    f.credit(0, 1, 501 * COIN);
    f.credit(1, 2, 501 * COIN);
    let inv = start(&f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    f.imported(1, &inv.message.text);
    let b = f.join(1, &id);
    f.imported(0, &b.message.text);
    let locked = f.lock(0, &id).unwrap();

    // A junk approval for B's share rides the Locked Terms: dropped, and
    // only `approve_shared` signs B's consent.
    let mut junk = parse(&locked.message.text);
    junk.sigs.push(dw_protx::shared::session::Signature {
        share_index: 1,
        signature: encode_base64(&[7; 65]),
    });
    junk.stage = Stage::Signing;
    let (s, _) = f.imported(1, &junk.to_text());
    assert_eq!(
        (s.stage, s.next),
        (SharedStage::Signing, SharedNext::Approve)
    );
    let held = parse(&f.message(1, &id));
    assert!(held.signature_for(1).is_none());
    assert!(
        held.signature_for(0).is_some(),
        "the coordinator's verifies"
    );
    let approval = f.approve(1, &id).unwrap();

    // M-1 case 1, then B's own old Approval pasted back: its signature
    // isn't carried over either.
    let unlocked = f
        .run(f.mn().unlock_shared_terms(f.wallets[0], id.clone()))
        .unwrap();
    assert_eq!(
        f.imported(1, &unlocked.message.text).0.stage,
        SharedStage::Draft
    );
    let (s, _) = f.imported(1, &approval.message.text);
    assert_eq!(
        (s.stage, s.next),
        (SharedStage::Signing, SharedNext::Approve)
    );
    assert!(parse(&f.message(1, &id)).signature_for(1).is_none());
}

/// A two-party session, B's reply absorbed: A has 500 + fee, B 501 (so B's
/// contribution carries change). Returns the session id.
fn two_joined(f: &Fx) -> String {
    f.credit(0, 1, 501 * COIN);
    f.credit(1, 2, 501 * COIN);
    let inv = start(f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    f.imported(1, &inv.message.text);
    let b = f.join(1, &id);
    f.imported(0, &b.message.text);
    id
}

fn assert_funding(e: MasternodesError, want: &str) {
    match e {
        MasternodesError::SharedFundingMismatch { reason, .. } => assert_eq!(reason, want),
        other => panic!("expected shared_funding_mismatch{{{want}}}, got {other:?}"),
    }
}

fn assert_stage(e: MasternodesError, want: &str) {
    match e {
        MasternodesError::SharedStage { stage } => assert_eq!(stage, want),
        other => panic!("expected shared_stage{{{want}}}, got {other:?}"),
    }
}

/// H-1, D15 (mutant 8): a newer draft never replaces a session past
/// `signing`. A coordinator that locked refuses it `older`, one at
/// `combined` `conflict`; a participant at `combined`, and after its
/// `out_at`, `conflict`.
#[test]
fn test_QT_126_a_newer_draft_never_replaces_a_session_past_signing() {
    let g = fx(2, true);
    let gid = two_joined(&g);
    let locked = g.lock(0, &gid).unwrap();
    let mut draft = parse(&locked.message.text);
    super::build::unfreeze(&mut draft);
    assert_message(g.import(0, &draft.to_text()).unwrap_err(), "older");

    let f = fx(3, true);
    let id = three_approved(&f);
    let request = f.message(0, &id);
    // The same session as an unlock would leave it, one revision higher.
    let mut draft = parse(&request);
    super::build::unfreeze(&mut draft);
    let draft = draft.to_text();
    assert_message(f.import(0, &draft).unwrap_err(), "conflict");
    f.imported(1, &request);
    assert_message(f.import(1, &draft).unwrap_err(), "conflict");
    f.sign(1, &id, 301 * COIN).unwrap();
    assert_message(f.import(1, &draft).unwrap_err(), "conflict");
    assert_eq!(f.session_of(1, &id).stage, SharedStage::FundingSigned);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Combined);
}

/// M-1 (mutants 24 and 26): a participant that approved takes a newer copy
/// only with its own row and contribution unchanged. A re-locked copy,
/// validly frozen, with its refund moved is `row_changed`; a newer draft
/// with its change moved (its coins re-chosen for it) is
/// `contribution_changed`. Nothing of its session moves.
#[test]
fn test_QT_126_newer_copies_changing_our_side_are_refused() {
    let f = fx(2, true);
    let id = two_joined(&f);
    let locked = f.lock(0, &id).unwrap();
    f.imported(1, &locked.message.text);
    f.approve(1, &id).unwrap();
    let held = f.message(1, &id);
    let elsewhere = "ydwX3cfcvo1xezp1QeZ9Bvj8sgqVeAvWe5";

    // As unlock and lock make it, with B's refund moved: a new digest.
    let mut relocked = parse(&locked.message.text);
    super::build::unfreeze(&mut relocked);
    relocked.shares[1].refund_address = elsewhere.into();
    super::build::freeze(&mut relocked).unwrap();
    assert_funding(f.import(1, &relocked.to_text()).unwrap_err(), "row_changed");

    let mut draft = parse(&locked.message.text);
    super::build::unfreeze(&mut draft);
    draft.contributions[1]
        .change
        .as_mut()
        .expect("B's change")
        .address = elsewhere.into();
    dw_protx::shared::session::funding::rebuild_funding_tx(&mut draft).unwrap();
    assert_funding(
        f.import(1, &draft.to_text()).unwrap_err(),
        "contribution_changed",
    );
    assert_eq!(f.message(1, &id), held);
}

/// M-1 case 2 (mutant 25): after an unlock and re-lock, `approve_shared`
/// signs again: the earlier approval recorded as the diff base doesn't
/// count as one, and the Approval carries a signature over the frozen
/// registration it returns.
#[test]
fn test_QT_126_a_relocked_copy_is_signed_by_a_fresh_approval() {
    use dw_protx::shared::session::funding::verify_consent_signature;

    // Three parties, so B's approval alone doesn't let A combine.
    let f = fx(3, true);
    f.credit(0, 1, 401 * COIN);
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let inv = start(&f, 0, request(&[("A", 400), ("B", 300), ("C", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();
    for w in [1, 2] {
        f.imported(w, &inv.message.text);
        let details = f.join(w, &id);
        f.imported(0, &details.message.text);
    }
    let locked = f.lock(0, &id).unwrap();
    f.imported(1, &locked.message.text);
    let first = f.approve(1, &id).unwrap();
    f.imported(0, &first.message.text);
    let unlocked = f
        .run(f.mn().unlock_shared_terms(f.wallets[0], id.clone()))
        .unwrap();
    f.imported(1, &unlocked.message.text);
    let relocked = f.lock(0, &id).unwrap();
    let (s, _) = f.imported(1, &relocked.message.text);
    assert_eq!(
        (s.stage, s.next),
        (SharedStage::Signing, SharedNext::Approve)
    );
    assert!(
        s.approved_terms.is_some(),
        "the old approval stays the diff base"
    );
    assert!(parse(&f.message(1, &id)).signature_for(1).is_none());

    let again = parse(&f.approve(1, &id).unwrap().message.text);
    let frozen = decode_frozen(&again).unwrap();
    let sig = again.signature_for(1).expect("B's fresh approval");
    assert!(verify_consent_signature(&frozen, 1, &sig.signature));
}

/// D33 (mutant 32): `set_shared_fee` only on a `draft`. A draft whose
/// reserved coin was spent is `dead`: the fee isn't changed and no other
/// coin is selected for it.
#[test]
fn test_QT_126_set_shared_fee_refuses_a_dead_draft() {
    let f = fx(1, true);
    f.credit(0, 1, 501 * COIN);
    f.credit(0, 2, 501 * COIN);
    let inv = start(&f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let reserved = f.locked(0);
    assert_eq!(reserved.len(), 1);
    f.spend(0, *reserved.iter().next().unwrap());
    let e = f
        .run(f.mn().set_shared_fee(f.wallets[0], id.clone(), 2 * FEE))
        .unwrap_err();
    assert_stage(e, "dead");
    assert_eq!(f.locked(0), reserved);
    assert_eq!(f.session_of(0, &id).fee, Some(FEE));
}

/// D17 (mutant 6): a reservation whose lock was released (coin control
/// unlocks it, so another send may select the coin) is `dead` for signing:
/// `approve_shared` refuses and adds no signature.
#[test]
fn test_QT_126_a_released_reservation_signs_nothing() {
    let f = fx(2, true);
    let id = two_joined(&f);
    let locked = f.lock(0, &id).unwrap();
    f.imported(1, &locked.message.text);
    let coin = *f.locked(1).iter().next().unwrap();
    assert!(
        f.db()
            .unlock(
                &f.wallets[1].to_string(),
                &coin.txid.to_string(),
                coin.vout,
                1
            )
            .unwrap()
    );
    assert_stage(f.approve(1, &id).unwrap_err(), "dead");
    assert!(parse(&f.message(1, &id)).signature_for(1).is_none());
}

/// `signedOnlyOwnInputs` (mutant 14): a combined copy that already carries
/// a scriptSig on one of this wallet's inputs is never signed over. If the
/// import takes it, `sign_shared_funding` refuses `not_own_inputs` before
/// anything is saved or charged; nothing is out.
#[test]
fn test_QT_126_funding_never_signs_over_a_scriptsig_on_our_input() {
    let f = fx(3, true);
    let id = three_approved(&f);
    let mut forged = parse(&f.message(0, &id));
    let mut tx = decode_frozen(&forged).unwrap().tx;
    let b_coins = f.locked(1);
    for input in &mut tx.input {
        if b_coins.contains(&input.previous_output) {
            input.script_sig = dashcore::ScriptBuf::from_bytes(vec![0x51]);
        }
    }
    forged.protx = hex::encode(tx.to_bytes());
    // m-7 at the combine: the copy is refused, and the session stays where
    // it was, so the coordinator's real combined copy can still be taken.
    assert_funding(f.import(1, &forged.to_text()).unwrap_err(), "foreign_input");
    assert_ne!(f.session_of(1, &id).stage, SharedStage::Combined);
    assert!(f.row(1, &id).out_at.is_none());
}

/// fixes-1a M1 (D57): discarding reads the stored row, not the envelope. A
/// session whose envelope no longer parses is deleted and its coins
/// unlocked; one that is out stays, refused at its stage, locks kept.
#[test]
fn test_QT_126_an_unreadable_session_can_be_discarded() {
    let f = fx(2, true);
    let id = two_joined(&f);
    let corrupt = |w: usize| {
        let r = f.row(w, &id);
        let update = dw_appdb::SharedSessionUpdate {
            envelope: "{\"not\": \"an envelope\"".into(),
            my_shares: r.my_shares,
            contribution: r.contribution,
            approved_terms: r.approved_terms,
            signed_tx: None,
            txid: None,
            last_error: None,
            updated_at: r.updated_at,
        };
        assert!(
            f.db()
                .update_shared_session_durable(&f.wallets[w].to_string(), &id, &update)
                .unwrap()
        );
    };
    let discard = |w: usize| f.run(f.mn().discard_shared_session(f.wallets[w], id.clone()));

    assert!(!f.locked(0).is_empty());
    corrupt(0);
    assert!(
        f.run(f.mn().shared_session(f.wallets[0], id.clone()))
            .is_err(),
        "the envelope doesn't parse"
    );
    discard(0).unwrap();
    assert!(
        f.db()
            .shared_session(&f.wallets[0].to_string(), &id)
            .unwrap()
            .is_none()
    );
    assert!(f.locked(0).is_empty(), "its coins are unlocked");

    let held = f.locked(1);
    assert!(!held.is_empty());
    assert!(
        f.db()
            .mark_shared_session_out_durable(&f.wallets[1].to_string(), &id, &[7], "tx", 1)
            .unwrap()
    );
    corrupt(1);
    assert_stage(discard(1).unwrap_err(), "funding_signed");
    assert!(f.row(1, &id).out_at.is_some(), "an out row stays");
    assert_eq!(f.locked(1), held);
}

/// fixes-1a M2 (D56): consent binds to the copy the caller reviewed. A
/// lock on the revision before a reply absorbed since, and an approval of
/// the consent hash before a re-locked copy imported since, are
/// `shared_message{conflict}` and sign nothing; the current values sign.
#[test]
fn test_QT_126_consent_binds_to_the_reviewed_copy() {
    let f = fx(3, true);
    f.credit(0, 1, 401 * COIN);
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let inv = start(&f, 0, request(&[("A", 400), ("B", 300), ("C", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();
    assert_eq!(inv.session.consent_hash, None, "a draft has none");
    for w in [1, 2] {
        f.imported(w, &inv.message.text);
    }
    let (b, c) = (f.join(1, &id), f.join(2, &id));
    f.imported(0, &b.message.text);
    let reviewed = f.session_of(0, &id).revision;
    // D56 comes before the incomplete table (r2 nit 1).
    assert_message(f.lock_at(0, &id, reviewed - 1).unwrap_err(), "conflict");
    assert_stage(f.lock_at(0, &id, reviewed).unwrap_err(), "incomplete");
    f.imported(0, &c.message.text);
    assert_eq!(f.session_of(0, &id).revision, reviewed + 1);
    assert_message(f.lock_at(0, &id, reviewed).unwrap_err(), "conflict");
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Draft);
    let locked = f.lock_at(0, &id, reviewed + 1).unwrap();
    assert_eq!(
        locked.session.consent_hash.as_deref(),
        Some(parse(&locked.message.text).consent_hash.as_str())
    );

    let (s, _) = f.imported(1, &locked.message.text);
    let seen = s
        .consent_hash
        .expect("locked terms carry their consent hash");
    // M-1 case 2: a re-locked copy with other terms, imported concurrently.
    let mut relocked = parse(&locked.message.text);
    super::build::unfreeze(&mut relocked);
    relocked.terms.early_penalty += 1;
    super::build::freeze(&mut relocked).unwrap();
    let (s, _) = f.imported(1, &relocked.to_text());
    assert_eq!(
        (s.stage, s.next),
        (SharedStage::Signing, SharedNext::Approve)
    );
    assert_ne!(s.consent_hash.as_deref(), Some(seen.as_str()));
    assert_message(f.approve_with(1, &id, &seen).unwrap_err(), "conflict");
    assert!(
        parse(&f.message(1, &id)).signature_for(1).is_none(),
        "nothing signed"
    );
    // The hash is the record's, read ignoring case; not 64 hex is an
    // argument error before any session check (r2 nit 2).
    let current = s.consent_hash.clone().unwrap();
    assert_eq!(
        code(f.approve_with(1, &id, "zz").unwrap_err()),
        "invalid_argument"
    );
    let approval = parse(
        &f.approve_with(1, &id, &current.to_uppercase())
            .unwrap()
            .message
            .text,
    );
    assert!(approval.signature_for(1).is_some());
    assert_eq!(Some(approval.consent_hash), s.consent_hash);
    // Already approved: a stale hash is still `conflict`, not the Approval.
    assert_message(f.approve_with(1, &id, &seen).unwrap_err(), "conflict");
}

/// Review r2 m1: a row a call changed after the pass listed it isn't judged
/// by the older chain snapshot. A re-reservation onto a coin newer than the
/// snapshot would read as spent, and its live draft would lose its locks.
#[test]
fn test_QT_126_the_follow_pass_never_judges_a_row_newer_than_its_chain() {
    let f = fx(1, true);
    f.credit(0, 1, 500 * COIN + FEE);
    let inv = start(&f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let wallet = f.wallets[0];
    let rows = f.run(f.session.shared_rows(wallet)).unwrap();
    let chain = f.run(f.session.shared_chain(wallet)).unwrap();
    // After the snapshot: a new coin, and the fee change takes it.
    let newer = f.credit(0, 2, COIN);
    f.run(f.mn().set_shared_fee(wallet, id.clone(), 2 * FEE))
        .unwrap();
    let reserved = f.locked(0);
    assert!(reserved.contains(&newer));
    let listed = rows.iter().find(|r| r.id == id).unwrap();
    let released = f
        .run(f.session.follow_shared_row(wallet, listed, &chain))
        .unwrap();
    assert!(!released);
    assert_eq!(f.locked(0), reserved);
    // The next pass, with a fresh snapshot, keeps the live draft too.
    f.run(f.session.protx_follow_pass()).unwrap();
    assert_eq!(f.locked(0), reserved);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Draft);
}

/// fixes-1a M4: the follower's pass leaves a session a call holds in
/// flight for the next pass, which then releases it.
#[test]
fn test_QT_126_the_follow_pass_waits_for_a_session_in_flight() {
    let f = fx(1, true);
    f.credit(0, 1, 501 * COIN);
    let inv = start(&f, 0, request(&[("A", 500), ("B", 500)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let reserved = f.locked(0);
    f.spend(0, *reserved.iter().next().unwrap());
    assert!(matches!(
        f.session_of(0, &id).stage,
        SharedStage::Dead { .. }
    ));
    let flight = f.session.claim_shared_session(f.wallets[0], &id).unwrap();
    f.run(f.session.protx_follow_pass()).unwrap();
    assert_eq!(f.locked(0), reserved, "held in flight: kept this pass");
    drop(flight);
    f.run(f.session.protx_follow_pass()).unwrap();
    assert!(f.locked(0).is_empty());
    assert_eq!(f.row(0, &id).reserved, "[]");
}
