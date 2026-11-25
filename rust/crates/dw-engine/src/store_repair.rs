//! The open's repair of stored unsigned pending claims (#739, Q112).
//!
//! A peer's unconfirmed transaction that spends one of the wallet's coins
//! without its owner's signature is recorded by key-wallet, and the store
//! keeps it with the coin marked spent. Before the persister opens
//! `wallet.sqlite`, each such claim is dropped from the store and its coin
//! given back; what it was is recorded in the app database (never in the
//! wallet store, whose schema belongs to platform-wallet-storage), where the
//! history load reads it ([`store_dropped`]).
//!
//! A claim drops only when, for a coin the store holds whose funding
//! transaction is stored and hashes to the coin's outpoint, one of its
//! inputs is proven unsigned (`Ok(false)`) and is push-only or structurally
//! invalid (S3, S4); a script that isn't push-only can build its key and
//! signature as it runs, so its `Ok(false)` alone keeps the claim (S1). It
//! is kept when anything of its tree is settled in any wallet of the file,
//! or may be (a lock row of any kind, a sweep's named winner), or when that
//! can't be told (S2), and when it is mined or locked (S6).
//!
//! The store's schema is read only at the versions this build was checked
//! against ([`KNOWN_SCHEMA`]); any other repairs nothing. The repair runs in
//! one write transaction, and its records are staged in the app database
//! before that transaction commits: a staged record counts only once its
//! claim's row is gone from the store, which is the store transaction's
//! commit, so an exit anywhere leaves the two agreeing.
//!
//! Before that, in the same transaction, each claim a fork build's open
//! dropped (`core_dropped_unsigned_spends`, published pins 373342cb3 and
//! c5917e6bd, which dropped any input proven unsigned, push-only or not,
//! and saw a claim settled only by its own block or lock) is judged again
//! by these rules (C3 L1); a row that took a lock row with it is kept,
//! whether the lock is verified or reads at all (L1a). One they drop stays
//! dropped (L2); one they keep, or can't judge, is put back pending with
//! its coins held again, and its row goes (L3), except a coin something
//! stored spends since (L4), which also keeps the claim's outputs out. On
//! a fork build's store this runs at the first open, before the persister
//! upgrades it ([`FORK_SCHEMA`]); after that, at each open, for the rows
//! still there.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::str::FromStr;

use dashcore::{OutPoint, Transaction, TxOut, Txid};
use dw_appdb::AppDb;
use key_wallet::managed_account::transaction_record::TransactionRecord;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{EngineError, WalletId};

/// The `wallet.sqlite` schema versions (refinery's highest applied
/// migration) whose tables this repair was checked against:
/// platform-wallet-storage V024 at the platform pin.
pub(crate) const KNOWN_SCHEMA: &[i64] = &[24];

/// The refinery version of the published fork builds' stores. Before the
/// persister upgrades one, only the re-check runs, and only when the
/// fork's [`LEGACY`] table is there, so no session offers a coin the fork
/// freed that these rules hold.
const FORK_SCHEMA: i64 = 18;

/// The table the fork's schema lacks (V021 creates it and indexes every
/// stored record, a put-back's included).
const INPUT_INDEX: &str = "core_transaction_inputs";

/// The most rows (transactions and InstantSend locks, every wallet) the
/// walk for a settled descendant reads; a larger file repairs nothing.
pub(crate) const MAX_WALK_ROWS: usize = 250_000;

/// A wallet's record of a claim this repair dropped from the store.
pub(crate) const STORE_DROPPED_PREFIX: &str = "store_dropped:";

/// Where the claims of a repair whose store transaction may not have
/// committed are listed, across wallets.
const STAGED_SCOPE: &str = "store_repair";
const STAGED_KEY: &str = "staged";

/// The account type of a contact's address pool: its coins are not given
/// back.
const CONTACT_POOL: &str = "dashpay_external";

/// The fork's record of the claims its open dropped as unsigned.
const LEGACY: &str = "core_dropped_unsigned_spends";

/// The columns of [`LEGACY`] the re-check reads.
const LEGACY_COLUMNS: &[&str] = &[
    "id",
    "wallet_id",
    "txid",
    "record_blob",
    "islock_blob",
    "islock_verified",
    "deleted_outputs",
    "released_coins",
];

/// The app-database key prefix of a claim the re-check put back.
pub(crate) const STORE_RESTORED_PREFIX: &str = "store_restored:";
const STAGED_RESTORED_KEY: &str = "staged_restored";

/// Each table and column the repair reads or writes.
const COLUMNS: &[(&str, &[&str])] = &[
    (
        "core_transactions",
        &[
            "wallet_id",
            "txid",
            "height",
            "block_hash",
            "block_time",
            "finalized",
            "record_blob",
        ],
    ),
    (INPUT_INDEX, &["wallet_id", "txid", "outpoint"]),
    (
        "core_utxos",
        &[
            "wallet_id",
            "outpoint",
            "value",
            "script",
            "spent",
            "spent_in_txid",
            "winner_mined_height",
            "is_sweep_placeholder",
        ],
    ),
    (
        "core_instant_locks",
        &["wallet_id", "txid", "islock_blob", "verified"],
    ),
    (
        "core_address_pool",
        &["wallet_id", "account_type", "script"],
    ),
    ("meta_data_versions", &["wallet_id", "domain", "seq"]),
];

/// The verifier's verdict on input `index` of `tx` for `prevout`: proven
/// not signed by the coin's owner. An error (a form the verifier can't
/// decide, a signature built at run time) is no proof.
fn proven_unsigned(tx: &Transaction, index: usize, prevout: &TxOut) -> bool {
    crate::input_authorization::verify_input_authorization(tx, index, prevout) == Ok(false)
}

/// Whether input `index` makes `tx` consensus-invalid whatever its script
/// computes (S4): the script is longer than `MAX_SCRIPT_SIZE` or doesn't
/// parse, or its outpoint is spent twice in `tx`.
fn structurally_invalid(tx: &Transaction, index: usize) -> bool {
    let input = &tx.input[index];
    input.script_sig.len() > crate::input_authorization::MAX_SCRIPT_SIZE
        || input.script_sig.instructions().any(|i| i.is_err())
        || (tx.input.iter())
            .filter(|i| i.previous_output == input.previous_output)
            .count()
            > 1
}

/// Why a claim was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Cause {
    /// A push-only input proven unsigned (S3).
    Unsigned,
    /// A structurally invalid input proven unsigned (S4).
    Structural,
}

/// A record's state: staged before the store transaction commits, done
/// once it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Staged,
    Done,
}

/// A `core_utxos` row as found, blobs in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CoinRow {
    outpoint: String,
    value: i64,
    script: String,
    spent: i64,
    spent_in_txid: Option<String>,
    winner_mined_height: Option<i64>,
    is_sweep_placeholder: i64,
}

/// What the repair took out of the store for one claim, so it can be put
/// back by hand: its record, its lock row, its output rows and the coin
/// rows it released, as found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DropRecord {
    state: State,
    cause: Cause,
    record_blob: String,
    islock_blob: Option<String>,
    deleted_outputs: Vec<CoinRow>,
    released_coins: Vec<CoinRow>,
}

/// One planned drop.
struct Planned {
    wallet: [u8; 32],
    txid: Txid,
    record: DropRecord,
    released: Vec<Vec<u8>>,
    deleted: Vec<Vec<u8>>,
}

