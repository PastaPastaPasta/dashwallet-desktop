//! Migration tests: a fresh database, an upgrade from the previous schema,
//! and a re-run, plus the DashPay tables' constraints.

use refinery::Target;
use rusqlite::{Connection, params};

use crate::{AppDb, embedded};

const INITIAL: i64 = 2026100501;
const DASHPAY: i64 = 2026100801;

const DP_TABLES: [&str; 8] = [
    "dp_avatar",
    "dp_contest_watch",
    "dp_events",
    "dp_main_identity",
    "dp_payment_lock",
    "dp_prefs",
    "dp_registration",
    "dp_trust_unverified",
];

const W: &str = "aa";
const W2: &str = "bb";

fn tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'dp\\_%' ESCAPE '\\' \
             ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn versions(conn: &Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT version FROM refinery_schema_history ORDER BY version")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// A database as the previous release left it: only the initial migration.
fn previous_schema(path: &std::path::Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    embedded::migrations::runner()
        .set_target(Target::Version(INITIAL as i32))
        .run(&mut conn)
        .unwrap();
    assert_eq!(versions(&conn), [INITIAL]);
    assert!(tables(&conn).is_empty());
    conn
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn fresh_database_gets_every_migration_and_the_dp_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let db = AppDb::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), DASHPAY);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(versions(&conn), [INITIAL, DASHPAY]);
    assert_eq!(tables(&conn), DP_TABLES);
    // Every dp_ table is STRICT, like the rest of the schema.
    for table in DP_TABLES {
        let strict: i64 = conn
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(strict, 1, "{table} is STRICT");
    }
}

#[test]
fn upgrade_from_the_previous_schema_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    {
        let conn = previous_schema(&path);
        conn.execute(
            "INSERT INTO wallets (wallet_id, name, created_at) VALUES (?1, 'Main', 1)",
            [W],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO labels (wallet_id, kind, target, label, updated_at)
             VALUES (?1, 'tx', 't', 'rent', 2)",
            [W],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings_kv (scope, key, value) VALUES ('', 'k', 'v')",
            [],
        )
        .unwrap();
    }

    let db = AppDb::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), DASHPAY);
    assert_eq!(db.wallet_names().unwrap(), vec![(W.into(), "Main".into())]);
    assert_eq!(
        db.label(W, crate::LabelKind::Tx, "t").unwrap().as_deref(),
        Some("rent")
    );
    assert_eq!(
        db.setting(crate::GLOBAL_SCOPE, "k").unwrap().as_deref(),
        Some("v")
    );
    drop(db);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(versions(&conn), [INITIAL, DASHPAY]);
    assert_eq!(tables(&conn), DP_TABLES);
    for table in DP_TABLES {
        assert_eq!(count(&conn, &format!("SELECT COUNT(*) FROM {table}")), 0);
    }
}

#[test]
fn reopening_and_rerunning_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    {
        let _ = previous_schema(&path);
    }
    for _ in 0..3 {
        let db = AppDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), DASHPAY);
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO dp_main_identity (wallet_id, identity) VALUES (?1, 'id1')",
                [W],
            )
            .unwrap();
    }

    // The runner again on the same connection applies nothing.
    let mut conn = Connection::open(&path).unwrap();
    let report = embedded::migrations::runner()
        .set_abort_divergent(true)
        .set_abort_missing(true)
        .run(&mut conn)
        .unwrap();
    assert!(report.applied_migrations().is_empty());
    assert_eq!(versions(&conn), [INITIAL, DASHPAY]);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_main_identity"), 1);
}

/// Shipped migrations are never edited (refinery refuses a database whose
/// applied checksum differs, but only after release): a change to one of
/// these files fails here, and the fix is a new migration.
#[test]
fn shipped_migrations_are_unchanged() {
    let runner = embedded::migrations::runner();
    let shipped: Vec<(i64, String, u64)> = runner
        .get_migrations()
        .iter()
        .map(|m| (i64::from(m.version()), m.name().to_string(), m.checksum()))
        .collect();
    assert_eq!(
        shipped,
        [
            (INITIAL, "initial".to_string(), CHECKSUM_INITIAL),
            (DASHPAY, "dashpay".to_string(), CHECKSUM_DASHPAY),
        ]
    );
}

const CHECKSUM_INITIAL: u64 = 15039092102494657885;
const CHECKSUM_DASHPAY: u64 = 17807412343103651304;

