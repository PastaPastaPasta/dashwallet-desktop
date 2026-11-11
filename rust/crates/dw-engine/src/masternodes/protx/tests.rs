//! The ProTx owner flows end to end on a regtest session (MG-04 §7): a
//! `MasternodeOp` lease, the vault's `MasternodeOwner` signer, the dispatch
//! fence and `masternode_ops`, with the library's broadcast and the SPV list
//! faked through the hand-off's seam. The crash tests stop the hand-off at
//! each point between the save and the record step, restart the session on
//! the same data, and check that nothing is sent twice, nothing is lost and
//! no row whose transaction may be out is deleted.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::hashes::Hash;
use dashcore::{OutPoint, Transaction, TxOut, Txid};
use dw_protx::params::{EVONODE_COLLATERAL, MASTERNODE_COLLATERAL};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use key_wallet::transaction_checking::TransactionContext;
use platform_wallet::PlatformWalletError;
use zeroize::Zeroizing;

use super::ops::{CrashPoint, Seam};
use crate::masternodes::records::MasternodeType;
use crate::masternodes::{
    AttachedKeyRole, CollateralChoice, EvonodeTerms, KeepOr, KeyCheck, Masternodes,
    MasternodesError, OperatorChoice, OwnerKeySource, PayoutChoice, PlatformFacts,
    PlatformNodeChoice, ProTxOperation, ProTxOperationKind, ProTxOperationState,
    RegistrationRequest, UpdateRegistrarPreview, UpdateRegistrarRequest, VotingChoice,
};
use crate::send::test_support::{FOREIGN, pay, send_failure};
use crate::send::{SendFailure, SpendOwner, TestBroadcaster};
use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, WalletId};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const COIN: u64 = 100_000_000;
/// The external collateral's key (another wallet's), for the `External`
/// flow's signature.
const EXTERNAL_KEY: [u8; 32] = [0x5e; 32];

/// What the fake library does with a hand-off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Library {
    Accept,
    /// Definitely not sent (no peers), as dash-spv refuses before dispatch.
    NoPeers,
    /// Handed off without a verdict.
    Unconfirmed,
}

/// The library's broadcast: what it was handed, and whether the row was
/// already durable with `sent_at` when it was.
struct FakeLibrary {
    mode: Mutex<Library>,
    sent: Mutex<Vec<Transaction>>,
    saved_first: Mutex<Vec<bool>>,
    db: Mutex<Option<(Arc<dw_appdb::AppDb>, String)>>,
}

impl FakeLibrary {
    fn new(mode: Library) -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(mode),
            sent: Mutex::default(),
            saved_first: Mutex::default(),
            db: Mutex::default(),
        })
    }

    fn sent(&self) -> Vec<Transaction> {
        self.sent.lock().unwrap().clone()
    }

    fn distinct(&self) -> HashSet<Txid> {
        self.sent().iter().map(Transaction::txid).collect()
    }
}

impl TestBroadcaster for FakeLibrary {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn broadcast(
        &self,
        tx: &Transaction,
        _first: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        let txid = tx.txid();
        if let Some((db, wallet)) = self.db.lock().unwrap().as_ref() {
            let saved = db
                .masternode_ops(wallet)
                .unwrap()
                .iter()
                .any(|r| r.txid.as_deref() == Some(&txid.to_string()) && r.sent_at.is_some());
            self.saved_first.lock().unwrap().push(saved);
        }
        self.sent.lock().unwrap().push(tx.clone());
        let mode = *self.mode.lock().unwrap();
        Box::pin(async move {
            match mode {
                Library::Accept => Ok(txid),
                Library::NoPeers => Err(PlatformWalletError::TransactionBroadcast(
                    "no peers connected".into(),
                )),
                Library::Unconfirmed => Err(PlatformWalletError::TransactionBroadcastUnconfirmed(
                    "no verdict yet".into(),
                )),
            }
        })
    }
}

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<FakeLibrary>,
    seam: Arc<Seam>,
}

fn config(dir: &tempfile::TempDir, store: Arc<MemoryOsStore>) -> EngineConfig {
    EngineConfig {
        data_root: dir.path().join("data"),
        worker_threads: Some(2),
        vault: VaultConfig {
            kdf: KdfPolicy::Fixed(KdfParams::TEST),
            os_store: store,
            ..VaultConfig::default()
        },
    }
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

fn seam_for(session: &NetworkSession, lib: &Arc<FakeLibrary>) -> Arc<Seam> {
    let seam = Arc::new(Seam {
        broadcaster: lib.clone(),
        crash: Mutex::new(None),
        list: Mutex::new(Some(Vec::new())),
        list_height: Mutex::new(100),
        shared_mined: Mutex::default(),
    });
    session.masternodes.set_protx_seam(Some(seam.clone()));
    seam
}

struct NoEvents;

impl crate::events::EventSink for NoEvents {
    fn emit(&self, _: crate::events::EngineEvent) {}
}

fn fx(mode: Library) -> Fx {
    let dir = dw_testutil::private_tempdir();
    let store = Arc::new(MemoryOsStore::new());
    let engine = Engine::new(config(&dir, store), Arc::new(NoEvents)).unwrap();
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    let lib = FakeLibrary::new(mode);
    let seam = seam_for(&session, &lib);
    let f = Fx {
        dir,
        engine,
        session,
        wallet,
        lib,
        seam,
    };
    f.watch_rows();
    f
}

impl Fx {
    fn mn(&self) -> Masternodes {
        self.session.masternodes()
    }

    fn db(&self) -> Arc<dw_appdb::AppDb> {
        self.session.live().unwrap().appdb
    }

    fn watch_rows(&self) {
        *self.lib.db.lock().unwrap() = Some((self.db(), self.wallet.to_string()));
    }

    /// Closes the session and opens it again on the same data, as a crash
    /// and restart would leave it: memory (reservations, leases, in-flight
    /// marks) gone, app.sqlite kept. The coins are credited again, as the
    /// wallet's sync would restore them.
    fn restart(&mut self, coins: &[(u8, u64)]) {
        assert!(
            self.engine
                .block_on(self.engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        self.session = open(&self.engine);
        self.seam = seam_for(&self.session, &self.lib);
        self.watch_rows();
        for (seed, value) in coins {
            self.credit(*seed, *value);
        }
    }

    fn grant(&self, max_duffs: u64) -> String {
        self.authorize(GrantPurpose::MasternodeOp { max_duffs })
    }

    fn spend_grant(&self, max_duffs: u64) -> String {
        self.authorize(GrantPurpose::Spend { max_duffs })
    }

    fn authorize(&self, purpose: GrantPurpose) -> String {
        self.session
            .vault()
            .authorize(purpose, Some(&self.wallet.0), Credential::None)
            .unwrap()
            .id
    }

    /// A confirmed coin `seed` of `value` at a fresh BIP44 receive address
    /// (the same outpoint for the same seed after a restart), its funding
    /// transaction in the history.
    fn credit(&self, seed: u8, value: u64) -> OutPoint {
        let outpoint = self.credit_unseen(seed, value);
        (self.session.hub.history).hold_funding(self.wallet, outpoint.txid);
        outpoint
    }

    /// [`Self::credit`] before its transaction's event reaches the history.
    fn credit_unseen(&self, seed: u8, value: u64) -> OutPoint {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine.block_on(async move {
            session
                .on_runtime(async move {
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
                })
                .await
                .unwrap()
        })
    }

    /// The wallet no longer holds `outpoint` (spent elsewhere).
    fn spend(&self, outpoint: OutPoint) {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
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

    /// The wallet holds `outpoint` paying `txout` (its own script).
    fn hold(&self, outpoint: OutPoint, txout: TxOut) {
        self.hold_in(self.wallet, outpoint, txout);
    }

    fn hold_in(&self, id: WalletId, outpoint: OutPoint, txout: TxOut) {
        let session = Arc::clone(&self.session);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                let address = dashcore::Address::from_script(
                    &txout.script_pubkey,
                    dashcore::Network::Regtest,
                )
                .unwrap();
                let mut utxo = Utxo::new(outpoint, txout, address, 2, false);
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
                Ok(())
            }))
            .unwrap();
    }

    fn tip(&self, height: u32) {
        self.session.hub.tracker().set_tip_height(height);
    }

    /// `tx` in a block of the wallet's history, as key-wallet records it.
    fn confirm(&self, tx: Transaction, height: u32) {
        self.confirm_in(self.wallet, tx, height);
    }

    fn confirm_in(&self, wallet: WalletId, tx: Transaction, height: u32) {
        self.record_in(wallet, tx, chain_locked(height));
    }

    /// `tx` in the wallet's history in `context`.
    fn record(&self, tx: Transaction, context: TransactionContext) {
        self.record_in(self.wallet, tx, context);
    }

    fn record_in(&self, wallet: WalletId, tx: Transaction, context: TransactionContext) {
        let record = tx_record(tx, context);
        (self.session.hub.history).with_wallet(wallet, |h| h.upsert(&record, None));
    }

    fn follow(&self) {
        self.engine
            .block_on(self.session.protx_follow_pass())
            .unwrap();
    }

    fn locked(&self) -> HashSet<OutPoint> {
        self.locked_in(self.wallet)
    }

    fn locked_in(&self, wallet: WalletId) -> HashSet<OutPoint> {
        self.db()
            .locks(&wallet.to_string())
            .unwrap()
            .into_iter()
            .filter(|r| r.is_locked())
            .map(|r| OutPoint::new(r.txid.parse().unwrap(), r.vout))
            .collect()
    }

    fn rows(&self) -> Vec<dw_appdb::MasternodeOpRow> {
        self.db().masternode_ops(&self.wallet.to_string()).unwrap()
    }

    fn operations(&self) -> Vec<ProTxOperation> {
        self.engine
            .block_on(self.mn().operations(self.wallet))
            .unwrap()
    }

    /// Loads a second wallet, as an attached owner key can update from any
    /// loaded wallet (D24); returns its id.
    fn import_other_wallet(&self) -> WalletId {
        self.engine
            .block_on(self.session.import_wallet(
                Zeroizing::new(b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong".to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap()
    }

    /// Wallet `other`'s copy of update `id`, saved `offset` seconds after
    /// it, `sent` or else `not_sent`, as an attached owner key saves one
    /// there (D24).
    fn other_wallets_update(
        &self,
        other: WalletId,
        id: &str,
        new_id: &str,
        offset: i64,
        sent: bool,
    ) {
        let mut row = self.rows().into_iter().find(|r| r.id == id).unwrap();
        row.id = new_id.into();
        row.created_at = row.created_at.checked_add_signed(offset).unwrap();
        row.sent_at = None;
        row.last_error = Some("send.no_peers".into());
        let (db, w) = (self.db(), other.to_string());
        db.insert_masternode_op_durable(&w, &row, &[], 1).unwrap();
        if sent {
            db.begin_masternode_op_attempt(&w, new_id, row.created_at)
                .unwrap()
                .unwrap();
        }
    }

    fn register(
        &self,
        request: RegistrationRequest,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.engine
            .block_on(self.mn().register(self.wallet, request, grant))
    }

    fn fee_of(&self, request: &RegistrationRequest) -> u64 {
        self.engine
            .block_on(self.mn().preview_registration(self.wallet, request.clone()))
            .unwrap()
            .fee
    }

    fn crash_at(&self, point: Option<CrashPoint>) {
        *self.seam.crash.lock().unwrap() = point;
    }
}

fn request(collateral: CollateralChoice) -> RegistrationRequest {
    RegistrationRequest {
        kind: MasternodeType::Regular,
        collateral,
        service: "1.2.3.4".into(),
        operator: OperatorChoice::NextUnused,
        operator_reward: 0,
        owner_index: None,
        voting: VotingChoice::NextUnused,
        payout: PayoutChoice::NewAddress,
        evonode: None,
    }
}

/// An evonode registration of `collateral` with `node` and the network's
/// default ports.
fn evonode(collateral: CollateralChoice, node: PlatformNodeChoice) -> RegistrationRequest {
    RegistrationRequest {
        kind: MasternodeType::Evonode,
        evonode: Some(EvonodeTerms {
            node,
            p2p_port: None,
            http_port: None,
        }),
        ..request(collateral)
    }
}

/// The `send.*` code a facade error carries (§6).
fn send_code(e: &MasternodesError) -> String {
    match e.engine() {
        Some(e @ crate::EngineError::Send(_)) => super::ops::error_code(e),
        _ => panic!("not a send failure: {e:?}"),
    }
}

fn payload(tx: &Transaction) -> &dashcore::blockdata::transaction::special_transaction::provider_registration::ProviderRegistrationPayload{
    match &tx.special_transaction_payload {
        Some(TransactionPayload::ProviderRegistrationPayloadType(p)) => p,
        _ => panic!("not a ProRegTx"),
    }
}

#[test]
fn mg04_fund_new_is_charged_its_fee_and_saved_before_the_hand_off() {
    let f = fx(Library::Accept);
    let funding = f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let req = request(CollateralChoice::FundNew);
    let fee = f.fee_of(&req);
    assert!(fee > 0 && fee < 10_000, "{fee}");

    let short = f.register(req.clone(), f.grant(fee - 1)).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    assert!(
        f.rows().is_empty() && f.lib.sent().is_empty(),
        "nothing signed"
    );

    let op = f.register(req.clone(), f.grant(fee)).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(op.fee, fee);
    let sent = f.lib.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        *f.lib.saved_first.lock().unwrap(),
        vec![true],
        "the row was durable first"
    );
    let tx = &sent[0];
    let p = payload(tx);
    assert_eq!(p.version, 2, "D1");
    let collateral = &tx.output[p.collateral_outpoint.vout as usize];
    assert_eq!(collateral.value, MASTERNODE_COLLATERAL);
    assert_eq!(
        op.pro_tx_hash.as_deref(),
        Some(tx.txid().to_string().as_str())
    );
    assert_eq!(tx.input[0].previous_output, funding);
    assert_eq!(
        (tx.version, tx.lock_time, tx.input[0].sequence),
        (3, 0, 0xffff_ffff),
        "D25: as the wallet's ordinary send"
    );
    assert!(p.signature.is_empty());
    // The inputs and the new collateral stay locked (D9, D25).
    let collateral_outpoint = OutPoint::new(tx.txid(), p.collateral_outpoint.vout);
    assert_eq!(f.locked(), HashSet::from([funding, collateral_outpoint]));
    assert_eq!(
        op.terms.collateral_outpoint,
        Some(format!("{}-{}", tx.txid(), p.collateral_outpoint.vout))
    );
    // The keys are the keychain's first, and now in use: the next
    // registration's defaults move on (D13).
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    assert_eq!(
        (keys.owner.index, keys.voting.index, keys.operator.index),
        (1, 1, 1)
    );
    assert_eq!(op.terms.owner_index, Some(0));
    assert_eq!(op.terms.operator_index, Some(0));
}

#[test]
fn mg04_existing_collateral_signs_cores_message_and_stays_locked() {
    let f = fx(Library::Accept);
    let collateral = f.credit(1, MASTERNODE_COLLATERAL);
    let fee_coin = f.credit(2, COIN);
    let wrong = f.credit(3, MASTERNODE_COLLATERAL - 1);
    let refused = f
        .register(
            request(CollateralChoice::ExistingUtxo {
                outpoint: format!("{}-0", wrong.txid),
            }),
            f.grant(COIN),
        )
        .unwrap_err();
    assert_eq!(refused.code(), "masternodes.collateral_unavailable");

    let req = request(CollateralChoice::ExistingUtxo {
        outpoint: format!("{}-0", collateral.txid),
    });
    let op = f.register(req.clone(), f.grant(COIN)).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    let tx = f.lib.sent().pop().unwrap();
    let p = payload(&tx);
    assert_eq!(p.collateral_outpoint, collateral);
    assert!(
        tx.input.iter().all(|i| i.previous_output != collateral),
        "never spent"
    );
    // Largest first: the short coin pays the fee.
    assert_eq!(tx.input.len(), 1);
    assert_eq!(tx.input[0].previous_output, wrong);
    // The rules re-check the vault's signature over Core's message.
    let script = f.engine.block_on(async {
        let w = f.session.wallet(&f.wallet).await.unwrap();
        let state = w.state().await;
        crate::coins::read_coins(&state.core_wallet)
            .coins
            .into_iter()
            .find(|c| c.utxo.outpoint == collateral)
            .unwrap()
            .utxo
            .txout
            .script_pubkey
    });
    dw_protx::rules::check_registration(
        &tx,
        p,
        Some((&script, Some(MASTERNODE_COLLATERAL))),
        &[],
        dashcore::Network::Regtest,
    )
    .unwrap();
    assert!(f.locked().contains(&collateral), "D9");
    assert!(f.locked().contains(&wrong), "D25");
    assert!(!f.locked().contains(&fee_coin));
    // In use now: another registration of it is refused (D23).
    let again = f.register(req, f.grant(COIN)).unwrap_err();
    assert_eq!(again.code(), "masternodes.collateral_unavailable");
}

#[test]
fn mg04_not_sent_keeps_the_row_and_discard_releases_it() {
    let f = fx(Library::NoPeers);
    let funding = f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::NotSent);
    assert_eq!(op.sent_at, None);
    assert_eq!(op.last_error.as_deref(), Some("send.no_peers"));
    assert!(f.locked().contains(&funding), "kept for a resend (D25)");
    let collateral = OutPoint::new(
        op.txid.as_deref().unwrap().parse().unwrap(),
        payload(&f.lib.sent()[0]).collateral_outpoint.vout,
    );
    assert_eq!(
        f.locked(),
        HashSet::from([funding, collateral]),
        "the new collateral is locked with the row (D53)"
    );

    f.engine
        .block_on(f.mn().discard_operation(f.wallet, op.id.clone()))
        .unwrap();
    assert!(f.rows().is_empty());
    assert!(f.locked().is_empty());
    let gone = f
        .engine
        .block_on(f.mn().discard_operation(f.wallet, op.id))
        .unwrap_err();
    assert_eq!(gone.code(), "masternodes.operation_not_found");
}

/// D53 (review r1 m-1): a new collateral is locked in the write that
/// saves its row, so neither a resend after a first `not_sent` nor a crash
/// between the hand-off and the record leaves it to the follower: an
/// ordinary send never counts it, though the wallet holds it.
#[test]
fn mg04_a_new_collateral_is_locked_with_its_row_before_any_hand_off() {
    let ordinary = |f: &Fx| {
        f.engine
            .block_on(f.session.max_spendable(
                f.wallet,
                crate::send::CoinSource::Any,
                crate::send::FeeMode::PerKb(1_000),
            ))
            .unwrap()
    };
    let new_collateral = |tx: &Transaction| {
        let vout = payload(tx).collateral_outpoint.vout;
        (
            OutPoint::new(tx.txid(), vout),
            tx.output[vout as usize].clone(),
        )
    };

    // A first hand-off with no peers, then a resend that goes out.
    let f = fx(Library::NoPeers);
    let funding = f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::NotSent);
    *f.lib.mode.lock().unwrap() = Library::Accept;
    f.engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, op.id.clone(), f.grant(op.fee)),
        )
        .unwrap();
    let (collateral, txout) = new_collateral(&f.lib.sent()[1]);
    // Its outputs reach the wallet (IS-locked) before any follower pass.
    f.spend(funding);
    f.hold(collateral, txout.clone());
    f.credit(2, COIN);
    assert!(f.locked().contains(&collateral));
    assert_eq!(ordinary(&f), COIN, "the other coin only");

    // A crash after the hand-off, before its outcome is recorded.
    let coins = [(1u8, MASTERNODE_COLLATERAL + COIN), (2u8, COIN)];
    let mut f = fx(Library::Accept);
    f.credit(1, coins[0].1);
    f.crash_at(Some(CrashPoint::HandedOff));
    assert!(
        f.register(request(CollateralChoice::FundNew), f.grant(COIN))
            .is_err()
    );
    let (collateral, txout) = new_collateral(&f.lib.sent()[0]);
    f.restart(&coins);
    f.crash_at(None);
    f.spend(OutPoint::new(Txid::from_byte_array([1; 32]), 0));
    f.hold(collateral, txout);
    assert_eq!(f.operations()[0].state, ProTxOperationState::Sent);
    assert!(f.locked().contains(&collateral));
    assert_eq!(ordinary(&f), COIN, "the other coin only");
    drop(f.dir);
}

