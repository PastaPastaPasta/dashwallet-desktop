//! Migration tests: a fresh database, an upgrade from the previous schema,
//! and a re-run, plus the DashPay tables' constraints.

use std::path::Path;

use refinery::Target;
use rusqlite::{Connection, params};

use crate::{AppDb, embedded};

const INITIAL: i64 = 2026100501;
const DASHPAY: i64 = 2026100801;
const TRUST: i64 = 2026100901;
const TRUST_DIGEST: i64 = 2026101101;
const OWNER: i64 = 2026101201;
const OUTCOMES: i64 = 2026101202;
const CONTACT_INFO: i64 = 2026101301;
const REMOVED_WALLETS: i64 = 2026101302;
const DP_EVENTS_PAGE: i64 = 2026101303;
const LOCK_INPUTS: i64 = 2026101401;
const GOV_REGISTRATIONS: i64 = 2026101901;
const PROTX_OPS: i64 = 2026102101;

const DP_TABLES: [&str; 10] = [
    "dp_avatar",
    "dp_contact_info",
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
    assert_eq!(db.schema_version().unwrap(), PROTX_OPS);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        versions(&conn),
        [
            INITIAL,
            DASHPAY,
            TRUST,
            TRUST_DIGEST,
            OWNER,
            OUTCOMES,
            CONTACT_INFO,
            REMOVED_WALLETS,
            DP_EVENTS_PAGE,
            LOCK_INPUTS,
            GOV_REGISTRATIONS,
            PROTX_OPS
        ]
    );
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
    assert_eq!(db.schema_version().unwrap(), PROTX_OPS);
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
    assert_eq!(
        versions(&conn),
        [
            INITIAL,
            DASHPAY,
            TRUST,
            TRUST_DIGEST,
            OWNER,
            OUTCOMES,
            CONTACT_INFO,
            REMOVED_WALLETS,
            DP_EVENTS_PAGE,
            LOCK_INPUTS,
            GOV_REGISTRATIONS,
            PROTX_OPS
        ]
    );
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
        assert_eq!(db.schema_version().unwrap(), PROTX_OPS);
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
    assert_eq!(
        versions(&conn),
        [
            INITIAL,
            DASHPAY,
            TRUST,
            TRUST_DIGEST,
            OWNER,
            OUTCOMES,
            CONTACT_INFO,
            REMOVED_WALLETS,
            DP_EVENTS_PAGE,
            LOCK_INPUTS,
            GOV_REGISTRATIONS,
            PROTX_OPS
        ]
    );
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
            (
                TRUST_DIGEST,
                "trust_digest".to_string(),
                CHECKSUM_TRUST_DIGEST
            ),
            (OWNER, "registration_owner".to_string(), CHECKSUM_OWNER),
            (
                OUTCOMES,
                "registration_outcomes".to_string(),
                CHECKSUM_OUTCOMES
            ),
            (
                CONTACT_INFO,
                "contact_info".to_string(),
                CHECKSUM_CONTACT_INFO
            ),
            (
                REMOVED_WALLETS,
                "removed_wallets".to_string(),
                CHECKSUM_REMOVED_WALLETS
            ),
            (
                DP_EVENTS_PAGE,
                "dp_events_page".to_string(),
                CHECKSUM_DP_EVENTS_PAGE
            ),
            (
                LOCK_INPUTS,
                "payment_lock_inputs".to_string(),
                CHECKSUM_LOCK_INPUTS
            ),
            (
                GOV_REGISTRATIONS,
                "governance_registrations".to_string(),
                CHECKSUM_GOV_REGISTRATIONS
            ),
            (PROTX_OPS, "mg_protx_ops".to_string(), CHECKSUM_PROTX_OPS),
        ]
    );
}

