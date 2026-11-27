//! Upstream's own tables of the unreleased fork tables' names, pinned as
//! written by platform 3ab0f0f54c48 (V025, V026), byte-identical at
//! 7f0a1b0c378c. `shapes::check` accepts
//! such a table only when it is this build's fresh one, so a repin that
//! changes either shape fails here and must bring a version-aware comparison.

use std::collections::BTreeSet;

use super::*;
use crate::fork;
use crate::sql::{SchemaObject, normalize_sql, objects_on};

type Pinned = (&'static str, &'static str, Option<&'static str>);

const PINNED: &[(&str, &[Pinned])] = &[
    (
        "core_block_spends",
        &[
            (
                "index",
                "idx_core_block_spends_height",
                Some(
                    "CREATE INDEX idx_core_block_spends_height ON core_block_spends(wallet_id,height)",
                ),
            ),
            ("index", "sqlite_autoindex_core_block_spends_1", None),
            (
                "table",
                "core_block_spends",
                Some(
                    "CREATE TABLE core_block_spends(wallet_id BLOB NOT NULL,outpoint BLOB NOT NULL,\
                     height INTEGER NOT NULL,block_hash BLOB NOT NULL,spender_txid BLOB NOT NULL,\
                     PRIMARY KEY(wallet_id,outpoint),\
                     FOREIGN KEY(wallet_id)REFERENCES wallets(wallet_id)ON DELETE CASCADE)",
                ),
            ),
        ],
    ),
    (
        "core_dismissed_txids",
        &[
            ("index", "sqlite_autoindex_core_dismissed_txids_1", None),
            (
                "table",
                "core_dismissed_txids",
                Some(
                    "CREATE TABLE core_dismissed_txids(id INTEGER PRIMARY KEY,\
                     wallet_id BLOB NOT NULL,txid BLOB NOT NULL,\
                     abandoned INTEGER NOT NULL DEFAULT 0 CHECK(abandoned IN(0,1)),\
                     UNIQUE(wallet_id,txid,abandoned),\
                     FOREIGN KEY(wallet_id)REFERENCES wallets(wallet_id)ON DELETE CASCADE)",
                ),
            ),
        ],
    ),
];

#[test]
fn upstreams_block_spends_and_dismissed_txids_are_the_pinned_shapes() {
    assert_eq!(
        PINNED.iter().map(|(t, _)| *t).collect::<BTreeSet<_>>(),
        fork::UNRELEASED_TABLES
            .iter()
            .map(|(_, t, _)| *t)
            .collect::<BTreeSet<_>>(),
    );
    let (_dir, fresh) = fresh_store();
    let conn = Connection::open(&fresh).unwrap();
    for (table, objects) in PINNED {
        let want: Vec<SchemaObject> = objects
            .iter()
            .map(|(kind, name, sql)| (kind.to_string(), name.to_string(), sql.map(str::to_owned)))
            .collect();
        assert_eq!(
            objects_on(&conn, table).unwrap(),
            want,
            "{table}, fresh store"
        );
        let mut probed: Vec<SchemaObject> = build()
            .fresh
            .iter()
            .filter(|o| o.table == *table)
            .map(|o| {
                (
                    o.kind.clone(),
                    o.name.clone(),
                    o.sql.as_deref().map(normalize_sql),
                )
            })
            .collect();
        probed.sort();
        assert_eq!(probed, want, "{table}, this build's probe");
    }
}