#[test]
fn journal_writes_are_idempotent_per_kind_contact_and_ref() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let insert = |wallet: &str, kind: &str, contact: &str, reference: &str| {
        conn.execute(
            "INSERT OR IGNORE INTO dp_events (wallet_id, identity, kind, contact, ref, at)
             VALUES (?1, 'me', ?2, ?3, ?4, 10)",
            params![wallet, kind, contact, reference],
        )
        .unwrap()
    };
    assert_eq!(insert(W, "payment_received", "bob", "txid1"), 1);
    assert_eq!(insert(W, "payment_received", "bob", "txid1"), 0);
    assert_eq!(insert(W, "payment_received", "bob", "txid2"), 1);
    assert_eq!(insert(W, "contact_request", "bob", "txid1"), 1);
    // The same event of another wallet is its own row.
    assert_eq!(insert(W2, "payment_received", "bob", "txid1"), 1);
    // An event with no contact defaults to '' and dedups too; a name event
    // carries its label in `ref`, so a second name is a second event.
    for (label, expected) in [("alice", 1), ("alice", 0), ("alice2", 1)] {
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO dp_events (wallet_id, identity, kind, ref, at)
                 VALUES (?1, 'me', 'username_registered', ?2, 10)",
                [W, label],
            )
            .unwrap();
        assert_eq!(n, expected, "{label}");
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_events"), 6);
    let unread = "SELECT COUNT(*) FROM dp_events WHERE read_at IS NULL";
    assert_eq!(count(&conn, unread), 6);
}

#[test]
fn constraints_reject_out_of_domain_rows() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let registration = |index: Option<i64>, error: Option<&str>, retryable: i64| {
        conn.execute(
            "INSERT INTO dp_registration
                 (wallet_id, identity_index, label, funding, phase, error, retryable,
                  created_at, updated_at)
             VALUES (?1, ?2, 'alice', 'wallet', 'draft', ?3, ?4, 1, 1)",
            params![W, index, error, retryable],
        )
    };
    registration(None, None, 0).unwrap();
    registration(Some(0), Some("dapi"), 1).unwrap();
    registration(Some(0), Some("dapi"), 0).unwrap();
    registration(Some(-1), None, 0).unwrap_err();
    registration(Some(1), Some("dapi"), 2).unwrap_err();
    // Only a failed flow (one with an error) can be retryable.
    registration(Some(1), None, 1).unwrap_err();

    conn.execute(
        "INSERT INTO dp_avatar (url_sha, status, fetched_at, bytes) VALUES ('u', 'ok', 1, 10)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO dp_avatar (url_sha, status, fetched_at, bytes) VALUES ('v', 'ok', 1, -1)",
        [],
    )
    .unwrap_err();

    // Keyed tables refuse a duplicate key.
    let main = "INSERT INTO dp_main_identity (wallet_id, identity) VALUES ('aa', 'me')";
    conn.execute(main, []).unwrap();
    conn.execute(main, []).unwrap_err();
    let unverified =
        "INSERT INTO dp_trust_unverified (kind, key, since) VALUES ('identity', 'x', 1)";
    conn.execute(unverified, []).unwrap();
    conn.execute(unverified, []).unwrap_err();
}

