//! C3: claims a fork build's open dropped as unsigned
//! (`core_dropped_unsigned_spends`, published pins 373342cb3 and
//! c5917e6bd), judged again at the next open by the store repair's rules.
//! One the rules keep, or can't judge, comes back pending with its coins
//! held again (L3); one they drop stays dropped (L2); a coin something
//! spends since is never held again (L4).

use std::collections::HashSet;

use rusqlite::OptionalExtension;

use super::own_spend_disk_tests::{eventually, foreign, send, spendable, start_spv};
use super::store_repair_tests::{
    block_spend, claim_on_a_held_coin, close, dropped, on_store, repair_closed, setting, stored,
};
use super::*;
use crate::send::CoinSource;
use crate::session::WALLET_DB_FILE;
use crate::store_repair::{Stop, arm, encode_outpoint, repair};

/// The fork's table, as its migration V3 created it (the same at both pins).
const V3_SQL: &str = include_str!(
    "../../dw-store-upgrade/data/fork-c5917e6bd5/V3__dropped_unsigned_spends_recovery_record.sql"
);

/// The published pins whose open dropped claims. Their drops differ only in
/// how a kept row's `spent_in_txid` is written.
#[derive(Clone, Copy, Debug)]
pub(super) enum Pin {
    /// 373342cb3 (v0.3.0-dev.1105).
    Dev1105,
    /// c5917e6bd (v0.3.0-dev.1387).
    Dev1387,
}

const PINS: [Pin; 2] = [Pin::Dev1105, Pin::Dev1387];

/// The pin's JSON of a kept `core_utxos` row (`KEPT_UTXO_COLUMNS`).
fn kept_columns(pin: Pin) -> String {
    let spent_in = match pin {
        Pin::Dev1105 => "hex(spent_in_txid)",
        Pin::Dev1387 => "CASE WHEN spent_in_txid IS NULL THEN NULL ELSE hex(spent_in_txid) END",
    };
    format!(
        "json_object('outpoint', hex(outpoint), 'value', value, 'script', hex(script), \
         'spent', spent, 'spent_in_txid', {spent_in}, \
         'winner_mined_height', winner_mined_height, 'is_sweep_placeholder', is_sweep_placeholder)"
    )
}