/// Why a claim the fork dropped is put back (C3 L3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    /// These rules keep it.
    Keep,
    /// They can't judge it: the walk can't complete.
    Doubt,
}

/// A [`LEGACY`] row as found, blobs in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyRow {
    id: i64,
    record_blob: String,
    islock_blob: Option<String>,
    islock_verified: Option<i64>,
    deleted_outputs: String,
    released_coins: String,
}

/// What the re-check put back for one claim the fork dropped, and the
/// rows it took that from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RestoreRecord {
    state: State,
    verdict: Verdict,
    reason: String,
    legacy_rows: Vec<LegacyRow>,
    /// False when only its coins are held: its record doesn't read, or the
    /// store holds the claim again.
    restored_transaction: bool,
    restored_lock: bool,
    restored_outputs: Vec<String>,
    held_coins: Vec<String>,
    /// Coins it spent that something stored spends since (L4).
    not_held: Vec<String>,
}

/// How a put-back writes its claim's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Write {
    /// A new row.
    Insert,
    /// Its record into the row the store holds without one.
    Fill,
    /// None: only its coins are held.
    Nothing,
}

/// One planned put-back.
struct Restore {
    wallet: [u8; 32],
    txid: Txid,
    write: Write,
    blob: Vec<u8>,
    block: Option<(i64, Vec<u8>, i64)>,
    inputs: Vec<Vec<u8>>,
    lock: Option<Vec<u8>>,
    outputs: Vec<(Vec<u8>, i64, Vec<u8>, i64)>,
    held: Vec<Vec<u8>>,
    record: RestoreRecord,
}

/// A test's stop inside the repair.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The whole repair skipped, as a build without it would open.
    Skip,
    /// After the records are staged, before the store commits.
    BeforeCommit,
    /// After the store commits, before the records are done.
    BeforePromote,
}

#[cfg(test)]
static STOPS: std::sync::Mutex<Vec<(std::path::PathBuf, Stop)>> = std::sync::Mutex::new(Vec::new());

/// Arms `stop` for the next repair of the store in `network_dir`.
#[cfg(test)]
pub(crate) fn arm(network_dir: &Path, stop: Stop) {
    (STOPS.lock().unwrap()).push((network_dir.to_path_buf(), stop));
}

#[cfg(test)]
fn stop_here(network_dir: &Path, stop: Stop) -> bool {
    let mut stops = STOPS.lock().unwrap();
    match stops
        .iter()
        .position(|(d, s)| d == network_dir && *s == stop)
    {
        Some(n) => {
            stops.remove(n);
            true
        }
        None => false,
    }
}

#[cfg(test)]
static ROW_LIMITS: std::sync::Mutex<Vec<(std::path::PathBuf, usize)>> =
    std::sync::Mutex::new(Vec::new());

/// Walks at most `rows` rows in the next repair of the store in
/// `network_dir`.
#[cfg(test)]
pub(crate) fn limit_rows(network_dir: &Path, rows: usize) {
    (ROW_LIMITS.lock().unwrap()).push((network_dir.to_path_buf(), rows));
}

fn row_limit(network_dir: &Path) -> usize {
    #[cfg(test)]
    {
        let mut limits = ROW_LIMITS.lock().unwrap();
        if let Some(n) = limits.iter().position(|(d, _)| d == network_dir) {
            return limits.remove(n).1;
        }
    }
    let _ = network_dir;
    MAX_WALK_ROWS
}

/// What [`repair`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// The schema version it read, when known.
    pub(crate) schema: Option<i64>,
    /// Claims dropped, by wallet.
    pub(crate) dropped: BTreeMap<WalletId, Vec<Txid>>,
    /// Claims a fork build dropped that the re-check put back, by wallet.
    pub(crate) restored: BTreeMap<WalletId, Vec<Txid>>,
}

fn storage(e: impl std::fmt::Display) -> EngineError {
    EngineError::Storage(format!("store repair: {e}"))
}

/// The open's repair of `network_dir`'s `wallet.sqlite`, before the
/// persister opens it and once the app database is open. A file of an
/// unknown schema, one a restore will replace, or a walk that can't
/// complete repairs nothing. Every error leaves the store as it was, or,
/// once the store committed, the records staged, which count from then on.
pub(crate) fn repair(network_dir: &Path, appdb: &AppDb) -> Result<Outcome, EngineError> {
    crate::store_upgrade::order::record(
        network_dir,
        crate::store_upgrade::order::Step::StoreRepair,
    );
    #[cfg(test)]
    if stop_here(network_dir, Stop::Skip) {
        return Ok(Outcome::default());
    }
    let path = network_dir.join(crate::session::WALLET_DB_FILE);
    if !path.exists() {
        return Ok(Outcome::default());
    }
    if dw_store_upgrade::pending_restore(network_dir)
        .map_err(storage)?
        .is_some()
    {
        tracing::warn!("store repair: a restore is pending; nothing repaired");
        return Ok(Outcome::default());
    }
    let mut conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(storage)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(storage)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(storage)?;
    let Some((schema, legacy)) = known_schema(&conn).map_err(storage)? else {
        return Ok(Outcome::default());
    };
    let mut outcome = Outcome {
        schema: Some(schema),
        ..Default::default()
    };
    resolve_staged(&conn, appdb, legacy)?;

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage)?;
    let max_rows = row_limit(network_dir);
    let fork = schema == FORK_SCHEMA;
    let restores = match legacy {
        true => recheck(&tx, max_rows, !fork).map_err(storage)?,
        false => Vec::new(),
    };
    restore(&tx, &restores).map_err(storage)?;
    let drops = match fork {
        true => Vec::new(),
        false => plan(&tx, max_rows).map_err(storage)?.unwrap_or_default(),
    };
    if drops.is_empty() && restores.is_empty() {
        return Ok(outcome);
    }
    apply(&tx, &drops).map_err(storage)?;
    stage(appdb, &drops, &restores)?;
    #[cfg(test)]
    if stop_here(network_dir, Stop::BeforeCommit) {
        return Err(storage("test: stopped before the commit"));
    }
    tx.commit().map_err(storage)?;
    for d in &drops {
        (outcome.dropped.entry(WalletId(d.wallet)).or_default()).push(d.txid);
    }
    for r in &restores {
        (outcome.restored.entry(WalletId(r.wallet)).or_default()).push(r.txid);
    }
    if !outcome.dropped.is_empty() {
        tracing::warn!(dropped = ?outcome.dropped, "store repair: dropped unconfirmed transactions that spend a coin without its owner's signature");
    }
    if !outcome.restored.is_empty() {
        tracing::warn!(restored = ?outcome.restored, "store repair: put back pending transactions a fork build dropped as unsigned");
    }
    #[cfg(test)]
    if stop_here(network_dir, Stop::BeforePromote) {
        return Err(storage("test: stopped before the records are done"));
    }
    promote(appdb, &drops, &restores)?;
    Ok(outcome)
}

