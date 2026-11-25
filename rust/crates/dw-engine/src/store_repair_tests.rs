//! The open's repair of stored unsigned pending claims (Q112): what it
//! drops (S3, S4), what it keeps (S1, S2, S6), its schema guard, its
//! records and a stop inside it.

use super::own_spend_disk_tests::{eventually, fresh_address, send, spendable};
use super::*;
use crate::send::CoinSource;
use crate::session::WALLET_DB_FILE;
use crate::store_repair::{Stop, arm, repair};

/// The wallet holds coin A, mined; a peer's claim on A paying [`CHANGE`]
/// back, with `script` as its input script, is in the store as a build
/// that recorded it left it, A spent ([`seed_claim`]). Returns the session
/// reopened on that store, A's funding and the claim.
pub(super) fn claim_on_a_held_coin(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    script: &[u8],
) -> (Arc<NetworkSession>, Transaction, Transaction) {
    edited_claim_on_a_held_coin(engine, session, id, |claim| {
        claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(script.to_vec());
    })
}

/// [`claim_on_a_held_coin`] with the claim as `edit` leaves it.
pub(super) fn edited_claim_on_a_held_coin(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    edit: impl FnOnce(&mut Transaction),
) -> (Arc<NetworkSession>, Transaction, Transaction) {
    let (to, funding, mut claim) = funding_and_spend(engine, &session, id);
    edit(&mut claim);
    mined(engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let owners = vec![to; claim.input.len()];
    let session = seed_claim(engine, session, id, &claim, &owners);
    let a = OutPoint::new(funding.txid(), 0);
    assert!(!spendable(engine, &session, id, a), "the claim holds A");
    (session, funding, claim)
}

/// Puts `claim`, a spend its coins' owner never signed, in the store as
/// the builds before key-wallet refused such spends recorded it (this
/// build's key-wallet ignores it, so it can only come from such a store):
/// its twin with each input signed for the address of `owners` at its
/// index (a repeated input once) goes through the mempool and is stored,
/// then the closed store's rows of the twin become the claim's, its record
/// holding the claim. The record keeps the twin's input details, so with
/// a repeated input their indices are the twin's; the repair reads only
/// the transaction. Returns the session reopened on that store with the
/// repair skipped, as a build without it opened it
/// ([`a_seeded_claim_is_stored_as_a_relayed_one`]).
pub(super) fn seed_claim(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    claim: &Transaction,
    owners: &[Address],
) -> Arc<NetworkSession> {
    assert_eq!(owners.len(), claim.input.len(), "an owner for each input");
    let mut seen = std::collections::HashSet::new();
    let (input, owners): (Vec<TxIn>, Vec<Address>) = (claim.input.iter().zip(owners))
        .filter(|(i, _)| seen.insert(i.previous_output))
        .map(|(i, o)| {
            let input = TxIn {
                script_sig: dashcore::ScriptBuf::new(),
                ..i.clone()
            };
            (input, o.clone())
        })
        .unzip();
    let twin = Transaction {
        input,
        ..claim.clone()
    };
    let twin = signed_by_wallet_each(engine, &session, id, &owners, twin);
    mempool(engine, &session, id, &twin);
    eventually("the twin is stored", || {
        stored(session.data_dir(), id, twin.txid())
    });
    let dir = close(engine, session);
    swap_in(&dir, &twin, claim);
    arm(&dir, Stop::Skip);
    let session = open(engine);
    assert!(stored(session.data_dir(), id, claim.txid()), "seeded");
    session
}

/// In the closed store in `dir`, every row of `twin` becomes `claim`'s
/// (same inputs and outputs, another txid): its record holds `claim`, and
/// its txid and outpoint keys name `claim`.
pub(super) fn swap_in(dir: &std::path::Path, twin: &Transaction, claim: &Transaction) {
    let mut conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch("PRAGMA defer_foreign_keys = ON").unwrap();
    let (from, to) = (
        twin.txid().as_byte_array().to_vec(),
        claim.txid().as_byte_array().to_vec(),
    );
    let blobs: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut stmt = tx
            .prepare("SELECT wallet_id, record_blob FROM core_transactions WHERE txid = ?1")
            .unwrap();
        stmt.query_map([&from], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(!blobs.is_empty(), "the twin is stored");
    for (wallet, blob) in blobs {
        let mut record = crate::history_ops::decode_record(&blob).unwrap();
        record.transaction = claim.clone();
        record.txid = claim.txid();
        let config = bincode::config::standard()
            .with_limit::<{ platform_wallet_storage::SIZE_LIMIT_BYTES }>();
        let blob = bincode::serde::encode_to_vec(&record, config).unwrap();
        tx.execute(
            "UPDATE core_transactions SET record_blob = ?3 WHERE wallet_id = ?1 AND txid = ?2",
            rusqlite::params![wallet, &from, blob],
        )
        .unwrap();
    }
    let tables: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    for table in tables {
        let columns: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT name FROM pragma_table_info(?1)")
                .unwrap();
            stmt.query_map([&table], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        for column in ["txid", "spent_in_txid"] {
            if columns.iter().any(|c| c == column) {
                tx.execute(
                    &format!("UPDATE \"{table}\" SET {column} = ?2 WHERE {column} = ?1"),
                    rusqlite::params![&from, &to],
                )
                .unwrap();
            }
        }
        if columns.iter().any(|c| c == "outpoint") {
            for vout in 0..twin.output.len() as u32 {
                tx.execute(
                    &format!("UPDATE \"{table}\" SET outpoint = ?2 WHERE outpoint = ?1"),
                    rusqlite::params![
                        crate::store_repair::encode_outpoint(&OutPoint::new(twin.txid(), vout)),
                        crate::store_repair::encode_outpoint(&OutPoint::new(claim.txid(), vout)),
                    ],
                )
                .unwrap();
            }
        }
    }
    tx.commit().unwrap();
}

/// Whether the store holds a row of `txid` for `id`.
pub(super) fn stored(dir: &std::path::Path, id: WalletId, txid: Txid) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2)",
        rusqlite::params![id.0.as_slice(), txid.as_byte_array().as_slice()],
        |r| r.get(0),
    )
    .unwrap()
}