const CHECKSUM_INITIAL: u64 = 15039092102494657885;
const CHECKSUM_DASHPAY: u64 = 11805424486264248187;
const CHECKSUM_TRUST: u64 = 11010845116850883893;
const CHECKSUM_TRUST_DIGEST: u64 = 5295667777470269757;
const CHECKSUM_OWNER: u64 = 12040521178707991804;
const CHECKSUM_OUTCOMES: u64 = 9183630881616259306;
const CHECKSUM_CONTACT_INFO: u64 = 6330962680869650951;
const CHECKSUM_REMOVED_WALLETS: u64 = 15219075048203792465;
const CHECKSUM_DP_EVENTS_PAGE: u64 = 3406283282772175885;
const CHECKSUM_LOCK_INPUTS: u64 = 5402975231604979268;
const CHECKSUM_GOV_REGISTRATIONS: u64 = 16553696798211738698;
const CHECKSUM_PROTX_OPS: u64 = 8127766132692660851;

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
            conn.execute(
                "INSERT INTO dp_contact_info (wallet_id, identity, contact, digest, recorded_at)
                 VALUES (?1, ?2, 'bob', zeroblob(32), 1)",
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
    src.mark_verified("identity", "x", "v1:aa", 1).unwrap();
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
    // dp_contact_info is the wallet's but derived from Platform: a restore
    // reads it again (DP2-04).
    for table in DP_TABLES
        .iter()
        .filter(|t| !matches!(**t, "dp_avatar" | "dp_trust_verified" | "dp_contact_info"))
    {
        let one_wallet = format!("SELECT COUNT(*) FROM {table} WHERE wallet_id = 'aa'");
        let all = format!("SELECT COUNT(*) FROM {table}");
        assert_eq!(count(&conn, &one_wallet), count(&conn, &all), "{table}");
        assert!(count(&conn, &all) > 0, "{table}");
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_avatar"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_trust_verified"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dp_contact_info"), 0);
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

/// DEC-183: a verification record written before the digest column existed
/// verifies nothing; a new verification of the entity replaces it.
#[test]
fn verification_records_without_a_digest_verify_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    {
        let mut conn = Connection::open(&path).unwrap();
        embedded::migrations::runner()
            .set_target(Target::Version(TRUST as i32))
            .run(&mut conn)
            .unwrap();
        conn.execute(
            "INSERT INTO dp_trust_verified (kind, key, verified_at) VALUES ('identity', 'x', 9)",
            [],
        )
        .unwrap();
    }
    let db = AppDb::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), PROTX_OPS);
    assert!(db.verified_entities().unwrap().is_empty(), "the old row");
    assert_eq!(
        count(
            &db.conn(),
            "SELECT COUNT(*) FROM dp_trust_verified WHERE digest IS NULL"
        ),
        1,
        "kept, not deleted"
    );
    db.mark_verified("identity", "x", "v1:aa", 8).unwrap();
    assert_eq!(
        db.verified_entities().unwrap(),
        vec![("identity".into(), "x".into(), "v1:aa".into())]
    );
    let at: i64 = db
        .conn()
        .query_row("SELECT verified_at FROM dp_trust_verified", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(at, 8, "the old row's time does not carry over");
}

