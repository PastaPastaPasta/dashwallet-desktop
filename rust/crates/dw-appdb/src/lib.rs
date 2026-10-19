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

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

mod avatar;
mod dashpay_journal;
pub mod dispatch;
#[cfg(all(debug_assertions, any(test, feature = "durability-sim")))]
pub mod durability_sim;
mod provenance;
mod registration;
mod rows;
pub use avatar::AvatarRow;
pub use dashpay_journal::{ContestWatch, JournalEntry, JournalPage, JournalRow};
pub use registration::{
    Adoption, LockOutpoint, NewRegistration, RegistrationChange, RegistrationRow,
    RegistrationUpdate,
};
pub use rows::{SqlValue, TableRows};

mod embedded {
    refinery::embed_migrations!("./migrations");
}

#[cfg(test)]
mod migration_tests;

/// File name of the metadata database inside a network directory.
pub const APP_DB_FILE: &str = "app.sqlite";

/// File name of its lock file ([`AppDb::open`]), next to it.
pub const APP_DB_LOCK_FILE: &str = "app.sqlite.lock";

/// Settings scope for network-wide values (`settings_kv.scope`).
pub const GLOBAL_SCOPE: &str = "";

/// Settings scope for a wallet's values that describe this installation
/// rather than the wallet (for example "created here"): deleted with the
/// wallet, but not exported to a `.dwbackup`, which carries only the scope
/// equal to the wallet id.
pub fn local_scope(wallet_id: &str) -> String {
    format!("local:{wallet_id}")
}

/// The lock file next to the database at `path` (`<file>.lock`), never the
/// database itself, whose bytes SQLite locks by range.
fn lock_path(path: &Path) -> PathBuf {
    let mut lock = OsString::from(path.as_os_str());
    lock.push(".lock");
    lock.into()
}

/// Opens the lock file without following a final symlink: a second name
/// would be a second lock (DEC-210 (3b)).
fn open_lock_file(path: &Path) -> Result<File> {
    dw_fs::open_sole_file(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidInput => refused(path, &e.to_string()),
        _ => file_error(path, e),
    })
}

fn file_error(path: &Path, source: std::io::Error) -> AppDbError {
    AppDbError::File {
        path: path.to_owned(),
        source,
    }
}

fn refused(path: &Path, what: &str) -> AppDbError {
    file_error(
        path,
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{what}; the app database and its lock file must each be a regular file with one name"
            ),
        ),
    )
}

/// Which file a database or lock file is: (device, inode). `None` where the
/// platform has no stable id in std (Windows: the gap DEC-210 (3b) accepts
/// there, with symlinks still refused).
type FileId = Option<(u64, u64)>;

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_: &std::fs::Metadata) -> FileId {
    None
}

fn lock_id(path: &Path, lock: &File) -> Result<FileId> {
    Ok(file_id(&lock.metadata().map_err(|e| file_error(path, e))?))
}

/// What is at the database's path, not following a final symlink: nothing
/// yet (SQLite creates it), or the one name of a regular file and its id.
fn sole_database(path: &Path) -> Result<Option<FileId>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => match dw_fs::not_sole(&meta) {
            Some(why) => Err(refused(path, &why)),
            None => Ok(Some(file_id(&meta))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(file_error(path, e)),
    }
}

/// `path` with its directory resolved: SQLite's `SQLITE_OPEN_NOFOLLOW`
/// refuses a symlink anywhere in the name, and a symlinked directory leads
/// to the same lock file anyway. The file name is kept, so a final symlink
/// is still refused.
#[cfg(unix)]
fn resolve_directory(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        file_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "names no file"),
        )
    })?;
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = dir.canonicalize().map_err(|e| file_error(dir, e))?;
    Ok(dir.join(name))
}

/// Windows: the name is kept.
#[cfg(not(unix))]
fn resolve_directory(path: &Path) -> Result<PathBuf> {
    Ok(path.to_owned())
}

/// SQLite reads a name starting with `file:` as a URI (the bundled build
/// sets `SQLITE_USE_URI`, whatever the open flags say), which can name the
/// database another way; such a path is refused.
fn refuse_uri(path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"file:"))
    {
        return Err(refused(path, "reads as a SQLite URI"));
    }
    Ok(())
}