#[test]
fn mg04_maybe_sent_stays_sent_and_is_never_discarded() {
    let f = fx(Library::Unconfirmed);
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(op.last_error.as_deref(), Some("send.broadcast_unknown"));
    let refused = f
        .engine
        .block_on(f.mn().discard_operation(f.wallet, op.id.clone()))
        .unwrap_err();
    assert_eq!(refused.code(), "masternodes.operation_state");
    assert_eq!(f.rows().len(), 1);
}

#[test]
fn mg04_a_resend_reserves_its_inputs_against_other_owners() {
    let f = fx(Library::NoPeers);
    let funding = f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::NotSent);

    let other = SpendOwner::Tx(Txid::from_byte_array([0x99; 32]));
    f.session.spends.reserve(f.wallet, other, [funding]);
    let held = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, op.id.clone(), f.grant(COIN)),
        )
        .unwrap_err();
    assert!(matches!(
        held.engine(),
        Some(crate::EngineError::Send(SendFailure::OutpointUnavailable(o))) if *o == funding
    ));
    assert_eq!(f.lib.sent().len(), 1, "nothing handed off");
    f.session.spends.release(&f.wallet, other);

    *f.lib.mode.lock().unwrap() = Library::Accept;
    let short = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, op.id.clone(), f.grant(op.fee - 1)),
        )
        .unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    let sent = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, op.id.clone(), f.grant(op.fee)),
        )
        .unwrap();
    assert_eq!(sent.state, ProTxOperationState::Sent);
    assert_eq!(f.lib.distinct().len(), 1, "the same bytes");
    assert_eq!(f.lib.sent().len(), 2);
}

#[test]
fn mg04_one_call_at_a_time_per_operation() {
    let f = fx(Library::NoPeers);
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    let held = f.session.claim_operation(f.wallet, &op.id).unwrap();
    for e in [
        f.engine
            .block_on(f.mn().discard_operation(f.wallet, op.id.clone()))
            .unwrap_err(),
        f.engine
            .block_on(
                f.mn()
                    .resend_operation(f.wallet, op.id.clone(), f.grant(COIN)),
            )
            .unwrap_err(),
    ] {
        assert_eq!(e.code(), "masternodes.in_flight");
    }
    drop(held);
    f.engine
        .block_on(f.mn().discard_operation(f.wallet, op.id))
        .unwrap();
}

/// The manager's slice-2 addition: a crash at each point between the save
/// and the record step, then a restart.
#[test]
fn mg04_a_crash_between_save_and_record_loses_nothing_and_sends_once() {
    let coins = [
        (1u8, MASTERNODE_COLLATERAL + COIN),
        (2u8, MASTERNODE_COLLATERAL + COIN),
    ];
    for point in [CrashPoint::Signed, CrashPoint::Saved, CrashPoint::HandedOff] {
        let mut f = fx(Library::Accept);
        for (seed, value) in coins {
            f.credit(seed, value);
        }
        f.crash_at(Some(point));
        let crashed = f.register(request(CollateralChoice::FundNew), f.grant(COIN));
        assert!(crashed.is_err(), "{point:?}");
        let handed = f.lib.sent();
        f.restart(&coins);
        f.crash_at(None);

        let ops = f.operations();
        match point {
            CrashPoint::Signed => {
                assert!(ops.is_empty(), "nothing saved, nothing out");
                assert!(handed.is_empty());
                assert!(f.locked().is_empty(), "no lock without its row");
            }
            CrashPoint::Saved => {
                assert_eq!(ops.len(), 1);
                assert_eq!(ops[0].state, ProTxOperationState::Signed);
                assert!(handed.is_empty());
            }
            CrashPoint::HandedOff => {
                assert_eq!(ops.len(), 1);
                assert_eq!(ops[0].state, ProTxOperationState::Sent, "may be out");
                assert_eq!(handed.len(), 1);
                let refused = f
                    .engine
                    .block_on(f.mn().discard_operation(f.wallet, ops[0].id.clone()))
                    .unwrap_err();
                assert_eq!(refused.code(), "masternodes.operation_state");
                assert_eq!(f.rows().len(), 1, "never deleted");
            }
            CrashPoint::SharedOut
            | CrashPoint::Settle
            | CrashPoint::SharedFollow
            | CrashPoint::LockStep => unreachable!("not a registration point"),
        }
        // The saved inputs stay locked across the restart: a new
        // registration can't spend them in a second transaction.
        let first_inputs: HashSet<OutPoint> = f
            .rows()
            .iter()
            .filter_map(super::ops::signed_tx)
            .flat_map(|tx| tx.input.into_iter().map(|i| i.previous_output))
            .collect();
        // Its own service: the first's may be out (D51).
        let other_service = RegistrationRequest {
            service: "1.2.3.5".into(),
            ..request(CollateralChoice::FundNew)
        };
        let second = f.register(other_service, f.grant(COIN)).unwrap();
        let second_tx = f.lib.sent().pop().unwrap();
        assert_eq!(second.txid, Some(second_tx.txid().to_string()));
        assert!(
            second_tx
                .input
                .iter()
                .all(|i| !first_inputs.contains(&i.previous_output)),
            "{point:?}: a second spend of the first registration's inputs"
        );
        // A resend of the first hands out the same bytes, once more.
        if let Some(first) = f.operations().into_iter().find(|o| o.id != second.id) {
            let resent = f
                .engine
                .block_on(
                    f.mn()
                        .resend_operation(f.wallet, first.id.clone(), f.grant(COIN)),
                )
                .unwrap();
            assert_eq!(resent.state, ProTxOperationState::Sent);
            let txids: HashSet<Txid> = f
                .lib
                .sent()
                .iter()
                .filter(|t| t.txid() != second_tx.txid())
                .map(Transaction::txid)
                .collect();
            assert_eq!(
                txids.len(),
                1,
                "{point:?}: one transaction for the first row"
            );
        }
        drop(f.dir);
    }
}

#[test]
fn mg04_external_prepare_survives_a_restart_and_refreshes_when_spent() {
    let mut f = fx(Library::Accept);
    let fee_coin = f.credit(1, COIN);
    let other = f.credit(2, COIN);
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let address = dashcore::Address::p2pkh(&public, dashcore::Network::Regtest);
    let outpoint = OutPoint::new(Txid::from_byte_array([0x77; 32]), 1);
    let req = request(CollateralChoice::External {
        outpoint: format!("{}-1", outpoint.txid),
        address: address.to_string(),
    });
    let refused = f.register(req.clone(), f.grant(COIN)).unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");

    let op = f
        .engine
        .block_on(f.mn().prepare_external_registration(f.wallet, req.clone()))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::NeedsSignature);
    let ask = op.sign_request.clone().unwrap();
    assert_eq!(ask.address, address.to_string());
    let row = f.rows().pop().unwrap();
    let unsigned: Transaction =
        dashcore::consensus::deserialize(row.unsigned_tx.as_ref().unwrap()).unwrap();
    assert_eq!(
        ask.message,
        dw_protx::payloads::registration_sign_message(
            payload(&unsigned),
            dashcore::Network::Regtest
        )
        .unwrap(),
        "Core's MakeSignString of the prepared payload"
    );
    let first_input = unsigned.input[0].previous_output;
    assert!(f.locked().contains(&first_input), "D8");

    // A restart keeps the row and its lock.
    f.restart(&[(1, COIN), (2, COIN)]);
    assert_eq!(f.operations()[0].state, ProTxOperationState::NeedsSignature);
    assert!(f.locked().contains(&first_input));

    // Spent elsewhere: `inputs_spent`; a refresh plans anew.
    f.spend(first_input);
    let stale = f.operations().pop().unwrap();
    assert_eq!(stale.state, ProTxOperationState::InputsSpent);
    assert!(stale.sign_request.is_none());
    let submit_stale = f
        .engine
        .block_on(f.mn().submit_external_registration(
            f.wallet,
            op.id.clone(),
            "AA==".into(),
            f.grant(COIN),
        ))
        .unwrap_err();
    assert_eq!(submit_stale.code(), "masternodes.operation_state");
    let fresh = f
        .engine
        .block_on(
            f.mn()
                .refresh_external_registration(f.wallet, op.id.clone()),
        )
        .unwrap();
    assert_eq!(fresh.state, ProTxOperationState::NeedsSignature);
    let ask = fresh.sign_request.unwrap();
    assert_ne!(ask.message, op.sign_request.unwrap().message);
    assert_eq!(fresh.terms, op.terms, "the same terms");
    let row = f.rows().pop().unwrap();
    let refreshed: Transaction =
        dashcore::consensus::deserialize(row.unsigned_tx.as_ref().unwrap()).unwrap();
    assert!(
        refreshed
            .input
            .iter()
            .all(|i| f.locked().contains(&i.previous_output)),
        "the new inputs are locked with the row (D53)"
    );
    let _ = (fee_coin, other);

    // A signature by another key, then by the collateral's.
    let wrong = dw_protx::payloads::sign_message_compact(&[0x11; 32], &ask.message).unwrap();
    let b64 = |sig: [u8; 65]| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(sig)
    };
    let mismatch = f
        .engine
        .block_on(f.mn().submit_external_registration(
            f.wallet,
            op.id.clone(),
            b64(wrong),
            f.grant(COIN),
        ))
        .unwrap_err();
    assert_eq!(mismatch.code(), "masternodes.signature_mismatch");
    let right = dw_protx::payloads::sign_message_compact(&EXTERNAL_KEY, &ask.message).unwrap();
    let done = f
        .engine
        .block_on(f.mn().submit_external_registration(
            f.wallet,
            op.id.clone(),
            b64(right),
            f.grant(COIN),
        ))
        .unwrap();
    assert_eq!(done.state, ProTxOperationState::Sent);
    let tx = f.lib.sent().pop().unwrap();
    let p = payload(&tx);
    assert_eq!(p.collateral_outpoint, outpoint);
    assert_eq!(p.signature, right.to_vec());
}