/// Opus-high M1: a database that already holds wallets when `trust_digest`
/// runs was written by a build that never set the fallback latch; it comes
/// out latched. One without wallets does not.
#[test]
fn an_installation_with_wallets_starts_latched() {
    let latch = |db: &AppDb| {
        db.setting(crate::GLOBAL_SCOPE, "trust.fallback_used_at")
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    let at_trust = |name: &str, wallet: bool| {
        let path = dir.path().join(name);
        let mut conn = Connection::open(&path).unwrap();
        embedded::migrations::runner()
            .set_target(Target::Version(TRUST as i32))
            .run(&mut conn)
            .unwrap();
        if wallet {
            conn.execute(
                "INSERT INTO wallets (wallet_id, name, created_at) VALUES ('aa', 'w', 1)",
                [],
            )
            .unwrap();
        }
        drop(conn);
        AppDb::open(&path).unwrap()
    };
    let upgraded = at_trust("with-wallet.sqlite", true);
    let at: u64 = latch(&upgraded).expect("latched").parse().unwrap();
    assert!(at > 1_700_000_000, "{at}: UNIX seconds");
    assert_eq!(latch(&at_trust("empty.sqlite", false)), None);
    assert_eq!(
        latch(&AppDb::open(&dir.path().join("fresh.sqlite")).unwrap()),
        None
    );
}

/// Tables with a `wallet_id` column, but `removed_wallets` (which records
/// the removal itself).
fn wallet_scoped_tables(conn: &Connection) -> Vec<String> {
    conn.prepare(
        "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) c
         WHERE m.type = 'table' AND c.name = 'wallet_id' AND m.name != 'removed_wallets'
         ORDER BY m.name",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Inserts a row of `wallet` into `table`, every NOT NULL column set to a
/// value of its type (the initial schema's CHECKs name the odd ones).
/// Returns the number of rows inserted.
fn insert_any_row(conn: &Connection, table: &str, wallet: &str) -> usize {
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
    let wallet = format!("'{wallet}'");
    let values: Vec<_> = columns
        .iter()
        .map(|(name, ty)| match (name.as_str(), ty.as_str()) {
            ("wallet_id", _) => wallet.as_str(),
            ("purpose", _) => "'send'",
            ("reason", _) => "'manual'",
            ("retryable", _) => "0",
            ("kind", _) if table == "labels" => "'tx'",
            ("digest", "BLOB") => "zeroblob(32)",
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
        .unwrap_or_else(|e| panic!("{insert}: {e}"))
}

/// `delete_wallet` lists its tables; the export finds them by their
/// `wallet_id` column. A later migration that adds a wallet-scoped table must
/// extend the list, and this fails until it does.
#[test]
fn delete_wallet_covers_every_wallet_scoped_table() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let scoped = wallet_scoped_tables(&conn);
    assert!(scoped.len() > 6, "{scoped:?}");
    for table in &scoped {
        assert_eq!(insert_any_row(&conn, table, W), 1, "{table}");
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

/// DEC-210: every table `delete_wallet` clears has an insert trigger that
/// skips rows of a removed wallet, and `WALLET_TABLES` is every
/// wallet-scoped table, so a table added later without a trigger fails here.
#[test]
fn every_wallet_table_has_a_removal_trigger() {
    let db = AppDb::open_in_memory().unwrap();
    let conn = db.conn();
    let mut listed = crate::WALLET_TABLES.map(String::from).to_vec();
    listed.sort_unstable();
    assert_eq!(wallet_scoped_tables(&conn), listed);
    for table in listed.iter().map(String::as_str).chain(["settings_kv"]) {
        let triggers = count(
            &conn,
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'
                 AND tbl_name = '{table}' AND sql LIKE '%BEFORE INSERT%'
                 AND sql LIKE '%removed_wallets%' AND sql LIKE '%RAISE(IGNORE)%'"
            ),
        );
        assert_eq!(triggers, 1, "{table}");
    }
}

/// After `delete_wallet`, inserts of the wallet's rows are skipped in every
/// table and settings scope of the wallet, while other wallets and the
/// global scope are written as before; `readmit_wallet` lets them land again.
#[test]
fn a_removed_wallets_rows_are_not_written_until_it_is_readmitted() {
    let db = AppDb::open_in_memory().unwrap();
    db.delete_wallet(W).unwrap();
    let settings = |db: &AppDb| {
        for scope in [
            W.to_owned(),
            crate::local_scope(W),
            W2.to_owned(),
            String::new(),
        ] {
            db.set_setting(&scope, "k", Some("v")).unwrap();
        }
        [W.to_owned(), crate::local_scope(W)].map(|scope| db.setting(&scope, "k").unwrap())
    };
    {
        let conn = db.conn();
        for table in crate::WALLET_TABLES {
            assert_eq!(insert_any_row(&conn, table, W), 0, "{table}");
            assert_eq!(insert_any_row(&conn, table, W2), 1, "{table}");
        }
    }
    assert_eq!(settings(&db), [None, None]);
    assert_eq!(db.setting(W2, "k").unwrap().as_deref(), Some("v"));
    assert_eq!(
        db.setting(crate::GLOBAL_SCOPE, "k").unwrap().as_deref(),
        Some("v")
    );
    assert!(matches!(
        db.add_receive_request(W, 1, "a", None, None, None),
        Err(crate::AppDbError::WalletRemoved(w)) if w == W
    ));

    db.readmit_wallet(W).unwrap();
    let conn = db.conn();
    for table in crate::WALLET_TABLES {
        assert_eq!(insert_any_row(&conn, table, W), 1, "{table}");
    }
    drop(conn);
    assert_eq!(settings(&db), [Some("v".into()), Some("v".into())]);
}

fn recorded(db: &AppDb) -> Vec<String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT wallet_id FROM removed_wallets ORDER BY wallet_id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// An open with no other instance of the file alive forgets the removed
/// wallets: no write of theirs can still be running (DEC-210 (3b)).
#[test]
fn an_open_with_no_other_instance_forgets_the_removed_wallets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let db = AppDb::open(&path).unwrap();
    db.delete_wallet(W).unwrap();
    db.delete_wallet(W2).unwrap();
    assert!(db.readmit_wallet(W2).unwrap());
    assert!(!db.readmit_wallet(W2).unwrap());
    drop(db);

    let db = AppDb::open(&path).unwrap();
    assert!(recorded(&db).is_empty());
    db.set_wallet_name(W, "n", 1).unwrap();
    assert_eq!(db.wallet_names().unwrap(), vec![(W.into(), "n".into())]);
}

/// Makes every delete of a `wallets` row fail, or stops doing so.
fn fail_wallet_deletes(db: &AppDb, fail: bool) {
    db.conn()
        .execute_batch(if fail {
            "CREATE TRIGGER fail_delete BEFORE DELETE ON wallets \
             BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;"
        } else {
            "DROP TRIGGER fail_delete;"
        })
        .unwrap();
}

/// A removal whose row delete fails still records the wallet as removed, so
/// its rows are not written; its readmission deletes the rows the removal
/// left before it lets the wallet in, or fails and keeps the record.
#[test]
fn a_readmission_finishes_a_removal_that_failed_part_way() {
    let db = AppDb::open_in_memory().unwrap();
    db.set_wallet_name(W, "old", 1).unwrap();
    db.set_wallet_name(W2, "other", 1).unwrap();
    fail_wallet_deletes(&db, true);
    assert!(db.delete_wallet(W).is_err());
    assert_eq!(recorded(&db), [W]);
    assert_eq!(insert_any_row(&db.conn(), "labels", W), 0);

    assert!(matches!(
        db.readmit_wallet(W),
        Err(crate::AppDbError::RemovalUnfinished { wallet, .. }) if wallet == W
    ));
    assert_eq!(recorded(&db), [W]);
    assert_eq!(db.wallet_names().unwrap().len(), 2);

    fail_wallet_deletes(&db, false);
    assert!(db.readmit_wallet(W).unwrap());
    assert!(recorded(&db).is_empty());
    assert_eq!(
        db.wallet_names().unwrap(),
        vec![(W2.into(), "other".into())]
    );
}

/// A removal that could not even record the wallet is remembered by the
/// instance: its readmission deletes the rows it left. A readmission of a
/// wallet never removed deletes nothing.
#[test]
fn a_readmission_finishes_a_removal_that_could_not_record_the_wallet() {
    let db = AppDb::open_in_memory().unwrap();
    db.set_wallet_name(W, "old", 1).unwrap();
    db.set_wallet_name(W2, "other", 1).unwrap();
    db.conn()
        .execute_batch(
            "CREATE TRIGGER fail_record BEFORE INSERT ON removed_wallets \
             BEGIN SELECT RAISE(FAIL, 'injected record failure'); END;",
        )
        .unwrap();
    assert!(db.delete_wallet(W).is_err());
    assert!(recorded(&db).is_empty());
    assert_eq!(db.wallet_names().unwrap().len(), 2);

    assert!(!db.readmit_wallet(W2).unwrap());
    assert!(db.readmit_wallet(W).unwrap());
    assert!(!db.readmit_wallet(W).unwrap());
    assert_eq!(
        db.wallet_names().unwrap(),
        vec![(W2.into(), "other".into())]
    );
}

/// An open with no other instance alive deletes the rows a removal that
/// failed part-way left before it forgets the record; while they cannot be
/// deleted it keeps the record.
#[test]
fn an_open_with_no_other_instance_finishes_a_removal_that_failed_part_way() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let db = AppDb::open(&path).unwrap();
    db.set_wallet_name(W, "old", 1).unwrap();
    fail_wallet_deletes(&db, true);
    assert!(db.delete_wallet(W).is_err());
    drop(db);

    let db = AppDb::open(&path).unwrap();
    assert_eq!(recorded(&db), [W]);
    assert_eq!(db.wallet_names().unwrap().len(), 1);
    fail_wallet_deletes(&db, false);
    drop(db);

    let db = AppDb::open(&path).unwrap();
    assert!(recorded(&db).is_empty());
    assert!(db.wallet_names().unwrap().is_empty());
}

/// While another instance of the file at `path` is open in this process (a
/// job a closed session left running holds one), an open, at `path` or at
/// `alias`, keeps the removed wallets recorded and a late write through the
/// other instance is skipped.
fn removal_is_kept_while_another_instance_is_open(path: &Path, alias: &Path) {
    let old = AppDb::open(path).unwrap();
    old.delete_wallet(W).unwrap();

    let new = AppDb::open(alias).unwrap();
    assert_eq!(recorded(&new), [W]);
    old.set_wallet_name(W, "late", 1).unwrap();
    assert!(new.wallet_names().unwrap().is_empty());
    // Not the newer instance's lock alone: `old` keeps it recorded too.
    drop(new);
    let newer = AppDb::open(alias).unwrap();
    assert_eq!(recorded(&newer), [W]);
    drop((old, newer));
    assert!(recorded(&AppDb::open(alias).unwrap()).is_empty());
}

#[test]
fn a_removal_is_kept_while_another_instance_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    removal_is_kept_while_another_instance_is_open(&path, &path);
}

/// [`removal_is_kept_while_another_instance_is_open`] at a path that is not
/// UTF-8.
#[cfg(target_os = "linux")]
#[test]
fn a_removal_is_kept_at_a_path_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let net = dir.path().join(std::ffi::OsStr::from_bytes(b"net-\xff"));
    std::fs::create_dir(&net).unwrap();
    let path = net.join(crate::APP_DB_FILE);
    assert!(path.to_str().is_none());
    removal_is_kept_while_another_instance_is_open(&path, &path);
}

/// The lock is a file of its own next to the database, owner-only.
#[test]
fn the_lock_file_is_next_to_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let _db = AppDb::open(&path).unwrap();
    let lock = dir.path().join(crate::APP_DB_LOCK_FILE);
    assert_eq!(crate::lock_path(&path), lock);
    let meta = std::fs::metadata(&lock).unwrap();
    assert!(meta.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o077, 0);
    }
}

