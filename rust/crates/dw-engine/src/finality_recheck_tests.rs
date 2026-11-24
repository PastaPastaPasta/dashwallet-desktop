//! An older build's InstantSend-locked asset locks and confirmed DashPay
//! payments, stored on a lock that is not stored as verified, come back
//! from the store on the next open as broadcast and pending; those a block
//! or a verified lock backs, and those of a transaction the store has no
//! record of, stay. Through the production session, store and wallet load.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::asset_lock::AssetLockPayload;
use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::hashes::Hash;
use dashcore::{BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dpp::identity::Identity;
use dpp::identity::state_transition::asset_lock_proof::InstantAssetLockProof;
use dpp::identity::v0::IdentityV0;
use dpp::prelude::{AssetLockProof, Identifier};
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::managed_account::transaction_record::{TransactionDirection, TransactionRecord};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
use key_wallet::wallet::managed_wallet_info::asset_lock_builder::AssetLockFundingType;
use platform_wallet::changeset::{
    AssetLockChangeSet, AssetLockEntry, CoreChangeSet, PlatformWalletChangeSet,
    PlatformWalletPersistence,
};
use platform_wallet::wallet::asset_lock::tracked::AssetLockStatus;
use platform_wallet::wallet::identity::{PaymentDirection, PaymentEntry, PaymentStatus};
use zeroize::Zeroizing;

use crate::session::{NetworkSession, WALLET_DB_FILE};
use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, SessionOptions, WalletId};

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

struct Quiet;
impl crate::EventSink for Quiet {
    fn emit(&self, _: crate::EngineEvent) {}
}

fn offline() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap()
}

fn close(engine: &Engine, session: Arc<NetworkSession>) {
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

fn engine_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>, WalletId) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: dw_vault::VaultConfig {
                kdf: dw_vault::KdfPolicy::Fixed(dw_vault::KdfParams::TEST),
                os_store: Arc::new(dw_vault::MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(Quiet),
    )
    .unwrap();
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    (dir, engine, session, id)
}

/// An asset-lock transaction, told apart by `n`.
fn lock_tx(n: u8) -> Transaction {
    Transaction {
        version: 3,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: vec![TxOut {
                    value: 1_000_000,
                    script_pubkey: ScriptBuf::new(),
                }],
            },
        )),
    }
}

fn record(tx: &Transaction, context: TransactionContext) -> TransactionRecord {
    TransactionRecord::new(
        tx.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
        TransactionType::Standard,
        TransactionDirection::Outgoing,
        Vec::new(),
        Vec::new(),
        1_000_000,
    )
}

fn islock(tx: &Transaction) -> InstantLock {
    InstantLock {
        txid: tx.txid(),
        ..InstantLock::default()
    }
}

fn entry(tx: &Transaction, status: AssetLockStatus) -> AssetLockEntry {
    AssetLockEntry {
        out_point: OutPoint::new(tx.txid(), 0),
        transaction: tx.clone(),
        account_index: 0,
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index: 0,
        amount_duffs: 1_000_000,
        status,
        proof: Some(AssetLockProof::Instant(InstantAssetLockProof::new(
            islock(tx),
            tx.clone(),
            0,
        ))),
    }
}

fn me() -> Identifier {
    Identifier::from([0x4D; 32])
}

fn sent(status: PaymentStatus) -> PaymentEntry {
    PaymentEntry {
        counterparty_id: Identifier::from([0x5E; 32]),
        amount_duffs: 1_000,
        memo: None,
        direction: PaymentDirection::Sent,
        status,
    }
}

struct Seen {
    locks: BTreeMap<OutPoint, (AssetLockStatus, bool)>,
    payments: BTreeMap<String, PaymentStatus>,
}

fn seen(engine: &Engine, session: &NetworkSession, id: WalletId) -> Seen {
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        let state = wallet.state().await;
        Seen {
            locks: (state.tracked_asset_locks.iter())
                .map(|(op, l)| (*op, (l.status.clone(), l.proof.is_some())))
                .collect(),
            payments: (state.identity_manager.managed_identity(&me()))
                .map(|m| {
                    (m.dashpay().payments.iter())
                        .map(|(t, e)| (t.clone(), e.status))
                        .collect()
                })
                .unwrap_or_default(),
        }
    })
}

fn stored_status(session: &NetworkSession, id: WalletId, op: OutPoint) -> AssetLockStatus {
    session
        .live()
        .unwrap()
        .persister
        .asset_lock_status(&id.0, &op)
        .unwrap()
        .unwrap()
}