#[test]
fn mg04_requests_are_refused_offline_first() {
    let f = fx(Library::Accept);
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let preview = |req: RegistrationRequest| {
        f.engine
            .block_on(f.mn().preview_registration(f.wallet, req))
            .unwrap_err()
    };
    let mut v6 = request(CollateralChoice::FundNew);
    v6.service = "[2001:db8::1]:19899".into();
    let e = preview(v6);
    assert!(
        matches!(&e, MasternodesError::Unsupported { feature, field } if feature == "protx_v3" && field.as_deref() == Some("service")),
        "{e:?}"
    );
    let mut no_service = request(CollateralChoice::FundNew);
    no_service.service = String::new();
    assert_eq!(preview(no_service).code(), "invalid_argument");
    let mut reward = request(CollateralChoice::FundNew);
    reward.operator_reward = 10_001;
    assert_eq!(preview(reward).code(), "invalid_argument");
    let mut secret = request(CollateralChoice::FundNew);
    secret.operator = OperatorChoice::Given {
        public_key: "ab".repeat(32),
    };
    let e = preview(secret);
    assert!(e.to_string().contains("secret") && !e.to_string().contains(&"ab".repeat(32)));
    let mut port = request(CollateralChoice::FundNew);
    port.service = "1.2.3.4:9999".into();
    assert!(
        matches!(preview(port), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-netinfo-port")
    );

    // A listed operator key is Core's duplicate (D5).
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    let mut operator = [0u8; 48];
    hex::decode_to_slice(keys.operator.public_key.as_ref().unwrap(), &mut operator).unwrap();
    *f.seam.list.lock().unwrap() = Some(vec![platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: [9; 32],
        service_address: Some("1.2.3.5:19899".parse().unwrap()),
        platform_http_port: None,
        operator_public_key: operator,
        voting_key_id: [9; 20],
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: false,
        net_info: None,
    }]);
    let mut given = request(CollateralChoice::FundNew);
    given.operator = OperatorChoice::Given {
        public_key: hex::encode(operator),
    };
    assert!(
        matches!(preview(given), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-key")
    );
    let mut dup_service = request(CollateralChoice::FundNew);
    dup_service.service = "1.2.3.5".into();
    assert!(
        matches!(preview(dup_service), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-netinfo-entry")
    );
    // MG-04b D39: any net-info entry of a listed masternode, here another
    // version-3 masternode's second Core P2P entry; a persisted summary,
    // which carries no entries, compares its primary service only.
    let second = |net_info| platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: [8; 32],
        service_address: Some("1.2.3.7:19899".parse().unwrap()),
        platform_http_port: None,
        operator_public_key: [8; 48],
        voting_key_id: [8; 20],
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: true,
        net_info,
    };
    let ext = {
        use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
        use dashcore::sml::masternode_list_entry::net_info::{
            Bip155Network, ExtNetInfo, NetInfoEntry, NetInfoPurpose,
        };
        let ipv4 = |last| NetInfoEntry::Service {
            network: Bip155Network::Ipv4,
            addr: vec![1, 2, 3, last],
            port: 19899,
        };
        MasternodeNetInfo::Extended(ExtNetInfo {
            version: 1,
            purposes: vec![(NetInfoPurpose::CoreP2P, vec![ipv4(7), ipv4(6)])],
        })
    };
    let with_service = |service: &str| {
        let mut r = request(CollateralChoice::FundNew);
        r.service = service.into();
        r
    };
    let listed = f.seam.list.lock().unwrap().clone();
    *f.seam.list.lock().unwrap() = Some(vec![second(Some(ext))]);
    assert!(
        matches!(preview(with_service("1.2.3.6")), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-netinfo-entry")
    );
    *f.seam.list.lock().unwrap() = Some(vec![second(None)]);
    f.engine
        .block_on(
            f.mn()
                .preview_registration(f.wallet, with_service("1.2.3.6")),
        )
        .unwrap();
    *f.seam.list.lock().unwrap() = listed;
    // NextUnused skips the listed key.
    let ok = f
        .engine
        .block_on(
            f.mn()
                .preview_registration(f.wallet, request(CollateralChoice::FundNew)),
        )
        .unwrap();
    assert_eq!(ok.terms.operator_index, Some(1));

    *f.seam.list.lock().unwrap() = None;
    let e = f
        .engine
        .block_on(
            f.mn()
                .preview_registration(f.wallet, request(CollateralChoice::FundNew)),
        )
        .unwrap_err();
    assert_eq!(e.code(), "masternodes.list_unavailable");
}

/// D25: a ProRegTx in a block releases its inputs' locks; D9: the
/// collateral stays locked, the user may release it, and the follower
/// relocks it unless released.
#[test]
fn mg04_a_confirmed_registration_settles_and_its_collateral_lock_follows_the_user() {
    let f = fx(Library::Accept);
    let funding = f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let op = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    let tx = f.lib.sent().pop().unwrap();
    let vout = payload(&tx).collateral_outpoint.vout;
    let collateral = OutPoint::new(tx.txid(), vout);
    f.spend(funding);
    f.hold(collateral, tx.output[vout as usize].clone());
    f.confirm(tx.clone(), 7);

    let ops = f.operations();
    assert_eq!(ops[0].state, ProTxOperationState::Confirmed { height: 7 });
    assert_eq!(f.locked(), HashSet::from([collateral]), "inputs settled");
    let hash = op.pro_tx_hash.unwrap();

    f.engine
        .block_on(f.mn().release_collateral(f.wallet, hash.clone()))
        .unwrap();
    assert!(f.locked().is_empty());
    f.follow();
    assert!(f.locked().is_empty(), "a release holds");

    f.engine
        .block_on(f.mn().lock_collateral(f.wallet, hash.clone()))
        .unwrap();
    assert_eq!(f.locked(), HashSet::from([collateral]));
    // Unlocked by coin control without a release: the follower relocks.
    f.db()
        .unlock(
            &f.wallet.to_string(),
            &collateral.txid.to_string(),
            collateral.vout,
            1,
        )
        .unwrap();
    assert!(f.locked().is_empty());
    f.follow();
    assert_eq!(f.locked(), HashSet::from([collateral]));
    // A coin a row holds stays locked through a release (MG-04b D69).
    let session = dw_appdb::SharedSessionRow {
        id: "ab".repeat(32),
        kind: "registration".into(),
        role: "coordinator".into(),
        envelope: "{}".into(),
        my_shares: "{}".into(),
        contribution: None,
        reserved: format!("[\"{collateral}\"]"),
        approved_terms: None,
        base_keys: None,
        signed_tx: None,
        txid: None,
        out_at: None,
        abandon_txid: None,
        created_at: 1,
        updated_at: 1,
        last_error: None,
    };
    (f.db())
        .insert_shared_session_durable(&f.wallet.to_string(), &session, 1)
        .unwrap();
    f.engine
        .block_on(f.mn().release_collateral(f.wallet, hash.clone()))
        .unwrap();
    assert_eq!(f.locked(), HashSet::from([collateral]));

    let unknown = f
        .engine
        .block_on(f.mn().release_collateral(f.wallet, "11".repeat(32)))
        .unwrap_err();
    assert_eq!(unknown.code(), "masternodes.not_found");
}

/// The payload's evonode fields as the terms show them.
fn platform_of(tx: &Transaction) -> Option<PlatformFacts> {
    let p = payload(tx);
    assert_eq!(p.masternode_type as u16 == 1, p.platform_node_id.is_some());
    Some(PlatformFacts {
        node_id: hex::encode(&p.platform_node_id?[..]),
        p2p_port: p.platform_p2p_port?,
        http_port: p.platform_http_port?,
    })
}

/// D18: an evonode's `FundNew` funds 4,000 DASH, and its platform fields
/// are the keychain's next node id and the network's default ports.
#[test]
fn mg04_evonode_fund_new_carries_the_keychains_node_and_default_ports() {
    let f = fx(Library::Accept);
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Evonode))
        .unwrap();
    let node = keys.platform_node.clone().expect("an evonode's node key");
    assert_eq!(node.index, 0);
    let regular = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    assert!(regular.platform_node.is_none());

    // A regular collateral's worth doesn't fund an evonode.
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    let req = evonode(CollateralChoice::FundNew, PlatformNodeChoice::NextUnused);
    let short = f
        .engine
        .block_on(f.mn().preview_registration(f.wallet, req.clone()))
        .unwrap_err();
    assert_eq!(send_code(&short), "send.amount_exceeds_balance");

    f.credit(2, EVONODE_COLLATERAL);
    let preview = f
        .engine
        .block_on(f.mn().preview_registration(f.wallet, req.clone()))
        .unwrap();
    assert_eq!(preview.collateral_amount, EVONODE_COLLATERAL);
    let op = f.register(req, f.grant(preview.fee)).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    let tx = f.lib.sent().pop().unwrap();
    let p = payload(&tx);
    assert_eq!((p.version, p.masternode_type as u16), (2, 1), "D1");
    let collateral = &tx.output[p.collateral_outpoint.vout as usize];
    assert_eq!(collateral.value, EVONODE_COLLATERAL);
    let facts = PlatformFacts {
        node_id: node.node_id.clone().unwrap(),
        p2p_port: 22_200,
        http_port: 22_201,
    };
    assert_eq!(
        platform_of(&tx),
        Some(facts.clone()),
        "chainparams' regtest"
    );
    assert_eq!(op.terms.platform, Some(facts));
    assert_eq!(op.terms.platform_node_index, Some(0));
    assert_eq!(op.terms.masternode_type, MasternodeType::Evonode);
    dw_protx::rules::check_registration(&tx, p, None, &[], dashcore::Network::Regtest).unwrap();
    // The node key is in use now: the next default moves on (D13).
    let next = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Evonode))
        .unwrap();
    assert_eq!(next.platform_node.unwrap().index, 1);
}

/// An existing evonode collateral is exactly 4,000 DASH; the node id and
/// ports given are carried as given.
#[test]
fn mg04_evonode_existing_collateral_carries_the_fields_given() {
    let f = fx(Library::Accept);
    let regular = f.credit(1, MASTERNODE_COLLATERAL);
    let collateral = f.credit(2, EVONODE_COLLATERAL);
    f.credit(3, COIN);
    let node_id = "ab".repeat(20);
    let given = |outpoint: OutPoint| RegistrationRequest {
        evonode: Some(EvonodeTerms {
            node: PlatformNodeChoice::Given {
                node_id: node_id.clone(),
            },
            p2p_port: Some(26_000),
            http_port: Some(26_001),
        }),
        ..evonode(
            CollateralChoice::ExistingUtxo {
                outpoint: format!("{}-0", outpoint.txid),
            },
            PlatformNodeChoice::NextUnused,
        )
    };
    let refused = f.register(given(regular), f.grant(COIN)).unwrap_err();
    assert_eq!(refused.code(), "masternodes.collateral_unavailable");

    let op = f.register(given(collateral), f.grant(COIN)).unwrap();
    let tx = f.lib.sent().pop().unwrap();
    let p = payload(&tx);
    assert_eq!(p.collateral_outpoint, collateral);
    assert!(tx.input.iter().all(|i| i.previous_output != collateral));
    let facts = PlatformFacts {
        node_id: node_id.clone(),
        p2p_port: 26_000,
        http_port: 26_001,
    };
    assert_eq!(platform_of(&tx), Some(facts.clone()));
    assert_eq!(op.terms.platform, Some(facts));
    assert_eq!(
        op.terms.platform_node_index, None,
        "given, not the keychain's"
    );
    let script = f.engine.block_on(async {
        let w = f.session.wallet(&f.wallet).await.unwrap();
        let state = w.state().await;
        crate::coins::read_coins(&state.core_wallet)
            .coins
            .into_iter()
            .find(|c| c.utxo.outpoint == collateral)
            .unwrap()
            .utxo
            .txout
            .script_pubkey
    });
    dw_protx::rules::check_registration(
        &tx,
        p,
        Some((&script, Some(EVONODE_COLLATERAL))),
        &[],
        dashcore::Network::Regtest,
    )
    .unwrap();
    assert!(f.locked().contains(&collateral), "D9");
}

/// `register_prepare_evo`'s flow: the platform fields are in the prepared
/// payload, so in the message the collateral's key signs.
#[test]
fn mg04_evonode_external_signs_over_its_platform_fields() {
    let f = fx(Library::Accept);
    f.credit(1, COIN);
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let address = dashcore::Address::p2pkh(&public, dashcore::Network::Regtest);
    let outpoint = OutPoint::new(Txid::from_byte_array([0x78; 32]), 0);
    let req = evonode(
        CollateralChoice::External {
            outpoint: format!("{}-0", outpoint.txid),
            address: address.to_string(),
        },
        PlatformNodeChoice::Keychain { index: 3 },
    );
    let op = f
        .engine
        .block_on(f.mn().prepare_external_registration(f.wallet, req))
        .unwrap();
    assert_eq!(op.terms.platform_node_index, Some(3));
    let unsigned: Transaction =
        dashcore::consensus::deserialize(f.rows().pop().unwrap().unsigned_tx.as_ref().unwrap())
            .unwrap();
    assert_eq!(platform_of(&unsigned), op.terms.platform);
    let ask = op.sign_request.unwrap();
    assert_eq!(
        ask.message,
        dw_protx::payloads::registration_sign_message(
            payload(&unsigned),
            dashcore::Network::Regtest
        )
        .unwrap()
    );
    let sig = dw_protx::payloads::sign_message_compact(&EXTERNAL_KEY, &ask.message).unwrap();
    let done = f
        .engine
        .block_on(f.mn().submit_external_registration(
            f.wallet,
            op.id,
            {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(sig)
            },
            f.grant(COIN),
        ))
        .unwrap();
    assert_eq!(done.state, ProTxOperationState::Sent);
    let tx = f.lib.sent().pop().unwrap();
    assert_eq!(payload(&tx).masternode_type as u16, 1);
    assert_eq!(platform_of(&tx), done.terms.platform);
    assert_eq!(payload(&tx).signature, sig.to_vec());
}