/// Keeps every open in this process from probing the lock file until the
/// guard is dropped, once the child started meanwhile runs (it has printed,
/// say): until its exec it holds this process's descriptors
/// ([`crate::CHILD_STARTS`]).
fn child_starts() -> std::sync::RwLockWriteGuard<'static, ()> {
    crate::CHILD_STARTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run by [`a_removal_is_kept_while_another_process_has_the_file_open`] as
/// its child process (a test that does nothing otherwise): opens the file
/// `DW_APPDB_CHILD` names, removes `W`, and writes a row of `W` when its
/// parent says so.
#[test]
fn child_with_the_file_open() {
    let Some(path) = std::env::var_os("DW_APPDB_CHILD") else {
        return;
    };
    let db = AppDb::open(std::path::Path::new(&path)).unwrap();
    db.delete_wallet(W).unwrap();
    println!("removed");
    std::io::stdin().read_line(&mut String::new()).unwrap();
    db.set_wallet_name(W, "late", 1).unwrap();
    println!("written");
}

/// Reads `out` up to a line ending in `line` (libtest prints the test
/// name first on it).
fn wait_for_line(out: &mut impl std::io::BufRead, line: &str) {
    let mut read = String::new();
    loop {
        read.clear();
        assert!(out.read_line(&mut read).unwrap() > 0, "no {line:?}");
        if read.trim_end().ends_with(line) {
            return;
        }
    }
}

