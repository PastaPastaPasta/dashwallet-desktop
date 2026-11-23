//! Export and import of one wallet's rows, for `.dwbackup` bundles
//! (QT-110, QT-116). Generic over the schema: every table with a
//! `wallet_id` column contributes the wallet's rows, and `settings_kv` its
//! rows scoped to the wallet, so tables added by later migrations are
//! carried without changes here, except the ones in [`NOT_EXPORTED`].

use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, ValueRef};

use crate::{AppDb, Result};

/// One SQLite value.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// A TEXT value whose bytes are not valid UTF-8, kept as stored.
    NonUtf8Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl From<ValueRef<'_>> for SqlValue {
    fn from(v: ValueRef<'_>) -> Self {
        match v {
            ValueRef::Null => SqlValue::Null,
            ValueRef::Integer(i) => SqlValue::Integer(i),
            ValueRef::Real(r) => SqlValue::Real(r),
            ValueRef::Text(t) => match std::str::from_utf8(t) {
                Ok(s) => SqlValue::Text(s.to_owned()),
                Err(_) => SqlValue::NonUtf8Text(t.to_vec()),
            },
            ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
        }
    }
}

impl ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            SqlValue::Null => ValueRef::Null,
            SqlValue::Integer(i) => ValueRef::Integer(*i),
            SqlValue::Real(r) => ValueRef::Real(*r),
            SqlValue::Text(t) => ValueRef::Text(t.as_bytes()),
            SqlValue::NonUtf8Text(t) => ValueRef::Text(t),
            SqlValue::Blob(b) => ValueRef::Blob(b),
        }))
    }
}

/// The rows of one table that belong to a wallet.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRows {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

/// Column name, and whether it is the table's INTEGER rowid alias.
fn columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<(String, bool)>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let cols: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(5)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let pk_count = cols.iter().filter(|c| c.2 > 0).count();
    Ok(cols
        .into_iter()
        .map(|(name, ty, pk)| {
            let rowid_alias = pk > 0 && pk_count == 1 && ty.eq_ignore_ascii_case("INTEGER");
            (name, rowid_alias)
        })
        .collect())
}

/// Wallet tables a `.dwbackup` does not carry, neither out nor in: what
/// they hold is derived from the network and a restore records it again.
/// `dp_contact_info` (DP2-04): what Platform holds of the private details,
/// which the restore's own read of the `contactInfo` documents records.
/// `removed_wallets` (DEC-210): which wallets were removed, forgotten by an
/// open with no other instance of the database.
/// `masternode_ops` (MG-04 D24): this installation's ProTx attempts, whose
/// fee inputs and locks are this machine's; a sent one is in the history a
/// restore reads again. `mg_proposals_kept` (MG-06 D23): copies of a removed
/// wallet's proposals, which `mg_proposals` carries.
/// `shared_mn_sessions` (MG-04b D19): this installation's sessions, whose
/// reserved coins and locks are this machine's; a sent one is in the history
/// a restore reads again. `shared_mn_standby` (MG-04b D19): signed
/// dissolutions of this installation; the exported file is the backup.
const NOT_EXPORTED: [&str; 6] = [
    "dp_contact_info",
    "masternode_ops",
    "removed_wallets",
    "mg_proposals_kept",
    "shared_mn_sessions",
    "shared_mn_standby",
];

/// Tables and the column that scopes their rows to a wallet.
fn scoped_tables(conn: &rusqlite::Connection) -> Result<Vec<(String, &'static str)>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         AND name NOT LIKE 'refinery_%' ORDER BY name",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for name in names {
        if NOT_EXPORTED.contains(&name.as_str()) {
            continue;
        }
        let cols = columns(conn, &name)?;
        if cols.iter().any(|(c, _)| c == "wallet_id") {
            out.push((name, "wallet_id"));
        } else if name == "settings_kv" {
            out.push((name, "scope"));
        }
    }
    Ok(out)
}

