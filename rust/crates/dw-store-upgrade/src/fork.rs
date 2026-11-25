//! What the fork builds wrote: their history table, the three migrations
//! every published pin carries (SQL copied verbatim from fork c5917e6bd5,
//! byte-identical at all 17 pins), and the accepted version sets (Q78 R4).

use std::collections::BTreeSet;

pub(crate) const FORK_HISTORY: &str = "dw_fork_schema_history";
pub(crate) const FORK_HISTORY_RETIRED: &str = "dw_fork_schema_history_retired";

/// refinery's history DDL as the fork wrote it.
pub(crate) const FORK_HISTORY_SQL: &str = "CREATE TABLE dw_fork_schema_history(\
     version int4 PRIMARY KEY, name VARCHAR(255), applied_on VARCHAR(255), checksum VARCHAR(255))";

pub(crate) const V1_SQL: &str =
    include_str!("../data/fork-c5917e6bd5/V1__dashpay_external_rotation.sql");
pub(crate) const V2_SQL: &str =
    include_str!("../data/fork-c5917e6bd5/V2__verified_instant_locks.sql");
pub(crate) const V3_SQL: &str =
    include_str!("../data/fork-c5917e6bd5/V3__dropped_unsigned_spends_recovery_record.sql");

pub(crate) const RETIRED_POOL: &str = "core_address_pool_retired";
pub(crate) const DROPPED_SPENDS: &str = "core_dropped_unsigned_spends";
pub(crate) const CONTACTS: &str = "contacts";
pub(crate) const EXTERNAL_REF: &str = "external_account_reference";
pub(crate) const INSTANT_LOCKS: &str = "core_instant_locks";
pub(crate) const VERIFIED: &str = "verified";

/// Tables later fork versions create (dev builds only; never released).
pub(crate) const UNRELEASED_TABLES: &[(u32, &str)] =
    &[(4, "core_dismissed_txids"), (5, "core_block_spends")];

/// The name the fork recorded in `dw_fork_schema_history`, and its checksum.
pub(crate) struct KnownForkMigration {
    pub version: u32,
    pub name: &'static str,
    pub checksum: &'static str,
}

pub(crate) const KNOWN: &[KnownForkMigration] = &[
    KnownForkMigration {
        version: 1,
        name: "dashpay_external_rotation",
        checksum: "13037568497809321770",
    },
    KnownForkMigration {
        version: 2,
        name: "verified_instant_locks",
        checksum: "7947297170294142560",
    },
    KnownForkMigration {
        version: 3,
        name: "dropped_unsigned_spends_recovery_record",
        checksum: "14354827916965344210",
    },
];

/// Names builds before the fork history table recorded fork V1 under, as
/// refinery version 19. Such a file is refused (R4).
pub(crate) const PRE_HISTORY_V19_NAMES: &[&str] = &[
    "dashpay_external_rotation",
    "contact_external_account_reference",
];

pub(crate) const PUBLISHED_PINS: &str = include_str!("../data/published-pins.md");

/// The fork version sets of every published desktop build, read from
/// `data/published-pins.md` (Q78 R4: the accepted sets come from that table).
pub(crate) fn published_sets() -> BTreeSet<BTreeSet<u32>> {
    PUBLISHED_PINS
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let pin = cells.get(1)?;
            if pin.len() != 40 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let set = cells.get(4)?.strip_prefix('{')?.strip_suffix('}')?;
            set.split(',').map(|v| v.trim().parse().ok()).collect()
        })
        .collect()
}

/// The pin rows of `data/published-pins.md`.
#[cfg(test)]
pub(crate) fn published_pin_count() -> usize {
    PUBLISHED_PINS
        .lines()
        .filter(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            cells
                .get(1)
                .is_some_and(|p| p.len() == 40 && p.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        .count()
}