/// Whether `txid` is out of the history or abandoned in it.
pub(super) fn dropped(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> bool {
    rows(engine, session, id)
        .iter()
        .all(|r| r.0 != txid.to_string() || r.1 == TxStatusKind::Abandoned)
}

/// The repair's records of `id`: dropped, and of those the doubtful.
fn records(
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (
    std::collections::HashSet<Txid>,
    std::collections::HashSet<Txid>,
) {
    let dir = session.data_dir().to_path_buf();
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    crate::store_repair::store_dropped(&appdb, id, |t| stored(&dir, id, *t)).unwrap()
}

/// The app's settings value of `key` in `scope`.
pub(super) fn setting(session: &Arc<NetworkSession>, scope: &str, key: &str) -> Option<String> {
    (session.live().unwrap().appdb).setting(scope, key).unwrap()
}

/// Closes the network; returns its directory.
pub(super) fn close(engine: &Engine, session: Arc<NetworkSession>) -> std::path::PathBuf {
    let dir = session.data_dir().to_path_buf();
    *session.close_load_wait.lock().unwrap() = Some(LOAD_WAIT);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    dir
}

/// Runs the repair on the closed network `dir` with its own app database.
pub(super) fn repair_closed(dir: &std::path::Path) -> crate::store_repair::Outcome {
    let appdb = dw_appdb::AppDb::open(&dir.join(dw_appdb::APP_DB_FILE)).unwrap();
    repair(dir, &appdb).unwrap()
}

/// SQL on the closed network `dir`'s store.
pub(super) fn on_store(dir: &std::path::Path, sql: &str, params: impl rusqlite::Params) {
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    conn.execute(sql, params).unwrap();
}

/// W2: the claim is recorded on a held coin and the app stops before
/// anything abandons it. The next open drops it from the store, gives A
/// back, records the drop, and A is offered.
#[test]
fn a_claim_recorded_on_a_held_coin_and_left_at_an_exit_is_dropped_at_the_next_open() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let session = reopen(&engine, session);
    let a = OutPoint::new(funding.txid(), 0);
    assert!(
        !stored(session.data_dir(), id, claim.txid()),
        "the store dropped the claim"
    );
    assert!(
        dropped(&engine, &session, id, claim.txid()),
        "the claim is gone"
    );
    let (all, doubtful) = records(&session, id);
    assert!(
        all.contains(&claim.txid()) && doubtful.is_empty(),
        "{all:?} {doubtful:?}"
    );
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert_eq!(balances(&session, id).total, PAID, "A counts once");
    let session = reopen(&engine, session);
    assert!(
        spendable(&engine, &session, id, a),
        "A is offered after a restart"
    );
    assert_eq!(balances(&session, id).total, PAID);
    assert!(
        records(&session, id).0.contains(&claim.txid()),
        "the record stays"
    );
}

/// S1: the claim's one input script isn't push-only but parses, and the
/// verifier says unsigned: kept, A stays spent, nothing is recorded.
#[test]
fn a_claim_on_a_held_coin_whose_input_script_isnt_push_only_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[0x61, 0x01, 0x01]);
    assert_eq!(
        crate::input_authorization::verify_input_authorization(&claim, 0, &funding.output[0]),
        Ok(false)
    );
    let session = reopen(&engine, session);
    assert!(
        stored(session.data_dir(), id, claim.txid()),
        "the store keeps the claim"
    );
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is listed"
    );
    let a = OutPoint::new(funding.txid(), 0);
    assert!(!spendable(&engine, &session, id, a), "A stays spent");
    assert!(records(&session, id).0.is_empty(), "nothing is recorded");
}