/// The lock file is not the database itself: one file with one name at
/// each path is still possible under two mounts of it.
fn refuse_same_file(lock_path: &Path, lock: FileId, database: FileId) -> Result<()> {
    if database.is_some() && lock == database {
        return Err(refused(lock_path, "is the database itself"));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Tests: runs once between the database's check and SQLite's open.
    static BEFORE_SQLITE_OPEN: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
    /// Tests: runs once between the lock file's two opens.
    static BEFORE_SHARED_LOCK: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
}

/// Tests: held shared by an open's exclusive probe of the lock file, and
/// exclusively by a test from starting a child process until the child runs.
/// A child being started holds a copy of every descriptor of this process,
/// close-on-exec ones included, until its exec closes them (glibc's
/// `posix_spawn` child; XNU's copies the table the same way). The shared
/// lock of an instance dropped meanwhile lives on in that copy, so a probe
/// in the gap finds the file busy and keeps the removed wallets recorded:
/// safe, but a test expecting them forgotten failed (macOS CI 38570533549,
/// `a_removal_is_kept_while_another_instance_is_open`; 7 of 144 loaded runs
/// of `migration_tests` on Linux).
#[cfg(test)]
pub(crate) static CHILD_STARTS: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// The tables keyed by `wallet_id` that [`AppDb::delete_wallet`] clears, each
/// with an insert trigger that skips rows of a removed wallet (migration
/// `V2026101302`, DEC-210). Settings are cleared by scope.
pub(crate) const WALLET_TABLES: [&str; 14] = [
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
    "dp_contact_info",
];

#[derive(Debug, thiserror::Error)]
pub enum AppDbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration: {0}")]
    Migration(#[from] refinery::Error),
    /// The database or its lock file could not be opened or locked, or was
    /// refused: another name for the file (a symlink, a hard link, a SQLite
    /// URI) or not a regular file.
    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A value read from the database is outside its schema domain.
    #[error("corrupt row: {0}")]
    Corrupt(String),
    /// The wallet was removed (and not readmitted): the row was not written.
    #[error("wallet {0} was removed")]
    WalletRemoved(String),
    /// The wallet's rows, left by a removal that failed part-way, could not
    /// be deleted: it stays removed and is not readmitted.
    #[error("the removal of wallet {wallet} could not be finished: {source}")]
    RemovalUnfinished {
        wallet: String,
        source: rusqlite::Error,
    },
}

pub type Result<T> = std::result::Result<T, AppDbError>;

/// [`AppDb::unfinished`]; a poisoned lock is still a usable set (each
/// change is one insert or removal).
fn unfinished(set: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    set.lock().unwrap_or_else(|p| p.into_inner())
}

/// Deletes every row of `wallet_id`: the tables [`WALLET_TABLES`] and its
/// two settings scopes.
fn delete_wallet_rows(tx: &rusqlite::Transaction<'_>, wallet_id: &str) -> rusqlite::Result<()> {
    for table in WALLET_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE wallet_id = ?1"),
            [wallet_id],
        )?;
    }
    tx.execute(
        "DELETE FROM settings_kv WHERE scope IN (?1, ?2)",
        params![wallet_id, local_scope(wallet_id)],
    )?;
    Ok(())
}

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

/// A `dp_payment_lock` row (DP3-01): payments from `identity` to `contact`
/// wait for `txid`'s outcome, locked at `since`; `inputs` are its
/// comma-separated `txid:vout` outpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpPaymentLockRow {
    pub identity: String,
    pub contact: String,
    pub txid: String,
    pub since: u64,
    pub inputs: String,
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
    /// A shared lock on the database's lock file for as long as this
    /// instance lives, a job still holding it after its session closed
    /// included: while it is held, no open forgets the removed wallets
    /// (DEC-210). With it, the id of the database file the lock is for.
    /// `None` in memory.
    _alive: Option<(File, FileId)>,
    /// Wallets whose [`Self::delete_wallet`] failed in this instance, even
    /// before it could record them as removed: their readmission deletes
    /// their rows first. Used with `conn` held.
    unfinished: Mutex<HashSet<String>>,
}

impl AppDb {
    /// Opens (creating if needed) the database at `path` and applies pending
    /// migrations. When no other instance of the file is open, in this
    /// process or another, it forgets the removed wallets: no write of
    /// theirs can still be running (DEC-210). Rows a removal that failed
    /// part-way left are deleted first.
    ///
    /// Needs a file system whose advisory locks work per open file (local
    /// disks, as SQLite's WAL mode does): without lock support the open
    /// fails, and an NFS emulation of `flock` does not keep the removed
    /// wallets of instances in the same process.
    ///
    /// The lock file is named after `path`, so the database must have no
    /// other name that leads to another lock file: a symlink, a second hard
    /// link or a `file:` URI is refused (DEC-210 (3b)). Symlinked
    /// directories above it are fine; they lead to the same lock file.
    pub fn open(path: &Path) -> Result<Self> {
        refuse_uri(path)?;
        let path = &resolve_directory(path)?;
        let before = sole_database(path)?;
        let lock_path = lock_path(path);
        // Exclusive, on a handle of its own and before this instance's
        // shared lock: granted only while no instance holds one. Never
        // waited for; without it the removed wallets stay recorded.
        let sole = open_lock_file(&lock_path)?;
        let lock = lock_id(&lock_path, &sole)?;
        #[cfg(test)]
        let starts = CHILD_STARTS
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let alone = match sole.try_lock() {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(error = %e, "app database open elsewhere: the removed wallets stay recorded");
                false
            }
        };
        #[cfg(test)]
        drop(starts);
        #[cfg(test)]
        if let Some(hook) = BEFORE_SQLITE_OPEN.take() {
            hook();
        }
        // No URI and no symlink. The database file itself is never opened
        // here: closing a descriptor of it would drop SQLite's POSIX locks.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        // SQLite opened the file checked, or created it just now.
        let database = match sole_database(path)? {
            Some(id) if before.is_none_or(|before| before == id) => id,
            _ => return Err(refused(path, "changed while it was opened")),
        };
        refuse_same_file(&lock_path, lock, database)?;
        let mut db = Self::init(conn)?;
        if alone {
            db.forget_removals();
        }
        // Released before the shared lock is taken, on a handle of its own:
        // a shared lock of a second handle would wait for this one (on
        // Windows, and with `flock` the conversion is not atomic either).
        // An open in the gap may forget the removals again, which is safe:
        // this instance has written nothing yet. The handle stays open
        // until the second one is compared: a file still open keeps its
        // inode, so a replacement cannot reuse it (ext4 hands a freed inode
        // number straight back) and pass for the file checked.
        // (Windows refuses to unlock a handle that holds no lock.)
        if alone {
            sole.unlock().map_err(|e| file_error(&lock_path, e))?;
        }
        #[cfg(test)]
        if let Some(hook) = BEFORE_SHARED_LOCK.take() {
            hook();
        }
        let alive = open_lock_file(&lock_path)?;
        if lock_id(&lock_path, &alive)? != lock {
            return Err(refused(&lock_path, "changed while it was opened"));
        }
        drop(sole);
        alive.lock_shared().map_err(|e| file_error(&lock_path, e))?;
        // Deleting or replacing the lock file from outside after this
        // returns leaves the lock on a file later opens no longer see (an
        // accepted residual, m4 item 42); nothing here removes it.
        db._alive = Some((alive, database));
        Ok(db)
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
            _alive: None,
            unfinished: Mutex::default(),
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