/// The fork's drop of `claim` from `id`'s store in the closed `dir`, as its
/// `drop_unauthorized_pending_spends` wrote it at `pin`: the row, with the
/// claim's record and lock and the rows it changed, goes to the fork's
/// table; the claim, its lock row and its unspent output rows go; each
/// coin it spends that nothing else of the wallet spends, held by it or by
/// nothing, in an own address pool, is given back.
pub(super) fn fork_drop(dir: &std::path::Path, id: WalletId, claim: &Transaction, pin: Pin) {
    let mut conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let tx = conn.transaction().unwrap();
    let has: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' \
             AND name = 'core_dropped_unsigned_spends')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !has {
        tx.execute_batch(V3_SQL).unwrap();
    }
    let (w, t) = (
        id.0.to_vec(),
        claim.txid().as_byte_array().as_slice().to_vec(),
    );
    let cols = kept_columns(pin);
    let mut deleted = Vec::new();
    for vout in 0..claim.output.len() {
        let op = encode_outpoint(&OutPoint::new(claim.txid(), vout as u32));
        let row: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT {cols} FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2 \
                     AND spent = 0 AND is_sweep_placeholder = 0"
                ),
                rusqlite::params![w, op],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        if let Some(row) = row {
            deleted.push((op, row));
        }
    }
    let mut claimed = HashSet::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT record_blob FROM core_transactions \
                 WHERE wallet_id = ?1 AND txid <> ?2 AND record_blob IS NOT NULL",
            )
            .unwrap();
        let blobs: Vec<Vec<u8>> = stmt
            .query_map(rusqlite::params![w, t], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for blob in blobs {
            let record = crate::history_ops::decode_record(&blob).unwrap();
            claimed.extend(record.transaction.input.iter().map(|i| i.previous_output));
        }
    }
    let mut released = Vec::new();
    for input in &claim.input {
        if claimed.contains(&input.previous_output) {
            continue;
        }
        let op = encode_outpoint(&input.previous_output);
        let row: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT {cols} FROM core_utxos u \
                     WHERE u.wallet_id = ?1 AND u.outpoint = ?2 AND u.spent = 1 \
                       AND u.is_sweep_placeholder = 0 \
                       AND (u.spent_in_txid IS NULL OR u.spent_in_txid = ?3) \
                       AND EXISTS (SELECT 1 FROM core_address_pool p \
                                   WHERE p.wallet_id = u.wallet_id AND p.script = u.script \
                                     AND p.account_type <> 'dashpay_external')"
                ),
                rusqlite::params![w, op, t],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        if let Some(row) = row {
            released.push((op, row));
        }
    }
    let json = |rows: &[(Vec<u8>, String)]| {
        format!(
            "[{}]",
            rows.iter()
                .map(|(_, row)| row.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let inserted = tx
        .execute(
            "INSERT INTO core_dropped_unsigned_spends \
                (wallet_id, txid, record_blob, islock_blob, islock_verified, \
                 deleted_outputs, released_coins) \
             SELECT t.wallet_id, t.txid, t.record_blob, l.islock_blob, l.verified, ?3, ?4 \
             FROM core_transactions t LEFT JOIN core_instant_locks l \
                ON l.wallet_id = t.wallet_id AND l.txid = t.txid \
             WHERE t.wallet_id = ?1 AND t.txid = ?2",
            rusqlite::params![w, t, json(&deleted), json(&released)],
        )
        .unwrap();
    assert_eq!(inserted, 1, "the claim is stored for the fork to drop");
    tx.execute(
        "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
        rusqlite::params![w, t],
    )
    .unwrap();
    tx.execute(
        "DELETE FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2",
        rusqlite::params![w, t],
    )
    .unwrap();
    for (op, _) in &deleted {
        tx.execute(
            "DELETE FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![w, op],
        )
        .unwrap();
    }
    for (op, _) in &released {
        tx.execute(
            "UPDATE core_utxos SET spent = 0, spent_in_txid = NULL \
             WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![w, op],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

/// A copy of the closed network `dir`, repaired: what the repair itself
/// writes, before a session loads it (the original stays for the open's
/// own repair).
fn repaired_copy(dir: &std::path::Path) -> tempfile::TempDir {
    let copy = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, copy.path().join(p.file_name().unwrap())).unwrap();
        }
    }
    repair_closed(copy.path());
    copy
}

/// How many fork rows the closed or open store in `dir` holds for `txid`.
fn fork_rows(dir: &std::path::Path, id: WalletId, txid: Txid) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT count(*) FROM core_dropped_unsigned_spends WHERE wallet_id = ?1 AND txid = ?2",
        rusqlite::params![id.0.as_slice(), txid.as_byte_array().as_slice()],
        |r| r.get(0),
    )
    .unwrap()
}

/// The re-check's record of `txid`, as stored.
fn restored_record(session: &Arc<NetworkSession>, id: WalletId, txid: Txid) -> Option<String> {
    setting(session, &id.to_string(), &format!("store_restored:{txid}"))
}

/// A claim on a held coin A whose input script isn't push-only, recorded,
/// then dropped by a fork build at `pin` with A given back, and the
/// network closed. Returns the engine's wallet, A's funding, the claim
/// and the network directory.
fn non_push_only_claim_the_fork_dropped(
    pin: Pin,
) -> (
    tempfile::TempDir,
    Engine,
    WalletId,
    Transaction,
    Transaction,
    std::path::PathBuf,
) {
    let (dir, engine, session, id) = engine_wallet();
    let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[0x61, 0x01, 0x01]);
    let network = close(&engine, session);
    fork_drop(&network, id, &claim, pin);
    assert!(!stored(&network, id, claim.txid()), "the fork dropped it");
    (dir, engine, id, funding, claim, network)
}

/// (a) A claim whose input script isn't push-only (S1) that a fork build
/// dropped: the next open puts it back pending with A held, records the
/// put-back, and Abandon is its way out, freeing A (S5).
#[test]
fn a_non_push_only_claim_the_fork_dropped_is_held_again_until_abandoned() {
    for pin in PINS {
        let (_dir, engine, id, funding, claim, _) = non_push_only_claim_the_fork_dropped(pin);
        let a = OutPoint::new(funding.txid(), 0);
        let session = open(&engine);
        assert!(
            stored(session.data_dir(), id, claim.txid()),
            "{pin:?}: put back"
        );
        assert_eq!(fork_rows(session.data_dir(), id, claim.txid()), 0);
        assert!(!spendable(&engine, &session, id, a), "{pin:?}: A is held");
        assert!(!dropped(&engine, &session, id, claim.txid()), "pending");
        let raw = restored_record(&session, id, claim.txid()).expect("recorded");
        assert!(raw.contains("\"state\":\"done\""), "{raw}");
        assert!(raw.contains("\"verdict\":\"keep\""), "{raw}");
        let extras = engine
            .block_on(session.tx_detail_extras(id, claim.txid()))
            .unwrap();
        assert!(
            extras.can_abandon,
            "{pin:?}: Abandon is offered: {extras:?}"
        );

        start_spv(&engine, &session);
        engine
            .block_on(session.abandon_transaction(id, claim.txid()))
            .unwrap();
        mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
        mined(&engine, &session, id, BIRTH + 1, vec![foreign()]);
        assert!(spendable(&engine, &session, id, a), "{pin:?}: A is offered");
        drop(session);
    }
}