#[test]
fn removing_a_wallet_removes_its_dashpay_rows_only() {
    let db = AppDb::open_in_memory().unwrap();
    {
        let conn = db.conn();
        for (wallet, identity) in [(W, "id-a"), (W2, "id-b")] {
            conn.execute(
                "INSERT INTO dp_main_identity (wallet_id, identity) VALUES (?1, ?2)",
                [wallet, identity],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_registration
                     (wallet_id, label, funding, phase, created_at, updated_at)
                 VALUES (?1, 'alice', 'wallet', 'draft', 1, 1)",
                [wallet],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_events (wallet_id, identity, kind, at) VALUES (?1, ?2, 'k', 1)",
                [wallet, identity],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_payment_lock (wallet_id, identity, contact, txid, since)
                 VALUES (?1, ?2, 'bob', 't', 1)",
                [wallet, identity],
            )
            .unwrap();
            // Both wallets also know identity 'shared', which no other row names.
            for identity in [identity, "shared"] {
                conn.execute(
                    "INSERT INTO dp_contest_watch (wallet_id, identity, label, ends_at)
                     VALUES (?1, ?2, 'alice', 9)",
                    [wallet, identity],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO dp_prefs (wallet_id, identity, key, value)
                     VALUES (?1, ?2, 'k', 'v')",
                    [wallet, identity],
                )
                .unwrap();
            }
        }
        // Not wallet-scoped: untouched.
        conn.execute(
            "INSERT INTO dp_trust_unverified (kind, key, since) VALUES ('identity', 'x', 1)",
            [],
        )
        .unwrap();
    }

    db.delete_wallet(W).unwrap();

    let conn = db.conn();
    for table in [
        "dp_main_identity",
        "dp_registration",
        "dp_events",
        "dp_payment_lock",
    ] {
        let only_b = format!("SELECT COUNT(*) FROM {table} WHERE wallet_id = 'bb'");
        assert_eq!(count(&conn, &only_b), 1, "{table}");
        assert_eq!(
            count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
            1,
            "{table}"
        );
    }
    // Two rows each (its own identity and the shared one), all wallet B's.
    for table in ["dp_contest_watch", "dp_prefs"] {
        let only_b = format!("SELECT COUNT(*) FROM {table} WHERE wallet_id = 'bb'");
        assert_eq!(count(&conn, &only_b), 2, "{table}");
        assert_eq!(
            count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
            2,
            "{table}"
        );
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_trust_unverified"), 1);
}

#[test]
fn dashpay_rows_travel_in_the_wallet_export() {
    let src = AppDb::open_in_memory().unwrap();
    src.conn()
        .execute(
            "INSERT INTO dp_main_identity (wallet_id, identity) VALUES (?1, 'id-a')",
            [W],
        )
        .unwrap();
    src.conn()
        .execute(
            "INSERT INTO dp_registration (wallet_id, label, funding, phase, created_at, updated_at)
             VALUES (?1, 'alice', 'wallet', 'draft', 1, 1)",
            [W],
        )
        .unwrap();
    let rows = src.export_wallet_rows(W).unwrap();
    let names: Vec<_> = rows.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(names, ["dp_main_identity", "dp_registration"]);

    let dst = AppDb::open_in_memory().unwrap();
    assert_eq!(dst.import_wallet_rows(W, &rows).unwrap(), 2);
    assert_eq!(dst.import_wallet_rows(W, &rows).unwrap(), 0);
    assert_eq!(
        count(&dst.conn(), "SELECT COUNT(*) FROM dp_registration"),
        1
    );
}

/// The upgrade path and a fresh open end in the same schema, indexes included.
#[test]
fn upgraded_schema_equals_the_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let upgraded = dir.path().join("upgraded.sqlite");
    drop(previous_schema(&upgraded));
    drop(AppDb::open(&upgraded).unwrap());
    let fresh = dir.path().join("fresh.sqlite");
    drop(AppDb::open(&fresh).unwrap());

    let schema = |path: &std::path::Path| -> Vec<(String, String, String, Option<String>)> {
        let conn = Connection::open(path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let (fresh, upgraded) = (schema(&fresh), schema(&upgraded));
    assert!(
        fresh
            .iter()
            .any(|(kind, name, ..)| kind == "index" && name == "dp_events_unread")
    );
    assert_eq!(fresh, upgraded);
}

/// `delete_wallet` lists its tables; the export finds them by their
/// `wallet_id` column. A later migration that adds a wallet-scoped table must
/// extend the list, and this fails until it does.
#[test]
fn delete_wallet_covers_every_wallet_scoped_table() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let scoped: Vec<String> = conn
        .prepare(
            "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) c
             WHERE m.type = 'table' AND c.name = 'wallet_id' ORDER BY m.name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(scoped.len() > 6, "{scoped:?}");
    for table in &scoped {
        // A row with every NOT NULL column set to a value of its type (the
        // initial schema's CHECKs name the odd ones) and the wallet column W.
        let columns: Vec<(String, String)> = conn
            .prepare(&format!(
                "SELECT name, type FROM pragma_table_info('{table}')
                 WHERE \"notnull\" = 1 OR name = 'wallet_id'"
            ))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let names: Vec<_> = columns.iter().map(|(name, _)| name.as_str()).collect();
        let values: Vec<_> = columns
            .iter()
            .map(|(name, ty)| match (name.as_str(), ty.as_str()) {
                ("wallet_id", _) => "'aa'",
                ("purpose", _) => "'send'",
                ("reason", _) => "'manual'",
                ("retryable", _) => "0",
                ("kind", _) if table == "labels" => "'tx'",
                (_, "INTEGER") => "1",
                _ => "'x'",
            })
            .collect();
        let insert = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            names.join(", "),
            values.join(", ")
        );
        conn.execute(&insert, [])
            .unwrap_or_else(|e| panic!("{insert}: {e}"));
    }
    drop(conn);

    db.delete_wallet(W).unwrap();
    let conn = db.conn();
    for table in &scoped {
        assert_eq!(
            count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
}
