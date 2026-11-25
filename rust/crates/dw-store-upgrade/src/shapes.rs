//! Exact shape checks for every fork object (Q78 R1a, N1): the fork's
//! statements applied to this build's fresh tables in memory are the
//! reference, compared as normalized `sqlite_master` rows (columns, types,
//! NOT NULL, defaults, CHECKs, keys, FKs, indexes, triggers).

use std::collections::BTreeSet;

use rusqlite::Connection;

use crate::adopters::Adopters;
use crate::error::Result;
use crate::fork;
use crate::sql::{
    SchemaObject, has_column, has_table, map_sqlite, normalize_sql, objects_on, table_sql,
};

pub(crate) struct Reference {
    retired_pool: Vec<SchemaObject>,
    /// This build's own `core_address_pool_retired` (all objects), when its
    /// fresh schema has one.
    fresh_retired_pool: Option<Vec<SchemaObject>>,
    dropped_spends: Vec<SchemaObject>,
    contacts_with_column: Option<String>,
    contacts_fresh: Option<String>,
    contacts_others: Vec<SchemaObject>,
    instant_locks_v2: Option<String>,
    instant_locks_fresh: Option<String>,
    instant_locks_others: Vec<SchemaObject>,
}

fn fresh_others(adopters: &Adopters, table: &str) -> Vec<SchemaObject> {
    let mut v: Vec<SchemaObject> = adopters
        .fresh
        .iter()
        .filter(|o| o.table == table && o.kind != "table")
        .map(|o| {
            (
                o.kind.clone(),
                o.name.clone(),
                o.sql.as_deref().map(normalize_sql),
            )
        })
        .collect();
    v.sort();
    v
}

fn fresh_objects(adopters: &Adopters, table: &str) -> Option<Vec<SchemaObject>> {
    adopters.fresh_table_sql(table)?;
    let mut v: Vec<SchemaObject> = adopters
        .fresh
        .iter()
        .filter(|o| o.table == table)
        .map(|o| {
            (
                o.kind.clone(),
                o.name.clone(),
                o.sql.as_deref().map(normalize_sql),
            )
        })
        .collect();
    v.sort();
    Some(v)
}

fn without_table(objects: Vec<SchemaObject>) -> Vec<SchemaObject> {
    objects.into_iter().filter(|o| o.0 != "table").collect()
}

impl Reference {
    pub(crate) fn build(adopters: &Adopters) -> Result<Self> {
        let q = map_sqlite("build the fork shape reference");
        let mem = Connection::open_in_memory().map_err(&q)?;
        let contacts_fresh = adopters.fresh_table_sql(fork::CONTACTS).map(str::to_owned);
        let instant_fresh = adopters
            .fresh_table_sql(fork::INSTANT_LOCKS)
            .map(str::to_owned);
        let mut contacts_with_column = None;
        if let Some(sql) = &contacts_fresh {
            mem.execute_batch(sql).map_err(&q)?;
            if !has_column(&mem, fork::CONTACTS, fork::EXTERNAL_REF).map_err(&q)? {
                mem.execute_batch(fork::V1_SQL).map_err(&q)?;
                contacts_with_column = table_sql(&mem, fork::CONTACTS).map_err(&q)?;
            }
        }
        if !has_table(&mem, fork::RETIRED_POOL).map_err(&q)? {
            let tables_only = fork::V1_SQL
                .split(';')
                .filter(|s| !s.trim().is_empty() && !s.contains("ALTER TABLE"))
                .map(|s| format!("{s};"))
                .collect::<String>();
            mem.execute_batch(&tables_only).map_err(&q)?;
        }
        let mut instant_locks_v2 = None;
        if let Some(sql) = &instant_fresh {
            mem.execute_batch(sql).map_err(&q)?;
            if has_column(&mem, fork::INSTANT_LOCKS, fork::VERIFIED).map_err(&q)? {
                mem.execute_batch("ALTER TABLE core_instant_locks DROP COLUMN verified")
                    .map_err(&q)?;
            }
            mem.execute_batch(fork::V2_SQL).map_err(&q)?;
            instant_locks_v2 = table_sql(&mem, fork::INSTANT_LOCKS).map_err(&q)?;
        }
        mem.execute_batch(fork::V3_SQL).map_err(&q)?;
        Ok(Self {
            retired_pool: objects_on(&mem, fork::RETIRED_POOL).map_err(&q)?,
            fresh_retired_pool: fresh_objects(adopters, fork::RETIRED_POOL),
            dropped_spends: objects_on(&mem, fork::DROPPED_SPENDS).map_err(&q)?,
            contacts_with_column,
            contacts_fresh: contacts_fresh.as_deref().map(normalize_sql),
            contacts_others: fresh_others(adopters, fork::CONTACTS),
            instant_locks_v2,
            instant_locks_fresh: instant_fresh.as_deref().map(normalize_sql),
            instant_locks_others: fresh_others(adopters, fork::INSTANT_LOCKS),
        })
    }