#[test]
fn failed_legacy_repair_never_offers_the_claim_input() {
    for unreadable in [false, true] {
        let (_dir, engine, id, funding, claim, network) =
            non_push_only_claim_the_fork_dropped(Pin::Dev1387);
        if unreadable {
            on_store(
                &network,
                "UPDATE core_dropped_unsigned_spends SET record_blob = X'00'",
                [],
            );
        }
        arm(&network, Stop::BeforeCommit);
        let session = open(&engine);
        let input = OutPoint::new(funding.txid(), 0);
        assert!(session.spends.snapshot(&id).contains(&input));
        session.spends.forget_wallet(&id);
        assert!(!spendable(&engine, &session, id, input));
        assert_eq!(fork_rows(&network, id, claim.txid()), 1);
        assert_eq!(coin_spent(&network, id, &encode_outpoint(&input)).0, 0);
    }
}

#[test]
fn legacy_output_must_match_its_transaction() {
    for field in ["value", "script", "outpoint"] {
        let (_dir, _engine, id, funding, claim, network) =
            non_push_only_claim_the_fork_dropped(Pin::Dev1387);
        let change = encode_outpoint(&OutPoint::new(claim.txid(), 1));
        let nonexistent = encode_outpoint(&OutPoint::new(claim.txid(), 42));
        let value = match field {
            "value" => serde_json::json!(1),
            "script" => serde_json::json!("51"),
            _ => serde_json::json!(hex::encode(&nonexistent)),
        };
        on_store(
            &network,
            "UPDATE core_dropped_unsigned_spends SET deleted_outputs = json_set(deleted_outputs, ?1, json(?2))",
            rusqlite::params![format!("$[0].{field}"), value.to_string()],
        );
        repair_closed(&network);
        assert!(stored(&network, id, claim.txid()), "{field}");
        let input = encode_outpoint(&OutPoint::new(funding.txid(), 0));
        assert_eq!(coin_spent(&network, id, &input).0, 1, "{field}");
        let conn = rusqlite::Connection::open(network.join(WALLET_DB_FILE)).unwrap();
        let restored: i64 = conn
            .query_row(
                "SELECT count(*) FROM core_utxos WHERE wallet_id = ?1 AND outpoint IN (?2, ?3)",
                rusqlite::params![id.0.as_slice(), change, nonexistent],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(restored, 0, "{field}");
    }
}

/// L4 with a block's spend (V025): the claim is put back, and its change,
/// which a block spent by a transaction the store has no record of, comes
/// back spent.
#[test]
fn an_output_a_block_spent_since_comes_back_spent() {
    for pin in PINS {
        let (_dir, _engine, id, _, claim, network) = non_push_only_claim_the_fork_dropped(pin);
        let change = OutPoint::new(claim.txid(), 1);
        block_spend(&network, id, &change, [9u8; 32]);
        let outcome = repair_closed(&network);
        assert_eq!(
            outcome.restored.get(&id),
            Some(&vec![claim.txid()]),
            "{pin:?}"
        );
        assert!(stored(&network, id, claim.txid()), "{pin:?}: put back");
        assert_eq!(
            coin_spent(&network, id, &encode_outpoint(&change)).0,
            1,
            "{pin:?}: its change comes back spent"
        );
    }
}

/// L4 with a block's spend of the claim's coin A by another transaction
/// (V025), marked as the persister marks it: A isn't held again and stays
/// spent by that transaction, and the claim's outputs stay out.
#[test]
fn a_coin_a_block_spent_since_is_not_held_again() {
    for pin in PINS {
        let (_dir, _engine, id, funding, claim, network) =
            non_push_only_claim_the_fork_dropped(pin);
        let a = OutPoint::new(funding.txid(), 0);
        block_spend(&network, id, &a, [9u8; 32]);
        repair_closed(&network);
        assert!(stored(&network, id, claim.txid()), "{pin:?}: put back");
        assert_eq!(
            coin_spent(&network, id, &encode_outpoint(&a)),
            (1, Some(hex::encode_upper([9u8; 32]))),
            "{pin:?}: A stays spent by the block's spender"
        );
        let conn = rusqlite::Connection::open(network.join(WALLET_DB_FILE)).unwrap();
        let outputs: i64 = conn
            .query_row(
                "SELECT count(*) FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
                rusqlite::params![
                    id.0.as_slice(),
                    encode_outpoint(&OutPoint::new(claim.txid(), 1))
                ],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outputs, 0, "{pin:?}: its change stays out");
    }
}

/// L1 with a block's spend (V025): a push-only unsigned claim the fork
/// dropped, whose change a block spent since, is mined; it is put back as
/// settled, not left dropped (L2).
#[test]
fn a_push_only_claim_whose_output_a_block_spent_is_put_back() {
    for pin in PINS {
        let (_dir, engine, session, id) = engine_wallet();
        let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
        let network = close(&engine, session);
        fork_drop(&network, id, &claim, pin);
        block_spend(&network, id, &OutPoint::new(claim.txid(), 1), [9u8; 32]);
        let outcome = repair_closed(&network);
        assert_eq!(
            outcome.restored.get(&id),
            Some(&vec![claim.txid()]),
            "{pin:?}"
        );
        let appdb = dw_appdb::AppDb::open(&network.join(dw_appdb::APP_DB_FILE)).unwrap();
        let raw = (appdb.setting(&id.to_string(), &format!("store_restored:{}", claim.txid())))
            .unwrap()
            .expect("recorded");
        assert!(raw.contains("of its tree is settled"), "{pin:?}: {raw}");
    }
}

/// (b) A push-only unsigned claim whose tree is settled in another loaded
/// wallet: the claim's change pays S1 to wallet B, whose spend of it is
/// InstantSend-locked. The fork saw only the claim's own block or lock and
/// dropped it; the next open puts it back with A held (S2).
#[test]
fn an_unsigned_claim_settled_in_another_wallet_is_held_again() {
    use super::own_spend_disk_tests::{fresh_address, second_wallet, send_to, wait_stored};
    for pin in PINS {
        let (_dir, engine, session, id) = engine_wallet();
        let other = second_wallet(&engine, &session);
        engine
            .block_on(session.set_load_on_startup(other, true))
            .unwrap();
        let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
        let to = fresh_address(&engine, &session, other);
        let s1 = send_to(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
            &to.to_string(),
            CHANGE / 2,
            false,
        );
        mempool(&engine, &session, id, &s1);
        wait_stored(&session, other, &[s1.txid()]);
        let received = (s1.output.iter())
            .position(|o| o.script_pubkey == to.script_pubkey())
            .unwrap() as u32;
        let child = spend_of(
            OutPoint::new(s1.txid(), received),
            vec![
                (elsewhere(), CHANGE / 8),
                (
                    fresh_address(&engine, &session, other).script_pubkey(),
                    CHANGE / 8,
                ),
            ],
        );
        instant_lock(&engine, &session, other, &child);
        wait_stored(&session, other, &[child.txid()]);
        wait_stored_locked(&session, other, child.txid());
        let network = close(&engine, session);
        fork_drop(&network, id, &claim, pin);

        let session = open(&engine);
        let a = OutPoint::new(funding.txid(), 0);
        assert!(
            stored(session.data_dir(), id, claim.txid()),
            "{pin:?}: put back"
        );
        assert!(!spendable(&engine, &session, id, a), "{pin:?}: A is held");
        let raw = restored_record(&session, id, claim.txid()).expect("recorded");
        assert!(raw.contains("of its tree is settled"), "{raw}");
        drop(session);
    }
}

/// (c) A push-only unsigned claim with nothing of its tree settled: the
/// rules drop it too, so its fork row stays and A stays offered (L2).
#[test]
fn a_push_only_unsigned_claim_the_fork_dropped_stays_dropped() {
    for pin in PINS {
        let (_dir, engine, session, id) = engine_wallet();
        let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
        let network = close(&engine, session);
        fork_drop(&network, id, &claim, pin);
        let session = open(&engine);
        assert!(!stored(session.data_dir(), id, claim.txid()));
        assert_eq!(fork_rows(session.data_dir(), id, claim.txid()), 1);
        assert!(restored_record(&session, id, claim.txid()).is_none());
        assert!(
            spendable(&engine, &session, id, OutPoint::new(funding.txid(), 0)),
            "{pin:?}: A is offered"
        );
        drop(session);
    }
}

/// A row of the closed or open store in `dir`, as JSON, or `None`.
fn row_json(dir: &std::path::Path, sql: &str, params: impl rusqlite::Params) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(sql, params, |r| r.get(0))
        .optional()
        .unwrap()
}