/// S3: one input isn't push-only (S1 alone keeps it), the other is
/// push-only and unsigned for a stored coin: the claim drops, both coins
/// come back, and the verdict is sure, so a payment built on the claim's
/// change goes with it.
#[test]
fn one_unsigned_push_only_input_drops_a_claim_whatever_the_others_are() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to_a, to_b) = (
        fresh_address(&engine, &session, id),
        fresh_address(&engine, &session, id),
    );
    let funding = super::own_spend_disk_tests::funding(5, &[to_a.clone(), to_b.clone()], PAID);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let (a, b) = (
        OutPoint::new(funding.txid(), 0),
        OutPoint::new(funding.txid(), 1),
    );
    let change = fresh_address(&engine, &session, id);
    let mut claim = spend_of(
        a,
        vec![(elsewhere(), PAID), (change.script_pubkey(), CHANGE)],
    );
    claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
    claim.input.push(TxIn {
        previous_output: b,
        ..Default::default()
    });
    let session = seed_claim(&engine, session, id, &claim, &[to_a, to_b]);
    assert!(!spendable(&engine, &session, id, a) && !spendable(&engine, &session, id, b));
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);

    let session = reopen(&engine, session);
    assert!(!stored(session.data_dir(), id, claim.txid()), "dropped");
    assert!(spendable(&engine, &session, id, a), "A is back");
    assert!(spendable(&engine, &session, id, b), "B is back");
    let (all, doubtful) = records(&session, id);
    assert!(all.contains(&claim.txid()));
    assert!(doubtful.is_empty(), "sure: {doubtful:?}");
    eventually("the payment on the claim's change goes", || {
        dropped(&engine, &session, id, payment.txid())
    });
    assert_eq!(
        balances(&session, id).total,
        2 * PAID,
        "A and B count, nothing else"
    );
}