/// The store's schema version when this build was checked against it and
/// every table and column the repair uses is there, and whether the fork's
/// [`LEGACY`] table is, with every column the re-check reads; else `None`
/// (logged). A store at [`FORK_SCHEMA`] counts only with that table, and
/// needs no [`INPUT_INDEX`].
fn known_schema(conn: &rusqlite::Connection) -> rusqlite::Result<Option<(i64, bool)>> {
    let history: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' \
         AND name = 'refinery_schema_history')",
        [],
        |r| r.get(0),
    )?;
    if !history {
        tracing::warn!("store repair: the store has no schema history; nothing repaired");
        return Ok(None);
    }
    let version: Option<i64> = conn.query_row(
        "SELECT MAX(version) FROM refinery_schema_history",
        [],
        |r| r.get(0),
    )?;
    let Some(version) = version.filter(|v| KNOWN_SCHEMA.contains(v) || *v == FORK_SCHEMA) else {
        tracing::warn!(?version, known = ?KNOWN_SCHEMA, "store repair: an unknown store schema; nothing repaired");
        return Ok(None);
    };
    for (table, columns) in COLUMNS {
        if version == FORK_SCHEMA && *table == INPUT_INDEX {
            continue;
        }
        let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let have: HashSet<String> = stmt
            .query_map([table], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if let Some(missing) = columns.iter().find(|c| !have.contains(**c)) {
            tracing::warn!(
                version,
                table,
                column = missing,
                "store repair: a column it uses is missing; nothing repaired"
            );
            return Ok(None);
        }
    }
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let have: HashSet<String> = stmt
        .query_map([LEGACY], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if have.is_empty() {
        if version == FORK_SCHEMA {
            return Ok(None);
        }
        return Ok(Some((version, false)));
    }
    if let Some(missing) = LEGACY_COLUMNS.iter().find(|c| !have.contains(**c)) {
        tracing::warn!(
            version,
            column = missing,
            "store repair: a column of the fork's dropped-spend table is missing; nothing repaired"
        );
        return Ok(None);
    }
    Ok(Some((version, true)))
}

/// What [`plan`] reads of the whole file: which transactions are settled
/// or may be (mined, any lock row, a sweep's named winner), what spends
/// each transaction's outputs, and the pending claims.
#[derive(Default)]
struct Graph {
    settled: HashSet<Txid>,
    children: HashMap<Txid, Vec<Txid>>,
    /// Each spent outpoint, with the transactions that spend it.
    inputs: HashMap<OutPoint, HashSet<Txid>>,
    pending: Vec<([u8; 32], TransactionRecord)>,
}

/// Reads every wallet's transactions and InstantSend locks; `None` when the
/// walk could not complete (a row that doesn't decode, a key of the wrong
/// size, or more than `max_rows` rows), logged.
fn read_graph(tx: &rusqlite::Transaction<'_>, max_rows: usize) -> rusqlite::Result<Option<Graph>> {
    let limit = crate::history_ops::lock_size_limit();
    let rows: i64 = tx.query_row(
        "SELECT (SELECT count(*) FROM core_transactions) + (SELECT count(*) FROM core_instant_locks) \
         + (SELECT count(*) FROM core_utxos WHERE spent_in_txid IS NOT NULL)",
        [],
        |r| r.get(0),
    )?;
    if usize::try_from(rows).unwrap_or(usize::MAX) > max_rows {
        tracing::warn!(
            rows,
            "store repair: too many rows to walk; nothing repaired"
        );
        return Ok(None);
    }
    let incomplete = |what: &str| {
        tracing::warn!(
            what,
            "store repair: a row doesn't read, so a settled descendant can't be ruled out; nothing repaired"
        );
        Ok(None)
    };
    let mut graph = Graph::default();
    let mut stmt = tx.prepare(
        "SELECT wallet_id, txid, height IS NOT NULL OR finalized <> 0, \
         record_blob IS NOT NULL, \
         CASE WHEN length(record_blob) <= ?1 THEN record_blob END \
         FROM core_transactions",
    )?;
    let mut q = stmt.query([limit])?;
    while let Some(r) = q.next()? {
        let (wallet, txid, settled, has_blob, blob): (
            Vec<u8>,
            Vec<u8>,
            bool,
            bool,
            Option<Vec<u8>>,
        ) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?);
        let (Ok(wallet), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid)) else {
            return incomplete("a transaction key");
        };
        if settled {
            graph.settled.insert(txid);
        }
        if !has_blob {
            continue;
        }
        let Some(record) = blob
            .as_deref()
            .and_then(crate::history_ops::decode_record)
            .filter(|rec| rec.txid == txid && rec.transaction.txid() == txid)
        else {
            return incomplete("a transaction record");
        };
        for input in &record.transaction.input {
            let parent = input.previous_output;
            graph.children.entry(parent.txid).or_default().push(txid);
            graph.inputs.entry(parent).or_default().insert(txid);
        }
        if record.block_info().is_some() {
            graph.settled.insert(txid);
        } else if !settled {
            graph.pending.push((wallet, record));
        }
    }
    drop(q);
    let mut stmt = tx.prepare(
        "SELECT wallet_id, txid, verified, \
         CASE WHEN length(islock_blob) <= ?1 THEN islock_blob END \
         FROM core_instant_locks",
    )?;
    let mut q = stmt.query([limit])?;
    while let Some(r) = q.next()? {
        let (wallet, txid, verified, blob): (Vec<u8>, Vec<u8>, bool, Option<Vec<u8>>) =
            (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let (Ok(_), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid)) else {
            return incomplete("a lock key");
        };
        // Any lock row may be the network's: an unverified one, or one
        // naming another transaction, keeps its transaction too.
        graph.settled.insert(txid);
        if !verified {
            continue;
        }
        let Some(lock) = blob.as_deref().and_then(crate::history_ops::decode_lock) else {
            return incomplete("an InstantSend lock");
        };
        if lock.txid != txid {
            continue;
        }
        for parent in &lock.inputs {
            graph.children.entry(parent.txid).or_default().push(txid);
            graph.inputs.entry(*parent).or_default().insert(txid);
        }
    }
    drop(q);
    // Only a mined or InstantSend-locked transaction wins a sweep.
    let mut stmt = tx
        .prepare("SELECT DISTINCT spent_in_txid FROM core_utxos WHERE spent_in_txid IS NOT NULL")?;
    let mut q = stmt.query([])?;
    while let Some(r) = q.next()? {
        let Ok(winner) = Txid::from_slice(&r.get::<_, Vec<u8>>(0)?) else {
            return incomplete("a sweep's winner");
        };
        graph.settled.insert(winner);
    }
    Ok(Some(graph))
}

/// The bytes the store keys `outpoint` by.
pub(crate) fn encode_outpoint(outpoint: &OutPoint) -> Vec<u8> {
    let config =
        bincode::config::standard().with_limit::<{ platform_wallet_storage::SIZE_LIMIT_BYTES }>();
    bincode::serde::encode_to_vec(outpoint, config).expect("an outpoint encodes")
}

fn decode_outpoint(bytes: &[u8]) -> Option<OutPoint> {
    crate::history_ops::decode_stored(bytes)
}