/// `id`'s row of the coin `op`: spent, and the hex of its `spent_in_txid`.
fn coin_spent(dir: &std::path::Path, id: WalletId, op: &[u8]) -> (i64, Option<String>) {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join(WALLET_DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT spent, CASE WHEN spent_in_txid IS NULL THEN NULL ELSE hex(spent_in_txid) END \
         FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
        rusqlite::params![id.0.as_slice(), op],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

/// `txid`'s stored row for `id`, as JSON.
fn transaction_row(dir: &std::path::Path, id: WalletId, txid: Txid) -> Option<String> {
    row_json(
        dir,
        "SELECT json_object('height', height, 'block_hash', hex(block_hash), \
         'block_time', block_time, 'finalized', finalized, 'record', hex(record_blob)) \
         FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
        rusqlite::params![id.0.as_slice(), txid.as_byte_array().as_slice()],
    )
}

/// (d) A coin the fork gave back that the user spent since (P, stored,
/// before this build), its row left spent with no `spent_in_txid` as the
/// fork's own stores have it: A isn't held for the claim again (L4, read
/// from the stored graph): it stays spent and isn't bound to the claim,
/// P's row and the balance don't change, and the record names A. The
/// claim itself comes back, without its outputs.
#[test]
fn a_coin_spent_since_the_fork_gave_it_back_is_not_held_again() {
    for pin in PINS {
        let (_dir, engine, id, funding, claim, network) = non_push_only_claim_the_fork_dropped(pin);
        let a = OutPoint::new(funding.txid(), 0);
        let a_op = encode_outpoint(&a);
        arm(&network, Stop::Skip);
        let session = open(&engine);
        // A build without the re-check offers A: P spends it.
        let payment = send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![a]),
            PAID / 2,
            false,
        );
        mempool(&engine, &session, id, &payment);
        eventually("P is stored", || {
            stored(session.data_dir(), id, payment.txid())
        });
        let network = close(&engine, session);
        on_store(
            &network,
            "UPDATE core_utxos SET spent_in_txid = NULL WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![id.0.as_slice(), &a_op],
        );
        assert_eq!(coin_spent(&network, id, &a_op), (1, None));
        // The balance a reopen without the re-check shows.
        arm(&network, Stop::Skip);
        let session = open(&engine);
        let before = balances(&session, id);
        let network = close(&engine, session);
        let spender = transaction_row(&network, id, payment.txid()).expect("P is stored");

        let outcome = repair_closed(&network);
        eprintln!("{pin:?}: (d)'s claim: {outcome:?}");
        assert!(stored(&network, id, claim.txid()), "{pin:?}: put back");
        assert_eq!(fork_rows(&network, id, claim.txid()), 0);
        let (spent, spent_in) = coin_spent(&network, id, &a_op);
        assert_eq!(spent, 1, "{pin:?}: A stays spent");
        let claim_hex = hex::encode_upper(claim.txid().as_byte_array());
        assert_ne!(
            spent_in.as_deref(),
            Some(claim_hex.as_str()),
            "{pin:?}: A isn't bound to the claim"
        );
        assert_eq!(
            transaction_row(&network, id, payment.txid()).as_ref(),
            Some(&spender),
            "{pin:?}: P's row doesn't change"
        );
        let session = open(&engine);
        assert!(!dropped(&engine, &session, id, payment.txid()), "P stays");
        assert_eq!(balances(&session, id), before, "{pin:?}: the balance");
        let raw = restored_record(&session, id, claim.txid()).expect("recorded");
        eprintln!("{pin:?}: (d)'s record: {raw}");
        let a_hex = hex::encode(&a_op);
        assert!(
            raw.contains(&format!("\"not_held\":[\"{a_hex}\"]")),
            "{raw}"
        );
        assert!(raw.contains("\"held_coins\":[]"), "{raw}");
        assert!(raw.contains("\"restored_outputs\":[]"), "{raw}");
        drop(session);
    }
}