/// [`removal_is_kept_while_another_instance_is_open`] with the other
/// instance in another process.
#[test]
fn a_removal_is_kept_while_another_process_has_the_file_open() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let starting = child_starts();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "migration_tests::child_with_the_file_open",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DW_APPDB_CHILD", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    wait_for_line(&mut out, "removed");
    drop(starting);

    let db = AppDb::open(&path).unwrap();
    assert_eq!(recorded(&db), [W]);
    child.stdin.take().unwrap().write_all(b"write\n").unwrap();
    wait_for_line(&mut out, "written");
    assert!(child.wait().unwrap().success());
    assert!(db.wallet_names().unwrap().is_empty());
    drop(db);
    assert!(recorded(&AppDb::open(&path).unwrap()).is_empty());
}

/// The name an open uses for `path`, its directory resolved.
fn real(path: &Path) -> std::path::PathBuf {
    crate::resolve_directory(path).unwrap()
}

/// The open fails with the reason `what`, naming `path`.
fn assert_refused(opened: crate::Result<AppDb>, path: &Path, what: &str) {
    match opened {
        Ok(_) => panic!("{} opened", path.display()),
        Err(crate::AppDbError::File {
            path: named,
            source,
        }) => {
            assert_eq!(named, path);
            assert!(source.to_string().contains(what), "{source}");
        }
        Err(e) => panic!("{} refused for another reason: {e}", path.display()),
    }
}

