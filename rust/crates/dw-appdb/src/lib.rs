//! App metadata store: one `app.sqlite` per network directory (DESIGN-opus §1.7).
//!
//! Holds what the wallet database (`wallet.sqlite`, platform-wallet's
//! `SqlitePersister`) does not: wallet names, the dash-qt address book,
//! address and transaction labels, transaction messages, receive requests,
//! user and dust UTXO locks, and engine settings.
//!
//! The schema is created by refinery migrations embedded from `migrations/`.
//! Migrations are append-only and named `V<yyyymmddnn>__<name>.sql`; refinery
//! refuses a database whose applied history diverges from the embedded set
//! and a build with two migrations of the same version.
//!
//! Every method is a short synchronous SQLite call on one connection behind a
//! mutex. Async callers run them off their executor (`spawn_blocking`).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};

mod rows;
pub use rows::{SqlValue, TableRows};

mod embedded {
    refinery::embed_migrations!("./migrations");
}

#[cfg(test)]
mod migration_tests;

/// File name of the metadata database inside a network directory.
pub const APP_DB_FILE: &str = "app.sqlite";

/// Settings scope for network-wide values (`settings_kv.scope`).
pub const GLOBAL_SCOPE: &str = "";

#[derive(Debug, thiserror::Error)]
pub enum AppDbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration: {0}")]
    Migration(#[from] refinery::Error),
    /// A value read from the database is outside its schema domain.
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, AppDbError>;

/// Address-book page an entry belongs to (dash-qt `AddressTableModel` purpose).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BookPurpose {
    Send,
    Receive,
}

impl BookPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Receive => "receive",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "send" => Ok(Self::Send),
            "receive" => Ok(Self::Receive),
            other => Err(AppDbError::Corrupt(format!(
                "address_book.purpose {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookEntry {
    pub address: String,
    /// `None` when the address has no label (dash-qt shows "(no label)").
    pub label: Option<String>,
    pub purpose: BookPurpose,
    pub created_at: u64,
}

/// What a label is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelKind {
    Address,
    Tx,
}

impl LabelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Address => "address",
            Self::Tx => "tx",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveRequestRow {
    pub id: i64,
    pub created_at: u64,
    pub address: String,
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
}

/// Why an outpoint is excluded from automatic coin selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockReason {
    /// "Lock unspent" (QT-070).
    Manual,
    /// Dust attack protection (QT-075).
    Dust,
}