/// The evonode refusals, before anything is signed (D18): terms only for
/// an evonode, a node id of 40 hex, Core's port rules and its duplicate
/// node id; `NextUnused` skips a listed node id.
#[test]
fn mg04_evonode_requests_are_refused_before_signing() {
    let f = fx(Library::Accept);
    f.credit(1, EVONODE_COLLATERAL + COIN);
    let preview = |req: RegistrationRequest| {
        f.engine
            .block_on(f.mn().preview_registration(f.wallet, req))
    };
    let next = || evonode(CollateralChoice::FundNew, PlatformNodeChoice::NextUnused);
    let mut no_terms = next();
    no_terms.evonode = None;
    assert_eq!(preview(no_terms).unwrap_err().code(), "invalid_argument");
    let mut regular_with_terms = next();
    regular_with_terms.kind = MasternodeType::Regular;
    assert_eq!(
        preview(regular_with_terms).unwrap_err().code(),
        "invalid_argument"
    );
    let bad_id = evonode(
        CollateralChoice::FundNew,
        PlatformNodeChoice::Given {
            node_id: "ab".repeat(19),
        },
    );
    assert_eq!(preview(bad_id).unwrap_err().code(), "invalid_argument");
    let null_id = evonode(
        CollateralChoice::FundNew,
        PlatformNodeChoice::Given {
            node_id: "00".repeat(20),
        },
    );
    assert!(
        matches!(preview(null_id).unwrap_err(), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-platform-nodeid")
    );
    let mut same_ports = next();
    same_ports.evonode.as_mut().unwrap().http_port = Some(22_200);
    assert!(
        matches!(preview(same_ports).unwrap_err(), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-platform-dup-ports")
    );
    let mut mainnet_core = next();
    mainnet_core.evonode.as_mut().unwrap().p2p_port = Some(9_999);
    assert!(
        matches!(preview(mainnet_core).unwrap_err(), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-platform-p2p-port")
    );
    let mut core_port = next();
    core_port.evonode.as_mut().unwrap().p2p_port = Some(19_899);
    assert!(
        matches!(preview(core_port).unwrap_err(), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-platform-dup-ports")
    );

    // The keychain's first node id, listed: given, it is Core's duplicate;
    // by default the next one is picked.
    let first = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Evonode))
        .unwrap()
        .platform_node
        .unwrap()
        .node_id
        .unwrap();
    let mut listed = [0u8; 20];
    hex::decode_to_slice(&first, &mut listed).unwrap();
    *f.seam.list.lock().unwrap() = Some(vec![platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: [9; 32],
        service_address: Some("1.2.3.5:19899".parse().unwrap()),
        platform_http_port: Some(22_201),
        operator_public_key: [0x99; 48],
        voting_key_id: [9; 20],
        platform_node_id: Some(listed),
        is_valid: true,
        is_evonode: true,
        has_extended_net_info: false,
        net_info: None,
    }]);
    let dup = evonode(
        CollateralChoice::FundNew,
        PlatformNodeChoice::Given { node_id: first },
    );
    assert!(
        matches!(preview(dup).unwrap_err(), MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-platformnodeid")
    );
    let ok = preview(next()).unwrap();
    assert_eq!(ok.terms.platform_node_index, Some(1));
}

// ---------------------------------------------------------------------------
// ProUpRegTx (slice 4)
// ---------------------------------------------------------------------------

/// An attached owner key's secret (a masternode registered elsewhere).
const OWNER_KEY: [u8; 32] = [0x6f; 32];

fn address_of(key_id: [u8; 20]) -> String {
    dashcore::Address::new(
        dashcore::Network::Regtest,
        dashcore::address::Payload::PubkeyHash(dashcore::PubkeyHash::from_byte_array(key_id)),
    )
    .to_string()
}

fn key_id_of(secret: [u8; 32]) -> [u8; 20] {
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(secret).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    public.pubkey_hash().to_byte_array()
}

/// The list entry of the ProRegTx `tx`, as its payload registered it.
fn entry_of(tx: &Transaction) -> platform_wallet::masternode::MasternodeListSummary {
    let p = payload(tx);
    platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: tx.txid().to_byte_array(),
        service_address: p.service_address.primary_service_address(),
        platform_http_port: None,
        operator_public_key: *p.operator_public_key.as_ref(),
        voting_key_id: p.voting_key_hash.to_byte_array(),
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: false,
        net_info: None,
    }
}

fn up_payload(
    tx: &Transaction,
) -> &dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload{
    super::update::update_payload(tx).expect("a ProUpRegTx")
}

fn keep(hash: &str) -> UpdateRegistrarRequest {
    UpdateRegistrarRequest {
        pro_tx_hash: hash.into(),
        operator: KeepOr::Keep,
        voting: KeepOr::Keep,
        payout: PayoutChoice::NewAddress,
    }
}

impl Fx {
    /// A confirmed `FundNew` registration of the keychain's first keys, in
    /// the synced list, and a coin to pay an update with. Returns the
    /// ProRegTx and its operation.
    fn registered(&self) -> (Transaction, ProTxOperation) {
        let funding = self.credit(1, MASTERNODE_COLLATERAL + COIN);
        let op = self
            .register(request(CollateralChoice::FundNew), self.grant(COIN))
            .unwrap();
        let tx = self.lib.sent().pop().unwrap();
        let vout = payload(&tx).collateral_outpoint.vout;
        self.spend(funding);
        self.hold(
            OutPoint::new(tx.txid(), vout),
            tx.output[vout as usize].clone(),
        );
        self.confirm(tx.clone(), 7);
        *self.seam.list.lock().unwrap() = Some(vec![entry_of(&tx)]);
        self.credit(2, COIN);
        (tx, op)
    }

    fn preview_update(
        &self,
        req: UpdateRegistrarRequest,
    ) -> Result<UpdateRegistrarPreview, MasternodesError> {
        self.engine
            .block_on(self.mn().preview_update_registrar(self.wallet, req))
    }

    fn update(
        &self,
        req: UpdateRegistrarRequest,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.engine
            .block_on(self.mn().update_registrar(self.wallet, req, grant))
    }
}

/// D10, D11: kept keys come from the list, the payout is named, the
/// keychain owner key signs the payload hash, the lease is charged the fee.
#[test]
fn mg04_update_registrar_keeps_or_sets_each_key_signed_by_the_keychain_owner() {
    let f = fx(Library::Accept);
    let (reg, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let rp = payload(&reg).clone();
    let same_payout = PayoutChoice::Address {
        address: op.terms.payout_address.clone(),
    };

    let unchanged = f
        .preview_update(UpdateRegistrarRequest {
            payout: same_payout.clone(),
            ..keep(&hash)
        })
        .unwrap();
    assert_eq!(unchanged.owner_key, OwnerKeySource::Keychain { index: 0 });
    assert_eq!(unchanged.version, 2, "D1");
    assert_eq!(
        (
            unchanged.operator_changes,
            unchanged.voting_changes,
            unchanged.payout_changes,
            unchanged.pose_bans
        ),
        (false, false, Some(false), false)
    );
    assert_eq!(unchanged.total_debit, unchanged.fee);
    assert_eq!(
        unchanged.terms.operator_public_key,
        op.terms.operator_public_key
    );
    assert_eq!(unchanged.terms.voting_address, op.terms.voting_address);
    assert_eq!(unchanged.terms.owner_address, op.terms.owner_address);
    assert_eq!(unchanged.terms.service, op.terms.service);
    let fresh = f.preview_update(keep(&hash)).unwrap();
    assert_eq!(fresh.payout_changes, Some(true));
    assert_eq!(
        fresh.terms.payout_address, "",
        "issued only at the update (D29)"
    );
    let operator = f
        .preview_update(UpdateRegistrarRequest {
            operator: KeepOr::Set(OperatorChoice::NextUnused),
            ..keep(&hash)
        })
        .unwrap();
    assert!(operator.operator_changes && operator.pose_bans);
    assert_eq!(operator.terms.operator_index, Some(1), "the next unused");

    let req = UpdateRegistrarRequest {
        voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
        payout: same_payout,
        ..keep(&hash)
    };
    let preview = f.preview_update(req.clone()).unwrap();
    assert!(preview.voting_changes && !preview.operator_changes && !preview.pose_bans);
    let short = f.update(req.clone(), f.grant(preview.fee - 1)).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    assert_eq!(f.lib.sent().len(), 1, "nothing signed");

    let up = f.update(req, f.grant(preview.fee)).unwrap();
    assert_eq!(up.kind, ProTxOperationKind::UpdateRegistrar);
    assert_eq!(up.state, ProTxOperationState::Sent);
    assert_eq!(up.pro_tx_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(up.fee, preview.fee);
    let tx = f.lib.sent().pop().unwrap();
    assert_eq!(up.txid, Some(tx.txid().to_string()));
    assert!(
        *f.lib.saved_first.lock().unwrap().last().unwrap(),
        "the row was durable first"
    );
    assert_eq!((tx.version, tx.lock_time), (3, 0), "D25");
    let p = up_payload(&tx);
    assert_eq!(p.version, 2);
    assert_eq!(p.pro_tx_hash, reg.txid());
    assert_eq!(p.operator_public_key, rp.operator_public_key, "kept");
    assert_eq!(p.script_payout, rp.script_payout);
    assert_ne!(p.voting_key_hash, rp.voting_key_hash);
    assert_eq!(
        address_of(p.voting_key_hash.to_byte_array()),
        up.terms.voting_address
    );
    assert_eq!(up.terms.voting_index, Some(5));
    assert_eq!(p.payload_sig.len(), 65);
    dw_protx::rules::check_update_registrar(&tx, p, rp.owner_key_hash.to_byte_array(), None, &[])
        .expect("signed by the owner key over the payload hash");
    let row = f.rows().into_iter().find(|r| r.id == up.id).unwrap();
    assert_eq!(row.kind, "update_registrar");
    assert!(!row.locked_collateral);
}

/// D12: with no keychain owner key, an attached `owner` key matched to the
/// ProRegTx signs, and only then.
#[test]
fn mg04_update_registrar_signs_with_an_attached_owner_key() {
    let f = fx(Library::Accept);
    let (reg, _) = f.registered();
    // The same masternode, registered elsewhere with an owner key the
    // keychain doesn't have.
    let mut other = reg.clone();
    if let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
        &mut other.special_transaction_payload
    {
        p.owner_key_hash = dashcore::PubkeyHash::from_byte_array(key_id_of(OWNER_KEY));
        p.service_address = dashcore::sml::masternode_list_entry::MasternodeNetInfo::Legacy(
            "1.2.3.9:19899".parse().unwrap(),
        );
    }
    f.confirm(other.clone(), 8);
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    let mut entry = entry_of(&other);
    entry.voting_key_id = [0x42; 20];
    hex::decode_to_slice(
        keys.operator.public_key.unwrap(),
        &mut entry.operator_public_key,
    )
    .unwrap();
    *f.seam.list.lock().unwrap() = Some(vec![entry_of(&reg), entry]);
    let hash = other.txid().to_string();

    let e = f.preview_update(keep(&hash)).unwrap_err();
    assert_eq!(e.code(), "masternodes.no_owner_key");
    let service = f.session.manager().unwrap().tracked_masternodes_service();
    service
        .track_blocking(other.txid().to_byte_array(), None)
        .unwrap();
    drop(service);
    let attach = |secret: [u8; 32], role| {
        f.engine.block_on(f.mn().attach_key(
            hash.clone(),
            role,
            Zeroizing::new(hex::encode(secret)),
        ))
    };
    // Another key isn't this masternode's owner key (MG-03 §4).
    let e = attach([0x43; 32], AttachedKeyRole::Owner).unwrap_err();
    assert_eq!(e.code(), "masternodes.key_mismatch");
    let e = f.preview_update(keep(&hash)).unwrap_err();
    assert_eq!(e.code(), "masternodes.no_owner_key");
    assert_eq!(
        attach(OWNER_KEY, AttachedKeyRole::Owner).unwrap(),
        KeyCheck::Matched
    );

    let preview = f.preview_update(keep(&hash)).unwrap();
    assert_eq!(preview.owner_key, OwnerKeySource::Attached);
    assert_eq!(preview.terms.owner_index, None);
    assert_eq!(preview.payout_changes, Some(true));
    let up = f.update(keep(&hash), f.grant(preview.fee)).unwrap();
    let tx = f.lib.sent().pop().unwrap();
    let p = up_payload(&tx);
    assert_eq!(p.pro_tx_hash, other.txid());
    assert_eq!(p.voting_key_hash.to_byte_array(), [0x42; 20], "kept");
    dw_protx::rules::check_update_registrar(&tx, p, key_id_of(OWNER_KEY), None, &[])
        .expect("signed by the attached owner key");
    assert_eq!(up.terms.owner_address, address_of(key_id_of(OWNER_KEY)));
}

/// D33, §6: offline checks first, then the list, the masternode, the owner
/// key, Core's rules.
#[test]
fn mg04_update_registrar_requests_are_refused_in_order() {
    let f = fx(Library::Accept);
    let (reg, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let preview = |req: UpdateRegistrarRequest| f.preview_update(req).unwrap_err();

    assert_eq!(preview(keep("zz")).code(), "invalid_argument");
    let secret = preview(UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::Given {
            public_key: "ab".repeat(32),
        }),
        ..keep(&hash)
    });
    assert!(
        secret.to_string().contains("secret") && !secret.to_string().contains(&"ab".repeat(32))
    );
    let voting = preview(UpdateRegistrarRequest {
        voting: KeepOr::Set(VotingChoice::Address {
            address: "not an address".into(),
        }),
        ..keep(&hash)
    });
    assert_eq!(voting.code(), "invalid_argument");
    let payout = preview(UpdateRegistrarRequest {
        payout: PayoutChoice::Address {
            address: "1.2.3.4".into(),
        },
        ..keep(&hash)
    });
    assert_eq!(payout.code(), "invalid_argument");

    *f.seam.list.lock().unwrap() = None;
    assert_eq!(preview(keep(&hash)).code(), "masternodes.list_unavailable");
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    assert_eq!(preview(keep(&hash)).code(), "masternodes.not_found");

    // Listed, but no ProRegTx of it in a loaded wallet: no owner key id.
    let mut stranger = entry_of(&reg);
    stranger.pro_tx_hash = [0x99; 32];
    let mut shared = entry_of(&reg);
    shared.has_extended_net_info = true;
    *f.seam.list.lock().unwrap() = Some(vec![stranger.clone(), shared]);
    assert_eq!(
        preview(keep(&"99".repeat(32))).code(),
        "masternodes.no_owner_key"
    );
    let e = preview(keep(&hash));
    assert!(
        matches!(&e, MasternodesError::Unsupported { feature, .. } if feature == "shared_masternodes"),
        "{e:?}"
    );

    // Core's rules: another masternode's operator key, the owner's address
    // as the payee.
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    let mut operator = [0u8; 48];
    hex::decode_to_slice(keys.operator.public_key.as_ref().unwrap(), &mut operator).unwrap();
    stranger.operator_public_key = operator;
    *f.seam.list.lock().unwrap() = Some(vec![stranger, entry_of(&reg)]);
    let dup = preview(UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::Given {
            public_key: hex::encode(operator),
        }),
        ..keep(&hash)
    });
    assert!(
        matches!(&dup, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-key"),
        "{dup:?}"
    );
    let reuse = preview(UpdateRegistrarRequest {
        payout: PayoutChoice::Address {
            address: op.terms.owner_address.clone(),
        },
        ..keep(&hash)
    });
    assert!(
        matches!(&reuse, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-payee-reuse"),
        "{reuse:?}"
    );
    // A grant of another purpose signs nothing.
    let spend = f.spend_grant(COIN);
    let e = f.update(keep(&hash), spend).unwrap_err();
    assert_eq!(e.code(), "platform.grant_invalid");
    assert_eq!(f.lib.sent().len(), 1, "only the registration");
    assert!(f.rows().iter().all(|r| r.kind == "registration"));
}

/// D43, D49: a kept key is the list's, and two updates out at once may be
/// mined in either order, so any update is refused while the wallet's own
/// earlier update of the masternode may not be in the list: sent, or
/// confirmed above the list's height.
#[test]
fn mg04_an_update_waits_until_the_list_shows_the_last() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let voting = |index| KeepOr::Set(VotingChoice::Keychain { index });
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: voting(5),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::Sent);
    f.credit(3, COIN);
    let pending = |req: UpdateRegistrarRequest| {
        let e = f.preview_update(req).unwrap_err();
        assert!(
            matches!(&e, MasternodesError::UpdatePending { operation } if *operation == first.id),
            "{e:?}"
        );
        assert_eq!(e.code(), "masternodes.update_pending");
    };
    pending(keep(&hash));
    pending(UpdateRegistrarRequest {
        voting: voting(6),
        ..keep(&hash)
    });
    pending(UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::Keychain { index: 0 }),
        ..keep(&hash)
    });
    let explicit = UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::Keychain { index: 0 }),
        voting: voting(6),
        ..keep(&hash)
    };
    pending(explicit.clone());
    let e = f.update(explicit.clone(), f.grant(COIN)).unwrap_err();
    assert_eq!(e.code(), "masternodes.update_pending");
    assert_eq!(f.lib.sent().len(), 2, "the registration and the first");

    // In a block the list hasn't reached: still refused.
    let tx = f.lib.sent().pop().unwrap();
    f.confirm(tx.clone(), 150);
    pending(keep(&hash));
    // The list shows it now: kept keys are the update's.
    let mut entry = f.seam.list.lock().unwrap().as_ref().unwrap()[0].clone();
    entry.voting_key_id = up_payload(&tx).voting_key_hash.to_byte_array();
    *f.seam.list.lock().unwrap() = Some(vec![entry]);
    *f.seam.list_height.lock().unwrap() = 150;
    let p = f.preview_update(keep(&hash)).unwrap();
    assert!(!p.voting_changes && !p.operator_changes, "{p:?}");
    assert_eq!(p.terms.voting_address, first.terms.voting_address);
    let p = f.preview_update(explicit).unwrap();
    assert_eq!(p.terms.voting_index, Some(6));
}