/// (d) with the re-check's records unread at a load: P's change, withheld
/// while P alone spends A, stays withheld though the claim put back spends
/// A too. A load that can't tell which claims were put back counts no
/// other spender of the wallet's coins.
#[test]
fn a_put_back_claim_frees_no_withheld_change_while_its_record_does_not_read() {
    for pin in PINS {
        let (_dir, engine, id, funding, claim, network) = non_push_only_claim_the_fork_dropped(pin);
        let a = OutPoint::new(funding.txid(), 0);
        arm(&network, Stop::Skip);
        let session = open(&engine);
        let payment = send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![a]),
            PAID / 2,
            false,
        );
        mempool(&engine, &session, id, &payment);
        eventually("P is stored", || {
            stored(session.data_dir(), id, payment.txid())
        });
        let network = close(&engine, session);
        on_store(
            &network,
            "UPDATE core_utxos SET spent_in_txid = NULL WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![id.0.as_slice(), &encode_outpoint(&a)],
        );
        arm(&network, Stop::Skip);
        let session = open(&engine);
        let before = balances(&session, id);
        assert!(before.withheld > 0, "{pin:?}: P's change is withheld");
        let network = close(&engine, session);
        repair_closed(&network);

        let session = open(&engine);
        assert_eq!(balances(&session, id), before, "{pin:?}: read");
        assert!((session.hub.history).read_wallet(&id, |h| {
            !h.put_back_unread && h.put_back.contains(&claim.txid())
        }));
        (session.hub.fail_restored_read).store(true, std::sync::atomic::Ordering::SeqCst);
        let s = Arc::clone(&session);
        engine
            .block_on(session.on_runtime(async move { s.load_history_for(vec![id]).await }))
            .unwrap();
        assert_eq!(balances(&session, id), before, "{pin:?}: unread");
        assert!((session.hub.history).read_wallet(&id, |h| h.put_back_unread));

        (session.live().unwrap().appdb)
            .set_setting(&id.to_string(), "store_restored:not-a-txid", Some(""))
            .unwrap();
        let s = Arc::clone(&session);
        engine
            .block_on(session.on_runtime(async move { s.load_history_for(vec![id]).await }))
            .unwrap();
        assert_eq!(balances(&session, id), before, "{pin:?}: malformed");
        assert!(
            (session.hub.history).read_wallet(&id, |h| h.put_back_unread),
            "{pin:?}: a record naming no transaction reads as unread"
        );
        drop(session);
    }
}