/// S4: an input script that isn't push-only on an outpoint the claim
/// spends twice is structurally invalid: the claim drops, recorded as
/// structural and doubtful (what is built on it stays).
#[test]
fn a_claim_spending_one_coin_twice_is_dropped_as_structural() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = edited_claim_on_a_held_coin(&engine, session, id, |claim| {
        claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
        claim.input.push(claim.input[0].clone());
    });
    let session = reopen(&engine, session);
    assert!(!stored(session.data_dir(), id, claim.txid()), "dropped");
    assert!(
        spendable(&engine, &session, id, OutPoint::new(funding.txid(), 0)),
        "A is back"
    );
    let raw = setting(
        &session,
        &id.to_string(),
        &format!("store_dropped:{}", claim.txid()),
    )
    .expect("recorded");
    assert!(raw.contains("\"cause\":\"structural\""), "{raw}");
    assert!(records(&session, id).1.contains(&claim.txid()), "doubtful");
}

/// S2: a payment built on the claim's change is InstantSend-locked: the
/// claim is kept with A spent.
#[test]
fn a_locked_payment_on_the_claims_change_keeps_the_claim() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    instant_lock(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert!(stored(session.data_dir(), id, claim.txid()), "kept");
    assert!(!spendable(
        &engine,
        &session,
        id,
        OutPoint::new(funding.txid(), 0)
    ));
    assert!(records(&session, id).0.is_empty());
}

/// S2: an output of the claim held spent by what the store has no record
/// of (here a sweep's winner) keeps it: what spent it may be settled.
#[test]
fn an_output_of_the_claim_spent_by_an_unknown_transaction_keeps_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    on_store(
        &dir,
        "UPDATE core_utxos SET spent = 1, spent_in_txid = ?2 WHERE wallet_id = ?1 AND substr(outpoint, 2, 32) = ?3",
        rusqlite::params![
            id.0.as_slice(),
            [9u8; 32].as_slice(),
            claim.txid().as_byte_array().as_slice()
        ],
    );
    let outcome = repair_closed(&dir);
    assert_eq!(outcome.schema, Some(26));
    assert!(outcome.dropped.is_empty(), "{outcome:?}");
    assert!(stored(&dir, id, claim.txid()));
}

/// A claim a sweep named the winner of a coin (only a mined or locked
/// transaction wins one), or with a lock row stored unverified, is kept.
#[test]
fn a_claim_a_sweep_named_the_winner_or_with_an_unverified_lock_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    let a = crate::store_repair::encode_outpoint(&OutPoint::new(funding.txid(), 0));
    let txid = claim.txid().as_byte_array().to_vec();
    on_store(
        &dir,
        "UPDATE core_utxos SET spent_in_txid = ?3 WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), a, txid],
    );
    assert!(repair_closed(&dir).dropped.is_empty(), "a sweep's winner");
    on_store(
        &dir,
        "UPDATE core_utxos SET spent_in_txid = NULL WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), a],
    );
    on_store(
        &dir,
        "INSERT INTO core_instant_locks (wallet_id, txid, islock_blob, verified) VALUES (?1, ?2, x'00', 0)",
        rusqlite::params![id.0.as_slice(), txid],
    );
    assert!(
        repair_closed(&dir).dropped.is_empty(),
        "an unverified lock row"
    );
    assert!(stored(&dir, id, claim.txid()));
}

