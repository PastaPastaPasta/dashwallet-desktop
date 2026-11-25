//! Q93: fork V3 accepted without P8, its `core_dropped_unsigned_spends`
//! kept exactly as found (T2), the coins a drop gave back still unspent
//! (T3 b), and only user removal's cascade deletes rows (T3 c). Rows here
//! are built by hand on the v0.3.0 fixture; the fixture made by the fork's
//! own drop path is T3 (a).

use super::*;
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::sql::open_immutable;

const TABLE: &str = "core_dropped_unsigned_spends";

/// The table's schema objects and every row, as SQLite quotes them.
fn dump(path: &Path) -> (Vec<String>, Vec<String>) {
    let conn = open_immutable(path).unwrap();
    let schema = conn
        .prepare("SELECT type || ' ' || name || ' ' || sql FROM sqlite_master WHERE tbl_name = ?1 ORDER BY name")
        .unwrap()
        .query_map([TABLE], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let rows = conn
        .prepare(&format!(
            "SELECT quote(id) || ' ' || quote(wallet_id) || ' ' || quote(txid) || ' ' || \
             quote(record_blob) || ' ' || quote(islock_blob) || ' ' || quote(islock_verified) || ' ' || \
             quote(deleted_outputs) || ' ' || quote(released_coins) || ' ' || quote(dropped_at) \
             FROM {TABLE} ORDER BY id"
        ))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (schema, rows)
}

/// One row per wallet; each wallet's first unspent coin is its released coin.
fn add_dropped_rows(f: &Fixture) {
    exec(
        &f.wallet(),
        &format!(
            "INSERT INTO {TABLE} (wallet_id, txid, record_blob, islock_blob, islock_verified, \
             deleted_outputs, released_coins, dropped_at) \
             SELECT w.wallet_id, randomblob(32), x'01', x'0203', 1, '', \
                    COALESCE((SELECT hex(u.outpoint) FROM core_utxos u \
                              WHERE u.wallet_id = w.wallet_id AND u.spent = 0 \
                              ORDER BY u.outpoint LIMIT 1), ''), 1795567800 \
             FROM wallets w ORDER BY w.wallet_id"
        ),
    );
}

fn released(f: &Fixture) -> Vec<(Vec<u8>, i64)> {
    let conn = open_immutable(&f.wallet()).unwrap();
    conn.prepare(&format!(
        "SELECT u.outpoint, u.spent FROM core_utxos u JOIN {TABLE} d \
         ON d.wallet_id = u.wallet_id AND d.released_coins = hex(u.outpoint) ORDER BY u.outpoint"
    ))
    .unwrap()
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// The whole upgrade: prepare, reconcile, the upstream open and load, close,
/// then the retire on the next start and a second open.
fn upgrade_fully(f: &Fixture) {
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    drop(open_and_load(&f.net));
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Retired { .. }
    ));
    drop(open_and_load(&f.net));
}

#[test]
fn v3_rows_and_released_coins_survive_the_upgrade_unchanged() {
    let f = fixture("v0.3.0");
    add_dropped_rows(&f);
    let before = dump(&f.wallet());
    let coins = released(&f);
    assert!(!coins.is_empty());
    assert!(coins.iter().all(|(_, spent)| *spent == 0));
    upgrade_fully(&f);
    assert_eq!(dump(&f.wallet()), before);
    assert_eq!(released(&f), coins, "the released coins are still unspent");
    let conn = open_immutable(&f.wallet()).unwrap();
    let retired: Vec<i64> = conn
        .prepare("SELECT version FROM dw_fork_schema_history_retired ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(retired, [1, 2, 3], "V3's history row is kept");
}

#[test]
fn removing_one_wallet_deletes_only_its_v3_rows() {
    let f = fixture("v0.3.0");
    add_dropped_rows(&f);
    upgrade_fully(&f);
    let (_, rows) = dump(&f.wallet());
    let wallets: Vec<Vec<u8>> = open_immutable(&f.wallet())
        .unwrap()
        .prepare("SELECT wallet_id FROM wallets ORDER BY wallet_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(wallets.len() >= 2 && rows.len() == wallets.len());
    let removed: [u8; 32] = wallets[0].clone().try_into().unwrap();
    let (persister, token) = open_and_load(&f.net);
    persister.delete_wallet(removed).unwrap();
    drop(persister);
    drop(token);
    let (_, after) = dump(&f.wallet());
    let removed_quoted = format!(
        " X'{}' ",
        removed
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    );
    let kept: Vec<String> = rows
        .into_iter()
        .filter(|r| !r.contains(&removed_quoted))
        .collect();
    assert_eq!(after, kept);
    assert_eq!(after.len(), wallets.len() - 1);
}