/// D43, D49: a `not_sent` update can still go out, so it blocks the next
/// update. One another loaded wallet saved after it (its owner key is
/// attached there, D24) blocks its resend, which would restore what the
/// later one replaced.
#[test]
fn mg04_a_resendable_update_blocks_the_next_and_a_later_one_blocks_its_resend() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let voting = |index| KeepOr::Set(VotingChoice::Keychain { index });
    *f.lib.mode.lock().unwrap() = Library::NoPeers;
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: voting(5),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::NotSent);
    f.credit(3, COIN);
    for req in [
        keep(&hash),
        UpdateRegistrarRequest {
            operator: KeepOr::Set(OperatorChoice::Keychain { index: 0 }),
            voting: voting(6),
            ..keep(&hash)
        },
    ] {
        let e = f.preview_update(req).unwrap_err();
        assert!(
            matches!(&e, MasternodesError::UpdatePending { operation } if *operation == first.id),
            "{e:?}"
        );
    }

    *f.lib.mode.lock().unwrap() = Library::Accept;
    let other = f.import_other_wallet();
    f.other_wallets_update(other, &first.id, "later-update", 1, false);
    let handed = f.lib.sent().len();
    let e = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, first.id.clone(), f.grant(COIN)),
        )
        .unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdateSuperseded { operation } if operation == "later-update"),
        "{e:?}"
    );
    assert_eq!(e.code(), "masternodes.update_superseded");
    assert_eq!(f.lib.sent().len(), handed, "nothing handed off");
}

/// D43: an attached owner key updates a masternode from any loaded wallet,
/// so another loaded wallet's unlisted update blocks a keep too.
#[test]
fn mg04_a_keep_waits_for_another_loaded_wallets_update() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    // This wallet's update is in a block the list has passed.
    let tx = f.lib.sent().pop().unwrap();
    f.confirm(tx.clone(), 50);
    let mut entry = f.seam.list.lock().unwrap().as_ref().unwrap()[0].clone();
    entry.voting_key_id = up_payload(&tx).voting_key_hash.to_byte_array();
    *f.seam.list.lock().unwrap() = Some(vec![entry]);
    f.credit(3, COIN);
    f.preview_update(keep(&hash)).unwrap();

    let other = f.import_other_wallet();
    let mut row = f.rows().into_iter().find(|r| r.id == first.id).unwrap();
    row.id = "other-wallets-update".into();
    row.sent_at = None;
    row.last_error = None;
    f.db()
        .insert_masternode_op_durable(&other.to_string(), &row, &[], 1)
        .unwrap();
    let e = f.preview_update(keep(&hash)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation } if operation == "other-wallets-update"),
        "{e:?}"
    );
}

/// D7, D43: a `not_sent` update resends (its own row isn't "later"), and
/// an earlier update (another loaded wallet's) doesn't block it.
#[test]
fn mg04_an_update_resends_unless_a_later_one_exists() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    *f.lib.mode.lock().unwrap() = Library::NoPeers;
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::NotSent);
    let other = f.import_other_wallet();
    f.other_wallets_update(other, &first.id, "earlier-update", -1, false);
    *f.lib.mode.lock().unwrap() = Library::Accept;
    let sent = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, first.id.clone(), f.grant(COIN)),
        )
        .unwrap();
    assert_eq!(sent.state, ProTxOperationState::Sent);
}

/// D49: a resend waits for an earlier update that was handed off and isn't
/// in a block (another loaded wallet's), which it could be mined before.
#[test]
fn mg04_an_update_resend_waits_for_an_earlier_sent_one() {
    let f = fx(Library::NoPeers);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::NotSent);
    let other = f.import_other_wallet();
    f.other_wallets_update(other, &first.id, "earlier-sent", -1, true);
    *f.lib.mode.lock().unwrap() = Library::Accept;
    let handed = f.lib.sent().len();
    let e = f
        .engine
        .block_on(
            f.mn()
                .resend_operation(f.wallet, first.id.clone(), f.grant(COIN)),
        )
        .unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation } if operation == "earlier-sent"),
        "{e:?}"
    );
    assert_eq!(f.lib.sent().len(), handed, "nothing handed off");
}

/// D49 (r1 m2): an update that never lists, its transaction dropped, holds
/// the next one until it settles. The way out is to spend one of its
/// inputs (the user abandons the update's transaction, unlocks the coin
/// and spends it): once that spend is in a block the update reads
/// `conflicted`, can never be mined, `operations()` releases its inputs,
/// and it blocks nothing. Seen only unconfirmed, the rival changes nothing.
#[test]
fn mg04_a_conflicted_update_lets_the_next_one_through() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let first = f
        .update(
            UpdateRegistrarRequest {
                voting: KeepOr::Set(VotingChoice::Keychain { index: 5 }),
                ..keep(&hash)
            },
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::Sent);
    f.credit(3, COIN);
    let next = UpdateRegistrarRequest {
        voting: KeepOr::Set(VotingChoice::Keychain { index: 6 }),
        ..keep(&hash)
    };
    let e = f.preview_update(next.clone()).unwrap_err();
    assert_eq!(e.code(), "masternodes.update_pending");

    let tx = f.lib.sent().pop().unwrap();
    let input = tx.input[0].previous_output;
    let rival = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: input,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: COIN / 2,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    };
    // Review r1 m-3: a rival seen unconfirmed conflicts with nothing yet;
    // only a block settles the row, and its locks stay meanwhile.
    f.record(
        rival.clone(),
        key_wallet::transaction_checking::TransactionContext::Mempool,
    );
    f.spend(input);
    let ops = f.operations();
    let unsettled = ops.iter().find(|o| o.id == first.id).unwrap();
    assert_eq!(unsettled.state, ProTxOperationState::Sent);
    assert!(f.locked().contains(&input), "D25");
    let e = f.preview_update(next.clone()).unwrap_err();
    assert_eq!(e.code(), "masternodes.update_pending");

    f.confirm(rival, 60);
    // MG-04b D70 (review r1 of 1b, n-5): a conflict a reorg may undo keeps
    // the inputs locked; five blocks deep it may, six it won't.
    f.tip(64);
    let ops = f.operations();
    let settled = ops.iter().find(|o| o.id == first.id).unwrap();
    assert_eq!(settled.state, ProTxOperationState::Conflicted);
    assert!(f.locked().contains(&input), "D70");
    f.tip(65);
    f.operations();
    let locked = f.engine.block_on(f.session.locked_outpoints(f.wallet));
    let locked = locked.unwrap();
    assert!(
        tx.input
            .iter()
            .all(|i| !locked.contains(&i.previous_output)),
        "{locked:?}"
    );

    f.preview_update(next.clone()).unwrap();
    let op = f.update(next, f.grant(COIN)).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
}

/// MG-06 meets MG-04 in the coins (D47). A proposal fee the wallet hasn't
/// seen may be out, and its resend claims its coins again (MG-06 D10):
/// neither the in-session claim nor, after a restart, the saved row lets a
/// registration take one as a collateral, by preview or registration.
#[test]
fn mg04_a_coin_an_unseen_proposal_fee_spends_is_not_a_collateral() {
    let f = fx(Library::Accept);
    let collateral = f.credit(1, MASTERNODE_COLLATERAL);
    f.credit(2, COIN);
    let req = request(CollateralChoice::ExistingUtxo {
        outpoint: format!("{}-0", collateral.txid),
    });
    let refused = |f: &Fx| {
        let preview = f
            .engine
            .block_on(f.mn().preview_registration(f.wallet, req.clone()));
        assert_eq!(
            preview.unwrap_err().code(),
            "masternodes.collateral_unavailable"
        );
        let e = f.register(req.clone(), f.grant(COIN)).unwrap_err();
        assert_eq!(e.code(), "masternodes.collateral_unavailable");
        assert!(f.lib.sent().is_empty());
        assert!(f.rows().is_empty(), "no row");
        let locked = f.engine.block_on(f.session.locked_outpoints(f.wallet));
        assert!(locked.unwrap().is_empty(), "no lock");
    };

    // The fee's hand-off, in session: its claim.
    let fee = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: collateral,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: MASTERNODE_COLLATERAL - COIN,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    };
    let owner = SpendOwner::Tx(fee.txid());
    (f.session.spends)
        .reserve_unless_held(f.wallet, owner, &[collateral])
        .unwrap();
    refused(&f);

    // A restart drops the claim; the row, sent and unseen, stays.
    f.session.spends.release(&f.wallet, owner);
    let data = b"[]".to_vec();
    let mut object = dw_governance::object::GovernanceObject::new_proposal(1, 1, data.clone());
    object.collateral_hash = fee.txid().to_byte_array();
    let row = dw_appdb::ProposalRow {
        wallet_id: f.wallet.to_string(),
        hash: object.hash(),
        revision: 1,
        time: 1,
        data,
        fee_txid: fee.txid().to_byte_array(),
        fee_tx: dashcore::consensus::serialize(&fee),
        created_at: 1,
        fee_sent_at: Some(1),
        submitted_at: None,
        last_error: None,
    };
    let appdb = f.session.live().unwrap().appdb;
    appdb.insert_proposal(&row).unwrap();
    refused(&f);

    // Another coin of the amount is still a collateral.
    let other = f.credit(3, MASTERNODE_COLLATERAL);
    let req = request(CollateralChoice::ExistingUtxo {
        outpoint: format!("{}-0", other.txid),
    });
    let op = f.register(req, f.grant(COIN)).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
}

/// D51's refusal, naming `operation`, the row holding the key (r2 n-6).
fn protx_dup(e: &MasternodesError, operation: &str) -> bool {
    matches!(e, MasternodesError::ProtxRefused { reason, operation: Some(o) }
        if reason == "protx-dup" && o == operation)
}

/// D51 (review r1 m-2): an operator key, platform node id or service that
/// one of the wallet's own operations names while it may still go out, or
/// is out and not in the list, is refused as Core's mempool refuses it
/// (`protx-dup`), whatever the choice. A conflicted operation frees them.
#[test]
fn mg04_a_pending_operations_keys_and_service_are_refused_as_protx_dup() {
    use CollateralChoice::FundNew;
    let f = fx(Library::Accept);
    let funding = f.credit(1, EVONODE_COLLATERAL + COIN);
    let first = f
        .register(
            evonode(FundNew, PlatformNodeChoice::Keychain { index: 0 }),
            f.grant(COIN),
        )
        .unwrap();
    assert_eq!(first.state, ProTxOperationState::Sent);
    f.credit(2, EVONODE_COLLATERAL + COIN);
    let elsewhere = |r: RegistrationRequest| RegistrationRequest {
        service: "1.2.3.9".into(),
        ..r
    };
    let node_id = first.terms.platform.as_ref().unwrap().node_id.clone();
    let clashes = [
        (
            "keychain operator",
            elsewhere(RegistrationRequest {
                operator: OperatorChoice::Keychain {
                    index: first.terms.operator_index.unwrap(),
                },
                ..request(FundNew)
            }),
        ),
        (
            "given operator",
            elsewhere(RegistrationRequest {
                operator: OperatorChoice::Given {
                    public_key: first.terms.operator_public_key.clone(),
                },
                ..request(FundNew)
            }),
        ),
        (
            "given node",
            elsewhere(evonode(FundNew, PlatformNodeChoice::Given { node_id })),
        ),
        ("service", request(FundNew)),
    ];
    let preview = |r: &RegistrationRequest| {
        f.engine
            .block_on(f.mn().preview_registration(f.wallet, r.clone()))
    };
    for (name, req) in &clashes {
        let e = preview(req).unwrap_err();
        assert!(protx_dup(&e, &first.id), "{name}: {e:?}");
        let e = f.register(req.clone(), f.grant(COIN)).unwrap_err();
        assert!(protx_dup(&e, &first.id), "{name}: {e:?}");
    }
    assert_eq!(f.lib.sent().len(), 1, "nothing else handed off");
    preview(&elsewhere(request(FundNew))).unwrap();

    // A block spends its input elsewhere: it can never be mined.
    let rival = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: funding,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: COIN,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    };
    f.confirm(rival, 60);
    f.spend(funding);
    for (name, req) in &clashes {
        preview(req).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

/// D51: a ProUpRegTx's new operator key, against another masternode's
/// pending registration, until the list holds what was mined.
#[test]
fn mg04_an_updates_operator_key_is_refused_while_another_registration_holds_it() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    f.credit(3, MASTERNODE_COLLATERAL + COIN);
    let second = f
        .register(
            RegistrationRequest {
                service: "1.2.3.9".into(),
                operator: OperatorChoice::Keychain { index: 5 },
                ..request(CollateralChoice::FundNew)
            },
            f.grant(COIN),
        )
        .unwrap();
    let second_tx = f.lib.sent().pop().unwrap();
    let take = UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::Keychain { index: 5 }),
        ..keep(&hash)
    };
    let e = f.preview_update(take.clone()).unwrap_err();
    assert!(protx_dup(&e, &second.id), "{e:?}");
    let e = f.update(take.clone(), f.grant(COIN)).unwrap_err();
    assert!(protx_dup(&e, &second.id), "{e:?}");
    // Mined above the list's height: not in the list yet (D43's window).
    f.confirm(second_tx, 120);
    let e = f.preview_update(take.clone()).unwrap_err();
    assert!(protx_dup(&e, &second.id), "{e:?}");
    // The list reaches it: the list's own rules apply from here.
    *f.seam.list_height.lock().unwrap() = 150;
    f.preview_update(take).unwrap();
    assert_eq!(second.state, ProTxOperationState::Sent);
}