/// Why input `index` of `claim` (stored for `wallet`) drops it, if it does:
/// the coin it spends is stored for the wallet and is the output of a
/// stored funding transaction that hashes to its outpoint (S6), and the
/// verifier proves the input unsigned through a push-only (S3) or
/// structurally invalid (S4) script. A parseable script that isn't
/// push-only never drops (S1).
fn input_cause(
    tx: &rusqlite::Transaction<'_>,
    wallet: &[u8; 32],
    claim: &Transaction,
    index: usize,
) -> rusqlite::Result<Option<Cause>> {
    let input = &claim.input[index];
    let structural = structurally_invalid(claim, index);
    if !structural && !crate::tx_actions::push_only(&input.script_sig) {
        return Ok(None);
    }
    let o = input.previous_output;
    let coin: Option<(i64, Vec<u8>)> = tx
        .query_row(
            "SELECT value, script FROM core_utxos \
             WHERE wallet_id = ?1 AND outpoint = ?2 AND is_sweep_placeholder = 0",
            params![wallet.as_slice(), encode_outpoint(&o)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((value, script)) = coin else {
        return Ok(None);
    };
    let blob: Option<Vec<u8>> = tx
        .query_row(
            "SELECT record_blob FROM core_transactions \
             WHERE wallet_id = ?1 AND txid = ?2 AND record_blob IS NOT NULL",
            params![wallet.as_slice(), o.txid.as_byte_array().as_slice()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(output) = (blob.as_deref())
        .and_then(crate::history_ops::decode_record)
        .filter(|f| f.transaction.txid() == o.txid)
        .and_then(|f| f.transaction.output.get(o.vout as usize).cloned())
    else {
        return Ok(None);
    };
    if u64::try_from(value).ok() != Some(output.value) || script != output.script_pubkey.as_bytes()
    {
        tracing::warn!(coin = %o, "store repair: a stored coin differs from its funding's output; kept");
        return Ok(None);
    }
    if !proven_unsigned(claim, index, &output) {
        return Ok(None);
    }
    Ok(Some(match structural {
        true => Cause::Structural,
        false => Cause::Unsigned,
    }))
}

/// The first transaction of `root`'s tree (it and all that descends from
/// it in any wallet of the file) that is settled, or the root itself when
/// the tree can't be fully read: an output of it held spent by a sweep's
/// winner, by a transaction the file has no record of, or under a key that
/// doesn't decode (S2).
fn settled_in_tree(
    tx: &rusqlite::Transaction<'_>,
    graph: &Graph,
    root: Txid,
) -> rusqlite::Result<Option<Txid>> {
    let mut stmt = tx.prepare_cached(
        "SELECT outpoint, spent_in_txid IS NOT NULL OR is_sweep_placeholder <> 0 \
         FROM core_utxos WHERE substr(outpoint, 2, 32) = ?1 AND spent <> 0",
    )?;
    let (mut todo, mut seen) = (VecDeque::from([root]), HashSet::new());
    while let Some(t) = todo.pop_front() {
        if !seen.insert(t) {
            continue;
        }
        if graph.settled.contains(&t) {
            return Ok(Some(t));
        }
        let mut q = stmt.query([t.as_byte_array().as_slice()])?;
        while let Some(r) = q.next()? {
            let (key, swept): (Vec<u8>, bool) = (r.get(0)?, r.get(1)?);
            let known =
                decode_outpoint(&key).is_some_and(|o| o.txid == t && graph.inputs.contains_key(&o));
            if swept || !known {
                tracing::warn!(%root, at = %t, swept, "store repair: an output of the claim's tree is spent by what the store doesn't hold; kept");
                return Ok(Some(t));
            }
        }
        todo.extend(graph.children.get(&t).into_iter().flatten().copied());
    }
    Ok(None)
}

/// The drops of the whole file, `None` when the walk can't complete.
fn plan(tx: &rusqlite::Transaction<'_>, max_rows: usize) -> rusqlite::Result<Option<Vec<Planned>>> {
    let Some(graph) = read_graph(tx, max_rows)? else {
        return Ok(None);
    };
    let mut claims: Vec<([u8; 32], &TransactionRecord, Cause)> = Vec::new();
    for (wallet, record) in &graph.pending {
        // An unsigned input makes the drop sure (S3), whatever comes first.
        let mut cause = None;
        for index in 0..record.transaction.input.len() {
            match input_cause(tx, wallet, &record.transaction, index)? {
                Some(Cause::Unsigned) => {
                    cause = Some(Cause::Unsigned);
                    break;
                }
                Some(Cause::Structural) => cause = Some(Cause::Structural),
                None => {}
            }
        }
        let Some(cause) = cause else {
            continue;
        };
        if let Some(at) = settled_in_tree(tx, &graph, record.txid)? {
            tracing::warn!(claim = %record.txid, settled = %at, "store repair: kept an unsigned claim: something of its tree is settled");
            continue;
        }
        claims.push((*wallet, record, cause));
    }
    if claims.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let dropping: HashSet<Txid> = claims.iter().map(|(_, r, _)| r.txid).collect();
    let Some(claimed) = claimed_by_others(tx, &dropping)? else {
        return Ok(None);
    };
    let mut drops = Vec::new();
    for (wallet, record, cause) in claims {
        drops.push(drop_of(tx, &wallet, record, cause, &claimed)?);
    }
    Ok(Some(drops))
}

/// Every outpoint a stored transaction of any wallet spends, whatever its
/// finality, except the transactions of `except` (in every wallet that
/// stores them); `None` when a record doesn't read.
fn claimed_by_others(
    tx: &rusqlite::Transaction<'_>,
    except: &HashSet<Txid>,
) -> rusqlite::Result<Option<HashSet<OutPoint>>> {
    let mut stmt = tx
        .prepare("SELECT txid, record_blob FROM core_transactions WHERE record_blob IS NOT NULL")?;
    let mut q = stmt.query([])?;
    let mut claimed = HashSet::new();
    while let Some(r) = q.next()? {
        let (txid, blob): (Vec<u8>, Vec<u8>) = (r.get(0)?, r.get(1)?);
        let Some(record) = crate::history_ops::decode_record(&blob)
            .filter(|rec| rec.txid.as_byte_array().as_slice() == txid.as_slice())
        else {
            return Ok(None);
        };
        if !except.contains(&record.txid) {
            claimed.extend(record.transaction.input.iter().map(|i| i.previous_output));
        }
    }
    Ok(Some(claimed))
}

fn coin_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Vec<u8>, CoinRow)> {
    let outpoint: Vec<u8> = r.get(0)?;
    let row = CoinRow {
        outpoint: hex::encode(&outpoint),
        value: r.get(1)?,
        script: hex::encode(r.get::<_, Vec<u8>>(2)?),
        spent: r.get(3)?,
        spent_in_txid: r.get::<_, Option<Vec<u8>>>(4)?.map(hex::encode),
        winner_mined_height: r.get(5)?,
        is_sweep_placeholder: r.get(6)?,
    };
    Ok((outpoint, row))
}

const COIN_COLUMNS: &str =
    "outpoint, value, script, spent, spent_in_txid, winner_mined_height, is_sweep_placeholder";

/// The drop of `record`: its output rows go, and each coin it held spent
/// is given back when nothing else stored spends it (`claimed`), no
/// sweep's winner holds it, and one of the wallet's own address pools, not
/// a contact's, derived its script.
fn drop_of(
    tx: &rusqlite::Transaction<'_>,
    wallet: &[u8; 32],
    record: &TransactionRecord,
    cause: Cause,
    claimed: &HashSet<OutPoint>,
) -> rusqlite::Result<Planned> {
    let txid = record.txid;
    let key = txid.as_byte_array().as_slice().to_vec();
    let (mut deleted, mut deleted_rows) = (Vec::new(), Vec::new());
    {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT {COIN_COLUMNS} FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2"
        ))?;
        for vout in 0..record.transaction.output.len() {
            let o = OutPoint::new(txid, vout as u32);
            if let Some((op, row)) = stmt
                .query_row(params![wallet.as_slice(), encode_outpoint(&o)], coin_row)
                .optional()?
            {
                deleted.push(op);
                deleted_rows.push(row);
            }
        }
    }
    let (mut released, mut released_rows) = (Vec::new(), Vec::new());
    {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT {COIN_COLUMNS} FROM core_utxos u \
             WHERE u.wallet_id = ?1 AND u.outpoint = ?2 AND u.spent = 1 \
               AND u.is_sweep_placeholder = 0 \
               AND (u.spent_in_txid IS NULL OR u.spent_in_txid = ?3) \
               AND EXISTS (SELECT 1 FROM core_address_pool p \
                           WHERE p.wallet_id = u.wallet_id AND p.script = u.script \
                             AND p.account_type <> ?4)"
        ))?;
        let mut seen = HashSet::new();
        for input in &record.transaction.input {
            let o = input.previous_output;
            if claimed.contains(&o) || !seen.insert(o) {
                continue;
            }
            if let Some((op, row)) = stmt
                .query_row(
                    params![wallet.as_slice(), encode_outpoint(&o), &key, CONTACT_POOL],
                    coin_row,
                )
                .optional()?
            {
                released.push(op);
                released_rows.push(row);
            }
        }
    }
    let islock_blob: Option<Vec<u8>> = tx
        .query_row(
            "SELECT islock_blob FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2",
            params![wallet.as_slice(), &key],
            |r| r.get(0),
        )
        .optional()?;
    let record_blob: Vec<u8> = tx.query_row(
        "SELECT record_blob FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
        params![wallet.as_slice(), &key],
        |r| r.get(0),
    )?;
    Ok(Planned {
        wallet: *wallet,
        txid,
        record: DropRecord {
            state: State::Staged,
            cause,
            record_blob: hex::encode(record_blob),
            islock_blob: islock_blob.map(hex::encode),
            deleted_outputs: deleted_rows,
            released_coins: released_rows,
        },
        released,
        deleted,
    })
}