impl LockReason {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "manual" => Ok(Self::Manual),
            "dust" => Ok(Self::Dust),
            other => Err(AppDbError::Corrupt(format!("utxo_locks.reason {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRow {
    pub txid: String,
    pub vout: u32,
    pub reason: LockReason,
    pub created_at: u64,
    /// Set when the user unlocked a dust lock. A released row does not lock
    /// the coin; it only stops dust protection from locking it again.
    pub released_at: Option<u64>,
}

impl LockRow {
    /// Whether the outpoint is currently excluded from coin selection.
    pub fn is_locked(&self) -> bool {
        self.released_at.is_none()
    }
}

pub struct AppDb {
    conn: Mutex<Connection>,
}

impl AppDb {
    /// Opens (creating if needed) the database at `path` and applies pending
    /// migrations.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A private in-memory database (tests, fixture tooling).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        embedded::migrations::runner()
            .set_abort_divergent(true)
            .set_abort_missing(true)
            .run(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the guard cannot leave SQLite half-written
        // (each statement or transaction is atomic), so a poisoned lock is
        // still a usable connection.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Highest applied migration version.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn().query_row(
            "SELECT MAX(version) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )?)
    }

    // ---- wallets ----

    /// Sets the display name of a wallet, creating the row on first use.
    pub fn set_wallet_name(&self, wallet_id: &str, name: &str, now: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO wallets (wallet_id, name, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (wallet_id) DO UPDATE SET name = excluded.name",
            params![wallet_id, name, now as i64],
        )?;
        Ok(())
    }

    /// `(wallet_id, name)` for every named wallet, oldest first.
    pub fn wallet_names(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT wallet_id, name FROM wallets ORDER BY created_at, wallet_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `(wallet_id, name, created_at)` for every named wallet, in the order
    /// the rows were added (oldest first, insertion order within a second).
    pub fn wallets(&self) -> Result<Vec<(String, String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT wallet_id, name, created_at FROM wallets ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deletes every row that belongs to `wallet_id` (wallet removal).
    pub fn delete_wallet(&self, wallet_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for table in [
            "wallets",
            "address_book",
            "labels",
            "tx_meta",
            "receive_requests",
            "utxo_locks",
            "dp_main_identity",
            "dp_registration",
            "dp_contest_watch",
            "dp_events",
            "dp_payment_lock",
            "dp_trust_unverified",
            "dp_prefs",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE wallet_id = ?1"),
                [wallet_id],
            )?;
        }
        tx.execute("DELETE FROM settings_kv WHERE scope = ?1", [wallet_id])?;
        tx.commit()?;
        Ok(())
    }

    // ---- address book ----

    /// Entries of one wallet, optionally of one purpose, ordered by label
    /// (case-insensitive, unlabelled last) then address, as dash-qt sorts.
    pub fn address_book(
        &self,
        wallet_id: &str,
        purpose: Option<BookPurpose>,
    ) -> Result<Vec<BookEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT b.address, l.label, b.purpose, b.created_at
             FROM address_book b
             LEFT JOIN labels l
               ON l.wallet_id = b.wallet_id AND l.kind = 'address' AND l.target = b.address
             WHERE b.wallet_id = ?1 AND (?2 IS NULL OR b.purpose = ?2)
             ORDER BY l.label IS NULL, l.label COLLATE NOCASE, b.address",
        )?;
        let rows = stmt.query_map(params![wallet_id, purpose.map(BookPurpose::as_str)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (address, label, purpose, created_at) = row?;
            Ok(BookEntry {
                address,
                label,
                purpose: BookPurpose::parse(&purpose)?,
                created_at: created_at as u64,
            })
        })
        .collect()
    }

    pub fn book_entry(&self, wallet_id: &str, address: &str) -> Result<Option<BookEntry>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT b.purpose, b.created_at, l.label
                 FROM address_book b
                 LEFT JOIN labels l
                   ON l.wallet_id = b.wallet_id AND l.kind = 'address' AND l.target = b.address
                 WHERE b.wallet_id = ?1 AND b.address = ?2",
                params![wallet_id, address],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(purpose, created_at, label)| {
            Ok(BookEntry {
                address: address.to_string(),
                label,
                purpose: BookPurpose::parse(&purpose)?,
                created_at: created_at as u64,
            })
        })
        .transpose()
    }