/// An open at `path` fails with the reason `what`, naming it.
fn assert_refused_at(path: &Path, what: &str) {
    assert_refused(AppDb::open(path), &real(path), what);
}

/// `W`, removed through `old`, stays removed: a late write is skipped.
fn assert_removal_kept(old: &AppDb) {
    old.set_wallet_name(W, "late", 1).unwrap();
    assert!(old.wallet_names().unwrap().is_empty());
    assert_eq!(recorded(old), [W]);
}

/// An open through a directory symlink names the same lock file: the
/// removal is kept as at the real path.
#[cfg(unix)]
#[test]
fn a_removal_is_kept_through_a_symlinked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("a");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("b")).unwrap();
    removal_is_kept_while_another_instance_is_open(
        &real.join(crate::APP_DB_FILE),
        &dir.path().join("b").join(crate::APP_DB_FILE),
    );
}

#[cfg(unix)]
type Link = fn(&Path, &Path) -> std::io::Result<()>;

/// The removal outlives an open refused at `alias`, another name `link`
/// makes for the database open at `path` (it would lead to another lock
/// file, which the refused open does not create).
#[cfg(unix)]
fn an_alias_is_refused(path: &Path, alias: &Path, link: Link, what: &str) {
    let old = AppDb::open(path).unwrap();
    old.delete_wallet(W).unwrap();
    std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
    link(path, alias).unwrap();
    assert_refused_at(alias, what);
    assert!(!crate::lock_path(alias).exists());
    assert_removal_kept(&old);
}