/// D51: an `External` registration's submit is checked against the
/// wallet's other pending operations, not against itself.
#[test]
fn mg04_an_external_submit_is_refused_while_another_operation_holds_its_service() {
    let f = fx(Library::Accept);
    f.credit(1, COIN);
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let address = dashcore::Address::p2pkh(&public, dashcore::Network::Regtest);
    let outpoint = OutPoint::new(Txid::from_byte_array([0x79; 32]), 0);
    let req = request(CollateralChoice::External {
        outpoint: format!("{}-0", outpoint.txid),
        address: address.to_string(),
    });
    let op = f
        .engine
        .block_on(f.mn().prepare_external_registration(f.wallet, req))
        .unwrap();
    // A row saved before D51 naming the same service.
    let mut twin = f.rows().pop().unwrap();
    twin.id = super::ops::new_id();
    twin.fee_inputs = "[]".into();
    f.db()
        .insert_masternode_op_durable(&f.wallet.to_string(), &twin, &[], 1)
        .unwrap();
    let ask = op.sign_request.clone().unwrap();
    let sig = dw_protx::payloads::sign_message_compact(&EXTERNAL_KEY, &ask.message).unwrap();
    let signature = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(sig)
    };
    let submit = || {
        f.engine.block_on(f.mn().submit_external_registration(
            f.wallet,
            op.id.clone(),
            signature.clone(),
            f.grant(COIN),
        ))
    };
    let e = submit().unwrap_err();
    assert!(protx_dup(&e, &twin.id), "{e:?}");
    assert!(f.lib.sent().is_empty());
    f.engine
        .block_on(f.mn().discard_operation(f.wallet, twin.id))
        .unwrap();
    assert_eq!(submit().unwrap().state, ProTxOperationState::Sent);
}

/// D51 (review r2 n-5): an update claims only its operator key, the one
/// key Core's mempool indexes for a ProUpRegTx: the service an update row
/// carries from the list is the list's to judge, so once the operator moves
/// it (ProUpServTx) a new registration may take it.
#[test]
fn mg04_an_updates_service_is_not_claimed() {
    let f = fx(Library::Accept);
    let (reg, op) = f.registered();
    let update = f
        .update(keep(op.pro_tx_hash.as_deref().unwrap()), f.grant(COIN))
        .unwrap();
    assert_eq!(update.state, ProTxOperationState::Sent);
    assert_eq!(update.terms.service, "1.2.3.4:19899");
    let mut moved = entry_of(&reg);
    moved.service_address = Some("1.2.3.7:19899".parse().unwrap());
    *f.seam.list.lock().unwrap() = Some(vec![moved]);
    f.credit(3, MASTERNODE_COLLATERAL + COIN);
    f.engine
        .block_on(
            f.mn()
                .preview_registration(f.wallet, request(CollateralChoice::FundNew)),
        )
        .unwrap();
}

/// D54 (review r2 m-2): a conflicted registration never confirms, so its
/// collateral and keys are free again. The same coin registers again with
/// the same owner key, and the follower unlocks the coin once no other
/// operation names it.
#[test]
fn mg04_a_conflicted_registration_frees_its_collateral_and_keys() {
    let f = fx(Library::Accept);
    let collateral = f.credit(1, MASTERNODE_COLLATERAL);
    let req = request(CollateralChoice::ExistingUtxo {
        outpoint: format!("{}-0", collateral.txid),
    });
    // Another device spends the operation's fee input in a block.
    let conflict = |op: &ProTxOperation, height: u32| {
        let tx = f.lib.sent().pop().unwrap();
        assert_eq!(tx.txid().to_string(), op.txid.clone().unwrap());
        let input = tx.input[0].previous_output;
        f.confirm(
            Transaction {
                version: 2,
                lock_time: 0,
                input: vec![dashcore::TxIn {
                    previous_output: input,
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: COIN,
                    script_pubkey: dashcore::ScriptBuf::new(),
                }],
                special_transaction_payload: None,
            },
            height,
        );
        f.spend(input);
        let state = |id: &str| {
            f.operations()
                .into_iter()
                .find(|o| o.id == id)
                .unwrap()
                .state
        };
        assert_eq!(state(&op.id), ProTxOperationState::Conflicted);
    };
    f.credit(2, COIN);
    let first = f.register(req.clone(), f.grant(COIN)).unwrap();
    assert!(f.locked().contains(&collateral), "D9");
    conflict(&first, 60);

    // Before the follower runs: the coin, still locked, and the owner key
    // are free for a new registration.
    f.credit(3, COIN);
    let second = f.register(req.clone(), f.grant(COIN)).unwrap();
    assert_eq!(second.state, ProTxOperationState::Sent);
    assert_eq!(second.terms.owner_index, first.terms.owner_index);
    f.follow();
    assert!(f.locked().contains(&collateral), "the second row names it");

    // Once no operation that may confirm names it, it is unlocked.
    conflict(&second, 61);
    f.follow();
    assert!(!f.locked().contains(&collateral), "D54");
    assert!(f.rows().iter().all(|r| !r.locked_collateral));
    f.credit(4, COIN);
    f.register(req, f.grant(COIN)).unwrap();
    assert!(f.locked().contains(&collateral), "locked by its new row");
}

/// D52 (review r1 n-3): a voting address that is the owner key's is
/// refused, in a registration and in an update, though Core takes it.
#[test]
fn mg04_voting_never_shares_the_owner_key() {
    let f = fx(Library::Accept);
    let keys = f
        .engine
        .block_on(f.mn().registration_keys(f.wallet, MasternodeType::Regular))
        .unwrap();
    f.credit(5, MASTERNODE_COLLATERAL + COIN);
    let shared = RegistrationRequest {
        voting: VotingChoice::Address {
            address: keys.owner.address.clone().unwrap(),
        },
        ..request(CollateralChoice::FundNew)
    };
    let e = f
        .engine
        .block_on(f.mn().preview_registration(f.wallet, shared))
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e:?}");

    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    let e = f
        .preview_update(UpdateRegistrarRequest {
            voting: KeepOr::Set(VotingChoice::Address {
                address: op.terms.owner_address.clone(),
            }),
            ..keep(&hash)
        })
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e:?}");
}

impl Fx {
    /// [`crate::coins::write_paused`]: `first` held at its row write.
    fn write_paused<A, B>(
        &self,
        first: impl Future<Output = A> + Send + 'static,
        second: impl Future<Output = B> + Send + 'static,
    ) -> (A, B)
    where
        A: Send + 'static,
        B: Send + 'static,
    {
        crate::coins::write_paused(&self.engine, &self.session, first, second)
    }

    /// A `FundNew` registration of the next unused keys (at a service of
    /// its own), held at its row write by [`Self::write_paused`].
    fn first_registration(
        &self,
    ) -> impl Future<Output = Result<ProTxOperation, MasternodesError>> + Send + 'static {
        let (m, w, grant) = (self.mn(), self.wallet, self.grant(COIN));
        let req = RegistrationRequest {
            service: "1.2.3.5".into(),
            ..request(CollateralChoice::FundNew)
        };
        async move { m.register(w, req, grant).await }
    }
}

/// BL-352: a registration holds the coin fence from its keys to its row's
/// write (as MG-04b D63), so another one waits and takes the next keys and
/// other coins.
#[test]
fn mg04_concurrent_registrations_take_different_keys_and_coins() {
    let f = fx(Library::Accept);
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    f.credit(2, MASTERNODE_COLLATERAL + COIN);
    let (m, w, grant) = (f.mn(), f.wallet, f.grant(COIN));
    let (a, b) = f.write_paused(f.first_registration(), async move {
        m.register(w, request(CollateralChoice::FundNew), grant)
            .await
    });
    let (a, b) = (a.unwrap().terms, b.unwrap().terms);
    assert_ne!(a.owner_address, b.owner_address);
    assert_ne!(a.voting_address, b.voting_address);
    assert_ne!(a.operator_public_key, b.operator_public_key);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].fee_inputs, rows[1].fee_inputs);
}

/// BL-352: an external registration's prepare waits for the fence before
/// it picks its keys.
#[test]
fn mg04_an_external_prepare_waits_for_a_registrations_keys() {
    let f = fx(Library::Accept);
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    f.credit(2, COIN);
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let address = dashcore::Address::p2pkh(&public, dashcore::Network::Regtest);
    let req = request(CollateralChoice::External {
        outpoint: format!("{}-1", Txid::from_byte_array([0x77; 32])),
        address: address.to_string(),
    });
    let (m, w) = (f.mn(), f.wallet);
    let (a, b) = f.write_paused(f.first_registration(), async move {
        m.prepare_external_registration(w, req).await
    });
    let (a, b) = (a.unwrap().terms, b.unwrap().terms);
    assert_ne!(a.owner_address, b.owner_address);
    assert_ne!(a.voting_address, b.voting_address);
    assert_ne!(a.operator_public_key, b.operator_public_key);
}

/// BL-352: an update waits for the fence before it picks its new operator
/// key, so it never takes the one a registration is writing.
#[test]
fn mg04_an_update_waits_for_a_registrations_keys() {
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    f.credit(3, MASTERNODE_COLLATERAL + COIN);
    let hash = op.pro_tx_hash.clone().unwrap();
    let (m, w, grant) = (f.mn(), f.wallet, f.grant(COIN));
    let req = UpdateRegistrarRequest {
        operator: KeepOr::Set(OperatorChoice::NextUnused),
        ..keep(&hash)
    };
    let (a, b) = f.write_paused(f.first_registration(), async move {
        m.update_registrar(w, req, grant).await
    });
    let (a, b) = (a.unwrap().terms, b.unwrap().terms);
    assert_ne!(a.operator_public_key, b.operator_public_key);
    assert_ne!(op.terms.operator_public_key, b.operator_public_key);
}

/// Opus r1 m-1: a refresh releases its old inputs, and the new plan may
/// take the ones still unspent, though the row lists them until the plan
/// is saved: its own holds don't count against it (BL-346).
#[test]
fn mg04_a_refresh_reuses_its_own_unspent_input() {
    let f = fx(Library::Accept);
    let only = f.credit(1, COIN);
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let address = dashcore::Address::p2pkh(&public, dashcore::Network::Regtest);
    let outpoint = OutPoint::new(Txid::from_byte_array([0x77; 32]), 1);
    let req = request(CollateralChoice::External {
        outpoint: format!("{}-1", outpoint.txid),
        address: address.to_string(),
    });
    let op = f
        .engine
        .block_on(f.mn().prepare_external_registration(f.wallet, req))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::NeedsSignature);
    assert!(f.locked().contains(&only));
    let fresh = f
        .engine
        .block_on(
            f.mn()
                .refresh_external_registration(f.wallet, op.id.clone()),
        )
        .unwrap();
    assert_eq!(fresh.state, ProTxOperationState::NeedsSignature);
    assert!(f.locked().contains(&only));
    let row = f
        .db()
        .masternode_op(&f.wallet.to_string(), &op.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        super::ops::decode_inputs(&row.fee_inputs),
        vec![only],
        "the refreshed plan spends the same coin"
    );
}

/// 0.1.x-bal §1: while a replay runs (the filter checkpoint rewound below
/// the highest seen), a ProTx selects no fee inputs. A registration, an
/// external registration's plan and a registrar update are refused, and
/// their fee quotes still answer.
#[test]
fn mg04_a_replay_holds_protx_fee_inputs() {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let f = fx(Library::Accept);
    let (_, op) = f.registered();
    let hash = op.pro_tx_hash.clone().unwrap();
    f.credit(3, MASTERNODE_COLLATERAL + COIN);
    let w = f.engine.block_on(f.session.wallet(&f.wallet)).unwrap();
    let checkpoint = |height: u32| {
        (f.engine).block_on(async { w.state_mut().await.core_wallet.update_synced_height(height) })
    };
    checkpoint(1_000);
    (f.session.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::SyncHeightAdvanced {
        wallet_id: f.wallet.0,
        height: 1_000,
    });
    checkpoint(500);
    let replaying = |r: Result<ProTxOperation, MasternodesError>| {
        r.is_err_and(|e| matches!(e.engine(), Some(crate::EngineError::RescanInProgress)))
    };
    // Each on its own service: the registered node holds 1.2.3.4.
    let fund_new = RegistrationRequest {
        service: "1.2.3.5".into(),
        ..request(CollateralChoice::FundNew)
    };
    f.fee_of(&fund_new);
    assert!(replaying(f.register(fund_new, f.grant(COIN))));
    let secret = dashcore::secp256k1::SecretKey::from_secret_bytes(EXTERNAL_KEY).unwrap();
    let public = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret));
    let external = RegistrationRequest {
        service: "1.2.3.6".into(),
        ..request(CollateralChoice::External {
            outpoint: format!("{}-1", Txid::from_byte_array([0x77; 32])),
            address: dashcore::Address::p2pkh(&public, dashcore::Network::Regtest).to_string(),
        })
    };
    let prepare = || {
        (f.engine).block_on(
            f.mn()
                .prepare_external_registration(f.wallet, external.clone()),
        )
    };
    assert!(replaying(prepare()));
    let preview = f.preview_update(keep(&hash)).unwrap();
    assert!(replaying(f.update(keep(&hash), f.grant(preview.fee))));
    checkpoint(1_000);
    f.update(keep(&hash), f.grant(preview.fee)).unwrap();
    prepare().unwrap();
}

/// A key-wallet record of `tx` in the BIP44 account.
fn tx_record(
    tx: Transaction,
    context: TransactionContext,
) -> key_wallet::managed_account::transaction_record::TransactionRecord {
    use key_wallet::account::{AccountType, StandardAccountType};
    tx_record_on(
        tx,
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
    )
}

fn tx_record_on(
    tx: Transaction,
    account: key_wallet::account::AccountType,
    context: TransactionContext,
) -> key_wallet::managed_account::transaction_record::TransactionRecord {
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::TransactionType;
    TransactionRecord::new(
        tx,
        account,
        context,
        TransactionType::Standard,
        TransactionDirection::Internal,
        Vec::new(),
        Vec::new(),
        0,
    )
}

fn chain_locked(height: u32) -> TransactionContext {
    use key_wallet::transaction_checking::BlockInfo;
    TransactionContext::InChainLockedBlock(BlockInfo::new(
        height,
        dashcore::BlockHash::all_zeros(),
        0,
    ))
}

impl Fx {
    /// The follower's per-poll lock step for wallets whose history gained a
    /// ProRegTx.
    fn collateral_pass(&self) {
        self.engine.block_on(self.session.protx_collateral_pass());
    }

    fn clear_pending(&self) {
        self.session.hub.history.take_collateral_pending();
    }