    /// Inserts or updates an address-book entry and its label in one
    /// transaction. An existing row keeps its `created_at`. An empty label
    /// clears the label (dash-qt stores "" and shows "(no label)").
    pub fn upsert_book_entry(
        &self,
        wallet_id: &str,
        address: &str,
        purpose: BookPurpose,
        label: &str,
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO address_book (wallet_id, address, purpose, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (wallet_id, address) DO UPDATE SET purpose = excluded.purpose",
            params![wallet_id, address, purpose.as_str(), now as i64],
        )?;
        write_label(
            &tx,
            wallet_id,
            LabelKind::Address,
            address,
            Some(label),
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes an entry and its address label. Returns whether it existed.
    pub fn delete_book_entry(&self, wallet_id: &str, address: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM address_book WHERE wallet_id = ?1 AND address = ?2",
            params![wallet_id, address],
        )?;
        if n > 0 {
            write_label(&tx, wallet_id, LabelKind::Address, address, None, 0)?;
        }
        tx.commit()?;
        Ok(n > 0)
    }

    // ---- labels ----

    /// Sets (`Some`, non-empty) or clears (`None` or empty) a label.
    pub fn set_label(
        &self,
        wallet_id: &str,
        kind: LabelKind,
        target: &str,
        label: Option<&str>,
        now: u64,
    ) -> Result<()> {
        write_label(&self.conn(), wallet_id, kind, target, label, now)
    }

    pub fn label(&self, wallet_id: &str, kind: LabelKind, target: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT label FROM labels WHERE wallet_id = ?1 AND kind = ?2 AND target = ?3",
                params![wallet_id, kind.as_str(), target],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every label of one kind as `(target, label)`.
    pub fn labels(&self, wallet_id: &str, kind: LabelKind) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT target, label FROM labels WHERE wallet_id = ?1 AND kind = ?2")?;
        let rows = stmt.query_map(params![wallet_id, kind.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- transaction metadata ----

    /// Stores the payment message(s) of a sent transaction. `None` clears it.
    pub fn set_tx_message(
        &self,
        wallet_id: &str,
        txid: &str,
        message: Option<&str>,
        now: u64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO tx_meta (wallet_id, txid, message, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (wallet_id, txid) DO UPDATE SET message = excluded.message",
            params![wallet_id, txid, message, now as i64],
        )?;
        Ok(())
    }

    /// Records `now` as the first-seen time of each txid that has no
    /// `tx_meta` row yet. Existing rows (and their messages) are untouched.
    pub fn note_txs_seen(&self, wallet_id: &str, txids: &[String], now: u64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO tx_meta (wallet_id, txid, message, created_at) VALUES (?1, ?2, NULL, ?3)
                 ON CONFLICT (wallet_id, txid) DO NOTHING",
            )?;
            for txid in txids {
                stmt.execute(params![wallet_id, txid, now as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// `(txid, created_at)` of every `tx_meta` row of a wallet: when this
    /// device first saw (or sent) the transaction.
    pub fn tx_seen_times(&self, wallet_id: &str) -> Result<Vec<(String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT txid, created_at FROM tx_meta WHERE wallet_id = ?1")?;
        let rows = stmt.query_map([wallet_id], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn tx_message(&self, wallet_id: &str, txid: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT message FROM tx_meta WHERE wallet_id = ?1 AND txid = ?2",
                params![wallet_id, txid],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    // ---- receive requests ----

    /// Stores a request and returns its id.
    pub fn add_receive_request(
        &self,
        wallet_id: &str,
        created_at: u64,
        address: &str,
        amount: Option<u64>,
        label: Option<&str>,
        message: Option<&str>,
    ) -> Result<i64> {
        let amount = amount
            .map(|a| i64::try_from(a).map_err(|_| AppDbError::Corrupt(format!("amount {a}"))))
            .transpose()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO receive_requests (wallet_id, created_at, address, amount, label, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                wallet_id,
                created_at as i64,
                address,
                amount,
                label,
                message
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Requests of one wallet, newest first.
    pub fn receive_requests(&self, wallet_id: &str) -> Result<Vec<ReceiveRequestRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, created_at, address, amount, label, message FROM receive_requests
             WHERE wallet_id = ?1 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            Ok(ReceiveRequestRow {
                id: r.get(0)?,
                created_at: r.get::<_, i64>(1)? as u64,
                address: r.get(2)?,
                amount: r.get::<_, Option<i64>>(3)?.map(|a| a as u64),
                label: r.get(4)?,
                message: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns whether the request existed.
    pub fn delete_receive_request(&self, wallet_id: &str, id: i64) -> Result<bool> {
        Ok(self.conn().execute(
            "DELETE FROM receive_requests WHERE wallet_id = ?1 AND id = ?2",
            params![wallet_id, id],
        )? > 0)
    }

    // ---- UTXO locks ----

    /// Locks an outpoint by the user. Replaces a dust row (released or not).
    pub fn lock_manual(&self, wallet_id: &str, txid: &str, vout: u32, now: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO utxo_locks (wallet_id, txid, vout, reason, created_at, released_at)
             VALUES (?1, ?2, ?3, 'manual', ?4, NULL)
             ON CONFLICT (wallet_id, txid, vout)
             DO UPDATE SET reason = 'manual', created_at = excluded.created_at, released_at = NULL",
            params![wallet_id, txid, vout, now as i64],
        )?;
        Ok(())
    }

    /// Dust-locks an outpoint unless a row already exists for it: a manual
    /// lock stays manual and a released dust lock stays released. Returns
    /// whether a new lock was written.
    pub fn lock_dust(&self, wallet_id: &str, txid: &str, vout: u32, now: u64) -> Result<bool> {
        Ok(self.conn().execute(
            "INSERT OR IGNORE INTO utxo_locks (wallet_id, txid, vout, reason, created_at)
             VALUES (?1, ?2, ?3, 'dust', ?4)",
            params![wallet_id, txid, vout, now as i64],
        )? > 0)
    }

    /// Unlocks an outpoint: deletes a manual lock, marks a dust lock released.
    /// Returns whether a lock was active.
    pub fn unlock(&self, wallet_id: &str, txid: &str, vout: u32, now: u64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let deleted = tx.execute(
            "DELETE FROM utxo_locks
             WHERE wallet_id = ?1 AND txid = ?2 AND vout = ?3 AND reason = 'manual'",
            params![wallet_id, txid, vout],
        )?;
        let released = tx.execute(
            "UPDATE utxo_locks SET released_at = ?4
             WHERE wallet_id = ?1 AND txid = ?2 AND vout = ?3 AND reason = 'dust'
               AND released_at IS NULL",
            params![wallet_id, txid, vout, now as i64],
        )?;
        tx.commit()?;
        Ok(deleted + released > 0)
    }

    /// Every lock row of a wallet, released dust rows included.
    pub fn locks(&self, wallet_id: &str) -> Result<Vec<LockRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT txid, vout, reason, created_at, released_at FROM utxo_locks
             WHERE wallet_id = ?1 ORDER BY created_at, txid, vout",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (txid, vout, reason, created_at, released_at) = row?;
            Ok(LockRow {
                txid,
                vout,
                reason: LockReason::parse(&reason)?,
                created_at: created_at as u64,
                released_at: released_at.map(|t| t as u64),
            })
        })
        .collect()
    }

    /// Drops lock rows whose outpoint is no longer unspent (`keep` returns
    /// `false`). Returns how many rows were removed.
    pub fn prune_locks(&self, wallet_id: &str, keep: impl Fn(&str, u32) -> bool) -> Result<usize> {
        let stale: Vec<(String, u32)> = self
            .locks(wallet_id)?
            .into_iter()
            .filter(|row| !keep(&row.txid, row.vout))
            .map(|row| (row.txid, row.vout))
            .collect();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (txid, vout) in &stale {
            tx.execute(
                "DELETE FROM utxo_locks WHERE wallet_id = ?1 AND txid = ?2 AND vout = ?3",
                params![wallet_id, txid, vout],
            )?;
        }
        tx.commit()?;
        Ok(stale.len())
    }

    // ---- settings ----

    pub fn setting(&self, scope: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
                params![scope, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Sets a value, or deletes the key for `None`.
    pub fn set_setting(&self, scope: &str, key: &str, value: Option<&str>) -> Result<()> {
        let conn = self.conn();
        match value {
            Some(v) => conn.execute(
                "INSERT INTO settings_kv (scope, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value",
                params![scope, key, v],
            )?,
            None => conn.execute(
                "DELETE FROM settings_kv WHERE scope = ?1 AND key = ?2",
                params![scope, key],
            )?,
        };
        Ok(())
    }

    /// Every `(key, value)` of `scope` whose key starts with `prefix`, by
    /// key.
    pub fn settings_with_prefix(&self, scope: &str, prefix: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, value FROM settings_kv WHERE scope = ?1 AND substr(key, 1, ?3) = ?2
             ORDER BY key",
        )?;
        let rows = stmt.query_map(params![scope, prefix, prefix.chars().count() as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn write_label(
    conn: &Connection,
    wallet_id: &str,
    kind: LabelKind,
    target: &str,
    label: Option<&str>,
    now: u64,
) -> Result<()> {
    match label.filter(|l| !l.is_empty()) {
        Some(label) => conn.execute(
            "INSERT INTO labels (wallet_id, kind, target, label, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (wallet_id, kind, target)
             DO UPDATE SET label = excluded.label, updated_at = excluded.updated_at",
            params![wallet_id, kind.as_str(), target, label, now as i64],
        )?,
        None => conn.execute(
            "DELETE FROM labels WHERE wallet_id = ?1 AND kind = ?2 AND target = ?3",
            params![wallet_id, kind.as_str(), target],
        )?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";
    const W2: &str = "bb";

    #[test]
    fn migrations_apply_once_and_reopen_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DB_FILE);
        let db = AppDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), 2026100801);
        db.set_wallet_name(W, "Main", 1).unwrap();
        drop(db);
        let db = AppDb::open(&path).unwrap();
        assert_eq!(db.wallet_names().unwrap(), vec![(W.into(), "Main".into())]);
    }

    #[test]
    fn wallet_names_update_in_place_and_keep_creation_order() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_wallet_name(W2, "Second", 2).unwrap();
        db.set_wallet_name(W, "First", 1).unwrap();
        db.set_wallet_name(W2, "Renamed", 9).unwrap();
        assert_eq!(
            db.wallet_names().unwrap(),
            vec![(W.into(), "First".into()), (W2.into(), "Renamed".into())]
        );
    }

    #[test]
    fn address_book_round_trip_sorted_by_label() {
        let db = AppDb::open_in_memory().unwrap();
        db.upsert_book_entry(W, "addr-b", BookPurpose::Send, "bob", 1)
            .unwrap();
        db.upsert_book_entry(W, "addr-a", BookPurpose::Send, "Alice", 2)
            .unwrap();
        db.upsert_book_entry(W, "addr-c", BookPurpose::Receive, "", 3)
            .unwrap();
        db.upsert_book_entry(W2, "addr-x", BookPurpose::Send, "other wallet", 3)
            .unwrap();

        let all = db.address_book(W, None).unwrap();
        let order: Vec<_> = all.iter().map(|e| e.address.as_str()).collect();
        assert_eq!(order, ["addr-a", "addr-b", "addr-c"]);
        assert_eq!(all[2].label, None);

        let send = db.address_book(W, Some(BookPurpose::Send)).unwrap();
        assert_eq!(send.len(), 2);

        // Relabel keeps created_at.
        db.upsert_book_entry(W, "addr-b", BookPurpose::Send, "Bob", 50)
            .unwrap();
        let bob = db.book_entry(W, "addr-b").unwrap().unwrap();
        assert_eq!(bob.label.as_deref(), Some("Bob"));
        assert_eq!(bob.created_at, 1);

        assert!(db.delete_book_entry(W, "addr-b").unwrap());
        assert!(!db.delete_book_entry(W, "addr-b").unwrap());
        assert_eq!(db.book_entry(W, "addr-b").unwrap(), None);
        assert_eq!(db.label(W, LabelKind::Address, "addr-b").unwrap(), None);
    }

    #[test]
    fn tx_labels_and_messages() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_label(W, LabelKind::Tx, "t1", Some("rent"), 1)
            .unwrap();
        assert_eq!(
            db.label(W, LabelKind::Tx, "t1").unwrap().as_deref(),
            Some("rent")
        );
        assert_eq!(db.label(W, LabelKind::Address, "t1").unwrap(), None);
        db.set_label(W, LabelKind::Tx, "t1", None, 2).unwrap();
        assert_eq!(db.label(W, LabelKind::Tx, "t1").unwrap(), None);

        db.set_tx_message(W, "t1", Some("order 42"), 1).unwrap();
        assert_eq!(db.tx_message(W, "t1").unwrap().as_deref(), Some("order 42"));
        assert_eq!(db.tx_message(W2, "t1").unwrap(), None);
    }

    #[test]
    fn first_seen_times_keep_the_first_and_the_message() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_tx_message(W, "t1", Some("order 42"), 5).unwrap();
        db.note_txs_seen(W, &["t1".into(), "t2".into()], 9).unwrap();
        db.note_txs_seen(W, &["t2".into()], 20).unwrap();
        let mut seen = db.tx_seen_times(W).unwrap();
        seen.sort();
        assert_eq!(seen, vec![("t1".into(), 5), ("t2".into(), 9)]);
        assert_eq!(db.tx_message(W, "t1").unwrap().as_deref(), Some("order 42"));
        assert!(db.tx_seen_times(W2).unwrap().is_empty());

        db.set_wallet_name(W, "Savings", 3).unwrap();
        assert_eq!(
            db.wallets().unwrap(),
            vec![(W.to_string(), "Savings".to_string(), 3)]
        );
    }

    #[test]
    fn receive_requests_newest_first() {
        let db = AppDb::open_in_memory().unwrap();
        let a = db
            .add_receive_request(W, 10, "a1", Some(5), Some("l"), None)
            .unwrap();
        let b = db
            .add_receive_request(W, 20, "a2", None, None, Some("m"))
            .unwrap();
        let rows = db.receive_requests(W).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [b, a]);
        assert_eq!(rows[1].amount, Some(5));
        assert!(db.delete_receive_request(W, a).unwrap());
        assert!(!db.delete_receive_request(W2, b).unwrap());
        assert_eq!(db.receive_requests(W).unwrap().len(), 1);
    }

    #[test]
    fn zero_amount_request_is_refused_by_the_schema() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(
            db.add_receive_request(W, 1, "a", Some(0), None, None)
                .is_err()
        );
    }

    #[test]
    fn manual_and_dust_lock_lifecycle() {
        let db = AppDb::open_in_memory().unwrap();
        db.lock_manual(W, "t", 0, 1).unwrap();
        // A dust pass never downgrades a manual lock.
        assert!(!db.lock_dust(W, "t", 0, 2).unwrap());
        assert_eq!(db.locks(W).unwrap()[0].reason, LockReason::Manual);
        assert!(db.unlock(W, "t", 0, 3).unwrap());
        assert!(db.locks(W).unwrap().is_empty());

        // Dust lock, released by the user, is not re-locked by later passes.
        assert!(db.lock_dust(W, "d", 1, 4).unwrap());
        assert!(db.locks(W).unwrap()[0].is_locked());
        assert!(db.unlock(W, "d", 1, 5).unwrap());
        assert!(!db.unlock(W, "d", 1, 6).unwrap());
        assert!(!db.lock_dust(W, "d", 1, 7).unwrap());
        let row = &db.locks(W).unwrap()[0];
        assert_eq!(row.reason, LockReason::Dust);
        assert_eq!(row.released_at, Some(5));
        assert!(!row.is_locked());

        // The user can still lock it by hand.
        db.lock_manual(W, "d", 1, 8).unwrap();
        let row = &db.locks(W).unwrap()[0];
        assert_eq!(row.reason, LockReason::Manual);
        assert!(row.is_locked());
    }

    #[test]
    fn prune_drops_rows_of_spent_outpoints() {
        let db = AppDb::open_in_memory().unwrap();
        db.lock_manual(W, "keep", 0, 1).unwrap();
        db.lock_dust(W, "gone", 0, 1).unwrap();
        assert_eq!(db.prune_locks(W, |txid, _| txid == "keep").unwrap(), 1);
        assert_eq!(db.locks(W).unwrap().len(), 1);
    }

    #[test]
    fn settings_set_get_delete() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(db.setting(GLOBAL_SCOPE, "k").unwrap(), None);
        db.set_setting(GLOBAL_SCOPE, "k", Some("1")).unwrap();
        db.set_setting(GLOBAL_SCOPE, "k", Some("2")).unwrap();
        db.set_setting(W, "k", Some("w")).unwrap();
        assert_eq!(db.setting(GLOBAL_SCOPE, "k").unwrap().as_deref(), Some("2"));
        assert_eq!(db.setting(W, "k").unwrap().as_deref(), Some("w"));
        db.set_setting(GLOBAL_SCOPE, "k", None).unwrap();
        assert_eq!(db.setting(GLOBAL_SCOPE, "k").unwrap(), None);
    }

    #[test]
    fn settings_with_prefix_lists_one_scope() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_setting(W, "abandoned:b", Some("2")).unwrap();
        db.set_setting(W, "abandoned:a", Some("1")).unwrap();
        db.set_setting(W, "other", Some("x")).unwrap();
        db.set_setting(W2, "abandoned:c", Some("3")).unwrap();
        assert_eq!(
            db.settings_with_prefix(W, "abandoned:").unwrap(),
            vec![
                ("abandoned:a".to_string(), "1".to_string()),
                ("abandoned:b".to_string(), "2".to_string())
            ]
        );
        assert!(db.settings_with_prefix(W, "none").unwrap().is_empty());
    }

    #[test]
    fn delete_wallet_removes_only_that_wallets_rows() {
        let db = AppDb::open_in_memory().unwrap();
        for w in [W, W2] {
            db.set_wallet_name(w, "n", 1).unwrap();
            db.upsert_book_entry(w, "a", BookPurpose::Send, "l", 1)
                .unwrap();
            db.set_label(w, LabelKind::Tx, "t", Some("x"), 1).unwrap();
            db.set_tx_message(w, "t", Some("m"), 1).unwrap();
            db.add_receive_request(w, 1, "a", None, None, None).unwrap();
            db.lock_manual(w, "t", 0, 1).unwrap();
            db.set_setting(w, "k", Some("v")).unwrap();
        }
        db.delete_wallet(W).unwrap();
        assert!(db.address_book(W, None).unwrap().is_empty());
        assert!(db.labels(W, LabelKind::Tx).unwrap().is_empty());
        assert_eq!(db.tx_message(W, "t").unwrap(), None);
        assert!(db.receive_requests(W).unwrap().is_empty());
        assert!(db.locks(W).unwrap().is_empty());
        assert_eq!(db.setting(W, "k").unwrap(), None);
        assert_eq!(db.wallet_names().unwrap(), vec![(W2.into(), "n".into())]);
        assert_eq!(db.address_book(W2, None).unwrap().len(), 1);
        assert_eq!(db.locks(W2).unwrap().len(), 1);
    }
}