/// A symlink to the database is refused, and not followed when nothing is
/// at its target yet.
#[cfg(unix)]
#[test]
fn a_symlink_to_the_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join(crate::APP_DB_FILE);
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    let alias = dir.path().join("b").join(crate::APP_DB_FILE);
    let symlink: Link = |path, link| std::os::unix::fs::symlink(path, link);
    an_alias_is_refused(&path, &alias, symlink, "is a symbolic link");
    let dangling = dir.path().join(crate::APP_DB_FILE);
    symlink(&dir.path().join("d"), &dangling).unwrap();
    assert_refused_at(&dangling, "is a symbolic link");
    assert!(!dir.path().join("d").exists());
}

/// A second hard link of the database is refused, at either name.
#[cfg(unix)]
#[test]
fn a_hard_link_to_the_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let alias = dir.path().join("b").join(crate::APP_DB_FILE);
    let hard_link: Link = |path, link| std::fs::hard_link(path, link);
    an_alias_is_refused(&path, &alias, hard_link, "has 2 hard links");
    assert_refused_at(&path, "has 2 hard links");
}

/// A directory where the database would be is refused as such, not left to
/// SQLite.
#[test]
fn a_database_that_is_not_a_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    std::fs::create_dir(&path).unwrap();
    assert_refused_at(&path, "is not a regular file");
}

/// A symlink or a second hard link in place of the lock file is refused,
/// and a symlink is not followed.
#[cfg(unix)]
#[test]
fn a_lock_file_with_another_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let lock = crate::lock_path(&real(&path));
    let elsewhere = dir.path().join("elsewhere.lock");
    std::os::unix::fs::symlink(&elsewhere, &lock).unwrap();
    assert_refused(AppDb::open(&path), &lock, "is a symbolic link");
    assert!(!elsewhere.exists());
    std::fs::write(&elsewhere, b"").unwrap();
    assert_refused(AppDb::open(&path), &lock, "is a symbolic link");

    std::fs::remove_file(&lock).unwrap();
    std::fs::hard_link(&elsewhere, &lock).unwrap();
    assert_refused(AppDb::open(&path), &lock, "has 2 hard links");
}

/// Run by [`open_in_child`] as its child process, in the directory its
/// parent chose: opens the name `DW_APPDB_NAME` gives and prints what
/// happened.
#[test]
fn child_opening_a_name() {
    let Some(name) = std::env::var_os("DW_APPDB_NAME") else {
        return;
    };
    match AppDb::open(Path::new(&name)) {
        Ok(db) => println!("recorded {:?}", recorded(&db)),
        Err(e) => println!("refused: {e}"),
    }
}

/// What [`child_opening_a_name`] printed opening `name` in `dir`.
fn open_in_child(dir: &Path, name: &str) -> String {
    let starting = child_starts();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "migration_tests::child_opening_a_name",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(dir)
        .env("DW_APPDB_NAME", name)
        .output()
        .unwrap();
    drop(starting);
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    for line in out.lines() {
        for said in ["refused: ", "recorded "] {
            if let Some(at) = line.find(said) {
                return line[at..].to_owned();
            }
        }
    }
    panic!("the child printed no outcome: {out}");
}

/// A `file:` name, which SQLite would read as a URI naming the database
/// open at `a/app.sqlite` (with its lock file at `file:a/app.sqlite.lock`),
/// is refused and the removal kept.
#[test]
fn a_uri_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join(crate::APP_DB_FILE);
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    // Not a name Windows allows.
    #[cfg(unix)]
    std::fs::create_dir(dir.path().join("file:a")).unwrap();
    let old = AppDb::open(&path).unwrap();
    old.delete_wallet(W).unwrap();
    for name in ["file:a/app.sqlite", "FILE:a/app.sqlite"] {
        let said = open_in_child(dir.path(), name);
        assert!(said.starts_with("refused: "), "{name}: {said}");
        assert!(said.contains("reads as a SQLite URI"), "{name}: {said}");
    }
    assert_removal_kept(&old);
}

