//! The ProTx owner flows end to end on a regtest session (MG-04 §7): a
//! `MasternodeOp` lease, the vault's `MasternodeOwner` signer, the dispatch
//! fence and `masternode_ops`, with the library's broadcast and the SPV list
//! faked through the hand-off's seam. The crash tests stop the hand-off at
//! each point between the save and the record step, restart the session on
//! the same data, and check that nothing is sent twice, nothing is lost and
//! no row whose transaction may be out is deleted.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::hashes::Hash;
use dashcore::{OutPoint, Transaction, TxOut, Txid};
use dw_protx::params::MASTERNODE_COLLATERAL;
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use platform_wallet::PlatformWalletError;
use zeroize::Zeroizing;

use super::ops::{CrashPoint, Seam};
use crate::masternodes::records::MasternodeType;
use crate::masternodes::{
    CollateralChoice, Masternodes, MasternodesError, OperatorChoice, PayoutChoice, ProTxOperation,
    ProTxOperationState, RegistrationRequest, VotingChoice,
};
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
        self.session
            .vault()
            .authorize(
                GrantPurpose::MasternodeOp { max_duffs },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    /// A confirmed coin `seed` of `value` at a fresh BIP44 receive address
    /// (the same outpoint for the same seed after a restart).
    fn credit(&self, seed: u8, value: u64) -> OutPoint {
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
        let session = Arc::clone(&self.session);
        let id = self.wallet;
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

    /// `tx` in a block of the wallet's history, as key-wallet records it.
    fn confirm(&self, tx: Transaction, height: u32) {
        use key_wallet::account::{AccountType, StandardAccountType};
        use key_wallet::managed_account::transaction_record::{
            TransactionDirection, TransactionRecord,
        };
        use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
        let record = TransactionRecord::new(
            tx,
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            TransactionContext::InChainLockedBlock(BlockInfo::new(
                height,
                dashcore::BlockHash::all_zeros(),
                0,
            )),
            TransactionType::Standard,
            TransactionDirection::Internal,
            Vec::new(),
            Vec::new(),
            0,
        );
        (self.session.hub.history).with_wallet(self.wallet, |h| h.upsert(&record, None));
    }

    fn follow(&self) {
        self.engine
            .block_on(self.session.protx_follow_pass())
            .unwrap();
    }

    fn locked(&self) -> HashSet<OutPoint> {
        self.db()
            .locks(&self.wallet.to_string())
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
    assert!(
        !f.locked()
            .iter()
            .any(|o| o.txid.to_string() == op.txid.clone().unwrap()),
        "no collateral lock for a transaction that never left"
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
        }
        // The saved inputs stay locked across the restart: a new
        // registration can't spend them in a second transaction.
        let first_inputs: HashSet<OutPoint> = f
            .rows()
            .iter()
            .filter_map(super::ops::signed_tx)
            .flat_map(|tx| tx.input.into_iter().map(|i| i.previous_output))
            .collect();
        let second = f
            .register(request(CollateralChoice::FundNew), f.grant(COIN))
            .unwrap();
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
        matches!(preview(port), MasternodesError::ProtxRefused { reason } if reason == "bad-protx-netinfo-port")
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
    }]);
    let mut given = request(CollateralChoice::FundNew);
    given.operator = OperatorChoice::Given {
        public_key: hex::encode(operator),
    };
    assert!(
        matches!(preview(given), MasternodesError::ProtxRefused { reason } if reason == "bad-protx-dup-key")
    );
    let mut dup_service = request(CollateralChoice::FundNew);
    dup_service.service = "1.2.3.5".into();
    assert!(
        matches!(preview(dup_service), MasternodesError::ProtxRefused { reason } if reason == "bad-protx-dup-netinfo-entry")
    );
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

    let unknown = f
        .engine
        .block_on(f.mn().release_collateral(f.wallet, "11".repeat(32)))
        .unwrap_err();
    assert_eq!(unknown.code(), "masternodes.not_found");
}