/// Writes `drops` to the store, inside its transaction.
fn apply(tx: &rusqlite::Transaction<'_>, drops: &[Planned]) -> rusqlite::Result<()> {
    for d in drops {
        let (wallet, key) = (d.wallet.as_slice(), d.txid.as_byte_array().as_slice());
        tx.execute(
            "DELETE FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
            params![wallet, key],
        )?;
        tx.execute(
            "DELETE FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2",
            params![wallet, key],
        )?;
        for op in &d.deleted {
            tx.execute(
                "DELETE FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
                params![wallet, op],
            )?;
        }
        for op in &d.released {
            tx.execute(
                "UPDATE core_utxos SET spent = 0, spent_in_txid = NULL \
                 WHERE wallet_id = ?1 AND outpoint = ?2",
                params![wallet, op],
            )?;
        }
    }
    bump_core(tx, drops.iter().map(|d| d.wallet))
}

/// Bumps the `core` data version of each of `wallets`.
fn bump_core(
    tx: &rusqlite::Transaction<'_>,
    wallets: impl Iterator<Item = [u8; 32]>,
) -> rusqlite::Result<()> {
    for wallet in wallets.collect::<HashSet<_>>() {
        tx.execute(
            "INSERT INTO meta_data_versions (wallet_id, domain, seq) VALUES (?1, 'core', 1) \
             ON CONFLICT(wallet_id, domain) DO UPDATE SET \
                seq = CASE WHEN seq >= 9223372036854775807 THEN seq ELSE seq + 1 END",
            [wallet.as_slice()],
        )?;
    }
    Ok(())
}

/// C3 L1: each claim the fork dropped ([`LEGACY`]), by wallet and
/// transaction, judged again by these rules over the whole file as it is
/// now, the fork's rows included: a lock one of them took settles its
/// claim, and its record links its claim into its parents' trees. One the
/// rules drop keeps its rows (L2); any other is planned back (L3). A record
/// that doesn't read (the fork wrote only records it had read, so it
/// shouldn't happen) is a doubt: the coins it released are held again.
/// `indexed`: the store has `core_transaction_inputs` (the fork's own
/// schema doesn't; the persister's upgrade indexes the records then).
fn recheck(
    tx: &rusqlite::Transaction<'_>,
    max_rows: usize,
    indexed: bool,
) -> rusqlite::Result<Vec<Restore>> {
    let mut rows: BTreeMap<([u8; 32], Txid), Vec<LegacyRow>> = BTreeMap::new();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT id, wallet_id, txid, record_blob, islock_blob, islock_verified, \
             deleted_outputs, released_coins FROM {LEGACY} ORDER BY id"
        ))?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let (wallet, txid): (Vec<u8>, Vec<u8>) = (r.get(1)?, r.get(2)?);
            let (Ok(wallet), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid))
            else {
                tracing::warn!(
                    "store repair: a fork-dropped row's key doesn't read; left as it is"
                );
                continue;
            };
            let row = LegacyRow {
                id: r.get(0)?,
                record_blob: hex::encode(r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default()),
                islock_blob: r.get::<_, Option<Vec<u8>>>(4)?.map(hex::encode),
                islock_verified: r.get(5)?,
                deleted_outputs: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                released_coins: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            };
            rows.entry((wallet, txid)).or_default().push(row);
        }
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut graph = read_graph(tx, max_rows)?;
    let claims: Vec<Claim> = (rows.into_iter())
        .map(|((wallet, txid), legacy_rows)| {
            let decoded = legacy_rows.iter().find_map(|row| {
                let blob = hex::decode(&row.record_blob).ok()?;
                let record = crate::history_ops::decode_record(&blob)
                    .filter(|r| r.txid == txid && r.transaction.txid() == txid)?;
                Some((record, blob))
            });
            Claim {
                wallet,
                txid,
                legacy_rows,
                decoded,
            }
        })
        .collect();
    if let Some(graph) = graph.as_mut() {
        for claim in &claims {
            if claim.legacy_rows.iter().any(|r| r.islock_blob.is_some()) {
                graph.settled.insert(claim.txid);
            }
            for input in claim.decoded.iter().flat_map(|(r, _)| &r.transaction.input) {
                let parent = input.previous_output.txid;
                graph.children.entry(parent).or_default().push(claim.txid);
            }
        }
    }
    let mut kept = Vec::new();
    for claim in claims {
        let found = match &claim.decoded {
            Some((record, _)) => verdict(
                tx,
                graph.as_ref(),
                &claim.wallet,
                record,
                &claim.legacy_rows,
            )?,
            None => {
                tracing::warn!(txid = %claim.txid, "store repair: a fork-dropped record doesn't read; its coins are held again");
                Some((Verdict::Doubt, "its record doesn't read".into()))
            }
        };
        let Some((verdict, reason)) = found else {
            continue;
        };
        tracing::warn!(txid = %claim.txid, ?verdict, reason, "store repair: a claim a fork build dropped is put back");
        kept.push((claim, verdict, reason));
    }
    let mut kept_inputs: HashMap<OutPoint, HashSet<Txid>> = HashMap::new();
    for (claim, _, _) in &kept {
        for input in claim.decoded.iter().flat_map(|(r, _)| &r.transaction.input) {
            (kept_inputs.entry(input.previous_output).or_default()).insert(claim.txid);
        }
    }
    let spenders = Spenders {
        graph: graph.as_ref(),
        indexed,
        kept: &kept_inputs,
    };
    let mut restores = Vec::new();
    for (claim, verdict, reason) in kept {
        let record = RestoreRecord {
            state: State::Staged,
            verdict,
            reason,
            legacy_rows: claim.legacy_rows,
            restored_transaction: false,
            restored_lock: false,
            restored_outputs: Vec::new(),
            held_coins: Vec::new(),
            not_held: Vec::new(),
        };
        restores.extend(planned_restore(
            tx,
            &spenders,
            claim.wallet,
            &claim.txid,
            claim.decoded,
            record,
        )?);
    }
    Ok(restores)
}