/// (e) A second open changes nothing; a stop inside the store transaction
/// rolls the put-back back (its staged record goes at the next open), and
/// a stop after the commit keeps it (its record is done at the next open).
#[test]
fn the_recheck_runs_once_and_a_stop_either_side_of_its_commit_leaves_it_whole() {
    for pin in PINS {
        let (_dir, _engine, id, funding, claim, network) =
            non_push_only_claim_the_fork_dropped(pin);
        let a = encode_outpoint(&OutPoint::new(funding.txid(), 0));
        let appdb = dw_appdb::AppDb::open(&network.join(dw_appdb::APP_DB_FILE)).unwrap();
        let key = format!("store_restored:{}", claim.txid());
        let spent = |dir: &std::path::Path| -> i64 {
            let conn = rusqlite::Connection::open(dir.join(WALLET_DB_FILE)).unwrap();
            conn.query_row(
                "SELECT spent FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
                rusqlite::params![id.0.as_slice(), a],
                |r| r.get(0),
            )
            .unwrap()
        };

        arm(&network, Stop::BeforeCommit);
        assert!(repair(&network, &appdb).is_err());
        assert!(!stored(&network, id, claim.txid()), "{pin:?}: rolled back");
        assert_eq!(fork_rows(&network, id, claim.txid()), 1);
        assert_eq!(spent(&network), 0);
        assert!(
            (appdb.setting(&id.to_string(), &key).unwrap())
                .is_some_and(|r| r.contains("\"state\":\"staged\""))
        );

        arm(&network, Stop::BeforePromote);
        assert!(repair(&network, &appdb).is_err());
        assert!(
            (appdb.setting(&id.to_string(), &key).unwrap())
                .is_some_and(|r| r.contains("\"state\":\"staged\"")),
            "the rolled-back record went; this one is staged"
        );
        assert!(stored(&network, id, claim.txid()), "{pin:?}: committed");
        assert_eq!(fork_rows(&network, id, claim.txid()), 0);
        assert_eq!(spent(&network), 1);

        let outcome = repair(&network, &appdb).unwrap();
        assert!(outcome.restored.is_empty(), "{outcome:?}");
        let done = appdb.setting(&id.to_string(), &key).unwrap().unwrap();
        assert!(done.contains("\"state\":\"done\""), "{done}");
        assert!(
            appdb
                .setting("store_repair", "staged_restored")
                .unwrap()
                .is_none()
        );
        let outcome = repair(&network, &appdb).unwrap();
        assert!(outcome.restored.is_empty() && outcome.dropped.is_empty());
        assert_eq!(appdb.setting(&id.to_string(), &key).unwrap().unwrap(), done);
        assert!(stored(&network, id, claim.txid()) && spent(&network) == 1);
    }
}