impl AppDb {
    /// Every row that belongs to `wallet_id`, table by table. The rowid
    /// alias of a table (e.g. `receive_requests.id`) is left out: a restore
    /// numbers the rows anew.
    pub fn export_wallet_rows(&self, wallet_id: &str) -> Result<Vec<TableRows>> {
        let conn = self.conn();
        let mut out = Vec::new();
        for (table, scope) in scoped_tables(&conn)? {
            let cols: Vec<String> = columns(&conn, &table)?
                .into_iter()
                .filter(|(_, alias)| !alias)
                .map(|(c, _)| c)
                .collect();
            let list = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT {list} FROM \"{table}\" WHERE \"{scope}\" = ?1 ORDER BY rowid"
            ))?;
            let rows: Vec<Vec<SqlValue>> = stmt
                .query_map([wallet_id], |r| {
                    (0..cols.len())
                        .map(|i| r.get_ref(i).map(SqlValue::from))
                        .collect()
                })?
                .collect::<rusqlite::Result<_>>()?;
            if !rows.is_empty() {
                out.push(TableRows {
                    table,
                    columns: cols,
                    rows,
                });
            }
        }
        Ok(out)
    }

    /// Inserts exported rows into the tables of this database, keeping rows
    /// that already exist (`INSERT OR IGNORE`). Tables and columns this
    /// schema does not have are skipped; every row must belong to
    /// `wallet_id`. Returns the number of rows inserted.
    pub fn import_wallet_rows(&self, wallet_id: &str, tables: &[TableRows]) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let inserted = insert_rows(&tx, wallet_id, tables)?;
        tx.commit()?;
        Ok(inserted)
    }
}

