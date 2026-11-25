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
//! or when that can't be told (S2), and when it is mined or locked (S6).
//!
//! The store's schema is read only at the versions this build was checked
//! against ([`KNOWN_SCHEMA`]); any other repairs nothing. The repair runs in
//! one write transaction, and its records are staged in the app database
//! before that transaction commits: a staged record counts only once its
//! claim's row is gone from the store, which is the store transaction's
//! commit, so an exit anywhere leaves the two agreeing.

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

/// Each table and column the repair reads or writes.
const COLUMNS: &[(&str, &[&str])] = &[
    (
        "core_transactions",
        &["wallet_id", "txid", "height", "finalized", "record_blob"],
    ),
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

/// A test's stop inside the repair.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
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
    let Some(schema) = known_schema(&conn).map_err(storage)? else {
        return Ok(Outcome::default());
    };
    let mut outcome = Outcome {
        schema: Some(schema),
        ..Default::default()
    };
    resolve_staged(&conn, appdb)?;

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(storage)?;
    let drops = match plan(&tx, row_limit(network_dir)).map_err(storage)? {
        Some(drops) if !drops.is_empty() => drops,
        _ => return Ok(outcome),
    };
    apply(&tx, &drops).map_err(storage)?;
    stage(appdb, &drops)?;
    #[cfg(test)]
    if stop_here(network_dir, Stop::BeforeCommit) {
        return Err(storage("test: stopped before the commit"));
    }
    tx.commit().map_err(storage)?;
    for d in &drops {
        (outcome.dropped.entry(WalletId(d.wallet)).or_default()).push(d.txid);
    }
    tracing::warn!(dropped = ?outcome.dropped, "store repair: dropped unconfirmed transactions that spend a coin without its owner's signature");
    #[cfg(test)]
    if stop_here(network_dir, Stop::BeforePromote) {
        return Err(storage("test: stopped before the records are done"));
    }
    promote(appdb, &drops)?;
    Ok(outcome)
}