/// (f) A fork row whose record doesn't read: the claim can't go back, so
/// the coins the fork released for it are held again from its
/// `released_coins`, its row goes and the put-back is recorded as a doubt.
/// When its released coins don't read either, the repair fails with its row intact.
#[test]
fn a_fork_row_whose_record_doesnt_read_holds_its_released_coins_again() {
    for pin in PINS {
        let (_dir, engine, id, funding, claim, network) = non_push_only_claim_the_fork_dropped(pin);
        let a = OutPoint::new(funding.txid(), 0);
        on_store(
            &network,
            "UPDATE core_dropped_unsigned_spends SET record_blob = x'00'",
            [],
        );
        let copy = tempfile::tempdir().unwrap();
        for name in [WALLET_DB_FILE, "wallet.sqlite-wal", "wallet.sqlite-shm"] {
            if network.join(name).exists() {
                std::fs::copy(network.join(name), copy.path().join(name)).unwrap();
            }
        }
        on_store(
            copy.path(),
            "UPDATE core_dropped_unsigned_spends SET released_coins = 'not json'",
            [],
        );
        let db = dw_appdb::AppDb::open(&copy.path().join(dw_appdb::APP_DB_FILE)).unwrap();
        assert!(repair(copy.path(), &db).is_err());
        assert_eq!(fork_rows(copy.path(), id, claim.txid()), 1, "its row stays");

        let session = open(&engine);
        assert!(!stored(session.data_dir(), id, claim.txid()));
        assert_eq!(fork_rows(session.data_dir(), id, claim.txid()), 0);
        assert!(!spendable(&engine, &session, id, a), "{pin:?}: A is held");
        let raw = restored_record(&session, id, claim.txid()).expect("recorded");
        assert!(raw.contains("\"verdict\":\"doubt\""), "{raw}");
        assert!(raw.contains("\"restored_transaction\":false"), "{raw}");
        drop(session);
    }
}

/// A fork-dropped claim X whose change a fork-dropped Y spends, Y's fork
/// row holding a lock: the lock in Y's row settles X's tree, so both come
/// back with A held, and X's change, which the fork took with X, comes
/// back spent by Y.
#[test]
fn a_lock_in_a_fork_dropped_childs_row_keeps_its_fork_dropped_parent() {
    for pin in PINS {
        let (_dir, engine, session, id) = engine_wallet();
        let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
        let change = OutPoint::new(claim.txid(), 1);
        let child = send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![change]),
            CHANGE / 2,
            false,
        );
        mempool(&engine, &session, id, &child);
        eventually("the child is stored", || {
            stored(session.data_dir(), id, child.txid())
        });
        let network = close(&engine, session);
        fork_drop(&network, id, &child, pin);
        fork_drop(&network, id, &claim, pin);
        on_store(
            &network,
            "UPDATE core_dropped_unsigned_spends SET islock_blob = x'01', islock_verified = 0 \
             WHERE txid = ?1",
            [child.txid().as_byte_array().as_slice()],
        );

        let copy = repaired_copy(&network);
        assert_eq!(
            coin_spent(copy.path(), id, &encode_outpoint(&change)).0,
            1,
            "{pin:?}: the repair puts X's change back spent by Y"
        );
        let session = open(&engine);
        let dir = session.data_dir().to_path_buf();
        assert!(stored(&dir, id, claim.txid()), "{pin:?}: X is put back");
        assert!(stored(&dir, id, child.txid()), "{pin:?}: Y is put back");
        let raw = restored_record(&session, id, claim.txid()).expect("recorded");
        assert!(raw.contains("of its tree is settled"), "{raw}");
        assert!(
            !spendable(&engine, &session, id, OutPoint::new(funding.txid(), 0)),
            "{pin:?}: A is held"
        );
        assert_eq!(
            coin_spent(&dir, id, &encode_outpoint(&change)).0,
            1,
            "{pin:?}: X's change is back, spent by Y"
        );
        drop(session);
    }
}