/// [`AppDb::import_wallet_rows`] in the caller's transaction.
pub(crate) fn insert_rows(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: &str,
    tables: &[TableRows],
) -> Result<usize> {
    let mut inserted = 0;
    let known = scoped_tables(tx)?;
    for t in tables {
        let Some((_, scope)) = known.iter().find(|(name, _)| *name == t.table) else {
            continue;
        };
        let here: Vec<String> = columns(tx, &t.table)?
            .into_iter()
            .filter(|(_, alias)| !alias)
            .map(|(c, _)| c)
            .collect();
        let keep: Vec<usize> = (0..t.columns.len())
            .filter(|i| here.contains(&t.columns[*i]))
            .collect();
        let Some(scope_pos) = keep.iter().position(|i| t.columns[*i] == *scope) else {
            continue;
        };
        let names = keep
            .iter()
            .map(|i| format!("\"{}\"", t.columns[*i]))
            .collect::<Vec<_>>()
            .join(", ");
        let marks = (1..=keep.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = tx.prepare(&format!(
            "INSERT OR IGNORE INTO \"{}\" ({names}) VALUES ({marks})",
            t.table
        ))?;
        // Tables numbered by a rowid alias have no other key, so an
        // identical row is looked up before inserting it again.
        let same = keep
            .iter()
            .enumerate()
            .map(|(n, i)| format!("\"{}\" IS ?{}", t.columns[*i], n + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut exists = tx.prepare(&format!(
            "SELECT 1 FROM \"{}\" WHERE {same} LIMIT 1",
            t.table
        ))?;
        for row in &t.rows {
            let values: Vec<&SqlValue> = keep
                .iter()
                .map(|i| row.get(*i).unwrap_or(&SqlValue::Null))
                .collect();
            if !matches!(values.get(scope_pos), Some(SqlValue::Text(s)) if s == wallet_id) {
                continue;
            }
            if exists.exists(rusqlite::params_from_iter(values.iter()))? {
                continue;
            }
            inserted += stmt.execute(rusqlite::params_from_iter(values.iter()))?;
        }
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use crate::{AppDb, BookPurpose, LabelKind, SqlValue, TableRows};

    /// A restore's retry deletes a wallet's rows from `WALLET_TABLES`
    /// (#359): none of them refuses a delete, and no table that does is a
    /// wallet's rows.
    #[test]
    fn no_wallet_table_refuses_a_delete() {
        let db = AppDb::open_in_memory().unwrap();
        let conn = db.conn();
        let mut stmt = conn
            .prepare("SELECT tbl_name, sql FROM sqlite_master WHERE type = 'trigger'")
            .unwrap();
        let refusing: Vec<String> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .filter(|(_, sql)| sql.to_uppercase().contains("BEFORE DELETE"))
            .map(|(table, _)| table)
            .collect();
        assert!(
            refusing.contains(&"mn_withdrawals".to_owned()),
            "{refusing:?}"
        );
        let scoped = super::scoped_tables(&conn).unwrap();
        for table in &refusing {
            assert!(!crate::WALLET_TABLES.contains(&table.as_str()), "{table}");
            assert!(scoped.iter().all(|(t, _)| t != table), "{table}");
        }
    }

    #[test]
    fn wallet_rows_round_trip_into_another_database() {
        let a = AppDb::open_in_memory().unwrap();
        let w = "ab".repeat(32);
        let other = "cd".repeat(32);
        a.set_wallet_name(&w, "Savings", 10).unwrap();
        a.set_wallet_name(&other, "Other", 11).unwrap();
        a.upsert_book_entry(&w, "yAddr", BookPurpose::Send, "Alice é", 12)
            .unwrap();
        a.set_label(&w, LabelKind::Tx, &"00".repeat(32), Some("rent"), 13)
            .unwrap();
        a.add_receive_request_with_username(&w, 14, "yRecv", Some(5), Some("l"), None, Some(true))
            .unwrap();
        a.set_setting(&w, "dust", Some("1")).unwrap();
        a.set_setting(&other, "dust", Some("2")).unwrap();
        let rows = a.export_wallet_rows(&w).unwrap();
        assert!(rows.iter().all(|t| !t.rows.is_empty()));
        assert!(rows.iter().any(|t| t.table == "settings_kv"));

        let b = AppDb::open_in_memory().unwrap();
        let n = b.import_wallet_rows(&w, &rows).unwrap();
        assert_eq!(n, rows.iter().map(|t| t.rows.len()).sum::<usize>());
        assert_eq!(
            b.wallets().unwrap(),
            vec![(w.clone(), "Savings".into(), 10)]
        );
        assert_eq!(b.setting(&w, "dust").unwrap().as_deref(), Some("1"));
        assert_eq!(b.setting(&other, "dust").unwrap(), None);
        assert_eq!(b.receive_requests(&w).unwrap().len(), 1);
        assert_eq!(
            b.receive_requests(&w).unwrap()[0].include_username,
            Some(true)
        );
        assert_eq!(
            b.book_entry(&w, "yAddr").unwrap().unwrap().label.as_deref(),
            Some("Alice é")
        );
        // Importing again inserts nothing; rows of another wallet are refused.
        assert_eq!(b.import_wallet_rows(&w, &rows).unwrap(), 0);
        assert_eq!(b.import_wallet_rows(&other, &rows).unwrap(), 0);
    }

    /// Issue 340: a TEXT value that is not valid UTF-8 is exported as its
    /// bytes and imported as the same TEXT bytes, not a panic.
    #[test]
    fn a_text_value_that_is_not_utf8_exports_as_its_bytes_and_imports_byte_identical() {
        let key = b"abandoned:\xff".to_vec();
        let value = b"v\xfe1".to_vec();
        let a = AppDb::open_in_memory().unwrap();
        let w = "ab".repeat(32);
        a.set_wallet_name(&w, "Savings", 10).unwrap();
        a.set_setting(&w, "dust", Some("1")).unwrap();
        a.conn()
            .execute(
                "INSERT INTO settings_kv (scope, key, value) \
                 VALUES (?1, CAST(?2 AS TEXT), CAST(?3 AS TEXT))",
                rusqlite::params![w, key, value],
            )
            .unwrap();
        let rows = a.export_wallet_rows(&w).unwrap();
        let settings = rows.iter().find(|t| t.table == "settings_kv").unwrap();
        assert!(settings.rows.contains(&vec![
            SqlValue::Text(w.clone()),
            SqlValue::NonUtf8Text(key.clone()),
            SqlValue::NonUtf8Text(value.clone()),
        ]));

        let b = AppDb::open_in_memory().unwrap();
        let n = b.import_wallet_rows(&w, &rows).unwrap();
        assert_eq!(n, rows.iter().map(|t| t.rows.len()).sum::<usize>());
        let stored: Vec<(String, Vec<u8>, String, Vec<u8>)> = b
            .conn()
            .prepare(
                "SELECT typeof(key), CAST(key AS BLOB), typeof(value), CAST(value AS BLOB) \
                 FROM settings_kv WHERE scope = ?1 AND key <> 'dust'",
            )
            .unwrap()
            .query_map([&w], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(stored, [("text".into(), key, "text".into(), value)]);
        assert_eq!(b.setting(&w, "dust").unwrap().as_deref(), Some("1"));
        assert_eq!(b.import_wallet_rows(&w, &rows).unwrap(), 0, "already there");
        assert_eq!(b.export_wallet_rows(&w).unwrap(), rows);
    }

    /// `dp_contact_info` stays on its machine, out and in (DP2-04): a
    /// restore records it from its own read of Platform.
    #[test]
    fn contact_info_digests_are_not_carried() {
        let a = AppDb::open_in_memory().unwrap();
        let w = "ab".repeat(32);
        a.set_wallet_name(&w, "Savings", 10).unwrap();
        let set = |digest, at| a.set_contact_info_digest(&w, "id", "contact", &digest, at);
        assert!(set([7; 32], 11).unwrap());
        assert!(
            !set([7; 32], 12).unwrap(),
            "the same digest changes nothing"
        );
        assert!(set([8; 32], 13).unwrap());
        assert_eq!(
            a.contact_info_digest(&w, "id", "contact").unwrap(),
            Some([8; 32])
        );
        let rows = a.export_wallet_rows(&w).unwrap();
        assert!(rows.iter().all(|t| t.table != "dp_contact_info"));
        let forget = || a.forget_contact_info_digest(&w, "id", "contact");
        assert!(forget().unwrap());
        assert!(!forget().unwrap());
        assert_eq!(a.contact_info_digest(&w, "id", "contact").unwrap(), None);

        for contact in ["c1", "c2", "c3"] {
            a.set_contact_info_digest(&w, "id", contact, &[1; 32], 14)
                .unwrap();
        }
        a.set_contact_info_digest(&w, "other", "c1", &[2; 32], 14)
            .unwrap();
        let keep = ["c2".to_string()];
        assert_eq!(
            a.forget_contact_infos_except(&w, "id", &keep).unwrap(),
            ["c1", "c3"]
        );
        assert_eq!(
            a.contact_info_digests(&w).unwrap(),
            [
                ("id".into(), "c2".into(), [1; 32]),
                ("other".into(), "c1".into(), [2; 32])
            ]
        );

        let b = AppDb::open_in_memory().unwrap();
        let crafted = TableRows {
            table: "dp_contact_info".into(),
            columns: ["wallet_id", "identity", "contact", "digest", "recorded_at"]
                .map(String::from)
                .to_vec(),
            rows: vec![vec![
                SqlValue::Text(w.clone()),
                SqlValue::Text("id".into()),
                SqlValue::Text("contact".into()),
                SqlValue::Blob(vec![7; 32]),
                SqlValue::Integer(11),
            ]],
        };
        assert_eq!(b.import_wallet_rows(&w, &[crafted]).unwrap(), 0);
        assert_eq!(b.contact_info_digest(&w, "id", "contact").unwrap(), None);
    }

    /// Which wallets were removed stays with its database (DEC-210): never
    /// exported, never imported.
    #[test]
    fn removed_wallets_are_not_carried() {
        let a = AppDb::open_in_memory().unwrap();
        let w = "ab".repeat(32);
        a.set_wallet_name(&w, "Savings", 10).unwrap();
        a.delete_wallet(&w).unwrap();
        assert!(a.export_wallet_rows(&w).unwrap().is_empty());

        let b = AppDb::open_in_memory().unwrap();
        let crafted = TableRows {
            table: "removed_wallets".into(),
            columns: vec!["wallet_id".into()],
            rows: vec![vec![SqlValue::Text(w.clone())]],
        };
        assert_eq!(b.import_wallet_rows(&w, &[crafted]).unwrap(), 0);
        b.set_wallet_name(&w, "Savings", 10).unwrap();
        assert_eq!(b.wallets().unwrap().len(), 1, "b's writes still land");
    }
}