/// One claim of the fork's rows: its rows, and the first of their records
/// that reads, with its blob.
struct Claim {
    wallet: [u8; 32],
    txid: Txid,
    legacy_rows: Vec<LegacyRow>,
    decoded: Option<(TransactionRecord, Vec<u8>)>,
}

/// Why `record`, which the fork dropped, is put back, or `None` when these
/// rules drop it too (L2): an input proven unsigned through a push-only or
/// structurally invalid script, for a coin the wallet stores in either
/// state whose stored funding hashes to its outpoint, and nothing of its
/// tree, it included, settled or maybe settled in any wallet. A fork row
/// with a lock keeps it, whatever the lock (L1a).
fn verdict(
    tx: &rusqlite::Transaction<'_>,
    graph: Option<&Graph>,
    wallet: &[u8; 32],
    record: &TransactionRecord,
    legacy_rows: &[LegacyRow],
) -> rusqlite::Result<Option<(Verdict, String)>> {
    let Some(graph) = graph else {
        return Ok(Some((Verdict::Doubt, "the walk can't complete".into())));
    };
    if legacy_rows.iter().any(|r| r.islock_blob.is_some()) {
        return Ok(Some((Verdict::Keep, "it had a lock row".into())));
    }
    if record.block_info().is_some() {
        return Ok(Some((Verdict::Keep, "its record is mined".into())));
    }
    if let Some(at) = settled_in_tree(tx, graph, record.txid)? {
        return Ok(Some((
            Verdict::Keep,
            format!("{at} of its tree is settled"),
        )));
    }
    for index in 0..record.transaction.input.len() {
        if input_cause(tx, wallet, &record.transaction, index)?.is_some() {
            return Ok(None);
        }
    }
    Ok(Some((
        Verdict::Keep,
        "no input is proven unsigned through a push-only or structurally invalid script".into(),
    )))
}

/// What the re-check knows spends an outpoint: the stored transactions and
/// verified locks of every wallet, their input index when the store has
/// one, and the claims it puts back.
struct Spenders<'a> {
    graph: Option<&'a Graph>,
    indexed: bool,
    kept: &'a HashMap<OutPoint, HashSet<Txid>>,
}

/// Whether a transaction stored for any wallet, other than `claim` (which
/// another wallet may store too), spends `outpoint`; with `kept`, a claim
/// put back counts too.
fn spent_elsewhere(
    tx: &rusqlite::Transaction<'_>,
    spenders: &Spenders<'_>,
    outpoint: &OutPoint,
    claim: &Txid,
    kept: bool,
) -> rusqlite::Result<bool> {
    let other = |by: &HashSet<Txid>| by.iter().any(|t| t != claim);
    if (spenders.graph).is_some_and(|g| g.inputs.get(outpoint).is_some_and(other))
        || (kept && spenders.kept.get(outpoint).is_some_and(other))
    {
        return Ok(true);
    }
    if !spenders.indexed {
        return Ok(false);
    }
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM core_transaction_inputs WHERE outpoint = ?1 AND txid <> ?2)",
        params![encode_outpoint(outpoint), claim.as_byte_array().as_slice()],
        |r| r.get(0),
    )
}

/// The put-back of the claim `txid` the fork dropped for `wallet`: its row
/// from the fork's record, its lock row (unverified), its output rows the
/// fork took (none when a coin of it is spent since), and each coin it
/// spends that the wallet holds unspent in one of its own address pools,
/// not a contact's, and that nothing stored spends since (L4). When the
/// store holds the claim again with its record, or a row of it that the
/// record can't fill, only its coins are held and the fork's rows go.
/// `None` when nothing of the claim can be told (see
/// [`held_without_record`]).
fn planned_restore(
    tx: &rusqlite::Transaction<'_>,
    spenders: &Spenders<'_>,
    wallet: [u8; 32],
    txid: &Txid,
    decoded: Option<(TransactionRecord, Vec<u8>)>,
    mut record: RestoreRecord,
) -> rusqlite::Result<Option<Restore>> {
    let key = txid.as_byte_array().as_slice();
    let stored: Option<(bool, bool)> = tx
        .query_row(
            "SELECT record_blob IS NOT NULL, height IS NULL FROM core_transactions \
             WHERE wallet_id = ?1 AND txid = ?2",
            params![wallet.as_slice(), key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut restore = Restore {
        wallet,
        txid: *txid,
        write: Write::Insert,
        blob: Vec::new(),
        block: None,
        inputs: Vec::new(),
        lock: None,
        outputs: Vec::new(),
        held: Vec::new(),
        record: record.clone(),
    };
    // A lock the fork took keeps its claim at every later open (L1a), so
    // it goes back however the claim does.
    if let Some(row) = record.legacy_rows.iter().find(|r| r.islock_blob.is_some()) {
        restore.lock = hex::decode(row.islock_blob.as_deref().unwrap_or_default()).ok();
        record.restored_lock = restore.lock.is_some();
        restore.record.restored_lock = record.restored_lock;
    }
    let Some((decoded, blob)) = decoded else {
        return held_without_record(tx, spenders, txid, restore);
    };
    let coins: Vec<OutPoint> = (decoded.transaction.input.iter())
        .map(|i| i.previous_output)
        .collect();
    restore.write = match stored {
        None => Write::Insert,
        // A row with no record, of a transaction not mined: the record
        // fills it, as the store's own writes do.
        Some((false, true)) if decoded.block_info().is_none() => Write::Fill,
        Some(_) => {
            tracing::info!(%txid, "store repair: the store holds a fork-dropped claim again; its coins are held and its fork rows go");
            hold(
                tx,
                spenders,
                wallet,
                txid,
                &coins,
                &mut record,
                &mut restore.held,
            )?;
            restore.write = Write::Nothing;
            restore.record = record;
            return Ok(Some(restore));
        }
    };
    restore.blob = blob;
    restore.block = decoded.block_info().map(|b| {
        (
            i64::from(b.height()),
            AsRef::<[u8]>::as_ref(&b.block_hash()).to_vec(),
            i64::from(b.timestamp()),
        )
    });
    if spenders.indexed {
        restore.inputs = coins.iter().map(encode_outpoint).collect();
    }
    hold(
        tx,
        spenders,
        wallet,
        txid,
        &coins,
        &mut record,
        &mut restore.held,
    )?;
    // A claim whose coin something stored spends since conflicts with
    // that spend: its outputs stay out, so nothing is offered on them.
    if record.not_held.is_empty() {
        let mut seen = HashSet::new();
        for legacy in &record.legacy_rows {
            let outputs: Vec<serde_json::Value> = serde_json::from_str(&legacy.deleted_outputs)
                .unwrap_or_else(|e| {
                    tracing::warn!(%txid, error = %e, "store repair: a fork-dropped claim's output rows don't read; not put back");
                    Vec::new()
                });
            for row in outputs {
                let field = |name: &str| {
                    row.get(name)
                        .and_then(|v| v.as_str())
                        .and_then(|h| hex::decode(h).ok())
                };
                let (Some(op), Some(value), Some(script)) = (
                    field("outpoint"),
                    row.get("value").and_then(|v| v.as_i64()),
                    field("script"),
                ) else {
                    tracing::warn!(%txid, "store repair: a fork-dropped output row doesn't read; not put back");
                    continue;
                };
                let Some(o) = decode_outpoint(&op).filter(|o| o.txid == *txid) else {
                    tracing::warn!(%txid, "store repair: a fork-dropped output row names another transaction; not put back");
                    continue;
                };
                if !seen.insert(o) {
                    continue;
                }
                let present: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2)",
                    params![wallet.as_slice(), &op],
                    |r| r.get(0),
                )?;
                if present {
                    continue;
                }
                let spent = spent_elsewhere(tx, spenders, &o, txid, true)?;
                record.restored_outputs.push(hex::encode(&op));
                restore.outputs.push((op, value, script, i64::from(spent)));
            }
        }
    }
    restore.record = RestoreRecord {
        restored_transaction: true,
        ..record
    };
    Ok(Some(restore))
}