/// S2: a payment built on the claim's change that a sweep named the winner
/// (so mined or locked, though the store holds neither), or a verified lock
/// row on the claim naming another transaction, keeps the claim.
#[test]
fn a_sweep_winner_built_on_the_claim_or_a_stray_lock_row_keeps_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    eventually("the payment is stored", || {
        stored(session.data_dir(), id, payment.txid())
    });
    let dir = close(&engine, session);
    let a = crate::store_repair::encode_outpoint(&OutPoint::new(funding.txid(), 0));
    on_store(
        &dir,
        "UPDATE core_utxos SET spent_in_txid = ?3 WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![
            id.0.as_slice(),
            a,
            payment.txid().as_byte_array().as_slice()
        ],
    );
    assert!(
        repair_closed(&dir).dropped.is_empty(),
        "a sweep's winner on the claim"
    );
    on_store(
        &dir,
        "UPDATE core_utxos SET spent_in_txid = NULL WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), a],
    );
    let stray = InstantLock {
        txid: <Txid as dashcore::hashes::Hash>::from_byte_array([7; 32]),
        ..InstantLock::default()
    };
    let blob = bincode::serde::encode_to_vec(&stray, bincode::config::standard()).unwrap();
    on_store(
        &dir,
        "INSERT INTO core_instant_locks (wallet_id, txid, islock_blob, verified) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![
            id.0.as_slice(),
            claim.txid().as_byte_array().as_slice(),
            blob
        ],
    );
    assert!(
        repair_closed(&dir).dropped.is_empty(),
        "a lock row naming another transaction"
    );
    assert!(stored(&dir, id, claim.txid()));
}

/// S3 over S4: a claim whose first inputs spend one coin twice and whose
/// last is a push-only unsigned input on another coin drops as sure.
#[test]
fn an_unsigned_input_after_structural_ones_makes_the_drop_sure() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to_a, to_b) = (
        fresh_address(&engine, &session, id),
        fresh_address(&engine, &session, id),
    );
    let funding = super::own_spend_disk_tests::funding(5, &[to_a.clone(), to_b.clone()], PAID);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let (a, b) = (
        OutPoint::new(funding.txid(), 0),
        OutPoint::new(funding.txid(), 1),
    );
    let mut claim = spend_of(a, vec![(elsewhere(), PAID)]);
    claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
    claim.input.push(claim.input[0].clone());
    claim.input.push(TxIn {
        previous_output: b,
        ..Default::default()
    });
    let session = seed_claim(&engine, session, id, &claim, &[to_a.clone(), to_a, to_b]);
    assert!(!spendable(&engine, &session, id, a) && !spendable(&engine, &session, id, b));
    let session = reopen(&engine, session);
    assert!(!stored(session.data_dir(), id, claim.txid()), "dropped");
    let raw = setting(
        &session,
        &id.to_string(),
        &format!("store_dropped:{}", claim.txid()),
    )
    .expect("recorded");
    assert!(raw.contains("\"cause\":\"unsigned\""), "{raw}");
    let (all, doubtful) = records(&session, id);
    assert!(all.contains(&claim.txid()) && doubtful.is_empty(), "sure");
    assert!(spendable(&engine, &session, id, a) && spendable(&engine, &session, id, b));
}

/// S2: a row anywhere in the file that doesn't decode, or more rows than
/// the walk reads, keeps every claim.
#[test]
fn a_walk_that_cant_complete_keeps_the_claim() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    let copy = tempfile::tempdir().unwrap();
    for name in std::fs::read_dir(&dir).unwrap() {
        let p = name.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, copy.path().join(p.file_name().unwrap())).unwrap();
        }
    }
    on_store(
        &dir,
        "INSERT INTO core_instant_locks (wallet_id, txid, islock_blob, verified) VALUES (?1, ?2, x'ff00', 1)",
        rusqlite::params![id.0.as_slice(), [8u8; 32].as_slice()],
    );
    assert!(
        repair_closed(&dir).dropped.is_empty(),
        "a lock that doesn't decode"
    );
    assert!(stored(&dir, id, claim.txid()));

    let rows: i64 = rusqlite::Connection::open(copy.path().join(WALLET_DB_FILE))
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM core_transactions) + (SELECT count(*) FROM core_instant_locks)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    crate::store_repair::limit_rows(copy.path(), rows as usize - 1);
    assert!(
        repair_closed(copy.path()).dropped.is_empty(),
        "too many rows"
    );
    assert!(stored(copy.path(), id, claim.txid()));
    assert_eq!(
        repair_closed(copy.path()).dropped.get(&id),
        Some(&vec![claim.txid()]),
        "within the limit it drops"
    );
}