    /// The wallet's coin fence, as a build holds it.
    fn fence(&self) -> crate::coins::CoinFence {
        self.engine
            .block_on(self.session.coin_fence(self.wallet))
            .unwrap()
    }

    /// A ProRegTx this wallet holds the collateral of but didn't build:
    /// internal (its own output) or external (coin `seed` of the wallet's).
    fn foreign_registration(&self, internal: bool, seed: u8) -> (Transaction, OutPoint) {
        use crate::masternodes::aggregate::fixtures::{coins, registration};
        if internal {
            return self.internal_registration_in(self.wallet, seed);
        }
        let collateral = self.credit(seed, MASTERNODE_COLLATERAL);
        (
            registration(collateral, false, 0, coins(&[COIN])),
            collateral,
        )
    }

    /// A ProRegTx whose own output, held by `wallet`, is the collateral;
    /// `seed` sets its operator reward, so its txid.
    fn internal_registration_in(&self, wallet: WalletId, seed: u8) -> (Transaction, OutPoint) {
        use crate::masternodes::aggregate::fixtures::{coins, null, registration};
        let tx = registration(null(0), false, seed.into(), coins(&[MASTERNODE_COLLATERAL]));
        let collateral = OutPoint::new(tx.txid(), 0);
        self.hold_in(wallet, collateral, tx.output[0].clone());
        (tx, collateral)
    }

    /// `tx` in key-wallet's own history only, then the engine's history
    /// loaded from it, as at session open.
    fn load_with(&self, tx: Transaction) {
        use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
        let record = tx_record(tx, chain_locked(7));
        let session = Arc::clone(&self.session);
        let (id, inner) = (self.wallet, Arc::clone(&session));
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                wallet
                    .state_mut()
                    .await
                    .core_wallet
                    .accounts
                    .standard_bip44_accounts
                    .get_mut(&0)
                    .expect("BIP44 account 0")
                    .transactions_mut()
                    .insert(record.txid, record);
                inner.load_history_for(vec![id]).await
            }))
            .unwrap();
    }
}

/// Row 71 item 1: a ProRegTx entering the history locks the collateral it
/// names, internal or external, with no masternode list.
#[test]
fn r71_a_registration_in_the_history_locks_its_collateral_without_the_list() {
    for internal in [true, false] {
        let f = fx(Library::Accept);
        let (tx, collateral) = f.foreign_registration(internal, 2);
        assert!(f.locked().is_empty());
        f.record(tx, TransactionContext::Mempool);
        f.collateral_pass();
        assert_eq!(
            f.locked(),
            HashSet::from([collateral]),
            "internal: {internal}"
        );
    }
}

/// Row 71 item 1: after a load (session open or rescan), before any list.
#[test]
fn r71_a_load_locks_both_collateral_shapes_without_the_list() {
    let f = fx(Library::Accept);
    let (internal, a) = f.foreign_registration(true, 2);
    let (external, b) = f.foreign_registration(false, 3);
    f.load_with(internal);
    f.load_with(external);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([a, b]));
}

/// Row 71 item 2: a wallet skipped because a build holds its coin fence is
/// locked on the next poll, with no new block or list height, from either
/// path.
#[test]
fn r71_a_fence_skip_is_retried_on_the_next_poll() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(true, 2);
    f.confirm(tx, 7);
    let fence = f.fence();
    f.collateral_pass();
    assert!(f.locked().is_empty(), "the build holds the fence");
    drop(fence);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]), "history path");

    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 3);
    f.confirm(tx, 7);
    f.clear_pending();
    let fence = f.fence();
    f.follow();
    assert!(f.locked().is_empty(), "the build holds the fence");
    drop(fence);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]), "list-tick path");
}

/// A lock step that fails is retried on the next poll.
#[test]
fn r71_a_failed_lock_step_is_retried_on_the_next_poll() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(true, 2);
    f.confirm(tx, 7);
    f.crash_at(Some(CrashPoint::LockStep));
    f.collateral_pass();
    assert!(f.locked().is_empty());
    f.crash_at(None);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 item 3: a failed earlier step of the follower pass skips no
/// collateral lock, for this wallet or a later one.
#[test]
fn r71_a_failed_earlier_step_skips_no_collateral_lock() {
    for point in [CrashPoint::Settle, CrashPoint::SharedFollow] {
        let f = fx(Library::Accept);
        let other = f.import_other_wallet();
        let (tx, a) = f.internal_registration_in(f.wallet, 2);
        let (other_tx, b) = f.internal_registration_in(other, 3);
        f.confirm(tx, 7);
        f.confirm_in(other, other_tx, 7);
        f.clear_pending();
        f.crash_at(Some(point));
        f.follow();
        assert_eq!(f.locked(), HashSet::from([a]), "{point:?}");
        assert_eq!(
            f.locked_in(other),
            HashSet::from([b]),
            "{point:?}, the other wallet"
        );
    }
}

/// A ProRegTx the history keeps out of sight (a DashPay contact's record),
/// internal or external: the follower's lock step locks its collateral
/// with no coin read, the user's release frees it for good, and a re-lock
/// locks it again, all by its hash.
#[test]
fn r71_a_contacts_registration_collateral_is_locked_and_released_by_its_hash() {
    use key_wallet::account::AccountType;
    for internal in [true, false] {
        let f = fx(Library::Accept);
        let (tx, collateral) = f.foreign_registration(internal, 2);
        let record = tx_record_on(
            tx.clone(),
            AccountType::DashpayExternalAccount {
                index: 0,
                user_identity_id: [0xaa; 32],
                friend_identity_id: [0xbb; 32],
            },
            chain_locked(7),
        );
        (f.session.hub.history).with_wallet(f.wallet, |h| h.upsert(&record, None));
        assert!(
            (f.session.hub.history).read(&f.wallet, |txs| !txs.contains_key(&tx.txid())),
            "internal: {internal}"
        );
        f.collateral_pass();
        assert_eq!(
            f.locked(),
            HashSet::from([collateral]),
            "internal: {internal}"
        );
        f.engine
            .block_on(f.mn().release_collateral(f.wallet, tx.txid().to_string()))
            .unwrap();
        assert!(f.locked().is_empty(), "internal: {internal}");
        f.collateral_pass();
        assert!(f.locked().is_empty(), "internal: {internal}, released");
        f.engine
            .block_on(f.mn().lock_collateral(f.wallet, tx.txid().to_string()))
            .unwrap();
        assert_eq!(
            f.locked(),
            HashSet::from([collateral]),
            "internal: {internal}"
        );
    }
}

/// A ProRegTx the history keeps out of sight (a DashPay contact's record),
/// internal or external, locks its collateral in a coin read alone: the
/// read holds the wallet's fence from before the record arrives, so no
/// follower pass can lock it first. After the user's release, a read
/// leaves it unlocked.
#[test]
fn r71_a_coin_read_alone_locks_a_contacts_registration_collateral() {
    for internal in [true, false] {
        let f = fx(Library::Accept);
        let (tx, collateral) = f.foreign_registration(internal, 2);
        let fence = f.fence();
        let record = tx_record_on(tx.clone(), contact_account(), chain_locked(7));
        (f.session.hub.history).with_wallet(f.wallet, |h| h.upsert(&record, None));
        f.collateral_pass();
        assert!(
            f.locked().is_empty(),
            "internal: {internal}: the follower waits"
        );
        assert_eq!(
            f.fenced(&fence),
            vec![(collateral, true)],
            "internal: {internal}: the coin read locks it"
        );
        assert_eq!(
            f.locked(),
            HashSet::from([collateral]),
            "internal: {internal}"
        );
        drop(fence);
        f.engine
            .block_on(f.mn().release_collateral(f.wallet, tx.txid().to_string()))
            .unwrap();
        let fence = f.fence();
        assert_eq!(
            f.fenced(&fence),
            vec![(collateral, false)],
            "internal: {internal}: released"
        );
        drop(fence);
        assert!(f.locked().is_empty(), "internal: {internal}: released");
    }
}

/// A ProRegTx kept out of sight goes with a sweep that upstream deleted it
/// in: nothing names its collateral after the event.
#[test]
fn r71_a_swept_registration_kept_out_of_sight_is_forgotten() {
    use key_wallet::wallet::balance::WalletCoreBalance;
    use platform_wallet::events::WalletEvent;
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(true, 2);
    let record = tx_record_on(tx.clone(), contact_account(), chain_locked(7));
    (f.session.hub.history).with_wallet(f.wallet, |h| h.upsert(&record, None));
    let named = || {
        (f.session
            .hub
            .history
            .registration_collaterals(&f.wallet)
            .into_iter())
        .map(|(o, _)| o)
        .collect::<Vec<_>>()
    };
    assert_eq!(named(), vec![collateral]);
    f.session
        .hub
        .apply_wallet_event(&WalletEvent::TransactionsSwept {
            wallet_id: f.wallet.0,
            txids: vec![tx.txid()],
            superseded_by: Txid::all_zeros(),
            winner_block: None,
            released_outpoints: Vec::new(),
            balance: WalletCoreBalance::default(),
            account_balances: std::collections::BTreeMap::new(),
        });
    assert!(named().is_empty());
}

fn contact_account() -> key_wallet::account::AccountType {
    key_wallet::account::AccountType::DashpayExternalAccount {
        index: 0,
        user_identity_id: [0xaa; 32],
        friend_identity_id: [0xbb; 32],
    }
}

/// Row 71 item 4: the user's release holds against every new lock path.
#[test]
fn r71_a_release_holds_on_every_new_path() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    f.confirm(tx.clone(), 7);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
    f.engine
        .block_on(f.mn().release_collateral(f.wallet, tx.txid().to_string()))
        .unwrap();
    assert!(f.locked().is_empty());

    f.confirm(tx.clone(), 7);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "history insert");

    f.load_with(tx);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "load");

    let fence = f.fence();
    f.follow();
    drop(fence);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "fence re-arm");

    f.crash_at(Some(CrashPoint::LockStep));
    f.collateral_pass();
    f.crash_at(Some(CrashPoint::Settle));
    f.follow();
    f.crash_at(None);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "past failed steps");
}

/// Row 71 item 1's wake: a merged ProRegTx marks its wallet and wakes the
/// follower once; a plain transaction, a repeat or a re-arm doesn't.
#[test]
fn r71_a_registration_marks_its_wallet_and_wakes_the_follower_once() {
    use crate::masternodes::aggregate::fixtures::{coins, null, registration};
    use futures_util::FutureExt;
    let store = crate::history::HistoryStore::default();
    let wallet = WalletId([7; 32]);
    let wake = store.collateral_wake();
    let woken = || wake.notified().now_or_never().is_some();
    let plain = tx_record(
        Transaction {
            version: 2,
            lock_time: 0,
            input: Vec::new(),
            output: coins(&[COIN]),
            special_transaction_payload: None,
        },
        TransactionContext::Mempool,
    );
    store.with_wallet(wallet, |h| h.upsert(&plain, None));
    assert!(!woken(), "a plain transaction");
    assert!(store.take_collateral_pending().is_empty());

    let reg = tx_record(
        registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL])),
        TransactionContext::Mempool,
    );
    store.with_wallet(wallet, |h| h.upsert(&reg, None));
    assert!(woken());
    store.with_wallet(wallet, |h| h.upsert(&reg, None));
    assert!(!woken(), "already pending");
    assert_eq!(store.take_collateral_pending(), vec![wallet]);
    assert!(store.take_collateral_pending().is_empty());

    store.rearm_collateral(wallet);
    assert!(!woken(), "a re-arm waits for the next poll");
    assert_eq!(store.take_collateral_pending(), vec![wallet]);
}

const SEND: u64 = 10_000_000;

impl Fx {
    /// A send of [`SEND`] to a foreign address, prepared: signed, its
    /// inputs reserved.
    fn prepare_send(&self) -> Result<Vec<OutPoint>, crate::EngineError> {
        let grant = self.spend_grant(SEND + 100_000);
        let draft = self.session.new_tx_draft(self.wallet).unwrap();
        draft.set_recipients(vec![pay(FOREIGN, SEND)]).unwrap();
        let p = self.engine.block_on(draft.prepare(grant))?;
        let inputs = p.summary().inputs.iter().map(|i| i.outpoint).collect();
        self.engine.block_on(draft.abandon(p)).unwrap();
        Ok(inputs)
    }

    /// The coins a build reads under `fence`, with whether each is locked.
    fn fenced(&self, fence: &crate::coins::CoinFence) -> Vec<(OutPoint, bool)> {
        let snapshot = self
            .engine
            .block_on(self.session.fenced_coins(fence))
            .unwrap();
        (snapshot.coins.iter())
            .map(|c| (c.utxo.outpoint, c.user_locked))
            .collect()
    }