/// The put-back of a claim whose record doesn't read: the claim can't go
/// back (the store reads its records strictly), so only the coins the
/// fork released for it (its `released_coins`) are held again, and its
/// fork rows go. `None` when those don't read either: nothing is changed
/// and the rows stay.
fn held_without_record(
    tx: &rusqlite::Transaction<'_>,
    spenders: &Spenders<'_>,
    txid: &Txid,
    mut restore: Restore,
) -> rusqlite::Result<Option<Restore>> {
    let mut record = restore.record.clone();
    let mut coins = Vec::new();
    for row in &record.legacy_rows {
        let parsed: Option<Vec<serde_json::Value>> = serde_json::from_str(&row.released_coins).ok();
        let Some(outpoints) = parsed.and_then(|rows| {
            (rows.iter())
                .map(|r| {
                    r.get("outpoint")
                        .and_then(|v| v.as_str())
                        .and_then(|h| hex::decode(h).ok())
                        .and_then(|b| decode_outpoint(&b))
                })
                .collect::<Option<Vec<OutPoint>>>()
        }) else {
            tracing::warn!(%txid, "store repair: a fork-dropped claim's released coins don't read either; its rows stay");
            return Ok(None);
        };
        coins.extend(outpoints);
    }
    let wallet = restore.wallet;
    hold(
        tx,
        spenders,
        wallet,
        txid,
        &coins,
        &mut record,
        &mut restore.held,
    )?;
    restore.write = Write::Nothing;
    restore.record = record;
    Ok(Some(restore))
}

/// Of `coins`, those `wallet` holds unspent in one of its own address
/// pools, not a contact's, and that nothing stored spends since (L4), go
/// to `held`; those spent since are noted in `record`.
fn hold(
    tx: &rusqlite::Transaction<'_>,
    spenders: &Spenders<'_>,
    wallet: [u8; 32],
    txid: &Txid,
    coins: &[OutPoint],
    record: &mut RestoreRecord,
    held: &mut Vec<Vec<u8>>,
) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare_cached(
        "SELECT u.spent, u.is_sweep_placeholder = 0 AND EXISTS (SELECT 1 FROM core_address_pool p \
            WHERE p.wallet_id = u.wallet_id AND p.script = u.script AND p.account_type <> ?3) \
         FROM core_utxos u WHERE u.wallet_id = ?1 AND u.outpoint = ?2",
    )?;
    let mut seen = HashSet::new();
    for o in coins {
        if !seen.insert(*o) {
            continue;
        }
        let op = encode_outpoint(o);
        let Some((spent, own)): Option<(bool, bool)> = stmt
            .query_row(params![wallet.as_slice(), &op, CONTACT_POOL], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
        else {
            continue;
        };
        if !own {
            continue;
        }
        if spent || spent_elsewhere(tx, spenders, o, txid, false)? {
            tracing::warn!(%txid, coin = %o, "store repair: a coin a fork-dropped claim spends is spent; not held again");
            record.not_held.push(hex::encode(&op));
            continue;
        }
        record.held_coins.push(hex::encode(&op));
        held.push(op);
    }
    Ok(())
}

/// Writes `restores` to the store, inside its transaction (C3 L3).
fn restore(tx: &rusqlite::Transaction<'_>, restores: &[Restore]) -> rusqlite::Result<()> {
    for r in restores {
        let (wallet, key) = (r.wallet.as_slice(), r.txid.as_byte_array().as_slice());
        if r.write != Write::Nothing {
            let (height, hash, time) = match &r.block {
                Some((h, b, t)) => (Some(*h), Some(b.as_slice()), Some(*t)),
                None => (None, None, None),
            };
            if r.write == Write::Insert {
                tx.execute(
                    "INSERT INTO core_transactions \
                        (wallet_id, txid, height, block_hash, block_time, finalized, record_blob) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![wallet, key, height, hash, time, r.block.is_some(), &r.blob],
                )?;
            } else {
                tx.execute(
                    "UPDATE core_transactions SET record_blob = ?3 \
                     WHERE wallet_id = ?1 AND txid = ?2 AND record_blob IS NULL AND height IS NULL",
                    params![wallet, key, &r.blob],
                )?;
            }
            for op in &r.inputs {
                tx.execute(
                    "INSERT OR IGNORE INTO core_transaction_inputs (wallet_id, txid, outpoint) \
                     VALUES (?1, ?2, ?3)",
                    params![wallet, key, op],
                )?;
            }
            for (op, value, script, spent) in &r.outputs {
                tx.execute(
                    "INSERT OR IGNORE INTO core_utxos (wallet_id, outpoint, value, script, spent) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![wallet, op, value, script, spent],
                )?;
            }
        }
        // The store reads a verified lock row strictly and makes its
        // transaction final; a lock the fork took goes back unverified.
        if let Some(lock) = &r.lock {
            tx.execute(
                "INSERT OR IGNORE INTO core_instant_locks (wallet_id, txid, islock_blob, verified) \
                 VALUES (?1, ?2, ?3, 0)",
                params![wallet, key, lock],
            )?;
        }
        for op in &r.held {
            tx.execute(
                "UPDATE core_utxos SET spent = 1, spent_in_txid = NULL \
                 WHERE wallet_id = ?1 AND outpoint = ?2 AND spent = 0",
                params![wallet, op],
            )?;
        }
        tx.execute(
            &format!("DELETE FROM {LEGACY} WHERE wallet_id = ?1 AND txid = ?2"),
            params![wallet, key],
        )?;
    }
    bump_core(tx, restores.iter().map(|r| r.wallet))
}

pub(crate) fn record_key(txid: &Txid) -> String {
    format!("{STORE_DROPPED_PREFIX}{txid}")
}

pub(crate) fn restore_key(txid: &Txid) -> String {
    format!("{STORE_RESTORED_PREFIX}{txid}")
}