/// S6: a claim InstantSend-locked is kept.
#[test]
fn a_locked_claim_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    instant_lock(&engine, &session, id, &claim);
    let session = reopen(&engine, session);
    assert!(stored(session.data_dir(), id, claim.txid()));
    assert!(records(&session, id).0.is_empty());
}

/// S6: a stored coin that isn't its funding's output (another value) is
/// no proof: kept. A claim on a coin the store doesn't hold is kept too.
#[test]
fn a_coin_the_store_doesnt_hold_as_its_funding_pays_keeps_the_claim() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    let a = crate::store_repair::encode_outpoint(&OutPoint::new(funding.txid(), 0));
    on_store(
        &dir,
        "UPDATE core_utxos SET value = value - 1 WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), a],
    );
    assert!(
        repair_closed(&dir).dropped.is_empty(),
        "a coin unlike its funding's output"
    );
    on_store(
        &dir,
        "DELETE FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), a],
    );
    assert!(repair_closed(&dir).dropped.is_empty(), "no coin");
    assert!(stored(&dir, id, claim.txid()));
}

/// The guard: a store whose schema version this build wasn't checked
/// against is not repaired.
#[test]
fn a_store_of_an_unknown_schema_is_not_repaired() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    for (later, sql) in [
        (
            27,
            "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
             SELECT 27, 'later', applied_on, checksum FROM refinery_schema_history \
             WHERE version = 26",
        ),
        (25, "DELETE FROM refinery_schema_history WHERE version > 25"),
    ] {
        on_store(&dir, sql, []);
        assert_eq!(schema(&dir), later);
        let outcome = repair_closed(&dir);
        assert_eq!(outcome, crate::store_repair::Outcome::default(), "{later}");
        assert!(stored(&dir, id, claim.txid()), "{later}: nothing repaired");
    }
}

/// Every row of the closed store in `dir`, by table, but for the schema
/// history, times, flush counts (`meta_data_versions.seq` counts the
/// persister's flushes, which batch writes by timing) and the store's
/// random generation token.
fn dump(dir: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT IN ('refinery_schema_history', 'sqlite_sequence', 'meta_store_generation') \
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let mut all = Vec::new();
    for table in tables {
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info(?1)")
                .unwrap();
            stmt.query_map([&table], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let kept: Vec<String> = (columns.into_iter())
            .filter(|c| !c.ends_with("_at") && !c.ends_with("_time") && c != "time")
            .filter(|c| table != "meta_data_versions" || c != "seq")
            .map(|c| format!("quote(\"{c}\")"))
            .collect();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM \"{table}\"",
                kept.join(" || '|' || ")
            ))
            .unwrap();
        let mut rows: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        all.push((table, rows));
    }
    all
}

/// What [`seed_claim`] leaves is what the store keeps of the same claim
/// relayed through the mempool, compared row by row ([`dump`]): a
/// claim whose input script isn't push-only, which key-wallet still
/// records, after the same opens.
#[test]
fn a_seeded_claim_is_stored_as_a_relayed_one() {
    let script = [0x61, 0x01, 0x01];
    let (relayed, relayed_rows) = {
        let (_dir, engine, session, id) = engine_wallet();
        let (_, funding, mut claim) = funding_and_spend(&engine, &session, id);
        claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(script.to_vec());
        mined(&engine, &session, id, BIRTH - 20, vec![funding]);
        mempool(&engine, &session, id, &claim);
        eventually("the claim is stored", || {
            stored(session.data_dir(), id, claim.txid())
        });
        let dir = close(&engine, session);
        arm(&dir, Stop::Skip);
        let dir = close(&engine, open(&engine));
        (claim.txid(), dump(&dir))
    };
    let (seeded, seeded_rows) = {
        let (_dir, engine, session, id) = engine_wallet();
        let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &script);
        let dir = close(&engine, session);
        (claim.txid(), dump(&dir))
    };
    assert_eq!(relayed, seeded, "the same claim");
    for ((table, relayed), (_, seeded)) in relayed_rows.iter().zip(&seeded_rows) {
        assert_eq!(relayed, seeded, "{table}");
    }
    assert_eq!(relayed_rows, seeded_rows);
}