    /// Deletes every row that belongs to `wallet_id` (wallet removal) and
    /// records the wallet as removed: until [`Self::readmit_wallet`], or an
    /// open with no other instance of the file alive ([`Self::open`]),
    /// inserts of its rows are skipped (DEC-210), so a write still under way
    /// when the wallet was removed leaves nothing behind.
    ///
    /// The record is committed first, on its own (the migration's "in the
    /// transaction that deletes its rows" predates this): if deleting the
    /// rows then fails, the wallet stays recorded as removed, and its
    /// readmission or the next such open deletes them before anything else.
    /// Whatever failed, this instance remembers the wallet, so its
    /// readmission deletes the rows even if the record could not be written.
    pub fn delete_wallet(&self, wallet_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let delete = |conn: &mut Connection| -> rusqlite::Result<()> {
            conn.execute(
                "INSERT OR IGNORE INTO removed_wallets (wallet_id) VALUES (?1)",
                [wallet_id],
            )?;
            let tx = conn.transaction()?;
            delete_wallet_rows(&tx, wallet_id)?;
            tx.commit()
        };
        let deleted = delete(&mut conn);
        let mut failed = unfinished(&self.unfinished);
        match deleted {
            Ok(()) => failed.remove(wallet_id),
            Err(_) => failed.insert(wallet_id.to_owned()),
        };
        Ok(deleted?)
    }