/// Stages `drops` and `restores` durably in one write: each wallet's
/// record, staged, and the lists of them.
fn stage(appdb: &AppDb, drops: &[Planned], restores: &[Restore]) -> Result<(), EngineError> {
    let mut dropped = read_staged(appdb, STAGED_KEY)?;
    dropped.extend((drops.iter()).map(|d| (WalletId(d.wallet).to_string(), d.txid.to_string())));
    let mut restored = read_staged(appdb, STAGED_RESTORED_KEY)?;
    restored
        .extend((restores.iter()).map(|r| (WalletId(r.wallet).to_string(), r.txid.to_string())));
    let lists = [
        serde_json::to_string(&dropped).map_err(storage)?,
        serde_json::to_string(&restored).map_err(storage)?,
    ];
    write_records(appdb, drops, restores, State::Staged, Some(&lists))
}

/// Marks `drops` and `restores` done and empties the lists, in one write.
fn promote(appdb: &AppDb, drops: &[Planned], restores: &[Restore]) -> Result<(), EngineError> {
    write_records(appdb, drops, restores, State::Done, None)
}

/// Writes the records of `drops` and `restores` at `state`, and the staged
/// lists as `lists`, in one durable write.
fn write_records(
    appdb: &AppDb,
    drops: &[Planned],
    restores: &[Restore],
    state: State,
    lists: Option<&[String; 2]>,
) -> Result<(), EngineError> {
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for d in drops {
        let record = DropRecord {
            state,
            ..d.record.clone()
        };
        let value = serde_json::to_string(&record).map_err(storage)?;
        rows.push((WalletId(d.wallet).to_string(), record_key(&d.txid), value));
    }
    for r in restores {
        let record = RestoreRecord {
            state,
            ..r.record.clone()
        };
        let value = serde_json::to_string(&record).map_err(storage)?;
        rows.push((WalletId(r.wallet).to_string(), restore_key(&r.txid), value));
    }
    let mut writes: Vec<(&str, &str, Option<&str>)> = (rows.iter())
        .map(|(s, k, v)| (s.as_str(), k.as_str(), Some(v.as_str())))
        .collect();
    writes.push((STAGED_SCOPE, STAGED_KEY, lists.map(|l| l[0].as_str())));
    writes.push((
        STAGED_SCOPE,
        STAGED_RESTORED_KEY,
        lists.map(|l| l[1].as_str()),
    ));
    appdb.set_scoped_settings_durable(&writes).map_err(storage)
}

/// A staged list: `(wallet hex, txid)`.
fn read_staged(appdb: &AppDb, key: &str) -> Result<Vec<(String, String)>, EngineError> {
    match appdb.setting_lossy(STAGED_SCOPE, key).map_err(storage)? {
        None => Ok(Vec::new()),
        Some(list) => serde_json::from_str(&list).map_err(storage),
    }
}

/// Settles what an earlier repair staged. A drop committed once the store
/// no longer holds its claim's row; a put-back once the fork's rows of its
/// claim are gone. A committed record is done; any other was rolled back,
/// so its record goes.
fn resolve_staged(
    conn: &rusqlite::Connection,
    appdb: &AppDb,
    legacy: bool,
) -> Result<(), EngineError> {
    let dropped = read_staged(appdb, STAGED_KEY)?;
    let restored = read_staged(appdb, STAGED_RESTORED_KEY)?;
    if dropped.is_empty() && restored.is_empty() {
        return Ok(());
    }
    let exists = |sql: &str, id: &[u8], t: &Txid| -> Result<bool, EngineError> {
        conn.query_row(sql, params![id, t.as_byte_array().as_slice()], |r| r.get(0))
            .map_err(storage)
    };
    let mut writes: Vec<(String, String, Option<String>)> = Vec::new();
    for (list, put_back) in [(&dropped, false), (&restored, true)] {
        for (wallet, txid) in list {
            let (Ok(id), Ok(t)) = (hex::decode(wallet), Txid::from_str(txid)) else {
                continue;
            };
            let (committed, key) = match put_back {
                false => (
                    !exists(
                        "SELECT EXISTS (SELECT 1 FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2)",
                        &id,
                        &t,
                    )?,
                    record_key(&t),
                ),
                true => (
                    !legacy
                        || !exists(
                            &format!(
                                "SELECT EXISTS (SELECT 1 FROM {LEGACY} WHERE wallet_id = ?1 AND txid = ?2)"
                            ),
                            &id,
                            &t,
                        )?,
                    restore_key(&t),
                ),
            };
            let value = match committed {
                false => None,
                true => appdb
                    .setting_lossy(wallet, &key)
                    .map_err(storage)?
                    .map(|v| match serde_json::from_str::<serde_json::Value>(&v) {
                        Ok(mut record) => {
                            if let Some(fields) = record.as_object_mut() {
                                fields.insert("state".into(), "done".into());
                            }
                            record.to_string()
                        }
                        Err(_) => v,
                    }),
            };
            tracing::info!(%wallet, %txid, put_back, committed, "store repair: a staged record resolved");
            writes.push((wallet.clone(), key, value));
        }
    }
    let mut refs: Vec<(&str, &str, Option<&str>)> = (writes.iter())
        .map(|(s, k, v)| (s.as_str(), k.as_str(), v.as_deref()))
        .collect();
    refs.push((STAGED_SCOPE, STAGED_KEY, None));
    refs.push((STAGED_SCOPE, STAGED_RESTORED_KEY, None));
    appdb.set_scoped_settings_durable(&refs).map_err(storage)
}

/// The claims of `wallet` this repair dropped from the store, and of those
/// the ones whose verdict may be wrong: a structural drop (S4), or a
/// record that doesn't read. A staged record counts only
/// while the store doesn't hold its claim (`held`). Only the read itself
/// fails.
pub(crate) fn store_dropped(
    appdb: &AppDb,
    wallet: WalletId,
    held: impl Fn(&Txid) -> bool,
) -> Result<(HashSet<Txid>, HashSet<Txid>), dw_appdb::AppDbError> {
    let rows = appdb.settings_with_prefix_lossy(&wallet.to_string(), STORE_DROPPED_PREFIX)?;
    let (mut dropped, mut doubtful) = (HashSet::new(), HashSet::new());
    for (key, value) in rows {
        let Ok(txid) = Txid::from_str(&key[STORE_DROPPED_PREFIX.len()..]) else {
            tracing::warn!(%wallet, %key, "a store-dropped record names no transaction");
            continue;
        };
        let record = value.and_then(|v| serde_json::from_str::<DropRecord>(&v).ok());
        if record.as_ref().is_some_and(|r| r.state == State::Staged) && held(&txid) {
            continue;
        }
        let tx = (record.as_ref())
            .and_then(|r| hex::decode(&r.record_blob).ok())
            .and_then(|b| crate::history_ops::decode_record(&b))
            .filter(|r| r.txid == txid && r.transaction.txid() == txid);
        // S3 makes a push-only unsigned input sure whatever the others
        // are; only a structural drop (S4) or an unreadable record is
        // doubtful.
        let sure = tx.is_some() && record.as_ref().is_some_and(|r| r.cause == Cause::Unsigned);
        dropped.insert(txid);
        if !sure {
            doubtful.insert(txid);
        }
    }
    Ok((dropped, doubtful))
}

#[cfg(test)]
#[path = "store_repair_unit_tests.rs"]
mod tests;