#[test]
fn a_lock_status_without_verified_evidence_is_put_back_on_the_next_open() {
    let (_dir, engine, session, id) = engine_wallet();
    let unproven = lock_tx(0xA1);
    let mined = lock_tx(0xA2);
    let verified = lock_tx(0xA3);
    let no_record = lock_tx(0xA4);
    let consumed = lock_tx(0xA5);
    let other_proof = lock_tx(0xA6);
    let dropped = lock_tx(0xA7);
    let at = |tx: &Transaction| OutPoint::new(tx.txid(), 0);

    let live = session.live().unwrap();
    let mut locks = BTreeMap::new();
    for tx in [
        &unproven,
        &mined,
        &verified,
        &no_record,
        &other_proof,
        &dropped,
    ] {
        locks.insert(at(tx), entry(tx, AssetLockStatus::InstantSendLocked));
    }
    // A proof cached from a lock other than the one stored as verified.
    let stale = InstantLock {
        txid: other_proof.txid(),
        cyclehash: BlockHash::from_byte_array([0x07; 32]),
        ..InstantLock::default()
    };
    locks.get_mut(&at(&other_proof)).unwrap().proof = Some(AssetLockProof::Instant(
        InstantAssetLockProof::new(stale, other_proof.clone(), 0),
    ));
    locks.insert(at(&consumed), entry(&consumed, AssetLockStatus::Consumed));
    let in_block = TransactionContext::InBlock(BlockInfo::new(
        600,
        BlockHash::from_byte_array([0x0B; 32]),
        1_700_000_000,
    ));
    live.persister
        .store(
            id.0,
            PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    records: vec![
                        record(
                            &unproven,
                            TransactionContext::InstantSend(islock(&unproven)),
                        ),
                        record(&mined, in_block),
                        record(
                            &verified,
                            TransactionContext::InstantSend(islock(&verified)),
                        ),
                        record(
                            &consumed,
                            TransactionContext::InstantSend(islock(&consumed)),
                        ),
                        record(
                            &other_proof,
                            TransactionContext::InstantSend(islock(&other_proof)),
                        ),
                        record(&dropped, TransactionContext::InstantSend(islock(&dropped))),
                    ],
                    instant_locks_for_non_final_records: [
                        &unproven,
                        &verified,
                        &consumed,
                        &other_proof,
                    ]
                    .into_iter()
                    .map(|tx| (tx.txid(), islock(tx)))
                    .collect(),
                    ..Default::default()
                }),
                asset_locks: Some(AssetLockChangeSet {
                    asset_locks: locks,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    live.persister.flush(id.0).unwrap();
    let manager = session.manager().unwrap();
    let payments = [
        &unproven,
        &mined,
        &verified,
        &no_record,
        &other_proof,
        &dropped,
    ]
    .map(|tx| tx.txid().to_string());
    let to_record = payments.clone();
    engine.block_on(async move {
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        let mut state = wallet.state_mut().await;
        let identity = Identity::V0(IdentityV0 {
            id: me(),
            public_keys: Default::default(),
            balance: 0,
            revision: 0,
        });
        (state.identity_manager)
            .add_identity(identity, 0, id.0, wallet.persister())
            .unwrap();
        let m = state.identity_manager.managed_identity_mut(&me()).unwrap();
        for txid in to_record {
            m.record_dashpay_payment(txid, sent(PaymentStatus::Confirmed), wallet.persister())
                .unwrap();
        }
    });
    live.persister.flush(id.0).unwrap();
    drop(live);
    let db_path = session.data_dir().join(WALLET_DB_FILE);
    close(&engine, session);

    // What an older build left: its locks were never verified.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    for tx in [&unproven, &consumed] {
        let changed = conn
            .execute(
                "UPDATE core_instant_locks SET verified = 0 WHERE txid = ?1",
                [AsRef::<[u8]>::as_ref(&tx.txid())],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }
    // A spend the store's open dropped as unsigned: kept aside, its rows gone.
    let dropped_txid = AsRef::<[u8]>::as_ref(&dropped.txid()).to_vec();
    conn.execute(
        "INSERT INTO core_dropped_unsigned_spends \
            (wallet_id, txid, record_blob, deleted_outputs, released_coins) \
         SELECT wallet_id, txid, record_blob, '[]', '[]' FROM core_transactions WHERE txid = ?1",
        [&dropped_txid],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM core_transactions WHERE txid = ?1",
        [&dropped_txid],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM core_instant_locks WHERE txid = ?1",
        [&dropped_txid],
    )
    .unwrap();
    drop(conn);

    let session = open(&engine);
    let after = seen(&engine, &session, id);
    for tx in [&unproven, &mined, &other_proof, &dropped] {
        assert_eq!(
            after.locks[&at(tx)],
            (AssetLockStatus::Broadcast, false),
            "{}: no verified lock matching its proof, so it goes back to broadcast without it",
            tx.txid()
        );
        assert_eq!(
            stored_status(&session, id, at(tx)),
            AssetLockStatus::Broadcast
        );
    }
    for tx in [&verified, &no_record] {
        assert_eq!(
            after.locks[&at(tx)],
            (AssetLockStatus::InstantSendLocked, true)
        );
    }
    assert_eq!(
        stored_status(&session, id, at(&verified)),
        AssetLockStatus::InstantSendLocked
    );
    assert_eq!(
        stored_status(&session, id, at(&consumed)),
        AssetLockStatus::Consumed,
        "a consumed lock is not tracked and stays consumed"
    );
    for txid in [&payments[0], &payments[5]] {
        assert_eq!(after.payments[txid], PaymentStatus::Pending, "{txid}");
    }
    for txid in &payments[1..5] {
        assert_eq!(after.payments[txid], PaymentStatus::Confirmed, "{txid}");
    }

    close(&engine, session);
    let session = open(&engine);
    let again = seen(&engine, &session, id);
    assert_eq!(again.locks, after.locks, "the store kept them");
    assert_eq!(again.payments, after.payments);
}