/// A claim the fork dropped that the store holds again: with a row of no
/// record, not mined, the record fills it; with its record, the row stays.
/// Either way its coin, given back by the fork, is held again and its fork
/// row goes.
#[test]
fn a_fork_dropped_claim_the_store_holds_again_has_its_coin_held() {
    for pin in PINS {
        for with_record in [false, true] {
            let (_dir, engine, id, funding, claim, network) =
                non_push_only_claim_the_fork_dropped(pin);
            on_store(
                &network,
                &format!(
                    "INSERT INTO core_transactions \
                        (wallet_id, txid, height, block_hash, block_time, finalized, record_blob) \
                     SELECT wallet_id, txid, NULL, NULL, NULL, 0, {} \
                     FROM core_dropped_unsigned_spends",
                    if with_record { "record_blob" } else { "NULL" }
                ),
                [],
            );
            let copy = repaired_copy(&network);
            let a = encode_outpoint(&OutPoint::new(funding.txid(), 0));
            assert_eq!(
                coin_spent(copy.path(), id, &a).0,
                1,
                "{pin:?} {with_record}: the repair holds A"
            );
            let session = open(&engine);
            let dir = session.data_dir().to_path_buf();
            let row = transaction_row(&dir, id, claim.txid()).expect("stored");
            assert!(
                !row.contains("\"record\":null"),
                "{pin:?} {with_record}: its record is there: {row}"
            );
            assert_eq!(fork_rows(&dir, id, claim.txid()), 0);
            assert!(
                !spendable(&engine, &session, id, OutPoint::new(funding.txid(), 0)),
                "{pin:?} {with_record}: A is held"
            );
            let raw = restored_record(&session, id, claim.txid()).expect("recorded");
            assert!(
                raw.contains(&format!("\"restored_transaction\":{}", !with_record)),
                "{raw}"
            );
            drop(session);
        }
    }
}

/// A push-only unsigned claim kept only by the lock its fork row had,
/// which the store holds again with its record (relayed again, its lock
/// not): its lock row goes back with its coin held, so the drop of
/// unsigned claims, at this open and the next, keeps it (L1a).
#[test]
fn a_lock_from_a_fork_row_goes_back_when_the_store_holds_its_claim_again() {
    for pin in PINS {
        let (_dir, engine, session, id) = engine_wallet();
        let (session, funding, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
        let network = close(&engine, session);
        fork_drop(&network, id, &claim, pin);
        on_store(
            &network,
            "UPDATE core_dropped_unsigned_spends SET islock_blob = x'01', islock_verified = 0",
            [],
        );
        on_store(
            &network,
            "INSERT INTO core_transactions \
                (wallet_id, txid, height, block_hash, block_time, finalized, record_blob) \
             SELECT wallet_id, txid, NULL, NULL, NULL, 0, record_blob \
             FROM core_dropped_unsigned_spends",
            [],
        );
        let a = OutPoint::new(funding.txid(), 0);
        for open_n in 1..=2 {
            let session = open(&engine);
            let dir = session.data_dir().to_path_buf();
            assert!(stored(&dir, id, claim.txid()), "{pin:?} {open_n}: kept");
            assert_eq!(fork_rows(&dir, id, claim.txid()), 0);
            assert_eq!(
                row_json(
                    &dir,
                    "SELECT json_object('verified', verified) FROM core_instant_locks \
                     WHERE wallet_id = ?1 AND txid = ?2",
                    rusqlite::params![id.0.as_slice(), claim.txid().as_byte_array().as_slice()],
                )
                .as_deref(),
                Some("{\"verified\":0}"),
                "{pin:?} {open_n}: its lock row is back"
            );
            assert!(
                !spendable(&engine, &session, id, a),
                "{pin:?} {open_n}: A is held"
            );
            assert_eq!(
                setting(
                    &session,
                    &id.to_string(),
                    &format!("store_dropped:{}", claim.txid())
                ),
                None,
                "{pin:?} {open_n}: not dropped"
            );
            close(&engine, session);
        }
    }
}