    /// Waits up to two minutes (a loaded machine) for `outpoint` to be
    /// locked.
    fn wait_locked(&self, outpoint: OutPoint, what: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !self.locked().contains(&outpoint) {
            assert!(
                std::time::Instant::now() < deadline,
                "{what}: not locked by the follower"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Row 71 F1: a build never selects a coin a ProRegTx in the history names,
/// though the follower hasn't locked it (its mark taken, its lock not yet
/// written): inserted or loaded, internal or external collateral, read
/// under a build's fence; and a send (external, the coin at a wallet
/// address, so it could sign) refused.
#[test]
fn r71_b_a_send_never_selects_a_known_collateral() {
    for internal in [true, false] {
        for load in [false, true] {
            let what = format!("internal: {internal}, load: {load}");
            let f = fx(Library::Accept);
            let (tx, collateral) = f.foreign_registration(internal, 2);
            if load {
                f.load_with(tx);
            } else {
                f.record(tx, TransactionContext::Mempool);
            }
            f.clear_pending();
            assert!(f.locked().is_empty(), "{what}");
            let fence = f.fence();
            assert_eq!(f.fenced(&fence), vec![(collateral, true)], "{what}");
            drop(fence);
            assert_eq!(f.locked(), HashSet::from([collateral]), "{what}");
        }
    }
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    f.record(tx, TransactionContext::Mempool);
    f.clear_pending();
    assert_eq!(
        send_failure(f.prepare_send()),
        SendFailure::AmountExceedsBalance { available: 0 },
        "a send"
    );
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 F1: a collateral the user released stays selectable: a build's
/// read leaves it unlocked, and a send spends it (the external shape's coin
/// is at a wallet address, so the send can sign).
#[test]
fn r71_b_a_released_collateral_stays_selectable() {
    for internal in [true, false] {
        let f = fx(Library::Accept);
        let (tx, collateral) = f.foreign_registration(internal, 2);
        f.record(tx.clone(), TransactionContext::Mempool);
        f.collateral_pass();
        assert_eq!(f.locked(), HashSet::from([collateral]));
        f.engine
            .block_on(f.mn().release_collateral(f.wallet, tx.txid().to_string()))
            .unwrap();
        let fence = f.fence();
        assert_eq!(
            f.fenced(&fence),
            vec![(collateral, false)],
            "internal: {internal}"
        );
        drop(fence);
        if !internal {
            assert_eq!(f.prepare_send().unwrap(), vec![collateral]);
        }
        assert!(f.locked().is_empty(), "internal: {internal}");
    }
}

/// Row 71 F1: the lock applies to every read of the coins after the
/// ProRegTx entered the history, a read under a fence held across the
/// insert included; a read from before it isn't undone, as a transaction
/// Core built before the ProRegTx was added isn't.
#[test]
fn r71_b_a_build_holding_the_fence_across_the_insert() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    let fence = f.fence();
    assert_eq!(f.fenced(&fence), vec![(collateral, false)], "before");
    f.record(tx, TransactionContext::Mempool);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "the follower waits for the fence");
    assert_eq!(f.fenced(&fence), vec![(collateral, true)], "after");
    drop(fence);
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 F2: a collateral coin that arrives after its ProRegTx (a restore
/// past the gap) marks the wallet, and the follower locks it with no list.
#[test]
fn r71_b_a_collateral_coin_after_its_registration_is_locked() {
    use crate::masternodes::aggregate::fixtures::{coins, registration};
    use key_wallet::managed_account::transaction_record::{OutputDetail, OutputRole};
    let f = fx(Library::Accept);
    let funding = Transaction {
        version: 2,
        lock_time: 0,
        input: Vec::new(),
        output: coins(&[MASTERNODE_COLLATERAL]),
        special_transaction_payload: None,
    };
    let collateral = OutPoint::new(funding.txid(), 0);
    f.record(
        registration(collateral, false, 0, coins(&[COIN])),
        chain_locked(7),
    );
    f.collateral_pass();
    assert!(f.locked().is_empty(), "the coin isn't the wallet's yet");

    f.hold(collateral, funding.output[0].clone());
    let mut record = tx_record(funding.clone(), chain_locked(5));
    record.output_details = vec![OutputDetail {
        index: 0,
        role: OutputRole::Received,
        address: None,
        value: MASTERNODE_COLLATERAL,
    }];
    (f.session.hub.history).with_wallet(f.wallet, |h| h.upsert(&record, None));
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 nit: "Unlock unspent" doesn't free a collateral; the next build
/// locks it again (only `release-collateral` does).
#[test]
fn r71_b_a_generic_unlock_of_a_collateral_is_locked_again() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    f.record(tx, TransactionContext::Mempool);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
    f.engine
        .block_on(f.session.unlock_outpoints(f.wallet, vec![collateral]))
        .unwrap();
    assert!(f.locked().is_empty());
    assert_eq!(
        send_failure(f.prepare_send()),
        SendFailure::AmountExceedsBalance { available: 0 }
    );
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 F3: with no test seam and no list, the session's own follower
/// locks a ProRegTx's collateral after a real insert and a real load. The
/// poll is stretched to an hour, so the load's lock can only come from the
/// history's wake. Assumes nothing else holds the coin fence: a step that
/// finds it held re-arms with no wake and waits for the poll.
#[test]
fn r71_b_the_follower_locks_through_the_wake_with_no_list() {
    use platform_wallet::events::WalletEvent;
    let f = fx(Library::Accept);
    f.session.masternodes.set_poll(3600);
    f.session.masternodes.set_protx_seam(None);

    let (inserted, a) = f.foreign_registration(false, 2);
    (f.session.hub).apply_wallet_event(&WalletEvent::TransactionDetected {
        wallet_id: f.wallet.0,
        record: Box::new(tx_record(inserted, TransactionContext::Mempool)),
        balance: Default::default(),
        account_balances: std::collections::BTreeMap::new(),
        addresses_derived: Vec::new(),
    });
    f.wait_locked(a, "insert");
    assert!(
        f.session.hub.history.take_collateral_pending().is_empty(),
        "the insert's step left the wallet pending"
    );

    let (loaded, b) = f.foreign_registration(true, 3);
    f.load_with(loaded);
    f.wait_locked(b, "load");
}

/// Row 71 round 2: only a coin holding its type's collateral amount is
/// locked (Core accepts no other), regular or evonode, internal or
/// external, from a build's read and from the follower; an abandoned
/// ProRegTx's coin isn't locked again once unlocked.
#[test]
fn r71_b_only_a_collateral_amount_is_locked() {
    use crate::masternodes::aggregate::fixtures::{coins, null, registration};
    for follower in [false, true] {
        let f = fx(Library::Accept);
        let mut expect = HashSet::new();
        let mut external = |seed: u8, value: u64, evonode: bool, locks: bool| {
            let collateral = f.credit(seed, value);
            f.record(
                registration(collateral, evonode, 0, coins(&[COIN])),
                TransactionContext::Mempool,
            );
            if locks {
                expect.insert(collateral);
            }
        };
        external(2, MASTERNODE_COLLATERAL, false, true);
        external(3, EVONODE_COLLATERAL, true, true);
        external(4, COIN, false, false);
        external(5, MASTERNODE_COLLATERAL, true, false);
        external(6, EVONODE_COLLATERAL, false, false);
        for (seed, value, evonode, locks) in [
            (7, EVONODE_COLLATERAL, true, true),
            (8, COIN, false, false),
            (9, MASTERNODE_COLLATERAL, true, false),
        ] {
            let tx = registration(null(0), evonode, seed, coins(&[value]));
            let collateral = OutPoint::new(tx.txid(), 0);
            f.hold(collateral, tx.output[0].clone());
            f.record(tx, TransactionContext::Mempool);
            if locks {
                expect.insert(collateral);
            }
        }
        if follower {
            f.collateral_pass();
        } else {
            f.clear_pending();
            drop(f.fenced(&f.fence()));
        }
        assert_eq!(f.locked(), expect, "follower: {follower}");
    }

    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    f.record(tx.clone(), TransactionContext::Mempool);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
    (f.session.hub.history).with_wallet(f.wallet, |h| {
        h.txs.get_mut(&tx.txid()).unwrap().abandoned = true;
    });
    f.engine
        .block_on(f.session.unlock_outpoints(f.wallet, vec![collateral]))
        .unwrap();
    drop(f.fenced(&f.fence()));
    f.session.hub.history.rearm_collateral(f.wallet);
    f.collateral_pass();
    assert!(f.locked().is_empty(), "abandoned");
}

/// Row 71 round 2 (Sol N2): a collateral at output 1 of a funding
/// transaction that arrives in a block after its ProRegTx is locked; the
/// other output of that transaction, a wallet coin too, isn't.
#[test]
fn r71_b_a_late_collateral_at_a_later_output_is_locked_alone() {
    use crate::masternodes::aggregate::fixtures::{coins, registration};
    use key_wallet::managed_account::transaction_record::{OutputDetail, OutputRole};
    let f = fx(Library::Accept);
    let funding = Transaction {
        version: 2,
        lock_time: 0,
        input: Vec::new(),
        output: coins(&[MASTERNODE_COLLATERAL, MASTERNODE_COLLATERAL]),
        special_transaction_payload: None,
    };
    let (decoy, collateral) = (
        OutPoint::new(funding.txid(), 0),
        OutPoint::new(funding.txid(), 1),
    );
    f.record(
        registration(collateral, false, 0, coins(&[COIN])),
        chain_locked(7),
    );
    f.collateral_pass();
    assert!(f.locked().is_empty());

    f.hold(decoy, funding.output[0].clone());
    f.hold(collateral, funding.output[1].clone());
    let mut record = tx_record(funding, chain_locked(5));
    record.output_details = [0, 1]
        .map(|index| OutputDetail {
            index,
            role: OutputRole::Received,
            address: None,
            value: MASTERNODE_COLLATERAL,
        })
        .to_vec();
    f.block_event(vec![record]);
    f.collateral_pass();
    assert_eq!(f.locked(), HashSet::from([collateral]));
}

/// Row 71 round 2 (Opus n2): a coin read leaves an existing lock row on a
/// collateral as it is, its reason and time included.
#[test]
fn r71_b_a_coin_read_leaves_an_existing_collateral_lock_alone() {
    let f = fx(Library::Accept);
    let (tx, collateral) = f.foreign_registration(false, 2);
    f.record(tx, TransactionContext::Mempool);
    f.clear_pending();
    let (w, txid) = (f.wallet.to_string(), collateral.txid.to_string());
    assert!(f.db().lock_dust(&w, &txid, collateral.vout, 1).unwrap());
    let row = || {
        let rows = f.db().locks(&w).unwrap();
        assert_eq!(rows.len(), 1);
        (rows[0].reason, rows[0].created_at)
    };
    let before = row();
    drop(f.fenced(&f.fence()));
    assert_eq!(row(), before);
}

impl Fx {
    /// A transaction paying `value` to a fresh BIP44 receive address, its
    /// output held by the wallet and the transaction not yet in the history.
    fn paid(&self, value: u64) -> (Transaction, OutPoint) {
        let session = Arc::clone(&self.session);
        let (id, inner) = (self.wallet, Arc::clone(&session));
        let address = self
            .engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                Ok(wallet.core().next_receive_address_for_account(0).await?)
            }))
            .unwrap();
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: Vec::new(),
            output: vec![TxOut {
                value,
                script_pubkey: address.script_pubkey(),
            }],
            special_transaction_payload: None,
        };
        let outpoint = OutPoint::new(tx.txid(), 0);
        self.hold(outpoint, tx.output[0].clone());
        (tx, outpoint)
    }

    /// The coins a build reads under a fresh fence: each with whether it is
    /// locked and whether it is reserved.
    fn held(&self) -> BTreeSet<(OutPoint, bool, bool)> {
        let fence = self.fence();
        let snapshot = self
            .engine
            .block_on(self.session.fenced_coins(&fence))
            .unwrap();
        (snapshot.coins.iter())
            .map(|c| (c.utxo.outpoint, c.user_locked, c.reserved))
            .collect()
    }

    /// `inserted` in a block, through the engine's handling of a
    /// `BlockProcessed` event built here (key-wallet's own block processing
    /// doesn't run).
    fn block_event(
        &self,
        inserted: Vec<key_wallet::managed_account::transaction_record::TransactionRecord>,
    ) {
        use platform_wallet::events::WalletEvent;
        (self.session.hub).apply_wallet_event(&WalletEvent::BlockProcessed {
            wallet_id: self.wallet.0,
            height: 8,
            chain_lock: None,
            inserted,
            updated: Vec::new(),
            matured: Vec::new(),
            balance: Default::default(),
            account_balances: std::collections::BTreeMap::new(),
            addresses_derived: Vec::new(),
        });
    }
}

/// Row 71 F4: a collateral-sized coin credited after the history loaded is
/// reserved until its transaction's event reaches the history; then a
/// ProRegTx's own collateral is locked and a plain transaction's coin is
/// spendable. Both masternode types, a block and a ChainLocked block.
#[test]
fn r71_c_a_collateral_sized_coin_waits_for_its_transaction() {
    use crate::masternodes::aggregate::fixtures::{coins, null, registration};
    use key_wallet::transaction_checking::BlockInfo;
    for evonode in [false, true] {
        for locked in [false, true] {
            let what = format!("evonode: {evonode}, ChainLocked: {locked}");
            let value = if evonode {
                EVONODE_COLLATERAL
            } else {
                MASTERNODE_COLLATERAL
            };
            let context = if locked {
                chain_locked(7)
            } else {
                TransactionContext::InBlock(BlockInfo::new(7, dashcore::BlockHash::all_zeros(), 0))
            };
            let f = fx(Library::Accept);
            let reg = registration(null(0), evonode, 3, coins(&[value]));
            let collateral = OutPoint::new(reg.txid(), 0);
            f.hold(collateral, reg.output[0].clone());
            let (plain, coin) = f.paid(value);
            assert_eq!(
                f.held(),
                BTreeSet::from([(collateral, false, true), (coin, false, true)]),
                "{what}: a read before the event"
            );
            assert_eq!(
                send_failure(f.prepare_send()),
                SendFailure::AmountExceedsBalance { available: 0 },
                "{what}: a send before the event"
            );
            assert!(f.locked().is_empty(), "{what}");

            f.block_event(vec![
                tx_record(reg, context.clone()),
                tx_record(plain, context),
            ]);
            f.clear_pending();
            let after = f.held();
            assert!(after.contains(&(coin, false, false)), "{what}: {after:?}");
            assert!(
                after
                    .iter()
                    .any(|&(o, locked, _)| o == collateral && locked),
                "{what}: {after:?}"
            );
            assert_eq!(f.locked(), HashSet::from([collateral]), "{what}");
            assert_eq!(
                f.prepare_send().unwrap(),
                vec![coin],
                "{what}: a send after"
            );
        }
    }
}

/// Row 71 F4: only a collateral amount waits for its transaction: a coin of
/// any other amount whose transaction the history doesn't hold yet is
/// spendable at once.
#[test]
fn r71_c_a_coin_of_another_amount_needs_no_history_entry() {
    let f = fx(Library::Accept);
    let coins = [
        f.credit_unseen(2, MASTERNODE_COLLATERAL - 1),
        f.credit_unseen(3, MASTERNODE_COLLATERAL + 1),
        f.credit_unseen(4, EVONODE_COLLATERAL + 1),
    ];
    let expect: BTreeSet<_> = coins.iter().map(|&o| (o, false, false)).collect();
    assert_eq!(f.held(), expect);
    let inputs = f.prepare_send().unwrap();
    assert_eq!(inputs.len(), 1);
    assert!(coins.contains(&inputs[0]), "{inputs:?}");
}

/// Row 71 F5: an abandoned ProRegTx seen again in a block, or with an
/// InstantSend lock, counts again (the history's status rule, `status_of`):
/// its own-output collateral is locked by a coin read and by the follower,
/// and a send refuses it. The user's abandon stays recorded.
#[test]
fn r71_d_an_abandoned_registration_seen_again_locks_its_collateral() {
    use key_wallet::transaction_checking::BlockInfo;
    let block =
        || TransactionContext::InBlock(BlockInfo::new(9, dashcore::BlockHash::all_zeros(), 0));
    let instant = || TransactionContext::InstantSend(Default::default());
    for follower in [false, true] {
        for (seen_again, context) in [("block", block()), ("InstantSend", instant())] {
            let what = format!("{seen_again}, follower: {follower}");
            let f = fx(Library::Accept);
            let (reg, collateral) = f.internal_registration_in(f.wallet, 3);
            f.record(reg.clone(), TransactionContext::Mempool);
            (f.session.hub.history).with_wallet(f.wallet, |h| {
                h.txs.get_mut(&reg.txid()).unwrap().abandoned = true;
            });
            f.clear_pending();
            drop(f.held());
            assert!(f.locked().is_empty(), "{what}: abandoned");

            f.block_event(vec![tx_record(reg.clone(), context)]);
            if follower {
                f.collateral_pass();
            } else {
                f.clear_pending();
                drop(f.held());
            }
            assert_eq!(f.locked(), HashSet::from([collateral]), "{what}");
            assert_eq!(
                send_failure(f.prepare_send()),
                SendFailure::AmountExceedsBalance { available: 0 },
                "{what}: a send"
            );
            assert!(
                f.session
                    .hub
                    .history
                    .get(&f.wallet, &reg.txid())
                    .unwrap()
                    .abandoned,
                "{what}: the abandon stays"
            );
        }
    }
}