/// A bare file name is the file in the working directory, with its lock.
#[test]
fn a_bare_name_opens_in_the_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let old = AppDb::open(&dir.path().join(crate::APP_DB_FILE)).unwrap();
    old.delete_wallet(W).unwrap();
    let said = open_in_child(dir.path(), crate::APP_DB_FILE);
    assert_eq!(said, format!("recorded {:?}", [W]));
    assert_removal_kept(&old);
}

#[cfg(unix)]
type Hook = std::thread::LocalKey<std::cell::Cell<Option<Box<dyn FnOnce()>>>>;

/// Opens `path` with `swap` run at the `hook`'s point of the open.
#[cfg(unix)]
fn open_swapped(
    path: &Path,
    hook: &'static Hook,
    swap: impl FnOnce() + 'static,
) -> crate::Result<AppDb> {
    hook.set(Some(Box::new(swap)));
    AppDb::open(path)
}

/// A database replaced by another file after the open checked it is
/// refused: the lock file belongs to the one checked.
#[cfg(unix)]
#[test]
fn a_database_replaced_during_the_open_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    drop(AppDb::open(&path).unwrap());
    let other = dir.path().join("other.sqlite");
    drop(AppDb::open(&other).unwrap());
    let (from, to) = (other.clone(), path.clone());
    let opened = open_swapped(&path, &crate::BEFORE_SQLITE_OPEN, move || {
        std::fs::rename(from, to).unwrap()
    });
    assert_refused(opened, &real(&path), "changed while it was opened");
}

/// A symlink put in place of the database after the open checked it is not
/// followed.
#[cfg(unix)]
#[test]
fn a_symlink_swapped_in_during_the_open_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    drop(AppDb::open(&path).unwrap());
    let target = dir.path().join("target.sqlite");
    let (link, to) = (path.clone(), target.clone());
    let opened = open_swapped(&path, &crate::BEFORE_SQLITE_OPEN, move || {
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(to, link).unwrap();
    });
    assert!(opened.is_err());
    assert!(!target.exists());
}

/// A lock file replaced between its two opens is refused: the shared lock
/// would be on another file than a later open's exclusive one. Removed and
/// created again, the new file would get the freed inode number back on
/// ext4 if the open did not keep its first handle; renamed over the old
/// one, it is created while the old one still exists.
#[cfg(unix)]
#[test]
fn a_lock_file_replaced_during_the_open_is_refused() {
    for rename in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::APP_DB_FILE);
        let lock = crate::lock_path(&real(&path));
        let replaced = lock.clone();
        let opened = open_swapped(&path, &crate::BEFORE_SHARED_LOCK, move || {
            if rename {
                let new = replaced.with_extension("new");
                std::fs::File::create(&new).unwrap();
                std::fs::rename(new, &replaced).unwrap();
            } else {
                std::fs::remove_file(&replaced).unwrap();
                std::fs::File::create(&replaced).unwrap();
            }
        });
        assert_refused(opened, &lock, "changed while it was opened");
    }
}

/// The lock file and the database are one file only under two mounts of it
/// (each name with one link); refused as well. Without file ids (Windows)
/// there is nothing to compare.
#[test]
fn a_lock_file_that_is_the_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    drop(AppDb::open(&path).unwrap());
    let database = crate::sole_database(&path).unwrap().unwrap();
    let lock = crate::lock_path(&path);
    let lock_id = crate::lock_id(&lock, &std::fs::File::open(&lock).unwrap()).unwrap();
    crate::refuse_same_file(&lock, lock_id, database).unwrap();
    let refused = crate::refuse_same_file(&lock, database, database);
    if database.is_some() {
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("is the database itself")
        );
    } else {
        refused.unwrap();
    }
}