    /// The fresh `core_instant_locks` is the fork V2 shape exactly (P4 adds
    /// the fork's own column text).
    pub(crate) fn fresh_instant_locks_has_fork_verified(&self) -> bool {
        self.instant_locks_v2.is_some() && self.instant_locks_v2 == self.instant_locks_fresh
    }

    /// The contacts table after R1's drop is the fresh table exactly.
    pub(crate) fn contacts_is_fresh(&self, conn: &Connection) -> Result<bool> {
        let found = table_sql(conn, fork::CONTACTS).map_err(map_sqlite("read contacts"))?;
        Ok(found.is_some() && found == self.contacts_fresh)
    }
}

/// The first fork object of `conn` that is not exactly the reference shape,
/// or a fork table present without its history version. `None` when all
/// match. `p4_applied`: upstream's P4 already ran on this file, so a
/// `verified` column without fork V2 is upstream's own.
pub(crate) fn check(
    conn: &Connection,
    versions: &BTreeSet<u32>,
    reference: &Reference,
    p4_applied: bool,
) -> Result<Option<String>> {
    let q = map_sqlite("read schema");
    if reference
        .fresh_retired_pool
        .as_ref()
        .is_some_and(|fresh| fresh != &reference.retired_pool)
    {
        return Ok(Some(format!(
            "this build's {} (differs from fork version 1)",
            fork::RETIRED_POOL
        )));
    }
    if objects_on(conn, fork::RETIRED_POOL).map_err(&q)? != reference.retired_pool {
        return Ok(Some(fork::RETIRED_POOL.into()));
    }
    let contacts = table_sql(conn, fork::CONTACTS).map_err(&q)?;
    let contacts_ok = contacts.is_some()
        && (contacts == reference.contacts_with_column || contacts == reference.contacts_fresh);
    if !contacts_ok {
        return Ok(Some(format!("{} table", fork::CONTACTS)));
    }
    if without_table(objects_on(conn, fork::CONTACTS).map_err(&q)?) != reference.contacts_others {
        return Ok(Some(format!("{} indexes and triggers", fork::CONTACTS)));
    }
    let mut refs = conn
        .prepare(
            "SELECT type, name FROM sqlite_master WHERE sql LIKE '%external\\_account\\_reference%' \
             ESCAPE '\\' AND NOT (type = 'table' AND name = 'contacts') ORDER BY type, name",
        )
        .map_err(&q)?;
    let referrers: Vec<String> = refs
        .query_map([], |r| {
            Ok(format!(
                "{} {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?
            ))
        })
        .map_err(&q)?
        .collect::<rusqlite::Result<_>>()
        .map_err(&q)?;
    if let Some(first) = referrers.first() {
        return Ok(Some(format!(
            "{}.{} is referenced by {first}",
            fork::CONTACTS,
            fork::EXTERNAL_REF
        )));
    }
    if versions.contains(&2) {
        let found = table_sql(conn, fork::INSTANT_LOCKS).map_err(&q)?;
        let others = without_table(objects_on(conn, fork::INSTANT_LOCKS).map_err(&q)?);
        if found.is_none()
            || found != reference.instant_locks_v2
            || others != reference.instant_locks_others
        {
            return Ok(Some(format!("{}.{}", fork::INSTANT_LOCKS, fork::VERIFIED)));
        }
    } else if !p4_applied && has_column(conn, fork::INSTANT_LOCKS, fork::VERIFIED).map_err(&q)? {
        return Ok(Some(format!(
            "{}.{} without fork version 2",
            fork::INSTANT_LOCKS,
            fork::VERIFIED
        )));
    }
    if versions.contains(&3) {
        if objects_on(conn, fork::DROPPED_SPENDS).map_err(&q)? != reference.dropped_spends {
            return Ok(Some(fork::DROPPED_SPENDS.into()));
        }
    } else if has_table(conn, fork::DROPPED_SPENDS).map_err(&q)? {
        return Ok(Some(format!(
            "{} without fork version 3",
            fork::DROPPED_SPENDS
        )));
    }
    for (version, table) in fork::UNRELEASED_TABLES {
        if !versions.contains(version) && has_table(conn, table).map_err(&q)? {
            return Ok(Some(format!("{table} without fork version {version}")));
        }
    }
    Ok(None)
}
