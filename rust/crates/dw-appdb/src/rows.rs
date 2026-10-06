//! Export and import of one wallet's rows, for `.dwbackup` bundles
//! (QT-110, QT-116). Generic over the schema: every table with a
//! `wallet_id` column contributes the wallet's rows, and `settings_kv` its
//! rows scoped to the wallet, so tables added by later migrations are
//! carried without changes here.

use rusqlite::types::Value;

use crate::{AppDb, Result};

/// One SQLite value.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<Value> for SqlValue {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => SqlValue::Null,
            Value::Integer(i) => SqlValue::Integer(i),
            Value::Real(r) => SqlValue::Real(r),
            Value::Text(t) => SqlValue::Text(t),
            Value::Blob(b) => SqlValue::Blob(b),
        }
    }
}

impl From<&SqlValue> for Value {
    fn from(v: &SqlValue) -> Self {
        match v {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(i) => Value::Integer(*i),
            SqlValue::Real(r) => Value::Real(*r),
            SqlValue::Text(t) => Value::Text(t.clone()),
            SqlValue::Blob(b) => Value::Blob(b.clone()),
        }
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
                        .map(|i| r.get::<_, Value>(i).map(SqlValue::from))
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
        let mut inserted = 0;
        let known = scoped_tables(&tx)?;
        for t in tables {
            let Some((_, scope)) = known.iter().find(|(name, _)| *name == t.table) else {
                continue;
            };
            let here: Vec<String> = columns(&tx, &t.table)?
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
                let values: Vec<Value> = keep
                    .iter()
                    .map(|i| row.get(*i).map(Value::from).unwrap_or(Value::Null))
                    .collect();
                if values.get(scope_pos) != Some(&Value::Text(wallet_id.to_owned())) {
                    continue;
                }
                if exists.exists(rusqlite::params_from_iter(values.iter()))? {
                    continue;
                }
                inserted += stmt.execute(rusqlite::params_from_iter(values.iter()))?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppDb, BookPurpose, LabelKind};

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
        a.add_receive_request(&w, 14, "yRecv", Some(5), Some("l"), None)
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
            b.book_entry(&w, "yAddr").unwrap().unwrap().label.as_deref(),
            Some("Alice é")
        );
        // Importing again inserts nothing; rows of another wallet are refused.
        assert_eq!(b.import_wallet_rows(&w, &rows).unwrap(), 0);
        assert_eq!(b.import_wallet_rows(&other, &rows).unwrap(), 0);
    }
}