/// The store's schema version when this build was checked against it and
/// every table and column the repair uses is there; else `None` (logged).
fn known_schema(conn: &rusqlite::Connection) -> rusqlite::Result<Option<i64>> {
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
    let Some(version) = version.filter(|v| KNOWN_SCHEMA.contains(v)) else {
        tracing::warn!(?version, known = ?KNOWN_SCHEMA, "store repair: an unknown store schema; nothing repaired");
        return Ok(None);
    };
    for (table, columns) in COLUMNS {
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
    Ok(Some(version))
}

/// What [`plan`] reads of the whole file: which transactions are settled,
/// what spends each transaction's outputs, and the pending claims.
#[derive(Default)]
struct Graph {
    settled: HashSet<Txid>,
    children: HashMap<Txid, Vec<Txid>>,
    inputs: HashSet<OutPoint>,
    pending: Vec<([u8; 32], TransactionRecord)>,
    unverified_locks: HashSet<([u8; 32], Txid)>,
}

/// Reads every wallet's transactions and InstantSend locks; `None` when the
/// walk could not complete (a row that doesn't decode, a key of the wrong
/// size, or more than `max_rows` rows), logged.
fn read_graph(tx: &rusqlite::Transaction<'_>, max_rows: usize) -> rusqlite::Result<Option<Graph>> {
    let limit = i64::try_from(platform_wallet_storage::SIZE_LIMIT_BYTES).unwrap_or(i64::MAX);
    let rows: i64 = tx.query_row(
        "SELECT (SELECT count(*) FROM core_transactions) + (SELECT count(*) FROM core_instant_locks)",
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
            graph.inputs.insert(parent);
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
        let (Ok(wallet), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid)) else {
            return incomplete("a lock key");
        };
        if !verified {
            graph.unverified_locks.insert((wallet, txid));
            continue;
        }
        let Some(lock) = blob.as_deref().and_then(crate::history_ops::decode_lock) else {
            return incomplete("an InstantSend lock");
        };
        // The store ignores a lock row naming another transaction.
        if lock.txid != txid {
            continue;
        }
        graph.settled.insert(txid);
        for parent in &lock.inputs {
            graph.children.entry(parent.txid).or_default().push(txid);
            graph.inputs.insert(*parent);
        }
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
    let config =
        bincode::config::standard().with_limit::<{ platform_wallet_storage::SIZE_LIMIT_BYTES }>();
    match bincode::serde::decode_from_slice(bytes, config) {
        Ok((outpoint, used)) if used == bytes.len() => Some(outpoint),
        _ => None,
    }
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
                decode_outpoint(&key).is_some_and(|o| o.txid == t && graph.inputs.contains(&o));
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
        let mut cause = None;
        for index in 0..record.transaction.input.len() {
            if let Some(c) = input_cause(tx, wallet, &record.transaction, index)? {
                cause = Some(c);
                break;
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
    let mut by_wallet: BTreeMap<[u8; 32], Vec<(&TransactionRecord, Cause)>> = BTreeMap::new();
    for (wallet, record, cause) in claims {
        by_wallet.entry(wallet).or_default().push((record, cause));
    }
    let mut drops = Vec::new();
    for (wallet, records) in by_wallet {
        let dropping: HashSet<Txid> = records.iter().map(|(r, _)| r.txid).collect();
        let Some(claimed) = claimed_by_others(tx, &wallet, &dropping)? else {
            return Ok(None);
        };
        for (record, cause) in records {
            drops.push(drop_of(tx, &wallet, record, cause, &claimed)?);
        }
    }
    Ok(Some(drops))
}

/// Every outpoint a stored transaction of `wallet` other than `except`
/// spends, whatever its finality; `None` when a record doesn't read.
fn claimed_by_others(
    tx: &rusqlite::Transaction<'_>,
    wallet: &[u8; 32],
    except: &HashSet<Txid>,
) -> rusqlite::Result<Option<HashSet<OutPoint>>> {
    let mut stmt = tx.prepare(
        "SELECT txid, record_blob FROM core_transactions \
         WHERE wallet_id = ?1 AND record_blob IS NOT NULL",
    )?;
    let mut q = stmt.query([wallet.as_slice()])?;
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
    let wallets: HashSet<[u8; 32]> = drops.iter().map(|d| d.wallet).collect();
    for wallet in wallets {
        tx.execute(
            "INSERT INTO meta_data_versions (wallet_id, domain, seq) VALUES (?1, 'core', 1) \
             ON CONFLICT(wallet_id, domain) DO UPDATE SET \
                seq = CASE WHEN seq >= 9223372036854775807 THEN seq ELSE seq + 1 END",
            [wallet.as_slice()],
        )?;
    }
    Ok(())
}

fn record_key(txid: &Txid) -> String {
    format!("{STORE_DROPPED_PREFIX}{txid}")
}

/// Stages `drops` durably in one write: each wallet's record, staged, and
/// the list of them.
fn stage(appdb: &AppDb, drops: &[Planned]) -> Result<(), EngineError> {
    let mut staged: Vec<(String, String)> = read_staged(appdb)?;
    staged.extend(
        drops
            .iter()
            .map(|d| (hex::encode(d.wallet), d.txid.to_string())),
    );
    let list = serde_json::to_string(&staged).map_err(storage)?;
    let rows: Vec<(String, String, String)> = (drops.iter())
        .map(|d| {
            let value = serde_json::to_string(&d.record).map_err(storage)?;
            Ok((hex::encode(d.wallet), record_key(&d.txid), value))
        })
        .collect::<Result<_, EngineError>>()?;
    let mut writes: Vec<(&str, &str, Option<&str>)> = (rows.iter())
        .map(|(s, k, v)| (s.as_str(), k.as_str(), Some(v.as_str())))
        .collect();
    writes.push((STAGED_SCOPE, STAGED_KEY, Some(&list)));
    appdb.set_scoped_settings_durable(&writes).map_err(storage)
}

/// Marks `drops` done and empties the staged list, in one write.
fn promote(appdb: &AppDb, drops: &[Planned]) -> Result<(), EngineError> {
    let rows: Vec<(String, String, String)> = (drops.iter())
        .map(|d| {
            let record = DropRecord {
                state: State::Done,
                ..d.record.clone()
            };
            let value = serde_json::to_string(&record).map_err(storage)?;
            Ok((hex::encode(d.wallet), record_key(&d.txid), value))
        })
        .collect::<Result<_, EngineError>>()?;
    let mut writes: Vec<(&str, &str, Option<&str>)> = (rows.iter())
        .map(|(s, k, v)| (s.as_str(), k.as_str(), Some(v.as_str())))
        .collect();
    writes.push((STAGED_SCOPE, STAGED_KEY, None));
    appdb.set_scoped_settings_durable(&writes).map_err(storage)
}

/// The staged list: `(wallet hex, txid)`.
fn read_staged(appdb: &AppDb) -> Result<Vec<(String, String)>, EngineError> {
    match appdb
        .setting_lossy(STAGED_SCOPE, STAGED_KEY)
        .map_err(storage)?
    {
        None => Ok(Vec::new()),
        Some(list) => serde_json::from_str(&list).map_err(storage),
    }
}

/// Settles what an earlier repair staged: a claim whose row the store no
/// longer holds was dropped by a commit, so its record is done; one it
/// still holds was rolled back, so its record goes.
fn resolve_staged(conn: &rusqlite::Connection, appdb: &AppDb) -> Result<(), EngineError> {
    let staged = read_staged(appdb)?;
    if staged.is_empty() {
        return Ok(());
    }
    let mut writes: Vec<(String, String, Option<String>)> = Vec::new();
    for (wallet, txid) in &staged {
        let (Ok(id), Ok(t)) = (hex::decode(wallet), Txid::from_str(txid)) else {
            continue;
        };
        let held: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2)",
                params![id, t.as_byte_array().as_slice()],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let key = record_key(&t);
        let value = match held {
            true => None,
            false => appdb
                .setting_lossy(wallet, &key)
                .map_err(storage)?
                .map(|v| match serde_json::from_str::<DropRecord>(&v) {
                    Ok(record) => serde_json::to_string(&DropRecord {
                        state: State::Done,
                        ..record
                    })
                    .unwrap_or(v),
                    Err(_) => v,
                }),
        };
        tracing::info!(%wallet, %txid, committed = !held, "store repair: a staged record resolved");
        writes.push((wallet.clone(), key, value));
    }
    let mut refs: Vec<(&str, &str, Option<&str>)> = (writes.iter())
        .map(|(s, k, v)| (s.as_str(), k.as_str(), v.as_deref()))
        .collect();
    refs.push((STAGED_SCOPE, STAGED_KEY, None));
    appdb.set_scoped_settings_durable(&refs).map_err(storage)
}

/// The claims of `wallet` this repair dropped from the store, and of those
/// the ones whose verdict may be wrong: a record whose input scripts
/// aren't all push-only, or that doesn't read. A staged record counts only
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
        let sure = tx.is_some_and(|r| {
            (r.transaction.input.iter()).all(|i| crate::tx_actions::push_only(&i.script_sig))
        });
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