/// The persister keys a block's spend of a coin as the store keys the
/// coin, which the repair's block-spend checks rely on.
#[test]
fn a_block_spend_is_keyed_as_its_coin() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to, funding, spend) = funding_and_spend(&engine, &session, id);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let spend = signed_by_wallet(&engine, &session, id, &to, spend);
    mined(&engine, &session, id, BIRTH - 10, vec![spend.clone()]);
    let dir = close(&engine, session);
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    let mut stmt = conn
        .prepare("SELECT outpoint, spender_txid FROM core_block_spends WHERE wallet_id = ?1")
        .unwrap();
    let rows: Vec<(Vec<u8>, Vec<u8>)> = stmt
        .query_map([id.0.as_slice()], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![(
            crate::store_repair::encode_outpoint(&OutPoint::new(funding.txid(), 0)),
            spend.txid().as_byte_array().to_vec(),
        )]
    );
}

/// S2 from a block's spend (V025): an output of the claim a block spent
/// makes the claim mined, so it is kept, whatever the coin row says.
#[test]
fn a_claim_whose_output_a_block_spent_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    on_store(
        &dir,
        "INSERT INTO core_block_spends (wallet_id, outpoint, height, block_hash, spender_txid) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id.0.as_slice(),
            crate::store_repair::encode_outpoint(&OutPoint::new(claim.txid(), 1)),
            BIRTH - 10,
            [7u8; 32],
            [9u8; 32]
        ],
    );
    let outcome = repair_closed(&dir);
    assert_eq!(outcome.schema, Some(26));
    assert!(outcome.dropped.is_empty(), "{outcome:?}");
    assert!(stored(&dir, id, claim.txid()));
}

/// The store as a build at V024 left it: the tables of the later
/// migrations aren't there, and its history ends at 24.
pub(super) fn as_v24(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    conn.execute_batch(
        "DROP TABLE core_block_spends; DROP TABLE core_block_spends_migrated; \
         DROP TABLE core_dismissed_txids; \
         DELETE FROM refinery_schema_history WHERE version > 24;",
    )
    .unwrap();
}

