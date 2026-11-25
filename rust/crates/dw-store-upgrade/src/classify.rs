//! The refusal check: what a wallet.sqlite is, read without writing.

use std::collections::BTreeSet;

use rusqlite::Connection;

use crate::adopters::{Adopters, P4_NAME, P9aStatus};
use crate::error::{Result, StoreUpgradeError};
use crate::fork::{self, KNOWN};
use crate::sql::{count_rows, has_column, has_table, map_sqlite, normalize_sql, objects_on};

#[derive(Debug)]
pub(crate) enum Classification {
    NotForked,
    Fork(ForkState),
    Refuse(StoreUpgradeError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForkState {
    pub versions: BTreeSet<u32>,
    /// `contacts.external_account_reference` still present (R1 drop pending).
    pub column_present: bool,
    /// Refinery history equals every embedded migration: upstream migrated it.
    pub history_complete: bool,
}

impl ForkState {
    /// Whether this start's upgrade writes the file: R1's drop, or upstream's
    /// pending migrations.
    pub fn upgrade_writes(&self) -> bool {
        self.column_present || !self.history_complete
    }
}

const REFINERY: &str = "refinery_schema_history";

struct HistoryRow {
    version: i64,
    name: String,
    checksum: String,
}

fn read_history(conn: &Connection, table: &str) -> Result<Vec<HistoryRow>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT version, name, checksum FROM \"{table}\" ORDER BY version"
        ))
        .map_err(map_sqlite("read history"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(HistoryRow {
                version: r.get(0)?,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                checksum: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map_err(map_sqlite("read history"))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite("read history"))
}

fn mismatch(object: impl Into<String>) -> Classification {
    Classification::Refuse(StoreUpgradeError::ForkStoreShapeMismatch {
        object: object.into(),
    })
}

/// Classify `conn` for this build. Reads only. Every refusal the reconciler
/// can meet is decided here, so a refusal never follows a write.
pub(crate) fn classify(conn: &Connection, adopters: &Adopters) -> Result<Classification> {
    if let P9aStatus::Broken { detail } = &adopters.p9a {
        return Ok(Classification::Refuse(StoreUpgradeError::P9aBroken {
            detail: detail.clone(),
        }));
    }
    let q = map_sqlite("read schema");
    let has_fork = has_table(conn, fork::FORK_HISTORY).map_err(&q)?;
    let has_retired_history = has_table(conn, fork::FORK_HISTORY_RETIRED).map_err(&q)?;
    if !has_table(conn, REFINERY).map_err(&q)? {
        return Ok(if has_fork || has_retired_history {
            mismatch("refinery_schema_history (missing beside the fork history)")
        } else {
            Classification::NotForked
        });
    }
    let history = read_history(conn, REFINERY)?;
    if history
        .iter()
        .any(|r| r.version == 19 && fork::PRE_HISTORY_V19_NAMES.contains(&r.name.as_str()))
    {
        return Ok(Classification::Refuse(StoreUpgradeError::PreHistoryFork));
    }
    let found = history.iter().map(|r| r.version).max().unwrap_or(0);
    let supported = adopters.max_version();
    if found > supported {
        return Ok(Classification::Refuse(StoreUpgradeError::SchemaTooNew {
            found,
            supported,
        }));
    }
    if !has_fork {
        return Ok(Classification::NotForked);
    }
    if has_retired_history {
        return Ok(mismatch(fork::FORK_HISTORY_RETIRED));
    }
    let history_table = objects_on(conn, fork::FORK_HISTORY).map_err(&q)?;
    let want_history = vec![
        (
            "index".to_string(),
            format!("sqlite_autoindex_{}_1", fork::FORK_HISTORY),
            None,
        ),
        (
            "table".to_string(),
            fork::FORK_HISTORY.to_string(),
            Some(normalize_sql(fork::FORK_HISTORY_SQL)),
        ),
    ];
    if history_table != want_history {
        return Ok(mismatch(fork::FORK_HISTORY));
    }
    let fork_rows = read_history(conn, fork::FORK_HISTORY)?;
    let versions: BTreeSet<u32> = fork_rows
        .iter()
        .map(|r| u32::try_from(r.version).unwrap_or(u32::MAX))
        .collect();
    if versions.len() != fork_rows.len() {
        return Ok(mismatch(format!(
            "{} (duplicate versions)",
            fork::FORK_HISTORY
        )));
    }
    if !version_set_accepted(&versions, adopters) {
        return Ok(Classification::Refuse(
            StoreUpgradeError::ForkStoreUnsupported { versions },
        ));
    }
    for row in &fork_rows {
        let known = KNOWN.iter().find(|k| i64::from(k.version) == row.version);
        if !known.is_some_and(|k| k.name == row.name && k.checksum == row.checksum) {
            return Ok(mismatch(format!(
                "{} version {}",
                fork::FORK_HISTORY,
                row.version
            )));
        }
    }
    if let Some(refusal) = history_mismatch(&history, adopters) {
        return Ok(Classification::Refuse(refusal));
    }
    let reference = crate::shapes::Reference::build(adopters)?;
    let applied: BTreeSet<&str> = history.iter().map(|r| r.name.as_str()).collect();
    if let Some(object) = crate::shapes::check(conn, &versions, &reference, &applied)? {
        return Ok(mismatch(object));
    }
    let retired_rows = count_rows(conn, fork::RETIRED_POOL).map_err(&q)?;
    if retired_rows > 0 && adopters.p9a != P9aStatus::Present {
        return Ok(Classification::Refuse(
            StoreUpgradeError::RetiredPoolWithoutAdopter { rows: retired_rows },
        ));
    }
    let column_present = has_column(conn, fork::CONTACTS, fork::EXTERNAL_REF).map_err(&q)?;
    let history_complete = history.len() == adopters.embedded.len();
    Ok(Classification::Fork(ForkState {
        versions,
        column_present,
        history_complete,
    }))
}

/// Applied refinery rows must be exactly a prefix `1..=n` of this build's
/// embedded migrations, names and checksums included.
fn history_mismatch(history: &[HistoryRow], adopters: &Adopters) -> Option<StoreUpgradeError> {
    for (i, row) in history.iter().enumerate() {
        let detail = match adopters.embedded.get(i) {
            Some(e) if e.version != row.version => format!("expected version {}", e.version),
            Some(e) if e.name != row.name => format!("name {} is {} here", row.name, e.name),
            Some(e) if e.checksum != row.checksum => "checksum differs from this build's".into(),
            Some(_) => continue,
            None => "not embedded in this build".into(),
        };
        return Some(StoreUpgradeError::SchemaHistoryMismatch {
            version: row.version,
            detail,
        });
    }
    None
}

/// Q78 R4: the published sets, each version's adopter present in this build.
/// Fork V1: its column is dropped (R1) and its table kept, refused with rows
/// unless P9a is Present (checked after the shape). Fork V2 needs P4.
/// Fork V3: accepted without P8 (Q93 T1-T4): only the exact published V3
/// (history name, checksum, shape), and V3's table is kept as found, never
/// written, until P8 adopts its rows. Fork V4/V5:
/// refused until their adopters and exact shapes are compiled in
/// (FIXTURES-TODO.md).
pub(crate) fn version_set_accepted(versions: &BTreeSet<u32>, adopters: &Adopters) -> bool {
    if !fork::published_sets().contains(versions) || versions.iter().any(|v| *v >= 4) {
        return false;
    }
    !versions.contains(&2) || p4_present(adopters)
}

/// P4 adopts fork V2: embedded by name, and the fresh schema's
/// `core_instant_locks.verified` is the fork's exact column.
pub(crate) fn p4_present(adopters: &Adopters) -> bool {
    adopters.has_migration(P4_NAME)
        && crate::shapes::Reference::build(adopters)
            .is_ok_and(|r| r.fresh_instant_locks_has_fork_verified())
}