    /// Forgets every wallet recorded as removed (an open with no other
    /// instance alive: no write of it can be under way), deleting first the
    /// rows a removal that failed part-way left. If they cannot be deleted
    /// the records stay, so the wallets' readmission deletes them.
    fn forget_removals(&self) {
        let mut conn = self.conn();
        let forget = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            let removed = tx
                .prepare("SELECT wallet_id FROM removed_wallets")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for wallet_id in &removed {
                delete_wallet_rows(&tx, wallet_id)?;
            }
            tx.execute("DELETE FROM removed_wallets", [])?;
            tx.commit()
        };
        if let Err(e) = forget(&mut conn) {
            tracing::warn!(error = %e, "kept the removed wallets' records: their rows could not be deleted");
        }
    }

    /// Lets rows of `wallet_id` be written again after
    /// [`Self::delete_wallet`]: an import of the wallet calls it before its
    /// first write. Whether the wallet was removed (recorded, or its
    /// removal failed in this instance).
    ///
    /// Rows a removal that failed part-way left are deleted in the same
    /// transaction; if they cannot be, the wallet stays removed and the call
    /// fails ([`AppDbError::RemovalUnfinished`]).
    pub fn readmit_wallet(&self, wallet_id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM removed_wallets WHERE wallet_id = ?1",
            [wallet_id],
        )? > 0;
        if !(removed || unfinished(&self.unfinished).contains(wallet_id)) {
            tx.commit()?;
            return Ok(false);
        }
        (delete_wallet_rows(&tx, wallet_id).and_then(|()| tx.commit())).map_err(|source| {
            AppDbError::RemovalUnfinished {
                wallet: wallet_id.to_owned(),
                source,
            }
        })?;
        unfinished(&self.unfinished).remove(wallet_id);
        Ok(true)
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

    /// Stores a request and returns its id (`WalletRemoved` once the wallet
    /// was removed, DEC-210).
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
        if conn.changes() == 0 {
            return Err(AppDbError::WalletRemoved(wallet_id.to_owned()));
        }
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

    // ---- governance registrations (MG-02 D80) ----

    /// Every stored `(proTxHash, ProRegTx bytes)`, unchecked: the caller
    /// verifies each and deletes the ones that fail.
    pub fn governance_registrations(&self) -> Result<Vec<([u8; 32], Vec<u8>)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT pro_tx_hash, registration FROM governance_registrations")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (key, bytes) = row?;
            // The CHECK keeps every key 32 bytes long.
            if let Ok(key) = <[u8; 32]>::try_from(key.as_slice()) {
                out.push((key, bytes));
            }
        }
        Ok(out)
    }

    /// Stores (or replaces) registrations and deletes `remove`, in one
    /// transaction.
    pub fn update_governance_registrations(
        &self,
        put: &[([u8; 32], Vec<u8>)],
        remove: &[[u8; 32]],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (key, bytes) in put {
            tx.execute(
                "INSERT OR REPLACE INTO governance_registrations (pro_tx_hash, registration)
                 VALUES (?1, ?2)",
                params![key.as_slice(), bytes],
            )?;
        }
        for key in remove {
            tx.execute(
                "DELETE FROM governance_registrations WHERE pro_tx_hash = ?1",
                params![key.as_slice()],
            )?;
        }
        tx.commit()?;
        Ok(())
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
        Self::put_setting(&self.conn(), scope, key, value)
    }

    /// [`Self::set_setting`], durable against power loss when it returns
    /// `Ok` (as [`Self::set_dp_payment_lock_durable`]): for a setting a
    /// later write elsewhere relies on, such as an id that must be gone
    /// before another store holds a different key (MG-03 D34).
    pub fn set_setting_durable(&self, scope: &str, key: &str, value: Option<&str>) -> Result<()> {
        self.durably(|conn| Self::put_setting(conn, scope, key, value))
    }

    fn put_setting(conn: &Connection, scope: &str, key: &str, value: Option<&str>) -> Result<()> {
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

    /// Raises a numeric setting to `value` in one statement and returns what
    /// is stored afterwards: the stored value when it is a number at least
    /// `value`, or when it is not a number at all (left for the caller to
    /// report), else `value`. With `keep_existing`, any stored value stays
    /// (the first write wins). Concurrent callers cannot lower it. `Ok` only
    /// once the write has committed: the upsert runs in an explicit
    /// transaction, because a lone `RETURNING` statement commits when it is
    /// reset, where `query_row` would drop a commit failure.
    pub fn raise_setting(
        &self,
        scope: &str,
        key: &str,
        value: u64,
        keep_existing: bool,
    ) -> Result<String> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let stored = tx.query_row(
            "INSERT INTO settings_kv (scope, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (scope, key) DO UPDATE SET value = CASE
                 WHEN ?4 OR value = '' OR value GLOB '*[^0-9]*'
                     OR CAST(value AS INTEGER) >= ?5 THEN value
                 ELSE excluded.value
             END
             RETURNING value",
            params![scope, key, value.to_string(), keep_existing, value as i64],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(stored)
    }

    /// Makes every committed write durable against power loss: a full WAL
    /// checkpoint, which syncs the WAL, copies it into the database file and
    /// syncs that (the connection runs `synchronous=NORMAL`, under which a
    /// commit alone is not synced). `Err` (`SQLITE_BUSY`) when another
    /// connection kept the checkpoint from completing.
    pub fn checkpoint_full(&self) -> Result<()> {
        let busy: i64 = self
            .conn()
            .query_row("PRAGMA wal_checkpoint(FULL)", [], |r| r.get(0))?;
        if busy != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("the WAL checkpoint did not complete".into()),
            )
            .into());
        }
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

    // ---- DashPay identity choices ----

    /// The wallet's chosen main identity (DASHPAY §3.4 `dp_main_identity`).
    pub fn main_identity(&self, wallet_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT identity FROM dp_main_identity WHERE wallet_id = ?1",
                [wallet_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_main_identity(&self, wallet_id: &str, identity: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO dp_main_identity (wallet_id, identity) VALUES (?1, ?2)
             ON CONFLICT (wallet_id) DO UPDATE SET identity = excluded.identity",
            params![wallet_id, identity],
        )?;
        Ok(())
    }

    // ---- DashPay payment locks (DP3-01) ----

    /// Every lock of the wallet, by identity and contact.
    pub fn dp_payment_locks(&self, wallet_id: &str) -> Result<Vec<DpPaymentLockRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT identity, contact, txid, since, inputs FROM dp_payment_lock
             WHERE wallet_id = ?1 ORDER BY identity, contact",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            Ok(DpPaymentLockRow {
                identity: r.get(0)?,
                contact: r.get(1)?,
                txid: r.get(2)?,
                since: r.get::<_, i64>(3)? as u64,
                inputs: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Locks payments from `row.identity` to `row.contact` behind
    /// `row.txid`, unless another payment holds the pair: then nothing is
    /// written and that payment's txid is returned. Writing `row.txid`'s
    /// lock again keeps its `since` and takes `row.inputs`.
    pub fn set_dp_payment_lock(
        &self,
        wallet_id: &str,
        row: &DpPaymentLockRow,
    ) -> Result<Option<String>> {
        Self::upsert_payment_lock(&self.conn(), wallet_id, row)
    }

    /// [`Self::set_dp_payment_lock`], durable against power loss when it
    /// returns `Ok`: the write commits under `synchronous=FULL`, which syncs
    /// the WAL at the commit (the connection otherwise runs `NORMAL`, under
    /// which a commit is not synced), and a failed sync is an error. On
    /// Apple platforms also `fullfsync`, as the dispatch journal's: their
    /// fsync does not flush the disk cache. One WAL sync, not a checkpoint:
    /// no other connection can make it busy. The connection is back to
    /// `NORMAL` afterwards (DP3-01 D27).
    pub fn set_dp_payment_lock_durable(
        &self,
        wallet_id: &str,
        row: &DpPaymentLockRow,
    ) -> Result<Option<String>> {
        self.durably(|conn| Self::upsert_payment_lock(conn, wallet_id, row))
    }

    /// Runs `write` on the connection with its commits synced (`synchronous=
    /// FULL`, plus `fullfsync` on Apple platforms), then back to `NORMAL` on
    /// every exit, a panic included: the connection outlives a poisoned
    /// mutex (Sol r3). The write's error comes before the restore's.
    fn durably<T>(&self, write: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        /// Puts the connection back to `NORMAL` when dropped, unless
        /// `restore` already did.
        struct Synced<'c>(Option<&'c Connection>);
        impl Synced<'_> {
            fn restore(&mut self) -> rusqlite::Result<()> {
                match self.0.take() {
                    Some(conn) => set_synced(conn, false),
                    None => Ok(()),
                }
            }
        }
        impl Drop for Synced<'_> {
            fn drop(&mut self) {
                let _ = self.restore();
            }
        }
        fn set_synced(conn: &Connection, on: bool) -> rusqlite::Result<()> {
            conn.pragma_update(None, "synchronous", if on { "FULL" } else { "NORMAL" })?;
            if cfg!(target_vendor = "apple") {
                conn.pragma_update(None, "fullfsync", on)?;
            }
            Ok(())
        }

        let conn = self.conn();
        #[cfg(all(debug_assertions, any(test, feature = "durability-sim")))]
        if durability_sim::skips_barrier(&conn) {
            return write(&conn);
        }
        let mut synced = Synced(Some(&conn));
        let written = set_synced(&conn, true)
            .map_err(Into::into)
            .and_then(|()| write(&conn));
        let normal = synced.restore();
        let written = written?;
        normal?;
        #[cfg(all(debug_assertions, any(test, feature = "durability-sim")))]
        durability_sim::synced(&conn);
        Ok(written)
    }

    fn upsert_payment_lock(
        conn: &Connection,
        wallet_id: &str,
        row: &DpPaymentLockRow,
    ) -> Result<Option<String>> {
        conn.execute(
            "INSERT INTO dp_payment_lock (wallet_id, identity, contact, txid, since, inputs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (wallet_id, identity, contact)
             DO UPDATE SET inputs = excluded.inputs WHERE txid = excluded.txid",
            params![
                wallet_id,
                row.identity,
                row.contact,
                row.txid,
                row.since as i64,
                row.inputs
            ],
        )?;
        let held: String = conn.query_row(
            "SELECT txid FROM dp_payment_lock
             WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3",
            params![wallet_id, row.identity, row.contact],
            |r| r.get(0),
        )?;
        Ok((held != row.txid).then_some(held))
    }

    /// Removes the pair's lock if it is still `txid`'s; whether one went.
    pub fn clear_dp_payment_lock(
        &self,
        wallet_id: &str,
        identity: &str,
        contact: &str,
        txid: &str,
    ) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM dp_payment_lock
             WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3 AND txid = ?4",
            params![wallet_id, identity, contact, txid],
        )?;
        Ok(n > 0)
    }

    /// Every `(identity, key, value)` of the wallet's `dp_prefs` rows, by
    /// identity and key, from one statement: one snapshot of the rows, so a
    /// write between two reads cannot mix their generations.
    pub fn wallet_dp_prefs(&self, wallet_id: &str) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT identity, key, value FROM dp_prefs WHERE wallet_id = ?1 ORDER BY identity, key",
        )?;
        let rows = stmt.query_map([wallet_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every `(identity, value)` of the wallet's `dp_prefs` rows for `key`,
    /// by identity.
    pub fn dp_prefs(&self, wallet_id: &str, key: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT identity, value FROM dp_prefs WHERE wallet_id = ?1 AND key = ?2
             ORDER BY identity",
        )?;
        let rows = stmt.query_map(params![wallet_id, key], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The digest `dp_contact_info` holds for `identity`'s details of
    /// `contact` (DP2-04), if any.
    pub fn contact_info_digest(
        &self,
        wallet_id: &str,
        identity: &str,
        contact: &str,
    ) -> Result<Option<[u8; 32]>> {
        let digest: Option<Vec<u8>> = self
            .conn()
            .query_row(
                "SELECT digest FROM dp_contact_info
                 WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3",
                params![wallet_id, identity, contact],
                |r| r.get(0),
            )
            .optional()?;
        digest
            .map(|d| {
                <[u8; 32]>::try_from(d.as_slice())
                    .map_err(|_| AppDbError::Corrupt("dp_contact_info.digest".into()))
            })
            .transpose()
    }

    /// Records what Platform holds of `identity`'s details of `contact`;
    /// whether that changed the row (`recorded_at` is when its digest was
    /// first recorded).
    pub fn set_contact_info_digest(
        &self,
        wallet_id: &str,
        identity: &str,
        contact: &str,
        digest: &[u8; 32],
        now: u64,
    ) -> Result<bool> {
        let changed = self.conn().execute(
            "INSERT INTO dp_contact_info (wallet_id, identity, contact, digest, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (wallet_id, identity, contact)
             DO UPDATE SET digest = excluded.digest, recorded_at = excluded.recorded_at
             WHERE digest != excluded.digest",
            params![wallet_id, identity, contact, &digest[..], now as i64],
        )?;
        Ok(changed > 0)
    }

    /// Forgets the digest of `identity`'s details of `contact`: Platform
    /// holds none. Whether there was one.
    pub fn forget_contact_info_digest(
        &self,
        wallet_id: &str,
        identity: &str,
        contact: &str,
    ) -> Result<bool> {
        let changed = self.conn().execute(
            "DELETE FROM dp_contact_info WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3",
            params![wallet_id, identity, contact],
        )?;
        Ok(changed > 0)
    }

    /// Every digest `dp_contact_info` holds for the wallet, as (identity,
    /// contact, digest): what the engine keeps in memory for its sync reads.
    pub fn contact_info_digests(&self, wallet_id: &str) -> Result<Vec<(String, String, [u8; 32])>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT identity, contact, digest FROM dp_contact_info WHERE wallet_id = ?1
             ORDER BY identity, contact",
        )?;
        let rows = stmt
            .query_map([wallet_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(identity, contact, digest)| {
                let digest = <[u8; 32]>::try_from(digest.as_slice())
                    .map_err(|_| AppDbError::Corrupt("dp_contact_info.digest".into()))?;
                Ok((identity, contact, digest))
            })
            .collect()
    }

    /// Forgets the digests of `identity`'s details of every contact not in
    /// `keep` (no longer established); the contacts it forgot.
    pub fn forget_contact_infos_except(
        &self,
        wallet_id: &str,
        identity: &str,
        keep: &[String],
    ) -> Result<Vec<String>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let held = {
            let mut stmt = tx.prepare(
                "SELECT contact FROM dp_contact_info WHERE wallet_id = ?1 AND identity = ?2",
            )?;
            stmt.query_map(params![wallet_id, identity], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let gone: Vec<String> = held.into_iter().filter(|c| !keep.contains(c)).collect();
        for contact in &gone {
            tx.execute(
                "DELETE FROM dp_contact_info WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3",
                params![wallet_id, identity, contact],
            )?;
        }
        tx.commit()?;
        Ok(gone)
    }

    /// Sets an identity's preference, or deletes it for `None`.
    pub fn set_dp_pref(
        &self,
        wallet_id: &str,
        identity: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn();
        match value {
            Some(v) => conn.execute(
                "INSERT INTO dp_prefs (wallet_id, identity, key, value) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (wallet_id, identity, key) DO UPDATE SET value = excluded.value",
                params![wallet_id, identity, key, v],
            )?,
            None => conn.execute(
                "DELETE FROM dp_prefs WHERE wallet_id = ?1 AND identity = ?2 AND key = ?3",
                params![wallet_id, identity, key],
            )?,
        };
        Ok(())
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
        assert_eq!(db.schema_version().unwrap(), 2026101901);
        db.set_wallet_name(W, "Main", 1).unwrap();
        drop(db);
        let db = AppDb::open(&path).unwrap();
        assert_eq!(db.wallet_names().unwrap(), vec![(W.into(), "Main".into())]);
    }

    #[test]
    fn payment_locks_keep_their_payment_and_clear_only_their_txid() {
        let db = AppDb::open_in_memory().unwrap();
        let row = |c: &str, t: &str, since| DpPaymentLockRow {
            identity: "me".into(),
            contact: c.into(),
            txid: t.into(),
            since,
            inputs: format!("{t}:0"),
        };
        assert_eq!(
            db.set_dp_payment_lock(W, &row("bob", "t1", 10)).unwrap(),
            None
        );
        // Another payment never replaces the pair's lock.
        assert_eq!(
            db.set_dp_payment_lock(W, &row("bob", "t2", 20)).unwrap(),
            Some("t1".into())
        );
        // The same payment's again keeps its `since`, takes its inputs.
        let again = DpPaymentLockRow {
            inputs: "t1:1".into(),
            ..row("bob", "t1", 15)
        };
        assert_eq!(db.set_dp_payment_lock(W, &again).unwrap(), None);
        db.set_dp_payment_lock(W, &row("carol", "t3", 30)).unwrap();
        db.set_dp_payment_lock(W2, &row("bob", "t4", 40)).unwrap();
        let first = DpPaymentLockRow {
            inputs: "t1:1".into(),
            ..row("bob", "t1", 10)
        };
        assert_eq!(
            db.dp_payment_locks(W).unwrap(),
            vec![first, row("carol", "t3", 30)]
        );
        assert!(!db.clear_dp_payment_lock(W, "me", "bob", "t2").unwrap());
        assert!(db.clear_dp_payment_lock(W, "me", "bob", "t1").unwrap());
        assert_eq!(
            db.dp_payment_locks(W).unwrap(),
            vec![row("carol", "t3", 30)]
        );
        assert_eq!(db.dp_payment_locks(W2).unwrap(), vec![row("bob", "t4", 40)]);
    }

    /// MG-03 D34: a durable setting survives power loss, with every commit
    /// before it; a plain one after it doesn't; with the barrier skipped
    /// the durable one doesn't either. Debug builds only, as the simulator
    /// (D38).
    #[cfg(all(test, debug_assertions))]
    #[test]
    fn a_durable_setting_survives_power_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DB_FILE);
        drop(AppDb::open(&path).unwrap());
        let value = |key: &str| AppDb::open(&path).unwrap().setting("g", key).unwrap();

        durability_sim::arm(&path, false);
        let db = AppDb::open(&path).unwrap();
        db.set_setting("g", "before", Some("1")).unwrap();
        db.set_setting_durable("g", "durable", Some("2")).unwrap();
        db.set_setting("g", "after", Some("3")).unwrap();
        drop(db);
        durability_sim::power_loss(&path);
        assert_eq!(value("before").as_deref(), Some("1"), "synced with it");
        assert_eq!(value("durable").as_deref(), Some("2"));
        assert_eq!(value("after"), None, "never synced");

        durability_sim::arm(&path, true);
        let db = AppDb::open(&path).unwrap();
        db.set_setting_durable("g", "durable", None).unwrap();
        drop(db);
        durability_sim::power_loss(&path);
        assert_eq!(value("durable").as_deref(), Some("2"), "the delete is lost");
    }

    /// D27: the durable write commits under `synchronous=FULL` (a trigger
    /// refusing any other commit sees it), the plain one does not, and the
    /// connection is back to `NORMAL` after either outcome.
    #[test]
    fn the_durable_payment_lock_commits_synced() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDb::open(&dir.path().join(APP_DB_FILE)).unwrap();
        let row = DpPaymentLockRow {
            identity: "me".into(),
            contact: "bob".into(),
            txid: "t1".into(),
            since: 1,
            inputs: String::new(),
        };
        // Apple platforms also need `fullfsync` for a sync that survives
        // power loss.
        let synchronous = |db: &AppDb| -> i64 {
            let conn = db.conn();
            let level: i64 = conn
                .query_row("PRAGMA synchronous", [], |r| r.get(0))
                .unwrap();
            let full: i64 = conn
                .query_row("PRAGMA fullfsync", [], |r| r.get(0))
                .unwrap();
            assert_eq!(full, 0, "fullfsync is back off");
            level
        };
        let apple = if cfg!(target_vendor = "apple") {
            " OR (SELECT fullfsync FROM pragma_fullfsync) = 0"
        } else {
            ""
        };
        db.conn()
            .execute_batch(&format!(
                "CREATE TRIGGER need_full BEFORE INSERT ON dp_payment_lock BEGIN
                 SELECT RAISE(FAIL, 'not synced')
                 WHERE (SELECT synchronous FROM pragma_synchronous) < 2{apple}; END;"
            ))
            .unwrap();
        assert!(db.set_dp_payment_lock(W, &row).is_err());
        assert_eq!(synchronous(&db), 1);
        assert_eq!(db.set_dp_payment_lock_durable(W, &row).unwrap(), None);
        assert_eq!(synchronous(&db), 1, "back to NORMAL");
        db.conn()
            .execute_batch(
                "DROP TRIGGER need_full;
                 CREATE TRIGGER fail_lock BEFORE UPDATE ON dp_payment_lock BEGIN
                 SELECT RAISE(FAIL, 'injected'); END;",
            )
            .unwrap();
        assert!(db.set_dp_payment_lock_durable(W, &row).is_err());
        assert_eq!(synchronous(&db), 1, "back to NORMAL after an error");
        assert_eq!(db.dp_payment_locks(W).unwrap(), vec![row]);

        // A panic in the synced section (Sol r3): the reused connection is
        // back to NORMAL.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.durably(|conn| -> Result<()> {
                let level: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
                assert_eq!(level, 2, "FULL inside");
                panic!("injected panic");
            })
        }));
        assert!(panicked.is_err());
        assert_eq!(synchronous(&db), 1, "back to NORMAL after a panic");
    }

    #[test]
    fn identity_choices_round_trip_per_wallet() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(db.main_identity(W).unwrap(), None);
        db.set_main_identity(W, "id1").unwrap();
        db.set_main_identity(W, "id2").unwrap();
        db.set_main_identity(W2, "other").unwrap();
        assert_eq!(db.main_identity(W).unwrap().as_deref(), Some("id2"));

        db.set_dp_pref(W, "id2", "main_name", Some("alice"))
            .unwrap();
        db.set_dp_pref(W, "id1", "main_name", Some("bob")).unwrap();
        db.set_dp_pref(W, "id1", "main_name", Some("bobby"))
            .unwrap();
        db.set_dp_pref(W, "id1", "other", Some("x")).unwrap();
        db.set_dp_pref(W2, "id1", "main_name", Some("carol"))
            .unwrap();
        let prefs = |w| db.dp_prefs(w, "main_name").unwrap();
        assert_eq!(
            prefs(W),
            vec![
                ("id1".into(), "bobby".into()),
                ("id2".into(), "alice".into())
            ]
        );
        db.set_dp_pref(W, "id2", "main_name", None).unwrap();
        assert_eq!(prefs(W), vec![("id1".into(), "bobby".into())]);

        assert_eq!(
            db.wallet_dp_prefs(W).unwrap(),
            vec![
                ("id1".into(), "main_name".into(), "bobby".into()),
                ("id1".into(), "other".into(), "x".into()),
            ]
        );

        db.delete_wallet(W).unwrap();
        assert_eq!(db.main_identity(W).unwrap(), None);
        assert!(prefs(W).is_empty());
        assert!(db.wallet_dp_prefs(W).unwrap().is_empty());
        assert_eq!(prefs(W2), vec![("id1".into(), "carol".into())]);
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

    /// After a full checkpoint the write is in the database file itself:
    /// a copy of that file alone (no `-wal`) has it; an ordinary commit is
    /// only in the WAL.
    #[test]
    fn a_full_checkpoint_puts_committed_writes_in_the_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_DB_FILE);
        let db = AppDb::open(&path).unwrap();
        // Migrations leave a WAL; start from a checkpointed file.
        db.checkpoint_full().unwrap();
        let in_file_alone = |key: &str| {
            let copy = dir.path().join("copy.sqlite");
            std::fs::copy(&path, &copy).unwrap();
            let conn = Connection::open(&copy).unwrap();
            let found: Option<String> = conn
                .query_row("SELECT value FROM settings_kv WHERE key = ?1", [key], |r| {
                    r.get(0)
                })
                .optional()
                .unwrap();
            drop(conn);
            std::fs::remove_file(&copy).unwrap();
            found.is_some()
        };
        db.set_setting(GLOBAL_SCOPE, "wal_only", Some("1")).unwrap();
        assert!(!in_file_alone("wal_only"));
        db.checkpoint_full().unwrap();
        assert!(in_file_alone("wal_only"));

        let reader = Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM settings_kv;")
            .unwrap();
        db.set_setting(GLOBAL_SCOPE, "later", Some("1")).unwrap();
        assert!(db.checkpoint_full().is_err(), "a reader pins the WAL");
    }

    #[test]
    fn a_raised_setting_never_goes_down() {
        let db = AppDb::open_in_memory().unwrap();
        let raise = |v, keep| db.raise_setting(GLOBAL_SCOPE, "b", v, keep).unwrap();
        assert_eq!(raise(900, true), "900");
        assert_eq!(raise(2000, true), "900", "kept");
        assert_eq!(raise(2000, false), "2000");
        assert_eq!(raise(1000, false), "2000", "a stale caller");
        assert_eq!(raise(10_000, false), "10000", "numeric, not text, order");
        db.set_setting(GLOBAL_SCOPE, "b", Some("garbled")).unwrap();
        assert_eq!(raise(20_000, false), "garbled");
        assert_eq!(
            db.setting(GLOBAL_SCOPE, "b").unwrap().as_deref(),
            Some("garbled")
        );
    }

    #[test]
    fn governance_registrations_put_replace_and_remove() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(db.governance_registrations().unwrap().is_empty());
        db.update_governance_registrations(&[([1; 32], vec![1]), ([2; 32], vec![2])], &[])
            .unwrap();
        db.update_governance_registrations(&[([2; 32], vec![2, 2])], &[[1; 32], [9; 32]])
            .unwrap();
        assert_eq!(
            db.governance_registrations().unwrap(),
            vec![([2; 32], vec![2, 2])]
        );
        // A key of another length never comes back.
        db.conn()
            .execute(
                "INSERT INTO governance_registrations VALUES (x'01', x'02')",
                [],
            )
            .unwrap_err();
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
            db.set_setting(&local_scope(w), "k", Some("v")).unwrap();
        }
        // The local scope stays out of a backup.
        let exported = db.export_wallet_rows(W).unwrap();
        let settings = exported.iter().find(|t| t.table == "settings_kv").unwrap();
        assert_eq!(settings.rows.len(), 1);
        db.delete_wallet(W).unwrap();
        assert_eq!(db.setting(&local_scope(W), "k").unwrap(), None);
        assert_eq!(db.setting(&local_scope(W2), "k").unwrap(), Some("v".into()));
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