/// The store's schema version.
fn schema(dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    conn.query_row(
        "SELECT MAX(version) FROM refinery_schema_history",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// W2 on the store a build at V024 left: the first open of this build
/// repairs it before the persister upgrades it, and the next at V026 finds
/// nothing more.
#[test]
fn a_claim_in_a_store_a_v24_build_left_is_dropped_at_the_first_open() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    as_v24(&dir);
    let outcome = repair_closed(&dir);
    assert_eq!(outcome.schema, Some(24));
    assert_eq!(outcome.dropped.get(&id), Some(&vec![claim.txid()]));
    let session = open(&engine);
    assert_eq!(schema(session.data_dir()), 26, "upgraded after the repair");
    let a = OutPoint::new(funding.txid(), 0);
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert_eq!(balances(&session, id).total, PAID, "A counts once");
    let dir = close(&engine, session);
    let outcome = repair_closed(&dir);
    assert_eq!(outcome.schema, Some(26));
    assert!(outcome.dropped.is_empty() && outcome.restored.is_empty());
}

/// A coin of the claim that a block spent, whatever spent it (V025), is
/// not given back when the claim is dropped: it stays spent.
#[test]
fn a_coin_a_block_spent_is_not_given_back_by_the_drop() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let dir = close(&engine, session);
    let a = crate::store_repair::encode_outpoint(&OutPoint::new(funding.txid(), 0));
    on_store(
        &dir,
        "INSERT INTO core_block_spends (wallet_id, outpoint, height, block_hash, spender_txid) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id.0.as_slice(), a, BIRTH - 10, [7u8; 32], [9u8; 32]],
    );
    let outcome = repair_closed(&dir);
    assert_eq!(
        outcome.dropped.get(&id),
        Some(&vec![claim.txid()]),
        "dropped"
    );
    let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    let spent: i64 = conn
        .query_row(
            "SELECT spent FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![id.0.as_slice(), a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(spent, 1, "A stays spent");
    let appdb = dw_appdb::AppDb::open(&dir.join(dw_appdb::APP_DB_FILE)).unwrap();
    let raw = (appdb.setting(&id.to_string(), &format!("store_dropped:{}", claim.txid())))
        .unwrap()
        .unwrap();
    assert!(raw.contains("\"released_coins\":[]"), "{raw}");
}

/// A stop after the records are staged and before the store commits rolls
/// the store back: the claim stays, A stays spent, and the staged record
/// doesn't count. The next open settles it and repairs.
#[test]
fn a_stop_inside_the_repair_rolls_it_back() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let a = OutPoint::new(funding.txid(), 0);
    arm(session.data_dir(), Stop::BeforeCommit);
    let session = reopen(&engine, session);
    assert!(stored(session.data_dir(), id, claim.txid()), "rolled back");
    assert!(
        setting(&session, "store_repair", "staged").is_some(),
        "staged"
    );
    assert!(
        records(&session, id).0.is_empty(),
        "a staged record of a held claim doesn't count"
    );
    assert!(!dropped(&engine, &session, id, claim.txid()));
    assert!(!spendable(&engine, &session, id, a), "A stays spent");

    let session = reopen(&engine, session);
    assert!(setting(&session, "store_repair", "staged").is_none());
    assert!(!stored(session.data_dir(), id, claim.txid()), "repaired");
    assert!(spendable(&engine, &session, id, a));
    let raw = setting(
        &session,
        &id.to_string(),
        &format!("store_dropped:{}", claim.txid()),
    )
    .unwrap();
    assert!(raw.contains("\"state\":\"done\""), "{raw}");
}

/// A stop after the store commits and before its records are done: they
/// count already, as the store no longer holds the claim, and the next
/// open marks them done.
#[test]
fn a_stop_after_the_store_commits_keeps_the_drop() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    arm(session.data_dir(), Stop::BeforePromote);
    let session = reopen(&engine, session);
    let key = format!("store_dropped:{}", claim.txid());
    assert!(
        setting(&session, &id.to_string(), &key)
            .unwrap()
            .contains("\"state\":\"staged\"")
    );
    assert!(records(&session, id).0.contains(&claim.txid()), "it counts");
    eventually("the payment on the claim goes", || {
        dropped(&engine, &session, id, payment.txid())
    });
    assert!(spendable(
        &engine,
        &session,
        id,
        OutPoint::new(funding.txid(), 0)
    ));

    let session = reopen(&engine, session);
    assert!(setting(&session, "store_repair", "staged").is_none());
    assert!(
        setting(&session, &id.to_string(), &key)
            .unwrap()
            .contains("\"state\":\"done\"")
    );
    assert!(dropped(&engine, &session, id, payment.txid()));
}

/// The record round-trips through the history's listing: a drop is in
/// `Persisted::dropped`, restored from no mark, and only a structural
/// drop (S4) makes it doubtful.
#[test]
fn a_repair_record_reaches_the_listing_of_stored_transactions() {
    let (_dir, engine, session, id) = engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let session = reopen(&engine, session);
    let mut persisted =
        crate::history_ops::persisted_txids(&session.data_dir().join(WALLET_DB_FILE))
            .unwrap()
            .remove(&id.0)
            .unwrap_or_default();
    assert!(persisted.add_store_dropped(&session.live().unwrap().appdb, id));
    assert!(persisted.dropped.contains(&claim.txid()));
    assert!(persisted.dropped_doubtful.is_empty());
    assert!(!persisted.keeps_mark(&claim.txid(), &Default::default()));
}
