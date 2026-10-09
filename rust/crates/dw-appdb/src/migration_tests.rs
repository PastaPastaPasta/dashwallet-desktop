//! Migration tests: a fresh database, an upgrade from the previous schema,
//! and a re-run, plus the DashPay tables' constraints.

use refinery::Target;
use rusqlite::{Connection, params};

use crate::{AppDb, embedded};

const INITIAL: i64 = 2026100501;
const DASHPAY: i64 = 2026100801;
const TRUST: i64 = 2026100901;

const DP_TABLES: [&str; 9] = [
    "dp_avatar",
    "dp_contest_watch",
    "dp_events",
    "dp_main_identity",
    "dp_payment_lock",
    "dp_prefs",
    "dp_registration",
    "dp_trust_unverified",
    "dp_trust_verified",
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
    assert_eq!(db.schema_version().unwrap(), TRUST);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(versions(&conn), [INITIAL, DASHPAY, TRUST]);
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
    assert_eq!(db.schema_version().unwrap(), TRUST);
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
    assert_eq!(versions(&conn), [INITIAL, DASHPAY, TRUST]);
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
        assert_eq!(db.schema_version().unwrap(), TRUST);
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
    assert_eq!(versions(&conn), [INITIAL, DASHPAY, TRUST]);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_main_identity"), 1);
}

/// Shipped migrations are never edited (refinery refuses a database whose
/// applied checksum differs, but only after release): a change to one of
/// these files fails here, and the fix is a new migration.
#[test]
fn shipped_migrations_are_unchanged() {
    let runner = embedded::migrations::runner();
    let mut shipped: Vec<(i64, String, u64)> = runner
        .get_migrations()
        .iter()
        .map(|m| (i64::from(m.version()), m.name().to_string(), m.checksum()))
        .collect();
    // refinery embeds the files in directory-listing order, which the filesystem decides
    // (descending on the GitHub Linux runner).
    shipped.sort_unstable();
    assert_eq!(
        shipped,
        [
            (INITIAL, "initial".to_string(), CHECKSUM_INITIAL),
            (DASHPAY, "dashpay".to_string(), CHECKSUM_DASHPAY),
            (TRUST, "trust_verified".to_string(), CHECKSUM_TRUST),
        ]
    );
}

const CHECKSUM_INITIAL: u64 = 15039092102494657885;
const CHECKSUM_DASHPAY: u64 = 11805424486264248187;
const CHECKSUM_TRUST: u64 = 11010845116850883893;

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
                 (wallet_id, identity_index, label, funding, initial_profile, phase, error,
                  retryable, created_at, updated_at)
             VALUES (?1, ?2, 'alice', 'wallet', '{\"display_name\":\"Alice\"}', 'draft', ?3, ?4, 1, 1)",
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
    let unverified = "INSERT INTO dp_trust_unverified (wallet_id, kind, key, since)
                      VALUES ('aa', 'identity', 'x', 1)";
    conn.execute(unverified, []).unwrap();
    conn.execute(unverified, []).unwrap_err();
    // The same entity of another wallet is its own row.
    conn.execute(
        "INSERT INTO dp_trust_unverified (wallet_id, kind, key, since)
         VALUES ('bb', 'identity', 'x', 1)",
        [],
    )
    .unwrap();
}

#[test]
fn initial_profile_is_optional_and_kept_verbatim() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let profile = r#"{"display_name":"Alice é","public_message":"hi"}"#;
    for initial in [None, Some(profile)] {
        conn.execute(
            "INSERT INTO dp_registration
                 (wallet_id, label, funding, initial_profile, phase, created_at, updated_at)
             VALUES (?1, 'alice', 'wallet', ?2, 'draft', 1, 1)",
            params![W, initial],
        )
        .unwrap();
    }
    let stored: Vec<Option<String>> = conn
        .prepare("SELECT initial_profile FROM dp_registration ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(stored, [None, Some(profile.to_string())]);
}

/// One asset lock funds one flow (review m1): the outpoint is unique per
/// wallet, and rows with no outpoint yet are not constrained.
#[test]
fn an_asset_lock_outpoint_binds_one_flow_per_wallet() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let registration = |wallet: &str, outpoint: Option<&str>| {
        conn.execute(
            "INSERT INTO dp_registration
                 (wallet_id, label, funding, asset_lock_outpoint, phase, created_at, updated_at)
             VALUES (?1, 'alice', 'wallet', ?2, 'funding_sent', 1, 1)",
            params![wallet, outpoint],
        )
    };
    // Flows that have not built a lock yet are any number.
    registration(W, None).unwrap();
    registration(W, None).unwrap();
    registration(W, Some("aa00:0")).unwrap();
    // The same lock in a second flow is refused, in any phase.
    registration(W, Some("aa00:0")).unwrap_err();
    // Another output of the same transaction, and another wallet, are fine.
    registration(W, Some("aa00:1")).unwrap();
    registration(W2, Some("aa00:0")).unwrap();
    // A flow may record its outpoint later; a second flow cannot take it.
    conn.execute(
        "UPDATE dp_registration SET asset_lock_outpoint = 'bb11:0'
         WHERE id = (SELECT MIN(id) FROM dp_registration WHERE asset_lock_outpoint IS NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE dp_registration SET asset_lock_outpoint = 'bb11:0'
         WHERE id = (SELECT MAX(id) FROM dp_registration WHERE asset_lock_outpoint IS NULL)",
        [],
    )
    .unwrap_err();
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
            // The same entity is flagged in both wallets.
            conn.execute(
                "INSERT INTO dp_trust_unverified (wallet_id, kind, key, since)
                 VALUES (?1, 'identity', 'x', 1)",
                [wallet],
            )
            .unwrap();
        }
    }

    db.delete_wallet(W).unwrap();

    let conn = db.conn();
    for table in [
        "dp_main_identity",
        "dp_registration",
        "dp_events",
        "dp_payment_lock",
        "dp_trust_unverified",
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
}

#[test]
fn dashpay_rows_travel_in_the_wallet_export() {
    const PROFILE: &str = r#"{"display_name":"Alice","avatar_url":"https://example.org/a.png"}"#;
    let src = AppDb::open_in_memory().unwrap();
    {
        let conn = src.conn();
        for (wallet, identity) in [(W, "id-a"), (W2, "id-b")] {
            conn.execute(
                "INSERT INTO dp_main_identity (wallet_id, identity) VALUES (?1, ?2)",
                [wallet, identity],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_registration
                     (wallet_id, label, funding, initial_profile, asset_lock_outpoint, phase,
                      created_at, updated_at)
                 VALUES (?1, 'alice', 'wallet', ?2, 'aa00:0', 'funding_sent', 1, 1)",
                [wallet, PROFILE],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_contest_watch (wallet_id, identity, label, ends_at)
                 VALUES (?1, ?2, 'alice', 9)",
                [wallet, identity],
            )
            .unwrap();
            // Two events in a known order, one of them read.
            for (kind, read_at) in [("first", Some(5)), ("second", None)] {
                conn.execute(
                    "INSERT INTO dp_events (wallet_id, identity, kind, at, read_at)
                     VALUES (?1, ?2, ?3, 1, ?4)",
                    params![wallet, identity, kind, read_at],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO dp_payment_lock (wallet_id, identity, contact, txid, since)
                 VALUES (?1, ?2, 'bob', 't', 1)",
                [wallet, identity],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_trust_unverified (wallet_id, kind, key, since)
                 VALUES (?1, 'identity', 'x', 1)",
                [wallet],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO dp_prefs (wallet_id, identity, key, value)
                 VALUES (?1, ?2, 'k', 'v')",
                [wallet, identity],
            )
            .unwrap();
        }
        // Network-wide: not part of any wallet's export.
        conn.execute(
            "INSERT INTO dp_avatar (url_sha, status, fetched_at) VALUES ('u', 'ok', 1)",
            [],
        )
        .unwrap();
    }
    src.mark_verified("identity", "x", 1).unwrap();
    let rows = src.export_wallet_rows(W).unwrap();
    let names: Vec<_> = rows.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(
        names,
        [
            "dp_contest_watch",
            "dp_events",
            "dp_main_identity",
            "dp_payment_lock",
            "dp_prefs",
            "dp_registration",
            "dp_trust_unverified",
        ]
    );
    for table in &rows {
        assert!(
            !table.columns.contains(&"id".to_string()),
            "{}",
            table.table
        );
    }

    let dst = AppDb::open_in_memory().unwrap();
    let total: usize = rows.iter().map(|t| t.rows.len()).sum();
    assert_eq!(dst.import_wallet_rows(W, &rows).unwrap(), total);
    // Idempotent, and another wallet's rows are refused.
    assert_eq!(dst.import_wallet_rows(W, &rows).unwrap(), 0);
    assert_eq!(dst.import_wallet_rows(W2, &rows).unwrap(), 0);

    let conn = dst.conn();
    // dp_avatar and dp_trust_verified are global, and not exported: a
    // restored wallet's Platform data verifies again (DEC-125).
    for table in DP_TABLES
        .iter()
        .filter(|t| !matches!(**t, "dp_avatar" | "dp_trust_verified"))
    {
        let one_wallet = format!("SELECT COUNT(*) FROM {table} WHERE wallet_id = 'aa'");
        let all = format!("SELECT COUNT(*) FROM {table}");
        assert_eq!(count(&conn, &one_wallet), count(&conn, &all), "{table}");
        assert!(count(&conn, &all) > 0, "{table}");
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_avatar"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_trust_verified"), 0);
    // The initial profile and the outpoint survive; the row got a new id.
    let (profile, outpoint): (String, String) = conn
        .query_row(
            "SELECT initial_profile, asset_lock_outpoint FROM dp_registration",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((profile.as_str(), outpoint.as_str()), (PROFILE, "aa00:0"));
    // Events keep their order (ids are renumbered in export order) and their
    // read state.
    let events: Vec<(String, Option<i64>)> = conn
        .prepare("SELECT kind, read_at FROM dp_events ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        events,
        [("first".to_string(), Some(5)), ("second".to_string(), None)]
    );
}

/// Restoring next to a flow that already holds the same asset lock keeps the
/// existing flow (the unique outpoint, review m1) instead of failing the
/// restore or doubling the flow.
#[test]
fn restoring_a_registration_with_a_held_outpoint_adds_no_second_flow() {
    let insert = "INSERT INTO dp_registration
                      (wallet_id, label, funding, asset_lock_outpoint, phase, created_at, updated_at)
                  VALUES ('aa', 'alice', 'wallet', 'aa00:0', ?1, 1, 1)";
    let src = AppDb::open_in_memory().unwrap();
    src.conn().execute(insert, ["key_prepared"]).unwrap();
    let rows = src.export_wallet_rows(W).unwrap();

    let dst = AppDb::open_in_memory().unwrap();
    // The target machine is already further along with the same lock.
    dst.conn().execute(insert, ["done"]).unwrap();
    assert_eq!(dst.import_wallet_rows(W, &rows).unwrap(), 0);
    let phases: Vec<String> = dst
        .conn()
        .prepare("SELECT phase FROM dp_registration")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(phases, ["done"]);
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
    for index in ["dp_events_unread", "dp_registration_lock"] {
        assert!(
            fresh
                .iter()
                .any(|(kind, name, ..)| kind == "index" && name == index),
            "{index}"
        );
    }
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
